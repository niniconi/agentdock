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

fn pulled(log: &str) -> bool {
    log.contains("docker pull ")
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

    let log = sb.log();
    assert!(
        !log.contains("docker cp"),
        "the host template leaked in without --template: {log}"
    );
    assert!(
        !log.contains(":/root/.pi/agent"),
        "a host Pi config must not reach the container without --template: {log}"
    );
}

/// The pi-agent mount in full: the host leaf the record reports and the
/// container-side path Pi reads, which is not the XGD layout opencode uses.
#[test]
fn a_persisted_pi_agent_config_is_mounted_at_its_own_path() {
    let mut sb = sandbox("pi-mount");
    sb.run(&[
        "apply",
        "-a",
        "img/pi-agent",
        "-n",
        "box",
        "--persist",
        "config",
    ]);

    let log = sb.log();
    let pi = sb.persist_dir_for("box", "pi-agent", "config");
    assert!(
        log.contains(&format!("-v {}:/root/.pi/agent", pi.display())),
        "pi-agent's config mount is missing: {log}"
    );
    assert!(
        log.contains(&format!(
            "-v {}:/root/.config/opencode",
            sb.persist_dir("box", "config").display()
        )),
        "opencode's config mount is missing: {log}"
    );
}

/// The exact command sequence the copy path needs for Pi: `docker cp` copies a
/// directory *into* a target rather than over it, so the target exists first and
/// the copy names its contents.
#[test]
fn template_copies_the_host_pi_config_into_a_run_that_does_not_persist_it() {
    let mut sb = sandbox("pi-copy");
    let pi = sb.host_pi_agent_dir();
    std::fs::create_dir_all(&pi).expect("template dir");
    std::fs::write(pi.join("settings.json"), "{}").expect("template file");

    sb.run(&["apply", "-a", "img/pi-agent", "-n", "box", "--template"]);

    let log = sb.log();
    assert!(
        log.contains("docker exec box sh -c mkdir -p /root/.pi/agent"),
        "the target directory must exist before the copy: {log}"
    );
    assert!(
        log.contains(&format!("docker cp {}/. box:/root/.pi/agent", pi.display())),
        "the contents, not the directory itself: {log}"
    );
    assert!(
        !sb.persist_dir_for("box", "pi-agent", "config").exists(),
        "an unpersisted config must not be written under the data home"
    );
}

