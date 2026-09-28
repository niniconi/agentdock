use std::path::Path;

pub fn container_not_found_error(name: &str) -> String {
    format!(
        r#"Associated container '{}' does not exist

Possible causes:
  1. Container was manually deleted: docker rm {}
  2. Started with --rm flag, container was auto-deleted after exit
  3. Docker environment was reset

Suggested actions:
  - Re-run agentdock to start a new instance
  - Or manually clean up records: ~/.config/agentdock/records.json"#,
        name, name
    )
}

pub fn not_a_git_repo_error(path: &Path) -> String {
    format!(
        "'{}' is not inside a git repository.

Suggested actions:
  - Run this command from a git project
  - Or create one first: git init",
        path.display()
    )
}

pub fn already_converted_error(path: &Path) -> String {
    format!(
        "'{}' is already in worktree form.

Suggested actions:
  - agentdock worktree list
  - agentdock worktree add <branch> --run",
        path.display()
    )
}

pub fn nested_project_error(path: &Path) -> String {
    format!(
        "'{}' sits inside an existing worktree project, so it cannot be converted again.

Suggested actions:
  - Convert the outer project from its own root directory",
        path.display()
    )
}

pub fn no_parent_dir_error(path: &Path) -> String {
    format!(
        "'{}' has no parent directory, so it cannot host worktrees.",
        path.display()
    )
}

pub fn worktree_conflict_error(path: &Path, conflicts: &[(String, std::path::PathBuf)]) -> String {
    let mut msg = format!(
        "'{}' already has agentdock containers mounted inside it, so it cannot be converted.

These records point at the pre-move locations and would silently go stale:
",
        path.display()
    );
    for (name, record_path) in conflicts {
        msg.push_str(&format!("  {}  ->  {}\n", name, record_path.display()));
    }
    msg.push_str(
        "
Suggested actions:
  agentdock list
  agentdock delete <name>     # clean up one by one, then retry",
    );
    msg
}

pub fn target_exists_error(path: &Path) -> String {
    format!(
        "'{}' already exists.

Suggested actions:
  - Choose a different branch name
  - Or remove the existing directory first",
        path.display()
    )
}

pub fn destination_collision_error(path: &Path, taken_by: &str) -> String {
    format!(
        "Cannot place a worktree at '{}': the path is already used by {}.

Suggested actions:
  - Rename or remove the conflicting path, then retry",
        path.display(),
        taken_by
    )
}

pub fn worktree_inside_repo_error(path: &Path) -> String {
    format!(
        "Worktree '{}' is located inside the repository, so it cannot be relocated.

Moving the repository moves this worktree along with it, which would leave it
detached from git's records.

Suggested actions:
  - Move it outside the repository first: git worktree move {} <outside-path>
  - Then re-run agentdock worktree init",
        path.display(),
        path.display()
    )
}

pub fn stale_stage_dir_error(path: &Path) -> String {
    format!(
        "The staging directory '{}' already exists.

This usually means a previous conversion was interrupted.

Suggested actions:
  - Inspect it, then remove it: rm -rf {}",
        path.display(),
        path.display()
    )
}

pub fn not_in_worktree_project_error(path: &Path) -> String {
    format!(
        "'{}' is not part of an agentdock worktree project.

Suggested actions:
  - Run 'agentdock worktree init' from the project root to convert it first
  - Or run this command from inside a converted project",
        path.display()
    )
}

pub fn main_worktree_missing_error(path: &Path) -> String {
    format!(
        "The main worktree '{}' is missing, so this project cannot be used.

Possible causes:
  1. It was deleted manually
  2. git worktree prune removed it

Suggested actions:
  - Restore it from the remote, then retry",
        path.display()
    )
}

pub fn existing_branch_with_start_point_error(branch: &str) -> String {
    format!(
        "Branch '{}' already exists, so --start-point cannot be applied.

--start-point only applies to branches that agentdock creates for you.

Suggested actions:
  - Drop --start-point
  - Or use a new branch name",
        branch
    )
}

pub fn branch_in_use_error(branch: &str, path: &Path) -> String {
    format!(
        "Branch '{}' is already checked out at '{}'.

A branch can only be checked out in one worktree at a time.

Suggested actions:
  - Pick a different branch name
  - Or remove the other worktree: agentdock worktree rm {}",
        branch,
        path.display(),
        branch
    )
}

pub fn worktree_not_found_error(branch: &str, container: &Path) -> String {
    format!(
        "No worktree for branch '{}' in '{}'.

Suggested actions:
  - agentdock worktree list
  - agentdock worktree add {}",
        branch,
        container.display(),
        branch
    )
}

pub fn name_taken_by_detached_error(name: &str, path: &Path) -> String {
    format!(
        "'{}' is already used by the detached worktree at '{}'.

A detached worktree has no branch, so it is named after its short commit id,
and that name cannot be reused for a new branch.

Suggested actions:
  - Choose a different branch name
  - Or remove the detached worktree first: agentdock worktree rm {}",
        name,
        path.display(),
        name
    )
}

pub fn cannot_remove_main_error(path: &Path) -> String {
    format!(
        "'{}' is the main worktree and cannot be removed.

The main worktree holds the repository's .git directory, so removing it would
destroy the project.

Suggested actions:
  - Remove a different worktree instead",
        path.display()
    )
}

pub fn container_attached_error(name: &str, path: &Path) -> String {
    format!(
        "Worktree '{}' still has container '{}' attached to it.

Removing the worktree would leave that record pointing at a missing path.

Suggested actions:
  - agentdock delete {} --force
  - Or pass --force to remove both",
        path.display(),
        name,
        name
    )
}
