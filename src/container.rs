use anyhow::{Result, bail};
use std::path::Path;

use crate::config::Config;
use crate::docker::DockerClient;
use crate::error::ContainerError;
use crate::init::{read_init_content, run_init_script};
use crate::persist::{self, Mount};
use crate::state::StateManager;

/// Create a container in the state `config` describes.
pub fn create(
    name: &str,
    config: &Config,
    mount_path: &Path,
    init_path: Option<&str>,
    state: &mut StateManager,
) -> Result<()> {
    // Read before anything is created, so an unreadable script leaves neither a
    // container nor a record behind.
    let init_content = read_init(init_path, mount_path)?;
    let mounts = resolve_mounts(name, config)?;

    let mut applied = config.clone();
    applied.init_content = init_content.clone();

    println!("Creating container: {}", name);
    DockerClient::run(name, &applied, mount_path, &mounts)?;

    state.insert(
        name.to_string(),
        applied.to_record(mount_path.to_path_buf(), crate::state::now_string()),
    );
    state.save()?;
    println!("Record saved: {} -> {}", name, mount_path.display());

    run_init_script(name, init_content.as_deref())?;
    Ok(())
}

/// Replace an existing container with one in the state `config` describes.
///
/// `docker run` is the only way to change how a container is built, so applying
/// and creating converge on the same call.
pub fn replace(
    name: &str,
    config: &Config,
    mount_path: &Path,
    init_path: Option<&str>,
    state: &mut StateManager,
) -> Result<()> {
    let init_content = read_init(init_path, mount_path)?;
    // Resolved before the destroy rather than between it and the run: a failure
    // here would leave the user with no container at all, which is the one
    // outcome a rebuild exists to prevent.
    let mounts = resolve_mounts(name, config)?;

    let mut applied = config.clone();
    applied.init_content = init_content.clone();

    println!("Recreating container: {}", name);
    DockerClient::destroy(name)?;
    DockerClient::run(name, &applied, mount_path, &mounts)?;

    state.insert(
        name.to_string(),
        applied.to_record(mount_path.to_path_buf(), crate::state::now_string()),
    );
    state.save()?;

    run_init_script(name, init_content.as_deref())?;
    Ok(())
}

/// Reach a running container with the agent attached, leaving how it was built
/// alone. Nothing here can change the configuration.
pub fn start(name: &str, config: &Config) -> Result<()> {
    use crate::docker::ContainerStatus;

    // A stopped container is the only case that needs bringing up; a running one
    // is already there. A missing one was removed outside agentdock, and this
    // command does not create containers.
    match DockerClient::inspect(name)? {
        ContainerStatus::Stopped => {
            println!("Restarting stopped container: {}", name);
            DockerClient::restart(name)?;
        }
        ContainerStatus::NotFound => bail!(ContainerError::NotFound {
            name: name.to_string(),
            image: config.docker_image.clone(),
            agent: config.agent_name.clone(),
        }),
        ContainerStatus::Running => {}
    }

    println!("Starting Agent: {}", config.agent_name);
    DockerClient::exec(name, &config.agent_name)?;
    Ok(())
}

/// The init script's contents, or None when no path was given.
fn read_init(init_path: Option<&str>, mount_path: &Path) -> Result<Option<String>> {
    match init_path {
        Some(path) => Ok(Some(read_init_content(path, mount_path)?)),
        None => Ok(None),
    }
}

/// The bind mounts for the directories `config` asks to persist.
///
/// Empty when nothing is persisted, which is what leaving the flag out means.
///
/// The image matters only for its home directory; which directories exist is
/// not decided by the image or by the agent named in `-a`, since that flag picks
/// an entry point rather than an owner.
fn resolve_mounts(name: &str, config: &Config) -> Result<Vec<Mount>> {
    let Some(kinds) = config.persist.as_ref().filter(|k| !k.is_empty()) else {
        return Ok(Vec::new());
    };

    let mounts = persist::plan_mounts(name, &config.docker_image, kinds)?;

    for m in &mounts {
        println!("Persisting: {} -> {}", m.host.display(), m.container);
    }

    Ok(mounts)
}
