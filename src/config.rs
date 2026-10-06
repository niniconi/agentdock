use crate::cli::ApplyOpts;
use crate::state::Record;

/// Read KEY=VALUE lines from an env file.
///
/// Blank lines and lines whose first non-space character is `#` are skipped.
/// Each remaining line must be KEY=VALUE, or an error naming the line is
/// returned. Kept deliberately small: no `export ` prefixes, no quotes, no
/// interpolation.
pub fn parse_env_file(path: &std::path::Path) -> Result<Vec<String>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Cannot read env file '{}': {}", path.display(), e))?;
    let mut out = Vec::new();
    for (i, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        validate_env(line).map_err(|e| format!("{}:{}: {}", path.display(), i + 1, e))?;
        out.push(line.to_string());
    }
    Ok(out)
}

/// Merge env-file entries and -e entries the way `docker run --env-file` and
/// `-e` interact: both are honored, and `-e` wins on a duplicate key.
pub fn merge_envs(from_file: Vec<String>, from_flags: Vec<String>) -> Vec<String> {
    fn key_of(entry: &str) -> &str {
        entry.split_once('=').map(|(k, _)| k).unwrap_or(entry)
    }
    let mut out: Vec<String> = from_file;
    for entry in from_flags {
        let key = key_of(&entry);
        match out.iter().position(|e| key_of(e) == key) {
            Some(i) => out[i] = entry,
            None => out.push(entry),
        }
    }
    out
}

/// Validate port mapping format (HOST:CONTAINER)
pub fn validate_port_mapping(port: &str) -> Result<(), String> {
    let parts: Vec<&str> = port.split(':').collect();
    if parts.len() != 2 {
        return Err(format!(
            "Invalid port mapping format '{}'. Expected HOST:CONTAINER",
            port
        ));
    }
    for part in &parts {
        if part.is_empty() {
            return Err(format!("Port number cannot be empty in '{}'", port));
        }
        part.parse::<u16>()
            .map_err(|_| format!("Invalid port number '{}' in port mapping '{}'", part, port))?;
    }
    Ok(())
}

/// Validate KEY=VALUE format for a container environment variable.
pub fn validate_env(env: &str) -> Result<(), String> {
    match env.split_once('=') {
        Some((key, _)) if !key.is_empty() && !key.chars().any(char::is_whitespace) => Ok(()),
        Some(_) => Err(format!(
            "Invalid environment variable '{}': key cannot be empty or contain whitespace",
            env
        )),
        None => Err(format!(
            "Invalid environment variable '{}'. Expected KEY=VALUE",
            env
        )),
    }
}

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub docker_image: String,
    pub agent_name: String,
}

impl AgentConfig {
    /// Split `{docker_image}/{agent_name}` at the **last** slash.
    ///
    /// The last one, not the first, because an image reference carries slashes
    /// of its own: `ghcr.io/owner/nixos/opencode` is one image and one agent,
    /// and splitting at the first slash reads it as the image `ghcr.io`. That
    /// matters beyond the image field, since `agent_name` is what gets handed to
    /// `docker exec -it <name> sh -c <agent_name>`.
    pub fn parse(input: &str) -> Result<Self, String> {
        match input.rsplit_once('/') {
            Some((docker_image, agent_name)) => {
                if docker_image.is_empty() {
                    return Err("Docker image name cannot be empty".to_string());
                }
                if agent_name.is_empty() {
                    return Err("Agent name cannot be empty".to_string());
                }

                Ok(Self {
                    docker_image: docker_image.to_string(),
                    agent_name: agent_name.to_string(),
                })
            }
            None => Err(format!(
                "Invalid format: '{}', expected {{docker_image}}/{{agent_name}}, e.g.: nixos:latest/opencode",
                input
            )),
        }
    }
}

/// The state a container should be in, taken entirely from the command line.
///
/// Nothing is inherited from a record. `apply` replaces the configuration
/// rather than merging into it, so a flag left out is a flag that is turned
/// off: no ports, no proxy, no KVM. That matches `docker run`, which also
/// builds the container from the arguments it was given and nothing else.
#[derive(Debug, Clone)]
pub struct Config {
    pub docker_image: String,
    pub agent_name: String,
    /// Always a list. An empty one means no ports are published.
    pub ports: Vec<String>,
    pub http_proxy: Option<String>,
    pub https_proxy: Option<String>,
    pub init_content: Option<String>,
    pub kvm: bool,
    /// Which of opencode's directories to mount in, if any. `None` is the
    /// flag being left out, which means nothing is persisted.
    pub persist: Option<Vec<Persist>>,
    /// Environment variables passed to the container as `-e KEY=VALUE`.
    /// Always a list. An empty one means none are set.
    pub envs: Vec<String>,
}

