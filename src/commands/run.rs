use anyhow::Result;
use uuid::Uuid;

use crate::cli::RunArgs;
use crate::config::{validate_port_mapping, AgentConfig, RunOptions};
use crate::container;
use crate::init::read_init_content;
use crate::state::StateManager;

pub fn execute_run(args: RunArgs) -> Result<()> {
    let mount_path = args.get_mount_path();
    let agent_config = AgentConfig::parse(&args.opts.agent).map_err(|e| anyhow::anyhow!(e))?;
    let mut state = StateManager::new();
    let opts = RunOptions::from(&args.opts);

    // Validate port mappings
    for port in &opts.ports {
        validate_port_mapping(port).map_err(|e| anyhow::anyhow!(e))?;
    }

    // Read init script content if provided via CLI
    let cli_init_content = match &args.opts.init {
        Some(path) => Some(read_init_content(path, &mount_path)?),
        None => None,
    };

    // Deduplication and mode recognition
    let target_name = if let Some(ref name) = args.name {
        if let Some(record) = state.find_by_name(name) {
            let record = record.clone();
            container::handle_existing(
                name,
                &record,
                &agent_config,
                &mount_path,
                &opts,
                &cli_init_content,
                &mut state,
            )?
        } else {
            name.clone()
        }
    } else {
        if let Some((name, record)) = state.find_by_path(&mount_path) {
            let name = name.to_string();
            let record = record.clone();
            container::handle_existing(
                &name,
                &record,
                &agent_config,
                &mount_path,
                &opts,
                &cli_init_content,
                &mut state,
            )?
        } else {
            Uuid::new_v4().to_string()
        }
    };

    // First startup flow
    if state.find_by_name(&target_name).is_none() {
        container::start_new(
            &target_name,
            &agent_config,
            &mount_path,
            &opts,
            &cli_init_content,
            &mut state,
        )?;
    }

    Ok(())
}