/// `update` is the only path that pulls. It compares the image id the record
/// holds against the one the tag now resolves to, and rebuilds because they
/// differ.
#[test]
fn update_pulls_and_recreates_when_the_tag_moved() {
    let mut sb = sandbox("update-rebuild");
    sb.run(&["apply", "-a", "img/agent", "-n", "box"]);
    sb.set_image_id("sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
    sb.reset_log();

    sb.run(&["update", "-n", "box"]);

    let log = sb.log();
    assert!(pulled(&log), "update must refresh the tag: {log}");
    assert!(recreated(&log), "a moved tag must rebuild: {log}");
    assert!(
        sb.records().contains("sha256:bbbb"),
        "the rebuilt record must carry the new image id: {}",
        sb.records()
    );
}

/// The stored image reference is the one thing `update` cannot change on its
/// own: a tag that was never repointed moves nothing. `--image` names a new
/// reference, and the agent and every other recorded setting are kept.
#[test]
fn update_moves_onto_a_named_image_and_keeps_the_record() {
    let mut sb = sandbox("update-image");
    sb.run(&["apply", "-a", "img/agent", "-n", "box"]);
    sb.set_image_id("sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
    sb.reset_log();

    let out = sb.run(&["update", "-n", "box", "--image", "other:1", "--no-pull"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let log = sb.log();
    assert!(recreated(&log), "a different image must rebuild: {log}");
    assert_eq!(
        image_in_run(&log).as_deref(),
        Some("other:1"),
        "the rebuild must run the named image: {log}"
    );
    assert_recorded(&sb.records(), "other:1", "agent");
    assert!(
        sb.records().contains("sha256:bbbb"),
        "the record must capture the new image id: {}",
        sb.records()
    );
}

/// The whole point of comparing image ids rather than rebuilding blindly: an
/// unchanged tag leaves the container's identity alone while still refreshing
/// the image for the next apply.
#[test]
fn update_leaves_a_current_container_alone() {
    let mut sb = sandbox("update-noop");
    sb.run(&["apply", "-a", "img/agent", "-n", "box"]);
    sb.reset_log();

    sb.run(&["update", "-n", "box"]);

    let log = sb.log();
    assert!(pulled(&log), "update still refreshes the tag: {log}");
    assert!(
        !recreated(&log),
        "an unchanged image must not rebuild: {log}"
    );
    assert!(
        exec_agent(&log).is_none(),
        "update ensures the container is running, it does not attach the agent: {log}"
    );
}

/// `update` is a maintenance command, so it leaves a rebuilt or current
/// container in the same state `apply` does — running, agent not started. A
/// stopped one is brought back up, but never attached.
#[test]
fn update_restarts_a_stopped_container_without_attaching_the_agent() {
    let mut sb = sandbox("update-restart");
    sb.run(&["apply", "-a", "img/agent", "-n", "box"]);
    // The container was stopped outside agentdock; the tag has not moved, so the
    // no-op branch is the one that has to bring it back up.
    std::fs::remove_file(&sb.running).expect("stop the container");
    sb.reset_log();

    sb.run(&["update", "-n", "box", "--no-pull"]);

    let log = sb.log();
    assert!(
        log.contains("docker restart box"),
        "a stopped container must be restarted: {log}"
    );
    assert!(
        exec_agent(&log).is_none(),
        "restarting must not attach the agent: {log}"
    );
}

#[test]
fn update_without_pull_uses_the_local_image() {
    let mut sb = sandbox("update-nopull");
    sb.run(&["apply", "-a", "img/agent", "-n", "box"]);
    sb.set_image_id("sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc");
    sb.reset_log();

    sb.run(&["update", "-n", "box", "--no-pull"]);

    let log = sb.log();
    assert!(!pulled(&log), "--no-pull must not pull: {log}");
    assert!(
        recreated(&log),
        "the local tag moved, so it rebuilds: {log}"
    );
}

#[test]
fn update_dry_run_reports_without_recreating() {
    let mut sb = sandbox("update-dry");
    sb.run(&["apply", "-a", "img/agent", "-n", "box"]);
    sb.set_image_id("sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd");
    sb.reset_log();

    let out = sb.run(&["update", "-n", "box", "--dry-run"]);

    let log = sb.log();
    assert!(
        !recreated(&log),
        "a dry run must not touch the container: {log}"
    );
    // A dry run still refreshes the tag unless `--no-pull` is given, because the
    // id it reports is only known after the pull.
    assert!(pulled(&log), "a dry run still pulls by default: {log}");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("would be recreated"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    // Reporting must not move the record onto an id it did not rebuild onto.
    assert!(
        sb.records().contains("sha256:aaaa"),
        "the dry run rewrote the record: {}",
        sb.records()
    );
}

/// A record written before `update` existed carries no image id. It cannot be
/// shown to be current, so it is treated as out of date — the same branch a
/// moved tag takes, reached from `None` rather than from a differing id.
#[test]
fn update_treats_a_record_without_an_image_id_as_out_of_date() {
    let mut sb = sandbox("update-old-record");
    sb.write_records(&format!(
        r#"{{"records":{{"box":{{"path":"{mnt}","created_at":"2026-01-01T00:00:00+00:00","docker_image":"img","agent_name":"agent","kvm":false}}}}}}"#,
        mnt = sb.mount().display()
    ));
    sb.mark_running();

    let out = sb.run(&["update", "-n", "box", "--no-pull"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let log = sb.log();
    assert!(
        recreated(&log),
        "a record with no image id must be rebuilt: {log}"
    );
    assert!(
        sb.records().contains("sha256:aaaa"),
        "the rebuild must record the current image id: {}",
        sb.records()
    );
}

/// A record whose container was removed outside agentdock cannot be updated,
/// and must say so before it is touched — the same broken state `up` reports,
/// not the `docker rm` error the rebuild path would otherwise surface.
#[test]
fn update_reports_a_missing_container_before_touching_it() {
    let mut sb = sandbox("update-gone");
    sb.run(&["apply", "-a", "img/agent", "-n", "box"]);
    // Repoint the tag so the rebuild branch would be the one taken, then remove
    // the container: without the check the rebuild's destroy is what would fail.
    sb.set_image_id("sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff");
    sb.set_container_gone();
    sb.reset_log();

    let out = sb.run(&["update", "-n", "box", "--no-pull"]);
    assert!(
        !out.status.success(),
        "updating a container that no longer exists must fail"
    );

    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("does not exist"), "unfriendly error: {err}");

    let log = sb.log();
    assert!(
        !log.contains("docker rm"),
        "a missing container must not be destroyed: {log}"
    );
    assert!(
        !recreated(&log),
        "a missing container must not be recreated: {log}"
    );
}

/// The pull happens before the destroy, so a registry that refuses the image
/// leaves the container it could not replace untouched rather than deleted.
#[test]
fn update_does_not_touch_the_container_when_the_pull_fails() {
    let mut sb = sandbox("update-pull-fail");
    sb.run(&["apply", "-a", "img/agent", "-n", "box"]);
    sb.reset_log();
    sb.set_pull_failure();

    let out = sb.run(&["update", "-n", "box"]);
    assert!(!out.status.success(), "a failed pull must fail the update");

    let log = sb.log();
    assert!(pulled(&log), "the pull was attempted: {log}");
    assert!(
        !recreated(&log),
        "a failed pull must not destroy the container: {log}"
    );
}

/// With no `-n`, `update` finds its container by directory the same way `up`
/// does, so a bare call from the mounted directory locates it.
#[test]
fn update_locates_the_container_by_path() {
    let mut sb = sandbox("update-path");
    sb.run(&["apply", "-a", "img/agent", "-n", "box"]);
    sb.set_image_id("sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff");
    sb.reset_log();

    let mount = sb.mount().display().to_string();
    let out = sb.run(&["update", "--path", &mount, "--no-pull"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        recreated(&sb.log()),
        "update should have found the container by path: {}",
        sb.log()
    );
}

/// The update rebuild passes no `--init`, so the recorded script has to come
/// back from the record. Passing `None` through would silently erase it.
#[test]
fn update_keeps_the_recorded_init_script() {
    let mut sb = sandbox("update-init");
    std::fs::write(sb.mount().join("setup.sh"), "echo from-init\n").expect("write init");
    sb.run(&[
        "apply",
        "-a",
        "img/agent",
        "-n",
        "box",
        "--init",
        "setup.sh",
    ]);
    assert!(sb.records().contains("echo from-init"), "{}", sb.records());

    sb.set_image_id("sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");
    sb.reset_log();
    sb.run(&["update", "-n", "box", "--no-pull"]);

    assert!(
        sb.records().contains("echo from-init"),
        "update dropped the init script: {}",
        sb.records()
    );
    assert!(
        sb.log().contains("chmod +x /tmp/init.sh && /tmp/init.sh"),
        "the init script was not re-run on the rebuilt container: {}",
        sb.log()
    );
}

/// `stop` is the only command that shells out to `docker stop`, and it must
/// name the recorded container. Stopping is not deleting, so the record has to
/// survive for `up` to find afterwards.
#[test]
fn stop_issues_docker_stop_and_keeps_the_record() {
    let mut sb = sandbox("stop");
    sb.run(&["apply", "-a", "img/agent", "-n", "box"]);
    sb.reset_log();
    let before: serde_json::Value =
        serde_json::from_str(&sb.records()).expect("parse records before stop");

    let out = sb.runv(&common::scenarios::stop("box"));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let log = sb.log();
    assert!(
        log.contains("docker stop box"),
        "stop must name the container: {log}"
    );
    let after: serde_json::Value =
        serde_json::from_str(&sb.records()).expect("parse records after stop");
    assert_eq!(
        after["records"], before["records"],
        "stop must leave the records untouched"
    );
}

/// A stopped container is the state the user asked for, so `stop` succeeds
/// without issuing a second `docker stop`.
#[test]
fn stop_reports_a_container_that_is_already_stopped() {
    let mut sb = sandbox("stop-already");
    sb.run(&["apply", "-a", "img/agent", "-n", "box"]);
    // The stub's `stop` clears its running marker, so the container is now down.
    sb.run(&["stop", "-n", "box"]);
    sb.reset_log();

    let out = sb.run(&["stop", "-n", "box"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !sb.log().contains("docker stop"),
        "an already-stopped container is not stopped again: {}",
        sb.log()
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("already stopped"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}
