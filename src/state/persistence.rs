use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::RecordError;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Record {
    pub path: PathBuf,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub init_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub http_proxy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub https_proxy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ports: Option<Vec<String>>,
    /// Image the container was created from.
    pub docker_image: String,
    /// Agent executable passed to `docker exec`.
    pub agent_name: String,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct StateManager {
    records: HashMap<String, Record>,
}

impl StateManager {
    /// Load the records, treating an unreadable or malformed file as an error.
    ///
    /// Returning an empty manager here would be worse than failing: the next
    /// `save()` would overwrite whatever the user actually had.
    pub fn new() -> Result<Self> {
        Self::load()
    }

    fn config_dir() -> Result<PathBuf> {
        let home = dirs::home_dir().context("Failed to get user home directory")?;
        Ok(home.join(".config").join("agentdock"))
    }

    fn records_path() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("records.json"))
    }

    pub fn load() -> Result<Self> {
        let path = Self::records_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        // A malformed file is an error rather than an empty table: falling back
        // to default would let the next save() overwrite what the user had.
        let data = fs::read_to_string(&path).map_err(RecordError::Read)?;
        let manager: Self = serde_json::from_str(&data).map_err(RecordError::Parse)?;
        Ok(manager)
    }

    pub fn save(&self) -> Result<()> {
        let config_dir = Self::config_dir()?;
        fs::create_dir_all(&config_dir).context("Failed to create config directory")?;

        let path = Self::records_path()?;
        let data = serde_json::to_string_pretty(self).context("Failed to serialize records")?;

        // Write to a sibling then rename, so an interrupted write cannot leave a
        // truncated file that the next load would silently discard.
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, data).context("Failed to write persistent records")?;
        fs::rename(&tmp, &path).context("Failed to replace persistent records")?;
        Ok(())
    }

    pub fn find_by_name(&self, name: &str) -> Option<&Record> {
        self.records.get(name)
    }

    pub fn find_by_path(&self, path: &Path) -> Option<(&str, &Record)> {
        let canonical = path.canonicalize().ok()?;
        for (name, record) in &self.records {
            if let Ok(record_canonical) = record.path.canonicalize() {
                if record_canonical == canonical {
                    return Some((name.as_str(), record));
                }
            }
        }
        None
    }

    /// All records whose mount path lies inside `root`.
    ///
    /// Used before relocating a repository, so containers mounted anywhere below
    /// the project root are detected rather than just the root itself.
    pub fn find_within(&self, root: &Path) -> Vec<(String, PathBuf)> {
        let canonical = match root.canonicalize() {
            Ok(p) => p,
            Err(_) => return Vec::new(),
        };

        self.records
            .iter()
            .filter_map(|(name, record)| {
                let record_canonical = record.path.canonicalize().ok()?;
                if record_canonical.starts_with(&canonical) {
                    Some((name.clone(), record_canonical))
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn insert(&mut self, name: String, record: Record) {
        self.records.insert(name, record);
    }

    pub fn get_all(&self) -> &std::collections::HashMap<String, Record> {
        &self.records
    }

    #[allow(dead_code)]
    pub fn remove(&mut self, name: &str) {
        self.records.remove(name);
    }
}

pub fn now_string() -> String {
    let now: DateTime<Utc> = Utc::now();
    now.to_rfc3339()
}
