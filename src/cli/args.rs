use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "agentdock", about = "Docker-based AI Agent Manager", version)]
pub struct Cli {
    /// Agent config format: {docker_image}/{agent_name} (e.g., nixos/opencode)
    #[arg(short, long, default_value = "nixos/pi-agent")]
    pub agent: String,

    /// Auto-delete container after exit
    #[arg(long)]
    pub rm: bool,

    /// Host mount directory (default: current directory)
    #[arg(short, long)]
    pub path: Option<PathBuf>,

    /// Mount Herdr Unix Socket
    #[arg(long)]
    pub herdr_sock: bool,

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

impl Cli {
    pub fn get_mount_path(&self) -> PathBuf {
        self.path
            .clone()
            .unwrap_or_else(|| std::env::current_dir().expect("Failed to get current directory"))
    }
}
