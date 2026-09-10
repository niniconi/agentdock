use anyhow::{bail, Context, Result};
use clap::Parser;
use std::path::Path;
use uuid::Uuid;

mod cli;
mod docker;
mod error;
mod state;

use cli::Cli;
use docker::{AgentConfig, ContainerStatus, DockerClient, RunOptions};
use state::{Record, StateManager};

fn run_init_script(name: &str, init_content: Option<&str>) -> Result<()> {
    if let Some(content) = init_content {
        let tmp_dir = std::env::temp_dir();
        std::fs::create_dir_all(&tmp_dir)
            .with_context(|| format!("Failed to create temp dir: {}", tmp_dir.display()))?;

        let tmp_path = tmp_dir.join(format!("agentdock_init_{}.sh", Uuid::new_v4()));
        std::fs::write(&tmp_path, content)
            .with_context(|| format!("Failed to write temp file: {}", tmp_path.display()))?;

        println!("Copying init script to container...");
        DockerClient::cp(name, &tmp_path, "/tmp/init.sh")
            .with_context(|| format!("Failed to copy init script from: {}", tmp_path.display()))?;

        println!("Executing init script...");
        DockerClient::exec(name, "chmod +x /tmp/init.sh && /tmp/init.sh")?;

        let _ = std::fs::remove_file(&tmp_path);
    }
    Ok(())
}

fn read_init_content(init_path: &str, mount_path: &Path) -> Result<String> {
    let path = std::path::Path::new(init_path);
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        mount_path.join(path)
    };
    std::fs::read_to_string(&abs)
        .with_context(|| format!("Failed to read init script: {}", abs.display()))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mount_path = cli.get_mount_path();
    let agent_config = AgentConfig::parse(&cli.agent).map_err(|e| anyhow::anyhow!(e))?;
    let mut state = StateManager::new();

    let run_opts = RunOptions {
        rm: cli.rm,
        herdr_sock: cli.herdr_sock,
        kvm: cli.kvm,
        http_proxy: cli.http_proxy.clone(),
        https_proxy: cli.https_proxy.clone(),
    };

    // Read init script content if provided via CLI
    let cli_init_content = match cli.init {
        Some(ref path) => Some(read_init_content(path, &mount_path)?),
        None => None,
    };

    // Deduplication and mode recognition
    let target_name = if let Some(ref name) = cli.name {
        // Has --name, search directly
        if let Some(record) = state.find_by_name(name) {
            handle_existing(
                name,
                record,
                &agent_config,
                &mount_path,
                &run_opts,
                &cli_init_content,
            )?
        } else {
            name.clone()
        }
    } else {
        // No --name, search for path match
        if let Some((name, record)) = state.find_by_path(&mount_path) {
            let name = name.to_string();
            handle_existing(
                &name,
                record,
                &agent_config,
                &mount_path,
                &run_opts,
                &cli_init_content,
            )?
        } else {
            // Not found, generate UUID
            Uuid::new_v4().to_string()
        }
    };

    // First startup flow
    if state.find_by_name(&target_name).is_none() {
        println!("Starting new container: {}", target_name);

        DockerClient::run(&target_name, &agent_config, &mount_path, &run_opts)?;

        // Persistent storage
        if !run_opts.rm {
            let record = Record {
                path: mount_path.clone(),
                created_at: state::now_string(),
                init_content: cli_init_content.clone(),
                http_proxy: cli.http_proxy.clone(),
                https_proxy: cli.https_proxy.clone(),
            };
            state.insert(target_name.clone(), record);
            state.save()?;
            println!("Record saved: {} -> {}", target_name, mount_path.display());
        }

        // Execute initialization script
        run_init_script(&target_name, cli_init_content.as_deref())?;

        // Launch Agent
        println!("Starting Agent: {}", agent_config.agent_name);
        DockerClient::exec(&target_name, &agent_config.agent_name)?;
    }

    Ok(())
}

fn handle_existing(
    name: &str,
    record: &Record,
    agent_config: &AgentConfig,
    mount_path: &std::path::Path,
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
