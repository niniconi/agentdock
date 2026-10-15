use anyhow::{Result, bail};
use std::path::Path;

use crate::config::{Config, Persist};
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
    let init_content = read_init(init_path, mount_path, config.init_content.as_deref())?;
    let mounts = resolve_mounts(name, config)?;

    let mut applied = config.clone();
    applied.init_content = init_content.clone();

    println!("Creating container: {}", name);
    DockerClient::run(name, &applied, mount_path, &mounts)?;

    let mut record = applied.to_record(mount_path.to_path_buf(), crate::state::now_string());
    record.image_id = recorded_image_id(&config.docker_image);
    state.insert(name.to_string(), record);
    state.save()?;
    println!("Record saved: {} -> {}", name, mount_path.display());

    persist::template_into_container(
        name,
        &config.docker_image,
        config.template,
        config.persists(Persist::Config),
    )?;
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
    let init_content = read_init(init_path, mount_path, config.init_content.as_deref())?;
    // Resolved before the destroy rather than between it and the run: a failure
    // here would leave the user with no container at all, which is the one
    // outcome a rebuild exists to prevent.
    let mounts = resolve_mounts(name, config)?;

    let mut applied = config.clone();
    applied.init_content = init_content.clone();

    println!("Recreating container: {}", name);
    DockerClient::destroy(name)?;
    DockerClient::run(name, &applied, mount_path, &mounts)?;

    let mut record = applied.to_record(mount_path.to_path_buf(), crate::state::now_string());
    record.image_id = recorded_image_id(&config.docker_image);
    state.insert(name.to_string(), record);
    state.save()?;

    persist::template_into_container(
        name,
        &config.docker_image,
        config.template,
        config.persists(Persist::Config),
    )?;
    run_init_script(name, init_content.as_deref())?;
    Ok(())
}

/// Reach a running container with the agent attached, leaving how it was built
/// alone. Nothing here can change the configuration.
pub fn start(name: &str, config: &Config) -> Result<()> {
    ensure_running(name, config)?;

    println!("Starting Agent: {}", config.agent_name);
    DockerClient::exec(name, &config.agent_name)?;
    Ok(())
}

/// Make sure a recorded container is running, without attaching the agent.
///
/// `up` is the command that attaches; this is for callers that only need the
/// container to be up — currently `update`, which leaves a rebuilt container in
/// the same state `apply` does, with the agent not started. A running container
/// is left alone, a stopped one is restarted, and a missing one was removed
/// outside agentdock, which is the same broken state `up` reports.
pub fn ensure_running(name: &str, config: &Config) -> Result<()> {
    use crate::docker::ContainerStatus;

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
    Ok(())
}

/// The image id to record for `config`, or `None` with a warning if it cannot be
/// read.
///
/// The id is what `update` compares to decide whether a container is current, so
/// losing it only costs one redundant rebuild. That is not worth failing a create
/// that has already started the container, so a failure is reported and the
/// record is written without an id.
fn recorded_image_id(image: &str) -> Option<String> {
    match DockerClient::image_id(image) {
        Ok(id) => Some(id),
        Err(e) => {
            eprintln!("Warning: could not read the image id of {}: {}", image, e);
            None
        }
    }
}

/// The init script's contents, or the configuration's own when no path is given.
///
/// The fallback is what lets `update` rebuild a container without repeating
/// `--init`: it has the recorded script and no path, and passing `None` through
/// would silently drop the script from the rebuilt record.
fn read_init(
    init_path: Option<&str>,
    mount_path: &Path,
    fallback: Option<&str>,
) -> Result<Option<String>> {
    match init_path {
        Some(path) => Ok(Some(read_init_content(path, mount_path)?)),
        None => Ok(fallback.map(str::to_string)),
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

    let mounts = persist::plan_mounts(name, &config.docker_image, kinds, config.template)?;

    for m in &mounts {
        println!("Persisting: {} -> {}", m.host.display(), m.container);
    }

    Ok(mounts)
}
