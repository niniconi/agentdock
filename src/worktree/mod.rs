pub mod git;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::cli::{WorktreeAddArgs, WorktreeListArgs, WorktreeRmArgs};
use crate::error::{ContainerError, WorktreeError, format_conflicts};
use crate::state::StateManager;

const MARKER_FILE: &str = ".agentdock.json";
const MARKER_VERSION: u32 = 1;

/// Metadata written into the container directory so it can be recognised even
/// though the container itself is not a git repository.
#[derive(Debug, Serialize, Deserialize)]
pub struct Marker {
    pub version: u32,
    /// Base name of the original repository, e.g. `myproject`.
    pub repo: String,
    /// Directory name of the main worktree, e.g. `myproject-main`.
    pub main: String,
}

/// Turn a branch name into a flat directory name.
///
/// `feature/login` becomes `feature-login` so worktrees stay siblings in one
/// directory instead of nesting.
pub fn flatten(branch: &str) -> String {
    branch
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | ' ' => '-',
            other => other,
        })
        .collect()
}

fn marker_path(container: &Path) -> PathBuf {
    container.join(MARKER_FILE)
}

pub fn read_marker(container: &Path) -> Result<Marker> {
    let path = marker_path(container);
    let data =
        fs::read_to_string(&path).with_context(|| format!("Failed to read {}", path.display()))?;
    serde_json::from_str(&data).with_context(|| format!("Failed to parse {}", path.display()))
}

fn write_marker(container: &Path, marker: &Marker) -> Result<()> {
    let data =
        serde_json::to_string_pretty(marker).context("Failed to serialize worktree metadata")?;
    fs::write(marker_path(container), data).context("Failed to write worktree metadata")
}

/// Read the marker, explaining a missing one in terms the user can act on
/// instead of exposing the marker filename.
fn read_marker_or_explain(container: &Path) -> Result<Marker> {
    if !marker_path(container).is_file() {
        bail!(WorktreeError::NotInWorktreeProject {
            path: container.to_path_buf(),
        });
    }
    read_marker(container)
}

/// Locate the container directory from any path inside the project.
///
/// Walks upwards looking for the marker file, so it works from the container
/// itself, from inside a worktree, and from any nested subdirectory. Falls back
/// to the parent of the nearest worktree root when no marker is present.
pub fn resolve_container(start: &Path) -> Result<PathBuf> {
    let mut cursor = Some(start);
    let mut worktree_root: Option<PathBuf> = None;

    while let Some(dir) = cursor {
        if marker_path(dir).is_file() {
            return Ok(dir.to_path_buf());
        }
        if worktree_root.is_none() && (dir.join(".git").is_dir() || dir.join(".git").is_file()) {
            worktree_root = Some(dir.to_path_buf());
        }
        cursor = dir.parent();
    }

    if let Some(parent) = worktree_root.as_ref().and_then(|r| r.parent()) {
        return Ok(parent.to_path_buf());
    }

    bail!(WorktreeError::NotInWorktreeProject {
        path: start.to_path_buf(),
    })
}

/// Absolute path of the main worktree, used as the git dir for all operations.
fn main_worktree(container: &Path, marker: &Marker) -> Result<PathBuf> {
    let main = container.join(&marker.main);
    if !main.join(".git").exists() {
        bail!(WorktreeError::MainWorktreeMissing {
            path: main.to_path_buf(),
        });
    }
    Ok(main)
}

