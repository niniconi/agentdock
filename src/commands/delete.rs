use anyhow::{Result, bail};

use crate::cli::DeleteArgs;
use crate::docker::{ContainerStatus, DockerClient};
use crate::error::ContainerError;
use crate::persist;
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

    if args.purge {
        purge_data(&args.name)?;
    } else if let Ok(dir) = persist::container_data_dir(&args.name)
        && dir.exists()
    {
        // Only when there is something to keep. The record that said whether it
        // was persisted has just been deleted, so the directory itself is the
        // only thing left that knows.
        println!();
        println!("Persisted data kept at: {}", dir.display());
        println!("  Remove it with: rm -rf {}", dir.display());
    }

    Ok(())
}

/// Delete the host directory a container's persisted data lives in.
///
/// The whole container directory, so a container that was renamed cannot leave
/// an orphan half behind. A directory that is already gone is not an error: the
/// point of `--purge` is that the data is gone afterwards, and it already being
/// gone satisfies that.
///
/// An unusable name is a different matter and does fail: `--purge` asked for
/// the data to be gone, and returning success while deleting nothing would
/// report a purge that did not happen.
fn purge_data(name: &str) -> Result<()> {
    let dir = persist::container_data_dir(name)?;

    if !dir.exists() {
        return Ok(());
    }

    match std::fs::remove_dir_all(&dir) {
        Ok(()) => println!("Purged persisted data: {}", dir.display()),
        // The record is already deleted, so failing here cannot be undone and
        // the data is still there to remove by hand.
        Err(e) => {
            println!("warning    : could not remove {}, {}", dir.display(), e);
            println!("            remove it with: rm -rf {}", dir.display());
        }
    }
    Ok(())
}
