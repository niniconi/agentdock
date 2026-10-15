use anyhow::{Result, bail};
use std::path::{Component, Path, PathBuf};

use crate::config::Persist;
use crate::docker::DockerClient;
use crate::error::ContainerError;

/// The container-side config directory, relative to opencode's home.
const OPENCODE_CONFIG_REL: &str = ".config/opencode";
/// The container-side data directory, relative to opencode's home.
const OPENCODE_DATA_REL: &str = ".local/share/opencode";
/// Pi's directory, relative to its home. Pi keeps its config and its sessions in
/// one place, so this is the whole of what it persists and it has no data entry.
const PI_AGENT_CONFIG_REL: &str = ".pi/agent";

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
/// keeps. The first element is only a label: it names the directory under
/// `<data home>/agentdock/<container>/`, and it is deliberately not looked up
/// anywhere.
const SUPPORTED: &[(&str, Persist, &str)] = &[
    ("opencode", Persist::Config, OPENCODE_CONFIG_REL),
    ("opencode", Persist::Data, OPENCODE_DATA_REL),
    ("pi-agent", Persist::Config, PI_AGENT_CONFIG_REL),
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
///
/// When `template` is set, a config directory that is being created for the
/// first time is seeded from that agent's own host config directory, found by
/// `host_template_source`. It is the same host directory the mount serves, so
/// the seed is what the container sees.
pub fn plan_mounts(
    container: &str,
    image: &str,
    kinds: &[Persist],
    template: bool,
) -> Result<Vec<Mount>> {
    let wanted: Vec<&(&str, Persist, &str)> = SUPPORTED
        .iter()
        .filter(|(_, kind, _)| kinds.contains(kind))
        .collect();

    if wanted.is_empty() {
        return Ok(Vec::new());
    }

    let base = container_data_dir(container)?;

    // The container home is resolved once for the whole container, not per
    // directory: every mount hangs off the same home, so asking per directory
    // would call docker once per mount and could come back with two different
    // answers.
    let home = container_home(image)?;
    let home = home.trim_end_matches('/');

    let mut mounts = Vec::new();
    for (agent, kind, rel) in wanted {
        let host = base.join(agent).join(kind.as_str());
        // Whether the directory was already there decides whether a template
        // seeds it: seeding is a first-creation event, so a container's own
        // edits are never trampled by a later apply.
        let existed = host.exists();
        // Created here rather than left to docker, which would leave the
        // directory owned by root on the host when the container runs as root.
        std::fs::create_dir_all(&host).map_err(|e| {
            anyhow::Error::new(e).context(format!(
                "Failed to create persistence directory: {}",
                host.display()
            ))
        })?;

        // Resolved per directory rather than once for the container, because the
        // source is the agent's own and the supported config directories are not
        // the same path on the host. Only a config directory being created for
        // the first time asks at all, so a bare --persist makes no lookups.
        if template
            && !existed
            && *kind == Persist::Config
            && let Some(source) = host_template_source(agent)?
        {
            seed_config_dir(&source, &host)?;
        }

        mounts.push(Mount {
            host,
            container: format!("{home}/{rel}"),
        });
    }

    Ok(mounts)
}

/// Copy the host's agent config directories into `container` for a run that
/// asked for `--template` but does not persist config.
///
/// A run that persists config is seeded through `plan_mounts` instead, on the
/// host directory the mount serves, so this does nothing there. Here there is no
/// such directory, so each template is copied straight into the container; it is
/// overwritten on every apply because the container's copy is thrown away with
/// the container. The container-side path comes from the image's home, the same
/// as a mount target, and one directory is copied per supported agent whose host
/// directory exists.
///
/// A run that did not ask for `--template`, or whose host has no such directory,
/// is not an error: there is simply nothing to seed.
pub fn template_into_container(
    name: &str,
    image: &str,
    template: bool,
    config_persisted: bool,
) -> Result<()> {
    if !template || config_persisted {
        return Ok(());
    }

    let home = container_home(image)?;
    let home = home.trim_end_matches('/');

    for (agent, kind, rel) in SUPPORTED {
        if *kind != Persist::Config {
            continue;
        }
        let Some(source) = host_template_source(agent)? else {
            continue;
        };
        let dest = format!("{home}/{rel}");

        // `docker cp` copies a directory *into* an existing target rather than over
        // it, so copy its contents and make sure the target exists first. Parent
        // directories are created too, since `docker cp` will not.
        DockerClient::exec_script(name, &format!("mkdir -p {dest}"))?;
        println!("Templating: {} -> {}:{}", source.display(), name, dest);
        DockerClient::cp(name, &source.join("."), &dest)?;
    }
    Ok(())
}

/// Recursively copy `src` into `dst`, skipping anything `dst` already has.
///
/// The skip makes the "never overwrite a file that is already there" guarantee
/// unconditional: production only ever calls this on a directory it just
/// created, but the copy itself must not depend on that.
fn seed_config_dir(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst).map_err(|e| {
        anyhow::Error::new(e).context(format!("Failed to create {}", dst.display()))
    })?;

    let entries = std::fs::read_dir(src)
        .map_err(|e| anyhow::Error::new(e).context(format!("Failed to read {}", src.display())))?;

    for entry in entries {
        let entry = entry.map_err(|e| {
            anyhow::Error::new(e).context(format!("Failed to read {}", src.display()))
        })?;
        let target = dst.join(entry.file_name());
        let file_type = entry.file_type().map_err(|e| {
            anyhow::Error::new(e).context(format!("Failed to stat {}", entry.path().display()))
        })?;

        if file_type.is_dir() {
            seed_config_dir(&entry.path(), &target)?;
        } else if !target.exists() {
            std::fs::copy(entry.path(), &target).map_err(|e| {
                anyhow::Error::new(e).context(format!(
                    "Failed to copy {} to {}",
                    entry.path().display(),
                    target.display()
                ))
            })?;
        }
    }

    Ok(())
}

