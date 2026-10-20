use anyhow::Result;
use unicode_width::UnicodeWidthStr;

use crate::cli::ListArgs;
use crate::docker::{ContainerStatus, DockerClient, short_image_id};
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
        // A docker that cannot be run is a failure to report, not a container
        // that is missing: treating the two the same would print an empty table
        // for a user whose records still name real containers.
        let status = DockerClient::inspect(name)?;

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
                record.docker_image.clone(),
                record
                    .image_id
                    .as_deref()
                    .map(short_image_id)
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
                "PORTS",
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "PERSISTED",
                "IMAGE",
                "IMAGE_ID",
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
    print!("{}", render_table(headers, rows));
}

/// Render the table with every column as wide as its widest cell in **display**
/// columns, not bytes or characters: a path holding CJK text takes two columns
/// per character, and padding by either count would misalign the rows.
pub fn render_table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = headers
        .iter()
        .map(|header| UnicodeWidthStr::width(*header))
        .collect();

    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(UnicodeWidthStr::width(cell.as_str()));
            }
        }
    }

    let mut out = String::new();

    for (i, header) in headers.iter().enumerate() {
        if i > 0 {
            out.push_str("  ");
        }
        push_cell(&mut out, header, widths[i]);
    }
    out.push('\n');

    for (i, width) in widths.iter().enumerate() {
        if i > 0 {
            out.push_str("  ");
        }
        out.push_str(&"-".repeat(*width));
    }
    out.push('\n');

    for row in rows {
        for (i, width) in widths.iter().enumerate() {
            if i > 0 {
                out.push_str("  ");
            }
            push_cell(
                &mut out,
                row.get(i).map(String::as_str).unwrap_or(""),
                *width,
            );
        }
        out.push('\n');
    }

    out
}

fn push_cell(out: &mut String, cell: &str, width: usize) {
    out.push_str(cell);
    out.push_str(&" ".repeat(width.saturating_sub(UnicodeWidthStr::width(cell))));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every line must come out the same display width. Padding by bytes or by
    /// characters would misalign the row holding the CJK path.
    #[test]
    fn columns_are_as_wide_as_their_widest_cell() {
        let table = render_table(
            &["NAME", "PATH"],
            &[
                vec!["box".to_string(), "/tmp/项目".to_string()],
                vec!["longer".to_string(), "/tmp/plain".to_string()],
            ],
        );

        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 4);

        let expected = UnicodeWidthStr::width(lines[0]);
        for line in &lines {
            assert_eq!(
                UnicodeWidthStr::width(*line),
                expected,
                "line is not aligned: {line:?} in {table:?}"
            );
        }
    }

    #[test]
    fn the_last_column_carries_no_padding() {
        let table = render_table(&["A"], &[vec!["x".to_string()]]);
        assert_eq!(table, "A\n-\nx\n");
    }
}
