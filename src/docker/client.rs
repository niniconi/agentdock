use anyhow::{Result, bail};
use serde::Deserialize;
use std::io::IsTerminal;
use std::path::Path;
use std::process::{Command, Output};

use super::types::ContainerStatus;
use crate::config::Config;
use crate::error::ContainerError;
use crate::persist::Mount;

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
    /// Create a container from `config`, serving `mount_path` at /workspace and
    /// each of `mounts` where it says.
    ///
    /// The mount path is not part of the configuration: it locates the
    /// container, while the config describes how it is built.
    pub fn run(name: &str, config: &Config, mount_path: &Path, mounts: &[Mount]) -> Result<()> {
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

        // Persisted agent directories, already resolved to absolute paths on
        // both sides by the caller. Ordering is the caller's, so the command is
        // reproducible for a given configuration.
        for m in mounts {
            args.push("-v".to_string());
            args.push(format!("{}:{}", m.host.display(), m.container));
        }

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

        // User-supplied environment variables
        for env in &config.envs {
            args.push("-e".to_string());
            args.push(env.clone());
        }

        // Resource limits
        if let Some(memory) = &config.memory {
            args.push("--memory".to_string());
            args.push(memory.clone());
        }
        if let Some(cpus) = &config.cpus {
            args.push("--cpus".to_string());
            args.push(cpus.clone());
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

    /// Run a one-shot command inside the container, without a terminal.
    ///
    /// Init scripts are not a user sitting at a prompt, so `-it` would only
    /// make them fail anywhere without a TTY (CI, piped shells).
    pub fn exec_script(name: &str, command: &str) -> Result<()> {
        let status = Command::new("docker")
            .args(["exec", name, "sh", "-c", command])
            .status()?;

        if !status.success() {
            bail!(ContainerError::ExecFailed {
                code: status.code().unwrap_or(-1),
            });
        }

        Ok(())
    }

    pub fn exec(name: &str, command: &str) -> Result<()> {
        let mut args = vec!["exec"];
        if std::io::stdin().is_terminal() {
            args.push("-it");
        }
        let status = Command::new("docker")
            .args(args)
            .args([name, "sh", "-c", command])
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

    /// The parts of an image's config that say which user it runs as.
    ///
    /// Read with a single `docker image inspect` rather than one call per field:
    /// this is on the `apply` path and each call is a round trip to the daemon.
    /// Pulling first mirrors what `docker run` would have done anyway, so an
    /// image that is not present locally is still usable.
    pub fn image_config(image: &str) -> Result<ImageConfig> {
        match image_config_once(image) {
            Ok(cfg) => Ok(cfg),
            Err(first) => {
                // `docker image inspect` fails for a missing image and for a
                // daemon it cannot reach. Only the first is worth retrying, and
                // only after pulling.
                Self::pull(image)?;
                image_config_once(image).map_err(|_| first)
            }
        }
    }
}

/// One `docker image inspect -f '{{json .Config}}'`, with no pull attempted.
fn image_config_once(image: &str) -> Result<ImageConfig> {
    let output = docker(&["image", "inspect", "-f", "{{json .Config}}", image])?;

    if !output.status.success() {
        bail!(ContainerError::ImageInspectFailed {
            image: image.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    // An empty body here means docker exited 0 without saying anything, which
    // it does for an image it cannot resolve. Reporting that as the image's own
    // failure keeps the message about the image rather than about JSON.
    let raw = serde_json::from_slice::<ImageConfig>(&output.stdout).map_err(|_| {
        anyhow::Error::new(ContainerError::ImageInspectFailed {
            image: image.to_string(),
            stderr: "docker reported no configuration for it".to_string(),
        })
    })?;

    Ok(raw)
}

impl DockerClient {
    fn pull(image: &str) -> Result<()> {
        let output = docker(&["pull", image])?;
        check("pull image", &output)
    }
}

/// An image's `.Config`, as far as agentdock cares.
///
/// The rename is not optional. `docker image inspect -f '{{json .Config}}'`
/// serializes a Go struct, so the keys are its **field names**: `User`, `Env`.
/// Serde matches field names exactly, and it ignores keys it does not recognise
/// rather than failing, so without this every field would deserialize as `None`
/// for every real image — an image declaring `USER=node HOME=/home/node` would
/// look the same as one declaring nothing, and both would be taken as root.
///
/// Every field is optional because an image is free to declare none of them:
/// a `scratch`-based image in particular has no `User` and no `Env`, and an
/// absent `User` is meaningful rather than missing, since docker then runs the
/// container as root.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ImageConfig {
    user: Option<String>,
    #[serde(default)]
    env: Option<Vec<String>>,
}

impl ImageConfig {
    /// The user the image runs as, or an empty string when it declares none.
    ///
    /// Absent is not the same as unset: docker runs a container as root when
    /// the image names no user, so the caller needs to tell the two apart only
    /// to know that root is the answer either way.
    pub fn user(&self) -> &str {
        self.user.as_deref().unwrap_or_default()
    }

    /// The `HOME` the image declares, if it declares one.
    pub fn env_home(&self) -> Option<String> {
        self.env
            .as_ref()?
            .iter()
            .find_map(|entry| entry.strip_prefix("HOME=").map(str::to_string))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The keys are a Go struct's field names, so a lowercase fixture would
    /// deserialize as an image that declares nothing at all — silently, because
    /// serde ignores keys it does not recognise. Every fixture here is therefore
    /// spelled the way `docker image inspect -f '{{json .Config}}'` prints it.
    const REAL: &str = r#"{"User":"node","Env":["PATH=/usr/bin","HOME=/home/node"]}"#;

    #[test]
    fn reads_the_keys_docker_actually_prints() {
        let cfg: ImageConfig = serde_json::from_str(REAL).expect("parse");
        assert_eq!(cfg.user(), "node");
        assert_eq!(cfg.env_home().as_deref(), Some("/home/node"));
    }

    #[test]
    fn lowercase_keys_do_not_parse_into_anything() {
        // Not a test of correct behaviour, but of the failure this guards
        // against: had the struct lacked `rename_all`, this is the shape the
        // code would have seen, and every path would have fallen back to root.
        let cfg: ImageConfig =
            serde_json::from_str(r#"{"user":"node","env":["HOME=/home/node"]}"#).expect("parse");
        assert_eq!(cfg.user(), "");
        assert_eq!(cfg.env_home(), None);
    }

    #[test]
    fn an_image_declaring_nothing_is_not_an_error() {
        let cfg: ImageConfig = serde_json::from_str("{}").expect("parse");
        assert_eq!(cfg.user(), "");
        assert_eq!(cfg.env_home(), None);
    }
}
