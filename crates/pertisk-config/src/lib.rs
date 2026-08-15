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
    #[error("{0}")]
    Msg(String),
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
    /// Kubelet settings. Applied into `/var/lib/kubelet/config.yaml`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kubelet: Option<MachineKubelet>,
    /// Optional log ship to Loki / Grafana Alloy (`loki.source.api`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observability: Option<Observability>,
}

/// `machine.kubelet` — kubelet knobs we honor.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MachineKubelet {
    /// Extra KubeletConfiguration fields merged into the written config.
    #[serde(
        default,
        rename = "extraConfig",
        skip_serializing_if = "Option::is_none"
    )]
    pub extra_config: Option<KubeletExtraConfig>,
}

/// Subset of KubeletConfiguration we surface in machine YAML.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct KubeletExtraConfig {
    /// Max pods per node (`maxPods`). Upstream kubelet default is 110 when omitted.
    #[serde(default, rename = "maxPods", skip_serializing_if = "Option::is_none")]
    pub max_pods: Option<u32>,
}

impl MachineKubelet {
    pub fn with_max_pods(max_pods: u32) -> Self {
        Self {
            extra_config: Some(KubeletExtraConfig {
                max_pods: Some(max_pods),
            }),
        }
    }
}

impl Machine {
    /// `machine.kubelet.extraConfig.maxPods` when set.
    pub fn max_pods(&self) -> Option<u32> {
        self.kubelet
            .as_ref()?
            .extra_config
            .as_ref()?
            .max_pods
    }
}

/// Fleet logs → Loki (or Alloy push API). Omit / empty `lokiUrl` disables the pusher.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Observability {
    /// Loki push URL, e.g. `http://10.1.1.10:3500/loki/api/v1/push`.
    #[serde(default, rename = "lokiUrl", skip_serializing_if = "Option::is_none")]
    pub loki_url: Option<String>,
    /// Optional `Authorization: Bearer` for the push endpoint.
    #[serde(default, rename = "lokiToken", skip_serializing_if = "Option::is_none")]
    pub loki_token: Option<String>,
    /// Prometheus Pushgateway base URL, e.g. `http://10.1.1.10:9091`.
    /// When omitted, derived from `lokiUrl` if that uses compose Alloy port 3500
    /// (`http://host:3500/…` → `http://host:9091`).
    #[serde(
        default,
        rename = "prometheusPushUrl",
        skip_serializing_if = "Option::is_none"
    )]
    pub prometheus_push_url: Option<String>,
    /// Extra stream labels (merged after `job` / `service` / `hostname` / `cluster`).
    #[serde(
        default,
        rename = "extraLabels",
        skip_serializing_if = "std::collections::BTreeMap::is_empty"
    )]
    pub extra_labels: std::collections::BTreeMap<String, String>,
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
    /// `auto` | `ascii` | `light` | `rounded` | `heavy` | `double` | `bordered`
    ///
    /// Default when omitted: `line` (continuous full-block frames).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<String>,
    /// Optional dashboard background in `#RRGGBB` form. Omit to preserve the
    /// terminal's default background.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    /// Force column count (skips size probe). Omit to auto-detect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cols: Option<u16>,
    /// Force row count (skips size probe). Omit to auto-detect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u16>,
    /// Force Unicode box-drawing. Omit to follow console detection; set
    /// `false` if Serial renders multi-byte border glyphs incorrectly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utf8: Option<bool>,
    /// Public web management URL shown on the serial console (e.g.
    /// `https://mgmt.example.com`). Also set via `MGMT_PUBLIC_URL`.
    #[serde(
        default,
        alias = "mgmtUrl",
        skip_serializing_if = "Option::is_none"
    )]
    pub mgmt_url: Option<String>,
}

impl Dashboard {
    pub const DEFAULT_THEME: &'static str = "catppuccin";
    /// Solid ASCII `=` line frames. Hyphen and ambiguous-width Unicode paint
    /// as `-  -  -` on Proxmox Serial.
    pub const DEFAULT_BORDER: &'static str = "line";
    /// Probe fallback only — never pinned unless YAML/env sets cols/rows.
    pub const DEFAULT_COLS: u16 = 80;
    pub const DEFAULT_ROWS: u16 = 24;

