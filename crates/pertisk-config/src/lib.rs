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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install: Option<Install>,
    /// Console status dashboard (Serial / xterm.js).
    /// Omit for built-in defaults; set fields to override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dashboard: Option<Dashboard>,
}

/// Serial console TUI appearance and geometry.
///
/// Kernel cmdline `PERTISK_DASHBOARD_*` env vars override these when set.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Dashboard {
    /// `dracula` | `nord` | `gruvbox` | `wild-cherry` | `tokyo-night` |
    /// `catppuccin` | `solarized` | `cyberpunk` | `mono`
    ///
    /// Default when omitted: `catppuccin`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// `auto` | `ascii` | `light` | `rounded` | `heavy` | `double`
    ///
    /// Default when omitted: `bordered`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<String>,
    /// Force column count (skips size probe). Omit to auto-detect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cols: Option<u16>,
    /// Force row count (skips size probe). Omit to auto-detect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u16>,
    /// Force Unicode box-drawing even when the Serial UTF-8 probe fails.
    /// Omit to follow the probe (safer on Proxmox Serial).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utf8: Option<bool>,
}

impl Dashboard {
    pub const DEFAULT_THEME: &'static str = "catppuccin";
    /// ASCII frames by default — Unicode borders can blank Proxmox Serial
    /// when the UTF-8 probe is wrong.
    pub const DEFAULT_BORDER: &'static str = "bordered";
    /// Probe fallback only — never pinned unless YAML/env sets cols/rows.
    pub const DEFAULT_COLS: u16 = 80;
    pub const DEFAULT_ROWS: u16 = 22;

    /// Built-in console look (theme/border only).
    ///
    /// Size and UTF-8 are left unset so the console probe can run — pinning
    /// geometry that does not match the pane blanks Proxmox Serial.
    pub fn builtin() -> Self {
        Self {
            theme: Some(Self::DEFAULT_THEME.into()),
            border: Some(Self::DEFAULT_BORDER.into()),
            cols: None,
            rows: None,
            utf8: None,
        }
    }
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
    /// Logical cluster name (kubeconfig context / cluster entry).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub endpoint: String,
    #[serde(default)]
    pub token: Option<String>,
    /// PEM-encoded cluster CA certificate.
    #[serde(default)]
    pub ca: Option<String>,
    /// PEM-encoded cluster CA private key (control-plane bootstrap only).
    #[serde(default, rename = "caKey")]
    pub ca_key: Option<String>,
    /// PEM-encoded service-account signing key (control-plane bootstrap only).
    #[serde(default, rename = "saKey")]
    pub sa_key: Option<String>,
    /// Cluster-wide pod network CIDR (e.g. `10.244.0.0/16`).
    #[serde(default, rename = "podSubnet")]
    pub pod_subnet: Option<String>,
    /// Cluster service CIDR (default `10.96.0.0/12`).
    #[serde(default, rename = "serviceSubnet")]
    pub service_subnet: Option<String>,
    /// IPv6 pod CIDR when [`NetworkMode::DualStack`] (default `2001:db8:10:0::/56`).
    #[serde(default, rename = "podCidrIPv6")]
    pub pod_cidr_ipv6: Option<String>,
    /// IPv6 service CIDR when dual-stack (default `2001:db8:96:1::/112`).
    #[serde(default, rename = "serviceCidrIPv6")]
    pub service_cidr_ipv6: Option<String>,
    /// Node / cluster IP family mode. Default IPv4-only.
    #[serde(default, rename = "networkMode")]
    pub network_mode: NetworkMode,
    /// Optional IPv6 API VIP (HA dual-stack); also added to cert SANs.
    #[serde(default)]
    pub vip6: Option<String>,
    /// Kubernetes version tag for static-pod images (e.g. `v1.32.5`).
    #[serde(default, rename = "kubernetesVersion")]
    pub kubernetes_version: Option<String>,
    /// Pod CIDR for this node's bridge CNI (e.g. `10.244.0.0/24`).
    /// Unused when `cni: none` (cluster CNI DaemonSet owns networking).
    #[serde(default, rename = "podCidr")]
    pub pod_cidr: Option<String>,
    /// Pod networking mode: `bridge` (built-in) or `none` (Flannel/Cilium/etc.).
    #[serde(default)]
    pub cni: CniMode,
    /// Extra apiserver (and etcd) certificate SANs — VIP, extra DNS names, CP IPs.
    #[serde(default, rename = "certSANs")]
    pub cert_sans: Vec<String>,
}

