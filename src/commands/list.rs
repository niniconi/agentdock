use anyhow::Result;

use crate::cli::ListArgs;
use crate::docker::{ContainerStatus, DockerClient};
use crate::state::StateManager;

pub fn execute_list(args: ListArgs) -> Result<()> {
    let state = StateManager::new()?;
    let records = state.get_all();

    if records.is_empty() {
        println!("No managed containers found.");
        return Ok(());
    }

    let mut rows: Vec<Vec<String>> = Vec::new();

    for (name, record) in records {
        let status = DockerClient::inspect(name).unwrap_or(ContainerStatus::NotFound);

        if !args.all && matches!(status, ContainerStatus::NotFound) {
            continue;
        }

        let status_str = match status {
            ContainerStatus::Running => "Running",
            ContainerStatus::Stopped => "Stopped",
            ContainerStatus::NotFound => "NotFound",
        };

        if args.verbose {
            rows.push(vec![
                name.clone(),
                record.path.display().to_string(),
                status_str.to_string(),
                record.created_at.clone(),
                ports(record),
                record.http_proxy.clone().unwrap_or_else(|| "-".to_string()),
                record
                    .https_proxy
                    .clone()
                    .unwrap_or_else(|| "-".to_string()),
                persisted(name, record),
            ]);
        } else {
            rows.push(vec![
                name.clone(),
                record.path.display().to_string(),
                status_str.to_string(),
                record.created_at.clone(),
            ]);
        }
    }

    if rows.is_empty() {
        println!("No managed containers found.");
        return Ok(());
    }

    if args.format == "json" {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    if args.verbose {
        print_table(
            &[
                "NAME",
                "PATH",
                "STATUS",
                "CREATED",
                "PORTS",
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "PERSISTED",
            ],
            &rows,
        );
    } else {
        print_table(&["NAME", "PATH", "STATUS", "CREATED"], &rows);
    }

    Ok(())
}

fn ports(record: &crate::state::Record) -> String {
    record
        .ports
        .clone()
        .filter(|p| !p.is_empty())
        .map(|p| p.join(" "))
        .unwrap_or_else(|| "-".to_string())
}

/// What `agentdock list -v` reports for a record's persisted directories.
///
/// Relative to `<XDG_DATA_HOME|~/.local/share>/agentdock/` rather than absolute,
/// because the prefix is the same on every row and repeating it would stretch
/// the table past the terminal. So a row reads `box/opencode/config` and the
/// prefix is the directory `--purge` removes, one level up from `box`.
///
/// The container-side paths are not stored, because they are derived from the
/// image when `apply` runs and would go stale the moment the image changed. What
/// is listed does exist on the host and can be checked.
fn persisted(name: &str, record: &crate::state::Record) -> String {
    // The record holds the names the flag took, which is what `Config::of`
    // turns back into `Persist` for the rest of the code.
    let Some(config) = crate::config::Config::of(record)
        .persist
        .filter(|k| !k.is_empty())
    else {
        return "-".to_string();
    };

    crate::persist::container_data_dirs(name, &config).join(" ")
}

pub fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();

    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.len());
            }
        }
    }

    for (i, header) in headers.iter().enumerate() {
        print!("{:<width$}  ", header, width = widths[i]);
    }
    println!();

    for (i, _) in widths.iter().enumerate() {
        print!("{}", "-".repeat(widths[i]));
        if i < widths.len() - 1 {
            print!("  ");
        }
    }
    println!();

    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                print!("{:<width$}  ", cell, width = widths[i]);
            }
        }
        println!();
    }
}
