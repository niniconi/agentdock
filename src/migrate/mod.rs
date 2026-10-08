//! Automatic, in-place migration of the files agentdock persists.
//!
//! Every on-disk artifact carries a `version` and this module runs at
//! startup to bring each one up to the current version for that artifact.
//! The versions are independent integers, one per artifact:
//!
//! - `records.json` — the container registry, version stamped in the file.
//! - `.agentdock.json` — the worktree marker, version stamped in the file
//!   (its migration is lazy, via `marker_migrate`).
//! - the data home layout under `~/.local/share/agentdock/`, version
//!   stamped in a sibling `version` file.
//!
//! A missing version means v0, the last layout that existed
//! without version stamping. There is no downgrade path: a newer file
//! version is an error asking the user to upgrade, not something to guess
//! at. Before any rewrite, the previous file is copied next to it as
//! `<name>.bak.v<N>`, even when the current migration only stamps the
//! version, so a user who ran the wrong binary can diff by hand.
//!
//! Migrations are one file per version boundary under this directory (see
//! `v0_to_v1.rs`); each boundary's steps are registered in
//! `Artifact::steps`.

pub mod v0_to_v1;

use anyhow::{Context, Result, bail};
use std::fs;
use std::path::Path;

/// The current schema version of `records.json`.
pub const RECORDS_CURRENT: u32 = 1;
/// The current version of the data-home layout.
pub const LAYOUT_CURRENT: u32 = 1;
/// The current schema version of `.agentdock.json`.
pub const MARKER_CURRENT: u32 = 1;

/// One migration step. The runner supplies the version being
/// left behind (`from`) and the backup cadence; the step owns
/// the transform and the stamp of its own artifact. Steps are
/// ordered by the Vec they are registered in, not by a field
/// on the impl, so a step carries no version field of its own.
pub trait Migration {
    /// Move the artifact from `from` to `from + 1`. The step
    /// reads and writes its own artifact; the runner stamps
    /// nothing itself.
    fn apply(&self, from: u32) -> Result<()>;
}

pub fn run() -> Result<()> {
    for artifact in [Artifact::Records, Artifact::Layout] {
        if let Some(path) = artifact.locate()? {
            migrate(artifact, &path).with_context(|| format!("migrating {}", artifact.label()))?;
        }
    }
    Ok(())
}

/// Migrate one worktree marker. The marker lives inside each
/// project, so the startup runner cannot sweep it; `read_marker`
/// calls this lazily, passing the path it just found.
pub fn marker_migrate(path: &Path) -> Result<()> {
    migrate(Artifact::Marker, path)
}

fn migrate(artifact: Artifact, path: &Path) -> Result<()> {
    let current = artifact.current_version(path)?;
    apply_chain(
        artifact.label(),
        &artifact.backup_target(path),
        current,
        artifact.target(),
        &artifact.steps(path),
    )
}

/// One persisted artifact and everything needed to bring it up to date.
/// The step chain for each is registered in exactly this impl — `run` and
/// `marker_migrate` share it, so a new boundary is added once here.
#[derive(Clone, Copy)]
enum Artifact {
    Records,
    Layout,
    Marker,
}

impl Artifact {
    fn label(self) -> &'static str {
        match self {
            Artifact::Records => "records.json",
            Artifact::Layout => "data home layout",
            Artifact::Marker => "marker",
        }
    }

    fn target(self) -> u32 {
        match self {
            Artifact::Records => RECORDS_CURRENT,
            Artifact::Layout => LAYOUT_CURRENT,
            Artifact::Marker => MARKER_CURRENT,
        }
    }

    /// Where the artifact lives on disk. `None` means the runner
    /// has nothing to migrate: either nothing exists under that
    /// user's HOME yet (a fresh install), or — for the marker — the
    /// artifact is per-project and the caller supplies the path.
    fn locate(self) -> Result<Option<std::path::PathBuf>> {
        match self {
            Artifact::Records => {
                let path = crate::state::persistence::StateManager::records_path()?;
                Ok(path.exists().then_some(path))
            }
            Artifact::Layout => {
                let root = crate::persist::data_root()?;
                Ok(root.exists().then_some(root))
            }
            Artifact::Marker => Ok(None),
        }
    }

    /// The artifact's version, stamped per its own scheme.
    fn current_version(self, path: &Path) -> Result<u32> {
        match self {
            Artifact::Records | Artifact::Marker => json_version(path),
            Artifact::Layout => file_version(&path.join("version")),
        }
    }

    /// The file that gets backed up before a rewrite.
    fn backup_target(self, path: &Path) -> std::path::PathBuf {
        match self {
            Artifact::Records | Artifact::Marker => path.to_path_buf(),
            Artifact::Layout => path.join("version"),
        }
    }

    /// The chain that brings this artifact from v0 to its target
    /// version, one entry per version boundary, in order.
    fn steps(self, path: &Path) -> Vec<Box<dyn Migration>> {
        vec![Box::new(v0_to_v1::Step {
            file: self.backup_target(path),
            scheme: match self {
                Artifact::Layout => v0_to_v1::Scheme::Text,
                Artifact::Records | Artifact::Marker => v0_to_v1::Scheme::Json,
            },
        })]
    }
}