pub fn init() -> Result<()> {
    let cwd = std::env::current_dir().context("Failed to get current directory")?;

    if !git::is_inside_work_tree(&cwd)? {
        bail!(WorktreeError::NotAGitRepo {
            path: cwd.to_path_buf(),
        });
    }

    let toplevel = git::toplevel(&cwd)?
        .canonicalize()
        .context("Failed to resolve the worktree root")?;

    if marker_path(&toplevel).exists() {
        bail!(WorktreeError::AlreadyConverted {
            path: toplevel.to_path_buf(),
        });
    }
    if toplevel.parent().is_some_and(|p| marker_path(p).exists()) {
        bail!(WorktreeError::NestedProject {
            path: toplevel.to_path_buf(),
        });
    }
    if toplevel.parent().is_none() {
        bail!(WorktreeError::NoParentDir {
            path: toplevel.to_path_buf(),
        });
    }

    // Refuse when agentdock containers are already mounted inside this project:
    // their records point at the pre-move paths and would silently go stale.
    let state = StateManager::new()?;
    let conflicts = state.find_within(&toplevel);
    if !conflicts.is_empty() {
        bail!(WorktreeError::ContainerConflict {
            path: toplevel.to_path_buf(),
            conflicts: format_conflicts(&conflicts),
        });
    }

    let branch = git::symbolic_branch(&toplevel)?;
    let repo_name = toplevel
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "repo".to_string());
    let main_name = format!("{}-{}", repo_name, flatten(&branch));
    let target = toplevel.join(&main_name);

    if target.exists() {
        bail!(WorktreeError::TargetExists {
            path: target.to_path_buf(),
        });
    }

    // Snapshot the existing linked worktrees before anything moves.
    let entries = git::worktree_list(&toplevel)?;
    let linked: Vec<git::WorktreeEntry> =
        entries.into_iter().filter(|e| e.path != toplevel).collect();

    // Work out where every existing worktree will land, and refuse up front if
    // any destination is taken, so a failure cannot leave a half-moved project.
    let mut moves: Vec<(PathBuf, PathBuf, String)> = Vec::new();
    let mut planned: HashSet<PathBuf> = HashSet::new();

    for entry in &linked {
        if !entry.path.exists() {
            continue;
        }
        // A worktree nested inside the repository cannot be relocated: its path
        // moves together with the repository, so the source stops existing the
        // moment the main worktree is renamed.
        if entry.path.starts_with(&toplevel) {
            bail!(WorktreeError::WorktreeInsideRepo {
                path: entry.path.to_path_buf(),
            });
        }

        let label = match &entry.branch {
            Some(b) => b.clone(),
            None => entry.short_head(),
        };
        let dir = format!("{}-{}", repo_name, flatten(&label));
        let dest = toplevel.join(&dir);

        if dest == target {
            bail!(WorktreeError::DestinationCollision {
                path: dest,
                taken_by: "the main worktree".to_string(),
            });
        }
        // Distinct branches can flatten to the same directory name, e.g. `a/b`
        // and `a-b`. Neither destination exists yet, so only comparing against
        // what is already on disk would miss the clash.
        if !planned.insert(dest.clone()) {
            bail!(WorktreeError::DestinationCollision {
                path: dest,
                taken_by: format!("another branch that also maps to '{}'", dir),
            });
        }
        if dest.exists() {
            bail!(WorktreeError::DestinationCollision {
                path: dest,
                taken_by: format!("branch '{}'", label),
            });
        }

        moves.push((entry.path.clone(), dest, label));
    }

    let stage = stage_path(&toplevel, &repo_name);
    if stage.exists() {
        bail!(WorktreeError::StaleStageDir { path: stage });
    }

    println!("Converting project into worktree form...");
    println!("  repository : {}", repo_name);
    println!("  branch     : {}", branch);
    println!("  main       : {}", main_name);

    // Relative remote URLs resolve against the repository's own location, so
    // they must be made absolute *before* the directory moves.
    let rewrites = git::normalize_remote_urls(&toplevel)?;
    for (remote, from, to) in &rewrites {
        println!("  remote     : {} -> {} (was {})", remote, to, from);
    }

    // Relocate the whole repository. `.git` travels with the files, which keeps
    // the working tree, the index and untracked files untouched.
    fs::rename(&toplevel, &stage)
        .with_context(|| format!("Failed to stage {}", toplevel.display()))?;
    if let Err(err) = fs::create_dir(&toplevel) {
        // The repository is parked at the staging path now, so put it back
        // before returning; otherwise the project directory would be gone.
        let _ = fs::rename(&stage, &toplevel);
        return Err(err)
            .with_context(|| format!("Failed to recreate {} while staging", toplevel.display()));
    }
    if let Err(err) = fs::rename(&stage, &target) {
        // Put the project back where it was rather than leaving it stranded.
        let _ = fs::rename(&stage, &toplevel);
        let _ = fs::remove_dir(&toplevel);
        return Err(err)
            .with_context(|| format!("Failed to move repository to {}", target.display()));
    }

    // Bring pre-existing worktrees into the container, then repair their `.git`
    // pointers, which hold absolute paths to their old locations.
    let mut moved = Vec::new();
    for (source, dest, _) in &moves {
        if let Err(err) = fs::rename(source, dest) {
            println!(
                "  warning    : could not move {} into the container ({})",
                source.display(),
                err
            );
            continue;
        }
        moved.push(dest.clone());
    }

    if let Err(err) = git::worktree_repair(&target, &moved) {
        println!("  warning    : git worktree repair reported: {}", err);
    }
    let _ = git::worktree_prune(&target);

    let marker = Marker {
        version: MARKER_VERSION,
        repo: repo_name.clone(),
        main: main_name.clone(),
    };
    write_marker(&toplevel, &marker)?;

    println!();
    println!("Project converted. Layout:");
    println!("  {}", toplevel.display());
    println!("  ├── {}/", main_name);
    for (_, dest, _) in &moves {
        println!("  └── {}/", dest.file_name().unwrap().to_string_lossy());
    }
    println!();
    println!("Next steps:");
    println!("  cd {}/{}", toplevel.display(), main_name);
    println!("  agentdock worktree add <branch> --run   # spin up another agent");

    Ok(())
}

