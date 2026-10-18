//! Shared test infrastructure for the two docker-facing suites.
//!
//! `tests/record_image.rs` runs the binary against a stub `docker` whose
//! calls are recorded, so it can assert on the exact command sequence.
//! `tests/docker_real.rs` runs the same binary against a real daemon, so it
//! can assert on what the daemon really did. Both suites must drive the
//! binary the same way — same arguments, same sandbox layout — because the
//! thing being tested has to be identical; only the docker side differs.
//!
//! `Sandbox::new(label, DockerMode::Stub)` gives the stub setup.
//! `Sandbox::new(label, DockerMode::Real)` gives the real one, and returns
//! `None` (caller should skip) when no daemon is reachable.

// Shared between suites; each suite uses only a subset of it.
#![allow(dead_code)]

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DockerMode {
    Stub,
    Real,
}

pub struct Sandbox {
    pub root: PathBuf,
    pub mode: DockerMode,
    pub log: PathBuf,
    pub running: PathBuf,
    pull_fail: PathBuf,
    gone: PathBuf,
    containers: Vec<String>,
}

impl Sandbox {
    pub fn new(label: &str, mode: DockerMode) -> Option<Self> {
        if mode == DockerMode::Real && !docker_available() {
            return None;
        }
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "agentdock-{}-{}-{}-{}",
            match mode {
                DockerMode::Stub => "img",
                DockerMode::Real => "real",
            },
            label,
            std::process::id(),
            n
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("home/.config/agentdock")).expect("create home");
        std::fs::create_dir_all(root.join("mnt")).expect("create mount");
        std::fs::create_dir_all(root.join("bin")).expect("create bin");

        let log = root.join("docker.log");
        let running = root.join("running");
        let pull_fail = root.join("pull_fail");
        let gone = root.join("gone");
        let env_file = root.join("image_env");
        let id_file = root.join("image_id");
        std::fs::write(&env_file, "{}\n").expect("write image env");
        // A stable id by default, so `update` sees an unchanged tag until a test
        // repoints it with `set_image_id`.
        std::fs::write(
            &id_file,
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
        )
        .expect("write image id");

        if mode == DockerMode::Stub {
            let stub = format!(
                "#!/bin/sh\n\
                 echo \"docker $*\" >> \"{log}\"\n\
                 case \"$1 $2\" in\n\
                 'image inspect') if [ \"$4\" = \"{{{{.Id}}}}\" ]; then cat \"{id_file}\"; else cat \"{env_file}\"; fi ;;\n\
                 esac\n\
                 case \"$1\" in\n\
                 inspect) if [ -f \"{gone}\" ]; then exit 1; fi; if [ -f \"{running}\" ]; then echo true; else echo false; fi ;;\n\
                 run) shift; while [ $# -gt 0 ]; do case \"$1\" in -d) touch \"{running}\"; exit 0;; esac; shift; done; exit 0 ;;\n\
                 stop) rm -f \"{running}\"; exit 0 ;;\n\
                 pull) if [ -f \"{pull_fail}\" ]; then echo 'pull access denied' >&2; exit 1; fi; exit 0 ;;\n\
                 *) exit 0 ;;\n\
                 esac\n",
                log = log.display(),
                running = running.display(),
                pull_fail = pull_fail.display(),
                gone = gone.display(),
                env_file = env_file.display(),
                id_file = id_file.display()
            );
            let stub_path = root.join("bin/docker");
            std::fs::write(&stub_path, stub).expect("write stub");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&stub_path, std::fs::Permissions::from_mode(0o755))
                    .expect("chmod stub");
            }
        }

        Some(Sandbox {
            root,
            mode,
            log,
            running,
            pull_fail,
            gone,
            containers: Vec::new(),
        })
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

    pub fn set_image_config(&self, json: &str) {
        std::fs::write(self.root.join("image_env"), format!("{json}\n")).expect("write env");
    }

    /// Repoint the image id the stub reports for `docker image inspect {{.Id}}`.
    /// A different value stands for a tag that now resolves to a newer build.
    pub fn set_image_id(&self, id: &str) {
        std::fs::write(self.root.join("image_id"), format!("{id}\n")).expect("write image id");
    }

    /// Make the stub's next `docker pull` fail, the way a registry rejecting an
    /// unknown image does. Used to assert `update` does not destroy a container
    /// when it cannot fetch a replacement.
    pub fn set_pull_failure(&self) {
        std::fs::write(&self.pull_fail, "").expect("write pull failure marker");
    }

    /// Make the stub's `docker inspect` fail for any container, the way one
    /// removed outside agentdock does. Used to assert `update` reports a missing
    /// container the same way `up` does, before it touches it.
    pub fn set_container_gone(&self) {
        std::fs::write(&self.gone, "").expect("write gone marker");
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

    pub fn mark_running(&self) {
        let _ = std::fs::write(&self.running, "");
    }

    pub fn reset_log(&self) {
        let _ = std::fs::remove_file(&self.log);
    }

    pub fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
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

    /// The PATH a run gets: in stub mode the fake docker shadows the real one,
    /// in real mode it is untouched so agentdock reaches the daemon.
    fn path(&self) -> String {
        let mut path = match self.mode {
            DockerMode::Stub => self.root.join("bin").to_string_lossy().to_string(),
            DockerMode::Real => String::new(),
        };
        if let Ok(existing) = std::env::var("PATH") {
            if !path.is_empty() {
                path.push(':');
            }
            path.push_str(&existing);
        }
        path
    }

    /// Same as `run`, taking owned strings for scenario vectors.
    pub fn runv(&mut self, args: &[String]) -> Output {
        self.command()
            .env("PATH", self.path())
            .args(args)
            .output()
            .expect("run agentdock")
    }

    /// Run with a PATH that has no docker on it, the way a machine that never
    /// installed docker looks. The stub is left off PATH entirely, so a call
    /// that unexpectedly reaches docker cannot be answered by it.
    pub fn run_without_docker(&mut self, args: &[&str]) -> Output {
        let empty = self.root.join("empty-bin");
        std::fs::create_dir_all(&empty).expect("create empty bin");
        self.command()
            .env("PATH", empty)
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

    /// Track a real container name for cleanup; a no-op in stub mode.
    pub fn track(&mut self, name: &str) {
        if self.mode == DockerMode::Real {
            self.containers.push(name.to_string());
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        if self.mode == DockerMode::Real {
            for name in &self.containers {
                let _ = Command::new("docker").args(["rm", "-f", name]).output();
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub fn docker_available() -> bool {
    Command::new("docker")
        .args(["version", "--format", "{{.Server.Version}}"])
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

/// Scenarios every docker-facing suite drives, so stub and real tests invoke
/// agentdock with exactly the same arguments.
pub mod scenarios {
    pub fn full(name: &str) -> Vec<String> {
        [
            "apply",
            "-a",
            "alpine:3.20/sh",
            "-n",
            name,
            "-e",
            "FOO=bar",
            "--memory",
            "128m",
            "--cpus",
            "1",
            "--persist",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    pub fn bare(name: &str) -> Vec<String> {
        ["apply", "-a", "alpine:3.20/sh", "-n", name]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    pub fn up(name: &str) -> Vec<String> {
        ["up", "-n", name].iter().map(|s| s.to_string()).collect()
    }

    pub fn stop(name: &str) -> Vec<String> {
        ["stop", "-n", name].iter().map(|s| s.to_string()).collect()
    }
}
