//! Tests that exercise agentdock's local logic: records parsing, argument
//! validation, path safety and migrations. None needs a working daemon: the
//! sandbox's PATH either holds no docker at all, so a run that unexpectedly
//! reaches docker fails instead of being quietly stubbed, or points the docker
//! CLI at a socket that cannot answer. Assertions are on stderr, exit codes and
//! the records file. Anything that needs a real container lives in
//! tests/docker_real.rs.

mod common;

use common::{Sandbox, stderr, stdout};

fn sandbox(label: &str) -> Sandbox {
    Sandbox::local(label)
}

#[test]
fn a_malformed_records_file_is_not_overwritten() {
    let mut sb = sandbox("malformed");
    sb.write_records("{\"records\": {\"box\": {\"path\": ");

    // Treating an unparseable file as "no records" would let the next save()
    // replace whatever the user actually had, so the run must fail instead.
    // The migration runner reads the file before anything else and must stop
    // there; the parse error is the one reported, not a docker one from
    // further along.
    let out = sb.run(&["apply", "-a", "img/agent", "-n", "box"]);
    assert!(
        !out.status.success(),
        "a malformed records file must not be silently accepted: {}",
        stdout(&out)
    );
    assert!(
        stderr(&out).contains("migrating records.json"),
        "{}",
        stderr(&out)
    );
    assert_eq!(
        sb.records(),
        "{\"records\": {\"box\": {\"path\": ",
        "the malformed file was rewritten"
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
    let stderr = stderr(&out);

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
fn a_daemon_that_cannot_be_reached_is_not_a_missing_container() {
    // `delete --force` acts on "not found" by dropping the record, so with a
    // docker whose daemon cannot answer it must fail instead of reporting a
    // container that was removed without agentdock's knowledge.
    let Some(mut sb) = Sandbox::cli("no-daemon") else {
        return;
    };
    sb.write_records(&format!(
        r#"{{"records":{{"box":{{"path":"{mnt}","created_at":"2026-01-01T00:00:00+00:00","docker_image":"img","agent_name":"opencode","kvm":false}}}}}}"#,
        mnt = sb.mount().display()
    ));

    let out = sb.run_with_an_unreachable_daemon(&["delete", "box", "--force"]);

    assert!(
        !out.status.success(),
        "delete succeeded with no daemon: {}",
        stdout(&out)
    );
    // Docker's wording differs by version ("Cannot connect to the Docker
    // daemon" and "failed to connect to the docker API" are both in the wild),
    // but every version names the socket it could not reach.
    let stderr = stderr(&out);
    assert!(
        stderr.contains("no-such-docker.sock"),
        "the docker failure must be reported: {}",
        stderr
    );
    assert!(
        !stdout(&out).contains("not found"),
        "a failed docker call read as a missing container: {}",
        stdout(&out)
    );
    assert!(
        sb.records().contains("\"box\""),
        "the record was removed anyway: {}",
        sb.records()
    );
}

#[test]
fn apply_also_refuses_a_name_that_would_escape() {
    let mut sb = sandbox("unsafe-apply");
    // Same check, reached through `apply`. The directory is created before
    // docker runs, so without this an unusable name would leave directories
    // outside the base that `delete` could not then clean up.
    let out = sb.run(&["apply", "-a", "img/opencode", "-n", "../evil", "--persist"]);

    assert!(
        stderr(&out).contains("cannot be used as a container name"),
        "apply should refuse it: {out:?}"
    );
    assert!(
        !sb.root.join("home/.local/share/evil").exists(),
        "a directory was created outside the base"
    );
}

#[test]
fn apply_rejects_an_invalid_port() {
    let mut sb = sandbox("bad-port");
    let out = sb.run(&["apply", "-a", "img/agent", "-n", "box", "-P", "80:"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("Port number cannot be empty in '80:'"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn apply_rejects_an_invalid_env_entry() {
    let mut sb = sandbox("bad-env");
    let out = sb.run(&["apply", "-a", "img/agent", "-n", "box", "-e", "NOEQUALS"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("Invalid environment variable 'NOEQUALS'. Expected KEY=VALUE"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn apply_rejects_an_invalid_memory_and_cpus() {
    let mut sb = sandbox("bad-limits");
    let out = sb.run(&["apply", "-a", "img/agent", "-n", "box", "--memory", "lots"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("Invalid memory limit 'lots'"),
        "{}",
        stderr(&out)
    );

    let out = sb.run(&["apply", "-a", "img/agent", "-n", "box", "--cpus", "0"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("Invalid CPU limit '0'"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn init_with_a_missing_file_fails_before_creating_anything() {
    let mut sb = sandbox("init-missing");
    let out = sb.run(&["apply", "-a", "img/agent", "-n", "box", "--init", "nope.sh"]);
    assert!(!out.status.success());
    // The init error, and not a docker one, is what proves the run stopped
    // before any container work: there is no docker on this sandbox's PATH.
    assert!(
        stderr(&out).contains("Failed to read init script"),
        "{}",
        stderr(&out)
    );
    assert!(
        sb.records().is_empty(),
        "a record was written for a container that never started"
    );
}

#[test]
fn delete_unknown_name_explains_it() {
    let mut sb = sandbox("delete-unknown");
    let out = sb.run(&["delete", "ghost", "--force"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("ghost"), "{}", stderr(&out));
}

/// `update` reads the configuration from a record, so with no record there is
/// nothing to update and the failure has to point at `apply`.
#[test]
fn update_unknown_name_explains_it() {
    let mut sb = sandbox("update-unknown");
    let out = sb.run(&["update", "-n", "ghost"]);
    let stderr = stderr(&out);
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
    let stderr = stderr(&out);
    assert!(!out.status.success());
    assert!(stderr.contains("ghost"), "{stderr}");
    assert!(stderr.contains("apply"), "{stderr}");
}

/// docker has to be reported by name when PATH has none, before anything is
/// created, rather than surfacing as a bare OS error late in the run.
#[test]
fn a_missing_docker_is_named() {
    let mut sb = sandbox("no-docker");
    let out = sb.run(&["apply", "-a", "img/opencode", "-n", "box"]);
    assert!(!out.status.success(), "apply succeeded without docker");
    assert!(
        stderr(&out).contains("docker is not installed or not on PATH"),
        "{}",
        stderr(&out)
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
        stderr(&out).contains("Cannot work out where to keep agentdock's own data"),
        "{}",
        stderr(&out)
    );
    assert!(
        !sb.mount()
            .join("relative-home/.config/agentdock/records.json")
            .exists(),
        "a record was written into a relative home"
    );
}

#[test]
fn startup_migrates_an_unstamped_records_file() {
    let mut sb = sandbox("mig-records");
    std::fs::create_dir_all(sb.root.join("home/.local/share/agentdock/box")).expect("data home");
    sb.write_records(r#"{"records":{"box":{"path":"/tmp","created_at":"x","docker_image":"img","agent_name":"a","kvm":false}}}"#);

    let out = sb.run(&["list"]);
    assert!(out.status.success(), "{}", stderr(&out));

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
        stderr(&out).contains("Upgrade agentdock"),
        "{}",
        stderr(&out)
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
    assert!(out.status.success(), "{}", stderr(&out));

    let version_file = sb.root.join("home/.local/share/agentdock/version");
    assert_eq!(std::fs::read_to_string(&version_file).unwrap(), "1\n");
    assert!(
        !sb.root
            .join("home/.local/share/agentdock/version.bak.v0")
            .exists(),
        "nothing to back up when the version file did not exist"
    );
}
