use anyhow::{bail, Result};
use std::path::Path;
use std::process::{Command, Output};

use super::types::ContainerStatus;
use crate::config::Config;
use crate::error::ContainerError;

/// Run docker, reporting a missing binary separately from a command failure.
///
/// Without this, an absent docker surfaces as "No such file or directory",
/// which reads as if the container had no command rather than docker missing.
fn docker(args: &[&str]) -> Result<Output> {
    Command::new("docker").args(args).output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ContainerError::ToolMissing.into()
        } else {
            anyhow::Error::new(e).context("Failed to execute docker")
        }
    })
}

/// Bail with the docker command's own stderr, trimmed of its trailing newline.
fn check(op: &'static str, output: &Output) -> Result<()> {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(ContainerError::Command {
            op,
            stderr: stderr.trim().to_string(),
        });
    }
    Ok(())
}

pub struct DockerClient;

impl DockerClient {
    /// Create a container from `config`, serving `mount_path` at /workspace.
    ///
    /// The mount path is not part of the configuration: it locates the
    /// container, while the config describes how it is built.
    pub fn run(name: &str, config: &Config, mount_path: &Path) -> Result<()> {
        let mut args = vec![
            "run".to_string(),
            "-d".to_string(),
            "--name".to_string(),
            name.to_string(),
        ];

        // Mount workspace directory
        let mount = format!("{}:/workspace", mount_path.display());
        args.push("-v".to_string());
        args.push(mount);

        // Mount host /dev/kvm for KVM virtualization
        if config.kvm {
            args.push("--device".to_string());
            args.push("/dev/kvm".to_string());
        }

        // Set proxy environment variables
        if let Some(ref proxy) = config.http_proxy {
            args.push("-e".to_string());
            args.push(format!("HTTP_PROXY={}", proxy));
            args.push("-e".to_string());
            args.push(format!("http_proxy={}", proxy));
        }
        if let Some(ref proxy) = config.https_proxy {
            args.push("-e".to_string());
            args.push(format!("HTTPS_PROXY={}", proxy));
            args.push("-e".to_string());
            args.push(format!("https_proxy={}", proxy));
        }

        // Port mappings
        for port in &config.ports {
            args.push("-p".to_string());
            args.push(port.clone());
        }

        // Image name
        args.push(config.docker_image.clone());

        // Default entry command: keep container running
        args.push("sleep".to_string());
        args.push("inf".to_string());

        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let output = docker(&refs)?;
        check("start container", &output)
    }

    pub fn inspect(name: &str) -> Result<ContainerStatus> {
        let output = docker(&["inspect", "-f", "{{.State.Running}}", name])?;

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

    pub fn restart(name: &str) -> Result<()> {
        let output = docker(&["restart", name])?;
        check("restart container", &output)
    }

    pub fn exec(name: &str, command: &str) -> Result<()> {
        let status = Command::new("docker")
            .args(["exec", "-it", name, "sh", "-c", command])
            .status()?;

        if !status.success() {
            bail!(ContainerError::ExecFailed {
                code: status.code().unwrap_or(-1),
            });
        }

        Ok(())
    }

    pub fn cp(name: &str, src: &Path, dst: &str) -> Result<()> {
        let src = src.display().to_string();
        let target = format!("{}:{}", name, dst);
        let output = docker(&["cp", &src, &target])?;
        check("copy file to container", &output)
    }

    #[allow(dead_code)]
    pub fn stop(name: &str) -> Result<()> {
        let output = docker(&["stop", name])?;
        check("stop container", &output)
    }

    pub fn destroy(name: &str) -> Result<()> {
        let output = docker(&["rm", "-f", name])?;
        check("remove container", &output)
    }
}