/// The host config directory `--template` reads for `agent`, if it has one.
///
/// One source per supported agent, because they do not keep their config in the
/// same place: opencode follows the XDG config home, Pi ignores XDG entirely
/// and keeps everything in `~/.pi/agent`. A directory that does not exist is
/// `None` rather than an error, and so is an agent with no known source.
fn host_template_source(agent: &str) -> Result<Option<PathBuf>> {
    let dir = match agent {
        "opencode" => xdg_config_home()?.join("opencode"),
        "pi-agent" => pi_agent_dir()?,
        _ => return Ok(None),
    };
    Ok(dir.is_dir().then_some(dir))
}

/// Pi's agent directory, wherever this host keeps it.
///
/// Pi does not read XDG: it defaults to `~/.pi/agent`, the same path relative to
/// its home as `PI_AGENT_CONFIG_REL` is inside the container. `PI_CODING_AGENT_DIR`
/// names that directory itself rather than a parent to join it onto, so an
/// absolute value is used as it stands and a relative one is ignored — the same
/// way `xdg_config_home` treats a relative `XDG_CONFIG_HOME`.
fn pi_agent_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("PI_CODING_AGENT_DIR") {
        let dir = PathBuf::from(dir);
        if dir.is_absolute() {
            return Ok(dir);
        }
    }
    Ok(home_dir()?.join(PI_AGENT_CONFIG_REL))
}

/// The invoking user's config home, which is where their agent config lives.
///
/// Mirrors `agentdock_data_home`: XDG first, and only when it names an absolute
/// path, so a relative `XDG_CONFIG_HOME` falls back to the `~/.config` default.
fn xdg_config_home() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
        let dir = PathBuf::from(dir);
        if dir.is_absolute() {
            return Ok(dir);
        }
    }
    Ok(home_dir()?.join(".config"))
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