    /// Built-in console look — ASCII `=` line frames (Serial-safe).
    ///
    /// Size is left unset so the console probe can run — pinning geometry that
    /// does not match the pane blanks Proxmox Serial.
    pub fn builtin() -> Self {
        Self {
            theme: Some(Self::DEFAULT_THEME.into()),
            border: Some(Self::DEFAULT_BORDER.into()),
            background: None,
            cols: None,
            rows: None,
            // Borders are ASCII; leave UTF-8 unset so meters/labels can follow probe.
            utf8: None,
            mgmt_url: None,
        }
    }

    /// [`builtin`](Self::builtin) plus optional public management URL for the console.
    pub fn builtin_with_mgmt_url(url: Option<&str>) -> Self {
        let mut d = Self::builtin();
        if let Some(u) = url.map(str::trim).filter(|s| !s.is_empty()) {
            d.mgmt_url = Some(u.to_string());
        }
        d
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

/// Cluster networking (`cluster.network`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClusterNetwork {
    /// Pod CIDRs (IPv4 and optional IPv6), e.g. `10.244.0.0/16`, `2001:db8:10:0::/56`.
    #[serde(default, rename = "podSubnets", skip_serializing_if = "Vec::is_empty")]
    pub pod_subnets: Vec<String>,
    /// Service CIDRs (IPv4 and optional IPv6), e.g. `10.96.0.0/12`, `2001:db8:96:1::/112`.
    #[serde(default, rename = "serviceSubnets", skip_serializing_if = "Vec::is_empty")]
    pub service_subnets: Vec<String>,
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
    /// Pod/service subnet lists (preferred for new configs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<ClusterNetwork>,
    /// Legacy cluster-wide pod network CIDR (e.g. `10.244.0.0/16`).
    #[serde(default, rename = "podSubnet", skip_serializing_if = "Option::is_none")]
    pub pod_subnet: Option<String>,
    /// Legacy cluster service CIDR (default `10.96.0.0/12`).
    #[serde(default, rename = "serviceSubnet", skip_serializing_if = "Option::is_none")]
    pub service_subnet: Option<String>,
    /// Legacy IPv6 pod CIDR when [`NetworkMode::DualStack`].
    #[serde(default, rename = "podCidrIPv6", skip_serializing_if = "Option::is_none")]
    pub pod_cidr_ipv6: Option<String>,
    /// Legacy IPv6 service CIDR when dual-stack.
    #[serde(default, rename = "serviceCidrIPv6", skip_serializing_if = "Option::is_none")]
    pub service_cidr_ipv6: Option<String>,
    /// Node / cluster IP family mode. Default IPv4-only.
    #[serde(default, rename = "networkMode")]
    pub network_mode: NetworkMode,
    /// Optional IPv6 API VIP (HA dual-stack); also added to cert SANs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vip6: Option<String>,
    /// Kubernetes version tag for static-pod images (e.g. `v1.32.5`).
    #[serde(default, rename = "kubernetesVersion")]
    pub kubernetes_version: Option<String>,
    /// Pod CIDR for this node's bridge CNI (e.g. `10.244.0.0/24`).
    /// Unused when `cni: none` (cluster CNI DaemonSet owns networking).
    #[serde(default, rename = "podCidr", skip_serializing_if = "Option::is_none")]
    pub pod_cidr: Option<String>,
    /// Pod networking mode: `bridge` (built-in) or `none` (Flannel/Cilium/etc.).
    #[serde(default)]
    pub cni: CniMode,
    /// Extra apiserver (and etcd) certificate SANs — VIP, extra DNS names, CP IPs.
    #[serde(default, rename = "certSANs", skip_serializing_if = "Vec::is_empty")]
    pub cert_sans: Vec<String>,
}

impl Cluster {
    pub const DEFAULT_POD_SUBNET: &'static str = "10.244.0.0/16";
    pub const DEFAULT_SERVICE_SUBNET: &'static str = "10.96.0.0/12";
    pub const DEFAULT_POD_CIDR_IPV6: &'static str = "2001:db8:10:0::/56";
    pub const DEFAULT_SERVICE_CIDR_IPV6: &'static str = "2001:db8:96:1::/112";

    pub fn is_dual_stack(&self) -> bool {
        matches!(self.network_mode, NetworkMode::DualStack)
    }