impl Config {
    pub fn new(agent: AgentConfig, opts: &ApplyOpts) -> anyhow::Result<Self> {
        Ok(Self {
            docker_image: agent.docker_image,
            agent_name: agent.agent_name,
            ports: opts.port.clone(),
            http_proxy: opts.http_proxy.clone(),
            https_proxy: opts.https_proxy.clone(),
            init_content: None,
            kvm: opts.kvm,
            persist: opts.persist.as_deref().map(parse_persist),
            envs: match &opts.env_file {
                Some(path) => {
                    let from_file = parse_env_file(path).map_err(|e| anyhow::anyhow!(e))?;
                    merge_envs(from_file, opts.env.clone())
                }
                None => opts.env.clone(),
            },
        })
    }

    /// The configuration a record describes, for a command that reads it rather
    /// than writing it.
    pub fn of(record: &Record) -> Self {
        Self {
            docker_image: record.docker_image.clone(),
            agent_name: record.agent_name.clone(),
            ports: record.ports.clone().unwrap_or_default(),
            http_proxy: record.http_proxy.clone(),
            https_proxy: record.https_proxy.clone(),
            init_content: record.init_content.clone(),
            kvm: record.kvm,
            persist: record.persist.as_deref().map(parse_persist),
            envs: record.envs.clone().unwrap_or_default(),
        }
    }

    /// The record describing a container in this state.
    pub fn to_record(&self, path: std::path::PathBuf, created_at: String) -> Record {
        Record {
            path,
            created_at,
            init_content: self.init_content.clone(),
            http_proxy: self.http_proxy.clone(),
            https_proxy: self.https_proxy.clone(),
            ports: (!self.ports.is_empty()).then(|| self.ports.clone()),
            docker_image: self.docker_image.clone(),
            agent_name: self.agent_name.clone(),
            kvm: self.kvm,
            envs: (!self.envs.is_empty()).then(|| self.envs.clone()),
            persist: self
                .persist
                .as_ref()
                .map(|v| v.iter().map(|p| p.as_str().to_string()).collect()),
        }
    }
}

/// The directories named by `--persist`, in the order first named.
///
/// clap lets the flag repeat and accumulate, so `--persist config --persist
/// data` and `--persist config,config` both arrive as two entries. Deduped here
/// rather than rejected: the outcome is the one the user meant either way, and
/// what reaches the record is a set.
fn parse_persist(names: &[String]) -> Vec<Persist> {
    let mut out: Vec<Persist> = Vec::new();
    for name in names {
        if let Some(kind) = Persist::parse(name)
            && !out.contains(&kind)
        {
            out.push(kind);
        }
    }
    out
}

/// One of an agent's directories, as named on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Persist {
    Config,
    Data,
}

impl Persist {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "config" => Some(Self::Config),
            "data" => Some(Self::Data),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Data => "data",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid() {
        let config = AgentConfig::parse("nixos:latest/opencode").unwrap();
        assert_eq!(config.docker_image, "nixos:latest");
        assert_eq!(config.agent_name, "opencode");
    }

    #[test]
    fn test_parse_no_slash() {
        assert!(AgentConfig::parse("nixos:latest").is_err());
    }

    #[test]
    fn test_parse_empty_image() {
        assert!(AgentConfig::parse("/opencode").is_err());
    }

    #[test]
    fn test_parse_empty_agent() {
        assert!(AgentConfig::parse("nixos:latest/").is_err());
    }

    #[test]
    fn test_parse_keeps_registry_slashes_in_the_image() {
        // An image reference has slashes of its own. Splitting at the first
        // would call this image `ghcr.io` and hand `owner/nixos/opencode` to
        // docker exec as the agent.
        let config = AgentConfig::parse("ghcr.io/owner/nixos/opencode").unwrap();
        assert_eq!(config.docker_image, "ghcr.io/owner/nixos");
        assert_eq!(config.agent_name, "opencode");
    }

    #[test]
    fn test_parse_keeps_a_port_in_the_registry() {
        let config = AgentConfig::parse("localhost:5000/nixos/pi-agent").unwrap();
        assert_eq!(config.docker_image, "localhost:5000/nixos");
        assert_eq!(config.agent_name, "pi-agent");
    }
}
