use std::path::PathBuf;

/// Failures a user can act on. The text of each variant is the message the user
/// sees, so it keeps the explanation and the suggested commands together.
///
/// These are raised with `bail!` and carried by `anyhow`; they are not a single
/// top-level enum, because anyhow already provides that role while these types
/// stay matchable at the call site.
#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    #[error(
        "'{path}' is not inside a git repository.

Suggested actions:
  - Run this command from a git project
  - Or create one first: git init"
    )]
    NotAGitRepo { path: PathBuf },

    #[error(
        "'{path}' is already in worktree form.

Suggested actions:
  - agentdock worktree list
  - agentdock worktree add <branch> --run"
    )]
    AlreadyConverted { path: PathBuf },

    #[error(
        "'{path}' sits inside an existing worktree project, so it cannot be converted again.

Suggested actions:
  - Convert the outer project from its own root directory"
    )]
    NestedProject { path: PathBuf },

    #[error("'{path}' has no parent directory, so it cannot host worktrees.")]
    NoParentDir { path: PathBuf },

    #[error(
        "'{path}' already has agentdock containers mounted inside it, so it cannot be converted.

These records point at the pre-move locations and would silently go stale:
{conflicts}

Suggested actions:
  agentdock list
  agentdock delete <name>     # clean up one by one, then retry"
    )]
    ContainerConflict { path: PathBuf, conflicts: String },

    #[error(
        "'{path}' already exists.

Suggested actions:
  - Choose a different branch name
  - Or remove the existing directory first"
    )]
    TargetExists { path: PathBuf },

    #[error(
        "Cannot place a worktree at '{path}': the path is already used by {taken_by}.

Suggested actions:
  - Rename or remove the conflicting path, then retry"
    )]
    DestinationCollision { path: PathBuf, taken_by: String },

    #[error(
        "Worktree '{path}' is located inside the repository, so it cannot be relocated.

Moving the repository moves this worktree along with it, which would leave it
detached from git's records.

Suggested actions:
  - Move it outside the repository first: git worktree move {path} <outside-path>
  - Then re-run agentdock worktree init"
    )]
    WorktreeInsideRepo { path: PathBuf },

    #[error(
        "The staging directory '{path}' already exists.

This usually means a previous conversion was interrupted.

Suggested actions:
  - Inspect it, then remove it: rm -rf {path}"
    )]
    StaleStageDir { path: PathBuf },

    #[error(
        "'{path}' is not part of an agentdock worktree project.

Suggested actions:
  - Run 'agentdock worktree init' from the project root to convert it first
  - Or run this command from inside a converted project"
    )]
    NotInWorktreeProject { path: PathBuf },

    #[error(
        "The main worktree '{path}' is missing, so this project cannot be used.

Possible causes:
  1. It was deleted manually
  2. git worktree prune removed it

Suggested actions:
  - Restore it from the remote, then retry"
    )]
    MainWorktreeMissing { path: PathBuf },

    #[error(
        "Branch '{branch}' already exists, so --start-point cannot be applied.

--start-point only applies to branches that agentdock creates for you.

Suggested actions:
  - Drop --start-point
  - Or use a new branch name"
    )]
    ExistingBranchWithStartPoint { branch: String },

    #[error(
        "Branch '{branch}' is already checked out at '{path}'.

A branch can only be checked out in one worktree at a time.

Suggested actions:
  - Pick a different branch name
  - Or remove the other worktree: agentdock worktree rm {branch}"
    )]
    BranchInUse { branch: String, path: PathBuf },

    #[error(
        "No worktree for branch '{branch}' in '{container}'.

Suggested actions:
  - agentdock worktree list
  - agentdock worktree add {branch}"
    )]
    WorktreeNotFound { branch: String, container: PathBuf },

    #[error(
        "'{name}' is already used by the detached worktree at '{path}'.

A detached worktree has no branch, so it is named after its short commit id,
and that name cannot be reused for a new branch.

Suggested actions:
  - Choose a different branch name
  - Or remove the detached worktree first: agentdock worktree rm {name}"
    )]
    NameTakenByDetached { name: String, path: PathBuf },

    #[error(
        "'{path}' is the main worktree and cannot be removed.

The main worktree holds the repository's .git directory, so removing it would
destroy the project.

Suggested actions:
  - Remove a different worktree instead"
    )]
    CannotRemoveMain { path: PathBuf },
}

#[derive(Debug, thiserror::Error)]
pub enum ContainerError {
    #[error(
        "Associated container '{name}' does not exist

The container was removed outside agentdock, while its record remained.

Possible causes:
  1. Manually deleted: docker rm {name}
  2. Docker environment was reset

Suggested actions:
  - agentdock apply -n {name}     # recreate it from its recorded settings
  - Or drop the record: agentdock delete {name}"
    )]
    NotFound { name: String },

    #[error("Container '{name}' not found in managed records")]
    NotInRecords { name: String },

    #[error(
        "No container is managed {name}

This command starts what agentdock already configured, it does not create one.

Suggested actions:
  - agentdock apply{flag}     # create it from the flags you just passed"
    )]
    NotManaged { name: String, flag: String },

    #[error(
        "Container '{name}' is running.

Suggested actions:
  - Stop it first: docker stop {name}
  - Or pass --force to remove it"
    )]
    RunningWithoutForce { name: String },

    #[error(
        "Worktree '{path}' still has container '{name}' attached to it.

Removing the worktree would leave that record pointing at a missing path.

Suggested actions:
  - agentdock delete {name} --force
  - Or pass --force to remove both"
    )]
    Attached { name: String, path: PathBuf },

    #[error(
        "docker is not installed or not on PATH

Suggested actions:
  - Install docker, then retry"
    )]
    ToolMissing,

    #[error("docker {op} failed: {stderr}")]
    Command { op: &'static str, stderr: String },

    #[error("Command execution failed, exit code: {code}")]
    ExecFailed { code: i32 },
}

#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    #[error("Failed to read persistent records")]
    Read(#[source] std::io::Error),

    #[error("Failed to parse persistent records")]
    Parse(#[source] serde_json::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error(
        "git is not installed or not on PATH

Suggested actions:
  - Install git, then retry"
    )]
    ToolMissing,

    #[error("git {args} failed: {stderr}")]
    Command { args: String, stderr: String },

    #[error(
        "HEAD is detached. Please check out a branch before running this command.

Suggested actions:
  - git switch <branch-name>
  - Or create one: git switch -c <branch-name>"
    )]
    DetachedHead,
}

/// Format the conflicting records for `WorktreeError::ContainerConflict`.
pub fn format_conflicts(conflicts: &[(String, std::path::PathBuf)]) -> String {
    conflicts
        .iter()
        .map(|(name, path)| format!("  {}  ->  {}", name, path.display()))
        .collect::<Vec<_>>()
        .join("\n")
}
