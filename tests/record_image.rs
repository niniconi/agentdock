mod common;

use common::{DockerMode, Sandbox};

fn sandbox(label: &str) -> Sandbox {
    Sandbox::new(label, DockerMode::Stub).expect("stub mode always builds")
}

fn recreated(log: &str) -> bool {
    log.contains("rm -f")
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

#[test]
fn apply_without_an_agent_uses_the_default() {
    let mut sb = sandbox("defaultagent");
    sb.run(&["apply", "-n", "box"]);

    assert_recorded(&sb.records(), "nixos", "pi-agent");
}

/// `up` has no configuration flags, so nothing it is given can change how the
/// container was built. This is the case that used to rebuild on every call.
#[test]
fn a_user_with_a_group_is_still_root() {
    let mut sb = sandbox("persist-usergroup");
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
fn stub_sees_the_same_full_apply() {
    let mut sb = sandbox("scenario-full");
    sb.runv(&common::scenarios::full("box"));

    let log = sb.log();
    assert!(log.contains("-e FOO=bar"), "env flag missing: {log}");
    assert!(log.contains("--memory 128m"), "memory missing: {log}");
    assert!(log.contains("--cpus 1"), "cpus missing: {log}");
    assert!(
        log.contains("alpine:3.20/sh") || image_in_run(&log).as_deref() == Some("alpine:3.20"),
        "wrong image: {log}"
    );
    let cfg = sb.persist_dir("box", "config");
    assert!(
        log.contains(&format!("-v {}:/root/.config/opencode", cfg.display())),
        "persist missing: {log}"
    );
}

#[test]
fn stub_sees_the_same_bare_apply() {
    let mut sb = sandbox("scenario-bare");
    sb.runv(&common::scenarios::full("box"));
    sb.reset_log();
    sb.runv(&common::scenarios::bare("box"));

    let log = sb.log();
    assert!(
        !log.contains("-e FOO=bar"),
        "env survived a bare apply: {log}"
    );
    assert!(!log.contains("--memory"), "memory survived: {log}");
    assert!(!log.contains("opencode"), "persist survived: {log}");
}

#[test]
fn stub_sees_the_same_up_without_rebuild() {
    let mut sb = sandbox("scenario-up");
    sb.runv(&common::scenarios::bare("box"));
    sb.reset_log();
    sb.runv(&common::scenarios::up("box"));

    let log = sb.log();
    assert!(!recreated(&log), "up rebuilt: {log}");
    assert_eq!(exec_agent(&log).as_deref(), Some("sh"));
}

#[test]
fn kvm_flag_maps_the_device_in() {
    let mut sb = sandbox("kvm");
    sb.run(&["apply", "-a", "img/agent", "-n", "box", "--kvm"]);

    let log = sb.log();
    assert!(
        log.contains("--device /dev/kvm"),
        "kvm flag should map the device: {log}"
    );
    assert!(
        sb.records().contains("\"kvm\": true"),
        "kvm should be recorded: {}",
        sb.records()
    );
}

/// When config is not persisted there is no host directory for the container to
/// see, so the template is copied straight in. This pins the exact command
/// sequence `docker cp` needs to mean "copy the contents", which the stub can
/// assert but a real container only shows the result of.
#[test]
fn template_copies_the_host_config_into_a_run_that_does_not_persist_it() {
    let mut sb = sandbox("template-copy");
    let tmpl = sb.host_config_dir();
    std::fs::create_dir_all(&tmpl).expect("template dir");
    std::fs::write(tmpl.join("opencode.json"), "{}").expect("template file");

    sb.run(&["apply", "-a", "img/opencode", "-n", "box", "--template"]);

    let log = sb.log();
    assert!(
        log.contains("docker exec box sh -c mkdir -p /root/.config/opencode"),
        "the target directory must exist before the copy: {log}"
    );
    assert!(
        log.contains(&format!(
            "docker cp {}/. box:/root/.config/opencode",
            tmpl.display()
        )),
        "the contents, not the directory itself: {log}"
    );
    assert!(
        !sb.persist_dir("box", "config").exists(),
        "an unpersisted config must not be written under the data home"
    );
}

/// With config persisted there is a host directory the mount serves, so the
/// seed happens there and no `docker cp` is involved.
#[test]
fn a_persisted_config_is_seeded_rather_than_copied() {
    let mut sb = sandbox("template-nocp");
    let tmpl = sb.host_config_dir();
    std::fs::create_dir_all(&tmpl).expect("template dir");
    std::fs::write(tmpl.join("opencode.json"), "{}").expect("template file");

    sb.run(&[
        "apply",
        "-a",
        "img/opencode",
        "-n",
        "box",
        "--persist",
        "config",
        "--template",
    ]);

    assert!(
        !sb.log().contains("docker cp"),
        "a persisted config is seeded on the host, not copied: {}",
        sb.log()
    );
    assert!(
        sb.persist_dir("box", "config")
            .join("opencode.json")
            .is_file()
    );
}

/// `--template` is opt-in. A host config that exists must stay out of the
/// container when the flag is absent, even though the ephemeral path has no
/// other reason to skip it.
#[test]
fn without_template_the_host_config_is_not_copied_in() {
    let mut sb = sandbox("template-off");
    let tmpl = sb.host_config_dir();
    std::fs::create_dir_all(&tmpl).expect("template dir");
    std::fs::write(tmpl.join("opencode.json"), "{}").expect("template file");

    sb.run(&["apply", "-a", "img/opencode", "-n", "box"]);

    assert!(
        !sb.log().contains("docker cp"),
        "the host template leaked in without --template: {}",
        sb.log()
    );
}
