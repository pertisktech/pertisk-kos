//! Optional cluster add-ons installed through the management UI (NFS, cert-manager, ingress).

use std::collections::BTreeSet;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use rand::rngs::OsRng;
use rand::RngCore;

use crate::crypto;
use crate::db;
use crate::error::{ApiResult, AppError};
use crate::k8s::{
    helm_output, kubectl_apply_url, kubectl_apply_yaml, kubectl_json, kubectl_json_optional,
    kubectl_ok, resolve_ready_kubeconfig,
};
use crate::state::AppState;

const NFS_MODULES_YAML: &str =
    include_str!("../../../examples/addons/nfs/pertisk-nfs-modules-ds.yaml");
const NFS_PROVISIONER_YAML: &str =
    include_str!("../../../examples/addons/nfs/nfs-subdir-external-provisioner.yaml");

pub const CERT_MANAGER_VERSION: &str = "v1.21.1";
pub const CERT_MANAGER_MANIFEST_URL: &str =
    "https://github.com/cert-manager/cert-manager/releases/download/v1.21.1/cert-manager.yaml";
/// Avoid kubelet :10250 when the webhook runs on the host network (Cilium / no kube-proxy).
const WEBHOOK_HOST_PORT: u16 = 10260;

const NFS_ID: &str = "nfs";
const CERT_MANAGER_ID: &str = "cert-manager";
const CILIUM_LB_ID: &str = "cilium-lb";
const CILIUM_LB_POOL: &str = "default-pool";
const CILIUM_LB_L2: &str = "default-l2-announcement-policy";
const INGRESS_ID: &str = "ingress";
const INGRESS_HELM_REPO: &str = "https://chart.tools.pertisk.com";
const INGRESS_HELM_CHART: &str = "pertisk-ingress";
const INGRESS_RELEASE: &str = "pertisk-ingress";
const INGRESS_NAMESPACE: &str = "pertisk-proxy";
const INGRESS_DEPLOY: &str = "pertisk-proxy-ingress";
const INGRESS_ADMIN: &str = "pertisk-proxy-ingress-admin";
const INGRESS_PULL_SECRET: &str = "pertisk-ingress-harbor";
pub const INGRESS_IMAGE_REGISTRY: &str = "harbor.tools.pertisk.com";
pub const INGRESS_IMAGE_REPO: &str = "pertisk-proxy/ingress";
pub const INGRESS_IMAGE_TAG: &str = "v0.1.83";
const KOS_SCALER_ID: &str = "kos-scaler";
const KOS_SCALER_HELM_CHART: &str = "kos-scaler";
const KOS_SCALER_RELEASE: &str = "kos-scaler";
const KOS_SCALER_NAMESPACE: &str = "kos-scaler";
const KOS_SCALER_DEPLOY: &str = "kos-scaler";
pub const KOS_SCALER_IMAGE_TAG: &str = "0.1.0";
const KUBERNETES_DASHBOARD_ID: &str = "kubernetes-dashboard";
const KUBERNETES_DASHBOARD_HELM_CHART: &str = "pertisk-kube";
const KUBERNETES_DASHBOARD_RELEASE: &str = "pertisk-kube";
const KUBERNETES_DASHBOARD_NAMESPACE: &str = "pertisk-dashboard";
const KUBERNETES_DASHBOARD_DEPLOY: &str = "pertisk-kube";
const KUBERNETES_DASHBOARD_IMAGE_REGISTRY: &str = "harbor.tools.pertisk.com";
const KUBERNETES_DASHBOARD_IMAGE_REPO: &str = "pertisksoft/pertisk-kube/web";
const KUBERNETES_DASHBOARD_IMAGE_TAG: &str = "v0.2.6";
const CERT_NS: &str = "cert-manager";
const REFLECTOR_MANIFEST_URL: &str =
    "https://github.com/emberstack/kubernetes-reflector/releases/latest/download/reflector.yaml";

const ACME_PROD: &str = "https://acme-v02.api.letsencrypt.org/directory";
const ACME_STAGING: &str = "https://acme-staging-v02.api.letsencrypt.org/directory";

#[derive(Debug, Clone, Serialize)]
pub struct AddonField {
    pub name: &'static str,
    pub label: &'static str,
    /// text | password | select
    pub kind: &'static str,
    pub required: bool,
    pub placeholder: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<&'static [&'static str]>,
    pub help: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct AddonCatalogEntry {
    pub id: &'static str,
    pub name: &'static str,
    pub summary: &'static str,
    pub section: &'static str,
    pub fields: &'static [AddonField],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_cni: Option<&'static str>,
}

const NFS_FIELDS: &[AddonField] = &[
    AddonField {
        name: "server",
        label: "NFS server",
        kind: "text",
        required: true,
        placeholder: "10.1.1.150",
        options: None,
        help: "IP or hostname of the NFS export, reachable from every node.",
    },
    AddonField {
        name: "path",
        label: "Export path",
        kind: "text",
        required: true,
        placeholder: "/mnt/nfs_share",
        options: None,
        help: "Exported directory on the NFS server (StorageClass nfs-client).",
    },
];

const CERT_MANAGER_FIELDS: &[AddonField] = &[
    AddonField {
        name: "provider",
        label: "DNS provider",
        kind: "select",
        required: true,
        placeholder: "cloudflare",
        options: Some(&["cloudflare"]),
        help: "ACME DNS-01 solver used to issue certificates.",
    },
    AddonField {
        name: "email",
        label: "ACME email",
        kind: "text",
        required: true,
        placeholder: "ops@example.com",
        options: None,
        help: "Let’s Encrypt account email.",
    },
    AddonField {
        name: "api_token",
        label: "API token",
        kind: "password",
        required: true,
        placeholder: "Cloudflare API token",
        options: None,
        help: "Cloudflare API token with Zone:DNS:Edit on the zones you issue for.",
    },
    AddonField {
        name: "acme",
        label: "ACME environment",
        kind: "select",
        required: false,
        placeholder: "production",
        options: Some(&["production", "staging"]),
        help: "Use staging to test issuance without Let’s Encrypt rate limits.",
    },
    AddonField {
        name: "domain",
        label: "Wildcard domain",
        kind: "text",
        required: false,
        placeholder: "*.vsphere.pertisk.com",
        options: None,
        help: "Optional. Issues a Certificate for the apex and *.domain, then reflects the TLS Secret into every namespace.",
    },
];

const CILIUM_LB_FIELDS: &[AddonField] = &[
    AddonField {
        name: "ipv4",
        label: "ELB IPv4",
        kind: "text",
        required: true,
        placeholder: "10.1.1.50",
        options: None,
        help: "IPv4 address or CIDR announced for Service type LoadBalancer (e.g. 10.1.1.50 or 10.1.1.50/32).",
    },
    AddonField {
        name: "ipv6",
        label: "ELB IPv6",
        kind: "text",
        required: false,
        placeholder: "2001:db8::1",
        options: None,
        help: "IPv6 address or CIDR. Required on dual-stack clusters.",
    },
];

const INGRESS_FIELDS: &[AddonField] = &[
    AddonField {
        name: "image_tag",
        label: "Image tag",
        kind: "text",
        required: true,
        placeholder: INGRESS_IMAGE_TAG,
        options: None,
        help: "Multi-arch Harbor tag (e.g. v0.1.83). Install pins the cluster arch (linux/arm64 or linux/amd64) so ARM nodes do not pull amd64.",
    },
    AddonField {
        name: "admin_host",
        label: "Admin host",
        kind: "text",
        required: false,
        placeholder: "admin.ingress.example.com",
        options: None,
        help: "Hostname for the viewer admin Ingress. Example: admin.vsphere.pertisk.com. Leave empty to skip.",
    },
    AddonField {
        name: "tls_secret",
        label: "TLS secret",
        kind: "select",
        required: false,
        placeholder: "none",
        options: Some(&["none"]),
        help: "TLS Secret for the admin Ingress (from cert-manager). Choose none for HTTP only.",
    },
    AddonField {
        name: "admin_password",
        label: "Admin password",
        kind: "password",
        required: false,
        placeholder: "optional",
        options: None,
        help: "Management UI password (stored encrypted). Leave blank to keep the current value or chart default.",
    },
    AddonField {
        name: "registry_user",
        label: "Harbor user",
        kind: "text",
        required: false,
        placeholder: "optional",
        options: None,
        help: "Optional. Leave empty — harbor.tools.pertisk.com/pertisk-proxy is public. Set only if you use a private project.",
    },
    AddonField {
        name: "registry_password",
        label: "Harbor password",
        kind: "password",
        required: false,
        placeholder: "optional",
        options: None,
        help: "Optional registry credential (stored encrypted). Not needed for the public Harbor project.",
    },
];

const KUBERNETES_DASHBOARD_FIELDS: &[AddonField] = &[
    AddonField {
        name: "namespace",
        label: "Namespace",
        kind: "text",
        required: true,
        placeholder: KUBERNETES_DASHBOARD_NAMESPACE,
        options: None,
        help: "Namespace for the Dashboard Helm release. It is created automatically.",
    },
    AddonField {
        name: "image_tag",
        label: "Image tag",
        kind: "text",
        required: true,
        placeholder: KUBERNETES_DASHBOARD_IMAGE_TAG,
        options: None,
        help: "Dashboard image tag from harbor.tools.pertisk.com/pertisksoft/pertisk-kube/web.",
    },
    AddonField {
        name: "username",
        label: "Dashboard user",
        kind: "text",
        required: true,
        placeholder: "admin",
        options: None,
        help: "Username used to sign in to the Dashboard.",
    },
    AddonField {
        name: "password",
        label: "Dashboard password",
        kind: "password",
        required: true,
        placeholder: "required",
        options: None,
        help: "Stored encrypted. Leave blank on update to keep the current password.",
    },
    AddonField {
        name: "host",
        label: "Dashboard host",
        kind: "text",
        required: false,
        placeholder: "dashboard.example.com",
        options: None,
        help: "Hostname for the Dashboard Ingress. Leave empty to disable Ingress.",
    },
    AddonField {
        name: "tls_secret",
        label: "TLS secret",
        kind: "select",
        required: false,
        placeholder: "none",
        options: Some(&["none"]),
        help:
            "TLS Secret for the Dashboard Ingress (from cert-manager). Choose none for HTTP only.",
    },
];

const KOS_SCALER_FIELDS: &[AddonField] = &[
    AddonField {
        name: "username",
        label: "Mgmt username",
        kind: "text",
        required: true,
        placeholder: "admin",
        options: None,
        help: "pertisk-mgmt operator or admin. kos-scaler refreshes JWTs from this account.",
    },
    AddonField {
        name: "password",
        label: "Mgmt password",
        kind: "password",
        required: true,
        placeholder: "required",
        options: None,
        help: "Stored encrypted. Leave blank on update to keep the current password.",
    },
    AddonField {
        name: "min_size",
        label: "Worker min",
        kind: "text",
        required: true,
        placeholder: "2",
        options: None,
        help: "Minimum worker count kos-scaler will enforce.",
    },
    AddonField {
        name: "max_size",
        label: "Worker max",
        kind: "text",
        required: true,
        placeholder: "10",
        options: None,
        help: "Maximum workers kos-scaler may add via the management API.",
    },
    AddonField {
        name: "image_tag",
        label: "Image tag",
        kind: "text",
        required: false,
        placeholder: KOS_SCALER_IMAGE_TAG,
        options: None,
        help: "Harbor tag for harbor.tools.pertisk.com/pertisksoft/kos-scaler.",
    },
    AddonField {
        name: "storage_class",
        label: "State StorageClass",
        kind: "text",
        required: false,
        placeholder: "nfs-client",
        options: None,
        help: "PVC for scaler event history. Use none to skip persistence (emptyDir).",
    },
    AddonField {
        name: "mgmt_url",
        label: "Mgmt URL override",
        kind: "text",
        required: false,
        placeholder: "https://ptkos.example",
        options: None,
        help: "Must be reachable from cluster nodes. Leave empty to use this server’s public URL.",
    },
];

pub fn catalog() -> &'static [AddonCatalogEntry] {
    &[
        AddonCatalogEntry {
            id: NFS_ID,
            name: "NFS storage",
            summary: "Dynamic ReadWriteMany volumes via an external NFS export and nfs-subdir-external-provisioner.",
            section: "cluster",
            fields: NFS_FIELDS,
            requires_cni: None,
        },
        AddonCatalogEntry {
            id: CERT_MANAGER_ID,
            name: "cert-manager",
            summary: "Let’s Encrypt ClusterIssuer (Cloudflare DNS-01) and optional wildcard Certificate reflected to all namespaces.",
            section: "certificates",
            fields: CERT_MANAGER_FIELDS,
            requires_cni: None,
        },
        AddonCatalogEntry {
            id: CILIUM_LB_ID,
            name: "Cilium LoadBalancer",
            summary: "ELB IPs via CiliumLoadBalancerIPPool and L2 announcements (shown when CNI is Cilium).",
            section: "cluster",
            fields: CILIUM_LB_FIELDS,
            requires_cni: Some("cilium"),
        },
        AddonCatalogEntry {
            id: INGRESS_ID,
            name: "Pertisk Ingress",
            summary: "pertisk-proxy Ingress controller (Helm chart + Harbor image) with a LoadBalancer Service.",
            section: "ingress",
            fields: INGRESS_FIELDS,
            requires_cni: None,
        },
        AddonCatalogEntry {
            id: KOS_SCALER_ID,
            name: "KOS scaler",
            summary: "Worker-node autoscaler (Helm kos-scaler). Adds and removes Pertisk workers from pending pods and CPU/memory pressure.",
            section: "autoscaling",
            fields: KOS_SCALER_FIELDS,
            requires_cni: None,
        },
        AddonCatalogEntry {
            id: KUBERNETES_DASHBOARD_ID,
            name: "Kubernetes Dashboard",
            summary: "Pertisk Kubernetes web dashboard (Helm pertisk-kube) for monitoring and managing cluster resources.",
            section: "dashboard",
            fields: KUBERNETES_DASHBOARD_FIELDS,
            requires_cni: None,
        },
    ]
}

pub fn catalog_entry(id: &str) -> ApiResult<&'static AddonCatalogEntry> {
    catalog()
        .iter()
        .find(|e| e.id == id)
        .ok_or_else(|| AppError::bad(format!("unknown addon {id}")))
}

fn catalog_fields_json(entry: &AddonCatalogEntry, live: &Value, config: &Value) -> Value {
    let mut fields = serde_json::to_value(entry.fields).unwrap_or(json!([]));
    if entry.id != INGRESS_ID && entry.id != KUBERNETES_DASHBOARD_ID {
        return fields;
    }
    let mut opts: Vec<String> = vec!["none".into()];
    if let Some(arr) = live.get("tls_secrets").and_then(|v| v.as_array()) {
        for s in arr {
            if let Some(n) = s.as_str() {
                if n != "none" && !opts.iter().any(|o| o == n) {
                    opts.push(n.to_string());
                }
            }
        }
    }
    if let Some(cur) = config.get("tls_secret").and_then(|v| v.as_str()) {
        let t = cur.trim();
        if !t.is_empty() && t != "none" && !opts.iter().any(|o| o == t) {
            opts.push(t.to_string());
        }
    }
    if let Some(arr) = fields.as_array_mut() {
        for f in arr {
            if f.get("name").and_then(|n| n.as_str()) == Some("tls_secret") {
                f["options"] = json!(opts);
            }
        }
    }
    fields
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct NfsConfig {
    pub server: String,
    pub path: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CertManagerConfig {
    #[serde(default = "default_provider")]
    pub provider: String,
    pub email: String,
    #[serde(default)]
    pub api_token: String,
    #[serde(default = "default_acme")]
    pub acme: String,
    #[serde(default = "default_issuer")]
    pub issuer: String,
    /// Apex or wildcard DNS name, e.g. `vsphere.pertisk.com` or `*.vsphere.pertisk.com`.
    #[serde(default)]
    pub domain: String,
}

fn default_provider() -> String {
    "cloudflare".into()
}
fn default_acme() -> String {
    "production".into()
}
fn default_issuer() -> String {
    "letsencrypt-cloudflare".into()
}

fn ingress_tls_secret(cfg: &IngressConfig) -> String {
    let t = cfg.tls_secret.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("none") {
        String::new()
    } else {
        t.to_string()
    }
}

fn k8s_name_ok(name: &str) -> bool {
    let t = name.trim();
    (1..=253).contains(&t.len())
        && t.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '.'))
        && t.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && t.chars().last().is_some_and(|c| c.is_ascii_alphanumeric())
}

/// Apex + wildcard SAN list. `vsphere.example.com` and `*.vsphere.example.com` both work.
pub fn wildcard_dns_names(raw: &str) -> Vec<String> {
    let t = raw.trim().trim_end_matches('.');
    if t.is_empty() {
        return Vec::new();
    }
    let apex = t.strip_prefix("*.").unwrap_or(t).trim_start_matches('.');
    if apex.is_empty() || !apex.contains('.') {
        return Vec::new();
    }
    vec![format!("*.{apex}"), apex.to_string()]
}

pub fn cert_secret_name(raw: &str) -> String {
    let t = raw
        .trim()
        .trim_end_matches('.')
        .trim_start_matches("*.")
        .to_ascii_lowercase();
    let mut slug = String::new();
    for c in t.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if matches!(c, '.' | '-') && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-');
    let mut name = if slug.is_empty() {
        "wildcard-tls".into()
    } else {
        format!("{slug}-tls")
    };
    if name.len() > 63 {
        name.truncate(63);
        name = name.trim_end_matches('-').to_string();
    }
    name
}

