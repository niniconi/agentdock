//! Behavioral tests that need a real daemon.
//!
//! These used to run against a stub `docker`; they now assert against the
//! daemon itself (inspect output, cgroup limits, mount destinations, records
//! files). Every test returns early (passing) when no daemon is reachable,
//! so a machine without docker stays green. Shared driving vectors live in
//! tests/common/scenarios and are mirrored by the stub suite.

mod common;

use common::{DockerMode, Sandbox, docker, scenarios, stderr, stdout};
use std::path::PathBuf;

fn sb(label: &str) -> Option<Sandbox> {
    Sandbox::new(label, DockerMode::Real)
}

fn real_container_id(name: &str) -> String {
    docker(&["inspect", "-f", "{{.Id}}", name])
}

fn destinations(name: &str) -> String {
    docker(&[
        "inspect",
        "-f",
        "{{range .Mounts}}{{.Destination}} {{end}}",
        name,
    ])
}

fn persist_dir(sandbox: &Sandbox, name: &str, kind: &str) -> PathBuf {
    sandbox
        .root
        .join("home/.local/share/agentdock")
        .join(name)
        .join("opencode")
        .join(kind)
}

#[test]
fn apply_configures_a_real_container() {
    let Some(mut s) = sb("apply") else { return };
    s.track("real-apply");

    let out = s.runv(&scenarios::full("real-apply"));
    assert!(out.status.success(), "apply failed: {}", stderr(&out));

    let env = docker(&["exec", "real-apply", "sh", "-c", "echo $FOO"]);
    assert_eq!(env, "bar", "env var did not reach the container");

    assert_eq!(
        docker(&["inspect", "-f", "{{.HostConfig.Memory}}", "real-apply"]),
        "134217728"
    );
    assert_eq!(
        docker(&["inspect", "-f", "{{.HostConfig.NanoCpus}}", "real-apply"]),
        "1000000000"
    );

    let mounts = destinations("real-apply");
    assert!(
        mounts.contains("/root/.config/opencode"),
        "mounts: {mounts}"
    );

    let listed = stdout(&s.run(&["list", "-v"]));
    assert!(listed.contains("real-apply"), "list -v: {listed}");

    let restart = s.runv(&scenarios::up("real-apply"));
    assert!(restart.status.success(), "up failed: {}", stderr(&restart));

    let data_dir = s.root.join("home/.local/share/agentdock/real-apply");
    assert!(data_dir.exists(), "persisted dir missing");

    let del = s.run(&["delete", "real-apply", "--force", "--purge"]);
    assert!(del.status.success(), "delete failed: {}", stderr(&del));
    assert!(!data_dir.exists(), "--purge left the data dir");
}

#[test]
fn apply_always_recreates_an_existing_container() {
    let Some(mut s) = sb("alwaysrecreate") else {
        return;
    };
    s.track("real-recreate");
    s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-recreate"]);
    let first = real_container_id("real-recreate");

    s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-recreate"]);
    let second = real_container_id("real-recreate");
    assert_ne!(first, second, "apply must rebuild the container");
}

#[test]
fn changing_image_recreates_the_container() {
    let Some(mut s) = sb("changeimage") else {
        return;
    };
    s.track("real-img");
    s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-img"]);
    s.run(&[
        "apply",
        "-a",
        "docker.io/library/alpine:3.20/sh",
        "-n",
        "real-img",
    ]);

    assert!(
        s.records().contains("docker.io/library/alpine:3.20"),
        "record not updated: {}",
        s.records()
    );
}

#[test]
fn changing_only_the_agent_recreates_the_container() {
    let Some(mut s) = sb("agentonly") else { return };
    s.track("real-agent");
    s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-agent"]);
    let first = real_container_id("real-agent");

    s.run(&["apply", "-a", "alpine:3.20/true", "-n", "real-agent"]);
    let second = real_container_id("real-agent");
    assert_ne!(first, second, "changing the agent must rebuild");
    assert!(s.records().contains("\"true\""), "{}", s.records());
}

