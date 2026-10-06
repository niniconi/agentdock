use anyhow::{Result, bail};
use std::path::{Component, Path, PathBuf};

use crate::config::Persist;
use crate::docker::DockerClient;
use crate::error::ContainerError;

/// Where each supported agent keeps its directories, relative to its home.
///
/// Every entry is mounted for every container that asked for its kind, rather
/// than looked up by image or by the agent named in `-a`. A container entered
/// with `bash` is one whose agent has not been started yet, and mounting all of
/// them means the directories are already there when it is. The image is
/// consulted only for its home directory, which is why it never appears here:
/// keying on the image put `nixos`, `nixos:latest` and `ghcr.io/x/nixos` in three
/// separate arms, so pinning a tag meant editing this file and shipping it.
///
/// One entry per directory, so adding an agent is one line per directory it
/// keeps.
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
///
/// `container` is rejected unless it is a single path segment. `PathBuf::join`
/// resolves `..` and lets an absolute component replace the base outright, so a
/// record named `..` or `/somewhere/else` would otherwise turn `--purge` into a
/// recursive delete of a directory agentdock never created. The name arrives
/// from `delete`'s positional argument, from `-n`, from a generated UUID, or from
/// `records.json`, and that last is a plain user-owned file that a script or a
/// merge could have edited.
pub fn container_data_dir(container: &str) -> Result<PathBuf> {
    let base = agentdock_data_home()?;

    let components: Vec<_> = Path::new(container).components().collect();
    let single_segment = matches!(
        components.as_slice(),
        [Component::Normal(_)] if !container.is_empty()
    );
    if !single_segment {
        bail!(ContainerError::UnsafeContainerName {
            name: container.to_string(),
        });
    }

    Ok(base.join("agentdock").join(container))
}

/// The host paths `container` would have persisted, under the data home.
///
/// One entry per (agent, kind), so a container with both directories shows
/// `<container>/<agent>/config` and `<container>/<agent>/data`. `list -v` reports
/// these rather than the full paths because the prefix is the same for every row
/// and repeating it would stretch the table past the terminal, so a reader of
/// that column has to know the prefix is the data home.
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
/// expects this to land there. Private to this module: every caller wants a
/// path under it, and `container_data_dir` is the one that can say which.
fn agentdock_data_home() -> Result<PathBuf> {
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
/// `USER` is taken verbatim from the image, so it may carry a group: `USER
/// root:root` is a routine Dockerfile line, and read whole it names a user
/// called `root:root`, which is not root — the mount would land in
/// `/home/root:root`, a directory nothing writes to. Only the name before the
/// colon is compared.
///
/// Beyond that the image's own `HOME` is used. The authority here would be
/// `/etc/passwd`, since that is what a shell's `~` and an agent's `getpwuid`
/// both resolve to, but reading it means copying a file out of the image and
/// this has no way to test that.
fn container_home(image: &str) -> Result<String> {
    let cfg = DockerClient::image_config(image)?;

    // A uid on its own is as meaningful as the name: 0 is root either way, and
    // `1000` is what an image with no passwd entry tends to declare.
    let user = cfg.user().split(':').next().unwrap_or_default();
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