impl Cluster {
    pub const DEFAULT_POD_CIDR_IPV6: &'static str = "2001:db8:10:0::/56";
    pub const DEFAULT_SERVICE_CIDR_IPV6: &'static str = "2001:db8:96:1::/112";

    pub fn is_dual_stack(&self) -> bool {
        matches!(self.network_mode, NetworkMode::DualStack)
    }

    /// `--service-cluster-ip-range` value for kube-apiserver / controller-manager.
    pub fn service_cluster_ip_range(&self) -> String {
        let v4 = self
            .service_subnet
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("10.96.0.0/12");
        if self.is_dual_stack() {
            let v6 = self
                .service_cidr_ipv6
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or(Self::DEFAULT_SERVICE_CIDR_IPV6);
            format!("{v4},{v6}")
        } else {
            v4.to_string()
        }
    }

    pub fn effective_pod_cidr_ipv6(&self) -> Option<&str> {
        if !self.is_dual_stack() {
            return None;
        }
        Some(
            self.pod_cidr_ipv6
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or(Self::DEFAULT_POD_CIDR_IPV6),
        )
    }

    /// `--cluster-cidr` for kube-controller-manager (IPv4 or IPv4,IPv6).
    pub fn cluster_cidr(&self) -> String {
        let v4 = self
            .pod_subnet
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("10.244.0.0/16");
        if let Some(v6) = self.effective_pod_cidr_ipv6() {
            format!("{v4},{v6}")
        } else {
            v4.to_string()
        }
    }

    /// `certSANs` plus optional `vip6` for apiserver/etcd certificates.
    pub fn pki_extra_sans(&self) -> Vec<String> {
        let mut sans = self.cert_sans.clone();
        if let Some(v6) = self.vip6.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            if !sans.iter().any(|s| s == v6) {
                sans.push(v6.to_string());
            }
        }
        sans
    }
}

/// Node / cluster IP family policy.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum NetworkMode {
    /// IPv4 only (disable IPv6 on the guest before DHCP).
    #[default]
    Ipv4,
    /// IPv4 + IPv6 (SLAAC/static v6, dual-stack Services / Cilium).
    DualStack,
}