#[test]
fn an_omitted_flag_turns_the_setting_off() {
    let Some(mut s) = sb("override") else { return };
    s.track("real-override");
    s.run(&[
        "apply",
        "-a",
        "alpine:3.20/sh",
        "-n",
        "real-override",
        "-P",
        "8080:80",
        "--http-proxy",
        "http://p:3128",
    ]);

    s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-override"]);

    let env = docker(&["inspect", "-f", "{{json .Config.Env}}", "real-override"]);
    assert!(!env.contains("HTTP_PROXY"), "proxy survived: {env}");
    let ports = docker(&[
        "inspect",
        "-f",
        "{{json .HostConfig.PortBindings}}",
        "real-override",
    ]);
    assert!(!ports.contains("8080"), "port survived: {ports}");
    assert!(!s.records().contains("HTTP_PROXY"), "{}", s.records());
    assert!(!s.records().contains("8080:80"), "{}", s.records());
}

#[test]
fn up_never_recreates_the_container() {
    let Some(mut s) = sb("upnorecreate") else {
        return;
    };
    s.track("real-up");
    s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-up"]);
    let first = real_container_id("real-up");

    s.run(&["up", "-n", "real-up"]);
    assert_eq!(real_container_id("real-up"), first, "up rebuilt");
}

#[test]
fn a_bare_up_reaches_the_container() {
    let Some(mut s) = sb("bareup") else { return };
    s.track("real-bareup");
    s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-bareup"]);

    let out = s.run(&["up"]);
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn up_refuses_to_create_a_container() {
    let Some(s) = sb("upnocreate") else { return };
    let mut s = s;
    let out = s.run(&["up", "-n", "ghost"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("apply"), "{}", stderr(&out));
}

#[test]
fn up_restarts_a_stopped_container() {
    let Some(mut s) = sb("stopped") else { return };
    s.track("real-stopped");
    s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-stopped"]);

    docker(&["stop", "real-stopped"]);
    let id_before = real_container_id("real-stopped");

    let out = s.run(&["up", "-n", "real-stopped"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        docker(&["inspect", "-f", "{{.State.Running}}", "real-stopped"]),
        "true"
    );
    assert_eq!(real_container_id("real-stopped"), id_before, "up rebuilt");
}

#[test]
fn up_does_not_demand_a_tty() {
    let Some(mut s) = sb("tty") else { return };
    s.track("real-tty");
    let apply = s.runv(&scenarios::bare("real-tty"));
    assert!(apply.status.success(), "{}", stderr(&apply));

    let up = s.runv(&scenarios::up("real-tty"));
    let combined = format!("{}{}", stdout(&up), stderr(&up));
    assert!(
        !combined.contains("not a TTY"),
        "up demanded a TTY: {combined}"
    );
}

#[test]
fn persist_mounts_both_directories_under_root() {
    let Some(mut s) = sb("persist-both") else {
        return;
    };
    s.track("real-persist");
    s.run(&[
        "apply",
        "-a",
        "alpine:3.20/sh",
        "-n",
        "real-persist",
        "--persist",
    ]);

    let dests = destinations("real-persist");
    assert!(dests.contains("/root/.config/opencode"), "{dests}");
    assert!(dests.contains("/root/.local/share/opencode"), "{dests}");
}

#[test]
fn persist_creates_the_host_directories() {
    let Some(mut s) = sb("persist-mkdir") else {
        return;
    };
    s.track("real-persistdir");
    s.run(&[
        "apply",
        "-a",
        "alpine:3.20/sh",
        "-n",
        "real-persistdir",
        "--persist",
    ]);

    for kind in ["config", "data"] {
        assert!(persist_dir(&s, "real-persistdir", kind).is_dir(), "{kind}");
    }
}

#[test]
fn persist_config_alone_mounts_one_directory() {
    let Some(mut s) = sb("persist-cfg") else {
        return;
    };
    s.track("real-persistcfg");
    s.run(&[
        "apply",
        "-a",
        "alpine:3.20/sh",
        "-n",
        "real-persistcfg",
        "--persist",
        "config",
    ]);

    let dests = destinations("real-persistcfg");
    assert!(dests.contains("/root/.config/opencode"), "{dests}");
    assert!(!dests.contains(".local/share/opencode"), "{dests}");
}

#[test]
fn persist_data_alone_mounts_one_directory() {
    let Some(mut s) = sb("persist-data") else {
        return;
    };
    s.track("real-persistdata");
    s.run(&[
        "apply",
        "-a",
        "alpine:3.20/sh",
        "-n",
        "real-persistdata",
        "--persist",
        "data",
    ]);

    let dests = destinations("real-persistdata");
    assert!(dests.contains("/root/.local/share/opencode"), "{dests}");
    assert!(!dests.contains(".config/opencode"), "{dests}");
}

#[test]
fn omitting_persist_mounts_nothing_extra() {
    let Some(mut s) = sb("persist-off") else {
        return;
    };
    s.track("real-barepersist");
    s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-barepersist"]);

    let dests = destinations("real-barepersist");
    assert!(dests.contains("/workspace"), "{dests}");
    assert!(!dests.contains("opencode"), "{dests}");
}

#[test]
fn a_bare_apply_drops_a_previously_persisted_mount() {
    let Some(mut s) = sb("persist-replace") else {
        return;
    };
    s.track("real-persistdrop");
    s.run(&[
        "apply",
        "-a",
        "alpine:3.20/sh",
        "-n",
        "real-persistdrop",
        "--persist",
    ]);
    s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-persistdrop"]);

    let dests = destinations("real-persistdrop");
    assert!(!dests.contains("opencode"), "{dests}");
    assert!(!s.records().contains("\"persist\""), "{}", s.records());
}

#[test]
fn two_entry_points_share_one_directory() {
    let Some(mut s) = sb("persist-shared") else {
        return;
    };
    s.track("real-shared");
    s.run(&[
        "apply",
        "-a",
        "alpine:3.20/sh",
        "-n",
        "real-shared",
        "--persist",
    ]);
    let first = persist_dir(&s, "real-shared", "config");
    assert!(first.is_dir());

    s.run(&[
        "apply",
        "-a",
        "alpine:3.20/true",
        "-n",
        "real-shared",
        "--persist",
    ]);
    assert!(first.is_dir(), "second entry point mounted elsewhere");
    assert!(persist_dir(&s, "real-shared", "data").is_dir());
}

#[test]
fn persist_is_recorded_so_list_can_show_it() {
    let Some(mut s) = sb("persist-record") else {
        return;
    };
    s.track("real-provenance");
    s.run(&[
        "apply",
        "-a",
        "alpine:3.20/sh",
        "-n",
        "real-provenance",
        "--persist",
        "data",
    ]);

    let records: serde_json::Value = serde_json::from_str(&s.records()).expect("parse");
    assert_eq!(
        records["records"]["real-provenance"]["persist"],
        serde_json::json!(["data"]),
        "{}",
        s.records()
    );

    let listed = stdout(&s.run(&["list", "-v"]));
    assert!(listed.contains("real-provenance/opencode/data"), "{listed}");
}

#[test]
fn up_leaves_a_persisted_container_alone() {
    let Some(mut s) = sb("persist-up") else {
        return;
    };
    s.track("real-persistup");
    s.run(&[
        "apply",
        "-a",
        "alpine:3.20/sh",
        "-n",
        "real-persistup",
        "--persist",
    ]);

    docker(&["stop", "real-persistup"]);
    let dests_before = destinations("real-persistup");

    let out = s.run(&["up", "-n", "real-persistup"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let dests_after = destinations("real-persistup");
    assert_eq!(dests_before, dests_after, "up changed the mounts");
}

#[test]
fn delete_keeps_persisted_data_by_default() {
    let Some(mut s) = sb("purge-keep") else {
        return;
    };
    s.track("real-keep");
    s.run(&[
        "apply",
        "-a",
        "alpine:3.20/sh",
        "-n",
        "real-keep",
        "--persist",
    ]);

    let out = s.run(&["delete", "real-keep", "--force"]);
    let text = stdout(&out);
    assert!(persist_dir(&s, "real-keep", "config").is_dir(), "{text}");
    assert!(text.contains("Persisted data kept at"), "{text}");
}

#[test]
fn delete_purge_removes_persisted_data() {
    let Some(mut s) = sb("purge-yes") else { return };
    s.track("real-purge");
    s.run(&[
        "apply",
        "-a",
        "alpine:3.20/sh",
        "-n",
        "real-purge",
        "--persist",
    ]);

    let out = s.run(&["delete", "real-purge", "--force", "--purge"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!persist_dir(&s, "real-purge", "config").exists());
    assert!(!persist_dir(&s, "real-purge", "data").exists());
    assert!(s.mount().exists(), "--purge must not touch the workspace");
}

#[test]
fn delete_purge_on_a_container_that_never_persisted_is_fine() {
    let Some(mut s) = sb("purge-noop") else {
        return;
    };
    s.track("real-purgebare");
    s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-purgebare"]);

    let out = s.run(&["delete", "real-purgebare", "--force", "--purge"]);
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn delete_without_purge_says_nothing_when_there_was_no_data() {
    let Some(mut s) = sb("purge-quiet") else {
        return;
    };
    s.track("real-quiet");
    s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-quiet"]);

    let out = s.run(&["delete", "real-quiet", "--force"]);
    assert!(
        !stdout(&out).contains("Persisted data kept at"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn a_missing_image_reports_the_pull_failure() {
    let Some(mut s) = sb("persist-nopull") else {
        return;
    };
    let out = s.run(&[
        "apply",
        "-a",
        "localhost:1/absent/opencode",
        "-n",
        "real-absent",
        "--persist",
    ]);
    assert!(
        !out.status.success(),
        "apply should fail for an unpullable image"
    );
    let text = stderr(&out);
    assert!(!text.is_empty(), "the error must not be empty");
}

#[test]
fn status_reports_running_and_stopped() {
    let Some(mut s) = sb("status-branches") else {
        return;
    };
    s.track("real-status");
    s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-status"]);

    let out = s.run(&["status", "real-status"]);
    assert!(stdout(&out).contains("Running"), "{}", stdout(&out));

    docker(&["stop", "real-status"]);
    let out = s.run(&["status", "real-status"]);
    assert!(stdout(&out).contains("Stopped"), "{}", stdout(&out));
}

#[test]
fn init_runs_the_script_inside_the_container() {
    let Some(mut s) = sb("init-script") else {
        return;
    };
    s.track("real-init");
    std::fs::write(s.mount().join("setup.sh"), "echo hi > /tmp/init-ran\n").expect("write init");

    let out = s.run(&[
        "apply",
        "-a",
        "alpine:3.20/sh",
        "-n",
        "real-init",
        "--init",
        "setup.sh",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));

    let check = docker(&["exec", "real-init", "test", "-f", "/tmp/init-ran"]);
    assert_eq!(check, "", "init script did not run");
}

#[test]
fn persist_follows_the_images_declared_home_on_a_real_daemon() {
    let Some(mut s) = sb("nonroot") else { return };
    s.track("real-node");

    let ctx = s.root.join("ctx");
    std::fs::create_dir_all(&ctx).expect("ctx");
    std::fs::write(
        ctx.join("Dockerfile"),
        "FROM alpine:3.20\nRUN adduser -D -h /home/node node\nUSER node\nENV HOME=/home/node\n",
    )
    .expect("dockerfile");
    let build = std::process::Command::new("docker")
        .args([
            "build",
            "-t",
            "agentdock-probe-nonroot",
            ctx.to_str().unwrap(),
        ])
        .output()
        .expect("docker build");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let out = s.run(&[
        "apply",
        "-a",
        "agentdock-probe-nonroot/sh",
        "-n",
        "real-node",
        "--persist",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));

    let dests = destinations("real-node");
    assert!(dests.contains("/home/node/.config/opencode"), "{dests}");
    assert!(!dests.contains("/root/.config"), "{dests}");

    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", "agentdock-probe-nonroot"])
        .output();
}

#[test]
fn persist_finds_the_home_through_a_registry_qualified_image() {
    let Some(mut s) = sb("persist-registry") else {
        return;
    };
    s.track("real-regpersist");
    let out = s.run(&[
        "apply",
        "-a",
        "docker.io/library/alpine:3.20/sh",
        "-n",
        "real-regpersist",
        "--persist",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));

    let dests = destinations("real-regpersist");
    assert!(dests.contains("/root/.config/opencode"), "{dests}");
}
