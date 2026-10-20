//! Shared test infrastructure for the integration suites.
//!
//! `tests/docker_real.rs` runs the binary against a real daemon, so it can
//! assert on what the daemon really did; `tests/local.rs` needs no docker at
//! all. Both must drive the binary the same way — same arguments, same
//! sandbox layout — because the thing being tested has to be identical;
//! only the docker side differs.
//!
//! `Sandbox::local` needs no docker: PATH points at an empty directory, so a
//! call that unexpectedly reaches docker fails loudly instead of being
//! answered. `Sandbox::cli` needs the docker binary but no daemon, for paths
//! that shell out to it without asking anything of a container.
//! `Sandbox::real` gives the real setup, and returns `None` (caller should
//! skip) when no daemon is reachable.

// Shared between suites; each suite uses only a subset of it.
#![allow(dead_code)]

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Real,
    Cli,
    Local,
}

pub struct Sandbox {
    pub root: PathBuf,
    mode: Mode,
    containers: Vec<String>,
}

impl Sandbox {
    /// A sandbox for paths that need a real daemon. `None` when no daemon is
    /// reachable, so the caller skips instead of failing.
    pub fn real(label: &str) -> Option<Self> {
        if !docker_available() {
            return None;
        }
        Some(Self::new(label, Mode::Real))
    }

    /// A sandbox for paths that need the docker CLI but not a working daemon —
    /// a run told to talk to a socket that does not exist, say. `None` when the
    /// binary is not installed, so the caller skips there too.
    pub fn cli(label: &str) -> Option<Self> {
        if !docker_cli_available() {
            return None;
        }
        Some(Self::new(label, Mode::Cli))
    }

    /// A sandbox for paths that never need docker: the run's PATH leads to an
    /// empty directory, so reaching docker at all is an error the test sees
    /// rather than something the sandbox quietly answers.
    pub fn local(label: &str) -> Self {
        Self::new(label, Mode::Local)
    }

    fn new(label: &str, mode: Mode) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "agentdock-{}-{}-{}-{}",
            match mode {
                Mode::Real => "real",
                Mode::Cli => "cli",
                Mode::Local => "local",
            },
            label,
            std::process::id(),
            n
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("home/.config/agentdock")).expect("create home");
        std::fs::create_dir_all(root.join("mnt")).expect("create mount");
        std::fs::create_dir_all(root.join("empty-bin")).expect("create empty bin");

        Sandbox {
            root,
            mode,
            containers: Vec::new(),
        }
    }

    pub fn mount(&self) -> PathBuf {
        self.root.join("mnt")
    }

    /// The host's own agent config directory, the source `--template` reads for
    /// opencode.
    pub fn host_config_dir(&self) -> PathBuf {
        self.root.join("home/.config/opencode")
    }

    /// The host's own Pi agent directory, the source `--template` reads for
    /// pi-agent. Pi ignores XDG, so this hangs off HOME alone.
    pub fn host_pi_agent_dir(&self) -> PathBuf {
        self.root.join("home/.pi/agent")
    }

    pub fn records(&self) -> String {
        std::fs::read_to_string(self.root.join("home/.config/agentdock/records.json"))
            .unwrap_or_default()
    }

    pub fn write_records(&self, body: &str) {
        std::fs::write(self.root.join("home/.config/agentdock/records.json"), body)
            .expect("write records");
    }

    pub fn persist_dir(&self, container: &str, kind: &str) -> PathBuf {
        self.persist_dir_for(container, "opencode", kind)
    }

    pub fn persist_dir_for(&self, container: &str, agent: &str, kind: &str) -> PathBuf {
        self.root
            .join("home/.local/share/agentdock")
            .join(container)
            .join(agent)
            .join(kind)
    }

    /// Run agentdock with the given arguments.
    pub fn run(&mut self, args: &[&str]) -> Output {
        self.runv(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    /// The command every run starts from: the binary, run from the mount, with
    /// the sandbox's own HOME and XDG variables.
    ///
    /// The sandbox owns the whole environment the binary resolves paths from,
    /// not just HOME. CI images bake XDG_CONFIG_HOME into /etc/environment
    /// pointing at the runner's own home, so leaving it alone would read a
    /// config that exists nowhere in the sandbox; PI_CODING_AGENT_DIR is
    /// pinned for the same reason, since a leaked one would move Pi's source
    /// out from under the test. The pinned values equal the binary's own
    /// defaults, so a bare HOME-only environment still resolves to the same
    /// paths.
    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_agentdock"));
        cmd.current_dir(self.mount())
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("home/.config"))
            .env("XDG_DATA_HOME", self.root.join("home/.local/share"))
            .env("PI_CODING_AGENT_DIR", self.root.join("home/.pi/agent"));
        cmd
    }

    /// The PATH a run gets: untouched for real and cli sandboxes, so agentdock
    /// reaches the real binary; an empty directory for a local one, so
    /// reaching docker at all is impossible.
    fn path(&self) -> String {
        match self.mode {
            Mode::Real | Mode::Cli => std::env::var("PATH").unwrap_or_default(),
            Mode::Local => self.root.join("empty-bin").to_string_lossy().to_string(),
        }
    }

    /// Same as `run`, taking owned strings for scenario vectors.
    pub fn runv(&mut self, args: &[String]) -> Output {
        self.command()
            .env("PATH", self.path())
            .args(args)
            .output()
            .expect("run agentdock")
    }

    /// Run with a relative HOME and no XDG_DATA_HOME, so resolving agentdock's
    /// own data home has nothing absolute to stand on.
    pub fn run_without_an_absolute_home(&mut self, args: &[&str]) -> Output {
        self.command()
            .env("HOME", "relative-home")
            .env_remove("XDG_DATA_HOME")
            .env("PATH", self.path())
            .args(args)
            .output()
            .expect("run agentdock")
    }

    /// Run with `DOCKER_HOST` pointing at a socket that does not exist, so the
    /// docker CLI is present but no daemon can answer it. A command that reads
    /// any failed docker call as "no container" fails here instead.
    pub fn run_with_an_unreachable_daemon(&mut self, args: &[&str]) -> Output {
        let socket = self.root.join("no-such-docker.sock");
        self.command()
            .env("PATH", self.path())
            .env("DOCKER_HOST", format!("unix://{}", socket.display()))
            .args(args)
            .output()
            .expect("run agentdock")
    }

    /// Track a real container name for cleanup; a no-op where no daemon
    /// exists to have made one.
    pub fn track(&mut self, name: &str) {
        if self.mode == Mode::Real {
            self.containers.push(name.to_string());
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        if self.mode == Mode::Real {
            for name in &self.containers {
                let _ = Command::new("docker").args(["rm", "-f", name]).output();
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Whether a daemon is reachable, not just installed.
fn docker_available() -> bool {
    Command::new("docker")
        .args(["version", "--format", "{{.Server.Version}}"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Whether the docker binary is on PATH, daemon or not.
fn docker_cli_available() -> bool {
    Command::new("docker")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Run docker and return trimmed stdout, panicking with docker's own stderr
/// when the call fails. Silent empty output is never useful in a test.
pub fn docker(args: &[&str]) -> String {
    let out = Command::new("docker")
        .args(args)
        .output()
        .expect("run docker");
    assert!(
        out.status.success(),
        "docker {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

pub fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

pub fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}
