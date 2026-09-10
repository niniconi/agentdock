use anyhow::{bail, Result};
use std::path::Path;

use crate::config::{AgentConfig, RunOptions};
use crate::docker::{ContainerStatus, DockerClient};
use crate::error;
use crate::init::run_init_script;
use crate::state::{Record, StateManager};

pub fn handle_existing(
    name: &str,
    record: &Record,
    agent_config: &AgentConfig,
    mount_path: &Path,
    opts: &RunOptions,
    cli_init_content: &Option<String>,
) -> Result<String> {
    let name = name.to_string();

    let status = DockerClient::inspect(&name)?;

    // Prefer CLI --init content over Record's saved content
    let init_content = cli_init_content
        .as_deref()
        .or(record.init_content.as_deref());

    match status {
        ContainerStatus::Running => {
            let proxy_changed =
                record.http_proxy != opts.http_proxy || record.https_proxy != opts.https_proxy;
            if proxy_changed {
                println!(
                    "Proxy settings changed, recreating running container: {}",
                    name
                );
                DockerClient::stop(&name)?;
                DockerClient::destroy(&name)?;
                DockerClient::run(&name, agent_config, mount_path, opts)?;
                run_init_script(&name, init_content)?;
            }
            DockerClient::exec(&name, &agent_config.agent_name)?;
            Ok(name)
        }
        ContainerStatus::Stopped => {
            let proxy_changed =
                record.http_proxy != opts.http_proxy || record.https_proxy != opts.https_proxy;
            if proxy_changed {
                println!(
                    "Proxy settings changed, recreating stopped container: {}",
                    name
                );
                DockerClient::destroy(&name)?;
                DockerClient::run(&name, agent_config, mount_path, opts)?;
                run_init_script(&name, init_content)?;
            } else {
                println!("Restarting stopped container: {}", name);
                DockerClient::restart(&name)?;
            }
            DockerClient::exec(&name, &agent_config.agent_name)?;
            Ok(name)
        }
        ContainerStatus::NotFound => {
            bail!("{}", error::container_not_found_error(&name));
        }
    }
}

pub fn start_new(
    name: &str,
    agent_config: &AgentConfig,
    mount_path: &Path,
    opts: &RunOptions,
    cli_init_content: &Option<String>,
    state: &mut StateManager,
) -> Result<()> {
    println!("Starting new container: {}", name);

    DockerClient::run(name, agent_config, mount_path, opts)?;

    // Persistent storage
    if !opts.rm {
        let record = Record {
            path: mount_path.to_path_buf(),
            created_at: crate::state::now_string(),
            init_content: cli_init_content.clone(),
            http_proxy: opts.http_proxy.clone(),
            https_proxy: opts.https_proxy.clone(),
        };
        state.insert(name.to_string(), record);
        state.save()?;
        println!("Record saved: {} -> {}", name, mount_path.display());
    }

    // Execute initialization script
    run_init_script(name, cli_init_content.as_deref())?;

    // Launch Agent
    println!("Starting Agent: {}", agent_config.agent_name);
    DockerClient::exec(name, &agent_config.agent_name)?;

    Ok(())
}
