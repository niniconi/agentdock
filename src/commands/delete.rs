use anyhow::{bail, Result};

use crate::cli::DeleteArgs;
use crate::docker::{ContainerStatus, DockerClient};
use crate::state::StateManager;

pub fn execute_delete(args: DeleteArgs) -> Result<()> {
    let mut state = StateManager::new()?;

    if state.find_by_name(&args.name).is_none() {
        bail!("Container '{}' not found in managed records", args.name);
    }

    let status = DockerClient::inspect(&args.name)?;

    if args.force {
        match status {
            ContainerStatus::Running | ContainerStatus::Stopped => {
                DockerClient::destroy(&args.name)?;
                println!("Docker container removed: {}", args.name);
            }
            ContainerStatus::NotFound => {
                println!("Docker container not found, skipping removal.");
            }
        }
    } else {
        match status {
            ContainerStatus::Running => {
                bail!(
                    "Container '{}' is running. Use --force to remove it.",
                    args.name
                );
            }
            ContainerStatus::Stopped => {
                DockerClient::destroy(&args.name)?;
                println!("Docker container removed: {}", args.name);
            }
            ContainerStatus::NotFound => {
                println!("Docker container not found, skipping removal.");
            }
        }
    }

    state.remove(&args.name);
    state.save()?;
    println!("Deleted record: {}", args.name);

    Ok(())
}
