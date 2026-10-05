use anyhow::Result;
use std::path::PathBuf;

use crate::config::Persist;
use crate::docker::DockerClient;
use crate::error::ContainerError;

/// Where each supported agent keeps its directories, relative to its home.
///
/// Every entry is mounted for every container rather than looked up by image or
/// by the agent named in `-a`. A container entered with `bash` is one whose
/// agent has not been started yet, and mounting all of them means the
/// directories are already there when it is. It also means adding an agent
/// needs no code change here, which keying on the image did: that put `nixos`,
/// `nixos:latest` and `ghcr.io/x/nixos` in three separate arms, so pinning a
/// tag meant editing this file and shipping it.
const SUPPORTED: &[(&str, Persist, &str)] = &[
    ("opencode", Persist::Config, ".config/opencode"),
    ("opencode", Persist::Data, ".local/share/opencode"),
];

/// One resolved bind mount: a host directory and where it appears inside.
pub struct Mount {
    pub host: PathBuf,
    pub container: String,
}

/// Build the bind mounts for one container, creating the host directories.
///
/// The host side is `<data home>/agentdock/<container>/<agent>/<what>`, so two
/// containers never share a directory. That matters for `data`, which is a
/// database: two agents writing one SQLite file means one of them fails on the
/// lock, and `worktree add` builds a container per branch.
///
/// The container side comes from the image, because a mount target has to be an
/// absolute path and the image is the only thing that knows its own home
/// directory. Nothing here is asked for on the command line: the user says
/// which directories, not where they are.
pub fn plan_mounts(container: &str, image: &str, kinds: &[Persist]) -> Result<Vec<Mount>> {
    let wanted: Vec<&(&str, Persist, &str)> = SUPPORTED
        .iter()
        .filter(|(_, kind, _)| kinds.contains(kind))
        .collect();

    if wanted.is_empty() {
        return Ok(Vec::new());
    }

    let base = container_data_dir(container)?;

    // Resolved once for the whole container. Every directory hangs off the same
    // home, so asking per directory would call docker once per mount and could
    // come back with two different answers.
    let home = container_home(image)?;
    let home = home.trim_end_matches('/');

    let mut mounts = Vec::new();
    for (agent, kind, rel) in wanted {
        let host = base.join(agent).join(kind.as_str());
        // Created here rather than left to docker, which would leave the
        // directory owned by root on the host when the container runs as root.
        std::fs::create_dir_all(&host).map_err(|e| {
            anyhow::Error::new(e).context(format!(
                "Failed to create persistence directory: {}",
                host.display()
            ))
        })?;

        mounts.push(Mount {
            host,
            container: format!("{home}/{rel}"),
        });
    }

    Ok(mounts)
}

/// The directory `container` keeps its persisted data under.
///
/// This is what `delete --purge` removes, so it has to be the base
/// `plan_mounts` writes into rather than a second derivation of it.
pub fn container_data_dir(container: &str) -> Result<PathBuf> {
    Ok(agentdock_data_home()?.join("agentdock").join(container))
}

/// The host paths `container` would have persisted, relative to that base.
///
/// `list -v` reports these rather than the full paths: one line per supported
/// agent, and an absolute path repeated once per agent would stretch the table
/// past the terminal. The base is printed once by the caller.
pub fn container_data_dirs(container: &str, kinds: &[Persist]) -> Vec<String> {
    SUPPORTED
        .iter()
        .filter(|(_, kind, _)| kinds.contains(kind))
        .map(|(agent, kind, _)| format!("{container}/{agent}/{}", kind.as_str()))
        .collect()
}

/// The home directory agentdock keeps its own data under on the host.
///
/// XDG rather than a bare `~/`, so a user who has set `XDG_DATA_HOME` already
/// expects this to land there. Public because `list -v` reports the same paths,
/// and the two must not be able to disagree.
pub fn agentdock_data_home() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME") {
        let dir = PathBuf::from(dir);
        if dir.is_absolute() {
            return Ok(dir);
        }
    }
    Ok(home_dir()?.join(".local").join("share"))
}

/// The invoking user's home directory.
///
/// Read from the environment rather than from the passwd database, because
/// agentdock runs on the host where the invoking user's `HOME` is what a shell
/// would expand `~` to.
fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or(ContainerError::NoHome.into())
}

/// The home directory of the user `image` runs as.
///
/// An image with no `USER` runs as root, and root's home is `/root` rather than
/// `/home/root`, which is the one case where the username does not predict the
/// path and the guess is safe.
///
/// Beyond that the image's own `HOME` is used. The authority here would be
/// `/etc/passwd`, since that is what a shell's `~` and an agent's `getpwuid`
/// both resolve to, but reading it means copying a file out of the image and
/// this has no way to test that. An image declaring neither `USER` nor `HOME`
/// says nothing about where it writes, so that is an error rather than a guess.
fn container_home(image: &str) -> Result<String> {
    let cfg = DockerClient::image_config(image)?;

    let user = cfg.user();
    if user.is_empty() || user == "root" || user == "0" {
        return Ok("/root".to_string());
    }

    if let Some(home) = cfg.env_home().filter(|h| !h.is_empty()) {
        return Ok(home);
    }

    // A named user with no HOME declared. `/home/<user>` is the convention most
    // images follow, but it is a convention, so say which one was used.
    eprintln!("Note: image {image} declares no HOME for user '{user}', assuming /home/{user}.");
    eprintln!("      If the agent writes elsewhere, nothing is persisted silently.");
    Ok(format!("/home/{user}"))
}