/// A hidden sibling of the repository used as a staging area during the move.
fn stage_path(toplevel: &Path, repo_name: &str) -> PathBuf {
    toplevel.with_file_name(format!(".{}.agentdock-stage", repo_name))
}

pub fn add(args: &WorktreeAddArgs) -> Result<()> {
    let cwd = std::env::current_dir().context("Failed to get current directory")?;
    let container = resolve_container(&cwd)?;
    let marker = read_marker_or_explain(&container)?;
    let main = main_worktree(&container, &marker)?;

    if git::branch_exists(&main, &args.branch) && args.start_point.is_some() {
        bail!(WorktreeError::ExistingBranchWithStartPoint {
            branch: args.branch.clone(),
        });
    }

    // Report an occupied branch before the path check, otherwise adding the
    // branch that the main worktree is on fails with a confusing message about
    // an existing directory.
    let existing = git::worktree_list(&main)?;
    if let Some(entry) = find_worktree(&existing, &main, &args.branch, Match::Strict)
        && entry.branch.is_some()
    {
        bail!(WorktreeError::BranchInUse {
            branch: args.branch.clone(),
            path: entry.path,
        });
    }

    let dir_name = format!("{}-{}", marker.repo, flatten(&args.branch));
    let path = container.join(&dir_name);
    if path.exists() {
        // A detached worktree was named after its commit, so it can occupy a
        // path a new branch would want. Say so, rather than leaving the user
        // with a bare "directory already exists" and no hint why.
        if let Some(entry) = find_worktree(&existing, &main, &args.branch, Match::Loose)
            && entry.branch.is_none()
        {
            bail!(WorktreeError::NameTakenByDetached {
                name: args.branch.clone(),
                path: entry.path,
            });
        }
        bail!(WorktreeError::TargetExists { path });
    }

    println!("Creating worktree for branch '{}'...", args.branch);
    git::worktree_add(&main, &path, &args.branch, args.start_point.as_deref())
        .map_err(|err| annotate_add_failure(err, &main, &args.branch))?;

    println!("Worktree created: {}", path.display());

    // TODO: the container options are flattened onto `add` unconditionally, so
    // `worktree add dev -a some/image` without `--apply` parses fine and is then
    // discarded here without a word. Either reject options when `apply` is
    // unset, or warn that they were ignored.
    if args.apply {
        // The worktree directory doubles as the mount path, so the container is
        // bound to this worktree and stays independent from the others.
        let apply_args = crate::cli::ApplyArgs {
            path: Some(path.clone()),
            name: Some(dir_name.clone()),
            opts: args.opts.clone(),
        };
        crate::commands::execute_apply(apply_args)?;
    }

    Ok(())
}

/// Git's own "already checked out" message is opaque, so name the offending
/// worktree when the branch is already in use somewhere.
fn annotate_add_failure(err: anyhow::Error, main: &Path, branch: &str) -> anyhow::Error {
    let text = err.to_string();
    if !text.contains("already checked out") && !text.contains("already used by worktree") {
        return err;
    }

    let entries = git::worktree_list(main).unwrap_or_default();
    for entry in entries {
        if entry.branch.as_deref() == Some(branch) {
            return WorktreeError::BranchInUse {
                branch: branch.to_string(),
                path: entry.path,
            }
            .into();
        }
    }
    err
}