    /// Resolved pod CIDR list (prefers `cluster.network.podSubnets`).
    pub fn effective_pod_subnets(&self) -> Vec<String> {
        if let Some(list) = self.network.as_ref().map(|n| {
            n.pod_subnets
                .iter()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        }) {
            if !list.is_empty() {
                return list;
            }
        }
        let mut out = vec![self
            .pod_subnet
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(Self::DEFAULT_POD_SUBNET)
            .to_string()];
        if self.is_dual_stack() {
            out.push(
                self.pod_cidr_ipv6
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(Self::DEFAULT_POD_CIDR_IPV6)
                    .to_string(),
            );
        }
        out
    }

    /// Resolved service CIDR list (prefers `cluster.network.serviceSubnets`).
    pub fn effective_service_subnets(&self) -> Vec<String> {
        if let Some(list) = self.network.as_ref().map(|n| {
            n.service_subnets
                .iter()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        }) {
            if !list.is_empty() {
                return list;
            }
        }
        let mut out = vec![self
            .service_subnet
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(Self::DEFAULT_SERVICE_SUBNET)
            .to_string()];
        if self.is_dual_stack() {
            out.push(
                self.service_cidr_ipv6
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(Self::DEFAULT_SERVICE_CIDR_IPV6)
                    .to_string(),
            );
        }
        out
    }

    /// First IPv4 CIDR from [`Self::effective_pod_subnets`].
    pub fn ipv4_pod_subnet(&self) -> String {
        self.effective_pod_subnets()
            .into_iter()
            .find(|s| looks_like_ipv4_cidr(s))
            .unwrap_or_else(|| Self::DEFAULT_POD_SUBNET.into())
    }

    /// First IPv4 CIDR from [`Self::effective_service_subnets`].
    pub fn ipv4_service_subnet(&self) -> String {
        self.effective_service_subnets()
            .into_iter()
            .find(|s| looks_like_ipv4_cidr(s))
            .unwrap_or_else(|| Self::DEFAULT_SERVICE_SUBNET.into())
    }

    /// `--service-cluster-ip-range` value for kube-apiserver / controller-manager.
    pub fn service_cluster_ip_range(&self) -> String {
        self.effective_service_subnets().join(",")
    }

    pub fn effective_pod_cidr_ipv6(&self) -> Option<String> {
        self.effective_pod_subnets()
            .into_iter()
            .find(|s| looks_like_ipv6_cidr(s))
            .or_else(|| {
                if !self.is_dual_stack() {
                    return None;
                }
                Some(
                    self.pod_cidr_ipv6
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .unwrap_or(Self::DEFAULT_POD_CIDR_IPV6)
                        .to_string(),
                )
            })
    }

    /// `--cluster-cidr` for kube-controller-manager (IPv4 or IPv4,IPv6).
    pub fn cluster_cidr(&self) -> String {
        self.effective_pod_subnets().join(",")
    }

