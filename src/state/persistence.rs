use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

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
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct StateManager {
    records: HashMap<String, Record>,
}

impl StateManager {
    pub fn new() -> Self {
        Self::load().unwrap_or_default()
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
        let data = fs::read_to_string(&path).context("Failed to read persistent records")?;
        let manager: Self =
            serde_json::from_str(&data).context("Failed to parse persistent records")?;
        Ok(manager)
    }

    pub fn save(&self) -> Result<()> {
        let config_dir = Self::config_dir()?;
        fs::create_dir_all(&config_dir).context("Failed to create config directory")?;

        let path = Self::records_path()?;
        let data = serde_json::to_string_pretty(self).context("Failed to serialize records")?;
        fs::write(&path, data).context("Failed to write persistent records")?;
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
