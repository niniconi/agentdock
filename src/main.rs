use anyhow::Result;
use clap::Parser;
use uuid::Uuid;

mod cli;
mod config;
mod container;
mod docker;
mod error;
mod init;
mod state;

use cli::Cli;
use config::RunOptions;
use init::read_init_content;

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mount_path = cli.get_mount_path();
    let agent_config = config::AgentConfig::parse(&cli.agent).map_err(|e| anyhow::anyhow!(e))?;
    let mut state = state::StateManager::new();
    let opts = RunOptions::from(&cli);

    // Read init script content if provided via CLI
    let cli_init_content = match cli.init {
        Some(ref path) => Some(read_init_content(path, &mount_path)?),
        None => None,
    };

    // Deduplication and mode recognition
    let target_name = if let Some(ref name) = cli.name {
        if let Some(record) = state.find_by_name(name) {
            container::handle_existing(
                name,
                record,
                &agent_config,
                &mount_path,
                &opts,
                &cli_init_content,
            )?
        } else {
            name.clone()
        }
    } else {
        if let Some((name, record)) = state.find_by_path(&mount_path) {
            let name = name.to_string();
            container::handle_existing(
                &name,
                record,
                &agent_config,
                &mount_path,
                &opts,
                &cli_init_content,
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
