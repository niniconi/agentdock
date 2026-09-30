use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A throwaway tree with an isolated HOME and a fake `docker` on PATH that
/// records every call, so tests can assert on the exact command sequence.
struct Sandbox {
    root: PathBuf,
    log: PathBuf,
    running: PathBuf,
    args: Vec<String>,
}

impl Sandbox {
    fn new(label: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "agentdock-img-{}-{}-{}",
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
        let stub = format!(
            "#!/bin/sh\n\
             echo \"docker $*\" >> \"{log}\"\n\
             case \"$1\" in\n\
             inspect) if [ -f \"{running}\" ]; then echo true; else echo false; fi ;;\n\
             run) shift; while [ $# -gt 0 ]; do case \"$1\" in -d) touch \"{running}\"; exit 0;; esac; shift; done; exit 0 ;;\n\
             *) exit 0 ;;\n\
             esac\n",
            log = log.display(),
            running = running.display()
        );
        let stub_path = root.join("bin/docker");
        std::fs::write(&stub_path, stub).expect("write stub");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&stub_path, std::fs::Permissions::from_mode(0o755))
                .expect("chmod stub");
        }

        Sandbox {
            root,
            log,
            running,
            args: Vec::new(),
        }
    }

    fn mount(&self) -> PathBuf {
        self.root.join("mnt")
    }

    fn records(&self) -> String {
        std::fs::read_to_string(self.root.join("home/.config/agentdock/records.json"))
            .unwrap_or_default()
    }

    fn write_records(&self, body: &str) {
        std::fs::write(self.root.join("home/.config/agentdock/records.json"), body)
            .expect("write records");
    }

    /// Make the stub report the container as already running.
    fn mark_running(&self) {
        let _ = std::fs::write(&self.running, "");
    }

    fn reset_log(&self) {
        let _ = std::fs::remove_file(&self.log);
    }

    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// Run agentdock with the given arguments, then forget them.
    fn run(&mut self, args: &[&str]) -> Output {
        self.args = args.iter().map(|s| s.to_string()).collect();

        // PATH must be assembled as a string; a PathBuf would treat ":" as a
        // directory name and produce an unusable PATH.
        let mut path = self.root.join("bin").to_string_lossy().to_string();
        if let Ok(existing) = std::env::var("PATH") {
            path.push(':');
            path.push_str(&existing);
        }

        let out = Command::new(env!("CARGO_BIN_EXE_agentdock"))
            .current_dir(self.mount())
            .env("HOME", self.root.join("home"))
            .env("PATH", path)
            .args(&self.args)
            .output()
            .expect("run agentdock");
        self.args.clear();
        out
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn recreated(log: &str) -> bool {
    log.contains("rm -f")
}

fn restarted(log: &str) -> bool {
    log.contains("docker restart ")
}

/// The image token is the word right before `sleep inf` in a `docker run` line.
fn image_in_run(log: &str) -> Option<String> {
    log.lines()
        .find(|l| l.contains("docker run"))
        .and_then(|l| {
            let words: Vec<&str> = l.split_whitespace().collect();
            let sleep = words.iter().position(|w| *w == "sleep")?;
            if sleep == 0 {
                return None;
            }
            Some(words[sleep - 1].to_string())
        })
}

fn exec_agent(log: &str) -> Option<String> {
    log.lines()
        .find_map(|l| l.strip_prefix("docker exec -it "))
        .and_then(|rest| rest.rsplit("sh -c ").next().map(|s| s.to_string()))
}

fn assert_recorded(records: &str, image: &str, agent: &str) {
    assert!(
        records.contains(&format!("\"docker_image\": \"{}\"", image)),
        "record missing docker_image {}: {}",
        image,
        records
    );
    assert!(
        records.contains(&format!("\"agent_name\": \"{}\"", agent)),
        "record missing agent_name {}: {}",
        agent,
        records
    );
}

#[test]
fn changing_image_recreates_the_container() {
    let mut sb = Sandbox::new("changeimage");
    sb.run(&["run", "-a", "imageA/agentA", "-n", "box", "-P", "8080:80"]);

    sb.reset_log();
    sb.run(&["run", "-a", "imageB/agentB", "-n", "box", "-P", "8080:80"]);

    let log = sb.log();
    assert!(
        recreated(&log),
        "changing the image must recreate the container: {}",
        log
    );
    assert_eq!(
        image_in_run(&log).as_deref(),
        Some("imageB"),
        "recreated with the wrong image: {}",
        log
    );
    assert_eq!(exec_agent(&log).as_deref(), Some("agentB"));
    assert_recorded(&sb.records(), "imageB", "agentB");
}

#[test]
fn unchanged_agent_does_not_recreate_the_container() {
    let mut sb = Sandbox::new("nochange");
    sb.run(&["run", "-a", "img/agent", "-n", "box", "-P", "8080:80"]);

    sb.reset_log();
    sb.run(&["run", "-a", "img/agent", "-n", "box", "-P", "8080:80"]);

    let log = sb.log();
    assert!(
        !recreated(&log),
        "identical settings must not recreate: {}",
        log
    );
    assert_eq!(exec_agent(&log).as_deref(), Some("agent"));
}

#[test]
fn changing_only_the_agent_recreates_the_container() {
    let mut sb = Sandbox::new("agentonly");
    sb.run(&["run", "-a", "img/one", "-n", "box"]);

    sb.reset_log();
    sb.run(&["run", "-a", "img/two", "-n", "box"]);

    let log = sb.log();
    assert!(recreated(&log), "changing the agent must recreate: {}", log);
    assert_eq!(exec_agent(&log).as_deref(), Some("two"));
    assert_recorded(&sb.records(), "img", "two");
}

#[test]
fn a_malformed_records_file_is_not_overwritten() {
    let mut sb = Sandbox::new("malformed");
    sb.write_records("{\"records\": {\"box\": {\"path\": ");
    sb.mark_running();

    // Treating an unparseable file as "no records" would let the next save()
    // replace whatever the user actually had, so the run must fail instead.
    let out = sb.run(&["run", "-a", "img/agent", "-n", "box"]);
    assert!(
        !out.status.success(),
        "a malformed records file must not be silently accepted: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(
        sb.records(),
        "{\"records\": {\"box\": {\"path\": ",
        "the malformed file was rewritten"
    );
    assert!(
        !recreated(&sb.log()),
        "nothing should have been touched: {}",
        sb.log()
    );
}

#[test]
fn a_malformed_records_file_still_explains_why() {
    let mut sb = Sandbox::new("whyrecords");
    // `docker_image` is a required field, so this file does not deserialize.
    // Both RecordError variants carry their source, so the failure has to reach
    // the user: without it the whole message is "Failed to parse persistent
    // records" and there is nothing to act on.
    sb.write_records("{\"records\": {\"box\": {\"path\": \"/tmp\", \"created_at\": \"x\"}}}");

    let out = sb.run(&["status", "box"]);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        stderr.contains("Failed to parse persistent records"),
        "missing the summary: {}",
        stderr
    );
    assert!(
        stderr.contains("docker_image"),
        "the parse error must name the offending field: {}",
        stderr
    );
}

#[test]
fn a_stopped_container_is_restarted_not_recreated() {
    let mut sb = Sandbox::new("stopped");
    sb.run(&["run", "-a", "img/agent", "-n", "box"]);
    assert_recorded(&sb.records(), "img", "agent");

    // The container is gone from docker's point of view, so the next run takes
    // the Stopped path and restarts rather than recreating.
    let _ = std::fs::remove_file(&sb.running);
    sb.reset_log();
    sb.run(&["run", "-a", "img/agent", "-n", "box"]);

    let log = sb.log();
    assert!(restarted(&log), "expected a restart: {}", log);
    assert!(!recreated(&log), "a restart must not recreate: {}", log);
}
