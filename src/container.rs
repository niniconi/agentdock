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
    state: &mut StateManager,
) -> Result<String> {
    let name = name.to_string();

    let status = DockerClient::inspect(&name)?;

    // Prefer CLI --init content over Record's saved content
    let init_content = cli_init_content
        .as_deref()
        .or(record.init_content.as_deref());

    let proxy_changed =
        record.http_proxy != opts.http_proxy || record.https_proxy != opts.https_proxy;
    let ports_changed = record
        .ports
        .as_ref()
        .map_or(!opts.ports.is_empty(), |p| p != &opts.ports);
    // Without this the container keeps running the old image while `exec` is
    // handed the new agent, so the two silently disagree.
    let image_changed = record.docker_image != agent_config.docker_image;
    let agent_changed = record.agent_name != agent_config.agent_name;
    let recreate = proxy_changed || ports_changed || image_changed || agent_changed;

    match status {
        ContainerStatus::Running => {
            if recreate {
                let reason = if image_changed || agent_changed {
                    "Image/agent settings changed"
                } else {
                    "Proxy/port settings changed"
                };
                println!("{}, recreating running container: {}", reason, name);
                DockerClient::stop(&name)?;
                DockerClient::destroy(&name)?;
                DockerClient::run(&name, agent_config, mount_path, opts)?;
                run_init_script(&name, init_content)?;

                let mut updated = record.clone();
                updated.http_proxy = opts.http_proxy.clone();
                updated.https_proxy = opts.https_proxy.clone();
                updated.ports = if opts.ports.is_empty() {
                    None
                } else {
                    Some(opts.ports.clone())
                };
                updated.docker_image = agent_config.docker_image.clone();
                updated.agent_name = agent_config.agent_name.clone();
                state.insert(name.clone(), updated);
                state.save()?;
            }
            DockerClient::exec(&name, &agent_config.agent_name)?;
            Ok(name)
        }
        ContainerStatus::Stopped => {
            if recreate {
                let reason = if image_changed || agent_changed {
                    "Image/agent settings changed"
                } else {
                    "Proxy/port settings changed"
                };
                println!("{}, recreating stopped container: {}", reason, name);
                DockerClient::destroy(&name)?;
                DockerClient::run(&name, agent_config, mount_path, opts)?;
                run_init_script(&name, init_content)?;

                let mut updated = record.clone();
                updated.http_proxy = opts.http_proxy.clone();
                updated.https_proxy = opts.https_proxy.clone();
                updated.ports = if opts.ports.is_empty() {
                    None
                } else {
                    Some(opts.ports.clone())
                };
                updated.docker_image = agent_config.docker_image.clone();
                updated.agent_name = agent_config.agent_name.clone();
                state.insert(name.clone(), updated);
                state.save()?;
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
            ports: if opts.ports.is_empty() {
                None
            } else {
                Some(opts.ports.clone())
            },
            docker_image: agent_config.docker_image.clone(),
            agent_name: agent_config.agent_name.clone(),
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
