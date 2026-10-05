use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "agentdock", about = "Docker-based AI Agent Manager", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Create a container, or change an existing one to match the given settings
    Apply(ApplyArgs),
    /// Start an existing container without changing how it was built
    Up(UpArgs),
    /// List all managed containers
    List(ListArgs),
    /// Delete a container and its record
    Delete(DeleteArgs),
    /// Show container status
    Status(StatusArgs),
    /// Convert a project into git worktree form for multi-agent collaboration
    #[command(subcommand)]
    Worktree(WorktreeArgs),
}

/// Options shared by `apply` and `worktree add --apply`.
///
/// Lives in its own struct so the worktree subcommand can reuse the exact same
/// flags instead of duplicating them.
#[derive(Args, Clone)]
pub struct ApplyOpts {
    /// Agent config format: {docker_image}/{agent_name} (e.g., nixos/opencode)
    ///
    /// Defaults to nixos/pi-agent.
    #[arg(short, long, default_value = "nixos/pi-agent")]
    pub agent: String,

    /// Custom initialization script
    #[arg(short, long)]
    pub init: Option<String>,

    /// Map host /dev/kvm into container (for KVM virtualization)
    #[arg(long)]
    pub kvm: bool,

    /// HTTP proxy address (e.g. http://127.0.0.1:7890)
    #[arg(long)]
    pub http_proxy: Option<String>,

    /// HTTPS proxy address (e.g. http://127.0.0.1:7890)
    #[arg(long)]
    pub https_proxy: Option<String>,

    /// Port mapping (e.g., 8080:80, 3000:3000). Can be specified multiple times.
    ///
    /// Omitted means no ports are published: apply replaces the configuration
    /// rather than merging into it.
    #[arg(short = 'P', long = "port", value_name = "HOST:CONTAINER")]
    pub port: Vec<String>,

    /// Persist opencode's config and data directories under
    /// ~/.local/share/agentdock/<container>/opencode/
    ///
    /// Bare --persist persists both. Name one to persist only that half:
    /// --persist config, or --persist data. The container-side paths are
    /// derived from the image's default user, not asked for.
    ///
    /// Omitted means nothing is persisted.
    #[arg(
        long,
        value_name = "WHAT",
        num_args = 0..=1,
        value_delimiter = ',',
        default_missing_value = "config,data",
        value_parser = ["config", "data"],
    )]
    pub persist: Option<Vec<String>>,
}

#[derive(Parser)]
pub struct ApplyArgs {
    /// Host mount directory (default: current directory)
    #[arg(short, long)]
    pub path: Option<PathBuf>,

    /// Record identifier name
    #[arg(short, long)]
    pub name: Option<String>,

    #[command(flatten)]
    pub opts: ApplyOpts,
}

impl ApplyArgs {
    pub fn get_mount_path(&self) -> PathBuf {
        self.path
            .clone()
            .unwrap_or_else(|| std::env::current_dir().expect("Failed to get current directory"))
    }
}

#[derive(Parser)]
pub struct UpArgs {
    /// Host mount directory (default: current directory)
    #[arg(short, long)]
    pub path: Option<PathBuf>,

    /// Record identifier name
    #[arg(short, long)]
    pub name: Option<String>,
}

impl UpArgs {
    pub fn get_mount_path(&self) -> PathBuf {
        self.path
            .clone()
            .unwrap_or_else(|| std::env::current_dir().expect("Failed to get current directory"))
    }
}

#[derive(Parser)]
pub struct ListArgs {
    /// Show all containers including stopped
    #[arg(long)]
    pub all: bool,

    /// Show detailed information
    #[arg(short, long)]
    pub verbose: bool,

    /// Output format (table, json)
    #[arg(short, long, default_value = "table")]
    pub format: String,
}

#[derive(Parser)]
pub struct DeleteArgs {
    /// Container name to delete
    pub name: String,

    /// Also remove the Docker container (not just the record)
    #[arg(long)]
    pub force: bool,

    /// Also delete the agent's persisted data under
    /// ~/.local/share/agentdock/<name>
    ///
    /// Off by default: that data lives outside the container, so removing the
    /// container does not remove it, and it is the only copy there is.
    #[arg(long)]
    pub purge: bool,
}

#[derive(Parser)]
pub struct StatusArgs {
    /// Container name
    pub name: String,
}

#[derive(Subcommand)]
pub enum WorktreeArgs {
    /// Convert the current project into worktree form
    Init(WorktreeInitArgs),
    /// Add a worktree for a new branch
    Add(WorktreeAddArgs),
    /// List all worktrees in this project
    List(WorktreeListArgs),
    /// Remove a worktree
    Rm(WorktreeRmArgs),
}

#[derive(Parser)]
pub struct WorktreeInitArgs {}

#[derive(Parser)]
pub struct WorktreeAddArgs {
    /// Branch name (a matching branch is created if it does not exist)
    pub branch: String,

    /// Branch or commit to base the new worktree on (default: the current HEAD)
    #[arg(long)]
    pub start_point: Option<String>,

    /// Create and start an agent container inside the new worktree
    #[arg(long)]
    pub apply: bool,

    /// Container options, only used together with --apply
    #[command(flatten)]
    pub opts: ApplyOpts,
}

#[derive(Parser)]
pub struct WorktreeListArgs {
    /// Output format (table, json)
    #[arg(short, long, default_value = "table")]
    pub format: String,
}

#[derive(Parser)]
pub struct WorktreeRmArgs {
    /// Branch whose worktree should be removed
    pub branch: String,

    /// Discard uncommitted changes and any attached container record
    #[arg(long)]
    pub force: bool,
}
