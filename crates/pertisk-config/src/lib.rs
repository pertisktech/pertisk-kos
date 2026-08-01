//! Machine configuration for Pertisk KOS (v0 / v1alpha1).

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const CONFIG_VERSION: &str = "v1alpha1";

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("parse error: {0}")]
    Parse(#[from] serde_yaml::Error),
    #[error("unsupported version: {0}")]
    UnsupportedVersion(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MachineConfig {
    pub version: String,
    pub machine: Machine,
    #[serde(default)]
    pub cluster: Option<Cluster>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Machine {
    #[serde(rename = "type")]
    pub machine_type: MachineType,
    #[serde(default)]
    pub network: Network,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MachineType {
    Controlplane,
    Worker,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Network {
    pub hostname: Option<String>,
    #[serde(default)]
    pub interfaces: Vec<Interface>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Interface {
    pub interface: String,
    #[serde(default)]
    pub dhcp: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Cluster {
    pub endpoint: String,
    #[serde(default)]
    pub token: Option<String>,
}

impl MachineConfig {
    pub fn from_yaml(yaml: &str) -> Result<Self, ConfigError> {
        let cfg: Self = serde_yaml::from_str(yaml)?;
        if cfg.version != CONFIG_VERSION {
            return Err(ConfigError::UnsupportedVersion(cfg.version));
        }
        Ok(cfg)
    }

    pub fn example_worker() -> Self {
        Self {
            version: CONFIG_VERSION.to_string(),
            machine: Machine {
                machine_type: MachineType::Worker,
                network: Network {
                    hostname: Some("pertisk-node-1".into()),
                    interfaces: vec![Interface {
                        interface: "eth0".into(),
                        dhcp: true,
                    }],
                },
            },
            cluster: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_example_yaml() {
        let yaml = r#"
version: v1alpha1
machine:
  type: worker
  network:
    hostname: node-1
    interfaces:
      - interface: eth0
        dhcp: true
"#;
        let cfg = MachineConfig::from_yaml(yaml).unwrap();
        assert_eq!(cfg.machine.machine_type, MachineType::Worker);
        assert_eq!(cfg.machine.network.hostname.as_deref(), Some("node-1"));
    }
}