pub fn list(args: &WorktreeListArgs) -> Result<()> {
    let cwd = std::env::current_dir().context("Failed to get current directory")?;
    let container = resolve_container(&cwd)?;
    let marker = read_marker_or_explain(&container)?;
    let main = main_worktree(&container, &marker)?;

    let state = StateManager::new()?;
    let entries = git::worktree_list(&main)?;

    let mut rows: Vec<Vec<String>> = Vec::new();
    for entry in &entries {
        let is_main = entry.path == main;
        // A detached worktree is named after its short commit id, so the BRANCH
        // column shows that name: it is what `worktree rm` accepts.
        let branch = entry.branch.clone().unwrap_or_else(|| entry.short_head());

        let container_name = state
            .find_by_path(&entry.path)
            .map(|(name, _)| name.to_string())
            .unwrap_or_else(|| "-".to_string());

        let role = if is_main {
            "main".to_string()
        } else if entry.prunable {
            // The directory is gone but git still tracks the registration.
            "stale".to_string()
        } else {
            "worktree".to_string()
        };

        rows.push(vec![
            role,
            branch,
            entry.path.display().to_string(),
            entry.head.chars().take(7).collect(),
            container_name,
        ]);
    }

    if args.format == "json" {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    crate::commands::print_table(&["ROLE", "BRANCH", "PATH", "HEAD", "CONTAINER"], &rows);
    Ok(())
}

/// Resolve the worktree that `rm`/`add` should act on.
///
/// Which names a worktree can be addressed by.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Match {
    /// Branch name only.
    ///
    /// `add` names a branch it is about to create, so accepting a detached
    /// worktree's directory name here would reject a perfectly valid branch
    /// such as `p-<sha>`, whose own directory would be `p-p-<sha>` and cannot
    /// collide.
    Strict,
    /// Branch name, then the identifiers a detached worktree is known by.
    ///
    /// `rm` addresses something that already exists, so a user reading a
    /// directory name out of `worktree list` should be able to pass it.
    Loose,
}

/// Resolve the worktree `name` refers to.
///
/// A detached worktree has no branch, so `rm` also accepts the short commit id
/// that `worktree list` prints, the full commit id, and the directory name
/// `init` assigned (`{repo}-{short-commit}`). Without those fallbacks a
/// detached worktree could be created by `init` but never removed through
/// agentdock, forcing a manual `git worktree remove` that bypasses the
/// container check.
fn find_worktree(
    entries: &[git::WorktreeEntry],
    main: &Path,
    name: &str,
    mode: Match,
) -> Option<git::WorktreeEntry> {
    if let Some(entry) = entries.iter().find(|e| e.branch.as_deref() == Some(name)) {
        return Some(entry.clone());
    }
    if mode == Match::Strict {
        return None;
    }

    let is_main = |e: &git::WorktreeEntry| e.path == main;

    entries
        .iter()
        .find(|e| !is_main(e) && e.branch.is_none() && e.short_head() == name)
        .or_else(|| {
            entries
                .iter()
                .find(|e| !is_main(e) && e.branch.is_none() && e.head == name)
        })
        .or_else(|| {
            entries.iter().find(|e| {
                !is_main(e)
                    && e.branch.is_none()
                    && e.path.file_name().map(|n| n.to_string_lossy().to_string())
                        == Some(name.to_string())
            })
        })
        .cloned()
}

pub fn rm(args: &WorktreeRmArgs) -> Result<()> {
    let cwd = std::env::current_dir().context("Failed to get current directory")?;
    let container = resolve_container(&cwd)?;
    let marker = read_marker_or_explain(&container)?;
    let main = main_worktree(&container, &marker)?;

    let entries = git::worktree_list(&main)?;
    let target = find_worktree(&entries, &main, &args.branch, Match::Loose).ok_or_else(|| {
        WorktreeError::WorktreeNotFound {
            branch: args.branch.clone(),
            container: container.clone(),
        }
    })?;

    if target.path == main {
        bail!(WorktreeError::CannotRemoveMain { path: target.path });
    }

    let mut state = StateManager::new()?;
    let attached = state
        .find_by_path(&target.path)
        .map(|(name, _)| name.to_string());

    if let Some(name) = attached {
        if !args.force {
            bail!(ContainerError::Attached {
                name,
                path: target.path,
            });
        }
        state.remove(&name);
        state.save()?;
        println!("Removed container record: {}", name);
    }

    git::worktree_remove(&main, &target.path, args.force)?;
    println!("Worktree removed: {}", target.path.display());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flatten_slashes() {
        assert_eq!(flatten("main"), "main");
        assert_eq!(flatten("feat/login"), "feat-login");
        assert_eq!(flatten("feature/a/b/c"), "feature-a-b-c");
    }

    #[test]
    fn test_flatten_stays_flat() {
        let flattened = flatten("feature/deeply/nested/branch");
        assert!(
            !flattened.contains('/'),
            "flattened name must not nest: {}",
            flattened
        );
    }

    #[test]
    fn test_flatten_sanitizes_path_hosts() {
        assert_eq!(flatten("a:b"), "a-b");
        assert_eq!(flatten("a b"), "a-b");
        assert_eq!(flatten("a*b?c"), "a-b-c");
    }
}
