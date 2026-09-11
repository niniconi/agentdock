use clap::{Parser, Subcommand};
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
}

#[derive(Parser)]
pub struct RunArgs {
    /// Agent config format: {docker_image}/{agent_name} (e.g., nixos/opencode)
    #[arg(short, long, default_value = "nixos/pi-agent")]
    pub agent: String,

    /// Auto-delete container after exit
    #[arg(long)]
    pub rm: bool,

    /// Host mount directory (default: current directory)
    #[arg(short, long)]
    pub path: Option<PathBuf>,

    /// Custom initialization script
    #[arg(short, long)]
    pub init: Option<String>,

    /// Record identifier name
    #[arg(short, long)]
    pub name: Option<String>,

    /// Map host /dev/kvm into container (for KVM virtualization)
    #[arg(long)]
    pub kvm: bool,

    /// HTTP proxy address (e.g. http://127.0.0.1:7890)
    #[arg(long)]
    pub http_proxy: Option<String>,

    /// HTTPS proxy address (e.g. http://127.0.0.1:7890)
    #[arg(long)]
    pub https_proxy: Option<String>,
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
