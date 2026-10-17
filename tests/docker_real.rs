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

/// The container's mount destinations, in a canonical order.
/// `docker inspect` does not guarantee the order of `.Mounts`
/// (it can change when a container is stopped and started),
/// so callers compare the set of destinations, never the
/// daemon's ordering.
fn destinations(name: &str) -> String {
    let inspect = docker(&[
        "inspect",
        "-f",
        "{{range .Mounts}}{{.Destination}} {{end}}",
        name,
    ]);
    let mut dests: Vec<&str> = inspect.split_whitespace().collect();
    dests.sort_unstable();
    dests.join(" ")
}

fn agent_persist_dir(sandbox: &Sandbox, name: &str, agent: &str, kind: &str) -> PathBuf {
    sandbox
        .root
        .join("home/.local/share/agentdock")
        .join(name)
        .join(agent)
        .join(kind)
}

fn persist_dir(sandbox: &Sandbox, name: &str, kind: &str) -> PathBuf {
    agent_persist_dir(sandbox, name, "opencode", kind)
}

/// Build an image from a Dockerfile body, with the context kept in the sandbox
/// so concurrent tests cannot share one.
fn build_image(sandbox: &Sandbox, tag: &str, body: &str) {
    let ctx = sandbox.root.join("ctx");
    std::fs::create_dir_all(&ctx).expect("ctx");
    std::fs::write(ctx.join("Dockerfile"), body).expect("dockerfile");
    let out = std::process::Command::new("docker")
        .args(["build", "-t", tag, ctx.to_str().unwrap()])
        .output()
        .expect("docker build");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Remove a probe image; failures are ignored because the container that used
/// it may still be around, and the sandbox removes that at the end.
fn remove_image(tag: &str) {
    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", tag])
        .output();
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

/// `--env-file` is parsed by agentdock itself, not handed to docker, so the
/// proof is that the variables reach the container's real environment and the
/// record. Comments and blank lines are skipped in the process.
#[test]
fn apply_reads_env_from_an_env_file_on_a_real_daemon() {
    let Some(mut s) = sb("env-file") else { return };
    s.track("real-envfile");
    std::fs::write(
        s.mount().join("app.env"),
        "# comment\n\nFROM_FILE=hello\nSECOND=world\n",
    )
    .expect("write env file");

    let out = s.run(&[
        "apply",
        "-a",
        "alpine:3.20/sh",
        "-n",
        "real-envfile",
        "--env-file",
        "app.env",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));

    assert_eq!(
        docker(&[
            "exec",
            "real-envfile",
            "sh",
            "-c",
            "echo $FROM_FILE $SECOND"
        ]),
        "hello world",
        "the env file did not reach the container"
    );
    let records: serde_json::Value = serde_json::from_str(&s.records()).expect("parse");
    assert_eq!(
        records["records"]["real-envfile"]["envs"],
        serde_json::json!(["FROM_FILE=hello", "SECOND=world"]),
        "{}",
        s.records()
    );
}

/// `-e` wins over the file on a duplicate key, the way `docker run` resolves
/// the two, and a key only the file carries still arrives.
#[test]
fn an_env_flag_overrides_a_duplicate_env_file_entry() {
    let Some(mut s) = sb("env-file-override") else {
        return;
    };
    s.track("real-envoverride");
    std::fs::write(s.mount().join("app.env"), "FOO=from-file\nONLY_FILE=kept\n")
        .expect("write env file");

    let out = s.run(&[
        "apply",
        "-a",
        "alpine:3.20/sh",
        "-n",
        "real-envoverride",
        "--env-file",
        "app.env",
        "-e",
        "FOO=from-flag",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));

    assert_eq!(
        docker(&[
            "exec",
            "real-envoverride",
            "sh",
            "-c",
            "echo $FOO $ONLY_FILE"
        ]),
        "from-flag kept",
        "the flag did not override the file entry"
    );
    let records: serde_json::Value = serde_json::from_str(&s.records()).expect("parse");
    assert_eq!(
        records["records"]["real-envoverride"]["envs"],
        serde_json::json!(["FOO=from-flag", "ONLY_FILE=kept"]),
        "{}",
        s.records()
    );
}

/// A proxy flag is applied as both `HTTP_PROXY` and `http_proxy`, since tools
/// read one or the other, and the same for HTTPS.
#[test]
fn apply_sets_both_proxy_cases_in_the_container() {
    let Some(mut s) = sb("proxy-env") else { return };
    s.track("real-proxyenv");
    let out = s.run(&[
        "apply",
        "-a",
        "alpine:3.20/sh",
        "-n",
        "real-proxyenv",
        "--http-proxy",
        "http://proxy.test:3128",
        "--https-proxy",
        "https://proxy.test:3128",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));

    let env = docker(&["inspect", "-f", "{{json .Config.Env}}", "real-proxyenv"]);
    for entry in [
        "\"HTTP_PROXY=http://proxy.test:3128\"",
        "\"http_proxy=http://proxy.test:3128\"",
        "\"HTTPS_PROXY=https://proxy.test:3128\"",
        "\"https_proxy=https://proxy.test:3128\"",
    ] {
        assert!(env.contains(entry), "{entry} not in {env}");
    }
}

/// The proxies are in the record, so `status` and `list -v` can report them
/// without the flags being repeated.
#[test]
fn status_and_list_show_the_recorded_proxies() {
    let Some(mut s) = sb("proxy-show") else {
        return;
    };
    s.track("real-proxyshow");
    let out = s.run(&[
        "apply",
        "-a",
        "alpine:3.20/sh",
        "-n",
        "real-proxyshow",
        "--http-proxy",
        "http://proxy.test:3128",
        "--https-proxy",
        "https://proxy.test:3128",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));

    let status = stdout(&s.run(&["status", "real-proxyshow"]));
    for expected in [
        "HTTP Proxy",
        "HTTPS Proxy",
        "http://proxy.test:3128",
        "https://proxy.test:3128",
    ] {
        assert!(status.contains(expected), "{expected} not in {status}");
    }

    let listed = stdout(&s.run(&["list", "-v"]));
    for expected in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "http://proxy.test:3128",
        "https://proxy.test:3128",
    ] {
        assert!(listed.contains(expected), "{expected} not in {listed}");
    }
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

/// `up` on a record whose container was removed outside agentdock reports the
/// broken state rather than creating anything.
#[test]
fn up_reports_a_container_removed_outside_agentdock() {
    let Some(mut s) = sb("up-gone") else { return };
    s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-upgone"]);
    docker(&["rm", "-f", "real-upgone"]);

    let out = s.run(&["up", "-n", "real-upgone"]);
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("does not exist"), "{err}");
    assert!(err.contains("apply"), "{err}");
    assert!(err.contains("delete"), "{err}");
}

#[test]
fn stop_stops_a_running_container_and_keeps_it() {
    let Some(mut s) = sb("stop") else { return };
    s.track("real-stop");
    s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-stop"]);
    let id = real_container_id("real-stop");
    let before: serde_json::Value =
        serde_json::from_str(&s.records()).expect("parse records before stop");

    let out = s.runv(&scenarios::stop("real-stop"));
    assert!(out.status.success(), "{}", stderr(&out));

    assert_eq!(
        docker(&["inspect", "-f", "{{.State.Running}}", "real-stop"]),
        "false",
        "stop left the container running"
    );
    // Stopping is not deleting: the container and its record both survive,
    // unchanged.
    assert_eq!(
        real_container_id("real-stop"),
        id,
        "stop removed the container"
    );
    let after: serde_json::Value =
        serde_json::from_str(&s.records()).expect("parse records after stop");
    assert_eq!(
        after["records"], before["records"],
        "stop must leave the records untouched"
    );
}

#[test]
fn stop_refuses_a_record_whose_container_is_gone() {
    let Some(mut s) = sb("stopgone") else { return };
    s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-stopgone"]);
    docker(&["rm", "-f", "real-stopgone"]);

    let out = s.run(&["stop", "-n", "real-stopgone"]);
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("apply"), "{err}");
    assert!(err.contains("delete"), "{err}");
}

