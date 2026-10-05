use anyhow::{Context, Result, bail};

use crate::error::GitError;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A single entry of `git worktree list --porcelain`.
#[derive(Debug, Clone)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    pub head: String,
    /// Short branch name, e.g. `main`. `None` when the worktree is detached.
    pub branch: Option<String>,
    pub detached: bool,
    pub prunable: bool,
}

impl WorktreeEntry {
    /// Short (7 char) commit id, useful as a fallback name for detached worktrees.
    pub fn short_head(&self) -> String {
        self.head.chars().take(7).collect()
    }
}

/// Run a git command, returning stdout. Bails with git's stderr on failure.
///
/// A missing git is reported separately from a failing command; otherwise a
/// missing binary surfaces as "not inside a git repository", which blames the
/// user's directory for a problem with their PATH.
fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                GitError::ToolMissing.into()
            } else {
                anyhow::Error::new(e).context(format!("Failed to execute git {}", args.join(" ")))
            }
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(GitError::Command {
            args: args.join(" "),
            stderr: stderr.trim().to_string(),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Run a git command, returning `None` instead of erroring on failure.
fn git_quiet(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Like `git_quiet`, but a missing binary is reported rather than folded into
/// `None`, which would otherwise read as "not a repository".
fn git_probe(cwd: &Path, args: &[&str]) -> Result<Option<String>> {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                GitError::ToolMissing.into()
            } else {
                anyhow::Error::new(e).context(format!("Failed to execute git {}", args.join(" ")))
            }
        })?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&output.stdout).to_string()))
}

/// `Err` carries a missing git; `Ok(false)` means the directory is not in a
/// worktree.
pub fn is_inside_work_tree(cwd: &Path) -> Result<bool> {
    Ok(git_probe(cwd, &["rev-parse", "--is-inside-work-tree"])?
        .map(|s| s.trim() == "true")
        .unwrap_or(false))
}

/// Absolute path of the worktree root containing `cwd`.
pub fn toplevel(cwd: &Path) -> Result<PathBuf> {
    let out = git(cwd, &["rev-parse", "--show-toplevel"])?;
    let path = PathBuf::from(out.trim());
    if path.as_os_str().is_empty() {
        bail!("Failed to determine the worktree root");
    }
    Ok(path)
}

/// Current branch name. Fails when HEAD is detached.
pub fn symbolic_branch(cwd: &Path) -> Result<String> {
    match git(cwd, &["symbolic-ref", "--short", "HEAD"]) {
        Ok(out) => {
            let name = out.trim().to_string();
            if name.is_empty() {
                bail!("Failed to determine the current branch");
            }
            Ok(name)
        }
        Err(e) if is_detached_head(&e) => bail!(GitError::DetachedHead),
        // A missing binary or a damaged repository is not a detached HEAD, and
        // telling the user to run `git switch` would send them the wrong way.
        Err(e) => Err(e),
    }
}

/// Whether git refused because HEAD is a raw commit id rather than a ref.
fn is_detached_head(err: &anyhow::Error) -> bool {
    match err.downcast_ref::<GitError>() {
        Some(GitError::Command { stderr, .. }) => stderr.contains("not a symbolic ref"),
        _ => false,
    }
}

pub fn branch_exists(cwd: &Path, branch: &str) -> bool {
    git_quiet(
        cwd,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{}", branch),
        ],
    )
    .is_some()
}

