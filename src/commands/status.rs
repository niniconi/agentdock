use anyhow::{bail, Result};

use crate::cli::StatusArgs;
use crate::docker::{ContainerStatus, DockerClient};
use crate::state::StateManager;

pub fn execute_status(args: StatusArgs) -> Result<()> {
    let state = StateManager::new()?;

    let record = match state.find_by_name(&args.name) {
        Some(record) => record,
        None => {
            bail!("Container '{}' not found in managed records", args.name);
        }
    };

    let status = DockerClient::inspect(&args.name)?;

    let status_str = match status {
        ContainerStatus::Running => "Running",
        ContainerStatus::Stopped => "Stopped",
        ContainerStatus::NotFound => "NotFound",
    };

    println!("Container:    {}", args.name);
    println!("Status:       {}", status_str);
    println!("Path:         {}", record.path.display());
    println!("Created:      {}", record.created_at);

    if let Some(ref proxy) = record.http_proxy {
        println!("HTTP Proxy:   {}", proxy);
    }
    if let Some(ref proxy) = record.https_proxy {
        println!("HTTPS Proxy:  {}", proxy);
    }

    Ok(())
}
