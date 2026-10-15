use anyhow::Result;

use crate::cli::UpdateArgs;
use crate::config::Config;
use crate::container;
use crate::docker::{ContainerStatus, DockerClient, short_image_id};
use crate::error::ContainerError;
use crate::state::StateManager;

/// Move a container onto the image its tag currently points at, keeping the
/// configuration it was created with.
///
/// The three existing paths each stop short of this. `apply` rebuilds from the
/// flags it is given and never pulls, so it can neither refresh a tag nor keep a
/// configuration the user does not retype — and a flag left out is turned off.
/// `up` reads the record but never rebuilds. `docker run` uses the local tag and
/// never fetches. This command fills the gap: the configuration comes from the
/// record, the image is pulled, and the container is recreated only when the
/// image actually changed.
pub fn execute_update(args: UpdateArgs) -> Result<()> {
    let mount_path = args.get_mount_path();
    let mut state = StateManager::new()?;

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
        return Err(ContainerError::NotManagedForUpdate { name, flag }.into());
    };

    // The configuration comes from the record, not from flags: an update changes
    // the image and nothing else, so the ports, environment, proxied directories
    // and persisted directories the container was created with are all kept.
    let config = Config::of(&record);

    // A record whose container is gone is the same broken state `up` reports.
    // Checked before the rebuild so that path fails the one way, rather than
    // letting the `docker rm` inside `replace` surface its own message.
    if let ContainerStatus::NotFound = DockerClient::inspect(&name)? {
        return Err(ContainerError::NotFound {
            name,
            image: config.docker_image,
            agent: config.agent_name,
        }
        .into());
    }

    if args.no_pull {
        println!("Using local image: {}", config.docker_image);
    } else {
        println!("Pulling image: {}", config.docker_image);
        DockerClient::pull(&config.docker_image)?;
    }

    // Resolved after the pull, so it is the image the tag now points at and not
    // the one that happened to be cached before it.
    let current = DockerClient::image_id(&config.docker_image)?;

    // The record stores the id the container was built from. When it still
    // matches the tag there is nothing to move onto, so the container is only
    // made sure to be running, and its identity is left alone.
    if record.image_id.as_deref() == Some(current.as_str()) {
        println!(
            "Container '{}' already runs image '{}' ({})",
            name,
            config.docker_image,
            short_image_id(&current)
        );
        if !args.dry_run {
            container::ensure_running(&name, &config)?;
        }
        return Ok(());
    }

    if args.dry_run {
        println!(
            "Container '{}' would be recreated onto image '{}' ({})",
            name,
            config.docker_image,
            short_image_id(&current)
        );
        return Ok(());
    }

    // Rebuilt from the record, so the mount path is the one recorded rather than
    // the directory this command happened to run from. `docker run -d` leaves the
    // new container running, the same state `apply` leaves, so there is nothing
    // to start; attaching the agent is `up`'s job.
    container::replace(&name, &config, &record.path, None, &mut state)?;
    Ok(())
}
