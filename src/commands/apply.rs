use anyhow::Result;

use crate::cli::ApplyArgs;
use crate::config::{AgentConfig, Config, validate_port_mapping};
use crate::container;
use crate::state::StateManager;

pub fn execute_apply(args: ApplyArgs) -> Result<()> {
    let mount_path = args.get_mount_path();
    let mut state = StateManager::new()?;

    for port in &args.opts.port {
        validate_port_mapping(port).map_err(|e| anyhow::anyhow!(e))?;
    }

    // The configuration comes from the flags alone. A container found here is
    // replaced rather than merged into, so anything left out is turned off.
    let existing = match &args.name {
        Some(name) => state.find_by_name(name).map(|r| (name.clone(), r.clone())),
        None => state
            .find_by_path(&mount_path)
            .map(|(name, record)| (name.to_string(), record.clone())),
    };

    let agent = AgentConfig::parse(&args.opts.agent).map_err(|e| anyhow::anyhow!(e))?;
    let config = Config::new(agent, &args.opts);

    let (name, existed) = match existing {
        Some((name, _)) => (name, true),
        None => (
            args.name
                .clone()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            false,
        ),
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
