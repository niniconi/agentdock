use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A throwaway directory tree that cleans itself up.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(label: &str) -> Option<Self> {
        if !git_available() {
            return None;
        }
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "agentdock-it-{}-{}-{}",
            label,
            std::process::id(),
            n
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create sandbox");
        // A private HOME keeps ~/.config/agentdock/records.json out of the way.
        std::fs::create_dir_all(root.join("home")).expect("create sandbox home");
        Some(Sandbox { root })
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Run git, panicking on failure, and return its trimmed stdout.
fn git(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn agentdock(cwd: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agentdock"))
        .current_dir(cwd)
        .args(args)
        .env("HOME", home)
        .output()
        .expect("run agentdock")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

/// Create a repo at `root/<name>` with one commit, and return its path.
fn make_repo(sandbox: &Sandbox, name: &str, branch: &str) -> PathBuf {
    let repo = sandbox.path(name);
    std::fs::create_dir_all(repo.join("src")).expect("create repo");
    std::fs::write(repo.join("src/lib.rs"), "code\n").expect("write source");
    git(&repo, &["init", "-q", "-b", branch]);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    repo
}

#[test]
fn init_produces_the_expected_layout() {
    let Some(sb) = Sandbox::new("layout") else {
        return;
    };
    let repo = make_repo(&sb, "myproject", "main");

    let out = agentdock(&repo, &sb.home(), &["worktree", "init"]);
    assert!(out.status.success(), "init failed: {}", stderr(&out));

    let main = sb.path("myproject/myproject-main");
    assert!(main.join(".git").exists(), "main worktree missing .git");
    assert!(main.join("src/lib.rs").exists(), "files did not move");
    assert!(
        sb.path("myproject/.agentdock.json").exists(),
        "marker not written"
    );

    // The container itself is not a git repository.
    assert!(
        !sb.path("myproject/.git").exists(),
        "container should not hold .git"
    );
}

#[test]
fn init_preserves_uncommitted_work() {
    let Some(sb) = Sandbox::new("dirty") else {
        return;
    };
    let repo = make_repo(&sb, "proj", "main");
    std::fs::write(repo.join("src/lib.rs"), "code\nmore\n").expect("modify");
    std::fs::write(repo.join("STAGED.md"), "staged\n").expect("new file");
    git(&repo, &["add", "STAGED.md"]);

    let out = agentdock(&repo, &sb.home(), &["worktree", "init"]);
    assert!(out.status.success(), "init failed: {}", stderr(&out));

    let main = sb.path("proj/proj-main");
    let text = git(&main, &["status", "--porcelain"]);
    assert!(
        text.contains(" M src/lib.rs"),
        "lost modification: {}",
        text
    );
    assert!(text.contains("A  STAGED.md"), "lost staged file: {}", text);
}

#[test]
fn init_rewrites_relative_remote_urls() {
    let Some(sb) = Sandbox::new("remote") else {
        return;
    };
    git(&sb.root, &["init", "-q", "--bare", "origin.git"]);

    let repo = make_repo(&sb, "proj", "main");
    git(&repo, &["remote", "add", "origin", "../origin.git"]);

    let out = agentdock(&repo, &sb.home(), &["worktree", "init"]);
    assert!(out.status.success(), "init failed: {}", stderr(&out));

    let main = sb.path("proj/proj-main");
    let url = git(&main, &["remote", "get-url", "origin"]);
    assert!(url.starts_with('/'), "remote url is not absolute: {}", url);

    // A relative url would resolve against the new location and break the push.
    git(&main, &["push", "-q", "origin", "main"]);
}

#[test]
fn init_refuses_when_branches_flatten_to_the_same_directory() {
    let Some(sb) = Sandbox::new("flatten") else {
        return;
    };
    let repo = make_repo(&sb, "p", "main");
    // `a/b` and `a-b` are distinct branches but flatten to the same name.
    git(&repo, &["worktree", "add", "-q", "-b", "a/b", "../w1"]);
    git(&repo, &["worktree", "add", "-q", "-b", "a-b", "../w2"]);

    let out = agentdock(&repo, &sb.home(), &["worktree", "init"]);
    assert!(
        !out.status.success(),
        "clash was accepted: {}",
        stdout(&out)
    );
    assert!(
        stderr(&out).contains("p-a-b"),
        "error should name the clashing dir: {}",
        stderr(&out)
    );
    // Refusing must leave the project exactly where it was.
    assert!(sb.path("p/src/lib.rs").exists(), "project was moved anyway");
    assert!(!sb.path("p/p-main").exists(), "conversion happened anyway");
}

#[test]
fn init_refuses_when_a_worktree_lives_inside_the_repository() {
    let Some(sb) = Sandbox::new("inner") else {
        return;
    };
    let repo = make_repo(&sb, "p", "main");
    git(
        &repo,
        &["worktree", "add", "-q", "-b", "inner", "./inner-wt"],
    );

    let out = agentdock(&repo, &sb.home(), &["worktree", "init"]);
    assert!(!out.status.success(), "accepted: {}", stdout(&out));
    assert!(
        stderr(&out).contains("inside the repository"),
        "unexpected message: {}",
        stderr(&out)
    );
    assert!(sb.path("p/src/lib.rs").exists(), "project was moved anyway");
    assert!(sb.path("p/inner-wt").exists(), "inner worktree was lost");
}

#[test]
fn init_refuses_when_container_records_exist_inside_the_project() {
    let Some(sb) = Sandbox::new("conflict") else {
        return;
    };
    let repo = make_repo(&sb, "proj", "main");
    std::fs::create_dir_all(repo.join("sub")).expect("create subdir");

    let records = sb.home().join(".config/agentdock");
    std::fs::create_dir_all(&records).expect("create records dir");
    std::fs::write(
        records.join("records.json"),
        format!(
            r#"{{"records":{{"in-sub":{{"path":"{}","created_at":"2026-01-01T00:00:00Z","docker_image":"img","agent_name":"agent","kvm":false}}}}}}"#,
            sb.path("proj/sub").display()
        ),
    )
    .expect("write records");

    let out = agentdock(&repo, &sb.home(), &["worktree", "init"]);
    assert!(!out.status.success(), "accepted: {}", stdout(&out));
    let err = stderr(&out);
    assert!(err.contains("in-sub"), "should list the record: {}", err);
    assert!(
        sb.path("proj/src/lib.rs").exists(),
        "project was moved anyway"
    );
}

#[test]
fn rm_removes_a_detached_worktree_by_its_short_commit() {
    let Some(sb) = Sandbox::new("rmdetached") else {
        return;
    };
    let repo = make_repo(&sb, "proj", "main");
    git(&repo, &["worktree", "add", "-q", "--detach", "../det"]);
    assert!(agentdock(&repo, &sb.home(), &["worktree", "init"])
        .status
        .success());

    let main = sb.path("proj/proj-main");
    // `list` names a detached worktree after its short commit, and `rm` must
    // accept exactly that; otherwise init can create one that agentdock can
    // never clean up.
    let listed = stdout(&agentdock(&main, &sb.home(), &["worktree", "list"]));
    let short = listed
        .lines()
        .filter(|l| l.starts_with("worktree"))
        .filter_map(|l| l.split_whitespace().nth(1))
        .next()
        .expect("detached worktree row")
        .to_string();
    assert_eq!(short.len(), 7, "expected a short commit id: {}", short);
    assert!(
        sb.path(&format!("proj/proj-{}", short)).exists(),
        "detached worktree was not moved into the container"
    );

    let out = agentdock(&main, &sb.home(), &["worktree", "rm", &short]);
    assert!(out.status.success(), "rm failed: {}", stderr(&out));
    assert!(
        !sb.path(&format!("proj/proj-{}", short)).exists(),
        "worktree directory survived"
    );
    let after = git(&main, &["worktree", "list", "--porcelain"]);
    assert_eq!(
        after.lines().filter(|l| l.starts_with("worktree ")).count(),
        1,
        "git should only know the main worktree: {}",
        after
    );
}

#[test]
fn rm_on_a_detached_worktree_still_checks_for_an_attached_container() {
    let Some(sb) = Sandbox::new("rmdetcontainer") else {
        return;
    };
    let repo = make_repo(&sb, "proj", "main");
    git(&repo, &["worktree", "add", "-q", "--detach", "../det"]);
    assert!(agentdock(&repo, &sb.home(), &["worktree", "init"])
        .status
        .success());

    let main = sb.path("proj/proj-main");
    let short = stdout(&agentdock(&main, &sb.home(), &["worktree", "list"]))
        .lines()
        .filter(|l| l.starts_with("worktree"))
        .filter_map(|l| l.split_whitespace().nth(1))
        .next()
        .expect("detached worktree row")
        .to_string();

    let records = sb.home().join(".config/agentdock");
    std::fs::create_dir_all(&records).expect("create records dir");
    std::fs::write(
        records.join("records.json"),
        format!(
            r#"{{"records":{{"det-c":{{"path":"{}","created_at":"2026-01-01T00:00:00Z","docker_image":"img","agent_name":"agent","kvm":false}}}}}}"#,
            sb.path(&format!("proj/proj-{}", short)).display()
        ),
    )
    .expect("write records");

    let out = agentdock(&main, &sb.home(), &["worktree", "rm", &short]);
    assert!(
        !out.status.success(),
        "removed despite container: {}",
        stdout(&out)
    );
    assert!(
        stderr(&out).contains("det-c"),
        "should name the container: {}",
        stderr(&out)
    );

    let out = agentdock(&main, &sb.home(), &["worktree", "rm", &short, "--force"]);
    assert!(out.status.success(), "force rm failed: {}", stderr(&out));
    let left = stdout(&agentdock(
        &main,
        &sb.home(),
        &["list", "--all", "--format", "json"],
    ));
    assert!(!left.contains("det-c"), "record survived: {}", left);
}

#[test]
fn add_refuses_a_name_taken_by_a_detached_worktree() {
    let Some(sb) = Sandbox::new("addcollide") else {
        return;
    };
    let repo = make_repo(&sb, "proj", "main");
    git(&repo, &["worktree", "add", "-q", "--detach", "../det"]);
    assert!(agentdock(&repo, &sb.home(), &["worktree", "init"])
        .status
        .success());

    let main = sb.path("proj/proj-main");
    let short = stdout(&agentdock(&main, &sb.home(), &["worktree", "list"]))
        .lines()
        .filter(|l| l.starts_with("worktree"))
        .filter_map(|l| l.split_whitespace().nth(1))
        .next()
        .expect("detached worktree row")
        .to_string();

    // The detached worktree occupies proj-<sha>, so a branch of that exact name
    // would land on the same path. add rejects it, naming the detached
    // worktree rather than leaving a bare "directory already exists".
    let out = agentdock(&main, &sb.home(), &["worktree", "add", &short]);
    assert!(!out.status.success(), "accepted: {}", stdout(&out));
    let err = stderr(&out);
    assert!(
        err.contains("detached") && err.contains("cannot be reused"),
        "should explain the collision: {}",
        err
    );
}

#[test]
fn add_accepts_a_branch_named_like_a_detached_worktree_directory() {
    let Some(sb) = Sandbox::new("adddirname") else {
        return;
    };
    let repo = make_repo(&sb, "proj", "main");
    git(&repo, &["worktree", "add", "-q", "--detach", "../det"]);
    assert!(agentdock(&repo, &sb.home(), &["worktree", "init"])
        .status
        .success());

    let main = sb.path("proj/proj-main");
    let short = stdout(&agentdock(&main, &sb.home(), &["worktree", "list"]))
        .lines()
        .filter(|l| l.starts_with("worktree"))
        .filter_map(|l| l.split_whitespace().nth(1))
        .next()
        .expect("detached worktree row")
        .to_string();

    // "proj-<sha>" is a valid branch name, and it would be checked out into
    // "proj-proj-<sha>", which does not collide with the detached worktree.
    // add names a branch it is creating, so it must not reject this just
    // because a detached worktree happens to be called the same thing.
    let branch = format!("proj-{}", short);
    let out = agentdock(&main, &sb.home(), &["worktree", "add", &branch]);
    assert!(
        out.status.success(),
        "rejected a valid branch name: {}",
        stderr(&out)
    );
    assert!(
        sb.path(&format!("proj/proj-{}", branch))
            .join("src/lib.rs")
            .exists(),
        "worktree for {} was not created",
        branch
    );
}

#[test]
fn init_refuses_a_second_time() {
    let Some(sb) = Sandbox::new("idempotent") else {
        return;
    };
    let repo = make_repo(&sb, "proj", "main");
    assert!(agentdock(&repo, &sb.home(), &["worktree", "init"])
        .status
        .success());

    let main = sb.path("proj/proj-main");
    let out = agentdock(&main, &sb.home(), &["worktree", "init"]);
    assert!(!out.status.success(), "second init was accepted");
}

#[test]
fn add_flattens_branch_names_and_stays_usable() {
    let Some(sb) = Sandbox::new("add") else {
        return;
    };
    let repo = make_repo(&sb, "proj", "main");
    assert!(agentdock(&repo, &sb.home(), &["worktree", "init"])
        .status
        .success());

    let main = sb.path("proj/proj-main");
    let out = agentdock(&main, &sb.home(), &["worktree", "add", "feat/login"]);
    assert!(out.status.success(), "add failed: {}", stderr(&out));

    let flat = sb.path("proj/proj-feat-login");
    assert!(flat.exists(), "branch name was not flattened");
    assert!(flat.join("src/lib.rs").exists(), "worktree has no files");

    let branch = git(&flat, &["rev-parse", "--abbrev-ref", "HEAD"]);
    assert_eq!(branch, "feat/login", "wrong branch checked out");
}

#[test]
fn add_refuses_a_branch_that_is_already_checked_out() {
    let Some(sb) = Sandbox::new("checkedout") else {
        return;
    };
    let repo = make_repo(&sb, "proj", "main");
    assert!(agentdock(&repo, &sb.home(), &["worktree", "init"])
        .status
        .success());

    let main = sb.path("proj/proj-main");
    let out = agentdock(&main, &sb.home(), &["worktree", "add", "main"]);
    assert!(!out.status.success(), "accepted: {}", stdout(&out));
    assert!(
        stderr(&out).contains("already checked out"),
        "unexpected message: {}",
        stderr(&out)
    );
}

#[test]
fn worktree_commands_report_a_non_converted_project_clearly() {
    let Some(sb) = Sandbox::new("unconverted") else {
        return;
    };
    let repo = make_repo(&sb, "plain", "main");

    for args in [vec!["worktree", "list"], vec!["worktree", "rm", "whatever"]] {
        let out = agentdock(&repo, &sb.home(), &args);
        assert!(!out.status.success(), "{:?} unexpectedly succeeded", args);
        let err = stderr(&out);
        assert!(
            err.contains("not part of an agentdock worktree project"),
            "message leaked internals for {:?}: {}",
            args,
            err
        );
        assert!(
            !err.contains(".agentdock.json"),
            "marker filename leaked for {:?}: {}",
            args,
            err
        );
    }
}

#[test]
fn list_marks_worktrees_whose_directory_is_gone() {
    let Some(sb) = Sandbox::new("stale") else {
        return;
    };
    let repo = make_repo(&sb, "proj", "main");
    assert!(agentdock(&repo, &sb.home(), &["worktree", "init"])
        .status
        .success());

    let main = sb.path("proj/proj-main");
    assert!(agentdock(&main, &sb.home(), &["worktree", "add", "dev"])
        .status
        .success());
    std::fs::remove_dir_all(sb.path("proj/proj-dev")).expect("remove worktree dir");

    let out = agentdock(&main, &sb.home(), &["worktree", "list"]);
    assert!(out.status.success(), "list failed: {}", stderr(&out));
    assert!(
        stdout(&out).contains("stale"),
        "stale worktree not flagged: {}",
        stdout(&out)
    );
}

#[test]
fn rm_protects_the_main_worktree() {
    let Some(sb) = Sandbox::new("protect") else {
        return;
    };
    let repo = make_repo(&sb, "proj", "main");
    assert!(agentdock(&repo, &sb.home(), &["worktree", "init"])
        .status
        .success());

    let main = sb.path("proj/proj-main");
    let out = agentdock(&main, &sb.home(), &["worktree", "rm", "main"]);
    assert!(!out.status.success(), "main worktree was removable");
    assert!(main.join(".git").exists(), "main worktree was destroyed");
}
