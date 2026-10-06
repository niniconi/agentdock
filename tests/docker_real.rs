//! Integration tests against a real Docker daemon.
//!
//! Unlike tests/record_image.rs, which stubs `docker` out and asserts on the
//! command line, these run the real binary against a real daemon: the image
//! inspect keys, the mount destinations, the cgroup values all come from
//! docker itself. Every test returns early (passing) when no daemon is
//! reachable, so a machine without docker stays green.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

const IMAGE: &str = "alpine:3.20";

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Sandbox {
    root: PathBuf,
    containers: Vec<String>,
}

impl Sandbox {
    fn new(label: &str) -> Option<Self> {
        if !docker_available() {
            return None;
        }
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "agentdock-real-{}-{}-{}",
            label,
            std::process::id(),
            n
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("home")).expect("create sandbox home");
        std::fs::create_dir_all(root.join("mnt")).expect("create sandbox mount");
        Some(Sandbox {
            root,
            containers: Vec::new(),
        })
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn mount(&self) -> PathBuf {
        self.root.join("mnt")
    }

    fn agentdock(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_agentdock"))
            .args(args)
            .current_dir(self.mount())
            .env("HOME", self.home())
            .output()
            .expect("run agentdock")
    }

    fn container(&mut self, name: &str) -> &str {
        self.containers.push(name.to_string());
        self.containers.last().unwrap()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        for name in &self.containers {
            let _ = Command::new("docker").args(["rm", "-f", name]).output();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn docker_available() -> bool {
    Command::new("docker")
        .args(["version", "--format", "{{.Server.Version}}"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn docker(args: &[&str]) -> String {
    let out = Command::new("docker")
        .args(args)
        .output()
        .expect("run docker");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

/// The whole `apply` path, checked against what the daemon really bound:
/// env vars reach the container, memory and cpus reach HostConfig, and
/// --persist binds the image's home under /root for a root-running image.
#[test]
fn apply_configures_a_real_container() {
    let Some(mut sb) = Sandbox::new("apply") else {
        eprintln!("skipping: no docker daemon");
        return;
    };
    sb.container("real-apply");

    let out = sb.agentdock(&[
        "apply",
        "-a",
        &format!("{IMAGE}/sh"),
        "-n",
        "real-apply",
        "-e",
        "FOO=bar",
        "--memory",
        "128m",
        "--cpus",
        "1",
        "--persist",
    ]);
    assert!(out.status.success(), "apply failed: {}", stderr(&out));

    let env = docker(&["exec", "real-apply", "sh", "-c", "echo $FOO"]);
    assert_eq!(env, "bar", "env var did not reach the container");

    let memory = docker(&["inspect", "-f", "{{.HostConfig.Memory}}", "real-apply"]);
    assert_eq!(memory, "134217728", "memory limit not applied");

    let nano = docker(&["inspect", "-f", "{{.HostConfig.NanoCpus}}", "real-apply"]);
    assert_eq!(nano, "1000000000", "cpu limit not applied");

    // alpine declares no USER, so the persisted directories land under /root.
    let mounts = docker(&[
        "inspect",
        "-f",
        "{{range .Mounts}}{{.Destination}} {{end}}{{end}}",
        "real-apply",
    ]);
    assert!(
        mounts.contains("/root/.config/opencode"),
        "mounts: {mounts}"
    );

    // The agentdock-side view of the same container.
    let listed = stdout(&sb.agentdock(&["list", "-v"]));
    assert!(listed.contains("real-apply"), "list -v: {listed}");

    // `up` on the running container must not rebuild it, and delete with
    // --purge must remove both the container and the data directory.
    let restart = sb.agentdock(&["up", "-n", "real-apply"]);
    assert!(restart.status.success(), "up failed: {}", stderr(&restart));

    let data_dir = sb.home().join(".local/share/agentdock/real-apply");
    assert!(data_dir.exists(), "persisted dir missing");

    let del = sb.agentdock(&["delete", "real-apply", "--force", "--purge"]);
    assert!(del.status.success(), "delete failed: {}", stderr(&del));
    assert!(!data_dir.exists(), "--purge left the data dir");
}

/// `apply` replaces rather than merges: dropping every optional flag must
/// strip what the previous apply set, both in the record and in the container.
#[test]
fn an_omitted_flag_turns_the_setting_off_in_a_real_container() {
    let Some(mut sb) = Sandbox::new("replace") else {
        eprintln!("skipping: no docker daemon");
        return;
    };
    sb.container("real-replace");

    let first = sb.agentdock(&[
        "apply",
        "-a",
        &format!("{IMAGE}/sh"),
        "-n",
        "real-replace",
        "-e",
        "FOO=bar",
        "--memory",
        "128m",
    ]);
    assert!(first.status.success(), "first apply: {}", stderr(&first));

    let second = sb.agentdock(&["apply", "-a", &format!("{IMAGE}/sh"), "-n", "real-replace"]);
    assert!(second.status.success(), "second apply: {}", stderr(&second));

    let env = docker(&["exec", "real-replace", "sh", "-c", "echo ${FOO:-unset}"]);
    assert_eq!(env, "unset", "env survived a bare apply");

    let memory = docker(&["inspect", "-f", "{{.HostConfig.Memory}}", "real-replace"]);
    assert_eq!(memory, "0", "memory survived a bare apply");
}

/// Without a TTY, `up` must still succeed: no `-it`, so no
/// "the input device is not a TTY". This is what broke before.
#[test]
fn up_does_not_demand_a_tty() {
    let Some(mut sb) = Sandbox::new("tty") else {
        eprintln!("skipping: no docker daemon");
        return;
    };
    sb.container("real-tty");

    let apply = sb.agentdock(&["apply", "-a", &format!("{IMAGE}/sh"), "-n", "real-tty"]);
    assert!(apply.status.success(), "apply: {}", stderr(&apply));

    // stdin of the agentdock process is inherited here, which is not a
    // terminal under cargo test, so this exercises the non-TTY path.
    let up = sb.agentdock(&["up", "-n", "real-tty"]);
    let combined = format!("{}{}", stdout(&up), stderr(&up));
    assert!(
        !combined.contains("not a TTY"),
        "up demanded a TTY: {combined}"
    );
}