    /// Build `cluster.network` from IPv4 (+ optional IPv6) CIDRs.
    pub fn network_from_cidrs(
        pod_v4: &str,
        service_v4: &str,
        dual_stack: bool,
        pod_v6: Option<&str>,
        service_v6: Option<&str>,
    ) -> ClusterNetwork {
        let mut pod_subnets = vec![pod_v4.trim().to_string()];
        let mut service_subnets = vec![service_v4.trim().to_string()];
        if dual_stack {
            pod_subnets.push(
                pod_v6
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(Self::DEFAULT_POD_CIDR_IPV6)
                    .to_string(),
            );
            service_subnets.push(
                service_v6
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(Self::DEFAULT_SERVICE_CIDR_IPV6)
                    .to_string(),
            );
        }
        ClusterNetwork {
            pod_subnets,
            service_subnets,
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

fn looks_like_ipv4_cidr(s: &str) -> bool {
    let ip = s.split('/').next().unwrap_or(s);
    ip.contains('.') && !ip.contains(':')
}

fn looks_like_ipv6_cidr(s: &str) -> bool {
    let ip = s.split('/').next().unwrap_or(s);
    ip.contains(':')
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
        let cfg: Self = serde_yaml::from_value(parse_yaml_value(yaml)?)?;
        if cfg.version != CONFIG_VERSION {
            return Err(ConfigError::UnsupportedVersion(cfg.version));
        }
        Ok(cfg)
    }

    /// Parse an apply payload, deep-merging onto the previous on-disk YAML when
    /// present. Partial updates (e.g. only `machine.dashboard`) keep
    /// `machine.type`, `network`, `cluster`, and other fields so kubelet is
    /// not left without a cluster block.
    pub fn from_yaml_merged(incoming: &str, previous_yaml: Option<&str>) -> Result<Self, ConfigError> {
        let patch = parse_yaml_value(incoming)?;
        let merged = match previous_yaml {
            Some(prev) => {
                let mut base = parse_yaml_value(prev)?;
                deep_merge_yaml(&mut base, patch);
                base
            }
            None => patch,
        };
        let cfg: Self = serde_yaml::from_value(merged)?;
        if cfg.version != CONFIG_VERSION {
            return Err(ConfigError::UnsupportedVersion(cfg.version));
        }
        Ok(cfg)
    }

    /// Parse a partial machine YAML (templates / Config-tab drafts).
    /// `machine.type` may be omitted — cluster apply sets it per node role.
    pub fn from_yaml_partial(yaml: &str) -> Result<Self, ConfigError> {
        const STUB: &str = "version: v1alpha1\nmachine:\n  type: worker\n";
        Self::from_yaml_merged(yaml, Some(STUB))
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
                kubelet: None,
                observability: None,
            },
            cluster: None,
        }
    }
}

/// Recursively merge `patch` into `base` (maps only). Sequences and scalars in
/// `patch` replace the corresponding `base` value.
fn deep_merge_yaml(base: &mut serde_yaml::Value, patch: serde_yaml::Value) {
    match (base, patch) {
        (serde_yaml::Value::Mapping(base_map), serde_yaml::Value::Mapping(patch_map)) => {
            for (k, v) in patch_map {
                match base_map.get_mut(&k) {
                    Some(existing) => deep_merge_yaml(existing, v),
                    None => {
                        base_map.insert(k, v);
                    }
                }
            }
        }
        (base_slot, patch_val) => {
            *base_slot = patch_val;
        }
    }
}

/// Parse YAML into a [`serde_yaml::Value`], tolerating duplicate mapping keys
/// (last wins). Editors often accidentally duplicate `dashboard.border`.
fn parse_yaml_value(yaml: &str) -> Result<serde_yaml::Value, ConfigError> {
    match serde_yaml::from_str::<serde_yaml::Value>(yaml) {
        Ok(v) => Ok(v),
        Err(e) => {
            let msg = e.to_string();
            if !msg.contains("duplicate entry") {
                return Err(ConfigError::Parse(e));
            }
            let cleaned = dedup_block_yaml_keys(yaml);
            serde_yaml::from_str(&cleaned).map_err(|e2| {
                ConfigError::Msg(format!(
                    "yaml parse failed after deduping duplicate keys ({msg}): {e2}"
                ))
            })
        }
    }
}

/// Block-style YAML: when serde_yaml reports a duplicate mapping key, drop the
/// **earlier** occurrence (keep the later value) and retry. Preserves lists and
/// unrelated structure — only the conflicting key line (+ its nested block) is
/// removed.
fn dedup_block_yaml_keys(input: &str) -> String {
    let mut current = input.to_string();
    for _ in 0..64 {
        match serde_yaml::from_str::<serde_yaml::Value>(&current) {
            Ok(_) => return current,
            Err(e) => {
                let msg = e.to_string();
                let Some(key) = dup_key_from_error(&msg) else {
                    return current;
                };
                let Some(line_no) = dup_line_from_error(&msg) else {
                    return current;
                };
                match remove_earlier_key_occurrence(&current, &key, line_no) {
                    Some(next) if next != current => current = next,
                    _ => return current,
                }
            }
        }
    }
    current
}

fn dup_key_from_error(msg: &str) -> Option<String> {
    // duplicate entry with key "border"
    let start = msg.find("key \"")?;
    let rest = &msg[start + 5..];
    let end = rest.find('"')?;
    let key = &rest[..end];
    (!key.is_empty()).then(|| key.to_string())
}

fn dup_line_from_error(msg: &str) -> Option<usize> {
    // at line 4 column 5
    let start = msg.find("at line ")?;
    let rest = &msg[start + 8..];
    let end = rest.find(" column")?;
    rest[..end].parse().ok()
}

fn line_indent(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ').count()
}

/// `hint_line` is 1-based (from serde_yaml). It may point at the first or
/// second duplicate; we collect every same-indent `key:` sibling and drop all
/// but the **last** (key line + nested block).
fn remove_earlier_key_occurrence(yaml: &str, key: &str, hint_line: usize) -> Option<String> {
    let lines: Vec<&str> = yaml.lines().collect();
    if lines.is_empty() {
        return None;
    }
    let needle = format!("{key}:");
    let is_key = |line: &str| {
        let t = line.trim_start();
        t == needle || t.starts_with(&format!("{needle} "))
    };

    let hint_idx = hint_line
        .saturating_sub(1)
        .min(lines.len().saturating_sub(1));

    // Prefer indent from a real key line near the hint.
    let indent = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| is_key(l))
        .map(|(i, l)| (i.abs_diff(hint_idx), line_indent(l)))
        .min_by_key(|(dist, _)| *dist)
        .map(|(_, ind)| ind)?;