pub fn remotes(cwd: &Path) -> Result<Vec<String>> {
    let out = git(cwd, &["remote"])?;
    Ok(out
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

pub fn remote_get_urls(cwd: &Path, remote: &str, push: bool) -> Result<Vec<String>> {
    let mut args = vec!["remote", "get-url", "--all"];
    if push {
        args.push("--push");
    }
    args.push(remote);
    let out = git(cwd, &args)?;
    Ok(out
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

pub fn remote_set_url(
    cwd: &Path,
    remote: &str,
    new_url: &str,
    old_url: &str,
    push: bool,
) -> Result<()> {
    let mut args = vec!["remote", "set-url"];
    if push {
        args.push("--push");
    }
    args.push(remote);
    args.push(new_url);
    args.push(old_url);
    git(cwd, &args)?;
    Ok(())
}

/// True when a remote URL points at a filesystem path relative to the repository.
///
/// Such URLs are resolved against the repository's own location, so they break
/// as soon as the repository directory is moved. Absolute paths, `~` paths,
/// `scheme://` URLs and scp-like `git@host:path` URLs are all left alone.
pub fn is_local_relative_url(url: &str) -> bool {
    if url.starts_with('/') || url.starts_with('~') {
        return false;
    }
    if url.contains("://") {
        return false;
    }
    match (url.find(':'), url.find('/')) {
        // A colon before the first slash means scp-like syntax, not a path.
        (Some(colon), Some(slash)) => colon >= slash,
        (Some(_), None) => false,
        _ => true,
    }
}

/// Turn a repository-relative remote URL into an absolute one, resolved against `repo`.
pub fn absolutize_remote_url(repo: &Path, url: &str) -> Result<String> {
    if !is_local_relative_url(url) {
        return Ok(url.to_string());
    }
    let joined = repo.join(url);
    let canonical = joined
        .canonicalize()
        .with_context(|| format!("Failed to resolve remote path '{}'", joined.display()))?;
    Ok(canonical.to_string_lossy().to_string())
}

/// Rewrite every repository-relative remote URL to an absolute path.
///
/// Must be called *before* the repository directory is relocated, otherwise the
/// relative URLs silently start resolving against the new location and pushes
/// break. Returns the list of rewrites performed, for user-facing reporting.
pub fn normalize_remote_urls(repo: &Path) -> Result<Vec<(String, String, String)>> {
    let mut rewrites = Vec::new();

    for remote in remotes(repo)? {
        for push in [false, true] {
            for url in remote_get_urls(repo, &remote, push)? {
                if !is_local_relative_url(&url) {
                    continue;
                }
                let absolute = absolutize_remote_url(repo, &url)?;
                if absolute == url {
                    continue;
                }
                remote_set_url(repo, &remote, &absolute, &url, push)?;
                rewrites.push((remote.clone(), url.clone(), absolute));
            }
        }
    }

    Ok(rewrites)
}

/// Parse `git worktree list --porcelain` output.
pub fn parse_worktree_list(porcelain: &str) -> Vec<WorktreeEntry> {
    let mut entries: Vec<WorktreeEntry> = Vec::new();

    for line in porcelain.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }

        let (key, value) = match line.split_once(' ') {
            Some((k, v)) => (k, v),
            None => (line, ""),
        };

        match key {
            "worktree" => {
                entries.push(WorktreeEntry {
                    path: PathBuf::from(value),
                    head: String::new(),
                    branch: None,
                    detached: false,
                    prunable: false,
                });
            }
            "HEAD" => {
                if let Some(last) = entries.last_mut() {
                    last.head = value.to_string();
                }
            }
            "branch" => {
                if let Some(last) = entries.last_mut() {
                    last.branch = Some(
                        value
                            .strip_prefix("refs/heads/")
                            .unwrap_or(value)
                            .to_string(),
                    );
                }
            }
            "detached" => {
                if let Some(last) = entries.last_mut() {
                    last.detached = true;
                }
            }
            "prunable" => {
                if let Some(last) = entries.last_mut() {
                    last.prunable = true;
                }
            }
            _ => {}
        }
    }

    entries
}

pub fn worktree_list(cwd: &Path) -> Result<Vec<WorktreeEntry>> {
    let out = git(cwd, &["worktree", "list", "--porcelain"])?;
    Ok(parse_worktree_list(&out))
}

/// Create a worktree. Creates the branch when it does not exist yet.
pub fn worktree_add(
    repo: &Path,
    path: &Path,
    branch: &str,
    start_point: Option<&str>,
) -> Result<()> {
    let mut args: Vec<String> = vec!["worktree".into(), "add".into()];

    if branch_exists(repo, branch) {
        args.push(path.to_string_lossy().to_string());
        args.push(branch.to_string());
    } else {
        args.push("-b".into());
        args.push(branch.to_string());
        args.push(path.to_string_lossy().to_string());
        if let Some(start) = start_point {
            args.push(start.to_string());
        }
    }

    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    git(repo, &refs).map(|_| ())
}

pub fn worktree_remove(repo: &Path, path: &Path, force: bool) -> Result<()> {
    let path_str = path.to_string_lossy().to_string();
    if force {
        git(repo, &["worktree", "remove", "--force", &path_str]).map(|_| ())
    } else {
        git(repo, &["worktree", "remove", &path_str]).map(|_| ())
    }
}

/// Repair the `.git` pointer files of the given worktrees.
///
/// The paths must be passed explicitly: `git worktree repair` without arguments
/// only inspects worktrees it already knows about at their recorded locations,
/// so it silently does nothing for worktrees that have just been moved.
pub fn worktree_repair(repo: &Path, paths: &[PathBuf]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }

    let mut args: Vec<String> = vec!["worktree".into(), "repair".into()];
    for p in paths {
        args.push(p.to_string_lossy().to_string());
    }

    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    // Repair reports recoverable oddities on stderr while still succeeding, so we
    // only fail when git actually returns a non-zero status.
    git(repo, &refs).map(|_| ())
}

pub fn worktree_prune(repo: &Path) -> Result<()> {
    git(repo, &["worktree", "prune"]).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_local_relative_url_detection() {
        // Repository-relative paths: these break when the repo directory moves.
        assert!(is_local_relative_url("../origin.git"));
        assert!(is_local_relative_url("./sibling.git"));
        assert!(is_local_relative_url("origin.git"));
        assert!(is_local_relative_url("sub/dir.git"));

        // Absolute / scheme / scp-like: safe to leave untouched.
        assert!(!is_local_relative_url("/srv/git/origin.git"));
        assert!(!is_local_relative_url("~/repos/origin.git"));
        assert!(!is_local_relative_url(
            "git@github.com:niniconi/agentdock.git"
        ));
        assert!(!is_local_relative_url("ssh://git@github.com/org/repo.git"));
        assert!(!is_local_relative_url("https://github.com/org/repo.git"));
        assert!(!is_local_relative_url("file:///srv/git/origin.git"));
    }

    #[test]
    fn test_parse_worktree_list() {
        let porcelain = "worktree /repo/repo-main\n\
                          HEAD 864399d1cb1e2f3a\n\
                          branch refs/heads/main\n\
                          \n\
                          worktree /repo/repo-dev\n\
                          HEAD 864399d1cb1e2f3a\n\
                          branch refs/heads/dev\n\
                          \n\
                          worktree /repo/repo-loose\n\
                          HEAD 864399d1cb1e2f3a\n\
                          detached\n";

        let entries = parse_worktree_list(porcelain);
        assert_eq!(entries.len(), 3);

        assert_eq!(entries[0].path, PathBuf::from("/repo/repo-main"));
        assert_eq!(entries[0].branch.as_deref(), Some("main"));
        assert!(!entries[0].detached);
        assert_eq!(entries[0].short_head(), "864399d");

        assert_eq!(entries[1].branch.as_deref(), Some("dev"));

        assert!(entries[2].detached);
        assert_eq!(entries[2].branch, None);
    }

    #[test]
    fn test_parse_worktree_list_marks_prunable() {
        let porcelain = "worktree /repo/gone\n\
                          HEAD 864399d1cb1e2f3a\n\
                          branch refs/heads/gone\n\
                          prunable gitdir file points to non-existent location\n";
        let entries = parse_worktree_list(porcelain);
        assert!(entries[0].prunable);
    }
}
