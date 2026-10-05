use anyhow::{Result, bail};

use crate::cli::DeleteArgs;
use crate::docker::{ContainerStatus, DockerClient};
use crate::error::ContainerError;
use crate::state::StateManager;

pub fn execute_delete(args: DeleteArgs) -> Result<()> {
    let mut state = StateManager::new()?;

    if state.find_by_name(&args.name).is_none() {
        bail!(ContainerError::NotInRecords { name: args.name });
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
                bail!(ContainerError::RunningWithoutForce { name: args.name });
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
