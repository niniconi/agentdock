use anyhow::{Context, Result};
use std::path::Path;
use uuid::Uuid;

use crate::docker::DockerClient;

pub fn run_init_script(name: &str, init_content: Option<&str>) -> Result<()> {
    if let Some(content) = init_content {
        let tmp_dir = std::env::temp_dir();
        std::fs::create_dir_all(&tmp_dir)
            .with_context(|| format!("Failed to create temp dir: {}", tmp_dir.display()))?;

        let tmp_path = tmp_dir.join(format!("agentdock_init_{}.sh", Uuid::new_v4()));
        std::fs::write(&tmp_path, content)
            .with_context(|| format!("Failed to write temp file: {}", tmp_path.display()))?;

        println!("Copying init script to container...");
        DockerClient::cp(name, &tmp_path, "/tmp/init.sh")
            .with_context(|| format!("Failed to copy init script from: {}", tmp_path.display()))?;

        println!("Executing init script...");
        DockerClient::exec(name, "chmod +x /tmp/init.sh && /tmp/init.sh")?;

        let _ = std::fs::remove_file(&tmp_path);
    }
    Ok(())
}

pub fn read_init_content(init_path: &str, mount_path: &Path) -> Result<String> {
    let path = std::path::Path::new(init_path);
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        mount_path.join(path)
    };
    std::fs::read_to_string(&abs)
        .with_context(|| format!("Failed to read init script: {}", abs.display()))
}
