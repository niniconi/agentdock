use anyhow::Result;

use crate::cli::ListArgs;
use crate::docker::{ContainerStatus, DockerClient};
use crate::state::StateManager;

pub fn execute_list(args: ListArgs) -> Result<()> {
    let state = StateManager::new();
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
                record.http_proxy.clone().unwrap_or_else(|| "-".to_string()),
                record
                    .https_proxy
                    .clone()
                    .unwrap_or_else(|| "-".to_string()),
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
                "HTTP_PROXY",
                "HTTPS_PROXY",
            ],
            &rows,
        );
    } else {
        print_table(&["NAME", "PATH", "STATUS", "CREATED"], &rows);
    }

    Ok(())
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
