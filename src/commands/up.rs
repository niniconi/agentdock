use anyhow::Result;

use crate::cli::UpArgs;
use crate::config::Config;
use crate::container;
use crate::error::ContainerError;
use crate::state::StateManager;

pub fn execute_up(args: UpArgs) -> Result<()> {
    let mount_path = args.get_mount_path();
    let state = StateManager::new()?;

    // This command starts what is already configured; it never creates. So a
    // directory with no record has nothing to bring up.
    let found = match &args.name {
        Some(name) => state
            .find_by_name(name)
            .map(|record| (name.clone(), record.clone())),
        None => state
            .find_by_path(&mount_path)
            .map(|(name, record)| (name.to_string(), record.clone())),
    };

    let Some((name, record)) = found else {
        let (name, flag) = match &args.name {
            Some(n) => (format!("named '{}'", n), format!(" -n {}", n)),
            None => (
                format!("mounted at '{}'", mount_path.display()),
                String::new(),
            ),
        };
        return Err(ContainerError::NotManaged { name, flag }.into());
    };

    container::start(&name, &Config::of(&record))?;
    Ok(())
}
