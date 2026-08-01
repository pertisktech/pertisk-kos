//! Machine configuration for Pertisk KOS (v0 / v1alpha1).

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const CONFIG_VERSION: &str = "v1alpha1";

/// OS / package release version.
///
/// Override at compile time: `PERTISK_BUILD_VERSION=0.2.0 cargo build`
/// (also used by `make build VERSION=...` / Docker `--build-arg VERSION=`).
pub fn release_version() -> &'static str {
    match option_env!("PERTISK_BUILD_VERSION") {
        Some(v) if !v.is_empty() => v,
        _ => env!("CARGO_PKG_VERSION"),
    }
}

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
    #[serde(default)]
    pub install: Option<Install>,
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
    /// DNS nameservers (used when not assigned by DHCP).
    #[serde(default)]
    pub nameservers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Interface {
    pub interface: String,
    #[serde(default)]
    pub dhcp: bool,
    /// CIDR addresses when `dhcp` is false (e.g. `10.0.0.5/24`).
    #[serde(default)]
    pub addresses: Vec<String>,
    pub gateway: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Install {
    /// Block device to install onto (e.g. `/dev/vda`, `/dev/sda`).
    pub disk: String,
    /// Wipe existing partition table before installing.
    #[serde(default)]
    pub wipe: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Cluster {
    pub endpoint: String,
    #[serde(default)]
    pub token: Option<String>,
    /// PEM-encoded cluster CA certificate.
    #[serde(default)]
    pub ca: Option<String>,
    /// Pod CIDR for this node's bridge CNI (e.g. `10.244.0.0/24`).
    /// Unused when `cni: none` (cluster CNI DaemonSet owns networking).
    #[serde(default, rename = "podCidr")]
    pub pod_cidr: Option<String>,
    /// Pod networking mode: `bridge` (built-in) or `none` (Flannel/Cilium/etc.).
    #[serde(default)]
    pub cni: CniMode,
}

/// How pod networking is provided on the node.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CniMode {
    /// Write bridge + host-local + portmap under `/etc/cni/net.d`.
    #[default]
    Bridge,
    /// Only loopback; expect a cluster CNI DaemonSet (Flannel, Cilium, …).
    None,
}

impl CniMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bridge => "bridge",
            Self::None => "none",
        }
    }
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
                        addresses: vec![],
                        gateway: None,
                    }],
                    nameservers: vec![],
                },
                install: None,
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
  install:
    disk: /dev/vda
    wipe: true
  network:
    hostname: node-1
    interfaces:
      - interface: eth0
        dhcp: true
"#;
        let cfg = MachineConfig::from_yaml(yaml).unwrap();
        assert_eq!(cfg.machine.machine_type, MachineType::Worker);
        assert_eq!(cfg.machine.network.hostname.as_deref(), Some("node-1"));
        assert_eq!(cfg.machine.install.as_ref().unwrap().disk, "/dev/vda");
    }

    #[test]
    fn parses_static_interface() {
        let yaml = r#"
version: v1alpha1
machine:
  type: worker
  network:
    interfaces:
      - interface: eth0
        dhcp: false
        addresses: ["10.0.2.15/24"]
        gateway: "10.0.2.2"
    nameservers: ["1.1.1.1"]
"#;
        let cfg = MachineConfig::from_yaml(yaml).unwrap();
        let iface = &cfg.machine.network.interfaces[0];
        assert!(!iface.dhcp);
        assert_eq!(iface.addresses, vec!["10.0.2.15/24"]);
        assert_eq!(iface.gateway.as_deref(), Some("10.0.2.2"));
    }

    #[test]
    fn parses_cluster_with_ca() {
        let yaml = r#"
version: v1alpha1
machine:
  type: worker
  network:
    hostname: node-1
cluster:
  endpoint: https://192.168.1.10:6443
  token: abc.def
  podCidr: 10.244.0.0/24
  ca: |
    -----BEGIN CERTIFICATE-----
    MIIB
    -----END CERTIFICATE-----
"#;
        let cfg = MachineConfig::from_yaml(yaml).unwrap();
        let cluster = cfg.cluster.unwrap();
        assert_eq!(cluster.endpoint, "https://192.168.1.10:6443");
        assert_eq!(cluster.token.as_deref(), Some("abc.def"));
        assert!(cluster.ca.unwrap().contains("BEGIN CERTIFICATE"));
        assert_eq!(cluster.pod_cidr.as_deref(), Some("10.244.0.0/24"));
        assert_eq!(cluster.cni, CniMode::Bridge);
    }

    #[test]
    fn parses_cni_none() {
        let yaml = r#"
version: v1alpha1
machine:
  type: worker
cluster:
  endpoint: https://192.168.1.10:6443
  token: abc.def
  cni: none
"#;
        let cfg = MachineConfig::from_yaml(yaml).unwrap();
        assert_eq!(cfg.cluster.unwrap().cni, CniMode::None);
    }
}