    let indices: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(i, l)| line_indent(l) == indent && is_key(l) && same_mapping_parent(&lines, *i, hint_idx, indent))
        .map(|(i, _)| i)
        .collect();

    if indices.len() < 2 {
        return None;
    }

    // Drop every occurrence except the last.
    let keep = *indices.last()?;
    let mut drop_ranges: Vec<(usize, usize)> = Vec::new();
    for &start in &indices {
        if start == keep {
            continue;
        }
        let end = key_block_end(&lines, start, indent);
        drop_ranges.push((start, end));
    }
    drop_ranges.sort_by_key(|(s, _)| *s);

    let mut out: Vec<&str> = Vec::with_capacity(lines.len());
    let mut i = 0usize;
    let mut ri = 0usize;
    while i < lines.len() {
        if ri < drop_ranges.len() && i == drop_ranges[ri].0 {
            i = drop_ranges[ri].1;
            ri += 1;
            continue;
        }
        out.push(lines[i]);
        i += 1;
    }

    let mut s = out.join("\n");
    if yaml.ends_with('\n') {
        s.push('\n');
    }
    Some(s)
}

/// True when `a` and `b` share the same parent mapping (no intervening line
/// with indent < key_indent between them).
fn same_mapping_parent(lines: &[&str], a: usize, b: usize, key_indent: usize) -> bool {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    for line in &lines[lo..=hi] {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if line_indent(line) < key_indent {
            return false;
        }
    }
    true
}

fn key_block_end(lines: &[&str], start: usize, key_indent: usize) -> usize {
    let mut end = start + 1;
    while end < lines.len() {
        let line = lines[end];
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            let mut j = end + 1;
            while j < lines.len()
                && (lines[j].trim().is_empty() || lines[j].trim_start().starts_with('#'))
            {
                j += 1;
            }
            if j < lines.len() && line_indent(lines[j]) > key_indent {
                end = j;
                continue;
            }
            break;
        }
        if line_indent(line) > key_indent {
            end += 1;
            continue;
        }
        break;
    }
    end
}

/// Set `machine.type` on a YAML document (used when pushing one draft to mixed
/// control-plane / worker nodes).
pub fn set_machine_type_yaml(yaml: &str, machine_type: MachineType) -> Result<String, ConfigError> {
    let mut doc = parse_yaml_value(yaml)?;
    let ty = match machine_type {
        MachineType::Controlplane => "controlplane",
        MachineType::Worker => "worker",
    };
    let root = doc
        .as_mapping_mut()
        .ok_or_else(|| ConfigError::Msg("root must be a mapping".into()))?;
    if !root.contains_key(serde_yaml::Value::from("machine")) {
        root.insert(
            serde_yaml::Value::from("machine"),
            serde_yaml::Value::Mapping(serde_yaml::Mapping::new()),
        );
    }
    let machine = root
        .get_mut(serde_yaml::Value::from("machine"))
        .and_then(|m| m.as_mapping_mut())
        .ok_or_else(|| ConfigError::Msg("machine must be a mapping".into()))?;
    machine.insert(
        serde_yaml::Value::from("type"),
        serde_yaml::Value::from(ty),
    );
    Ok(serde_yaml::to_string(&doc)?)
}

