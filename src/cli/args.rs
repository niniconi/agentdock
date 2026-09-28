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
    /// Start or manage an AI agent container
    Run(RunArgs),
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

/// Options shared by `run` and `worktree add --run`.
///
/// Lives in its own struct so the worktree subcommand can reuse the exact same
/// flags instead of duplicating them.
#[derive(Args, Clone)]
pub struct RunOpts {
    /// Agent config format: {docker_image}/{agent_name} (e.g., nixos/opencode)
    #[arg(short, long, default_value = "nixos/pi-agent")]
    pub agent: String,

    /// Auto-delete container after exit
    #[arg(long)]
    pub rm: bool,

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
    #[arg(short = 'P', long = "port", value_name = "HOST:CONTAINER")]
    pub port: Vec<String>,
}

#[derive(Parser)]
pub struct RunArgs {
    /// Host mount directory (default: current directory)
    #[arg(short, long)]
    pub path: Option<PathBuf>,

    /// Record identifier name
    #[arg(short, long)]
    pub name: Option<String>,

    #[command(flatten)]
    pub opts: RunOpts,
}

impl RunArgs {
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

    /// Start an agent container inside the new worktree
    #[arg(long)]
    pub run: bool,

    /// Container options, only used together with --run
    #[command(flatten)]
    pub opts: RunOpts,
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
