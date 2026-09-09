pub fn container_not_found_error(name: &str) -> String {
    format!(
        r#"Error: Associated container '{}' does not exist

Possible causes:
  1. Container was manually deleted: docker rm {}
  2. Started with --rm flag, container was auto-deleted after exit
  3. Docker environment was reset

Suggested actions:
  - Re-run agentdock to start a new instance
  - Or manually clean up records: ~/.config/agentdock/records.json"#,
        name, name
    )
}
