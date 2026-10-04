use anyhow::{bail, Result};

use crate::cli::ApplyArgs;
use crate::config::{validate_port_mapping, AgentConfig, Config};
use crate::container;
use crate::error::ContainerError;
use crate::state::StateManager;

/// The agent a container gets when neither the command line nor a record says.
const DEFAULT_AGENT: &str = "nixos/pi-agent";

pub fn execute_apply(args: ApplyArgs) -> Result<()> {
    let mount_path = args.get_mount_path();
    let mut state = StateManager::new()?;

    for port in args.opts.port.iter().flatten() {
        validate_port_mapping(port).map_err(|e| anyhow::anyhow!(e))?;
    }

    // Which container this apply is about settles what the flags mean. Given one,
    // they are changes against its record; without one they describe a new
    // container outright.
    let existing = match &args.name {
        Some(name) => state.find_by_name(name).map(|r| (name.clone(), r.clone())),
        None => state
            .find_by_path(&mount_path)
            .map(|(name, record)| (name.to_string(), record.clone())),
    };

    let (name, config, existed) = match existing {
        Some((name, record)) => {
            let agent = agent(&args, Some(&record))?;
            let config = Config::resolve(&args.opts, agent, &record);

            if config == Config::of(&record) && !args.force {
                bail!(ContainerError::AlreadyMatches { name });
            }
            (name, config, true)
        }
        None => {
            let agent = agent(&args, None)?;
            let name = args
                .name
                .clone()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            (name, Config::new(agent, &args.opts), false)
        }
    };

    if existed {
        container::replace(
            &name,
            &config,
            &mount_path,
            args.opts.init.as_deref(),
            &mut state,
        )?;
    } else {
        container::create(
            &name,
            &config,
            &mount_path,
            args.opts.init.as_deref(),
            &mut state,
        )?;
    }

    container::start(&name, &config)?;
    Ok(())
}

/// The agent the command line asks for, or the one already in place.
fn agent(args: &ApplyArgs, record: Option<&crate::state::Record>) -> Result<AgentConfig> {
    match (&args.opts.agent, record) {
        (Some(spec), _) => AgentConfig::parse(spec).map_err(|e| anyhow::anyhow!(e)),
        (None, Some(record)) => Ok(AgentConfig {
            docker_image: record.docker_image.clone(),
            agent_name: record.agent_name.clone(),
        }),
        // Nothing to inherit from, so a default is the only specification left.
        (None, None) => AgentConfig::parse(DEFAULT_AGENT).map_err(|e| anyhow::anyhow!(e)),
    }
}
