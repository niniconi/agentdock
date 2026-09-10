use crate::cli::RunArgs;

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

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub rm: bool,
    pub herdr_sock: bool,
    pub kvm: bool,
    pub http_proxy: Option<String>,
    pub https_proxy: Option<String>,
}

impl From<&RunArgs> for RunOptions {
    fn from(args: &RunArgs) -> Self {
        Self {
            rm: args.rm,
            herdr_sock: args.herdr_sock,
            kvm: args.kvm,
            http_proxy: args.http_proxy.clone(),
            https_proxy: args.https_proxy.clone(),
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