fn domain_ok(raw: &str) -> bool {
    !wildcard_dns_names(raw).is_empty()
        && raw
            .trim()
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '*'))
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CiliumLbConfig {
    pub ipv4: String,
    #[serde(default)]
    pub ipv6: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct IngressConfig {
    #[serde(default)]
    pub image_tag: String,
    #[serde(default)]
    pub admin_host: String,
    /// TLS Secret name in `pertisk-proxy`, or `none` / empty for HTTP-only.
    #[serde(default)]
    pub tls_secret: String,
    #[serde(default)]
    pub admin_password: String,
    #[serde(default)]
    pub registry_user: String,
    #[serde(default)]
    pub registry_password: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct KubernetesDashboardConfig {
    #[serde(default)]
    pub namespace: String,
    #[serde(default)]
    pub image_tag: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub tls_secret: String,
}

#[derive(Debug, Clone, Default)]
struct IngressSecrets {
    admin_password: String,
    registry_password: String,
}

#[derive(Debug, Clone, Default)]
struct KubernetesDashboardSecrets {
    password: String,
    jwt_secret: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct KosScalerConfig {
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub min_size: i64,
    #[serde(default)]
    pub max_size: i64,
    #[serde(default)]
    pub image_tag: String,
    #[serde(default)]
    pub storage_class: String,
    #[serde(default)]
    pub mgmt_url: String,
}

pub fn parse_addon_id(raw: &str) -> ApiResult<String> {
    match raw.trim() {
        NFS_ID => Ok(NFS_ID.into()),
        CERT_MANAGER_ID => Ok(CERT_MANAGER_ID.into()),
        CILIUM_LB_ID => Ok(CILIUM_LB_ID.into()),
        INGRESS_ID => Ok(INGRESS_ID.into()),
        KOS_SCALER_ID => Ok(KOS_SCALER_ID.into()),
        KUBERNETES_DASHBOARD_ID => Ok(KUBERNETES_DASHBOARD_ID.into()),
        other => Err(AppError::bad(format!("unknown addon {other}"))),
    }
}

async fn cluster_net(state: &AppState, cluster_id: &str) -> ApiResult<(String, String, String)> {
    let row: Option<(String, String, String)> = sqlx::query_as(
        "SELECT LOWER(cni), LOWER(COALESCE(network_mode, 'ipv4')), LOWER(COALESCE(arch, 'amd64')) \
         FROM clusters WHERE id = ?",
    )
    .bind(cluster_id)
    .fetch_optional(state.pool())
    .await?;
    row.ok_or(AppError::NotFound)
}

fn require_cilium_cni(cni: &str) -> ApiResult<()> {
    if cni == "cilium" {
        Ok(())
    } else {
        Err(AppError::bad(
            "Cilium LoadBalancer add-on requires cluster CNI cilium",
        ))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum IpFamily {
    V4,
    V6,
}

/// Accept a bare IP or CIDR and normalize to `addr/prefix`.
fn normalize_lb_cidr(raw: &str, family: IpFamily) -> Result<String, String> {
    let t = raw.trim();
    if t.is_empty() {
        return Err("empty".into());
    }
    if t.len() > 80
        || t.chars()
            .any(|c| !(c.is_ascii_hexdigit() || matches!(c, '.' | ':' | '/')))
    {
        return Err("invalid characters".into());
    }
    let (addr_s, prefix) = match t.split_once('/') {
        Some((a, p)) => {
            let n: u32 = p.parse().map_err(|_| "invalid prefix length".to_string())?;
            (a, Some(n))
        }
        None => (t, None),
    };
    let ip: IpAddr = addr_s
        .parse()
        .map_err(|_| "invalid IP address".to_string())?;
    match (ip, family) {
        (IpAddr::V4(v), IpFamily::V4) => {
            let p = prefix.unwrap_or(32);
            if p > 32 {
                return Err("IPv4 prefix must be 0–32".into());
            }
            Ok(format!("{v}/{p}"))
        }
        (IpAddr::V6(v), IpFamily::V6) => {
            let p = prefix.unwrap_or(128);
            if p > 128 {
                return Err("IPv6 prefix must be 0–128".into());
            }
            Ok(format!("{v}/{p}"))
        }
        (IpAddr::V4(_), IpFamily::V6) => Err("expected IPv6".into()),
        (IpAddr::V6(_), IpFamily::V4) => Err("expected IPv4".into()),
    }
}

pub fn validate_cilium_lb(cfg: &CiliumLbConfig, network_mode: &str) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    let mode = network_mode.trim().to_ascii_lowercase();
    let ipv4 = cfg.ipv4.trim();
    let ipv6 = cfg.ipv6.trim();
    let need_v4 = mode != "ipv6";
    let need_v6 = mode == "dual-stack" || mode == "ipv6";

    if need_v4 {
        if ipv4.is_empty() {
            errors.push("ELB IPv4 is required".into());
        } else if let Err(e) = normalize_lb_cidr(ipv4, IpFamily::V4) {
            errors.push(format!("ELB IPv4: {e}"));
        }
    } else if !ipv4.is_empty() {
        if let Err(e) = normalize_lb_cidr(ipv4, IpFamily::V4) {
            errors.push(format!("ELB IPv4: {e}"));
        }
    }

    if need_v6 {
        if ipv6.is_empty() {
            errors.push("ELB IPv6 is required on dual-stack / IPv6 clusters".into());
        } else if let Err(e) = normalize_lb_cidr(ipv6, IpFamily::V6) {
            errors.push(format!("ELB IPv6: {e}"));
        }
    } else if !ipv6.is_empty() {
        if let Err(e) = normalize_lb_cidr(ipv6, IpFamily::V6) {
            errors.push(format!("ELB IPv6: {e}"));
        }
    }

    if ipv4.is_empty() && ipv6.is_empty() && errors.is_empty() {
        errors.push("at least one ELB IP is required".into());
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

pub fn public_cilium_lb_config(cfg: &CiliumLbConfig) -> Value {
    let ipv4 = cfg.ipv4.trim();
    let ipv6 = cfg.ipv6.trim();
    json!({
        "ipv4": if ipv4.is_empty() {
            String::new()
        } else {
            normalize_lb_cidr(ipv4, IpFamily::V4).unwrap_or_else(|_| ipv4.to_string())
        },
        "ipv6": if ipv6.is_empty() {
            String::new()
        } else {
            normalize_lb_cidr(ipv6, IpFamily::V6).unwrap_or_else(|_| ipv6.to_string())
        },
    })
}

fn parse_cilium_lb_stored(v: &Value) -> CiliumLbConfig {
    CiliumLbConfig {
        ipv4: v
            .get("ipv4")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        ipv6: v
            .get("ipv6")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
    }
}

fn parse_ingress_stored(v: &Value) -> IngressConfig {
    let tag = v
        .get("image_tag")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim();
    IngressConfig {
        image_tag: if tag.is_empty() {
            INGRESS_IMAGE_TAG.into()
        } else {
            tag.to_string()
        },
        admin_host: v
            .get("admin_host")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        tls_secret: v
            .get("tls_secret")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        admin_password: v
            .get("admin_password")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        registry_user: v
            .get("registry_user")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        registry_password: v
            .get("registry_password")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
    }
}

fn parse_kubernetes_dashboard_stored(v: &Value) -> KubernetesDashboardConfig {
    let image_tag = v
        .get("image_tag")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim();
    KubernetesDashboardConfig {
        namespace: v
            .get("namespace")
            .and_then(|x| x.as_str())
            .filter(|x| !x.trim().is_empty())
            .unwrap_or(KUBERNETES_DASHBOARD_NAMESPACE)
            .to_string(),
        image_tag: if image_tag.is_empty() {
            KUBERNETES_DASHBOARD_IMAGE_TAG.into()
        } else {
            image_tag.to_string()
        },
        username: v
            .get("username")
            .and_then(|x| x.as_str())
            .filter(|x| !x.trim().is_empty())
            .unwrap_or("admin")
            .to_string(),
        password: v
            .get("password")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        host: v
            .get("host")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        tls_secret: v
            .get("tls_secret")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
    }
}

fn parse_ingress_secrets(raw: &str) -> IngressSecrets {
    let t = raw.trim();
    if t.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<Value>(t) {
            return IngressSecrets {
                admin_password: v
                    .get("admin_password")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                registry_password: v
                    .get("registry_password")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
            };
        }
    }
    IngressSecrets {
        admin_password: raw.to_string(),
        registry_password: String::new(),
    }
}

fn encode_ingress_secrets(s: &IngressSecrets) -> String {
    json!({
        "admin_password": s.admin_password,
        "registry_password": s.registry_password,
    })
    .to_string()
}

fn decrypt_ingress_secrets(state: &AppState, enc: Option<&str>) -> IngressSecrets {
    let Some(enc) = enc.filter(|s| !s.is_empty()) else {
        return IngressSecrets::default();
    };
    match crypto::decrypt(&state.cfg().secret_key, enc) {
        Ok(raw) => parse_ingress_secrets(&raw),
        Err(_) => IngressSecrets::default(),
    }
}

fn parse_kubernetes_dashboard_secrets(raw: &str) -> KubernetesDashboardSecrets {
    let raw = raw.trim();
    if raw.starts_with('{') {
        if let Ok(value) = serde_json::from_str::<Value>(raw) {
            return KubernetesDashboardSecrets {
                password: value
                    .get("password")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                jwt_secret: value
                    .get("jwt_secret")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            };
        }
    }
    KubernetesDashboardSecrets {
        password: raw.to_string(),
        jwt_secret: String::new(),
    }
}

fn encode_kubernetes_dashboard_secrets(secrets: &KubernetesDashboardSecrets) -> String {
    json!({
        "password": secrets.password,
        "jwt_secret": secrets.jwt_secret,
    })
    .to_string()
}

fn generate_dashboard_jwt_secret() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn public_ingress_config(cfg: &IngressConfig) -> Value {
    json!({
        "image_tag": if cfg.image_tag.trim().is_empty() {
            INGRESS_IMAGE_TAG
        } else {
            cfg.image_tag.trim()
        },
        "admin_host": cfg.admin_host.trim(),
        "tls_secret": ingress_tls_secret(cfg),
        "registry_user": cfg.registry_user.trim(),
        "image": format!(
            "{INGRESS_IMAGE_REGISTRY}/{INGRESS_IMAGE_REPO}:{}",
            if cfg.image_tag.trim().is_empty() {
                INGRESS_IMAGE_TAG
            } else {
                cfg.image_tag.trim()
            }
        ),
    })
}

pub fn public_kubernetes_dashboard_config(cfg: &KubernetesDashboardConfig) -> Value {
    json!({
        "namespace": if cfg.namespace.trim().is_empty() {
            KUBERNETES_DASHBOARD_NAMESPACE
        } else {
            cfg.namespace.trim()
        },
        "image_tag": if cfg.image_tag.trim().is_empty() {
            KUBERNETES_DASHBOARD_IMAGE_TAG
        } else {
            cfg.image_tag.trim()
        },
        "username": if cfg.username.trim().is_empty() { "admin" } else { cfg.username.trim() },
        "host": cfg.host.trim(),
        "tls_secret": dashboard_tls_secret(cfg),
        "image": format!(
            "{KUBERNETES_DASHBOARD_IMAGE_REGISTRY}/{KUBERNETES_DASHBOARD_IMAGE_REPO}:{}",
            if cfg.image_tag.trim().is_empty() {
                KUBERNETES_DASHBOARD_IMAGE_TAG
            } else {
                cfg.image_tag.trim()
            }
        ),
    })
}

fn kubernetes_dashboard_helm_values(cfg: &KubernetesDashboardConfig, jwt_secret: &str) -> Value {
    let mut values = json!({
        "app": {
            "image": {
                "registry": KUBERNETES_DASHBOARD_IMAGE_REGISTRY,
                "repository": KUBERNETES_DASHBOARD_IMAGE_REPO,
                "tag": cfg.image_tag.trim(),
            },
            "auth": {
                "username": cfg.username.trim(),
                "password": cfg.password.trim(),
                "jwtSecret": jwt_secret,
            },
        },
        "ingress": {
            "enabled": !cfg.host.trim().is_empty(),
            "className": "pertisk-proxy",
        },
    });
    let host = cfg.host.trim();
    if !host.is_empty() {
        values["ingress"]["hosts"] = json!([{
            "host": host,
            "paths": [{ "path": "/", "pathType": "Prefix" }],
        }]);
        let tls_secret = dashboard_tls_secret(cfg);
        values["ingress"]["tls"] = if tls_secret.is_empty() {
            json!([])
        } else {
            json!([{ "secretName": tls_secret, "hosts": [host] }])
        };
    }
    values
}

fn registry_user_ok(user: &str) -> bool {
    !user.is_empty()
        && user.len() <= 253
        && user
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+' | '$' | '@'))
}

pub fn validate_ingress(cfg: &IngressConfig, require_registry: bool) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    let tag = cfg.image_tag.trim();
    if tag.is_empty() {
        errors.push("image tag is required".into());
    } else if tag.len() > 128
        || tag
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+')))
    {
        errors.push("image tag must be a Docker tag (letters, digits, . _ - +)".into());
    }
    let host = cfg.admin_host.trim();
    if !host.is_empty() {
        if host.len() > 253
            || host.contains([' ', '\n', '\t', '/', '"', '\'', '$', '{', '}', '\\', ':'])
            || !host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
        {
            errors.push("admin host must be a DNS hostname".into());
        }
    }
    let tls = ingress_tls_secret(cfg);
    if !tls.is_empty() {
        if host.is_empty() {
            errors.push("TLS secret requires an admin host".into());
        } else if !k8s_name_ok(&tls) {
            errors.push("TLS secret must be a Kubernetes resource name".into());
        }
    }
    let password = cfg.admin_password.trim();
    if !password.is_empty() {
        if password.len() < 4 {
            errors.push("admin password is too short".into());
        } else if password.contains(['\n', '\r', '\0']) {
            errors.push("admin password contains invalid characters".into());
        }
    }
    let user = cfg.registry_user.trim();
    if !user.is_empty() && !registry_user_ok(user) {
        errors.push("Harbor user contains invalid characters".into());
    }
    let token = cfg.registry_password.trim();
    if !token.is_empty() {
        if token.len() < 4 {
            errors.push("Harbor password looks too short".into());
        } else if token.contains(['\n', '\r', '\0']) {
            errors.push("Harbor password contains invalid characters".into());
        }
    }
    if require_registry && token.is_empty() {
        errors.push("Harbor password is required when a Harbor user is set".into());
    }
    if user.is_empty() && !token.is_empty() {
        errors.push("Harbor user is required when a Harbor password is set".into());
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn dashboard_tls_secret(cfg: &KubernetesDashboardConfig) -> String {
    let tls_secret = cfg.tls_secret.trim();
    if tls_secret.is_empty() || tls_secret.eq_ignore_ascii_case("none") {
        String::new()
    } else {
        tls_secret.to_string()
    }
}

pub fn validate_kubernetes_dashboard(
    cfg: &KubernetesDashboardConfig,
    require_password: bool,
) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    if !k8s_name_ok(cfg.namespace.trim()) {
        errors.push("dashboard namespace must be a Kubernetes resource name".into());
    }
    let image_tag = cfg.image_tag.trim();
    if image_tag.is_empty()
        || image_tag.len() > 128
        || image_tag
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+')))
    {
        errors.push("image tag must be a Docker tag (letters, digits, . _ - +)".into());
    }
    let username = cfg.username.trim();
    if username.is_empty() || username.len() > 128 || username.contains(['\n', '\r', '\0']) {
        errors.push("dashboard user must be 1-128 characters without line breaks".into());
    }
    let password = cfg.password.trim();
    if require_password && password.is_empty() {
        errors.push("dashboard password is required".into());
    } else if !password.is_empty() && (password.len() < 8 || password.contains(['\n', '\r', '\0']))
    {
        errors.push("dashboard password must be at least 8 characters without line breaks".into());
    }
    let host = cfg.host.trim();
    if !host.is_empty()
        && (host.len() > 253
            || host.contains([' ', '\n', '\t', '/', '"', '\'', '$', '{', '}', '\\', ':'])
            || !host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-')))
    {
        errors.push("dashboard host must be a DNS hostname".into());
    }
    let tls_secret = dashboard_tls_secret(cfg);
    if !tls_secret.is_empty() {
        if host.is_empty() {
            errors.push("TLS secret requires a dashboard host".into());
        } else if !k8s_name_ok(&tls_secret) {
            errors.push("TLS secret must be a Kubernetes resource name".into());
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn json_str(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| {
            x.as_str()
                .map(str::to_string)
                .or_else(|| x.as_i64().map(|n| n.to_string()))
                .or_else(|| x.as_f64().map(|n| n.to_string()))
        })
        .unwrap_or_default()
}

fn json_i64(v: &Value, key: &str, default: i64) -> i64 {
    if let Some(n) = v.get(key).and_then(|x| x.as_i64()) {
        return n;
    }
    json_str(v, key).trim().parse::<i64>().unwrap_or(default)
}

pub fn parse_kos_scaler_stored(v: &Value) -> KosScalerConfig {
    KosScalerConfig {
        username: json_str(v, "username"),
        password: json_str(v, "password"),
        min_size: json_i64(v, "min_size", 2),
        max_size: json_i64(v, "max_size", 10),
        image_tag: json_str(v, "image_tag"),
        storage_class: json_str(v, "storage_class"),
        mgmt_url: json_str(v, "mgmt_url"),
    }
}

pub fn public_kos_scaler_config(cfg: &KosScalerConfig) -> Value {
    let tag = if cfg.image_tag.trim().is_empty() {
        KOS_SCALER_IMAGE_TAG
    } else {
        cfg.image_tag.trim()
    };
    let sc = cfg.storage_class.trim();
    json!({
        "username": cfg.username.trim(),
        "min_size": if cfg.min_size > 0 { cfg.min_size } else { 2 },
        "max_size": if cfg.max_size > 0 { cfg.max_size } else { 10 },
        "image_tag": tag,
        "storage_class": if sc.is_empty() { "nfs-client" } else { sc },
        "mgmt_url": cfg.mgmt_url.trim(),
    })
}

pub fn validate_kos_scaler(
    cfg: &KosScalerConfig,
    require_password: bool,
) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    if cfg.username.trim().is_empty() {
        errors.push("mgmt username is required".into());
    }
    if require_password && cfg.password.trim().is_empty() {
        errors.push("mgmt password is required".into());
    }
    if !cfg.password.trim().is_empty() && cfg.password.contains(['\n', '\r', '\0']) {
        errors.push("mgmt password contains invalid characters".into());
    }
    if cfg.min_size < 0 {
        errors.push("worker min must be >= 0".into());
    }
    if cfg.max_size < 1 {
        errors.push("worker max must be >= 1".into());
    }
    if cfg.max_size < cfg.min_size {
        errors.push("worker max must be >= worker min".into());
    }
    let tag = cfg.image_tag.trim();
    if !tag.is_empty()
        && (tag.len() > 128
            || tag
                .chars()
                .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))))
    {
        errors.push("image tag must be a Docker tag (letters, digits, . _ - +)".into());
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn kos_scaler_endpoint(cfg: &KosScalerConfig, public_url: &str) -> String {
    let override_url = cfg.mgmt_url.trim().trim_end_matches('/');
    if !override_url.is_empty() {
        return override_url.to_string();
    }
    public_url.trim().trim_end_matches('/').to_string()
}

fn kos_scaler_helm_values(
    cfg: &KosScalerConfig,
    cluster_id: &str,
    endpoint: &str,
    password: &str,
) -> Value {
    let tag = if cfg.image_tag.trim().is_empty() {
        KOS_SCALER_IMAGE_TAG
    } else {
        cfg.image_tag.trim()
    };
    let sc = cfg.storage_class.trim();
    let persist = !(sc.is_empty() || sc.eq_ignore_ascii_case("none"));
    let mut values = json!({
        "image": { "tag": tag },
        "mgmt": {
            "endpoint": endpoint,
            "clusterId": cluster_id,
            "username": cfg.username.trim(),
            "password": password,
        },
        "config": {
            "workerPool": {
                "minSize": if cfg.min_size > 0 { cfg.min_size } else { 2 },
                "maxSize": if cfg.max_size > 0 { cfg.max_size } else { 10 },
            }
        },
        "statePersistence": {
            "enabled": persist,
        }
    });
    if persist {
        values["statePersistence"]["storageClassName"] =
            json!(if sc.is_empty() { "nfs-client" } else { sc });
    }
    values
}

fn kube_arch(raw: &str) -> String {
    crate::os_upgrade::normalize_arch(raw).unwrap_or_else(|_| "amd64".into())
}

/// Pin a multi-arch tag to `tag@sha256:…` for `linux/{arch}`, or `tag-{arch}` when already suffixed.
pub fn ingress_pin_tag(raw: &str, arch: &str) -> String {
    let tag = if raw.trim().is_empty() {
        INGRESS_IMAGE_TAG
    } else {
        raw.trim()
    };
    let arch = kube_arch(arch);
    if tag.ends_with("-amd64") || tag.ends_with("-arm64") || tag.contains('@') {
        tag.to_string()
    } else {
        format!("{tag}-{arch}")
    }
}

fn pick_platform_digest(index: &Value, arch: &str) -> Option<String> {
    let manifests = index.get("manifests")?.as_array()?;
    for m in manifests {
        let media = m.get("mediaType").and_then(|v| v.as_str()).unwrap_or("");
        if media.contains("attestation") {
            continue;
        }
        let Some(p) = m.get("platform") else {
            continue;
        };
        let a = p.get("architecture").and_then(|v| v.as_str()).unwrap_or("");
        if a.is_empty() || a == "unknown" {
            continue;
        }
        let os = p.get("os").and_then(|v| v.as_str()).unwrap_or("linux");
        if os != "linux" {
            continue;
        }
        if a == arch || (arch == "arm64" && a.starts_with("arm64")) {
            if let Some(d) = m.get("digest").and_then(|v| v.as_str()) {
                if d.starts_with("sha256:") {
                    return Some(d.to_string());
                }
            }
        }
    }
    None
}

fn index_platforms(index: &Value) -> Vec<String> {
    index
        .get("manifests")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let a = m.pointer("/platform/architecture")?.as_str()?;
                    if a == "unknown" {
                        None
                    } else {
                        Some(a.to_string())
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn resolve_ingress_image_tag(
    tag: &str,
    arch: &str,
    user: &str,
    password: &str,
) -> anyhow::Result<String> {
    let arch = kube_arch(arch);
    let tag = if tag.trim().is_empty() {
        INGRESS_IMAGE_TAG.to_string()
    } else {
        tag.trim().to_string()
    };
    if tag.ends_with("-amd64") || tag.ends_with("-arm64") || tag.contains('@') {
        return Ok(tag);
    }

    let url = format!("https://{INGRESS_IMAGE_REGISTRY}/v2/{INGRESS_IMAGE_REPO}/manifests/{tag}");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let mut req = client.get(&url).header(
        "Accept",
        "application/vnd.oci.image.index.v1+json, \
         application/vnd.docker.distribution.manifest.list.v2+json, \
         application/vnd.oci.image.manifest.v1+json, \
         application/vnd.docker.distribution.manifest.v2+json",
    );
    if !user.trim().is_empty() && !password.is_empty() {
        req = req.basic_auth(user.trim(), Some(password));
    }
    let res = req.send().await;

    let Ok(res) = res else {
        return Ok(ingress_pin_tag(&tag, &arch));
    };
    if !res.status().is_success() {
        return Ok(ingress_pin_tag(&tag, &arch));
    }
    let body: Value = match res.json().await {
        Ok(v) => v,
        Err(_) => return Ok(ingress_pin_tag(&tag, &arch)),
    };
    if body.get("manifests").and_then(|v| v.as_array()).is_some() {
        if let Some(digest) = pick_platform_digest(&body, &arch) {
            return Ok(format!("{tag}@{digest}"));
        }
        let platforms = index_platforms(&body);
        anyhow::bail!(
            "image {INGRESS_IMAGE_REGISTRY}/{INGRESS_IMAGE_REPO}:{tag} has no linux/{arch} \
             (platforms: {}). Rebuild with docker buildx --platform linux/amd64,linux/arm64",
            if platforms.is_empty() {
                "none".into()
            } else {
                platforms.join(", ")
            }
        );
    }
    Ok(ingress_pin_tag(&tag, &arch))
}

fn ingress_service_ip_policy(network_mode: &str) -> (&'static str, Vec<&'static str>) {
    match network_mode.trim().to_ascii_lowercase().as_str() {
        "ipv6" => ("SingleStack", vec!["IPv6"]),
        "dual-stack" | "dualstack" => ("PreferDualStack", vec!["IPv4", "IPv6"]),
        _ => ("SingleStack", vec!["IPv4"]),
    }
}

/// Map image tag (`v0.1.85`, `v0.1.85-arm64`, `v0.1.85@sha256:…`) to chart version (`0.1.85`).
fn ingress_chart_version(image_tag: &str) -> Option<String> {
    let mut t = image_tag.trim();
    if t.is_empty() {
        t = INGRESS_IMAGE_TAG;
    }
    let t = t.strip_prefix('v').unwrap_or(t);
    let t = t.split('@').next().unwrap_or(t);
    let t = t
        .strip_suffix("-arm64")
        .or_else(|| t.strip_suffix("-amd64"))
        .unwrap_or(t);
    if t.is_empty() || !t.contains('.') || !t.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    Some(t.to_string())
}

pub fn ingress_helm_values(
    cfg: &IngressConfig,
    network_mode: &str,
    gateway_api: bool,
    password: Option<&str>,
    pull_secret: bool,
    resolved_tag: &str,
    cluster_arch: &str,
) -> Value {
    let (policy, families) = ingress_service_ip_policy(network_mode);
    let host = cfg.admin_host.trim();
    let tls = ingress_tls_secret(cfg);
    let tag = if resolved_tag.trim().is_empty() {
        if cfg.image_tag.trim().is_empty() {
            INGRESS_IMAGE_TAG
        } else {
            cfg.image_tag.trim()
        }
    } else {
        resolved_tag.trim()
    };
    let arch = kube_arch(cluster_arch);
    let mut values = json!({
        "image": {
            "registry": INGRESS_IMAGE_REGISTRY,
            "repository": INGRESS_IMAGE_REPO,
            "tag": tag,
            "pullPolicy": "Always",
        },
        "nodeSelector": {
            "kubernetes.io/arch": arch,
        },
        "service": {
            "type": "LoadBalancer",
            "ipFamilyPolicy": policy,
            "ipFamilies": families,
        },
        "gatewayApi": { "enabled": gateway_api },
        "gatewayClassResource": { "enabled": gateway_api },
        "adminIngress": {
            "enabled": !host.is_empty(),
            "host": host,
            "tlsSecretName": tls,
        },
        "ingressClassName": "pertisk-proxy",
    });
    if pull_secret {
        values["imagePullSecrets"] = json!([{ "name": INGRESS_PULL_SECRET }]);
    }
    if let Some(pw) = password.filter(|p| !p.trim().is_empty()) {
        values["auth"] = json!({
            "createSecret": true,
            "password": pw.trim(),
        });
    }
    values
}

/// Admin Ingress. Empty `tls_secret` means HTTP only (no `spec.tls`).
pub fn admin_ingress_doc(host: &str, tls_secret: &str) -> Value {
    let mut doc = json!({
        "apiVersion": "networking.k8s.io/v1",
        "kind": "Ingress",
        "metadata": {
            "name": INGRESS_ADMIN,
            "namespace": INGRESS_NAMESPACE,
            "annotations": {
                "meta.helm.sh/release-name": INGRESS_RELEASE,
                "meta.helm.sh/release-namespace": INGRESS_NAMESPACE,
                "proxy.pertisk.tech/security-exempt": "true",
            },
            "labels": {
                "app.kubernetes.io/instance": INGRESS_RELEASE,
                "app.kubernetes.io/managed-by": "Helm",
                "app.kubernetes.io/name": "pertisk-ingress",
            },
        },
        "spec": {
            "ingressClassName": "pertisk-proxy",
            "rules": [{
                "host": host.trim(),
                "http": {
                    "paths": [{
                        "path": "/",
                        "pathType": "Prefix",
                        "backend": {
                            "service": {
                                "name": INGRESS_DEPLOY,
                                "port": { "number": 9080 }
                            }
                        }
                    }]
                }
            }]
        }
    });
    let tls = tls_secret.trim();
    if !tls.is_empty() && !tls.eq_ignore_ascii_case("none") {
        doc["spec"]["tls"] = json!([{
            "hosts": [host.trim()],
            "secretName": tls,
        }]);
    }
    doc
}

pub fn harbor_pull_secret_doc(user: &str, password: &str) -> Value {
    let auth = B64.encode(format!("{}:{password}", user.trim()));
    let dockerconfig = json!({
        "auths": {
            INGRESS_IMAGE_REGISTRY: {
                "username": user.trim(),
                "password": password,
                "auth": auth,
            }
        }
    });
    json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": {
            "name": INGRESS_PULL_SECRET,
            "namespace": INGRESS_NAMESPACE,
        },
        "type": "kubernetes.io/dockerconfigjson",
        "stringData": {
            ".dockerconfigjson": dockerconfig.to_string(),
        }
    })
}

pub fn render_cilium_lb_pool_yaml(cfg: &CiliumLbConfig, api_version: &str) -> String {
    let mut blocks = String::new();
    if let Ok(c) = normalize_lb_cidr(cfg.ipv4.trim(), IpFamily::V4) {
        blocks.push_str(&format!("    - cidr: {c}\n"));
    }
    if let Ok(c) = normalize_lb_cidr(cfg.ipv6.trim(), IpFamily::V6) {
        blocks.push_str(&format!("    - cidr: {c}\n"));
    }
    format!(
        r#"apiVersion: {api_version}
kind: CiliumLoadBalancerIPPool
metadata:
  name: {CILIUM_LB_POOL}
spec:
  blocks:
{blocks}  disabled: false
"#
    )
}

pub fn render_cilium_l2_yaml(api_version: &str) -> String {
    format!(
        r#"apiVersion: {api_version}
kind: CiliumL2AnnouncementPolicy
metadata:
  name: {CILIUM_LB_L2}
spec:
  externalIPs: true
  loadBalancerIPs: true
"#
    )
}

pub fn validate_nfs(cfg: &NfsConfig) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    let server = cfg.server.trim();
    let path = cfg.path.trim();
    if server.is_empty() {
        errors.push("NFS server is required".into());
    } else if server.contains([' ', '\n', '\t', '/', '"', '\'', '$', '{', '}', '\\']) {
        errors.push("NFS server must be an IP or hostname".into());
    } else if server.len() > 253 {
        errors.push("NFS server is too long".into());
    }
    if path.is_empty() {
        errors.push("export path is required".into());
    } else if !path.starts_with('/') {
        errors.push("export path must start with /".into());
    } else if path
        .chars()
        .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-')))
    {
        errors.push("export path contains invalid characters".into());
    } else if path.len() > 512 {
        errors.push("export path is too long".into());
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

pub fn validate_cert_manager(
    cfg: &CertManagerConfig,
    require_token: bool,
) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    let provider = cfg.provider.trim().to_ascii_lowercase();
    if provider != "cloudflare" {
        errors.push("provider must be cloudflare".into());
    }
    let email = cfg.email.trim();
    if email.is_empty() {
        errors.push("ACME email is required".into());
    } else if !email_ok(email) {
        errors.push("ACME email is invalid".into());
    }
    let acme = cfg.acme.trim().to_ascii_lowercase();
    if !acme.is_empty() && acme != "production" && acme != "staging" {
        errors.push("acme must be production or staging".into());
    }
    if require_token {
        let token = cfg.api_token.trim();
        if token.is_empty() {
            errors.push("Cloudflare API token is required".into());
        } else if token.len() < 8 {
            errors.push("Cloudflare API token looks too short".into());
        } else if token.contains(['\n', '\r', '\0']) {
            errors.push("Cloudflare API token contains invalid characters".into());
        }
    }
    let domain = cfg.domain.trim();
    if !domain.is_empty() && !domain_ok(domain) {
        errors.push("wildcard domain must be a DNS name (e.g. vsphere.example.com or *.vsphere.example.com)".into());
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn email_ok(email: &str) -> bool {
    let Some((user, host)) = email.split_once('@') else {
        return false;
    };
    !user.is_empty() && host.contains('.') && !host.starts_with('.') && !host.ends_with('.')
}

pub fn render_nfs_provisioner(cfg: &NfsConfig) -> String {
    NFS_PROVISIONER_YAML
        .replace("${NFS_SERVER}", cfg.server.trim())
        .replace("${NFS_PATH}", cfg.path.trim())
}

fn acme_url(acme: &str) -> &'static str {
    if acme.trim().eq_ignore_ascii_case("staging") {
        ACME_STAGING
    } else {
        ACME_PROD
    }
}

pub fn cert_manager_issuer_yaml(cfg: &CertManagerConfig) -> String {
    let issuer = if cfg.issuer.trim().is_empty() {
        default_issuer()
    } else {
        cfg.issuer.trim().to_string()
    };
    let email = cfg.email.trim();
    let server = acme_url(&cfg.acme);
    format!(
        r#"apiVersion: cert-manager.io/v1
kind: ClusterIssuer
metadata:
  name: {issuer}
spec:
  acme:
    email: {email}
    server: {server}
    privateKeySecretRef:
      name: {issuer}
    solvers:
      - dns01:
          cloudflare:
            apiTokenSecretRef:
              name: cloudflare-api-token-secret
              key: api-token
"#
    )
}

pub fn wildcard_certificate_doc(cfg: &CertManagerConfig) -> Option<Value> {
    let names = wildcard_dns_names(&cfg.domain);
    if names.is_empty() {
        return None;
    }
    let name = cert_secret_name(&cfg.domain);
    let issuer = if cfg.issuer.trim().is_empty() {
        default_issuer()
    } else {
        cfg.issuer.trim().to_string()
    };
    Some(json!({
        "apiVersion": "cert-manager.io/v1",
        "kind": "Certificate",
        "metadata": {
            "name": name,
            "namespace": CERT_NS,
        },
        "spec": {
            "secretName": name,
            "duration": "2160h",
            "renewBefore": "360h",
            "isCA": false,
            "privateKey": {
                "algorithm": "RSA",
                "encoding": "PKCS1",
                "size": 2048,
            },
            "usages": ["server auth", "client auth"],
            "dnsNames": names,
            "issuerRef": {
                "name": issuer,
                "kind": "ClusterIssuer",
                "group": "cert-manager.io",
            },
            "secretTemplate": {
                "annotations": {
                    "reflector.v1.k8s.emberstack.com/reflection-allowed": "true",
                    "reflector.v1.k8s.emberstack.com/reflection-auto-enabled": "true",
                }
            }
        }
    }))
}

/// Pertisk apiserver is a hostNetwork static pod. Cilium kubeProxyReplacement has no
/// kube-proxy, so ClusterIP (webhook.cert-manager.svc) is often unreachable
/// (`dial tcp 10.x.x.x:443: no route to host`). Run the webhook on the host
/// network, on a port that does not collide with kubelet.
fn patch_webhook_host_network(deploy: &mut Value, port: u16) {
    strip_kubectl_noise(deploy);
    let Some(spec) = deploy.pointer_mut("/spec/template/spec") else {
        return;
    };
    spec["hostNetwork"] = json!(true);
    spec["dnsPolicy"] = json!("ClusterFirstWithHostNet");
    if let Some(containers) = spec.get_mut("containers").and_then(|c| c.as_array_mut()) {
        for c in containers {
            patch_webhook_container_port(c, port);
        }
    }
}

fn patch_webhook_container_port(container: &mut Value, port: u16) {
    if let Some(args) = container.get_mut("args").and_then(|a| a.as_array_mut()) {
        let mut found = false;
        for a in args.iter_mut() {
            if let Some(s) = a.as_str() {
                if let Some(rest) = s.strip_prefix("--secure-port=") {
                    if rest != port.to_string() {
                        *a = json!(format!("--secure-port={port}"));
                    }
                    found = true;
                }
            }
        }
        if !found {
            args.push(json!(format!("--secure-port={port}")));
        }
    }
    if let Some(ports) = container.get_mut("ports").and_then(|p| p.as_array_mut()) {
        for p in ports {
            let named_https = p.get("name").and_then(|n| n.as_str()) == Some("https");
            let old = p.get("containerPort").and_then(|n| n.as_u64());
            if named_https || old == Some(10250) || old == Some(443) {
                p["containerPort"] = json!(port);
            }
        }
    }
}

fn patch_webhook_service_target_port(svc: &mut Value, port: u16) {
    strip_kubectl_noise(svc);
    let Some(ports) = svc
        .pointer_mut("/spec/ports")
        .and_then(|p| p.as_array_mut())
    else {
        return;
    };
    for p in ports {
        match p.get("targetPort") {
            Some(Value::Number(n)) if n.as_u64() == Some(10250) || n.as_u64() == Some(443) => {
                p["targetPort"] = json!(port);
            }
            Some(Value::String(s)) if s == "10250" || s == "443" => {
                p["targetPort"] = json!(port);
            }
            _ => {}
        }
    }
}

fn strip_kubectl_noise(obj: &mut Value) {
    if let Some(map) = obj.as_object_mut() {
        map.remove("status");
    }
    if let Some(meta) = obj.get_mut("metadata").and_then(|m| m.as_object_mut()) {
        meta.remove("managedFields");
        meta.remove("resourceVersion");
        meta.remove("uid");
        meta.remove("generation");
        meta.remove("creationTimestamp");
    }
}

fn endpoints_have_addresses(ep: &Value) -> bool {
    ep.get("subsets")
        .and_then(|s| s.as_array())
        .map(|subs| {
            subs.iter().any(|s| {
                s.get("addresses")
                    .and_then(|a| a.as_array())
                    .is_some_and(|a| !a.is_empty())
            })
        })
        .unwrap_or(false)
}

fn webhook_dial_error(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("webhook.cert-manager.io")
        || m.contains("failed calling webhook")
        || m.contains("no route to host")
        || m.contains("connection refused")
        || m.contains("i/o timeout")
        || m.contains("context deadline exceeded")
}

fn cloudflare_token_secret_json(token: &str) -> Value {
    json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": {
            "name": "cloudflare-api-token-secret",
            "namespace": "cert-manager",
        },
        "type": "Opaque",
        "stringData": {
            "api-token": token,
        }
    })
}

async fn probe_tcp(host: &str, port: u16) -> Result<(), String> {
    let addr = if let Ok(ip) = host.parse::<IpAddr>() {
        SocketAddr::new(ip, port).to_string()
    } else {
        format!("{host}:{port}")
    };
    match tokio::time::timeout(
        Duration::from_secs(3),
        tokio::net::TcpStream::connect(&addr),
    )
    .await
    {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("timeout".into()),
    }
}

fn deploy_ready(obj: &Value) -> bool {
    let status = obj.get("status");
    let desired = status
        .and_then(|s| s.get("replicas"))
        .and_then(|v| v.as_u64())
        .unwrap_or(1);
    let ready = status
        .and_then(|s| s.get("readyReplicas"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    desired > 0 && ready >= desired
}

fn env_value(obj: &Value, name: &str) -> Option<String> {
    obj.pointer("/spec/template/spec/containers")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|c| c.get("env"))
        .and_then(|e| e.as_array())
        .and_then(|envs| {
            envs.iter()
                .find(|e| e.get("name").and_then(|n| n.as_str()) == Some(name))
        })
        .and_then(|e| e.get("value"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn ds_ready(obj: &Value) -> bool {
    let status = obj.get("status");
    let desired = status
        .and_then(|s| s.get("desiredNumberScheduled"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let ready = status
        .and_then(|s| s.get("numberReady"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    desired > 0 && ready >= desired
}

fn issuer_ready(obj: &Value) -> bool {
    obj.pointer("/status/conditions")
        .and_then(|c| c.as_array())
        .map(|conds| {
            conds.iter().any(|c| {
                c.get("type").and_then(|t| t.as_str()) == Some("Ready")
                    && c.get("status").and_then(|s| s.as_str()) == Some("True")
            })
        })
        .unwrap_or(false)
}

async fn live_nfs(kc: &Path) -> Value {
    let deploy = kubectl_json_optional(
        kc,
        &[
            "get",
            "deploy",
            "nfs-subdir-external-provisioner",
            "-n",
            "nfs-provisioner",
            "-o",
            "json",
        ],
    )
    .await
    .ok()
    .flatten();
    let sc = kubectl_json_optional(kc, &["get", "sc", "nfs-client", "-o", "json"])
        .await
        .ok()
        .flatten();
    let ds = kubectl_json_optional(
        kc,
        &[
            "get",
            "ds",
            "pertisk-nfs-modules",
            "-n",
            "kube-system",
            "-o",
            "json",
        ],
    )
    .await
    .ok()
    .flatten();

    let server = deploy.as_ref().and_then(|d| env_value(d, "NFS_SERVER"));
    let path = deploy.as_ref().and_then(|d| env_value(d, "NFS_PATH"));
    let provisioner_ready = deploy.as_ref().map(deploy_ready).unwrap_or(false);
    let modules_ready = ds.as_ref().map(ds_ready).unwrap_or(false);
    let installed = deploy.is_some() && sc.is_some();
    json!({
        "installed": installed,
        "partial": deploy.is_some() || sc.is_some() || ds.is_some(),
        "provisioner_ready": provisioner_ready,
        "storage_class": sc.is_some(),
        "nfs_modules": ds.is_some(),
        "nfs_modules_ready": modules_ready,
        "server": server,
        "path": path,
    })
}

async fn live_cert_manager(kc: &Path, issuer: &str, domain: &str) -> Value {
    let deploy = kubectl_json_optional(
        kc,
        &[
            "get",
            "deploy",
            "cert-manager",
            "-n",
            "cert-manager",
            "-o",
            "json",
        ],
    )
    .await
    .ok()
    .flatten();
    let webhook = kubectl_json_optional(
        kc,
        &[
            "get",
            "deploy",
            "cert-manager-webhook",
            "-n",
            "cert-manager",
            "-o",
            "json",
        ],
    )
    .await
    .ok()
    .flatten();
    let issuer_obj = kubectl_json_optional(kc, &["get", "clusterissuer", issuer, "-o", "json"])
        .await
        .ok()
        .flatten();
    let secret = kubectl_json_optional(
        kc,
        &[
            "get",
            "secret",
            "cloudflare-api-token-secret",
            "-n",
            "cert-manager",
            "-o",
            "json",
        ],
    )
    .await
    .ok()
    .flatten();
    let reflector = kubectl_get_any(
        kc,
        &[
            "get",
            "deploy",
            "reflector",
            "-n",
            "reflector",
            "-o",
            "json",
        ],
        &[
            "get",
            "deploy",
            "reflector",
            "-n",
            "kube-system",
            "-o",
            "json",
        ],
    )
    .await;

    let cert_name = if domain.trim().is_empty() {
        String::new()
    } else {
        cert_secret_name(domain)
    };
    let certificate = if cert_name.is_empty() {
        None
    } else {
        kubectl_json_optional(
            kc,
            &[
                "get",
                "certificate",
                &cert_name,
                "-n",
                CERT_NS,
                "-o",
                "json",
            ],
        )
        .await
        .ok()
        .flatten()
    };
    let tls_secret = if cert_name.is_empty() {
        None
    } else {
        kubectl_json_optional(
            kc,
            &["get", "secret", &cert_name, "-n", CERT_NS, "-o", "json"],
        )
        .await
        .ok()
        .flatten()
    };

    let version = deploy
        .as_ref()
        .and_then(|d| {
            d.pointer("/spec/template/spec/containers/0/image")
                .and_then(|v| v.as_str())
        })
        .map(|img| img.rsplit(':').next().unwrap_or(img).to_string());

    json!({
        "installed": deploy.is_some(),
        "partial": deploy.is_some() || issuer_obj.is_some() || secret.is_some(),
        "controller_ready": deploy.as_ref().map(deploy_ready).unwrap_or(false),
        "webhook_ready": webhook.as_ref().map(deploy_ready).unwrap_or(false),
        "issuer": issuer_obj.is_some(),
        "issuer_ready": issuer_obj.as_ref().map(issuer_ready).unwrap_or(false),
        "token_secret": secret.is_some(),
        "version": version,
        "reflector": reflector.is_some(),
        "reflector_ready": reflector.as_ref().map(deploy_ready).unwrap_or(false),
        "certificate": certificate.is_some(),
        "certificate_ready": certificate.as_ref().map(certificate_ready).unwrap_or(false),
        "tls_secret": if cert_name.is_empty() { Value::Null } else { json!(cert_name) },
        "tls_secret_present": tls_secret.is_some(),
        "dns_names": wildcard_dns_names(domain),
    })
}

fn certificate_ready(obj: &Value) -> bool {
    let Some(conds) = obj.pointer("/status/conditions").and_then(|v| v.as_array()) else {
        return false;
    };
    conds.iter().any(|c| {
        c.get("type").and_then(|t| t.as_str()) == Some("Ready")
            && c.get("status").and_then(|s| s.as_str()) == Some("True")
    })
}

async fn kubectl_get_any(kc: &Path, args_a: &[&str], args_b: &[&str]) -> Option<Value> {
    if let Ok(Some(v)) = kubectl_json_optional(kc, args_a).await {
        return Some(v);
    }
    kubectl_json_optional(kc, args_b).await.ok().flatten()
}

fn pool_cidrs(obj: &Value) -> Vec<String> {
    obj.pointer("/spec/blocks")
        .and_then(|b| b.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|b| {
                    b.get("cidr")
                        .and_then(|c| c.as_str())
                        .map(|s| s.to_string())
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn live_cilium_lb(kc: &Path) -> Value {
    let pool = kubectl_get_any(
        kc,
        &[
            "get",
            "ciliumloadbalancerippool",
            CILIUM_LB_POOL,
            "-o",
            "json",
        ],
        &[
            "get",
            "ciliumloadbalancerippool",
            CILIUM_LB_POOL,
            "-n",
            "cilium",
            "-o",
            "json",
        ],
    )
    .await;
    let l2 = kubectl_get_any(
        kc,
        &[
            "get",
            "ciliuml2announcementpolicy",
            CILIUM_LB_L2,
            "-o",
            "json",
        ],
        &[
            "get",
            "ciliuml2announcementpolicy",
            CILIUM_LB_L2,
            "-n",
            "cilium",
            "-o",
            "json",
        ],
    )
    .await;
    let cidrs = pool.as_ref().map(pool_cidrs).unwrap_or_default();
    let ipv4 = cidrs.iter().find(|c| !c.contains(':')).cloned();
    let ipv6 = cidrs.iter().find(|c| c.contains(':')).cloned();
    json!({
        "installed": pool.is_some() && l2.is_some(),
        "partial": pool.is_some() || l2.is_some(),
        "pool": pool.is_some(),
        "l2": l2.is_some(),
        "ipv4": ipv4,
        "ipv6": ipv6,
        "cidrs": cidrs,
    })
}

fn container_image(obj: &Value) -> Option<String> {
    obj.pointer("/spec/template/spec/containers/0/image")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn svc_lb_addrs(obj: &Value) -> (Vec<String>, Vec<String>) {
    let mut v4 = Vec::new();
    let mut v6 = Vec::new();
    let Some(arr) = obj
        .pointer("/status/loadBalancer/ingress")
        .and_then(|v| v.as_array())
    else {
        return (v4, v6);
    };
    for e in arr {
        if let Some(ip) = e.get("ip").and_then(|v| v.as_str()) {
            if ip.contains(':') {
                v6.push(ip.to_string());
            } else {
                v4.push(ip.to_string());
            }
        }
        if let Some(host) = e.get("hostname").and_then(|v| v.as_str()) {
            v4.push(host.to_string());
        }
    }
    (v4, v6)
}

async fn gateway_api_available(kc: &Path) -> bool {
    kubectl_json_optional(
        kc,
        &[
            "get",
            "crd",
            "gatewayclasses.gateway.networking.k8s.io",
            "-o",
            "json",
        ],
    )
    .await
    .ok()
    .flatten()
    .is_some()
}

async fn live_ingress(kc: &Path) -> Value {
    let deploy = kubectl_json_optional(
        kc,
        &[
            "get",
            "deploy",
            INGRESS_DEPLOY,
            "-n",
            INGRESS_NAMESPACE,
            "-o",
            "json",
        ],
    )
    .await
    .ok()
    .flatten();
    let svc = kubectl_json_optional(
        kc,
        &[
            "get",
            "svc",
            INGRESS_DEPLOY,
            "-n",
            INGRESS_NAMESPACE,
            "-o",
            "json",
        ],
    )
    .await
    .ok()
    .flatten();
    let class = kubectl_json_optional(kc, &["get", "ingressclass", "pertisk-proxy", "-o", "json"])
        .await
        .ok()
        .flatten();
    let (lb_ipv4, lb_ipv6) = svc.as_ref().map(svc_lb_addrs).unwrap_or_default();
    let secret = kubectl_json_optional(
        kc,
        &[
            "get",
            "secret",
            INGRESS_PULL_SECRET,
            "-n",
            INGRESS_NAMESPACE,
            "-o",
            "json",
        ],
    )
    .await
    .ok()
    .flatten();
    let pods = kubectl_json_optional(
        kc,
        &[
            "get",
            "pods",
            "-n",
            INGRESS_NAMESPACE,
            "-l",
            "app.kubernetes.io/instance=pertisk-ingress",
            "-o",
            "json",
        ],
    )
    .await
    .ok()
    .flatten();
    let pull_error = pods.as_ref().and_then(first_image_pull_error);
    let admin = kubectl_json_optional(
        kc,
        &[
            "get",
            "ingress",
            INGRESS_ADMIN,
            "-n",
            INGRESS_NAMESPACE,
            "-o",
            "json",
        ],
    )
    .await
    .ok()
    .flatten();
    let admin_host = admin
        .as_ref()
        .and_then(|i| i.pointer("/spec/rules/0/host"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let admin_tls = admin
        .as_ref()
        .and_then(|i| i.pointer("/spec/tls"))
        .and_then(|v| v.as_array())
        .is_some_and(|a| !a.is_empty());
    let admin_tls_secret = admin
        .as_ref()
        .and_then(|i| i.pointer("/spec/tls/0/secretName"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let tls_secrets = list_tls_secret_names(kc, &[INGRESS_NAMESPACE, CERT_NS]).await;
    json!({
        "installed": deploy.is_some() && svc.is_some(),
        "partial": deploy.is_some() || svc.is_some() || class.is_some(),
        "controller_ready": deploy.as_ref().map(deploy_ready).unwrap_or(false),
        "ingress_class": class.is_some(),
        "image": deploy.as_ref().and_then(container_image),
        "lb_ipv4": lb_ipv4.join(", "),
        "lb_ipv6": lb_ipv6.join(", "),
        "gateway_api": gateway_api_available(kc).await,
        "pull_secret": secret.is_some(),
        "pull_error": pull_error,
        "admin_host": admin_host,
        "admin_tls": admin_tls,
        "admin_tls_secret": admin_tls_secret,
        "tls_secrets": tls_secrets,
    })
}

async fn live_kos_scaler(kc: &Path) -> Value {
    let deploy = kubectl_json_optional(
        kc,
        &[
            "get",
            "deploy",
            KOS_SCALER_DEPLOY,
            "-n",
            KOS_SCALER_NAMESPACE,
            "-o",
            "json",
        ],
    )
    .await
    .ok()
    .flatten();
    json!({
        "installed": deploy.as_ref().map(deploy_ready).unwrap_or(false),
        "partial": deploy.is_some(),
        "ready": deploy.as_ref().map(deploy_ready).unwrap_or(false),
        "image": deploy.as_ref().and_then(container_image),
    })
}

async fn live_kubernetes_dashboard(kc: &Path, namespace: &str) -> Value {
    let deploy = kubectl_json_optional(
        kc,
        &[
            "get",
            "deploy",
            KUBERNETES_DASHBOARD_DEPLOY,
            "-n",
            namespace,
            "-o",
            "json",
        ],
    )
    .await
    .ok()
    .flatten();
    let service = kubectl_json_optional(
        kc,
        &[
            "get",
            "svc",
            KUBERNETES_DASHBOARD_DEPLOY,
            "-n",
            namespace,
            "-o",
            "json",
        ],
    )
    .await
    .ok()
    .flatten();
    let ingress = kubectl_json_optional(
        kc,
        &[
            "get",
            "ingress",
            KUBERNETES_DASHBOARD_DEPLOY,
            "-n",
            namespace,
            "-o",
            "json",
        ],
    )
    .await
    .ok()
    .flatten();
    json!({
        "installed": deploy.as_ref().map(deploy_ready).unwrap_or(false) && service.is_some(),
        "partial": deploy.is_some() || service.is_some(),
        "ready": deploy.as_ref().map(deploy_ready).unwrap_or(false),
        "service": service.is_some(),
        "image": deploy.as_ref().and_then(container_image),
        "host": ingress.as_ref().and_then(|v| v.pointer("/spec/rules/0/host")).and_then(|v| v.as_str()),
        "tls_secret": ingress.as_ref().and_then(|v| v.pointer("/spec/tls/0/secretName")).and_then(|v| v.as_str()),
        "namespace": namespace,
        "tls_secrets": list_tls_secret_names(kc, &[namespace]).await,
    })
}

async fn list_tls_secret_names(kc: &Path, namespaces: &[&str]) -> Vec<String> {
    let mut names = BTreeSet::new();
    for ns in namespaces {
        let Ok(Some(list)) =
            kubectl_json_optional(kc, &["get", "secrets", "-n", ns, "-o", "json"]).await
        else {
            continue;
        };
        let Some(items) = list.get("items").and_then(|v| v.as_array()) else {
            continue;
        };
        for s in items {
            if s.get("type").and_then(|v| v.as_str()) != Some("kubernetes.io/tls") {
                continue;
            }
            if let Some(n) = s.pointer("/metadata/name").and_then(|v| v.as_str()) {
                if k8s_name_ok(n) {
                    names.insert(n.to_string());
                }
            }
        }
    }
    if let Ok(Some(list)) = kubectl_json_optional(
        kc,
        &["get", "certificates.cert-manager.io", "-A", "-o", "json"],
    )
    .await
    {
        if let Some(items) = list.get("items").and_then(|v| v.as_array()) {
            for c in items {
                if let Some(n) = c.pointer("/spec/secretName").and_then(|v| v.as_str()) {
                    if k8s_name_ok(n) {
                        names.insert(n.to_string());
                    }
                }
            }
        }
    }
    names.into_iter().collect()
}

fn first_image_pull_error(list: &Value) -> Option<String> {
    let items = list.get("items").and_then(|v| v.as_array())?;
    for pod in items {
        for key in ["containerStatuses", "initContainerStatuses"] {
            let Some(arr) = pod
                .pointer(&format!("/status/{key}"))
                .and_then(|v| v.as_array())
            else {
                continue;
            };
            for cs in arr {
                let waiting = cs.pointer("/state/waiting");
                let reason = waiting
                    .and_then(|w| w.get("reason"))
                    .and_then(|r| r.as_str())
                    .unwrap_or("");
                if reason == "ErrImagePull" || reason == "ImagePullBackOff" {
                    let msg = waiting
                        .and_then(|w| w.get("message"))
                        .and_then(|m| m.as_str())
                        .unwrap_or(reason);
                    return Some(msg.to_string());
                }
            }
        }
    }
    None
}

async fn cilium_crd_api_version(kc: &Path, crd: &str, fallback: &str) -> String {
    let doc = kubectl_json_optional(kc, &["get", "crd", crd, "-o", "json"])
        .await
        .ok()
        .flatten();
    let Some(doc) = doc else {
        return fallback.to_string();
    };
    crd_storage_api(&doc, fallback)
}

fn crd_storage_api(crd: &Value, fallback: &str) -> String {
    let versions = crd
        .pointer("/spec/versions")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let name = versions.iter().find_map(|v| {
        if v.get("storage").and_then(|s| s.as_bool()) == Some(true) {
            v.get("name").and_then(|n| n.as_str())
        } else {
            None
        }
    });
    match name {
        Some(v) if !v.is_empty() => format!("cilium.io/{v}"),
        _ => fallback.to_string(),
    }
}

#[derive(sqlx::FromRow)]
struct AddonRow {
    status: String,
    config_json: String,
    secrets_enc: Option<String>,
    error: Option<String>,
    installed_at: Option<String>,
    updated_at: String,
}

pub fn public_nfs_config(cfg: &NfsConfig) -> Value {
    json!({
        "server": cfg.server.trim(),
        "path": cfg.path.trim(),
    })
}

pub fn public_cert_config(cfg: &CertManagerConfig) -> Value {
    json!({
        "provider": cfg.provider.trim().to_ascii_lowercase(),
        "email": cfg.email.trim(),
        "acme": if cfg.acme.trim().is_empty() { "production" } else { cfg.acme.trim() },
        "issuer": if cfg.issuer.trim().is_empty() { default_issuer() } else { cfg.issuer.trim().to_string() },
        "domain": cfg.domain.trim(),
        "tls_secret": if cfg.domain.trim().is_empty() {
            String::new()
        } else {
            cert_secret_name(&cfg.domain)
        },
    })
}

fn parse_nfs_stored(v: &Value) -> NfsConfig {
    NfsConfig {
        server: v
            .get("server")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        path: v
            .get("path")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
    }
}

fn parse_cert_stored(v: &Value) -> CertManagerConfig {
    CertManagerConfig {
        provider: v
            .get("provider")
            .and_then(|x| x.as_str())
            .unwrap_or("cloudflare")
            .to_string(),
        email: v
            .get("email")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        api_token: String::new(),
        acme: v
            .get("acme")
            .and_then(|x| x.as_str())
            .unwrap_or("production")
            .to_string(),
        issuer: v
            .get("issuer")
            .and_then(|x| x.as_str())
            .unwrap_or("letsencrypt-cloudflare")
            .to_string(),
        domain: v
            .get("domain")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
    }
}

async fn load_row(state: &AppState, cluster_id: &str, addon: &str) -> ApiResult<Option<AddonRow>> {
    let row = sqlx::query_as::<_, AddonRow>(
        "SELECT status, config_json, secrets_enc, error, installed_at, updated_at \
         FROM cluster_addons WHERE cluster_id = ? AND addon = ?",
    )
    .bind(cluster_id)
    .bind(addon)
    .fetch_optional(state.pool())
    .await?;
    Ok(row)
}

fn config_is_empty(config_json: &str) -> bool {
    let t = config_json.trim();
    t.is_empty() || t == "{}" || t == "null"
}

fn should_restore_addon(addon: &str, cni: &str) -> bool {
    if addon == CILIUM_LB_ID {
        return cni.eq_ignore_ascii_case("cilium");
    }
    true
}

async fn upsert_preset(
    state: &AppState,
    cluster_name: &str,
    addon: &str,
    config_json: &str,
    secrets_enc: Option<&str>,
    want_install: i64,
) -> anyhow::Result<()> {
    if cluster_name.trim().is_empty() || config_is_empty(config_json) {
        return Ok(());
    }
    let now = db::now_rfc3339();
    sqlx::query(
        r#"INSERT INTO addon_presets
             (cluster_name, addon, config_json, secrets_enc, want_install, updated_at)
           VALUES (?, ?, ?, ?, ?, ?)
           ON CONFLICT(cluster_name, addon) DO UPDATE SET
             config_json = excluded.config_json,
             secrets_enc = excluded.secrets_enc,
             want_install = excluded.want_install,
             updated_at = excluded.updated_at"#,
    )
    .bind(cluster_name.trim())
    .bind(addon)
    .bind(config_json)
    .bind(secrets_enc)
    .bind(want_install)
    .bind(&now)
    .execute(state.pool())
    .await?;
    Ok(())
}

/// Persist this cluster's add-on configs under its name so a later recreate can reuse them.
pub async fn snapshot_cluster(state: &AppState, cluster_id: &str) -> anyhow::Result<()> {
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM clusters WHERE id = ?")
        .bind(cluster_id)
        .fetch_optional(state.pool())
        .await?;
    let Some(name) = name.filter(|s| !s.trim().is_empty()) else {
        return Ok(());
    };
    let rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT addon, config_json, secrets_enc FROM cluster_addons WHERE cluster_id = ?",
    )
    .bind(cluster_id)
    .fetch_all(state.pool())
    .await?;
    for (addon, config_json, secrets_enc) in rows {
        upsert_preset(
            state,
            &name,
            &addon,
            &config_json,
            secrets_enc.as_deref(),
            1,
        )
        .await?;
    }
    Ok(())
}

async fn remember_addon(state: &AppState, cluster_id: &str, addon: &str) -> ApiResult<()> {
    let _ = sqlx::query(
        "UPDATE cluster_addons SET want_install = 1 WHERE cluster_id = ? AND addon = ?",
    )
    .bind(cluster_id)
    .bind(addon)
    .execute(state.pool())
    .await;
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM clusters WHERE id = ?")
        .bind(cluster_id)
        .fetch_optional(state.pool())
        .await?;
    let Some(name) = name.filter(|s| !s.trim().is_empty()) else {
        return Ok(());
    };
    let row = load_row(state, cluster_id, addon).await?;
    if let Some(row) = row {
        upsert_preset(
            state,
            &name,
            addon,
            &row.config_json,
            row.secrets_enc.as_deref(),
            1,
        )
        .await
        .map_err(AppError::Anyhow)?;
    }
    Ok(())
}

#[derive(Debug, sqlx::FromRow)]
struct PresetRow {
    cluster_name: String,
    addon: String,
    config_json: String,
    want_install: i64,
}

/// Saved add-on configs grouped by cluster name (no secrets).
pub async fn list_presets(state: &AppState) -> ApiResult<Vec<Value>> {
    let rows: Vec<PresetRow> = sqlx::query_as(
        r#"SELECT cluster_name, addon, config_json, want_install
           FROM addon_presets
           ORDER BY cluster_name COLLATE NOCASE, addon"#,
    )
    .fetch_all(state.pool())
    .await?;
    let mut grouped: Vec<(String, Vec<Value>)> = Vec::new();
    for row in rows {
        if config_is_empty(&row.config_json) {
            continue;
        }
        let entry = catalog_entry(&row.addon).ok();
        let item = json!({
            "id": row.addon,
            "name": entry.map(|e| e.name).unwrap_or(row.addon.as_str()),
            "want_install": row.want_install != 0,
            "config": serde_json::from_str::<Value>(&row.config_json).unwrap_or_else(|_| json!({})),
        });
        match grouped.last_mut() {
            Some((name, addons)) if name == &row.cluster_name => addons.push(item),
            _ => grouped.push((row.cluster_name, vec![item])),
        }
    }
    Ok(grouped
        .into_iter()
        .map(|(cluster_name, addons)| json!({ "cluster_name": cluster_name, "addons": addons }))
        .collect())
}

/// Copy saved add-on configs onto a new cluster. Returns restored addon ids.
pub async fn restore_presets(
    state: &AppState,
    cluster_id: &str,
    from_name: &str,
    cni: &str,
) -> anyhow::Result<Vec<String>> {
    let from = from_name.trim();
    if from.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<(String, String, Option<String>, i64)> = sqlx::query_as(
        r#"SELECT addon, config_json, secrets_enc, want_install
           FROM addon_presets WHERE cluster_name = ?"#,
    )
    .bind(from)
    .fetch_all(state.pool())
    .await?;
    let now = db::now_rfc3339();
    let mut restored = Vec::new();
    for (addon, config_json, secrets_enc, want_install) in rows {
        if config_is_empty(&config_json) || !should_restore_addon(&addon, cni) {
            continue;
        }
        sqlx::query(
            r#"INSERT INTO cluster_addons
                 (cluster_id, addon, status, config_json, secrets_enc, error, installed_at, updated_at, want_install)
               VALUES (?, ?, 'not_installed', ?, ?, NULL, NULL, ?, ?)
               ON CONFLICT(cluster_id, addon) DO UPDATE SET
                 config_json = excluded.config_json,
                 secrets_enc = excluded.secrets_enc,
                 want_install = excluded.want_install,
                 error = NULL,
                 updated_at = excluded.updated_at"#,
        )
        .bind(cluster_id)
        .bind(&addon)
        .bind(&config_json)
        .bind(secrets_enc.as_deref())
        .bind(&now)
        .bind(want_install)
        .execute(state.pool())
        .await?;
        restored.push(addon);
    }
    if !restored.is_empty() {
        let _ = snapshot_cluster(state, cluster_id).await;
    }
    Ok(restored)
}

/// After create succeeds, queue install jobs for restored add-ons.
pub async fn enqueue_restored_installs(
    state: &AppState,
    cluster_id: &str,
) -> anyhow::Result<Vec<String>> {
    let cni: String =
        sqlx::query_scalar("SELECT COALESCE(cni, 'cilium') FROM clusters WHERE id = ?")
            .bind(cluster_id)
            .fetch_optional(state.pool())
            .await?
            .unwrap_or_else(|| "cilium".into());
    let addons: Vec<(String, i64)> = sqlx::query_as(
        "SELECT addon, COALESCE(want_install, 0) FROM cluster_addons WHERE cluster_id = ?",
    )
    .bind(cluster_id)
    .fetch_all(state.pool())
    .await?;
    let mut queued = Vec::new();
    for (addon, want) in addons {
        if want == 0 || !should_restore_addon(&addon, &cni) {
            continue;
        }
        let existing: Option<String> = sqlx::query_scalar(
            r#"SELECT id FROM jobs
               WHERE cluster_id = ?
                 AND kind = 'install_addon'
                 AND status IN ('queued', 'running')
                 AND json_extract(payload_json, '$.addon') = ?
               LIMIT 1"#,
        )
        .bind(cluster_id)
        .bind(&addon)
        .fetch_optional(state.pool())
        .await?;
        if existing.is_some() {
            continue;
        }
        let now = db::now_rfc3339();
        sqlx::query(
            "UPDATE cluster_addons SET status = 'installing', want_install = 0, error = NULL, updated_at = ? \
             WHERE cluster_id = ? AND addon = ?",
        )
        .bind(&now)
        .bind(cluster_id)
        .bind(&addon)
        .execute(state.pool())
        .await?;
        crate::jobs::enqueue(
            state,
            Some(cluster_id),
            "install_addon",
            json!({ "addon": addon, "reused": true }),
        )
        .await?;
        queued.push(addon);
    }
    Ok(queued)
}

async fn installing_job(
    state: &AppState,
    cluster_id: &str,
    addon: &str,
) -> ApiResult<Option<String>> {
    let id: Option<String> = sqlx::query_scalar(
        r#"SELECT id FROM jobs
           WHERE cluster_id = ?
             AND kind = 'install_addon'
             AND status IN ('queued', 'running')
             AND json_extract(payload_json, '$.addon') = ?
           ORDER BY created_at DESC LIMIT 1"#,
    )
    .bind(cluster_id)
    .bind(addon)
    .fetch_optional(state.pool())
    .await?;
    Ok(id)
}

fn merge_status(
    stored: Option<&str>,
    live_installed: bool,
    live_partial: bool,
    installing: bool,
) -> String {
    if installing {
        return "installing".into();
    }
    match stored {
        Some("error") if !live_installed => "error".into(),
        Some("installing") => "installing".into(),
        _ if live_installed => "installed".into(),
        _ if live_partial => "partial".into(),
        Some("installed") => "missing".into(),
        Some(other) if !other.is_empty() => other.to_string(),
        _ => "not_installed".into(),
    }
}

pub async fn summarize_one(
    state: &AppState,
    cluster_id: &str,
    addon: &str,
    submitted: Option<Value>,
    probe_live: bool,
) -> ApiResult<Value> {
    let entry = catalog_entry(addon)?;
    let (cluster_cni, network_mode, cluster_arch) = cluster_net(state, cluster_id).await?;
    let row = load_row(state, cluster_id, addon).await?;
    let stored: Value = row
        .as_ref()
        .and_then(|r| serde_json::from_str(&r.config_json).ok())
        .unwrap_or_else(|| json!({}));
    let token_set = row
        .as_ref()
        .map(|r| r.secrets_enc.as_ref().is_some_and(|s| !s.is_empty()))
        .unwrap_or(false);
    let ingress_secrets = if addon == INGRESS_ID {
        decrypt_ingress_secrets(state, row.as_ref().and_then(|r| r.secrets_enc.as_deref()))
    } else {
        IngressSecrets::default()
    };
    let registry_set = !ingress_secrets.registry_password.trim().is_empty();
    let admin_secret_set = !ingress_secrets.admin_password.trim().is_empty();

    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut public_config = stored.clone();

    if let Some(body) = submitted.as_ref() {
        match addon {
            NFS_ID => {
                let cfg = NfsConfig {
                    server: body
                        .get("server")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    path: body
                        .get("path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                };
                if let Err(e) = validate_nfs(&cfg) {
                    errors.extend(e);
                } else {
                    match probe_tcp(cfg.server.trim(), 2049).await {
                        Ok(()) => {}
                        Err(e) => warnings.push(format!(
                            "NFS port 2049 on {} is not reachable from the management host ({e})",
                            cfg.server.trim()
                        )),
                    }
                }
                public_config = public_nfs_config(&cfg);
            }
            CERT_MANAGER_ID => {
                let mut cfg = parse_cert_stored(body);
                cfg.api_token = body
                    .get("api_token")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let has_new_token = !cfg.api_token.trim().is_empty();
                if let Err(e) = validate_cert_manager(&cfg, has_new_token || !token_set) {
                    errors.extend(e);
                }
                public_config = public_cert_config(&cfg);
            }
            CILIUM_LB_ID => {
                if let Err(e) = require_cilium_cni(&cluster_cni) {
                    errors.push(e.to_string());
                }
                let cfg = parse_cilium_lb_stored(body);
                if let Err(e) = validate_cilium_lb(&cfg, &network_mode) {
                    errors.extend(e);
                }
                public_config = public_cilium_lb_config(&cfg);
            }
            INGRESS_ID => {
                let mut cfg = parse_ingress_stored(body);
                cfg.admin_password = body
                    .get("admin_password")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                cfg.registry_password = body
                    .get("registry_password")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let stored_secrets = decrypt_ingress_secrets(
                    state,
                    row.as_ref().and_then(|r| r.secrets_enc.as_deref()),
                );
                let need_pw = !cfg.registry_user.trim().is_empty()
                    && cfg.registry_password.trim().is_empty()
                    && stored_secrets.registry_password.trim().is_empty();
                if let Err(e) = validate_ingress(&cfg, need_pw) {
                    errors.extend(e);
                }
                public_config = public_ingress_config(&cfg);
            }
            KOS_SCALER_ID => {
                let mut cfg = parse_kos_scaler_stored(body);
                cfg.password = json_str(body, "password");
                let need_pw = cfg.password.trim().is_empty() && !token_set;
                if let Err(e) = validate_kos_scaler(&cfg, need_pw) {
                    errors.extend(e);
                }
                let endpoint = kos_scaler_endpoint(&cfg, &state.cfg().public_url);
                if endpoint.is_empty() || crate::config::public_url_host_unusable(&endpoint) {
                    warnings.push(format!(
                                "mgmt URL {endpoint:?} is not reachable from cluster nodes; set Mgmt URL override or MGMT_PUBLIC_URL"
                            ));
                }
                public_config = public_kos_scaler_config(&cfg);
            }
            KUBERNETES_DASHBOARD_ID => {
                let mut cfg = parse_kubernetes_dashboard_stored(body);
                cfg.password = json_str(body, "password");
                if let Err(e) = validate_kubernetes_dashboard(&cfg, !token_set) {
                    errors.extend(e);
                }
                public_config = public_kubernetes_dashboard_config(&cfg);
            }
            _ => {}
        }
    } else if addon == NFS_ID
        && stored
            .get("server")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty())
    {
        let cfg = parse_nfs_stored(&stored);
        if let Err(e) = validate_nfs(&cfg) {
            errors.extend(e);
        }
    }

    if addon == INGRESS_ID
        && public_config
            .get("image_tag")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .is_empty()
    {
        public_config = public_ingress_config(&parse_ingress_stored(&public_config));
    }
    if addon == KOS_SCALER_ID {
        public_config = public_kos_scaler_config(&parse_kos_scaler_stored(&public_config));
        if submitted.is_none() {
            let endpoint = kos_scaler_endpoint(
                &parse_kos_scaler_stored(&public_config),
                &state.cfg().public_url,
            );
            if endpoint.is_empty() || crate::config::public_url_host_unusable(&endpoint) {
                warnings.push(format!(
                    "mgmt URL {endpoint:?} is not reachable from cluster nodes; set Mgmt URL override or MGMT_PUBLIC_URL"
                ));
            }
        }
    }
    if addon == KUBERNETES_DASHBOARD_ID {
        public_config =
            public_kubernetes_dashboard_config(&parse_kubernetes_dashboard_stored(&public_config));
    }

    let mut live = json!({ "available": false });
    if probe_live {
        match resolve_ready_kubeconfig(state, cluster_id).await {
            Ok((kc, _)) => {
                live = match addon {
                    NFS_ID => live_nfs(&kc).await,
                    CERT_MANAGER_ID => {
                        let issuer = public_config
                            .get("issuer")
                            .and_then(|v| v.as_str())
                            .unwrap_or("letsencrypt-cloudflare");
                        let domain = public_config
                            .get("domain")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        live_cert_manager(&kc, issuer, domain).await
                    }
                    CILIUM_LB_ID => live_cilium_lb(&kc).await,
                    INGRESS_ID => live_ingress(&kc).await,
                    KOS_SCALER_ID => live_kos_scaler(&kc).await,
                    KUBERNETES_DASHBOARD_ID => {
                        let cfg = parse_kubernetes_dashboard_stored(&public_config);
                        live_kubernetes_dashboard(&kc, cfg.namespace.trim()).await
                    }
                    _ => json!({}),
                };
                live["available"] = json!(true);
            }
            Err(e) => {
                live = json!({ "available": false, "error": e.to_string() });
            }
        }
    }

    let installing = installing_job(state, cluster_id, addon).await?;
    let live_installed = live
        .get("installed")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let live_partial = live
        .get("partial")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let status = merge_status(
        row.as_ref().map(|r| r.status.as_str()),
        live_installed,
        live_partial,
        installing.is_some(),
    );

    if addon == NFS_ID {
        if let (Some(live_server), Some(want)) = (
            live.get("server").and_then(|v| v.as_str()),
            public_config.get("server").and_then(|v| v.as_str()),
        ) {
            if !live_server.is_empty() && !want.is_empty() && live_server != want {
                warnings.push(format!(
                    "cluster provisioner uses NFS server {live_server}, form has {want}"
                ));
            }
        }
        if let (Some(live_path), Some(want)) = (
            live.get("path").and_then(|v| v.as_str()),
            public_config.get("path").and_then(|v| v.as_str()),
        ) {
            if !live_path.is_empty() && !want.is_empty() && live_path != want {
                warnings.push(format!(
                    "cluster provisioner uses export {live_path}, form has {want}"
                ));
            }
        }
        if live.get("available") == Some(&json!(true)) && live_installed {
            if !live
                .get("provisioner_ready")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                warnings.push("NFS provisioner is installed but not ready".into());
            }
            if !live
                .get("nfs_modules")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                warnings.push(
                    "NFS kernel-module DaemonSet is not present (guest image may already load nfs)"
                        .into(),
                );
            }
        }
    }
    if addon == CERT_MANAGER_ID && live.get("available") == Some(&json!(true)) && live_installed {
        if !live
            .get("webhook_ready")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            warnings.push("cert-manager webhook is not ready".into());
        }
        if !live
            .get("issuer")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            warnings.push("ClusterIssuer is not present yet".into());
        } else if !live
            .get("issuer_ready")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            warnings.push("ClusterIssuer is not Ready (check Cloudflare token / ACME)".into());
        }
        if !live
            .get("reflector")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            warnings.push(
                "kubernetes-reflector is not present; wildcard TLS secrets will not copy to other namespaces"
                    .into(),
            );
        }
        if !public_config
            .get("domain")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .is_empty()
        {
            if !live
                .get("certificate")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                warnings.push("wildcard Certificate is not present yet".into());
            } else if !live
                .get("certificate_ready")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                warnings.push(
                    "wildcard Certificate is not Ready (check DNS-01 / Cloudflare token)".into(),
                );
            }
        }
    }
    if addon == CILIUM_LB_ID {
        if cluster_cni != "cilium" {
            warnings.push("this add-on is only for clusters with CNI cilium".into());
        }
        if live.get("available") == Some(&json!(true))
            && live
                .get("partial")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        {
            if !live.get("pool").and_then(|v| v.as_bool()).unwrap_or(false) {
                warnings.push("CiliumLoadBalancerIPPool default-pool is not present".into());
            }
            if !live.get("l2").and_then(|v| v.as_bool()).unwrap_or(false) {
                warnings.push(
                    "CiliumL2AnnouncementPolicy is not present (enable l2announcements on Cilium)"
                        .into(),
                );
            }
        }
    }
    if addon == INGRESS_ID && live.get("available") == Some(&json!(true)) {
        if live_installed
            && !live
                .get("controller_ready")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        {
            warnings.push("Pertisk Ingress controller is installed but not ready".into());
        }
        if live_installed
            && live
                .get("lb_ipv4")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .is_empty()
            && live
                .get("lb_ipv6")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .is_empty()
        {
            warnings.push(
                "LoadBalancer has no address yet (install Cilium LoadBalancer or wait for an IP)"
                    .into(),
            );
        }
        if let Some(err) = live.get("pull_error").and_then(|v| v.as_str()) {
            if !err.is_empty() {
                warnings.push(format!("image pull failed ({err})"));
            }
        }
        if live.get("admin_tls").and_then(|v| v.as_bool()) == Some(true)
            && ingress_tls_secret(&parse_ingress_stored(&public_config)).is_empty()
        {
            warnings.push(
                "admin Ingress still has TLS; choose none or Update after clearing TLS secret"
                    .into(),
            );
        }
        if live.get("gateway_api").and_then(|v| v.as_bool()) != Some(true) && !live_installed {
            warnings.push(
                "Gateway API CRDs are not present; install will disable Gateway API reconciliation"
                    .into(),
            );
        }
    }
    if addon == KOS_SCALER_ID
        && live.get("available") == Some(&json!(true))
        && live_installed
        && !live.get("ready").and_then(|v| v.as_bool()).unwrap_or(false)
    {
        warnings.push("kos-scaler is installed but the deployment is not ready".into());
    }
    if addon == KUBERNETES_DASHBOARD_ID
        && live.get("available") == Some(&json!(true))
        && live_partial
        && (!live.get("ready").and_then(|v| v.as_bool()).unwrap_or(false)
            || !live
                .get("service")
                .and_then(|v| v.as_bool())
                .unwrap_or(false))
    {
        warnings.push("Kubernetes Dashboard is installed but not ready".into());
    }

    Ok(json!({
        "id": entry.id,
        "name": entry.name,
        "summary": entry.summary,
        "section": entry.section,
        "fields": catalog_fields_json(entry, &live, &public_config),
        "status": status,
        "ok": errors.is_empty(),
        "errors": errors,
        "warnings": warnings,
        "config": public_config,
        "token_set": if addon == INGRESS_ID { admin_secret_set } else { token_set },
        "registry_set": registry_set,
        "job_id": installing,
        "error": row.as_ref().and_then(|r| r.error.clone()),
        "installed_at": row.as_ref().and_then(|r| r.installed_at.clone()),
        "updated_at": row.as_ref().map(|r| r.updated_at.clone()),
        "live": live,
        "cluster_cni": cluster_cni,
        "network_mode": network_mode,
        "cluster_arch": cluster_arch,
        "cert_manager_version": if addon == CERT_MANAGER_ID { Some(CERT_MANAGER_VERSION) } else { None },
        "ingress_image": if addon == INGRESS_ID {
            public_config
                .get("image")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    Some(format!(
                        "{INGRESS_IMAGE_REGISTRY}/{INGRESS_IMAGE_REPO}:{INGRESS_IMAGE_TAG}"
                    ))
                })
        } else {
            None
        },
    }))
}

pub async fn list_addons(state: &AppState, cluster_id: &str) -> ApiResult<Vec<Value>> {
    let (cni, _, _) = cluster_net(state, cluster_id).await?;
    let mut out = Vec::new();
    for entry in catalog() {
        if let Some(need) = entry.requires_cni {
            if cni != need {
                continue;
            }
        }
        out.push(summarize_one(state, cluster_id, entry.id, None, true).await?);
    }
    Ok(out)
}

pub async fn upsert_install(
    state: &AppState,
    cluster_id: &str,
    addon: &str,
    body: Value,
) -> ApiResult<(NfsConfig, CertManagerConfig, Value)> {
    let now = db::now_rfc3339();
    let result = match addon {
        NFS_ID => {
            let cfg = NfsConfig {
                server: body
                    .get("server")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                path: body
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            };
            if let Err(e) = validate_nfs(&cfg) {
                return Err(AppError::bad(e.join("; ")));
            }
            let public = public_nfs_config(&cfg);
            sqlx::query(
                r#"INSERT INTO cluster_addons
                     (cluster_id, addon, status, config_json, secrets_enc, error, installed_at, updated_at)
                   VALUES (?, ?, 'installing', ?, NULL, NULL, NULL, ?)
                   ON CONFLICT(cluster_id, addon) DO UPDATE SET
                     status = 'installing',
                     config_json = excluded.config_json,
                     error = NULL,
                     updated_at = excluded.updated_at"#,
            )
            .bind(cluster_id)
            .bind(addon)
            .bind(public.to_string())
            .bind(&now)
            .execute(state.pool())
            .await?;
            Ok((cfg, CertManagerConfig::default(), public))
        }
        CERT_MANAGER_ID => {
            let row = load_row(state, cluster_id, addon).await?;
            let token_set = row
                .as_ref()
                .map(|r| r.secrets_enc.as_ref().is_some_and(|s| !s.is_empty()))
                .unwrap_or(false);
            let mut cfg = parse_cert_stored(&body);
            cfg.api_token = body
                .get("api_token")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let require_token = cfg.api_token.trim().is_empty() && !token_set;
            if let Err(e) =
                validate_cert_manager(&cfg, require_token || !cfg.api_token.trim().is_empty())
            {
                return Err(AppError::bad(e.join("; ")));
            }
            if cfg.api_token.trim().is_empty() && token_set {
                if let Some(enc) = row.and_then(|r| r.secrets_enc) {
                    cfg.api_token = crypto::decrypt(&state.cfg().secret_key, &enc)
                        .map_err(|e| AppError::bad(format!("stored token: {e}")))?;
                }
            }
            if cfg.api_token.trim().is_empty() {
                return Err(AppError::bad("Cloudflare API token is required"));
            }
            let public = public_cert_config(&cfg);
            let enc = crypto::encrypt(&state.cfg().secret_key, cfg.api_token.trim())
                .map_err(AppError::Anyhow)?;
            sqlx::query(
                r#"INSERT INTO cluster_addons
                     (cluster_id, addon, status, config_json, secrets_enc, error, installed_at, updated_at)
                   VALUES (?, ?, 'installing', ?, ?, NULL, NULL, ?)
                   ON CONFLICT(cluster_id, addon) DO UPDATE SET
                     status = 'installing',
                     config_json = excluded.config_json,
                     secrets_enc = excluded.secrets_enc,
                     error = NULL,
                     updated_at = excluded.updated_at"#,
            )
            .bind(cluster_id)
            .bind(addon)
            .bind(public.to_string())
            .bind(&enc)
            .bind(&now)
            .execute(state.pool())
            .await?;
            Ok((NfsConfig::default(), cfg, public))
        }
        CILIUM_LB_ID => {
            let (cni, mode, _) = cluster_net(state, cluster_id).await?;
            require_cilium_cni(&cni)?;
            let cfg = parse_cilium_lb_stored(&body);
            if let Err(e) = validate_cilium_lb(&cfg, &mode) {
                return Err(AppError::bad(e.join("; ")));
            }
            let public = public_cilium_lb_config(&cfg);
            sqlx::query(
                r#"INSERT INTO cluster_addons
                     (cluster_id, addon, status, config_json, secrets_enc, error, installed_at, updated_at)
                   VALUES (?, ?, 'installing', ?, NULL, NULL, NULL, ?)
                   ON CONFLICT(cluster_id, addon) DO UPDATE SET
                     status = 'installing',
                     config_json = excluded.config_json,
                     error = NULL,
                     updated_at = excluded.updated_at"#,
            )
            .bind(cluster_id)
            .bind(addon)
            .bind(public.to_string())
            .bind(&now)
            .execute(state.pool())
            .await?;
            Ok((NfsConfig::default(), CertManagerConfig::default(), public))
        }
        INGRESS_ID => {
            let row = load_row(state, cluster_id, addon).await?;
            let mut cfg = parse_ingress_stored(&body);
            cfg.admin_password = body
                .get("admin_password")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            cfg.registry_password = body
                .get("registry_password")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let mut secrets =
                decrypt_ingress_secrets(state, row.as_ref().and_then(|r| r.secrets_enc.as_deref()));
            let need_pw = !cfg.registry_user.trim().is_empty()
                && cfg.registry_password.trim().is_empty()
                && secrets.registry_password.trim().is_empty();
            if let Err(e) = validate_ingress(&cfg, need_pw) {
                return Err(AppError::bad(e.join("; ")));
            }
            if !cfg.admin_password.trim().is_empty() {
                secrets.admin_password = cfg.admin_password.trim().to_string();
            }
            if !cfg.registry_password.trim().is_empty() {
                secrets.registry_password = cfg.registry_password.trim().to_string();
            }
            let public = public_ingress_config(&cfg);
            let enc = if secrets.admin_password.trim().is_empty()
                && secrets.registry_password.trim().is_empty()
            {
                None
            } else {
                Some(
                    crypto::encrypt(&state.cfg().secret_key, &encode_ingress_secrets(&secrets))
                        .map_err(AppError::Anyhow)?,
                )
            };
            sqlx::query(
                r#"INSERT INTO cluster_addons
                     (cluster_id, addon, status, config_json, secrets_enc, error, installed_at, updated_at)
                   VALUES (?, ?, 'installing', ?, ?, NULL, NULL, ?)
                   ON CONFLICT(cluster_id, addon) DO UPDATE SET
                     status = 'installing',
                     config_json = excluded.config_json,
                     secrets_enc = excluded.secrets_enc,
                     error = NULL,
                     updated_at = excluded.updated_at"#,
            )
            .bind(cluster_id)
            .bind(addon)
            .bind(public.to_string())
            .bind(enc.as_deref())
            .bind(&now)
            .execute(state.pool())
            .await?;
            Ok((NfsConfig::default(), CertManagerConfig::default(), public))
        }
        KOS_SCALER_ID => {
            let row = load_row(state, cluster_id, addon).await?;
            let mut cfg = parse_kos_scaler_stored(&body);
            cfg.password = json_str(&body, "password");
            let token_set = row
                .as_ref()
                .map(|r| r.secrets_enc.as_ref().is_some_and(|s| !s.is_empty()))
                .unwrap_or(false);
            if let Err(e) = validate_kos_scaler(&cfg, cfg.password.trim().is_empty() && !token_set)
            {
                return Err(AppError::bad(e.join("; ")));
            }
            let password = if cfg.password.trim().is_empty() && token_set {
                if let Some(enc) = row.and_then(|r| r.secrets_enc) {
                    crypto::decrypt(&state.cfg().secret_key, &enc)
                        .map_err(|e| AppError::bad(format!("stored password: {e}")))?
                } else {
                    String::new()
                }
            } else {
                cfg.password.trim().to_string()
            };
            if password.is_empty() {
                return Err(AppError::bad("mgmt password is required"));
            }
            let public = public_kos_scaler_config(&cfg);
            let enc =
                crypto::encrypt(&state.cfg().secret_key, &password).map_err(AppError::Anyhow)?;
            sqlx::query(
                r#"INSERT INTO cluster_addons
                     (cluster_id, addon, status, config_json, secrets_enc, error, installed_at, updated_at)
                   VALUES (?, ?, 'installing', ?, ?, NULL, NULL, ?)
                   ON CONFLICT(cluster_id, addon) DO UPDATE SET
                     status = 'installing',
                     config_json = excluded.config_json,
                     secrets_enc = excluded.secrets_enc,
                     error = NULL,
                     updated_at = excluded.updated_at"#,
            )
            .bind(cluster_id)
            .bind(addon)
            .bind(public.to_string())
            .bind(&enc)
            .bind(&now)
            .execute(state.pool())
            .await?;
            Ok((NfsConfig::default(), CertManagerConfig::default(), public))
        }
        KUBERNETES_DASHBOARD_ID => {
            let row = load_row(state, cluster_id, addon).await?;
            let mut secrets = row
                .as_ref()
                .and_then(|r| r.secrets_enc.as_deref())
                .map(|enc| crypto::decrypt(&state.cfg().secret_key, enc))
                .transpose()
                .map_err(|e| AppError::bad(format!("stored dashboard secrets: {e}")))?
                .map(|raw| parse_kubernetes_dashboard_secrets(&raw))
                .unwrap_or_default();
            let mut cfg = parse_kubernetes_dashboard_stored(&body);
            cfg.password = json_str(&body, "password");
            if let Err(e) = validate_kubernetes_dashboard(
                &cfg,
                cfg.password.trim().is_empty() && secrets.password.trim().is_empty(),
            ) {
                return Err(AppError::bad(e.join("; ")));
            }
            let password = if cfg.password.trim().is_empty() {
                secrets.password.clone()
            } else {
                cfg.password.trim().to_string()
            };
            if secrets.jwt_secret.trim().is_empty() {
                secrets.jwt_secret = generate_dashboard_jwt_secret();
            }
            secrets.password = password;
            let public = public_kubernetes_dashboard_config(&cfg);
            let enc = crypto::encrypt(
                &state.cfg().secret_key,
                &encode_kubernetes_dashboard_secrets(&secrets),
            )
            .map_err(AppError::Anyhow)?;
            sqlx::query(
                r#"INSERT INTO cluster_addons
                     (cluster_id, addon, status, config_json, secrets_enc, error, installed_at, updated_at)
                   VALUES (?, ?, 'installing', ?, ?, NULL, NULL, ?)
                   ON CONFLICT(cluster_id, addon) DO UPDATE SET
                     status = 'installing',
                     config_json = excluded.config_json,
                     secrets_enc = excluded.secrets_enc,
                     error = NULL,
                     updated_at = excluded.updated_at"#,
            )
            .bind(cluster_id)
            .bind(addon)
            .bind(public.to_string())
            .bind(&enc)
            .bind(&now)
            .execute(state.pool())
            .await?;
            Ok((NfsConfig::default(), CertManagerConfig::default(), public))
        }
        other => Err(AppError::bad(format!("unknown addon {other}"))),
    }?;
    remember_addon(state, cluster_id, addon).await?;
    Ok(result)
}

async fn mark_addon(
    state: &AppState,
    cluster_id: &str,
    addon: &str,
    status: &str,
    error: Option<&str>,
) -> anyhow::Result<()> {
    let now = db::now_rfc3339();
    if status == "installed" {
        sqlx::query(
            "UPDATE cluster_addons SET status = ?, error = NULL, installed_at = COALESCE(installed_at, ?), updated_at = ? \
             WHERE cluster_id = ? AND addon = ?",
        )
        .bind(status)
        .bind(&now)
        .bind(&now)
        .bind(cluster_id)
        .bind(addon)
        .execute(state.pool())
        .await?;
    } else {
        sqlx::query(
            "UPDATE cluster_addons SET status = ?, error = ?, updated_at = ? WHERE cluster_id = ? AND addon = ?",
        )
        .bind(status)
        .bind(error)
        .bind(&now)
        .bind(cluster_id)
        .bind(addon)
        .execute(state.pool())
        .await?;
    }
    Ok(())
}

fn anyhow_api(err: AppError) -> anyhow::Error {
    anyhow::anyhow!("{err}")
}

/// Background job: apply addon manifests with the cluster kubeconfig.
pub async fn run_install_job(
    state: &AppState,
    cluster_id: Option<&str>,
    payload: &str,
    log_path: &str,
) -> anyhow::Result<()> {
    let cid = cluster_id.ok_or_else(|| anyhow::anyhow!("cluster_id required"))?;
    let p: Value = serde_json::from_str(payload)?;
    let addon = p
        .get("addon")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("payload.addon required"))?;
    crate::jobs::append_log(log_path, &format!("install addon {addon}\n"))?;

    let (kc, name) = resolve_ready_kubeconfig(state, cid)
        .await
        .map_err(anyhow_api)?;
    crate::jobs::append_log(
        log_path,
        &format!("cluster {name} kubeconfig {}\n", kc.display()),
    )?;

    let row = load_row(state, cid, addon)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .ok_or_else(|| anyhow::anyhow!("addon {addon} config missing"))?;
    let stored: Value = serde_json::from_str(&row.config_json).unwrap_or_else(|_| json!({}));

    let result = match addon {
        NFS_ID => install_nfs(state, &kc, log_path, &stored).await,
        CERT_MANAGER_ID => {
            let token = match row.secrets_enc.as_deref() {
                Some(enc) if !enc.is_empty() => crypto::decrypt(&state.cfg().secret_key, enc)?,
                _ => anyhow::bail!("Cloudflare API token is not stored"),
            };
            install_cert_manager(&kc, log_path, &stored, &token).await
        }
        CILIUM_LB_ID => install_cilium_lb(state, cid, &kc, log_path, &stored).await,
        INGRESS_ID => {
            let secrets = match row.secrets_enc.as_deref() {
                Some(enc) if !enc.is_empty() => {
                    parse_ingress_secrets(&crypto::decrypt(&state.cfg().secret_key, enc)?)
                }
                _ => IngressSecrets::default(),
            };
            install_ingress(state, cid, &kc, log_path, &stored, secrets).await
        }
        KOS_SCALER_ID => {
            let password = match row.secrets_enc.as_deref() {
                Some(enc) if !enc.is_empty() => crypto::decrypt(&state.cfg().secret_key, enc)?,
                _ => anyhow::bail!("mgmt password is not stored"),
            };
            install_kos_scaler(state, cid, &kc, log_path, &stored, &password).await
        }
        KUBERNETES_DASHBOARD_ID => {
            let secrets = match row.secrets_enc.as_deref() {
                Some(enc) if !enc.is_empty() => parse_kubernetes_dashboard_secrets(
                    &crypto::decrypt(&state.cfg().secret_key, enc)?,
                ),
                _ => anyhow::bail!("dashboard password is not stored"),
            };
            if secrets.password.trim().is_empty() || secrets.jwt_secret.trim().is_empty() {
                anyhow::bail!(
                    "dashboard credentials or JWT secret are not stored; update the add-on"
                )
            }
            install_kubernetes_dashboard(state, cid, &kc, log_path, &stored, &secrets).await
        }
        other => anyhow::bail!("unknown addon {other}"),
    };

    match result {
        Ok(()) => {
            mark_addon(state, cid, addon, "installed", None).await?;
            crate::jobs::append_log(log_path, "addon install complete\n")?;
            Ok(())
        }
        Err(e) => {
            let msg = e.to_string();
            let _ = mark_addon(state, cid, addon, "error", Some(&msg)).await;
            Err(e)
        }
    }
}

async fn install_kubernetes_dashboard(
    state: &AppState,
    cluster_id: &str,
    kc: &Path,
    log_path: &str,
    stored: &Value,
    secrets: &KubernetesDashboardSecrets,
) -> anyhow::Result<()> {
    let mut cfg = parse_kubernetes_dashboard_stored(stored);
    cfg.password = secrets.password.clone();
    validate_kubernetes_dashboard(&cfg, true).map_err(|e| anyhow::anyhow!(e.join("; ")))?;
    let values = kubernetes_dashboard_helm_values(&cfg, &secrets.jwt_secret);
    let mut logged = values.clone();
    logged["app"]["auth"]["password"] = json!("***");
    logged["app"]["auth"]["jwtSecret"] = json!("***");
    let values_path = state
        .cfg()
        .jobs_dir()
        .join(format!("{cluster_id}-kubernetes-dashboard-values.yaml"));
    write_restricted_file(&values_path, &serde_json::to_string_pretty(&values)?)?;
    let _cleanup = UnlinkOnDrop(values_path.clone());
    let values_s = values_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("dashboard values path is not utf-8"))?;
    crate::jobs::append_log(
        log_path,
        &format!(
            "helm values:\n{}\n",
            serde_json::to_string_pretty(&logged).unwrap_or_default()
        ),
    )?;
    crate::jobs::append_log(
        log_path,
        &format!(
            "helm upgrade --install {KUBERNETES_DASHBOARD_RELEASE} {KUBERNETES_DASHBOARD_HELM_CHART} --repo {INGRESS_HELM_REPO} -n {}\n",
            cfg.namespace.trim()
        ),
    )?;
    let helm_args = vec![
        "upgrade".to_string(),
        "--install".to_string(),
        KUBERNETES_DASHBOARD_RELEASE.to_string(),
        KUBERNETES_DASHBOARD_HELM_CHART.to_string(),
        "--repo".to_string(),
        INGRESS_HELM_REPO.to_string(),
        "--namespace".to_string(),
        cfg.namespace.trim().to_string(),
        "--create-namespace".to_string(),
        "--timeout".to_string(),
        "5m".to_string(),
        "-f".to_string(),
        values_s.to_string(),
    ];
    let helm_refs: Vec<&str> = helm_args.iter().map(String::as_str).collect();
    let out = helm_output(Some(kc), &helm_refs)
        .await
        .map_err(anyhow_api)?;
    crate::jobs::append_log(log_path, &out)?;
    crate::jobs::append_log(log_path, "wait for Kubernetes Dashboard deployment\n")?;
    kubectl_ok(
        kc,
        &[
            "wait",
            "--for=condition=Available",
            &format!("deploy/{KUBERNETES_DASHBOARD_DEPLOY}"),
            "-n",
            cfg.namespace.trim(),
            "--timeout=180s",
        ],
    )
    .await
    .map_err(anyhow_api)?;
    Ok(())
}

async fn install_cilium_lb(
    state: &AppState,
    cluster_id: &str,
    kc: &Path,
    log_path: &str,
    stored: &Value,
) -> anyhow::Result<()> {
    let (cni, mode, _) = cluster_net(state, cluster_id)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    require_cilium_cni(&cni).map_err(|e| anyhow::anyhow!("{e}"))?;
    let cfg = parse_cilium_lb_stored(stored);
    validate_cilium_lb(&cfg, &mode).map_err(|e| anyhow::anyhow!("{}", e.join("; ")))?;
    let pool_api = cilium_crd_api_version(
        kc,
        "ciliumloadbalancerippools.cilium.io",
        "cilium.io/v2alpha1",
    )
    .await;
    let l2_api = cilium_crd_api_version(
        kc,
        "ciliuml2announcementpolicies.cilium.io",
        "cilium.io/v2alpha1",
    )
    .await;
    crate::jobs::append_log(
        log_path,
        &format!(
            "Cilium LoadBalancer pool ipv4={} ipv6={} pool_api={pool_api} l2_api={l2_api}\n",
            cfg.ipv4.trim(),
            if cfg.ipv6.trim().is_empty() {
                "—"
            } else {
                cfg.ipv6.trim()
            }
        ),
    )?;
    crate::jobs::append_log(log_path, "apply CiliumLoadBalancerIPPool\n")?;
    let out = kubectl_apply_yaml(kc, &render_cilium_lb_pool_yaml(&cfg, &pool_api))
        .await
        .map_err(anyhow_api)?;
    crate::jobs::append_log(log_path, &out)?;
    crate::jobs::append_log(log_path, "apply CiliumL2AnnouncementPolicy\n")?;
    let out = kubectl_apply_yaml(kc, &render_cilium_l2_yaml(&l2_api))
        .await
        .map_err(anyhow_api)?;
    crate::jobs::append_log(log_path, &out)?;
    Ok(())
}

struct UnlinkOnDrop(PathBuf);

impl Drop for UnlinkOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn write_restricted_file(path: &Path, contents: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(path, perms)?;
    }
    Ok(())
}

async fn install_kos_scaler(
    state: &AppState,
    cluster_id: &str,
    kc: &Path,
    log_path: &str,
    stored: &Value,
    password: &str,
) -> anyhow::Result<()> {
    let cfg = parse_kos_scaler_stored(stored);
    validate_kos_scaler(&cfg, false).map_err(|e| anyhow::anyhow!("{}", e.join("; ")))?;
    let endpoint = kos_scaler_endpoint(&cfg, &state.cfg().public_url);
    if endpoint.is_empty() || crate::config::public_url_host_unusable(&endpoint) {
        anyhow::bail!(
            "mgmt URL {endpoint:?} is not reachable from cluster nodes; set Mgmt URL override or MGMT_PUBLIC_URL"
        );
    }
    let values = kos_scaler_helm_values(&cfg, cluster_id, &endpoint, password);
    let mut logged = values.clone();
    if logged.pointer_mut("/mgmt/password").is_some() {
        logged["mgmt"]["password"] = json!("***");
    }
    crate::jobs::append_log(
        log_path,
        &format!(
            "kos-scaler chart {KOS_SCALER_HELM_CHART} tag={} endpoint={endpoint} cluster={cluster_id} workers={}..{}\n",
            if cfg.image_tag.trim().is_empty() {
                KOS_SCALER_IMAGE_TAG
            } else {
                cfg.image_tag.trim()
            },
            if cfg.min_size > 0 { cfg.min_size } else { 2 },
            if cfg.max_size > 0 { cfg.max_size } else { 10 },
        ),
    )?;

    let values_path = state
        .cfg()
        .jobs_dir()
        .join(format!("{cluster_id}-kos-scaler-values.yaml"));
    write_restricted_file(&values_path, &serde_json::to_string_pretty(&values)?)?;
    let _cleanup = UnlinkOnDrop(values_path.clone());
    crate::jobs::append_log(
        log_path,
        &format!(
            "helm values:\n{}\n",
            serde_json::to_string_pretty(&logged).unwrap_or_default()
        ),
    )?;
    let values_s = values_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("kos-scaler values path is not utf-8"))?;
    crate::jobs::append_log(
        log_path,
        &format!(
            "helm upgrade --install {KOS_SCALER_RELEASE} {KOS_SCALER_HELM_CHART} --repo {INGRESS_HELM_REPO} -n {KOS_SCALER_NAMESPACE}\n"
        ),
    )?;
    let helm_args = [
        "upgrade",
        "--install",
        KOS_SCALER_RELEASE,
        KOS_SCALER_HELM_CHART,
        "--repo",
        INGRESS_HELM_REPO,
        "--namespace",
        KOS_SCALER_NAMESPACE,
        "--create-namespace",
        "--timeout",
        "5m",
        "-f",
        values_s,
    ];
    let out = helm_output(Some(kc), &helm_args)
        .await
        .map_err(anyhow_api)?;
    crate::jobs::append_log(log_path, &out)?;
    crate::jobs::append_log(log_path, "wait for kos-scaler deployment\n")?;
    kubectl_ok(
        kc,
        &[
            "wait",
            "--for=condition=Available",
            &format!("deploy/{KOS_SCALER_DEPLOY}"),
            "-n",
            KOS_SCALER_NAMESPACE,
            "--timeout=180s",
        ],
    )
    .await
    .map_err(anyhow_api)?;
    Ok(())
}

async fn install_ingress(
    state: &AppState,
    cluster_id: &str,
    kc: &Path,
    log_path: &str,
    stored: &Value,
    secrets: IngressSecrets,
) -> anyhow::Result<()> {
    let (_, mode, arch) = cluster_net(state, cluster_id)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let cfg = parse_ingress_stored(stored);
    validate_ingress(&cfg, false).map_err(|e| anyhow::anyhow!("{}", e.join("; ")))?;
    let gateway_api = gateway_api_available(kc).await;
    let use_pull_secret =
        !cfg.registry_user.trim().is_empty() && !secrets.registry_password.trim().is_empty();
    let resolved_tag = resolve_ingress_image_tag(
        &cfg.image_tag,
        &arch,
        &cfg.registry_user,
        secrets.registry_password.trim(),
    )
    .await?;
    let admin_pw = secrets.admin_password.trim();
    let values = ingress_helm_values(
        &cfg,
        &mode,
        gateway_api,
        if admin_pw.is_empty() {
            None
        } else {
            Some(admin_pw)
        },
        use_pull_secret,
        &resolved_tag,
        &arch,
    );
    let mut logged = values.clone();
    if logged.get("auth").and_then(|a| a.get("password")).is_some() {
        logged["auth"]["password"] = json!("***");
    }

    crate::jobs::append_log(
        log_path,
        &format!(
            "pertisk-ingress {INGRESS_IMAGE_REGISTRY}/{INGRESS_IMAGE_REPO}:{} (pinned {resolved_tag}) arch={arch} network_mode={mode} gateway_api={gateway_api} pull_secret={} registry_user={}\n",
            cfg.image_tag.trim(),
            if use_pull_secret { INGRESS_PULL_SECRET } else { "none (public Harbor)" },
            if cfg.registry_user.trim().is_empty() { "anonymous" } else { cfg.registry_user.trim() }
        ),
    )?;

    crate::jobs::append_log(log_path, "apply namespace pertisk-proxy\n")?;
    let ns = json!({
        "apiVersion": "v1",
        "kind": "Namespace",
        "metadata": { "name": INGRESS_NAMESPACE },
    });
    let out = kubectl_apply_yaml(kc, &ns.to_string())
        .await
        .map_err(anyhow_api)?;
    crate::jobs::append_log(log_path, &out)?;

    if use_pull_secret {
        crate::jobs::append_log(
            log_path,
            &format!("apply imagePullSecret {INGRESS_PULL_SECRET}\n"),
        )?;
        let secret = harbor_pull_secret_doc(&cfg.registry_user, secrets.registry_password.trim());
        let out = kubectl_apply_yaml(kc, &secret.to_string())
            .await
            .map_err(anyhow_api)?;
        crate::jobs::append_log(log_path, &out)?;
    } else {
        crate::jobs::append_log(
            log_path,
            "Harbor project is public; skipping imagePullSecret\n",
        )?;
    }
    let values_path = state
        .cfg()
        .jobs_dir()
        .join(format!("{cluster_id}-ingress-values.yaml"));
    write_restricted_file(&values_path, &serde_json::to_string_pretty(&values)?)?;
    let _cleanup = UnlinkOnDrop(values_path.clone());
    crate::jobs::append_log(
        log_path,
        &format!(
            "helm values:\n{}\n",
            serde_json::to_string_pretty(&logged).unwrap_or_default()
        ),
    )?;

    let values_s = values_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("ingress values path is not utf-8"))?;
    // Pin --repo so a misconfigured local helm repo alias (e.g. "pertisk" → Bitnami)
    // cannot steal chart resolution away from chart.tools.pertisk.com.
    let chart_ver = ingress_chart_version(&cfg.image_tag);
    crate::jobs::append_log(
        log_path,
        &format!(
            "helm upgrade --install {INGRESS_RELEASE} {INGRESS_HELM_CHART} --repo {INGRESS_HELM_REPO}{}\n",
            chart_ver
                .as_deref()
                .map(|v| format!(" --version {v}"))
                .unwrap_or_default()
        ),
    )?;
    let host = cfg.admin_host.trim();
    let mut helm_args: Vec<String> = vec![
        "upgrade".into(),
        "--install".into(),
        INGRESS_RELEASE.into(),
        INGRESS_HELM_CHART.into(),
        "--repo".into(),
        INGRESS_HELM_REPO.into(),
        "--namespace".into(),
        INGRESS_NAMESPACE.into(),
        "--create-namespace".into(),
        "--timeout".into(),
        "5m".into(),
        "-f".into(),
        values_s.to_string(),
    ];
    if let Some(ver) = chart_ver {
        helm_args.extend(["--version".into(), ver]);
    }
    if !host.is_empty() {
        let tls = ingress_tls_secret(&cfg);
        helm_args.extend([
            "--set".into(),
            "adminIngress.enabled=true".into(),
            "--set-string".into(),
            format!("adminIngress.host={host}"),
            "--set-string".into(),
            format!("adminIngress.tlsSecretName={tls}"),
        ]);
    } else {
        helm_args.extend(["--set".into(), "adminIngress.enabled=false".into()]);
    }
    let helm_refs: Vec<&str> = helm_args.iter().map(|s| s.as_str()).collect();
    let out = helm_output(Some(kc), &helm_refs)
        .await
        .map_err(anyhow_api)?;
    crate::jobs::append_log(log_path, &out)?;

    crate::jobs::append_log(log_path, "wait for pertisk-proxy-ingress deployment\n")?;
    kubectl_ok(
        kc,
        &[
            "wait",
            "--for=condition=Available",
            "deploy/pertisk-proxy-ingress",
            "-n",
            INGRESS_NAMESPACE,
            "--timeout=180s",
        ],
    )
    .await
    .map_err(anyhow_api)?;

    if !host.is_empty() {
        let tls = ingress_tls_secret(&cfg);
        crate::jobs::append_log(
            log_path,
            &format!(
                "apply admin Ingress {INGRESS_ADMIN} host={host} tls={}\n",
                if tls.is_empty() { "none" } else { tls.as_str() }
            ),
        )?;
        apply_admin_ingress(kc, log_path, host, &tls).await?;
    }
    Ok(())
}

async fn apply_admin_ingress(
    kc: &Path,
    log_path: &str,
    host: &str,
    tls_secret: &str,
) -> anyhow::Result<()> {
    let doc = admin_ingress_doc(host, tls_secret);
    let out = kubectl_apply_yaml(kc, &doc.to_string())
        .await
        .map_err(anyhow_api)?;
    crate::jobs::append_log(log_path, &out)?;
    if tls_secret.trim().is_empty() {
        // Helm 3-way merge can leave spec.tls from a previous release; strip it.
        match kubectl_ok(
            kc,
            &[
                "patch",
                "ingress",
                INGRESS_ADMIN,
                "-n",
                INGRESS_NAMESPACE,
                "--type=json",
                "-p",
                r#"[{"op":"remove","path":"/spec/tls"}]"#,
            ],
        )
        .await
        {
            Ok(()) => crate::jobs::append_log(log_path, "removed spec.tls from admin Ingress\n")?,
            Err(_) => crate::jobs::append_log(log_path, "admin Ingress has no spec.tls\n")?,
        }
    }
    Ok(())
}

async fn install_nfs(
    _state: &AppState,
    kc: &Path,
    log_path: &str,
    stored: &Value,
) -> anyhow::Result<()> {
    let cfg = parse_nfs_stored(stored);
    validate_nfs(&cfg).map_err(|e| anyhow::anyhow!("{}", e.join("; ")))?;
    crate::jobs::append_log(
        log_path,
        &format!(
            "NFS server={} path={}\n",
            cfg.server.trim(),
            cfg.path.trim()
        ),
    )?;

    crate::jobs::append_log(log_path, "apply nfs kernel-module DaemonSet\n")?;
    let out = kubectl_apply_yaml(kc, NFS_MODULES_YAML)
        .await
        .map_err(anyhow_api)?;
    crate::jobs::append_log(log_path, &out)?;

    crate::jobs::append_log(log_path, "apply nfs-subdir-external-provisioner\n")?;
    let yaml = render_nfs_provisioner(&cfg);
    let out = kubectl_apply_yaml(kc, &yaml).await.map_err(anyhow_api)?;
    crate::jobs::append_log(log_path, &out)?;
    Ok(())
}

async fn install_cert_manager(
    kc: &Path,
    log_path: &str,
    stored: &Value,
    token: &str,
) -> anyhow::Result<()> {
    let mut cfg = parse_cert_stored(stored);
    cfg.api_token = token.to_string();
    validate_cert_manager(&cfg, true).map_err(|e| anyhow::anyhow!("{}", e.join("; ")))?;

    crate::jobs::append_log(
        log_path,
        &format!(
            "cert-manager {CERT_MANAGER_VERSION} provider={} acme={}\n",
            cfg.provider.trim(),
            cfg.acme.trim()
        ),
    )?;
    crate::jobs::append_log(log_path, &format!("apply {CERT_MANAGER_MANIFEST_URL}\n"))?;
    let out = kubectl_apply_url(kc, CERT_MANAGER_MANIFEST_URL)
        .await
        .map_err(anyhow_api)?;
    crate::jobs::append_log(log_path, &out)?;

    crate::jobs::append_log(
        log_path,
        &format!(
            "patch cert-manager-webhook hostNetwork (port {WEBHOOK_HOST_PORT}) so host-networked apiserver can reach it\n"
        ),
    )?;
    patch_cert_manager_webhook_hostnet(kc, log_path).await?;

    crate::jobs::append_log(log_path, "wait for cert-manager deployments\n")?;
    kubectl_ok(
        kc,
        &[
            "wait",
            "--for=condition=Available",
            "deploy/cert-manager",
            "deploy/cert-manager-cainjector",
            "deploy/cert-manager-webhook",
            "-n",
            "cert-manager",
            "--timeout=180s",
        ],
    )
    .await
    .map_err(anyhow_api)?;

    wait_webhook_endpoints(kc, log_path).await?;

    crate::jobs::append_log(log_path, "apply Cloudflare API token Secret\n")?;
    let secret = cloudflare_token_secret_json(token);
    let out = kubectl_apply_yaml(kc, &secret.to_string())
        .await
        .map_err(anyhow_api)?;
    crate::jobs::append_log(log_path, &out)?;

    crate::jobs::append_log(log_path, "apply ClusterIssuer\n")?;
    let issuer = cert_manager_issuer_yaml(&cfg);
    apply_yaml_retry(kc, log_path, &issuer, 12, Duration::from_secs(5)).await?;

    crate::jobs::append_log(
        log_path,
        &format!("apply kubernetes-reflector {REFLECTOR_MANIFEST_URL}\n"),
    )?;
    match kubectl_apply_url(kc, REFLECTOR_MANIFEST_URL).await {
        Ok(out) => crate::jobs::append_log(log_path, &out)?,
        Err(e) => crate::jobs::append_log(
            log_path,
            &format!(
                "reflector apply failed ({e}); wildcard secrets may not copy across namespaces\n"
            ),
        )?,
    }
    wait_reflector(kc, log_path).await?;

    if let Some(cert) = wildcard_certificate_doc(&cfg) {
        let name = cert
            .pointer("/metadata/name")
            .and_then(|v| v.as_str())
            .unwrap_or("wildcard-tls");
        crate::jobs::append_log(
            log_path,
            &format!(
                "apply Certificate {name} dnsNames={:?} (reflect to all namespaces)\n",
                wildcard_dns_names(&cfg.domain)
            ),
        )?;
        apply_yaml_retry(kc, log_path, &cert.to_string(), 8, Duration::from_secs(5)).await?;
        crate::jobs::append_log(
            log_path,
            &format!(
                "Certificate {name} applied; ACME DNS-01 issuance continues in the background\n"
            ),
        )?;
        crate::jobs::append_log(
            log_path,
            &format!(
                "check later: kubectl -n {CERT_NS} get certificate {name} -o wide (and `describe` / challenges on failure)\n"
            ),
        )?;
    }
    Ok(())
}

async fn wait_reflector(kc: &Path, log_path: &str) -> anyhow::Result<()> {
    crate::jobs::append_log(log_path, "wait for reflector deployment\n")?;
    for ns in ["reflector", "kube-system"] {
        if kubectl_ok(
            kc,
            &[
                "wait",
                "--for=condition=Available",
                "deploy/reflector",
                "-n",
                ns,
                "--timeout=90s",
            ],
        )
        .await
        .is_ok()
        {
            crate::jobs::append_log(log_path, &format!("reflector Available in {ns}\n"))?;
            return Ok(());
        }
    }
    crate::jobs::append_log(
        log_path,
        "reflector deployment not ready (wildcard copy to other namespaces may lag)\n",
    )?;
    Ok(())
}

async fn patch_cert_manager_webhook_hostnet(kc: &Path, log_path: &str) -> anyhow::Result<()> {
    let mut deploy = kubectl_json(
        kc,
        &[
            "get",
            "deploy",
            "cert-manager-webhook",
            "-n",
            "cert-manager",
            "-o",
            "json",
        ],
    )
    .await
    .map_err(anyhow_api)?;
    patch_webhook_host_network(&mut deploy, WEBHOOK_HOST_PORT);
    let out = kubectl_apply_yaml(kc, &deploy.to_string())
        .await
        .map_err(anyhow_api)?;
    crate::jobs::append_log(log_path, &out)?;

    let mut svc = kubectl_json(
        kc,
        &[
            "get",
            "svc",
            "cert-manager-webhook",
            "-n",
            "cert-manager",
            "-o",
            "json",
        ],
    )
    .await
    .map_err(anyhow_api)?;
    patch_webhook_service_target_port(&mut svc, WEBHOOK_HOST_PORT);
    let out = kubectl_apply_yaml(kc, &svc.to_string())
        .await
        .map_err(anyhow_api)?;
    crate::jobs::append_log(log_path, &out)?;

    kubectl_ok(
        kc,
        &[
            "rollout",
            "status",
            "deploy/cert-manager-webhook",
            "-n",
            "cert-manager",
            "--timeout=180s",
        ],
    )
    .await
    .map_err(anyhow_api)?;
    Ok(())
}

async fn wait_webhook_endpoints(kc: &Path, log_path: &str) -> anyhow::Result<()> {
    for i in 1..=36 {
        let ep = kubectl_json_optional(
            kc,
            &[
                "get",
                "endpoints",
                "cert-manager-webhook",
                "-n",
                "cert-manager",
                "-o",
                "json",
            ],
        )
        .await
        .map_err(anyhow_api)?;
        if ep.as_ref().is_some_and(endpoints_have_addresses) {
            crate::jobs::append_log(log_path, "webhook endpoints ready\n")?;
            return Ok(());
        }
        crate::jobs::append_log(
            log_path,
            &format!("waiting for webhook endpoints ({i}/36)\n"),
        )?;
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    anyhow::bail!("cert-manager-webhook has no endpoints")
}

async fn apply_yaml_retry(
    kc: &Path,
    log_path: &str,
    yaml: &str,
    attempts: u32,
    delay: Duration,
) -> anyhow::Result<()> {
    let mut last = String::new();
    for i in 1..=attempts {
        match kubectl_apply_yaml(kc, yaml).await {
            Ok(out) => {
                crate::jobs::append_log(log_path, &out)?;
                return Ok(());
            }
            Err(e) => {
                last = e.to_string();
                crate::jobs::append_log(
                    log_path,
                    &format!("ClusterIssuer apply {i}/{attempts}: {last}\n"),
                )?;
                if !webhook_dial_error(&last) || i == attempts {
                    break;
                }
                tokio::time::sleep(delay).await;
            }
        }
    }
    anyhow::bail!("{last}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nfs_validates_ip_and_path() {
        assert!(validate_nfs(&NfsConfig {
            server: "10.1.1.150".into(),
            path: "/mnt/nfs_share".into(),
        })
        .is_ok());
        assert!(validate_nfs(&NfsConfig {
            server: "nas.lab.local".into(),
            path: "/exports/k8s".into(),
        })
        .is_ok());
        assert!(validate_nfs(&NfsConfig {
            server: "".into(),
            path: "/mnt".into(),
        })
        .is_err());
        assert!(validate_nfs(&NfsConfig {
            server: "10.1.1.150".into(),
            path: "mnt".into(),
        })
        .is_err());
        assert!(validate_nfs(&NfsConfig {
            server: "10.1.1.150/24".into(),
            path: "/mnt".into(),
        })
        .is_err());
        assert!(validate_nfs(&NfsConfig {
            server: "10.1.1.150".into(),
            path: "/mnt; rm -rf /".into(),
        })
        .is_err());
    }

    #[test]
    fn cert_manager_requires_cloudflare_email_token() {
        let ok = CertManagerConfig {
            provider: "cloudflare".into(),
            email: "ops@example.com".into(),
            api_token: "cf-token-123456".into(),
            acme: "staging".into(),
            issuer: String::new(),
            domain: String::new(),
        };
        assert!(validate_cert_manager(&ok, true).is_ok());
        let mut bad = ok.clone();
        bad.provider = "route53".into();
        assert!(validate_cert_manager(&bad, true).is_err());
        bad = ok.clone();
        bad.email = "not-an-email".into();
        assert!(validate_cert_manager(&bad, true).is_err());
        bad = ok;
        bad.api_token.clear();
        assert!(validate_cert_manager(&bad, true).is_err());
        assert!(validate_cert_manager(&bad, false).is_ok());
    }

    #[test]
    fn nfs_yaml_substitutes_server_and_path() {
        let yaml = render_nfs_provisioner(&NfsConfig {
            server: "10.1.1.150".into(),
            path: "/mnt/nfs_share".into(),
        });
        assert!(
            yaml.contains("value: \"10.1.1.150\"")
                || yaml.contains("value: 10.1.1.150")
                || yaml.contains("10.1.1.150")
        );
        assert!(yaml.contains("/mnt/nfs_share"));
        assert!(!yaml.contains("${NFS_SERVER}"));
        assert!(!yaml.contains("${NFS_PATH}"));
    }

    #[test]
    fn cert_issuer_yaml_uses_staging_when_requested() {
        let yaml = cert_manager_issuer_yaml(&CertManagerConfig {
            provider: "cloudflare".into(),
            email: "ops@example.com".into(),
            api_token: "x".into(),
            acme: "staging".into(),
            issuer: "letsencrypt-cloudflare".into(),
            domain: String::new(),
        });
        assert!(yaml.contains("acme-staging-v02"));
        assert!(yaml.contains("ops@example.com"));
        assert!(yaml.contains("cloudflare-api-token-secret"));
    }

    #[test]
    fn public_cert_config_redacts_token() {
        let v = public_cert_config(&CertManagerConfig {
            provider: "cloudflare".into(),
            email: "ops@example.com".into(),
            api_token: "super-secret".into(),
            acme: "production".into(),
            issuer: String::new(),
            domain: "vsphere.pertisk.com".into(),
        });
        assert!(v.get("api_token").is_none());
        assert_eq!(v["email"], "ops@example.com");
        assert_eq!(v["issuer"], "letsencrypt-cloudflare");
        assert_eq!(v["domain"], "vsphere.pertisk.com");
        assert_eq!(v["tls_secret"], "vsphere-pertisk-com-tls");
    }

    #[test]
    fn catalog_has_nfs_and_cert_manager() {
        let ids: Vec<_> = catalog().iter().map(|e| e.id).collect();
        assert_eq!(
            ids,
            [
                "nfs",
                "cert-manager",
                "cilium-lb",
                "ingress",
                "kos-scaler",
                "kubernetes-dashboard",
            ]
        );
        assert_eq!(catalog()[1].section, "certificates");
        assert_eq!(catalog()[2].requires_cni, Some("cilium"));
        assert_eq!(catalog()[3].id, "ingress");
        assert_eq!(catalog()[3].section, "ingress");
        assert_eq!(catalog()[5].section, "dashboard");
    }

    #[test]
    fn restore_skips_cilium_lb_when_cni_is_not_cilium() {
        assert!(should_restore_addon("nfs", "flannel"));
        assert!(should_restore_addon("cilium-lb", "cilium"));
        assert!(!should_restore_addon("cilium-lb", "flannel"));
        assert!(should_restore_addon("ingress", "calico"));
    }

    #[test]
    fn empty_addon_config_json_is_skipped() {
        assert!(config_is_empty(""));
        assert!(config_is_empty("{}"));
        assert!(config_is_empty("  null  "));
        assert!(!config_is_empty(r#"{"server":"10.1.1.150"}"#));
    }

    #[test]
    fn cilium_lb_normalizes_ip_and_cidr() {
        assert_eq!(
            normalize_lb_cidr("10.1.1.50", IpFamily::V4).unwrap(),
            "10.1.1.50/32"
        );
        assert_eq!(
            normalize_lb_cidr("10.1.1.50/32", IpFamily::V4).unwrap(),
            "10.1.1.50/32"
        );
        assert_eq!(
            normalize_lb_cidr("2a01:4f9:c013:14bc::1", IpFamily::V6).unwrap(),
            "2a01:4f9:c013:14bc::1/128"
        );
        assert!(normalize_lb_cidr("10.1.1.50", IpFamily::V6).is_err());
        assert!(validate_cilium_lb(
            &CiliumLbConfig {
                ipv4: "10.1.1.50".into(),
                ipv6: String::new(),
            },
            "ipv4"
        )
        .is_ok());
        assert!(validate_cilium_lb(
            &CiliumLbConfig {
                ipv4: "10.1.1.50".into(),
                ipv6: String::new(),
            },
            "dual-stack"
        )
        .is_err());
        assert!(validate_cilium_lb(
            &CiliumLbConfig {
                ipv4: "10.1.1.50".into(),
                ipv6: "2001:db8::1/128".into(),
            },
            "dual-stack"
        )
        .is_ok());
    }

    #[test]
    fn cilium_lb_yaml_has_pool_and_l2() {
        let pool = render_cilium_lb_pool_yaml(
            &CiliumLbConfig {
                ipv4: "65.108.209.120".into(),
                ipv6: "2a01:4f9:c013:14bc::1".into(),
            },
            "cilium.io/v2",
        );
        let l2 = render_cilium_l2_yaml("cilium.io/v2alpha1");
        assert!(pool.contains("kind: CiliumLoadBalancerIPPool"));
        assert!(pool.contains("apiVersion: cilium.io/v2"));
        assert!(pool.contains("65.108.209.120/32"));
        assert!(pool.contains("2a01:4f9:c013:14bc::1/128"));
        assert!(l2.contains("kind: CiliumL2AnnouncementPolicy"));
        assert!(l2.contains("apiVersion: cilium.io/v2alpha1"));
        assert!(l2.contains("loadBalancerIPs: true"));
        assert!(!l2.contains("cilium.io/v2\n"));
    }

    #[test]
    fn crd_storage_api_uses_storage_version() {
        let pool = json!({
            "spec": { "versions": [
                { "name": "v2alpha1", "storage": false },
                { "name": "v2", "storage": true }
            ]}
        });
        assert_eq!(crd_storage_api(&pool, "cilium.io/v2alpha1"), "cilium.io/v2");
        let l2 = json!({
            "spec": { "versions": [
                { "name": "v2alpha1", "storage": true }
            ]}
        });
        assert_eq!(
            crd_storage_api(&l2, "cilium.io/v2alpha1"),
            "cilium.io/v2alpha1"
        );
    }

    #[test]
    fn webhook_host_network_rewrites_secure_port() {
        let mut deploy = json!({
            "apiVersion": "apps/v1",
            "kind": "Deployment",
            "metadata": { "name": "cert-manager-webhook", "managedFields": [] },
            "spec": {
                "template": {
                    "spec": {
                        "containers": [{
                            "name": "cert-manager-webhook",
                            "args": ["--v=2", "--secure-port=10250"],
                            "ports": [{ "name": "https", "containerPort": 10250 }]
                        }]
                    }
                }
            },
            "status": { "readyReplicas": 1 }
        });
        patch_webhook_host_network(&mut deploy, 10260);
        assert_eq!(deploy["spec"]["template"]["spec"]["hostNetwork"], true);
        assert_eq!(
            deploy["spec"]["template"]["spec"]["dnsPolicy"],
            "ClusterFirstWithHostNet"
        );
        assert_eq!(
            deploy["spec"]["template"]["spec"]["containers"][0]["args"][1],
            "--secure-port=10260"
        );
        assert_eq!(
            deploy["spec"]["template"]["spec"]["containers"][0]["ports"][0]["containerPort"],
            10260
        );
        assert!(deploy.get("status").is_none());
    }

    #[test]
    fn webhook_service_numeric_target_port_is_updated() {
        let mut svc = json!({
            "spec": { "ports": [{ "name": "https", "port": 443, "targetPort": 10250 }] }
        });
        patch_webhook_service_target_port(&mut svc, 10260);
        assert_eq!(svc["spec"]["ports"][0]["targetPort"], 10260);

        let mut named = json!({
            "spec": { "ports": [{ "name": "https", "port": 443, "targetPort": "https" }] }
        });
        patch_webhook_service_target_port(&mut named, 10260);
        assert_eq!(named["spec"]["ports"][0]["targetPort"], "https");
    }

    #[test]
    fn webhook_dial_error_matches_no_route_to_host() {
        assert!(webhook_dial_error(
            r#"failed calling webhook "webhook.cert-manager.io": dial tcp 10.111.71.127:443: connect: no route to host"#
        ));
        assert!(!webhook_dial_error("ClusterIssuer.cert-manager.io created"));
    }

    #[test]
    fn public_kos_scaler_omits_password_and_defaults() {
        let v = public_kos_scaler_config(&KosScalerConfig {
            username: "admin".into(),
            password: "secret".into(),
            min_size: 0,
            max_size: 0,
            image_tag: String::new(),
            storage_class: String::new(),
            mgmt_url: String::new(),
        });
        assert!(v.get("password").is_none());
        assert_eq!(v["username"], "admin");
        assert_eq!(v["min_size"], 2);
        assert_eq!(v["max_size"], 10);
        assert_eq!(v["image_tag"], KOS_SCALER_IMAGE_TAG);
        assert_eq!(v["storage_class"], "nfs-client");
    }

    #[test]
    fn validate_kos_scaler_requires_user_and_max() {
        let err = validate_kos_scaler(&KosScalerConfig::default(), true).unwrap_err();
        assert!(err.iter().any(|e| e.contains("username")));
        assert!(err.iter().any(|e| e.contains("password")));
        let ok = KosScalerConfig {
            username: "ops".into(),
            password: "pw".into(),
            min_size: 2,
            max_size: 8,
            ..KosScalerConfig::default()
        };
        assert!(validate_kos_scaler(&ok, true).is_ok());
        let bad = KosScalerConfig {
            username: "ops".into(),
            min_size: 5,
            max_size: 2,
            ..KosScalerConfig::default()
        };
        assert!(validate_kos_scaler(&bad, false).is_err());
    }

    #[test]
    fn kos_scaler_helm_values_wire_mgmt() {
        let cfg = KosScalerConfig {
            username: "admin".into(),
            min_size: 3,
            max_size: 12,
            image_tag: "0.1.0".into(),
            storage_class: "nfs-client".into(),
            ..KosScalerConfig::default()
        };
        let v = kos_scaler_helm_values(&cfg, "cid", "https://ptkos.example", "s3cret");
        assert_eq!(v["mgmt"]["clusterId"], "cid");
        assert_eq!(v["mgmt"]["endpoint"], "https://ptkos.example");
        assert_eq!(v["mgmt"]["username"], "admin");
        assert_eq!(v["mgmt"]["password"], "s3cret");
        assert_eq!(v["config"]["workerPool"]["minSize"], 3);
        assert_eq!(v["config"]["workerPool"]["maxSize"], 12);
        assert_eq!(v["statePersistence"]["enabled"], true);
        assert_eq!(v["statePersistence"]["storageClassName"], "nfs-client");
    }

    #[test]
    fn public_ingress_redacts_password_and_defaults_tag() {
        let v = public_ingress_config(&IngressConfig {
            image_tag: String::new(),
            admin_host: "admin.example.com".into(),
            tls_secret: "none".into(),
            admin_password: "super-secret".into(),
            registry_user: String::new(),
            registry_password: "harbor-secret".into(),
        });
        assert!(v.get("admin_password").is_none());
        assert_eq!(v["image_tag"], INGRESS_IMAGE_TAG);
        assert_eq!(
            v["image"],
            format!("{INGRESS_IMAGE_REGISTRY}/{INGRESS_IMAGE_REPO}:{INGRESS_IMAGE_TAG}")
        );
        assert_eq!(v["admin_host"], "admin.example.com");
        assert_eq!(v["registry_user"], "");
    }

    #[test]
    fn ingress_values_follow_network_mode() {
        let cfg = IngressConfig {
            image_tag: "v0.1.83".into(),
            admin_host: String::new(),
            tls_secret: String::new(),
            admin_password: String::new(),
            registry_user: "robot$pertisk-proxy+pull".into(),
            registry_password: String::new(),
        };
        let v4 = ingress_helm_values(&cfg, "ipv4", false, None, true, "v0.1.83-amd64", "amd64");
        assert_eq!(v4["image"]["registry"], INGRESS_IMAGE_REGISTRY);
        assert_eq!(v4["image"]["repository"], INGRESS_IMAGE_REPO);
        assert_eq!(v4["image"]["tag"], "v0.1.83-amd64");
        assert_eq!(v4["nodeSelector"]["kubernetes.io/arch"], "amd64");
        assert_eq!(v4["service"]["ipFamilyPolicy"], "SingleStack");
        assert_eq!(v4["service"]["ipFamilies"][0], "IPv4");
        assert_eq!(v4["gatewayApi"]["enabled"], false);
        assert_eq!(v4["adminIngress"]["enabled"], false);
        assert_eq!(v4["imagePullSecrets"][0]["name"], "pertisk-ingress-harbor");
        assert!(v4.get("auth").is_none());

        let dual = ingress_helm_values(
            &IngressConfig {
                image_tag: "v0.1.83".into(),
                admin_host: "admin.example.com".into(),
                tls_secret: "vsphere-pertisk-com-tls".into(),
                admin_password: String::new(),
                registry_user: "robot$pertisk-proxy+pull".into(),
                registry_password: String::new(),
            },
            "dual-stack",
            true,
            Some("s3cret"),
            true,
            "v0.1.83@sha256:abc",
            "arm64",
        );
        assert_eq!(dual["service"]["ipFamilyPolicy"], "PreferDualStack");
        assert_eq!(dual["service"]["ipFamilies"][1], "IPv6");
        assert_eq!(dual["gatewayApi"]["enabled"], true);
        assert_eq!(dual["adminIngress"]["host"], "admin.example.com");
        assert_eq!(dual["adminIngress"]["enabled"], true);
        assert_eq!(
            dual["adminIngress"]["tlsSecretName"],
            "vsphere-pertisk-com-tls"
        );
        assert_eq!(dual["auth"]["password"], "s3cret");
        assert_eq!(dual["image"]["tag"], "v0.1.83@sha256:abc");
        assert_eq!(dual["nodeSelector"]["kubernetes.io/arch"], "arm64");
    }

    #[test]
    fn validate_ingress_rejects_bad_tag() {
        let ok = IngressConfig {
            image_tag: "v0.1.83".into(),
            registry_user: "robot$pertisk-proxy+pull".into(),
            registry_password: "s3cret".into(),
            ..Default::default()
        };
        assert!(validate_ingress(&ok, true).is_ok());
        assert!(validate_ingress(
            &IngressConfig {
                image_tag: "v0.1.83;rm -rf /".into(),
                registry_user: "robot$pertisk-proxy+pull".into(),
                registry_password: "s3cret".into(),
                ..Default::default()
            },
            true
        )
        .is_err());
        assert!(validate_ingress(
            &IngressConfig {
                image_tag: "v0.1.83".into(),
                ..Default::default()
            },
            false
        )
        .is_ok());
        assert!(validate_ingress(
            &IngressConfig {
                image_tag: "v0.1.83".into(),
                registry_user: "robot$pertisk-proxy+pull".into(),
                ..Default::default()
            },
            true
        )
        .is_err());
    }

    #[test]
    fn harbor_pull_secret_is_dockerconfigjson() {
        let doc = harbor_pull_secret_doc("robot$pertisk-proxy+pull", "s3cret");
        assert_eq!(doc["kind"], "Secret");
        assert_eq!(doc["type"], "kubernetes.io/dockerconfigjson");
        let raw = doc["stringData"][".dockerconfigjson"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(raw).unwrap();
        assert_eq!(
            parsed["auths"][INGRESS_IMAGE_REGISTRY]["username"],
            "robot$pertisk-proxy+pull"
        );
        assert!(
            parsed["auths"][INGRESS_IMAGE_REGISTRY]["auth"]
                .as_str()
                .unwrap()
                .len()
                > 8
        );
    }

    #[test]
    fn parse_ingress_secrets_accepts_legacy_plaintext() {
        let s = parse_ingress_secrets("old-admin-password");
        assert_eq!(s.admin_password, "old-admin-password");
        assert!(s.registry_password.is_empty());
        let s = parse_ingress_secrets(r#"{"admin_password":"a","registry_password":"b"}"#);
        assert_eq!(s.admin_password, "a");
        assert_eq!(s.registry_password, "b");
    }

    #[test]
    fn ingress_pin_tag_appends_cluster_arch() {
        assert_eq!(ingress_pin_tag("v0.1.83", "arm64"), "v0.1.83-arm64");
        assert_eq!(ingress_pin_tag("v0.1.83", "aarch64"), "v0.1.83-arm64");
        assert_eq!(ingress_pin_tag("v0.1.83", "amd64"), "v0.1.83-amd64");
        assert_eq!(ingress_pin_tag("v0.1.83-arm64", "amd64"), "v0.1.83-arm64");
        assert_eq!(
            ingress_pin_tag("v0.1.83@sha256:abc", "arm64"),
            "v0.1.83@sha256:abc"
        );
    }

    #[test]
    fn ingress_chart_version_from_image_tag() {
        assert_eq!(ingress_chart_version("v0.1.85").as_deref(), Some("0.1.85"));
        assert_eq!(
            ingress_chart_version("v0.1.85-arm64").as_deref(),
            Some("0.1.85")
        );
        assert_eq!(
            ingress_chart_version("v0.1.85@sha256:abc").as_deref(),
            Some("0.1.85")
        );
        assert_eq!(
            ingress_chart_version("").as_deref(),
            Some(INGRESS_IMAGE_TAG.trim_start_matches('v'))
        );
        assert!(ingress_chart_version("latest").is_none());
    }

    #[test]
    fn pick_platform_digest_skips_attestations() {
        let index = json!({
            "manifests": [
                {
                    "mediaType": "application/vnd.oci.image.manifest.v1+json",
                    "digest": "sha256:amd",
                    "platform": { "architecture": "amd64", "os": "linux" }
                },
                {
                    "mediaType": "application/vnd.oci.image.manifest.v1+json",
                    "digest": "sha256:arm",
                    "platform": { "architecture": "arm64", "os": "linux" }
                },
                {
                    "mediaType": "application/vnd.oci.image.manifest.v1+json",
                    "digest": "sha256:att",
                    "platform": { "architecture": "unknown", "os": "unknown" }
                }
            ]
        });
        assert_eq!(
            pick_platform_digest(&index, "arm64").as_deref(),
            Some("sha256:arm")
        );
        assert_eq!(
            pick_platform_digest(&index, "amd64").as_deref(),
            Some("sha256:amd")
        );
        assert!(pick_platform_digest(&index, "s390x").is_none());
    }

    #[test]
    fn admin_ingress_is_http_only_without_tls_secret() {
        let doc = admin_ingress_doc("admin.vsphere.pertisk.com", "");
        assert_eq!(doc["kind"], "Ingress");
        assert_eq!(doc["metadata"]["name"], "pertisk-proxy-ingress-admin");
        assert_eq!(doc["spec"]["rules"][0]["host"], "admin.vsphere.pertisk.com");
        assert_eq!(
            doc["spec"]["rules"][0]["http"]["paths"][0]["backend"]["service"]["port"]["number"],
            9080
        );
        assert_eq!(doc["spec"]["ingressClassName"], "pertisk-proxy");
        assert_eq!(
            doc["metadata"]["annotations"]["proxy.pertisk.tech/security-exempt"],
            "true"
        );
        assert!(doc["spec"].get("tls").is_none());
    }

    #[test]
    fn admin_ingress_attaches_selected_tls_secret() {
        let doc = admin_ingress_doc("admin.vsphere.pertisk.com", "vsphere-pertisk-com-tls");
        assert_eq!(
            doc["spec"]["tls"][0]["secretName"],
            "vsphere-pertisk-com-tls"
        );
        assert_eq!(
            doc["spec"]["tls"][0]["hosts"][0],
            "admin.vsphere.pertisk.com"
        );
    }

    #[test]
    fn dashboard_config_requires_host_for_tls_and_normalizes_none() {
        let http = KubernetesDashboardConfig {
            namespace: KUBERNETES_DASHBOARD_NAMESPACE.into(),
            image_tag: "v0.2.6".into(),
            username: "admin".into(),
            password: "dashboard-password".into(),
            host: "dashboard.example.com".into(),
            tls_secret: "none".into(),
        };
        assert!(validate_kubernetes_dashboard(&http, true).is_ok());
        let public = public_kubernetes_dashboard_config(&http);
        assert_eq!(public["host"], "dashboard.example.com");
        assert_eq!(public["tls_secret"], "");
        assert_eq!(public["namespace"], KUBERNETES_DASHBOARD_NAMESPACE);
        assert_eq!(public["image_tag"], "v0.2.6");
        assert_eq!(public["username"], "admin");
        assert!(public.get("password").is_none());
        assert!(validate_kubernetes_dashboard(
            &KubernetesDashboardConfig {
                host: String::new(),
                tls_secret: "dashboard-tls".into(),
                ..Default::default()
            },
            false
        )
        .is_err());
        assert!(
            validate_kubernetes_dashboard(&KubernetesDashboardConfig::default(), true).is_err()
        );
        assert_eq!(
            parse_kubernetes_dashboard_stored(&json!({})).namespace,
            KUBERNETES_DASHBOARD_NAMESPACE
        );
    }

    #[test]
    fn dashboard_helm_values_configure_image_login_and_ingress() {
        let values = kubernetes_dashboard_helm_values(
            &KubernetesDashboardConfig {
                namespace: KUBERNETES_DASHBOARD_NAMESPACE.into(),
                image_tag: "v0.2.7".into(),
                username: "operator".into(),
                password: "dashboard-password".into(),
                host: "dashboard.example.com".into(),
                tls_secret: "dashboard-tls".into(),
            },
            "jwt-secret",
        );
        assert_eq!(
            values["app"]["image"]["registry"],
            KUBERNETES_DASHBOARD_IMAGE_REGISTRY
        );
        assert_eq!(values["app"]["image"]["tag"], "v0.2.7");
        assert_eq!(values["app"]["auth"]["username"], "operator");
        assert_eq!(values["app"]["auth"]["password"], "dashboard-password");
        assert_eq!(values["app"]["auth"]["jwtSecret"], "jwt-secret");
        assert_eq!(
            values["ingress"]["hosts"][0]["host"],
            "dashboard.example.com"
        );
        assert_eq!(values["ingress"]["tls"][0]["secretName"], "dashboard-tls");
    }

    #[test]
    fn dashboard_secrets_migrate_password_only_data() {
        let legacy = parse_kubernetes_dashboard_secrets("dashboard-password");
        assert_eq!(legacy.password, "dashboard-password");
        assert!(legacy.jwt_secret.is_empty());
        let jwt_secret = generate_dashboard_jwt_secret();
        assert_eq!(jwt_secret.len(), 64);
        let encoded = encode_kubernetes_dashboard_secrets(&KubernetesDashboardSecrets {
            password: legacy.password,
            jwt_secret: jwt_secret.clone(),
        });
        let decoded = parse_kubernetes_dashboard_secrets(&encoded);
        assert_eq!(decoded.jwt_secret, jwt_secret);
    }

    #[test]
    fn wildcard_certificate_covers_apex_and_star() {
        assert_eq!(
            wildcard_dns_names("vsphere.pertisk.com"),
            ["*.vsphere.pertisk.com", "vsphere.pertisk.com"]
        );
        assert_eq!(
            wildcard_dns_names("*.vsphere.pertisk.com"),
            ["*.vsphere.pertisk.com", "vsphere.pertisk.com"]
        );
        assert_eq!(
            cert_secret_name("*.vsphere.pertisk.com"),
            "vsphere-pertisk-com-tls"
        );
        let doc = wildcard_certificate_doc(&CertManagerConfig {
            provider: "cloudflare".into(),
            email: "ops@example.com".into(),
            api_token: "x".into(),
            acme: "production".into(),
            issuer: "letsencrypt-cloudflare".into(),
            domain: "vsphere.pertisk.com".into(),
        })
        .expect("certificate");
        assert_eq!(doc["kind"], "Certificate");
        assert_eq!(doc["metadata"]["namespace"], "cert-manager");
        assert_eq!(doc["spec"]["secretName"], "vsphere-pertisk-com-tls");
        assert_eq!(doc["spec"]["dnsNames"][0], "*.vsphere.pertisk.com");
        assert_eq!(
            doc["spec"]["secretTemplate"]["annotations"]
                ["reflector.v1.k8s.emberstack.com/reflection-auto-enabled"],
            "true"
        );
        assert!(wildcard_certificate_doc(&CertManagerConfig::default()).is_none());
        let mut bad = CertManagerConfig {
            provider: "cloudflare".into(),
            email: "ops@example.com".into(),
            api_token: "cf-token-123456".into(),
            acme: "production".into(),
            domain: "not a domain".into(),
            ..Default::default()
        };
        assert!(validate_cert_manager(&bad, true).is_err());
        bad.domain = "vsphere.pertisk.com".into();
        assert!(validate_cert_manager(&bad, true).is_ok());
    }
}