/// The root of everything agentdock persists on the host
/// (`<XDG_DATA_HOME|~/.local/share>/agentdock`).
///
/// Callers that need one container's directory go through `container_data_dir`;
/// this one exists for code that manages the whole tree, which is currently
/// only the migration runner.
pub fn data_root() -> Result<PathBuf> {
    Ok(agentdock_data_home()?.join("agentdock"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes tests that mutate process-wide environment variables, which
    /// the test harness otherwise runs on concurrent threads.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Puts `HOME`, `XDG_CONFIG_HOME` and `PI_CODING_AGENT_DIR` back the way they
    /// were found, so a test that has to move them cannot leak into a concurrent
    /// one.
    struct EnvGuard {
        home: Option<std::ffi::OsString>,
        xdg: Option<std::ffi::OsString>,
        pi: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn capture() -> Self {
            Self {
                home: std::env::var_os("HOME"),
                xdg: std::env::var_os("XDG_CONFIG_HOME"),
                pi: std::env::var_os("PI_CODING_AGENT_DIR"),
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: the caller holds ENV_LOCK for the whole test, so no other
            // thread reads or writes these variables while they are restored.
            unsafe {
                match &self.home {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
                match &self.xdg {
                    Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                    None => std::env::remove_var("XDG_CONFIG_HOME"),
                }
                match &self.pi {
                    Some(v) => std::env::set_var("PI_CODING_AGENT_DIR", v),
                    None => std::env::remove_var("PI_CODING_AGENT_DIR"),
                }
            }
        }
    }

    fn with_host_config(files: &Path, json: &str) {
        std::fs::create_dir_all(files).expect("create config");
        std::fs::write(files.join("opencode.json"), json).expect("write opencode.json");
        std::fs::create_dir_all(files.join("skills")).expect("create skills");
        std::fs::write(files.join("skills/readme.md"), "host-skill").expect("write readme");
    }

    fn set_host_env(home: &Path) {
        // SAFETY: guarded from concurrent mutation by ENV_LOCK, and restored by
        // EnvGuard when the test returns.
        unsafe {
            std::env::set_var("HOME", home);
            std::env::remove_var("XDG_CONFIG_HOME");
            std::env::remove_var("PI_CODING_AGENT_DIR");
        }
    }

    #[test]
    fn host_template_source_reads_opencode_from_home_config_by_default() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _env = EnvGuard::capture();
        let root = scratch("tmpl-home");
        let files = root.join("home/.config/opencode");
        with_host_config(&files, "from-home");
        set_host_env(&root.join("home"));

        let found = host_template_source("opencode").expect("resolve");
        assert_eq!(found, Some(files.clone()));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn host_template_source_honors_xdg_config_home() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _env = EnvGuard::capture();
        let root = scratch("tmpl-xdg");
        // Somewhere with a config that must take precedence over ~/.config.
        let xdg = root.join("custom-config");
        let files = xdg.join("opencode");
        with_host_config(&files, "from-xdg");
        set_host_env(&root.join("home"));

        // SAFETY: guarded from concurrent mutation by ENV_LOCK, restored by
        // EnvGuard on return.
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &xdg);
        }
        let found = host_template_source("opencode").expect("resolve");
        assert_eq!(found, Some(files.clone()));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The two supported config sources are different paths, and each agent's
    /// template has to come from its own: seeding opencode's directory into
    /// pi-agent's would be the bug this split exists to prevent.
    #[test]
    fn each_supported_agent_resolves_its_own_host_directory() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _env = EnvGuard::capture();
        let root = scratch("tmpl-both");
        set_host_env(&root.join("home"));

        let opencode = root.join("home/.config/opencode");
        let pi = root.join("home/.pi/agent");
        std::fs::create_dir_all(&opencode).expect("opencode dir");
        std::fs::create_dir_all(&pi).expect("pi dir");

        assert_eq!(
            host_template_source("opencode").expect("resolve"),
            Some(opencode)
        );
        assert_eq!(host_template_source("pi-agent").expect("resolve"), Some(pi));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn host_template_source_is_none_without_a_directory() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _env = EnvGuard::capture();
        let root = scratch("tmpl-none");
        set_host_env(&root.join("home"));

        assert_eq!(
            host_template_source("opencode").expect("resolve"),
            None,
            "an absent directory is not an error"
        );
        assert_eq!(host_template_source("pi-agent").expect("resolve"), None);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn host_template_source_is_none_for_an_unknown_agent() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _env = EnvGuard::capture();
        let root = scratch("tmpl-unknown");
        set_host_env(&root.join("home"));
        let files = root.join("home/.config/opencode");
        std::fs::create_dir_all(&files).expect("opencode dir");

        assert_eq!(
            host_template_source("something-else").expect("resolve"),
            None
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// `PI_CODING_AGENT_DIR` names the agent directory itself, so it is the
    /// source whole, not a parent to join the default onto.
    #[test]
    fn pi_agent_dir_honors_pi_coding_agent_dir() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _env = EnvGuard::capture();
        let root = scratch("pi-override");
        set_host_env(&root.join("home"));
        let elsewhere = root.join("somewhere-else/pi");

        // SAFETY: guarded from concurrent mutation by ENV_LOCK, restored by
        // EnvGuard on return.
        unsafe {
            std::env::set_var("PI_CODING_AGENT_DIR", &elsewhere);
        }

        assert_eq!(pi_agent_dir().expect("resolve"), elsewhere);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pi_agent_dir_falls_back_to_the_agent_directory_under_home() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _env = EnvGuard::capture();
        let root = scratch("pi-default");
        set_host_env(&root.join("home"));

        assert_eq!(
            pi_agent_dir().expect("resolve"),
            root.join("home/.pi/agent")
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A relative `PI_CODING_AGENT_DIR` is ignored, matching what a relative
    /// `XDG_CONFIG_HOME` gets: an absolute path is the only thing that can name a
    /// directory from an environment variable on its own.
    #[test]
    fn pi_agent_dir_ignores_a_relative_override() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _env = EnvGuard::capture();
        let root = scratch("pi-relative");
        set_host_env(&root.join("home"));

        // SAFETY: guarded from concurrent mutation by ENV_LOCK, restored by
        // EnvGuard on return.
        unsafe {
            std::env::set_var("PI_CODING_AGENT_DIR", "relative/pi");
        }

        assert_eq!(
            pi_agent_dir().expect("resolve"),
            root.join("home/.pi/agent")
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "agentdock-persist-{}-{}",
            std::process::id(),
            label
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    #[test]
    fn seed_copies_a_nested_tree_but_keeps_files_already_present() {
        let root = scratch("seed");
        let src = root.join("src");
        let dst = root.join("dst");
        std::fs::create_dir_all(src.join("skills")).expect("src");
        std::fs::write(src.join("opencode.json"), "from-host").expect("write");
        std::fs::write(src.join("skills/readme.md"), "from-host").expect("write");

        // The seed never overwrites what is already there; together with the
        // first-creation gate in `plan_mounts` that is what keeps a container's
        // own config intact across a re-apply.
        std::fs::create_dir_all(&dst).expect("dst");
        std::fs::write(dst.join("opencode.json"), "kept").expect("write");

        seed_config_dir(&src, &dst).expect("seed");

        assert_eq!(
            std::fs::read_to_string(dst.join("opencode.json")).unwrap(),
            "kept"
        );
        assert_eq!(
            std::fs::read_to_string(dst.join("skills/readme.md")).unwrap(),
            "from-host"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn seed_creates_the_destination_when_it_does_not_exist_yet() {
        let root = scratch("seed-new");
        let src = root.join("src");
        std::fs::create_dir_all(&src).expect("src");
        std::fs::write(src.join("opencode.json"), "x").expect("write");

        let dst = root.join("nested/dst");
        seed_config_dir(&src, &dst).expect("seed");

        assert!(dst.join("opencode.json").is_file());

        let _ = std::fs::remove_dir_all(&root);
    }
}
