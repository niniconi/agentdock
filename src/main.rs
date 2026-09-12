use anyhow::Result;
use clap::Parser;
use uuid::Uuid;

mod cli;
mod commands;
mod config;
mod container;
mod docker;
mod error;
mod init;
mod state;

use cli::{Commands, RunArgs};
use config::{validate_port_mapping, RunOptions};
use init::read_init_content;

fn main() -> Result<()> {
    let cli = cli::Cli::parse();

    match cli.command {
        Commands::Run(args) => run(args),
        Commands::List(args) => commands::execute_list(args),
        Commands::Delete(args) => commands::execute_delete(args),
        Commands::Status(args) => commands::execute_status(args),
    }
}

fn run(args: RunArgs) -> Result<()> {
    let mount_path = args.get_mount_path();
    let agent_config = config::AgentConfig::parse(&args.agent).map_err(|e| anyhow::anyhow!(e))?;
    let mut state = state::StateManager::new();
    let opts = RunOptions::from(&args);

    // Validate port mappings
    for port in &opts.ports {
        validate_port_mapping(port).map_err(|e| anyhow::anyhow!(e))?;
    }

    // Read init script content if provided via CLI
    let cli_init_content = match args.init {
        Some(ref path) => Some(read_init_content(path, &mount_path)?),
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