/// Run a chain of steps, oldest first.
///
/// The contract that makes the ordering work without a `from` on
/// every step: `steps[i]` moves the artifact from v_i to v_{i+1},
/// so the chain is indexed by the version being left behind. The
/// caller guarantees the chain is complete and contiguous from the
/// artifact's v0; a missing entry is an error, not a skip. Each
/// rewrite is preceded by a backup of `path` (which may not exist
/// yet — a v0 data home has no `version` file, and the layout step
/// is what creates it).
fn apply_chain(
    label: &str,
    path: &Path,
    current: u32,
    target: u32,
    steps: &[Box<dyn Migration>],
) -> Result<()> {
    if current > target {
        bail!(
            "{label} at {} is version {current}, but this agentdock only understands up to {target}. Upgrade agentdock.",
            path.display()
        );
    }

    let mut version = current;
    while version < target {
        let step = steps
            .get(version as usize)
            .ok_or_else(|| anyhow::anyhow!("no migration step from v{version}"))?;
        // There may be nothing on disk to back up yet — a v0 data home has
        // no `version` file, and the layout step is what creates it.
        if path.exists() {
            backup(path, version)?;
        }
        step.apply(version)?;
        version += 1;
    }
    Ok(())
}

/// The `version` field of a JSON file, or 0 when the file predates the
/// field or the file does not exist.
fn json_version(path: &Path) -> Result<u32> {
    match fs::read_to_string(path) {
        Ok(data) => {
            let value: serde_json::Value = serde_json::from_str(&data)
                .with_context(|| format!("Failed to parse {} for migration", path.display()))?;
            Ok(value.get("version").and_then(|v| v.as_u64()).unwrap_or(0) as u32)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(e).with_context(|| format!("Failed to read {}", path.display())),
    }
}

/// The integer in a version file, or 0 when missing.
fn file_version(path: &Path) -> Result<u32> {
    match fs::read_to_string(path) {
        Ok(raw) => raw
            .trim()
            .parse::<u32>()
            .with_context(|| format!("Failed to parse {} as a version", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(e).with_context(|| format!("Failed to read {}", path.display())),
    }
}

/// Copy `path` next to itself as `<name>.bak.v<version>`, so
/// a migration can be diffed or rolled back by hand.
fn backup(path: &Path, version: u32) -> Result<()> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("backup");
    let target = path.with_file_name(format!("{name}.bak.v{version}"));
    fs::copy(path, &target).with_context(|| {
        format!(
            "Failed to back up {} to {}",
            path.display(),
            target.display()
        )
    })?;
    Ok(())
}

pub(crate) fn atomic_write(path: &Path, contents: &str) -> Result<()> {
    let tmp = path.with_extension("migrating");
    fs::write(&tmp, contents).with_context(|| format!("Failed to write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("Failed to replace {}", path.display()))?;
    Ok(())
}

/// Read a JSON file, hand it to `edit`, and write it back atomically.
pub(crate) fn edit_json_file(path: &Path, edit: impl FnOnce(&mut serde_json::Value)) -> Result<()> {
    let data =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let mut value: serde_json::Value = serde_json::from_str(&data)
        .with_context(|| format!("Failed to parse {} for migration", path.display()))?;
    edit(&mut value);
    let stamped =
        serde_json::to_string_pretty(&value).context("Failed to serialize migrated file")?;
    atomic_write(path, &stamped)
}
