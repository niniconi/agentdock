use anyhow::{bail, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

use super::types::{AgentConfig, ContainerStatus};

pub struct DockerClient;

impl DockerClient {
    fn herdr_socket_path() -> PathBuf {
        if let Ok(path) = std::env::var("HERDR_SOCKET_PATH") {
            return PathBuf::from(path);
        }
        let home = dirs::home_dir().expect("Failed to determine home directory");
        home.join(".config").join("herdr").join("herdr.sock")
    }

    pub fn run(
        name: &str,
        config: &AgentConfig,
        mount_path: &Path,
        rm: bool,
        herdr_sock: bool,
    ) -> Result<()> {
        let mut args = vec![
            "run".to_string(),
            "-d".to_string(),
            "--name".to_string(),
            name.to_string(),
        ];

        if rm {
            args.push("--rm".to_string());
        }

        // Mount workspace directory
        let mount = format!("{}:/workspace", mount_path.display());
        args.push("-v".to_string());
        args.push(mount);

        // Mount Herdr Unix socket
        if herdr_sock {
            let sock_path = Self::herdr_socket_path();
            let host_sock = sock_path.display().to_string();
            let container_sock = "/tmp/herdr.sock";
            args.push("-v".to_string());
            args.push(format!("{}:{}", host_sock, container_sock));
            args.push("-e".to_string());
            args.push(format!("HERDR_SOCKET_PATH={}", container_sock));
        }

        // Image name
        args.push(config.docker_image.clone());

        // Default entry command: keep container running
        args.push("sleep".to_string());
        args.push("inf".to_string());

        let output = Command::new("docker").args(&args).output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to start container: {}", stderr);
        }

        Ok(())
    }

    pub fn inspect(name: &str) -> Result<ContainerStatus> {
        let output = Command::new("docker")
            .args(["inspect", "-f", "{{.State.Running}}", name])
            .output()?;

        if !output.status.success() {
            return Ok(ContainerStatus::NotFound);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let status_str = stdout.trim();

        match status_str {
            "true" => Ok(ContainerStatus::Running),
            "false" => Ok(ContainerStatus::Stopped),
            _ => Ok(ContainerStatus::NotFound),
        }
    }

    pub fn exists(name: &str) -> bool {
        Command::new("docker")
            .args(["inspect", name])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    pub fn restart(name: &str) -> Result<()> {
        let output = Command::new("docker").args(["restart", name]).output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to restart container: {}", stderr);
        }

        Ok(())
    }

    pub fn exec(name: &str, command: &str) -> Result<()> {
        let status = Command::new("docker")
            .args(["exec", "-it", name, "sh", "-c", command])
            .status()?;

        if !status.success() {
            bail!(
                "Command execution failed, exit code: {}",
                status.code().unwrap_or(-1)
            );
        }

        Ok(())
    }

    pub fn cp(name: &str, src: &Path, dst: &str) -> Result<()> {
        let output = Command::new("docker")
            .args([
                "cp",
                &src.display().to_string(),
                &format!("{}:{}", name, dst),
            ])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to copy file to container: {}", stderr);
        }

        Ok(())
    }

    #[allow(dead_code)]
    pub fn stop(name: &str) -> Result<()> {
        let output = Command::new("docker").args(["stop", name]).output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to stop container: {}", stderr);
        }

        Ok(())
    }
}
