use std::fmt;

#[derive(Debug, Clone)]
pub enum ContainerStatus {
    Running,
    Stopped,
    NotFound,
}

impl fmt::Display for ContainerStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ContainerStatus::Running => write!(f, "Running"),
            ContainerStatus::Stopped => write!(f, "Stopped"),
            ContainerStatus::NotFound => write!(f, "Not Found"),
        }
    }
}
