use anyhow::{bail, Result};
use std::path::Path;

use crate::config::{AgentConfig, RunOptions};
use crate::docker::{ContainerStatus, DockerClient};
use crate::error::ContainerError;
use crate::init::run_init_script;
use crate::state::{Record, StateManager};

/// A setting the existing container was built with that no longer matches the
/// one being asked for. The variants are the settings that force a rebuild
/// rather than a restart, so they appear in `detect` and in the record write
/// back inside `recreate_container`, and those two have to stay in step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drift {
    Image,
    Agent,
    Ports,
    HttpProxy,
    HttpsProxy,
}

impl Drift {
    fn label(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Agent => "agent",
            Self::Ports => "ports",
            Self::HttpProxy => "http proxy",
            Self::HttpsProxy => "https proxy",
        }
    }
}

/// What the container was built with that differs from what is being asked for
/// now. Empty means the existing container is still correct and only has to be
/// reached, not rebuilt.
fn detect(record: &Record, agent_config: &AgentConfig, opts: &RunOptions) -> Vec<Drift> {
    let mut drift = Vec::new();

    // Without the image and the agent the container keeps running the old
    // image while `exec` is handed the new agent, so the two silently disagree.
    if record.docker_image != agent_config.docker_image {
        drift.push(Drift::Image);
    }
    if record.agent_name != agent_config.agent_name {
        drift.push(Drift::Agent);
    }
    // None, Some(vec![]) and an empty opts.ports all mean "no ports". Flattening
    // the two sides before comparing says that directly, where the previous
    // map_or matched None against opts and Some against opts by different rules.
    if record.ports.as_deref().unwrap_or_default() != opts.ports.as_slice() {
        drift.push(Drift::Ports);
    }
    if record.http_proxy != opts.http_proxy {
        drift.push(Drift::HttpProxy);
    }
    if record.https_proxy != opts.https_proxy {
        drift.push(Drift::HttpsProxy);
    }

    drift
}

/// Replace the container and record what it was built from, so the next run
/// sees no drift for a container that is already correct.
fn recreate_container(
    name: &str,
    record: &Record,
    agent_config: &AgentConfig,
    mount_path: &Path,
    opts: &RunOptions,
    init_content: Option<&str>,
    state: &mut StateManager,
) -> Result<()> {
    DockerClient::destroy(name)?;
    DockerClient::run(name, agent_config, mount_path, opts)?;
    run_init_script(name, init_content)?;

    let mut updated = record.clone();
    updated.docker_image = agent_config.docker_image.clone();
    updated.agent_name = agent_config.agent_name.clone();
    updated.ports = (!opts.ports.is_empty()).then(|| opts.ports.clone());
    updated.http_proxy = opts.http_proxy.clone();
    updated.https_proxy = opts.https_proxy.clone();
    state.insert(name.to_string(), updated);
    state.save()
}

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
    if matches!(status, ContainerStatus::NotFound) {
        bail!(ContainerError::NotFound { name });
    }

    // Prefer CLI --init content over Record's saved content
    let init_content = cli_init_content
        .as_deref()
        .or(record.init_content.as_deref());

    // A running container has to be stopped before it can be removed; a stopped
    // one is already down, and is the only case that can just be restarted.
    let stopped = matches!(status, ContainerStatus::Stopped);

    let drift = detect(record, agent_config, opts);

    if drift.is_empty() {
        if stopped {
            println!("Restarting stopped container: {}", name);
            DockerClient::restart(&name)?;
        }
    } else {
        println!(
            "{} changed, recreating {} container: {}",
            drift
                .iter()
                .map(|d| d.label())
                .collect::<Vec<_>>()
                .join(", "),
            if stopped { "stopped" } else { "running" },
            name
        );
        if !stopped {
            DockerClient::stop(&name)?;
        }
        recreate_container(
            &name,
            record,
            agent_config,
            mount_path,
            opts,
            init_content,
            state,
        )?;
    }

    DockerClient::exec(&name, &agent_config.agent_name)?;
    Ok(name)
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
