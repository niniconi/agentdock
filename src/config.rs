use crate::cli::ApplyOpts;
use crate::state::Record;

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

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub docker_image: String,
    pub agent_name: String,
}

impl AgentConfig {
    pub fn parse(input: &str) -> Result<Self, String> {
        if let Some(slash_pos) = input.find('/') {
            let docker_image = input[..slash_pos].to_string();
            let agent_name = input[slash_pos + 1..].to_string();

            if docker_image.is_empty() {
                return Err("Docker image name cannot be empty".to_string());
            }
            if agent_name.is_empty() {
                return Err("Agent name cannot be empty".to_string());
            }

            Ok(Self {
                docker_image,
                agent_name,
            })
        } else {
            Err(format!(
                "Invalid format: '{}', expected {{docker_image}}/{{agent_name}}, e.g.: nixos:latest/opencode",
                input
            ))
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
}

impl Config {
    pub fn new(agent: AgentConfig, opts: &ApplyOpts) -> Self {
        Self {
            docker_image: agent.docker_image,
            agent_name: agent.agent_name,
            ports: opts.port.clone(),
            http_proxy: opts.http_proxy.clone(),
            https_proxy: opts.https_proxy.clone(),
            init_content: None,
            kvm: opts.kvm,
        }
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
}