/// Fill `machine.dashboard.mgmt_url` from the mgmt Public URL when the YAML
/// omits it or leaves it empty. An explicit URL in the document is kept.
pub fn ensure_dashboard_mgmt_url(yaml: &str, public_url: &str) -> Result<String, ConfigError> {
    let url = public_url.trim().trim_end_matches('/');
    if url.is_empty() {
        return Ok(yaml.to_string());
    }
    let mut doc = parse_yaml_value(yaml)?;
    let root = doc
        .as_mapping_mut()
        .ok_or_else(|| ConfigError::Msg("root must be a mapping".into()))?;
    if !root.contains_key(serde_yaml::Value::from("machine")) {
        root.insert(
            serde_yaml::Value::from("machine"),
            serde_yaml::Value::Mapping(serde_yaml::Mapping::new()),
        );
    }
    let machine = root
        .get_mut(serde_yaml::Value::from("machine"))
        .and_then(|m| m.as_mapping_mut())
        .ok_or_else(|| ConfigError::Msg("machine must be a mapping".into()))?;
    if !machine.contains_key(serde_yaml::Value::from("dashboard"))
        || machine
            .get(&serde_yaml::Value::from("dashboard"))
            .and_then(|v| v.as_mapping())
            .is_none()
    {
        machine.insert(
            serde_yaml::Value::from("dashboard"),
            serde_yaml::Value::Mapping(serde_yaml::Mapping::new()),
        );
    }
    let dashboard = machine
        .get_mut(serde_yaml::Value::from("dashboard"))
        .and_then(|m| m.as_mapping_mut())
        .ok_or_else(|| ConfigError::Msg("dashboard must be a mapping".into()))?;
    let existing = dashboard
        .get(&serde_yaml::Value::from("mgmt_url"))
        .or_else(|| dashboard.get(&serde_yaml::Value::from("mgmtUrl")))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if existing.is_some() {
        return Ok(yaml.to_string());
    }
    dashboard.insert(
        serde_yaml::Value::from("mgmt_url"),
        serde_yaml::Value::from(url),
    );
    Ok(serde_yaml::to_string(&doc)?)
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
    fn parses_cluster_network_subnets() {
        let yaml = r#"
version: v1alpha1
machine:
  type: controlplane
cluster:
  endpoint: https://10.1.1.210:6443
  networkMode: dual-stack
  network:
    podSubnets:
      - 10.10.0.0/16
      - 2001:db8:10:0::/56
    serviceSubnets:
      - 10.96.0.0/12
      - 2001:db8:96:1::/112
"#;
        let cfg = MachineConfig::from_yaml(yaml).unwrap();
        let cluster = cfg.cluster.unwrap();
        assert_eq!(
            cluster.effective_pod_subnets(),
            vec!["10.10.0.0/16".to_string(), "2001:db8:10:0::/56".to_string()]
        );
        assert_eq!(
            cluster.effective_service_subnets(),
            vec!["10.96.0.0/12".to_string(), "2001:db8:96:1::/112".to_string()]
        );
        assert_eq!(cluster.ipv4_pod_subnet(), "10.10.0.0/16");
        assert_eq!(cluster.ipv4_service_subnet(), "10.96.0.0/12");
        assert_eq!(
            cluster.cluster_cidr(),
            "10.10.0.0/16,2001:db8:10:0::/56"
        );
        assert_eq!(
            cluster.service_cluster_ip_range(),
            "10.96.0.0/12,2001:db8:96:1::/112"
        );
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
    mgmt_url: https://mgmt.example.com
"#;
        let cfg = MachineConfig::from_yaml(yaml).unwrap();
        let dash = cfg.machine.dashboard.unwrap();
        assert_eq!(dash.theme.as_deref(), Some("nord"));
        assert_eq!(dash.border.as_deref(), Some("light"));
        assert_eq!(dash.cols, Some(140));
        assert_eq!(dash.rows, Some(40));
        assert_eq!(dash.utf8, None);
        assert_eq!(
            dash.mgmt_url.as_deref(),
            Some("https://mgmt.example.com")
        );
    }

    #[test]
    fn parses_kubelet_max_pods() {
        let yaml = r#"
version: v1alpha1
machine:
  type: worker
  kubelet:
    extraConfig:
      maxPods: 250
"#;
        let cfg = MachineConfig::from_yaml(yaml).unwrap();
        assert_eq!(cfg.machine.max_pods(), Some(250));
        let out = serde_yaml::to_string(&cfg).unwrap();
        assert!(out.contains("maxPods: 250"));
    }

    #[test]
    fn parses_observability_loki() {
        let yaml = r#"
version: v1alpha1
machine:
  type: worker
  observability:
    lokiUrl: http://10.1.1.10:3500/loki/api/v1/push
    lokiToken: s3cret
    extraLabels:
      env: lab
"#;
        let cfg = MachineConfig::from_yaml(yaml).unwrap();
        let obs = cfg.machine.observability.as_ref().unwrap();
        assert_eq!(
            obs.loki_url.as_deref(),
            Some("http://10.1.1.10:3500/loki/api/v1/push")
        );
        assert_eq!(obs.loki_token.as_deref(), Some("s3cret"));
        assert_eq!(obs.extra_labels.get("env").map(String::as_str), Some("lab"));
        let out = serde_yaml::to_string(&cfg).unwrap();
        assert!(out.contains("lokiUrl:"));
    }

    #[test]
    fn parses_observability_prometheus_push() {
        let yaml = r#"
version: v1alpha1
machine:
  type: worker
  observability:
    lokiUrl: http://10.1.1.10:3500/loki/api/v1/push
    prometheusPushUrl: http://10.1.1.10:9091
"#;
        let cfg = MachineConfig::from_yaml(yaml).unwrap();
        let obs = cfg.machine.observability.as_ref().unwrap();
        assert_eq!(
            obs.prometheus_push_url.as_deref(),
            Some("http://10.1.1.10:9091")
        );
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
        assert_eq!(dash.border.as_deref(), Some("line"));
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

    #[test]
    fn from_yaml_partial_allows_omitted_type() {
        let yaml = r#"
version: v1alpha1
machine:
  dashboard:
    theme: catppuccin
    border: bordered
    mgmt_url: https://mgmt.example.com
  observability:
    lokiUrl: https://loki.example.com/loki/api/v1/push
"#;
        let cfg = MachineConfig::from_yaml_partial(yaml).unwrap();
        assert_eq!(cfg.machine.machine_type, MachineType::Worker);
        let dash = cfg.machine.dashboard.unwrap();
        assert_eq!(
            dash.mgmt_url.as_deref(),
            Some("https://mgmt.example.com")
        );
        assert_eq!(
            cfg.machine
                .observability
                .as_ref()
                .and_then(|o| o.loki_url.as_deref()),
            Some("https://loki.example.com/loki/api/v1/push")
        );
    }

    #[test]
    fn from_yaml_merged_adds_mgmt_url() {
        let previous = r#"
version: v1alpha1
machine:
  type: controlplane
  network:
    hostname: cp-1
  dashboard:
    theme: catppuccin
    border: ascii
cluster:
  endpoint: https://10.1.1.1:6443
"#;
        let patch = r#"
version: v1alpha1
machine:
  dashboard:
    mgmt_url: https://mgmt.example.com
"#;
        let cfg = MachineConfig::from_yaml_merged(patch, Some(previous)).unwrap();
        assert_eq!(cfg.machine.machine_type, MachineType::Controlplane);
        let dash = cfg.machine.dashboard.unwrap();
        assert_eq!(dash.theme.as_deref(), Some("catppuccin"));
        assert_eq!(
            dash.mgmt_url.as_deref(),
            Some("https://mgmt.example.com")
        );
    }

    #[test]
    fn from_yaml_merged_preserves_cluster_and_type() {
        let previous = r#"
version: v1alpha1
machine:
  type: worker
  network:
    hostname: wk-1
    interfaces:
      - interface: eth0
        dhcp: true
cluster:
  endpoint: https://10.1.1.210:6443
  token: abc.def
  cni: none
"#;
        let patch = r#"
version: v1alpha1
machine:
  dashboard:
    theme: catppuccin
    border: bordered
"#;
        let cfg = MachineConfig::from_yaml_merged(patch, Some(previous)).unwrap();
        assert_eq!(cfg.machine.machine_type, MachineType::Worker);
        assert_eq!(
            cfg.machine.network.hostname.as_deref(),
            Some("wk-1")
        );
        assert_eq!(
            cfg.cluster.as_ref().map(|c| c.endpoint.as_str()),
            Some("https://10.1.1.210:6443")
        );
        let dash = cfg.machine.dashboard.unwrap();
        assert_eq!(dash.theme.as_deref(), Some("catppuccin"));
        assert_eq!(dash.border.as_deref(), Some("bordered"));
    }

    #[test]
    fn ensure_dashboard_mgmt_url_fills_dashboard_only_yaml() {
        let yaml = r#"version: v1alpha1
machine:
  dashboard:
    theme: catppuccin
    border: bordered
"#;
        let out = ensure_dashboard_mgmt_url(yaml, "http://10.1.1.15:8080").unwrap();
        assert!(
            out.contains("mgmt_url: http://10.1.1.15:8080"),
            "{out}"
        );
        assert!(out.contains("theme: catppuccin"));
    }

    #[test]
    fn ensure_dashboard_mgmt_url_fills_when_missing() {
        let yaml = r#"
version: v1alpha1
machine:
  type: worker
  dashboard:
    theme: catppuccin
    border: bordered
"#;
        let out = ensure_dashboard_mgmt_url(yaml, "https://ptkos.example:8080/").unwrap();
        let cfg = MachineConfig::from_yaml(&out).unwrap();
        let dash = cfg.machine.dashboard.unwrap();
        assert_eq!(dash.theme.as_deref(), Some("catppuccin"));
        assert_eq!(
            dash.mgmt_url.as_deref(),
            Some("https://ptkos.example:8080")
        );
    }

    #[test]
    fn ensure_dashboard_mgmt_url_keeps_explicit() {
        let yaml = r#"
version: v1alpha1
machine:
  type: worker
  dashboard:
    mgmt_url: https://keep.example
"#;
        let out =
            ensure_dashboard_mgmt_url(yaml, "https://settings.example").unwrap();
        let cfg = MachineConfig::from_yaml(&out).unwrap();
        assert_eq!(
            cfg.machine.dashboard.unwrap().mgmt_url.as_deref(),
            Some("https://keep.example")
        );
    }

    #[test]
    fn set_machine_type_yaml_overrides() {
        let yaml = r#"
version: v1alpha1
machine:
  type: controlplane
  dashboard:
    theme: nord
"#;
        let out = set_machine_type_yaml(yaml, MachineType::Worker).unwrap();
        let cfg = MachineConfig::from_yaml(&out).unwrap();
        assert_eq!(cfg.machine.machine_type, MachineType::Worker);
        assert_eq!(
            cfg.machine.dashboard.unwrap().theme.as_deref(),
            Some("nord")
        );
    }

    #[test]
    fn set_machine_type_dedups_duplicate_dashboard_border() {
        let yaml = r#"version: v1alpha1
machine:
  dashboard:
    border: bordered
    theme: catppuccin
    border: bordered
    mgmt_url: https://mgmt.example.com
"#;
        let out = set_machine_type_yaml(yaml, MachineType::Controlplane).unwrap();
        let cfg = MachineConfig::from_yaml(&out).unwrap();
        assert_eq!(cfg.machine.machine_type, MachineType::Controlplane);
        let dash = cfg.machine.dashboard.unwrap();
        assert_eq!(dash.border.as_deref(), Some("bordered"));
        assert_eq!(dash.theme.as_deref(), Some("catppuccin"));
        assert_eq!(
            dash.mgmt_url.as_deref(),
            Some("https://mgmt.example.com")
        );
        // Round-trip must not reintroduce duplicates.
        assert_eq!(out.matches("border:").count(), 1);
    }

    #[test]
    fn from_yaml_preserves_gen_style_lists_after_dedup_path() {
        let yaml = r#"
version: v1alpha1
machine:
  type: worker
  network:
    hostname: wk-1
    interfaces:
    - interface: eth0
      dhcp: true
      addresses: []
      gateway: null
    nameservers:
    - 1.1.1.1
  dashboard:
    theme: catppuccin
    border: bordered
"#;
        let cfg = MachineConfig::from_yaml(yaml).unwrap();
        assert_eq!(cfg.machine.network.interfaces.len(), 1);
        assert_eq!(cfg.machine.network.interfaces[0].interface, "eth0");
        assert_eq!(cfg.machine.network.nameservers, vec!["1.1.1.1"]);
    }
}
