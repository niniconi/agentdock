//! The v0 -> v1 boundary for every persisted artifact.
//!
//! Before any version was stamped, the on-disk layout was already what v1
//! describes: `records.json` had exactly today's fields, the data home had
//! today's directory shape, and the marker serialized the same struct. So
//! this boundary does not move or rename anything — it only stamps the new
//! version number. Real structural migrations start at v1 -> v2.

use std::path::PathBuf;

use anyhow::Result;

use super::{Migration, atomic_write, edit_json_file};

/// How an artifact records its version.
pub enum Scheme {
    /// A `version` field inside a JSON document.
    Json,
    /// The whole file is the version number.
    Text,
}

/// One migration step: stamp `file` with the next version.
pub struct Step {
    pub file: PathBuf,
    pub scheme: Scheme,
}

impl Migration for Step {
    fn apply(&self, from: u32) -> Result<()> {
        match self.scheme {
            Scheme::Json => edit_json_file(&self.file, |value| {
                value["version"] = serde_json::json!(from + 1);
            }),
            Scheme::Text => atomic_write(&self.file, &format!("{}\n", from + 1)),
        }
    }
}
