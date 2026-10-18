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

/// The PERSISTED column is derived from the same table the mounts are, so an
/// agent added to it is reported without a second list to keep in step.
#[test]
fn list_verbose_reports_each_persisted_agent_directory() {
    let mut sb = sandbox("list-persisted");
    sb.run(&[
        "apply",
        "-a",
        "img/agent",
        "-n",
        "box",
        "--persist",
        "config",
    ]);

    let out = sb.run(&["list", "-v"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("box/opencode/config"), "{stdout}");
    assert!(stdout.contains("box/pi-agent/config"), "{stdout}");
    assert!(!stdout.contains("box/pi-agent/data"), "{stdout}");
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

/// `update` reads the configuration from a record, so with no record there is
/// nothing to update and the failure has to point at `apply`.
#[test]
fn update_unknown_name_explains_it() {
    let mut sb = sandbox("update-unknown");
    let out = sb.run(&["update", "-n", "ghost"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("ghost"), "{stderr}");
    assert!(stderr.contains("apply"), "{stderr}");
}

/// `stop` reads from a record, so with no record there is nothing to stop and
/// the failure has to point at `apply`.
#[test]
fn stop_unknown_name_explains_it() {
    let mut sb = sandbox("stop-unknown");
    let out = sb.run(&["stop", "-n", "ghost"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("ghost"), "{stderr}");
    assert!(stderr.contains("apply"), "{stderr}");
}

/// docker has to be reported by name when PATH has none, before anything is
/// created, rather than surfacing as a bare OS error late in the run.
#[test]
fn a_missing_docker_is_named() {
    let mut sb = sandbox("no-docker");
    let out = sb.run_without_docker(&["apply", "-a", "img/opencode", "-n", "box"]);
    assert!(!out.status.success(), "apply succeeded without docker");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("docker is not installed or not on PATH"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(sb.records().is_empty(), "a record was written anyway");
}

/// `--persist` needs the data home, and with a relative HOME and no
/// XDG_DATA_HOME there is nothing to derive it from: the failure must name the
/// problem before docker is reached.
#[test]
fn a_relative_home_cannot_host_the_data_directory() {
    let mut sb = sandbox("no-home");
    let out =
        sb.run_without_an_absolute_home(&["apply", "-a", "img/opencode", "-n", "box", "--persist"]);
    assert!(
        !out.status.success(),
        "persist succeeded without a data home"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("Cannot work out where to keep agentdock's own data"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !sb.log().contains("docker run"),
        "docker was reached before the data home failed: {}",
        sb.log()
    );
    assert!(
        !sb.mount()
            .join("relative-home/.config/agentdock/records.json")
            .exists(),
        "a record was written into a relative home"
    );
}

/// The image tag and the id `update` compares are both recorded at creation and
/// shown to the user, so both have to survive a round trip through the records
/// file. The tag is asserted with a name that appears nowhere else in the row,
/// so the check cannot be satisfied by the path column.
#[test]
fn list_verbose_shows_the_image_and_its_id() {
    let mut sb = sandbox("list-image");
    sb.run(&["apply", "-a", "myimg:v2/agent", "-n", "box"]);

    let out = sb.run(&["list", "-v"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("IMAGE_ID"), "{stdout}");
    assert!(stdout.contains("myimg:v2"), "{stdout}");
    // The stub's default id is `sha256:` + 64 a's, shown in docker's short form.
    assert!(stdout.contains("aaaaaaaaaaaa"), "{stdout}");
}

#[test]
fn status_shows_the_image_and_its_id() {
    let mut sb = sandbox("status-image");
    sb.run(&["apply", "-a", "img/agent", "-n", "box"]);

    let out = sb.run(&["status", "box"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Image:        img"), "{stdout}");
    assert!(stdout.contains("Image ID:     aaaaaaaaaaaa"), "{stdout}");
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

/// Give the sandbox a host-side Pi config to use as a template.
fn seed_host_pi_template(sb: &Sandbox) {
    let tmpl = sb.host_pi_agent_dir();
    std::fs::create_dir_all(tmpl.join("skills")).expect("template dir");
    std::fs::write(tmpl.join("settings.json"), r#"{"model":"p"}"#).expect("template file");
    std::fs::write(tmpl.join("skills/readme.md"), "pi-skill").expect("template file");
}

/// pi-agent is the default `-a`, and it contributes a config directory of its
/// own that opencode's row does not cover.
#[test]
fn pi_agent_config_is_mounted_alongside_opencode_s() {
    let mut sb = sandbox("pi-mount");
    sb.run(&[
        "apply",
        "-a",
        "img/agent",
        "-n",
        "box",
        "--persist",
        "config",
    ]);

    let log = sb.log();
    assert!(
        log.contains(&format!(
            "-v {}:/root/.pi/agent",
            sb.persist_dir_for("box", "pi-agent", "config").display()
        )),
        "pi-agent's config is missing: {log}"
    );
    assert!(
        log.contains(&format!(
            "-v {}:/root/.config/opencode",
            sb.persist_dir("box", "config").display()
        )),
        "opencode's config should still be mounted: {log}"
    );
}

/// Pi keeps its sessions inside `~/.pi/agent`, so it has no data directory to
/// contribute and `--persist data` must not invent one.
#[test]
fn persist_data_mounts_no_pi_agent_directory() {
    let mut sb = sandbox("pi-data");
    sb.run(&["apply", "-a", "img/agent", "-n", "box", "--persist", "data"]);

    let log = sb.log();
    assert!(
        log.contains(&format!(
            "-v {}:/root/.local/share/opencode",
            sb.persist_dir("box", "data").display()
        )),
        "opencode's data should be mounted: {log}"
    );
    assert!(
        !log.contains(":/root/.pi/agent"),
        "pi-agent has no data directory to persist: {log}"
    );
}

/// The whole point of resolving the template source per agent: each host config
/// has to land in its own per-container directory, and neither may appear in the
/// other's.
#[test]
fn template_seeds_each_agent_from_its_own_host_directory() {
    let mut sb = sandbox("pi-seed");
    seed_host_template(&sb);
    seed_host_pi_template(&sb);

    sb.run(&[
        "apply",
        "-a",
        "img/agent",
        "-n",
        "box",
        "--persist",
        "config",
        "--template",
    ]);

    let opencode = sb.persist_dir("box", "config");
    let pi = sb.persist_dir_for("box", "pi-agent", "config");
    assert_eq!(
        std::fs::read_to_string(opencode.join("opencode.json")).unwrap(),
        r#"{"mcp":{}}"#
    );
    assert_eq!(
        std::fs::read_to_string(pi.join("settings.json")).unwrap(),
        r#"{"model":"p"}"#,
        "pi-agent's own config was not seeded"
    );
    assert!(
        !pi.join("opencode.json").exists(),
        "opencode's config was seeded into pi-agent's directory"
    );
    assert!(
        !opencode.join("settings.json").exists(),
        "pi-agent's config was seeded into opencode's directory"
    );
}

/// The copy path, taken for a run that does not persist config: one `docker cp`
/// per supported agent whose host directory exists, each to its own target.
#[test]
fn template_copies_each_agent_directory_into_a_run_that_does_not_persist_it() {
    let mut sb = sandbox("pi-copy");
    seed_host_template(&sb);
    seed_host_pi_template(&sb);

    sb.run(&["apply", "-a", "img/agent", "-n", "box", "--template"]);

    let log = sb.log();
    assert!(
        log.contains("docker exec box sh -c mkdir -p /root/.pi/agent"),
        "the target directory must exist before the copy: {log}"
    );
    assert!(
        log.contains(&format!(
            "docker cp {}/. box:/root/.pi/agent",
            sb.host_pi_agent_dir().display()
        )),
        "pi-agent's config was not copied in: {log}"
    );
    assert!(
        !sb.persist_dir_for("box", "pi-agent", "config").exists(),
        "an unpersisted config must not be written under the data home"
    );
}

/// `--persist data` leaves the config directory unpersisted, so `--template`
/// takes the copy path for every agent — pi-agent included, since config is its
/// only directory. A data-only run must not skip the config templates, and it
/// must not create pi-agent's config under the data home either.
#[test]
fn template_copies_config_for_a_run_that_persists_only_data() {
    let mut sb = sandbox("pi-data-template");
    seed_host_pi_template(&sb);

    sb.run(&[
        "apply",
        "-a",
        "img/agent",
        "-n",
        "box",
        "--persist",
        "data",
        "--template",
    ]);

    let log = sb.log();
    assert!(
        log.contains(&format!(
            "docker cp {}/. box:/root/.pi/agent",
            sb.host_pi_agent_dir().display()
        )),
        "pi-agent's config must still be copied when only data is persisted: {log}"
    );
    assert!(
        !sb.persist_dir_for("box", "pi-agent", "config").exists(),
        "a data-only run must not create pi-agent's config under the data home"
    );
}

/// A host Pi config that exists must stay out of the container when `--template`
/// is absent, the same guarantee opencode's directory has.
#[test]
fn without_template_no_pi_agent_directory_is_copied_in() {
    let mut sb = sandbox("pi-no-template");
    seed_host_pi_template(&sb);

    sb.run(&["apply", "-a", "img/agent", "-n", "box"]);

    assert!(
        !sb.log().contains("box:/root/.pi/agent"),
        "the host Pi config leaked in without --template: {}",
        sb.log()
    );
}
