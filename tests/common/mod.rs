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
        let env_file = root.join("image_env");
        std::fs::write(&env_file, "{}\n").expect("write image env");

        if mode == DockerMode::Stub {
            let stub = format!(
                "#!/bin/sh\n\
                 echo \"docker $*\" >> \"{log}\"\n\
                 case \"$1 $2\" in\n\
                 'image inspect') cat \"{env_file}\" ;;\n\
                 esac\n\
                 case \"$1\" in\n\
                 inspect) if [ -f \"{running}\" ]; then echo true; else echo false; fi ;;\n\
                 run) shift; while [ $# -gt 0 ]; do case \"$1\" in -d) touch \"{running}\"; exit 0;; esac; shift; done; exit 0 ;;\n\
                 *) exit 0 ;;\n\
                 esac\n",
                log = log.display(),
                running = running.display(),
                env_file = env_file.display()
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
            containers: Vec::new(),
        })
    }

    pub fn mount(&self) -> PathBuf {
        self.root.join("mnt")
    }

    /// The host's own agent config directory, the source `--template` reads.
    pub fn host_config_dir(&self) -> PathBuf {
        self.root.join("home/.config/opencode")
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

    /// Same as `run`, taking owned strings for scenario vectors.
    pub fn runv(&mut self, args: &[String]) -> Output {
        // In stub mode the fake docker shadows the real one on PATH. In real
        // mode the PATH is untouched, so agentdock reaches the daemon.
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

        // The sandbox owns the whole XDG environment, not just HOME. CI images
        // bake XDG_CONFIG_HOME into /etc/environment pointing at the runner's
        // own home, so leaving it alone would read a config that exists nowhere
        // in the sandbox. The pinned values equal the binary's own fallbacks,
        // so a bare HOME-only environment still resolves to the same paths.
        Command::new(env!("CARGO_BIN_EXE_agentdock"))
            .current_dir(self.mount())
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("home/.config"))
            .env("XDG_DATA_HOME", self.root.join("home/.local/share"))
            .env("PATH", path)
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
}