/// A stopped container is the state the user asked for, so `stop` says so and
/// succeeds instead of stopping it again.
#[test]
fn stop_reports_a_container_that_is_already_stopped() {
    let Some(mut s) = sb("stop-already") else {
        return;
    };
    s.track("real-stopalready");
    s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-stopalready"]);
    docker(&["stop", "real-stopalready"]);
    let id = real_container_id("real-stopalready");

    let out = s.run(&["stop", "-n", "real-stopalready"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("already stopped"), "{}", stdout(&out));
    assert_eq!(
        real_container_id("real-stopalready"),
        id,
        "the container must survive a second stop"
    );
}

/// Without `-n`, `stop` finds the record by the directory the container serves,
/// the same way `up` does.
#[test]
fn a_bare_stop_reaches_the_container_by_path() {
    let Some(mut s) = sb("bare-stop") else { return };
    s.track("real-barestop");
    s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-barestop"]);

    let out = s.run(&["stop"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        docker(&["inspect", "-f", "{{.State.Running}}", "real-barestop"]),
        "false",
        "a bare stop did not stop the container"
    );
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
fn persist_mounts_every_supported_directory_under_root() {
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
    assert!(dests.contains("/root/.pi/agent"), "{dests}");
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
    assert!(
        agent_persist_dir(&s, "real-persistdir", "pi-agent", "config").is_dir(),
        "pi-agent's own directory"
    );
}

#[test]
fn persist_config_alone_mounts_no_data_directory() {
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
    assert!(dests.contains("/root/.pi/agent"), "{dests}");
    assert!(!dests.contains(".local/share/opencode"), "{dests}");
}

#[test]
fn persist_data_alone_mounts_no_config_directory() {
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
    // pi-agent contributes a config directory and no data directory, so it is
    // half of `config` and never half of `data`.
    assert!(!dests.contains(".pi/agent"), "{dests}");
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
    assert!(!dests.contains(".pi/agent"), "{dests}");
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
    assert!(!dests.contains(".pi/agent"), "{dests}");
    assert!(!s.records().contains("\"persist\""), "{}", s.records());
}

/// A bare `apply` with nothing recorded under the directory has no name to
/// take, so it invents one; the record then lets a bare `up` find it by path.
#[test]
fn a_bare_apply_generates_a_name_and_a_bare_up_reaches_it() {
    let Some(mut s) = sb("bare-apply") else {
        return;
    };
    let out = s.run(&["apply", "-a", "alpine:3.20/sh"]);
    assert!(out.status.success(), "{}", stderr(&out));

    let records: serde_json::Value = serde_json::from_str(&s.records()).expect("parse");
    let names: Vec<String> = records["records"]
        .as_object()
        .expect("records object")
        .keys()
        .cloned()
        .collect();
    assert_eq!(names.len(), 1, "{}", s.records());
    let name = &names[0];
    s.track(name);
    assert_eq!(name.len(), 36, "the generated name is not a UUID: {name}");
    assert_eq!(
        name.matches('-').count(),
        4,
        "the generated name is not a UUID: {name}"
    );
    let id = real_container_id(name);

    let out = s.run(&["up"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        real_container_id(name),
        id,
        "the bare up rebuilt the container"
    );
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

/// Without `--force`, a running container is left alone and so is its record:
/// `delete` refuses rather than guessing.
#[test]
fn delete_without_force_refuses_a_running_container() {
    let Some(mut s) = sb("delete-running") else {
        return;
    };
    s.track("real-deleterunning");
    s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-deleterunning"]);

    let out = s.run(&["delete", "real-deleterunning"]);
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("is running"), "{err}");
    assert!(err.contains("--force"), "{err}");
    assert_eq!(
        docker(&["inspect", "-f", "{{.State.Running}}", "real-deleterunning"]),
        "true",
        "the refused delete must not touch the container"
    );
    assert!(
        s.records().contains("real-deleterunning"),
        "the refused delete must not drop the record: {}",
        s.records()
    );
}

/// A stopped container is already down, so a plain `delete` removes it and its
/// record without `--force`.
#[test]
fn delete_without_force_removes_a_stopped_container() {
    let Some(mut s) = sb("delete-stopped") else {
        return;
    };
    s.track("real-deletestopped");
    s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-deletestopped"]);
    docker(&["stop", "real-deletestopped"]);

    let out = s.run(&["delete", "real-deletestopped"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        docker(&["ps", "-a", "-q", "-f", "name=real-deletestopped"]),
        "",
        "the stopped container should have been removed"
    );
    assert!(
        !s.records().contains("real-deletestopped"),
        "{}",
        s.records()
    );
}

/// The record outliving its container is not an error for `delete`: there is
/// nothing left to remove, so it says so and drops the record.
#[test]
fn delete_skips_a_container_that_is_already_gone() {
    let Some(mut s) = sb("delete-gone") else {
        return;
    };
    s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-deletegone"]);
    docker(&["rm", "-f", "real-deletegone"]);

    let out = s.run(&["delete", "real-deletegone"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("skipping removal"),
        "{}",
        stdout(&out)
    );
    assert!(!s.records().contains("real-deletegone"), "{}", s.records());
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

/// `status` names the recorded reference and the image id `update` compares,
/// cut the same short way `list` cuts it.
#[test]
fn status_reports_the_image_and_its_id() {
    let Some(mut s) = sb("status-image") else {
        return;
    };
    s.track("real-statusimage");
    s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-statusimage"]);

    let out = s.run(&["status", "real-statusimage"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    let id = docker(&["image", "inspect", "-f", "{{.Id}}", "alpine:3.20"]);
    let short: String = id.trim_start_matches("sha256:").chars().take(12).collect();
    assert!(text.contains("Image:        alpine:3.20"), "{text}");
    assert!(text.contains(&format!("Image ID:     {short}")), "{text}");
}

/// A record whose container was removed outside agentdock still reports, with
/// `NotFound` standing in for the daemon's missing container.
#[test]
fn status_reports_a_container_that_is_gone() {
    let Some(mut s) = sb("status-gone") else {
        return;
    };
    s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-statusgone"]);
    docker(&["rm", "-f", "real-statusgone"]);

    let out = s.run(&["status", "real-statusgone"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("NotFound"), "{}", stdout(&out));
}

#[test]
fn status_refuses_an_unknown_name() {
    let Some(mut s) = sb("status-ghost") else {
        return;
    };
    let out = s.run(&["status", "ghost"]);
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("ghost"), "{err}");
    assert!(err.contains("not found in managed records"), "{err}");
}

#[test]
fn list_says_when_nothing_is_managed() {
    let Some(mut s) = sb("list-empty") else {
        return;
    };
    let out = s.run(&["list"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "No managed containers found.");
}

/// A record whose container is gone is not a managed container: the plain
/// listing skips it, and `--all` is what asks to see it.
#[test]
fn list_hides_a_gone_container_unless_all_is_passed() {
    let Some(mut s) = sb("list-hides") else {
        return;
    };
    s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-listgone"]);
    docker(&["rm", "-f", "real-listgone"]);

    let hidden = s.run(&["list"]);
    assert!(hidden.status.success(), "{}", stderr(&hidden));
    assert!(
        !stdout(&hidden).contains("real-listgone"),
        "{}",
        stdout(&hidden)
    );
    assert!(
        stdout(&hidden).contains("No managed containers found."),
        "{}",
        stdout(&hidden)
    );

    let all = s.run(&["list", "--all"]);
    let text = stdout(&all);
    assert!(text.contains("real-listgone"), "{text}");
    assert!(text.contains("NotFound"), "{text}");
}

/// The same row vector the stub suite pins, produced on a real daemon: `-v
/// --format json` is a JSON array and the ports column is the `-P` flag. The
/// host port is not 8080, which another test binds, so the two can run in
/// parallel without fighting over it.
#[test]
fn list_json_emits_the_rows_as_json_on_a_real_daemon() {
    let Some(mut s) = sb("list-json") else { return };
    s.track("real-listjson");
    s.run(&[
        "apply",
        "-a",
        "alpine:3.20/sh",
        "-n",
        "real-listjson",
        "-P",
        "48080:80",
    ]);

    let out = s.run(&["list", "--format", "json", "-v"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("json");
    let row = &v[0];
    assert_eq!(row[0], "real-listjson");
    // Verbose row: name, path, status, created, ports, proxies, persisted.
    assert_eq!(row[2], "Running", "{}", stdout(&out));
    assert_eq!(row[4], "48080:80", "{}", stdout(&out));
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

/// `USER root:root` is a routine Dockerfile line and docker passes it through
/// verbatim: read whole it is not `root`, and the mount would land on
/// /home/root:root, a directory nothing writes to.
#[test]
fn root_user_with_a_group_mounts_under_root() {
    let Some(mut s) = sb("user-rootgroup") else {
        return;
    };
    s.track("real-rootgroup");
    build_image(
        &s,
        "agentdock-probe-rootgroup",
        "FROM alpine:3.20\nUSER root:root\n",
    );

    let out = s.run(&[
        "apply",
        "-a",
        "agentdock-probe-rootgroup/sh",
        "-n",
        "real-rootgroup",
        "--persist",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));

    let dests = destinations("real-rootgroup");
    assert!(dests.contains("/root/.config/opencode"), "{dests}");
    assert!(!dests.contains("root:root"), "{dests}");

    remove_image("agentdock-probe-rootgroup");
}

/// A named user with no HOME declared: `/home/<user>` is a convention rather
/// than a fact, so agentdock uses it and says so on stderr. The group suffix
/// never reaches the path.
#[test]
fn a_named_user_without_home_guesses_the_conventional_directory() {
    let Some(mut s) = sb("user-guess") else {
        return;
    };
    s.track("real-userguess");
    build_image(
        &s,
        "agentdock-probe-guess",
        "FROM alpine:3.20\nRUN adduser -D -h /home/node node\nUSER node:node\n",
    );

    let out = s.run(&[
        "apply",
        "-a",
        "agentdock-probe-guess/sh",
        "-n",
        "real-userguess",
        "--persist",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));

    let dests = destinations("real-userguess");
    assert!(dests.contains("/home/node/.config/opencode"), "{dests}");
    assert!(!dests.contains("node:node"), "{dests}");
    let err = stderr(&out);
    assert!(err.contains("declares no HOME"), "{err}");
    assert!(err.contains("/home/node"), "{err}");

    remove_image("agentdock-probe-guess");
}

/// Omitting `-a` uses the built-in default: the image `nixos` and the agent
/// `pi-agent`. There is nothing to pull under that name, so the image is built
/// locally; the test skips when a `nixos` image already exists, so it never
/// clobbers one.
#[test]
fn apply_without_an_agent_uses_the_default_on_a_real_daemon() {
    let exists = std::process::Command::new("docker")
        .args(["image", "inspect", "nixos"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if exists {
        return;
    }
    let Some(mut s) = sb("default-agent") else {
        return;
    };
    s.track("real-defaultagent");
    build_image(
        &s,
        "nixos",
        "FROM alpine:3.20\nRUN printf '#!/bin/sh\\necho pi-agent-up\\n' > /usr/local/bin/pi-agent && chmod +x /usr/local/bin/pi-agent\n",
    );

    let out = s.run(&["apply", "-n", "real-defaultagent"]);
    assert!(out.status.success(), "{}", stderr(&out));

    let records: serde_json::Value = serde_json::from_str(&s.records()).expect("parse");
    assert_eq!(
        records["records"]["real-defaultagent"]["docker_image"],
        "nixos"
    );
    assert_eq!(
        records["records"]["real-defaultagent"]["agent_name"],
        "pi-agent"
    );

    // The resolved name is what `up` executes, so it has to be the real default
    // rather than something empty that exec would ignore.
    let out = s.run(&["up", "-n", "real-defaultagent"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("Starting Agent: pi-agent"),
        "{}",
        stdout(&out)
    );

    // Removed before the image, so the tag is really gone and the guard above
    // stays honest on the next run.
    docker(&["rm", "-f", "real-defaultagent"]);
    remove_image("nixos");
}

/// `--kvm` maps the host's /dev/kvm into the container, asserted on the
/// daemon's own device list. The host has to provide the device, so the test
/// skips where it does not exist.
#[test]
fn kvm_flag_maps_the_device_in_on_a_real_daemon() {
    if !std::path::Path::new("/dev/kvm").exists() {
        return;
    }
    let Some(mut s) = sb("kvm") else { return };
    s.track("real-kvm");
    let out = s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-kvm", "--kvm"]);
    assert!(out.status.success(), "{}", stderr(&out));

    let devices = docker(&["inspect", "-f", "{{json .HostConfig.Devices}}", "real-kvm"]);
    assert!(devices.contains("/dev/kvm"), "{devices}");
    assert!(s.records().contains("\"kvm\": true"), "{}", s.records());
}

#[test]
fn template_seeds_a_persisted_config_on_a_real_daemon() {
    let Some(mut s) = sb("tmpl-seed") else { return };
    s.track("real-tmplseed");
    let tmpl = s.host_config_dir();
    std::fs::create_dir_all(&tmpl).expect("template dir");
    std::fs::write(tmpl.join("opencode.json"), "from-host").expect("template file");

    let out = s.run(&[
        "apply",
        "-a",
        "alpine:3.20/sh",
        "-n",
        "real-tmplseed",
        "--persist",
        "config",
        "--template",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        docker(&[
            "exec",
            "real-tmplseed",
            "cat",
            "/root/.config/opencode/opencode.json"
        ]),
        "from-host",
        "the seeded file is not visible in the container"
    );

    // The seed is a first-creation event: an edit the agent made through the
    // mount survives a rebuild, and the host template does not overwrite it.
    docker(&[
        "exec",
        "real-tmplseed",
        "sh",
        "-c",
        "echo edited > /root/.config/opencode/opencode.json",
    ]);
    let out = s.run(&[
        "apply",
        "-a",
        "alpine:3.20/sh",
        "-n",
        "real-tmplseed",
        "--persist",
        "config",
        "--template",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        docker(&[
            "exec",
            "real-tmplseed",
            "cat",
            "/root/.config/opencode/opencode.json"
        ]),
        "edited",
        "a reapply re-seeded an existing config directory"
    );
}

#[test]
fn template_copies_host_config_into_an_unpersisted_container() {
    let Some(mut s) = sb("tmpl-copy") else { return };
    s.track("real-tmplcopy");
    let tmpl = s.host_config_dir();
    std::fs::create_dir_all(tmpl.join("skills")).expect("template dir");
    std::fs::write(tmpl.join("skills/readme.md"), "host-skill").expect("template file");

    let out = s.run(&[
        "apply",
        "-a",
        "alpine:3.20/sh",
        "-n",
        "real-tmplcopy",
        "--template",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        docker(&[
            "exec",
            "real-tmplcopy",
            "cat",
            "/root/.config/opencode/skills/readme.md"
        ]),
        "host-skill",
        "the copied template is not visible in the container"
    );
    assert!(
        !persist_dir(&s, "real-tmplcopy", "config").exists(),
        "an unpersisted config must not be written under the data home"
    );
}

#[test]
fn without_template_the_container_has_no_host_config() {
    let Some(mut s) = sb("tmpl-off") else { return };
    s.track("real-tmploff");
    let tmpl = s.host_config_dir();
    std::fs::create_dir_all(&tmpl).expect("template dir");
    std::fs::write(tmpl.join("opencode.json"), "from-host").expect("template file");

    let out = s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-tmploff"]);
    assert!(out.status.success(), "{}", stderr(&out));

    // The host config exists, but `--template` was not given, so it must not
    // appear inside the container.
    let present = docker(&[
        "exec",
        "real-tmploff",
        "sh",
        "-c",
        "test -e /root/.config/opencode/opencode.json && echo yes || echo no",
    ]);
    assert_eq!(
        present, "no",
        "the host template leaked in without --template"
    );
}

#[test]
fn pi_agent_config_mounts_on_a_real_daemon() {
    let Some(mut s) = sb("pi-mount") else { return };
    s.track("real-pimount");

    let out = s.run(&[
        "apply",
        "-a",
        "alpine:3.20/sh",
        "-n",
        "real-pimount",
        "--persist",
        "config",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));

    let dests = destinations("real-pimount");
    assert!(dests.contains("/root/.pi/agent"), "{dests}");
    assert!(dests.contains("/root/.config/opencode"), "{dests}");

    let host = agent_persist_dir(&s, "real-pimount", "pi-agent", "config");
    assert!(host.is_dir(), "the host directory was not created");

    // Pi reads and writes this path, so the mount has to be the agent's own and
    // not a second copy of opencode's layout.
    docker(&[
        "exec",
        "real-pimount",
        "sh",
        "-c",
        "echo ok > /root/.pi/agent/marker",
    ]);
    assert_eq!(
        std::fs::read_to_string(host.join("marker"))
            .unwrap_or_default()
            .trim(),
        "ok",
        "writes through the pi-agent mount did not reach the host"
    );
}

#[test]
fn template_seeds_pi_agent_config_on_a_real_daemon() {
    let Some(mut s) = sb("pi-tmpl") else { return };
    s.track("real-pitmpl");
    let pi = s.host_pi_agent_dir();
    std::fs::create_dir_all(&pi).expect("template dir");
    std::fs::write(pi.join("settings.json"), "from-host").expect("template file");

    // Seed opencode's own host directory too, so the isolation check below is a
    // real cross-contamination test: without it, "opencode.json is absent from
    // pi's directory" would hold even if one source fed both directories.
    let oc = s.host_config_dir();
    std::fs::create_dir_all(&oc).expect("opencode template dir");
    std::fs::write(oc.join("opencode.json"), "opencode-from-host").expect("opencode template file");

    let out = s.run(&[
        "apply",
        "-a",
        "alpine:3.20/sh",
        "-n",
        "real-pitmpl",
        "--persist",
        "config",
        "--template",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));

    assert_eq!(
        docker(&[
            "exec",
            "real-pitmpl",
            "cat",
            "/root/.pi/agent/settings.json"
        ]),
        "from-host",
        "pi-agent's template was not seeded into the container"
    );
    assert_eq!(
        docker(&[
            "exec",
            "real-pitmpl",
            "cat",
            "/root/.config/opencode/opencode.json"
        ]),
        "opencode-from-host",
        "opencode's template did not land in its own directory"
    );
    // Each agent seeds from its own host directory, so opencode's file must not
    // follow pi's into pi-agent's directory.
    assert_eq!(
        docker(&[
            "exec",
            "real-pitmpl",
            "sh",
            "-c",
            "test -e /root/.pi/agent/opencode.json && echo yes || echo no",
        ]),
        "no",
        "opencode's config was seeded into pi-agent's directory"
    );

    // Seeding is a first-creation event, the same as opencode's: an edit made
    // through the mount survives a reapply.
    docker(&[
        "exec",
        "real-pitmpl",
        "sh",
        "-c",
        "echo edited > /root/.pi/agent/settings.json",
    ]);
    let out = s.run(&[
        "apply",
        "-a",
        "alpine:3.20/sh",
        "-n",
        "real-pitmpl",
        "--persist",
        "config",
        "--template",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        docker(&[
            "exec",
            "real-pitmpl",
            "cat",
            "/root/.pi/agent/settings.json"
        ]),
        "edited",
        "a reapply re-seeded an existing config directory"
    );
}

#[test]
fn template_copies_pi_agent_config_into_an_unpersisted_container() {
    let Some(mut s) = sb("pi-copy") else { return };
    s.track("real-picopy");
    let pi = s.host_pi_agent_dir();
    std::fs::create_dir_all(&pi).expect("template dir");
    std::fs::write(pi.join("settings.json"), "from-host").expect("template file");

    let out = s.run(&[
        "apply",
        "-a",
        "alpine:3.20/sh",
        "-n",
        "real-picopy",
        "--template",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));

    assert_eq!(
        docker(&[
            "exec",
            "real-picopy",
            "cat",
            "/root/.pi/agent/settings.json"
        ]),
        "from-host",
        "the copied template is not visible in the container"
    );
    assert!(
        !agent_persist_dir(&s, "real-picopy", "pi-agent", "config").exists(),
        "an unpersisted config must not be written under the data home"
    );
}

/// Update the container onto a moved tag and check the three things the rebuild
/// has to keep: the published port, the environment, and the mounted data. The
/// tag is repointed by building the same reference again, which is what a
/// rebuilt `:latest` does in practice.
#[test]
fn update_recreates_onto_a_repointed_tag_and_keeps_its_configuration() {
    let Some(mut s) = sb("update-move") else {
        return;
    };
    s.track("real-update");

    let ctx = s.root.join("ctx");
    std::fs::create_dir_all(&ctx).expect("ctx");
    let build = |body: &str| {
        std::fs::write(ctx.join("Dockerfile"), body).expect("dockerfile");
        let out = std::process::Command::new("docker")
            .args([
                "build",
                "-t",
                "agentdock-update-probe:1",
                ctx.to_str().unwrap(),
            ])
            .output()
            .expect("docker build");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    build("FROM alpine:3.20\n");

    let out = s.run(&[
        "apply",
        "-a",
        "agentdock-update-probe:1/sh",
        "-n",
        "real-update",
        "--persist",
        "-e",
        "FOO=bar",
        "-P",
        "8080:80",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));

    // Written through the mount, so a rebuild that dropped the mount would lose
    // it from the container's point of view even though it survived on the host.
    docker(&[
        "exec",
        "real-update",
        "sh",
        "-c",
        "echo kept > /root/.config/opencode/keep.txt",
    ]);
    let first = real_container_id("real-update");

    build("FROM alpine:3.20\nRUN true\n");
    let out = s.run(&["update", "-n", "real-update", "--no-pull"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        !stdout(&out).contains("Pulling image"),
        "--no-pull still pulled: {}",
        stdout(&out)
    );

    assert_ne!(
        real_container_id("real-update"),
        first,
        "update did not rebuild onto the moved tag"
    );
    assert_eq!(
        docker(&["exec", "real-update", "sh", "-c", "echo $FOO"]),
        "bar",
        "the environment was not kept"
    );
    let ports = docker(&[
        "inspect",
        "-f",
        "{{json .HostConfig.PortBindings}}",
        "real-update",
    ]);
    assert!(ports.contains("8080"), "the port was not kept: {ports}");
    assert_eq!(
        docker(&[
            "exec",
            "real-update",
            "cat",
            "/root/.config/opencode/keep.txt"
        ]),
        "kept",
        "the persisted directory did not survive the update"
    );

    let records: serde_json::Value = serde_json::from_str(&s.records()).expect("parse");
    let new_id = docker(&[
        "image",
        "inspect",
        "-f",
        "{{.Id}}",
        "agentdock-update-probe:1",
    ]);
    assert_eq!(
        records["records"]["real-update"]["image_id"],
        serde_json::json!(new_id),
        "the record did not pick up the new image id"
    );

    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", "agentdock-update-probe:1"])
        .output();
}

/// `update --image` moves the container onto a differently-tagged image, keeping
/// the recorded configuration. The two references are built separately, so the
/// named flag is the only way onto the second one.
#[test]
fn update_moves_onto_a_named_image_and_keeps_its_configuration() {
    let Some(mut s) = sb("update-image") else {
        return;
    };
    s.track("real-updateimage");

    let ctx = s.root.join("ctx");
    std::fs::create_dir_all(&ctx).expect("ctx");
    let build = |tag: &str, body: &str| {
        std::fs::write(ctx.join("Dockerfile"), body).expect("dockerfile");
        let out = std::process::Command::new("docker")
            .args(["build", "-t", tag, ctx.to_str().unwrap()])
            .output()
            .expect("docker build");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    build("agentdock-update-from:1", "FROM alpine:3.20\n");
    build("agentdock-update-to:1", "FROM alpine:3.20\nRUN true\n");

    let out = s.run(&[
        "apply",
        "-a",
        "agentdock-update-from:1/sh",
        "-n",
        "real-updateimage",
        "-e",
        "FOO=bar",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    let first = real_container_id("real-updateimage");

    let out = s.run(&[
        "update",
        "-n",
        "real-updateimage",
        "--image",
        "agentdock-update-to:1",
        "--no-pull",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));

    assert_ne!(
        real_container_id("real-updateimage"),
        first,
        "update did not rebuild onto the named image"
    );
    assert_eq!(
        docker(&["inspect", "-f", "{{.Image}}", "real-updateimage"]),
        docker(&["image", "inspect", "-f", "{{.Id}}", "agentdock-update-to:1"]),
        "the container does not run the named image"
    );
    assert_eq!(
        docker(&["exec", "real-updateimage", "sh", "-c", "echo $FOO"]),
        "bar",
        "the environment was not kept"
    );

    let records: serde_json::Value = serde_json::from_str(&s.records()).expect("parse");
    let new_id = docker(&["image", "inspect", "-f", "{{.Id}}", "agentdock-update-to:1"]);
    assert_eq!(
        records["records"]["real-updateimage"]["docker_image"],
        serde_json::json!("agentdock-update-to:1"),
        "the record did not move onto the named image"
    );
    assert_eq!(
        records["records"]["real-updateimage"]["image_id"],
        serde_json::json!(new_id),
        "the record did not capture the named image's id"
    );

    for tag in ["agentdock-update-from:1", "agentdock-update-to:1"] {
        let _ = std::process::Command::new("docker")
            .args(["rmi", "-f", tag])
            .output();
    }
}

/// `update` on a record whose container was removed outside agentdock fails the
/// same way `up` does, before the rebuild path can surface the `docker rm` error.
/// The recorded id is dropped so the rebuild branch is the one that would
/// otherwise run, regardless of the tag.
#[test]
fn update_reports_a_container_removed_outside_agentdock() {
    let Some(mut s) = sb("update-gone") else {
        return;
    };
    s.track("real-updategone");
    let out = s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-updategone"]);
    assert!(out.status.success(), "{}", stderr(&out));

    let mut records: serde_json::Value = serde_json::from_str(&s.records()).expect("parse");
    records["records"]["real-updategone"]
        .as_object_mut()
        .expect("record object")
        .remove("image_id");
    s.write_records(&serde_json::to_string(&records).expect("serialise"));
    docker(&["rm", "-f", "real-updategone"]);

    let out = s.run(&["update", "-n", "real-updategone", "--no-pull"]);
    assert!(
        !out.status.success(),
        "a missing container must fail the update"
    );
    let err = stderr(&out);
    assert!(err.contains("does not exist"), "unfriendly error: {err}");
    assert!(
        !err.contains("remove container"),
        "the rebuild's destroy should not have run: {err}"
    );
}

/// A container already on the tag's current image is left as it is: `update`
/// must not rebuild for nothing.
#[test]
fn update_leaves_an_unchanged_container_alone() {
    let Some(mut s) = sb("update-noop") else {
        return;
    };
    s.track("real-updatetime");
    s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-updatetime"]);
    let first = real_container_id("real-updatetime");

    let out = s.run(&["update", "-n", "real-updatetime", "--no-pull"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        real_container_id("real-updatetime"),
        first,
        "an unchanged image must not rebuild"
    );
    assert!(
        !stdout(&out).contains("Starting Agent"),
        "update must not attach the agent: {}",
        stdout(&out)
    );
}

/// The pull path against a real registry: after `update`, the container runs the
/// image the tag now resolves to. Asserting that invariant rather than "nothing
/// changed", since pulling `alpine:3.20` could legitimately move the tag.
#[test]
fn update_pulls_the_tag_and_lands_on_its_current_image() {
    let Some(mut s) = sb("update-pull") else {
        return;
    };
    s.track("real-updatepull");
    s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-updatepull"]);

    let out = s.run(&["update", "-n", "real-updatepull"]);
    assert!(out.status.success(), "{}", stderr(&out));

    assert_eq!(
        docker(&["inspect", "-f", "{{.Image}}", "real-updatepull"]),
        docker(&["image", "inspect", "-f", "{{.Id}}", "alpine:3.20"]),
        "the container does not run the image the tag now points at"
    );
}

/// A dry run reports what would change without changing it: the container keeps
/// its id and the record keeps the image id it had.
#[test]
fn update_dry_run_reports_without_recreating() {
    let Some(mut s) = sb("update-dry") else {
        return;
    };
    s.track("real-updatedry");

    let ctx = s.root.join("ctx");
    std::fs::create_dir_all(&ctx).expect("ctx");
    let build = |body: &str| {
        std::fs::write(ctx.join("Dockerfile"), body).expect("dockerfile");
        let out = std::process::Command::new("docker")
            .args([
                "build",
                "-t",
                "agentdock-update-dry:1",
                ctx.to_str().unwrap(),
            ])
            .output()
            .expect("docker build");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    build("FROM alpine:3.20\n");

    let out = s.run(&[
        "apply",
        "-a",
        "agentdock-update-dry:1/sh",
        "-n",
        "real-updatedry",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    let first = real_container_id("real-updatedry");
    // Parsed rather than raw: the next command stamps the records version on
    // startup, so the file's bytes legitimately change; what must not change is
    // the records themselves.
    let before: serde_json::Value =
        serde_json::from_str(&s.records()).expect("parse records before dry run");

    // The tag is repointed, so the rebuild branch is the one a real update
    // would take.
    build("FROM alpine:3.20\nRUN true\n");
    let out = s.run(&["update", "-n", "real-updatedry", "--no-pull", "--dry-run"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("would be recreated"),
        "{}",
        stdout(&out)
    );

    assert_eq!(
        real_container_id("real-updatedry"),
        first,
        "a dry run must not rebuild the container"
    );
    let after: serde_json::Value =
        serde_json::from_str(&s.records()).expect("parse records after dry run");
    assert_eq!(
        after["records"], before["records"],
        "a dry run must not rewrite the record"
    );

    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", "agentdock-update-dry:1"])
        .output();
}

/// A record written before `update` existed carries no image id, so it cannot be
/// shown to be current. It takes the rebuild branch, reached from `None` rather
/// than from a differing id.
#[test]
fn update_treats_a_record_without_an_image_id_as_out_of_date() {
    let Some(mut s) = sb("update-old-record") else {
        return;
    };
    s.track("real-updateold");
    let out = s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-updateold"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let first = real_container_id("real-updateold");

    let mut records: serde_json::Value = serde_json::from_str(&s.records()).expect("parse");
    records["records"]["real-updateold"]
        .as_object_mut()
        .expect("record object")
        .remove("image_id");
    s.write_records(&serde_json::to_string(&records).expect("serialise"));

    let out = s.run(&["update", "-n", "real-updateold", "--no-pull"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_ne!(
        real_container_id("real-updateold"),
        first,
        "a record with no image id must rebuild"
    );

    let current = docker(&["image", "inspect", "-f", "{{.Id}}", "alpine:3.20"]);
    let records: serde_json::Value = serde_json::from_str(&s.records()).expect("parse");
    assert_eq!(
        records["records"]["real-updateold"]["image_id"],
        serde_json::json!(current),
        "the rebuild must record the current image id"
    );
}

/// The rebuild passes no `--init`, so the recorded script has to come back from
/// the record and run again on the replacement container.
#[test]
fn update_reruns_the_recorded_init_script() {
    let Some(mut s) = sb("update-init") else {
        return;
    };
    s.track("real-updateinit");
    std::fs::write(s.mount().join("setup.sh"), "echo hi > /tmp/init-rerun\n").expect("write init");

    let ctx = s.root.join("ctx");
    std::fs::create_dir_all(&ctx).expect("ctx");
    let build = |tag: &str, body: &str| {
        std::fs::write(ctx.join("Dockerfile"), body).expect("dockerfile");
        let out = std::process::Command::new("docker")
            .args(["build", "-t", tag, ctx.to_str().unwrap()])
            .output()
            .expect("docker build");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    build("agentdock-update-init:1", "FROM alpine:3.20\n");
    build("agentdock-update-init:2", "FROM alpine:3.20\nRUN true\n");

    let out = s.run(&[
        "apply",
        "-a",
        "agentdock-update-init:1/sh",
        "-n",
        "real-updateinit",
        "--init",
        "setup.sh",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    docker(&["exec", "real-updateinit", "test", "-f", "/tmp/init-rerun"]);

    let out = s.run(&[
        "update",
        "-n",
        "real-updateinit",
        "--image",
        "agentdock-update-init:2",
        "--no-pull",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        docker(&["inspect", "-f", "{{.Config.Image}}", "real-updateinit"]),
        "agentdock-update-init:2",
        "the rebuild did not move onto the named image"
    );
    // `docker exec test` exits non-zero when the file is missing, and the helper
    // panics on that, so reaching this line means the script ran.
    docker(&["exec", "real-updateinit", "test", "-f", "/tmp/init-rerun"]);
    assert!(
        s.records().contains("echo hi"),
        "update dropped the init script: {}",
        s.records()
    );

    for tag in ["agentdock-update-init:1", "agentdock-update-init:2"] {
        let _ = std::process::Command::new("docker")
            .args(["rmi", "-f", tag])
            .output();
    }
}

/// `update` brings a stopped container back up but never attaches the agent:
/// the same division of labour `apply` and `up` have.
#[test]
fn update_restarts_a_stopped_container_without_attaching_the_agent() {
    let Some(mut s) = sb("update-wake") else {
        return;
    };
    s.track("real-updatewake");
    s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-updatewake"]);
    docker(&["stop", "real-updatewake"]);

    // The image has not moved, so the no-op branch is the one that has to bring
    // the container back up.
    let out = s.run(&["update", "-n", "real-updatewake", "--no-pull"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        docker(&["inspect", "-f", "{{.State.Running}}", "real-updatewake"]),
        "true",
        "update left the stopped container down"
    );
    assert!(
        !stdout(&out).contains("Starting Agent"),
        "update must not attach the agent: {}",
        stdout(&out)
    );
}

/// The pull happens before the destroy, so an image that cannot be fetched
/// leaves the container it could not replace untouched.
#[test]
fn update_leaves_the_container_alone_when_the_image_cannot_be_pulled() {
    let Some(mut s) = sb("update-pullfail") else {
        return;
    };
    s.track("real-updatepullfail");
    let out = s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-updatepullfail"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let first = real_container_id("real-updatepullfail");

    let out = s.run(&[
        "update",
        "-n",
        "real-updatepullfail",
        "--image",
        "localhost:1/absent/opencode",
    ]);
    assert!(
        !out.status.success(),
        "an unfetchable image must fail the update"
    );

    assert_eq!(
        real_container_id("real-updatepullfail"),
        first,
        "a failed pull must not destroy the container"
    );
    assert_eq!(
        docker(&["inspect", "-f", "{{.State.Running}}", "real-updatepullfail"]),
        "true",
        "a failed pull must leave the container running"
    );
}

/// With no `-n`, `update` finds its container by the directory it serves, the
/// same way `up` does.
#[test]
fn update_locates_the_container_by_path() {
    let Some(mut s) = sb("update-path") else {
        return;
    };
    s.track("real-updatepath");
    s.run(&["apply", "-a", "alpine:3.20/sh", "-n", "real-updatepath"]);

    // The sandbox runs agentdock from the mounted directory, so a bare update
    // has only the path to go on.
    let out = s.run(&["update", "--no-pull"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("already runs image"),
        "update did not find the container by path: {}",
        stdout(&out)
    );
}