impl NetworkMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ipv4 => "ipv4",
            Self::DualStack => "dual-stack",
        }
    }
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

    /// Keep an existing on-disk dashboard theme/border when the new YAML omits
    /// the section; clear size/utf8 pins so the console probe can run (a stale
    /// `cols`/`rows` that does not match the pane blanks Proxmox Serial).
    /// When nothing exists yet, fill [`Dashboard::builtin`].
    pub fn resolve_dashboard(&mut self, previous: Option<&MachineConfig>) {
        if self.machine.dashboard.is_some() {
            return;
        }
        if let Some(mut prev) = previous.and_then(|c| c.machine.dashboard.clone()) {
            prev.cols = None;
            prev.rows = None;
            prev.utf8 = None;
            if prev.theme.is_none() {
                prev.theme = Some(Dashboard::DEFAULT_THEME.into());
            }
            if prev.border.is_none() {
                prev.border = Some(Dashboard::DEFAULT_BORDER.into());
            }
            self.machine.dashboard = Some(prev);
        } else {
            self.machine.dashboard = Some(Dashboard::builtin());
        }
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
                dashboard: None,
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
        assert_eq!(cluster.network_mode, NetworkMode::Ipv4);
    }

    #[test]
    fn parses_dual_stack_cluster() {
        let yaml = r#"
version: v1alpha1
machine:
  type: controlplane
cluster:
  endpoint: https://10.1.1.210:6443
  networkMode: dual-stack
  podSubnet: 10.244.0.0/16
  serviceSubnet: 10.96.0.0/12
  podCidrIPv6: 2001:db8:10:0::/56
  serviceCidrIPv6: 2001:db8:96:1::/112
  vip6: fd00:1::210
  certSANs: ["10.1.1.210"]
"#;
        let cfg = MachineConfig::from_yaml(yaml).unwrap();
        let cluster = cfg.cluster.unwrap();
        assert!(cluster.is_dual_stack());
        assert_eq!(
            cluster.service_cluster_ip_range(),
            "10.96.0.0/12,2001:db8:96:1::/112"
        );
        assert_eq!(
            cluster.cluster_cidr(),
            "10.244.0.0/16,2001:db8:10:0::/56"
        );
        assert_eq!(cluster.vip6.as_deref(), Some("fd00:1::210"));
        assert!(cluster.pki_extra_sans().contains(&"fd00:1::210".into()));
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

    #[test]
    fn parses_dashboard() {
        let yaml = r#"
version: v1alpha1
machine:
  type: controlplane
  network:
    hostname: cp-1
  dashboard:
    theme: nord
    border: light
    cols: 140
    rows: 40
"#;
        let cfg = MachineConfig::from_yaml(yaml).unwrap();
        let dash = cfg.machine.dashboard.unwrap();
        assert_eq!(dash.theme.as_deref(), Some("nord"));
        assert_eq!(dash.border.as_deref(), Some("light"));
        assert_eq!(dash.cols, Some(140));
        assert_eq!(dash.rows, Some(40));
        assert_eq!(dash.utf8, None);
    }

    #[test]
    fn parses_dashboard_utf8_flag() {
        let yaml = r#"
version: v1alpha1
machine:
  type: controlplane
  dashboard:
    theme: gruvbox
    border: double
    utf8: true
"#;
        let cfg = MachineConfig::from_yaml(yaml).unwrap();
        let dash = cfg.machine.dashboard.unwrap();
        assert_eq!(dash.border.as_deref(), Some("double"));
        assert_eq!(dash.utf8, Some(true));
    }

    #[test]
    fn dashboard_optional() {
        let yaml = r#"
version: v1alpha1
machine:
  type: worker
"#;
        let cfg = MachineConfig::from_yaml(yaml).unwrap();
        assert!(cfg.machine.dashboard.is_none());
    }

    #[test]
    fn resolve_dashboard_preserves_previous() {
        let mut incoming = MachineConfig::from_yaml(
            r#"
version: v1alpha1
machine:
  type: controlplane
"#,
        )
        .unwrap();
        let previous = MachineConfig::from_yaml(
            r#"
version: v1alpha1
machine:
  type: controlplane
  dashboard:
    theme: nord
    border: rounded
    cols: 93
    rows: 25
    utf8: true
"#,
        )
        .unwrap();
        incoming.resolve_dashboard(Some(&previous));
        let dash = incoming.machine.dashboard.unwrap();
        assert_eq!(dash.theme.as_deref(), Some("nord"));
        assert_eq!(dash.border.as_deref(), Some("rounded"));
        // Size pins dropped so probe can run after gen-config apply.
        assert_eq!(dash.cols, None);
        assert_eq!(dash.rows, None);
        assert_eq!(dash.utf8, None);
    }

    #[test]
    fn resolve_dashboard_fills_builtin_when_absent() {
        let mut incoming = MachineConfig::from_yaml(
            r#"
version: v1alpha1
machine:
  type: controlplane
"#,
        )
        .unwrap();
        incoming.resolve_dashboard(None);
        let dash = incoming.machine.dashboard.unwrap();
        assert_eq!(dash.theme.as_deref(), Some("catppuccin"));
        assert_eq!(dash.border.as_deref(), Some("bordered"));
        assert_eq!(dash.cols, None);
        assert_eq!(dash.rows, None);
        assert_eq!(dash.utf8, None);
    }

    #[test]
    fn resolve_dashboard_keeps_explicit() {
        let mut incoming = MachineConfig::from_yaml(
            r#"
version: v1alpha1
machine:
  type: controlplane
  dashboard:
    theme: wild-cherry
"#,
        )
        .unwrap();
        let previous = MachineConfig::from_yaml(
            r#"
version: v1alpha1
machine:
  type: controlplane
  dashboard:
    theme: nord
"#,
        )
        .unwrap();
        incoming.resolve_dashboard(Some(&previous));
        assert_eq!(
            incoming.machine.dashboard.unwrap().theme.as_deref(),
            Some("wild-cherry")
        );
    }
}
