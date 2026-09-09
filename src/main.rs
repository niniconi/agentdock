use anyhow::{bail, Result};
use clap::Parser;
use uuid::Uuid;

mod cli;
mod docker;
mod error;
mod state;

use cli::Cli;
use docker::{AgentConfig, ContainerStatus, DockerClient};
use state::{Record, StateManager};

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mount_path = cli.get_mount_path();
    let agent_config = AgentConfig::parse(&cli.agent).map_err(|e| anyhow::anyhow!(e))?;
    let mut state = StateManager::new();

    // Deduplication and mode recognition
    let target_name = if let Some(ref name) = cli.name {
        // Has --name, search directly
        if let Some(record) = state.find_by_name(name) {
            handle_existing(name, record, &agent_config)?
        } else {
            name.clone()
        }
    } else {
        // No --name, search for path match
        if let Some((name, record)) = state.find_by_path(&mount_path) {
            let name = name.to_string();
            handle_existing(&name, record, &agent_config)?
        } else {
            // Not found, generate UUID
            Uuid::new_v4().to_string()
        }
    };

    // First startup flow
    if !DockerClient::exists(&target_name) {
        println!("Starting new container: {}", target_name);

        DockerClient::run(&target_name, &agent_config, &mount_path, cli.rm)?;

        // Persistent storage
        if !cli.rm {
            let record = Record {
                path: mount_path.clone(),
                created_at: state::now_string(),
            };
            state.insert(target_name.clone(), record);
            state.save()?;
            println!("Record saved: {} -> {}", target_name, mount_path.display());
        }

        // Execute initialization script
        if let Some(ref init_script) = cli.init {
            let init_path = std::path::Path::new(init_script);
            let init_abs = if init_path.is_absolute() {
                init_path.to_path_buf()
            } else {
                mount_path.join(init_path)
            };

            if !init_abs.exists() {
                bail!("Init script does not exist: {}", init_abs.display());
            }

            println!("Copying init script to container...");
            DockerClient::cp(&target_name, &init_abs, "/tmp/init.sh")?;

            println!("Executing init script...");
            DockerClient::exec(&target_name, "chmod +x /tmp/init.sh && /tmp/init.sh")?;
        }

        // Launch Agent
        println!("Starting Agent: {}", agent_config.agent_name);
        DockerClient::exec(&target_name, &agent_config.agent_name)?;
    }

    Ok(())
}

fn handle_existing(name: &str, _record: &Record, agent_config: &AgentConfig) -> Result<String> {
    let name = name.to_string();

    let status = DockerClient::inspect(&name)?;

    match status {
        ContainerStatus::Running => {
            // Still try to launch Agent (user may need restart)
            DockerClient::exec(&name, &agent_config.agent_name)?;
            Ok(name)
        }
        ContainerStatus::Stopped => {
            println!("Restarting stopped container: {}", name);
            // Restart to bring container back to running state, then launch Agent
            DockerClient::restart(&name)?;
            DockerClient::exec(&name, &agent_config.agent_name)?;
            Ok(name)
        }
        ContainerStatus::NotFound => {
            bail!("{}", error::container_not_found_error(&name));
        }
    }
}
