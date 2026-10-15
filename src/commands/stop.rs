use anyhow::Result;

use crate::cli::StopArgs;
use crate::docker::{ContainerStatus, DockerClient};
use crate::error::ContainerError;
use crate::state::StateManager;

/// Stop a container agentdock manages, leaving its record and how it was built
/// alone.
///
/// The counterpart to `up`: it brings a running container down without removing
/// it, so the same container comes back with `up` and keeps its configuration.
/// Deleting is a separate command, and stopping never chooses to delete.
pub fn execute_stop(args: StopArgs) -> Result<()> {
    let mount_path = args.get_mount_path();
    let state = StateManager::new()?;

    // Stop what is already recorded. A directory with no record has nothing to
    // stop, and this command does not create anything.
    let found = match &args.name {
        Some(name) => state
            .find_by_name(name)
            .map(|record| (name.clone(), record.clone())),
        None => state
            .find_by_path(&mount_path)
            .map(|(name, record)| (name.to_string(), record.clone())),
    };

    let Some((name, record)) = found else {
        let (name, flag) = match &args.name {
            Some(n) => (format!("named '{}'", n), format!(" -n {}", n)),
            None => (
                format!("mounted at '{}'", mount_path.display()),
                String::new(),
            ),
        };
        return Err(ContainerError::NotManagedForStop { name, flag }.into());
    };

    match DockerClient::inspect(&name)? {
        ContainerStatus::Running => {
            DockerClient::stop(&name)?;
            println!("Stopped container: {}", name);
        }
        // Stopping an already-stopped container is the outcome the user asked
        // for, so it succeeds without touching anything.
        ContainerStatus::Stopped => {
            println!("Container '{}' is already stopped", name);
        }
        // The record outlived the container, which is the same broken state
        // `up` reports: recreating is `apply`, and making the record match
        // reality is `delete`.
        ContainerStatus::NotFound => {
            return Err(ContainerError::NotFound {
                name,
                image: record.docker_image,
                agent: record.agent_name,
            }
            .into());
        }
    }

    Ok(())
}
