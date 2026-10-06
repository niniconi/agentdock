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
        // An image config for `docker image inspect`. Defaults to a bare root
        // image, which is what a nixos-style image reports: no USER, no HOME.
        let env_file = root.join("image_env");
        std::fs::write(&env_file, "{}\n").expect("write image env");
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

    /// The `.Config` JSON that `docker image inspect` reports for an image.
    fn set_image_config(&self, json: &str) {
        std::fs::write(self.root.join("image_env"), format!("{json}\n")).expect("write env");
    }

    /// The directory `agentdock` persists into for this sandbox and container.
    fn persist_dir(&self, container: &str, kind: &str) -> PathBuf {
        self.persist_dir_for(container, "opencode", kind)
    }

    /// Where one supported agent's directory lives under a container.
    fn persist_dir_for(&self, container: &str, agent: &str, kind: &str) -> PathBuf {
        self.root
            .join("home/.local/share/agentdock")
            .join(container)
            .join(agent)
            .join(kind)
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
        .find_map(|l| {
            l.strip_prefix("docker exec -it ")
                .or_else(|| l.strip_prefix("docker exec "))
        })
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

/// The names in a records.json, which is `{"records": {}}` rather than empty
/// text once the last one is removed.
fn record_names(records: &str) -> Vec<String> {
    let v: serde_json::Value = serde_json::from_str(records).expect("parse records");
    v["records"]
        .as_object()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

#[test]
fn changing_image_recreates_the_container() {
    let mut sb = Sandbox::new("changeimage");
    sb.run(&["apply", "-a", "imageA/agentA", "-n", "box", "-P", "8080:80"]);

    sb.reset_log();
    sb.run(&["apply", "-a", "imageB/agentB", "-n", "box", "-P", "8080:80"]);

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
fn apply_always_recreates_an_existing_container() {
    let mut sb = Sandbox::new("alwaysrecreate");
    sb.run(&["apply", "-a", "img/agent", "-n", "box"]);

    sb.reset_log();
    let out = sb.run(&["apply", "-a", "img/agent", "-n", "box"]);

    assert!(
        out.status.success(),
        "apply must not refuse: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        recreated(&sb.log()),
        "apply replaces the configuration, so it rebuilds: {}",
        sb.log()
    );
}

/// A flag left out is a flag that is turned off. Nothing is inherited from what
/// the container had, which is what makes a repeat apply mean something.
#[test]
fn an_omitted_flag_turns_the_setting_off() {
    let mut sb = Sandbox::new("override");
    sb.run(&[
        "apply",
        "-a",
        "img/agent",
        "-n",
        "box",
        "-P",
        "8080:80",
        "--http-proxy",
        "http://p:3128",
        "--kvm",
    ]);
    assert_eq!(
        sb.records().matches("\"kvm\": true").count(),
        1,
        "the first apply should record kvm: {}",
        sb.records()
    );

    // Only the agent is named, so ports, proxy and kvm all go.
    sb.reset_log();
    sb.run(&["apply", "-a", "img/two", "-n", "box"]);

    let log = sb.log();
    let run_line = log.lines().find(|l| l.contains("docker run")).unwrap_or("");
    assert!(
        !run_line.contains("-p "),
        "ports must be gone: {}",
        run_line
    );
    assert!(
        !run_line.contains("HTTP_PROXY"),
        "the proxy must be gone: {}",
        run_line
    );
    assert!(
        !run_line.contains("--device"),
        "kvm must be gone: {}",
        run_line
    );
    assert_eq!(
        sb.records().matches("\"kvm\": true").count(),
        0,
        "kvm must be false in the record: {}",
        sb.records()
    );
    assert!(!sb.records().contains("8080:80"), "ports must be gone");
    assert!(!sb.records().contains("3128"), "the proxy must be gone");
}

#[test]
fn apply_without_an_agent_uses_the_default() {
    let mut sb = Sandbox::new("defaultagent");
    sb.run(&["apply", "-n", "box"]);

    assert_recorded(&sb.records(), "nixos", "pi-agent");
}

/// `up` has no configuration flags, so nothing it is given can change how the
/// container was built. This is the case that used to rebuild on every call.
#[test]
fn up_never_recreates_the_container() {
    let mut sb = Sandbox::new("upnorecreate");
    sb.run(&["apply", "-a", "imageA/agentA", "-n", "box", "-P", "8080:80"]);

    sb.reset_log();
    sb.run(&["up", "-n", "box"]);

    let log = sb.log();
    assert!(!recreated(&log), "up must not recreate: {}", log);
    assert_eq!(exec_agent(&log).as_deref(), Some("agentA"));
    assert_recorded(&sb.records(), "imageA", "agentA");
}

/// A bare `up` reaches the container by mount path, as `run` used to.
#[test]
fn a_bare_up_reaches_the_container() {
    let mut sb = Sandbox::new("bareup");
    sb.run(&["apply", "-a", "imageA/agentA", "-n", "box"]);

    sb.reset_log();
    sb.run(&["up"]);

    let log = sb.log();
    assert!(!recreated(&log), "up must not recreate: {}", log);
    assert_eq!(exec_agent(&log).as_deref(), Some("agentA"));
}

/// `up` does not create containers, so an unknown one has to say what does.
#[test]
fn up_refuses_to_create_a_container() {
    let mut sb = Sandbox::new("upnocreate");
    let out = sb.run(&["up", "-n", "ghost"]);

    assert!(!out.status.success(), "up must not create");
    assert!(
        !sb.log().contains("docker run"),
        "no container should be created: {}",
        sb.log()
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("apply"),
        "the error should point at apply: {}",
        err
    );
}

#[test]
fn changing_only_the_agent_recreates_the_container() {
    let mut sb = Sandbox::new("agentonly");
    sb.run(&["apply", "-a", "img/one", "-n", "box"]);

    sb.reset_log();
    sb.run(&["apply", "-a", "img/two", "-n", "box"]);

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
    let out = sb.run(&["apply", "-a", "img/agent", "-n", "box"]);
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
fn up_restarts_a_stopped_container() {
    let mut sb = Sandbox::new("stopped");
    sb.run(&["apply", "-a", "img/agent", "-n", "box"]);
    assert_recorded(&sb.records(), "img", "agent");

    // The container is down from docker's point of view, so bringing it up is a
    // restart rather than a rebuild.
    let _ = std::fs::remove_file(&sb.running);
    sb.reset_log();
    sb.run(&["up", "-n", "box"]);

    let log = sb.log();
    assert!(restarted(&log), "expected a restart: {}", log);
    assert!(!recreated(&log), "a restart must not recreate: {}", log);
}

#[test]
fn persist_mounts_both_directories_under_root() {
    let mut sb = Sandbox::new("persist-both");
    sb.run(&["apply", "-a", "nixos/opencode", "-n", "box", "--persist"]);

    let log = sb.log();
    let cfg = sb.persist_dir("box", "config");
    let data = sb.persist_dir("box", "data");

    // The stub image declares no USER, so it runs as root and its home is
    // /root. Both mounts have to land under it or opencode writes past them.
    assert!(
        log.contains(&format!("-v {}:/root/.config/opencode", cfg.display())),
        "config not mounted at the root home: {log}"
    );
    assert!(
        log.contains(&format!(
            "-v {}:/root/.local/share/opencode",
            data.display()
        )),
        "data not mounted at the root home: {log}"
    );
}

#[test]
fn persist_creates_the_host_directories() {
    let mut sb = Sandbox::new("persist-mkdir");
    sb.run(&["apply", "-a", "nixos/opencode", "-n", "box", "--persist"]);

    // docker would create these too, but as root, which would leave the user
    // unable to delete what they asked to be persisted.
    for kind in ["config", "data"] {
        let dir = sb.persist_dir("box", kind);
        assert!(dir.is_dir(), "{} was not created", dir.display());
    }
}

#[test]
fn persist_config_alone_mounts_one_directory() {
    let mut sb = Sandbox::new("persist-cfg");
    sb.run(&[
        "apply",
        "-a",
        "nixos/opencode",
        "-n",
        "box",
        "--persist",
        "config",
    ]);

    let log = sb.log();
    let cfg = sb.persist_dir("box", "config");

    assert!(
        log.contains(&format!("-v {}:/root/.config/opencode", cfg.display())),
        "config missing: {log}"
    );
    assert!(
        !log.contains(".local/share/opencode"),
        "data mounted though only config was asked for: {log}"
    );
    assert!(
        !sb.persist_dir("box", "data").exists(),
        "data directory created though only config was asked for"
    );
}

#[test]
fn persist_data_alone_mounts_one_directory() {
    let mut sb = Sandbox::new("persist-data");
    sb.run(&[
        "apply",
        "-a",
        "nixos/opencode",
        "-n",
        "box",
        "--persist",
        "data",
    ]);

    let log = sb.log();
    let data = sb.persist_dir("box", "data");

    assert!(
        log.contains(&format!(
            "-v {}:/root/.local/share/opencode",
            data.display()
        )),
        "data missing: {log}"
    );
    assert!(
        !log.contains(".config/opencode"),
        "config mounted though only data was asked for: {log}"
    );
}

#[test]
fn omitting_persist_mounts_nothing_extra() {
    let mut sb = Sandbox::new("persist-off");
    sb.run(&["apply", "-a", "nixos/opencode", "-n", "box"]);

    let log = sb.log();
    assert_eq!(
        log.matches(" -v ").count(),
        1,
        "only the workspace should be mounted: {log}"
    );
    assert!(
        !log.contains("opencode/"),
        "persisted without the flag: {log}"
    );
}

#[test]
fn a_bare_apply_drops_a_previously_persisted_mount() {
    let mut sb = Sandbox::new("persist-replace");
    sb.run(&["apply", "-a", "nixos/opencode", "-n", "box", "--persist"]);

    sb.reset_log();
    sb.run(&["apply", "-a", "nixos/opencode", "-n", "box"]);

    let log = sb.log();
    assert!(
        !log.contains(".config/opencode"),
        "a flag left out must turn persistence off, like every other flag: {log}"
    );
    assert!(
        !sb.records().contains("\"persist\""),
        "record kept persist after a bare apply: {}",
        sb.records()
    );
}

#[test]
fn persist_uses_the_images_declared_home() {
    let mut sb = Sandbox::new("persist-home");
    // A non-root image that declares where its home is. The username alone does
    // not give the path, so the image has to say it.
    sb.set_image_config(r#"{"User":"node","Env":["PATH=/usr/bin","HOME=/home/node"]}"#);
    sb.run(&["apply", "-a", "nixos/opencode", "-n", "box", "--persist"]);

    let log = sb.log();
    let cfg = sb.persist_dir("box", "config");
    assert!(
        log.contains(&format!("-v {}:/home/node/.config/opencode", cfg.display())),
        "the image's own HOME was not used: {log}"
    );
}

#[test]
fn the_agent_in_a_does_not_decide_where_persistence_goes() {
    let mut sb = Sandbox::new("persist-byagent");
    // Entering the same image with bash is how you look around inside a
    // container whose agent you have not started yet. Persistence follows
    // neither the entry point nor the image, so this container keeps its data.
    let out = sb.run(&["apply", "-a", "nixos/bash", "-n", "box", "--persist"]);

    assert!(out.status.success(), "bash should be persistable: {out:?}");

    let log = sb.log();
    let cfg = sb.persist_dir("box", "config");
    assert!(
        log.contains(&format!("-v {}:/root/.config/opencode", cfg.display())),
        "opencode's config not mounted through a bash entry point: {log}"
    );
}

#[test]
fn two_entry_points_share_one_directory() {
    let mut sb = Sandbox::new("persist-shared");
    sb.run(&["apply", "-a", "nixos/opencode", "-n", "box", "--persist"]);

    // The same container entered differently must not look like a
    // different set of data: the entry point is not an owner.
    sb.reset_log();
    sb.run(&["apply", "-a", "nixos/bash", "-n", "box", "--persist"]);

    let log = sb.log();
    let cfg = sb.persist_dir("box", "config");
    assert!(
        log.contains(&format!("-v {}:/root/.config/opencode", cfg.display())),
        "the second entry point mounted somewhere else: {log}"
    );
}

#[test]
fn persist_works_for_any_image() {
    let mut sb = Sandbox::new("persist-anyimg");
    // There is no whitelist to miss any more: the directories mounted are a
    // property of the supported agents, and the image only supplies the home
    // directory they hang off. Keying on the image used to put `nixos`,
    // `nixos:latest` and `ghcr.io/x/nixos` in separate arms, so pinning a tag
    // meant editing the table.
    let out = sb.run(&["apply", "-a", "ubuntu/agent", "-n", "box", "--persist"]);

    assert!(out.status.success(), "any image should persist: {out:?}");

    let log = sb.log();
    let cfg = sb.persist_dir("box", "config");
    assert!(
        log.contains(&format!("-v {}:/root/.config/opencode", cfg.display())),
        "config not mounted: {log}"
    );
}

#[test]
fn a_registry_qualified_image_still_finds_its_home() {
    let mut sb = Sandbox::new("persist-registry");
    // The image field keeps its own slashes, so the agent name is what is left
    // after the last one. Reading the first slash instead would call the image
    // `ghcr.io` and hand `owner/nixos/opencode` to docker exec as the agent.
    sb.run(&[
        "apply",
        "-a",
        "ghcr.io/owner/nixos/opencode",
        "-n",
        "box",
        "--persist",
    ]);

    let log = sb.log();
    let cfg = sb.persist_dir("box", "config");
    assert!(
        log.contains(&format!("-v {}:/root/.config/opencode", cfg.display())),
        "config not mounted: {log}"
    );
    assert!(
        exec_agent(&log).as_deref() == Some("opencode"),
        "the agent name should be what is left after the last slash: {log}"
    );
}

#[test]
fn persist_is_recorded_so_list_can_show_it() {
    let mut sb = Sandbox::new("persist-record");
    sb.mark_running();
    sb.run(&[
        "apply",
        "-a",
        "nixos/opencode",
        "-n",
        "box",
        "--persist",
        "data",
    ]);

    // Compared on the parsed values rather than on the text, since serde_json
    // pretty-prints an array across several lines.
    let records: serde_json::Value = serde_json::from_str(&sb.records()).expect("parse records");
    assert_eq!(
        records["records"]["box"]["persist"],
        serde_json::json!(["data"]),
        "record missing persist: {}",
        sb.records()
    );

    let out = sb.run(&["list", "-v"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("PERSISTED"),
        "verbose list should have the column: {stdout}"
    );
    // Relative to the container's base, since there is one directory per
    // supported agent and an absolute path repeated for each would stretch the
    // table past the terminal.
    assert!(
        stdout.contains("box/opencode/data"),
        "verbose list should name the directory it persisted: {stdout}"
    );
}

#[test]
fn a_missing_image_reports_the_pull_failure() {
    let mut sb = Sandbox::new("persist-nopull");
    // The stub reports no image, so the first inspect fails and the retry runs.
    // The stub's last arm exits 0 for anything it does not handle, so `pull`
    // succeeds and the second inspect fails too — which is the path that
    // re-reports the first error. The message has to be about the image rather
    // than about persistence, or the user goes looking in the wrong place.
    std::fs::remove_file(sb.root.join("image_env")).expect("remove image env");

    let out = sb.run(&["apply", "-a", "absent/opencode", "-n", "box", "--persist"]);

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("absent"),
        "the error should name the image: {stderr}"
    );
    assert!(
        !sb.log().contains("docker run"),
        "a container was created despite the image being unusable: {}",
        sb.log()
    );
}

#[test]
fn up_leaves_a_persisted_container_alone() {
    let mut sb = Sandbox::new("persist-up");
    sb.run(&["apply", "-a", "nixos/opencode", "-n", "box", "--persist"]);

    sb.reset_log();
    let _ = std::fs::remove_file(&sb.running);
    sb.run(&["up", "-n", "box"]);

    // up carries no configuration flags, so it cannot change how the container
    // was built. A mount appearing here would mean it had.
    let log = sb.log();
    assert!(restarted(&log), "expected a restart: {log}");
    assert!(!recreated(&log), "up must not rebuild: {log}");
    assert!(
        !log.contains("image inspect"),
        "up should not need the image config: {log}"
    );
}

#[test]
fn delete_keeps_persisted_data_by_default() {
    let mut sb = Sandbox::new("purge-keep");
    sb.run(&["apply", "-a", "nixos/opencode", "-n", "box", "--persist"]);

    let out = sb.run(&["delete", "box", "--force"]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    // The data lives outside the container, so removing the container does not
    // remove it, and it is the only copy there is.
    assert!(
        sb.persist_dir("box", "config").is_dir(),
        "delete removed the data without being asked to: {stdout}"
    );
    assert!(
        stdout.contains("Persisted data kept at"),
        "the user should be told where it went: {stdout}"
    );
    assert!(
        stdout.contains("rm -rf"),
        "the user should be told how to remove it: {stdout}"
    );
}

#[test]
fn delete_purge_removes_persisted_data() {
    let mut sb = Sandbox::new("purge-yes");
    sb.run(&["apply", "-a", "nixos/opencode", "-n", "box", "--persist"]);

    let out = sb.run(&["delete", "box", "--force", "--purge"]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        !sb.persist_dir("box", "config").exists(),
        "--purge left the data behind: {stdout}"
    );
    assert!(
        !sb.persist_dir("box", "data").exists(),
        "--purge left the data behind: {stdout}"
    );
    assert_eq!(
        record_names(&sb.records()),
        Vec::<String>::new(),
        "{}",
        sb.records()
    );
    assert!(
        sb.mount().exists(),
        "--purge must not touch the workspace, only the agent's own data"
    );
}

#[test]
fn delete_purge_on_a_container_that_never_persisted_is_fine() {
    let mut sb = Sandbox::new("purge-noop");
    sb.run(&["apply", "-a", "nixos/bash", "-n", "box"]);

    // Nothing to remove is the desired end state, so it is not an error.
    let out = sb.run(&["delete", "box", "--force", "--purge"]);
    assert!(
        out.status.success(),
        "--purge with nothing to purge should succeed: {out:?}"
    );
    assert_eq!(
        record_names(&sb.records()),
        Vec::<String>::new(),
        "{}",
        sb.records()
    );
}

#[test]
fn delete_without_purge_says_nothing_when_there_was_no_data() {
    let mut sb = Sandbox::new("purge-quiet");
    sb.run(&["apply", "-a", "nixos/bash", "-n", "box"]);

    let out = sb.run(&["delete", "box", "--force"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("Persisted data kept at"),
        "nothing was persisted, so nothing should be mentioned: {stdout}"
    );
}

#[test]
fn a_container_name_cannot_escape_the_data_directory() {
    let mut sb = Sandbox::new("unsafe-name");
    // `--purge` is a recursive delete, and `records.json` is a plain user-owned
    // file a script or a merge could have edited. `PathBuf::join` resolves `..`
    // and lets an absolute component replace the base, so without a check these
    // would delete a directory agentdock never created — here, the data of
    // every other container.
    let victim = sb.root.join("outside");
    std::fs::create_dir_all(&victim).expect("victim");
    std::fs::write(victim.join("notes.txt"), "not agentdock's").expect("seed");

    for name in ["..", "../outside", &victim.display().to_string()] {
        let body = format!(
            r#"{{"records":{{"{name}":{{"path":"{mnt}","created_at":"2026-01-01T00:00:00+00:00","docker_image":"img","agent_name":"opencode","kvm":false}}}}}}"#,
            name = name,
            mnt = sb.mount().display()
        );
        sb.write_records(&body);

        let out = sb.run(&["delete", name, "--force", "--purge"]);
        let stderr = String::from_utf8_lossy(&out.stderr);

        assert!(
            stderr.contains("cannot be used as a container name"),
            "{name} should be refused, got: {out:?}"
        );
        assert!(
            !String::from_utf8_lossy(&out.stdout).contains("Purged"),
            "{name} must not report a purge it did not do"
        );
    }

    assert!(victim.join("notes.txt").exists(), "the victim was deleted");
}

#[test]
fn apply_also_refuses_a_name_that_would_escape() {
    let mut sb = Sandbox::new("unsafe-apply");
    // Same check, reached through `apply`. The directory is created before
    // docker runs, so without this an unusable name would leave directories
    // outside the base that `delete` could not then clean up.
    let out = sb.run(&["apply", "-a", "img/opencode", "-n", "../evil", "--persist"]);

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cannot be used as a container name"),
        "apply should refuse it: {out:?}"
    );
    assert!(
        !sb.root.join("home/.local/share/evil").exists(),
        "a directory was created outside the base"
    );
}

#[test]
fn a_user_with_a_group_is_still_root() {
    let mut sb = Sandbox::new("persist-usergroup");
    // `USER root:root` is a routine Dockerfile line and docker passes it
    // through verbatim. Read whole, `root:root` is not `root`, so the mount
    // would go to /home/root:root, a directory nothing writes to.
    sb.set_image_config(r#"{"User":"root:root","Env":["PATH=/usr/bin"]}"#);
    sb.run(&["apply", "-a", "img/opencode", "-n", "box", "--persist"]);

    let log = sb.log();
    let cfg = sb.persist_dir("box", "config");
    assert!(
        log.contains(&format!("-v {}:/root/.config/opencode", cfg.display())),
        "root:root should be root, got: {log}"
    );
}

#[test]
fn persist_repeats_do_not_accumulate() {
    let mut sb = Sandbox::new("persist-repeat");
    // clap lets the flag repeat and append, so this arrives as two entries.
    // The outcome the user meant is the same either way; what reaches the
    // record should not be a list with the same thing in it twice.
    sb.run(&[
        "apply",
        "-a",
        "img/opencode",
        "-n",
        "box",
        "--persist",
        "config,config",
    ]);

    let records: serde_json::Value = serde_json::from_str(&sb.records()).expect("parse");
    assert_eq!(
        records["records"]["box"]["persist"],
        serde_json::json!(["config"]),
        "{}",
        sb.records()
    );
}
