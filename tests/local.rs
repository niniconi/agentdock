//! Tests that exercise agentdock's local logic (records parsing, argument
//! validation, path safety, list formatting). They never need a real daemon,
//! so the sandbox stubs `docker` out; their assertions are on stderr text,
//! exit codes and the records file rather than on docker effects.

mod common;

use common::{DockerMode, Sandbox};

fn sandbox(label: &str) -> Sandbox {
    Sandbox::new(label, DockerMode::Stub).expect("stub mode always builds")
}

fn recreated(log: &str) -> bool {
    log.contains("rm -f")
}

#[test]
fn a_malformed_records_file_is_not_overwritten() {
    let mut sb = sandbox("malformed");
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
    let mut sb = sandbox("whyrecords");
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
fn a_container_name_cannot_escape_the_data_directory() {
    let mut sb = sandbox("unsafe-name");
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
    let mut sb = sandbox("unsafe-apply");
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
fn persist_repeats_do_not_accumulate() {
    let mut sb = sandbox("persist-repeat");
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

/// The same invocation vectors tests/docker_real.rs uses against a real
/// daemon, asserted here against the stub's command log: the stub and real
/// suites drive agentdock identically, only the docker side differs.
#[test]
fn list_json_emits_the_rows_as_json() {
    let mut sb = sandbox("list-json");
    sb.run(&["apply", "-a", "img/agent", "-n", "box", "-P", "8080:80"]);

    let out = sb.run(&["list", "--format", "json", "-v"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let row = &v[0];
    assert_eq!(row[0], "box");
    // Verbose row: name, path, status, created, ports, proxies, persisted.
    assert_eq!(row[4], "8080:80", "{stdout}");
}

#[test]
fn list_verbose_shows_the_ports_column() {
    let mut sb = sandbox("list-ports");
    sb.run(&["apply", "-a", "img/agent", "-n", "box", "-P", "8080:80"]);

    let out = sb.run(&["list", "-v"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("PORTS"), "{stdout}");
    assert!(stdout.contains("8080:80"), "{stdout}");
}

#[test]
fn apply_rejects_an_invalid_port() {
    let mut sb = sandbox("bad-port");
    let out = sb.run(&["apply", "-a", "img/agent", "-n", "box", "-P", "80:"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("8080")
            || String::from_utf8_lossy(&out.stderr).contains("Invalid port")
            || String::from_utf8_lossy(&out.stderr).contains("Port"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn apply_rejects_an_invalid_env_entry() {
    let mut sb = sandbox("bad-env");
    let out = sb.run(&["apply", "-a", "img/agent", "-n", "box", "-e", "NOEQUALS"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("KEY=VALUE"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn apply_rejects_an_invalid_memory_and_cpus() {
    let mut sb = sandbox("bad-limits");
    let out = sb.run(&["apply", "-a", "img/agent", "-n", "box", "--memory", "lots"]);
    assert!(!out.status.success());

    let out = sb.run(&["apply", "-a", "img/agent", "-n", "box", "--cpus", "0"]);
    assert!(!out.status.success());
}

#[test]
fn init_with_a_missing_file_fails_before_creating_anything() {
    let mut sb = sandbox("init-missing");
    let out = sb.run(&["apply", "-a", "img/agent", "-n", "box", "--init", "nope.sh"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("Failed to read init script"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !sb.log().contains("docker run"),
        "no container should be created when the init script is unreadable"
    );
}

#[test]
fn delete_unknown_name_explains_it() {
    let mut sb = sandbox("delete-unknown");
    let out = sb.run(&["delete", "ghost", "--force"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("ghost"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn startup_migrates_an_unstamped_records_file() {
    let mut sb = sandbox("mig-records");
    std::fs::create_dir_all(sb.root.join("home/.local/share/agentdock/box")).expect("data home");
    sb.write_records(r#"{"records":{"box":{"path":"/tmp","created_at":"x","docker_image":"img","agent_name":"a","kvm":false}}}"#);

    let out = sb.run(&["list"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let records = sb.records();
    assert!(
        records.contains("\"version\": 1"),
        "version not stamped: {records}"
    );
    assert!(records.contains("\"box\""), "records lost: {records}");
    assert!(
        sb.root
            .join("home/.config/agentdock/records.json.bak.v0")
            .exists(),
        "no backup written"
    );
    let version_file = sb.root.join("home/.local/share/agentdock/version");
    assert_eq!(std::fs::read_to_string(version_file).unwrap(), "1\n");
}

#[test]
fn startup_refuses_a_newer_records_version() {
    let mut sb = sandbox("mig-newer");
    sb.write_records(r#"{"version":99,"records":{}}"#);

    let out = sb.run(&["list"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("Upgrade agentdock"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_records_file_at_the_current_version_is_left_alone() {
    let mut sb = sandbox("mig-current");
    sb.write_records(r#"{"version":1,"records":{}}"#);

    let out = sb.run(&["list"]);
    assert!(out.status.success());
    assert!(
        !sb.root
            .join("home/.config/agentdock/records.json.bak.v0")
            .exists(),
        "no backup should be written for a current-version file"
    );
}

#[test]
fn the_layout_version_file_is_stamped_when_missing() {
    let mut sb = sandbox("mig-layout");
    // records.json already current, but the data home has no version file.
    sb.write_records(r#"{"version":1,"records":{}}"#);
    std::fs::create_dir_all(sb.root.join("home/.local/share/agentdock/box")).expect("data home");

    let out = sb.run(&["list"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let version_file = sb.root.join("home/.local/share/agentdock/version");
    assert_eq!(std::fs::read_to_string(&version_file).unwrap(), "1\n");
    assert!(
        !sb.root
            .join("home/.local/share/agentdock/version.bak.v0")
            .exists(),
        "nothing to back up when the version file did not exist"
    );
}

/// Give the sandbox a host-side opencode config to use as a template.
fn seed_host_template(sb: &Sandbox) {
    let tmpl = sb.host_config_dir();
    std::fs::create_dir_all(tmpl.join("skills")).expect("template dir");
    std::fs::write(tmpl.join("opencode.json"), r#"{"mcp":{}}"#).expect("template file");
    std::fs::write(tmpl.join("skills/readme.md"), "from-host").expect("template file");
}

#[test]
fn template_seeds_a_persisted_config_directory() {
    let mut sb = sandbox("template-seed");
    seed_host_template(&sb);

    let out = sb.run(&[
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
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let cfg = sb.persist_dir("box", "config");
    assert_eq!(
        std::fs::read_to_string(cfg.join("opencode.json")).unwrap(),
        r#"{"mcp":{}}"#
    );
    assert_eq!(
        std::fs::read_to_string(cfg.join("skills/readme.md")).unwrap(),
        "from-host"
    );
    assert!(
        !sb.persist_dir("box", "data").exists(),
        "only config was asked for"
    );
}

#[test]
fn template_seeds_a_persisted_config_only_once() {
    let mut sb = sandbox("template-once");
    seed_host_template(&sb);

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
    let cfg = sb.persist_dir("box", "config");

    // Whatever the agent wrote must survive the next apply, and a file added to
    // the host template afterwards must not turn up: seeding happens once, when
    // the directory is first created.
    std::fs::write(cfg.join("opencode.json"), "edited-in-container").expect("edit");
    std::fs::write(sb.host_config_dir().join("added-later.json"), "added-later")
        .expect("host file");

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

    assert_eq!(
        std::fs::read_to_string(cfg.join("opencode.json")).unwrap(),
        "edited-in-container"
    );
    assert!(
        !cfg.join("added-later.json").exists(),
        "an already-existing directory was re-seeded"
    );
}

#[test]
fn template_without_a_host_directory_is_not_an_error() {
    let mut sb = sandbox("template-absent");
    // No ~/.config/opencode exists here, which must simply mean no seeding.
    let out = sb.run(&[
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
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let cfg = sb.persist_dir("box", "config");
    assert!(cfg.is_dir(), "the directory is still created");
    assert_eq!(
        std::fs::read_dir(&cfg).unwrap().count(),
        0,
        "nothing seeded"
    );
}

#[test]
fn without_template_nothing_seeds_a_new_config_directory() {
    let mut sb = sandbox("no-template");
    seed_host_template(&sb);

    sb.run(&["apply", "-a", "img/opencode", "-n", "box", "--persist"]);

    let cfg = sb.persist_dir("box", "config");
    assert!(
        std::fs::read_dir(&cfg).unwrap().count() == 0,
        "the host template leaked in without --template"
    );
}
