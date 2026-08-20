//! Optional cluster add-ons installed through the management UI (NFS, cert-manager).

use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::crypto;
use crate::db;
use crate::error::{ApiResult, AppError};
use crate::k8s::{
    kubectl_apply_url, kubectl_apply_yaml, kubectl_json, kubectl_json_optional, kubectl_ok,
    resolve_ready_kubeconfig,
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

pub fn catalog() -> &'static [AddonCatalogEntry] {
    &[
        AddonCatalogEntry {
            id: NFS_ID,
            name: "NFS storage",
            summary: "Dynamic ReadWriteMany volumes via an external NFS export and nfs-subdir-external-provisioner.",
            fields: NFS_FIELDS,
            requires_cni: None,
        },
        AddonCatalogEntry {
            id: CERT_MANAGER_ID,
            name: "cert-manager",
            summary: "TLS certificates with cert-manager and a Let’s Encrypt ClusterIssuer (Cloudflare DNS-01).",
            fields: CERT_MANAGER_FIELDS,
            requires_cni: None,
        },
        AddonCatalogEntry {
            id: CILIUM_LB_ID,
            name: "Cilium LoadBalancer",
            summary: "ELB IPs via CiliumLoadBalancerIPPool and L2 announcements (shown when CNI is Cilium).",
            fields: CILIUM_LB_FIELDS,
            requires_cni: Some("cilium"),
        },
    ]
}

pub fn catalog_entry(id: &str) -> ApiResult<&'static AddonCatalogEntry> {
    catalog()
        .iter()
        .find(|e| e.id == id)
        .ok_or_else(|| AppError::bad(format!("unknown addon {id}")))
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

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CiliumLbConfig {
    pub ipv4: String,
    #[serde(default)]
    pub ipv6: String,
}

pub fn parse_addon_id(raw: &str) -> ApiResult<String> {
    match raw.trim() {
        NFS_ID => Ok(NFS_ID.into()),
        CERT_MANAGER_ID => Ok(CERT_MANAGER_ID.into()),
        CILIUM_LB_ID => Ok(CILIUM_LB_ID.into()),
        other => Err(AppError::bad(format!("unknown addon {other}"))),
    }
}

async fn cluster_net(state: &AppState, cluster_id: &str) -> ApiResult<(String, String)> {
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT LOWER(cni), LOWER(COALESCE(network_mode, 'ipv4')) FROM clusters WHERE id = ?",
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

async fn live_cert_manager(kc: &Path, issuer: &str) -> Value {
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
    let (cluster_cni, network_mode) = cluster_net(state, cluster_id).await?;
    let row = load_row(state, cluster_id, addon).await?;
    let stored: Value = row
        .as_ref()
        .and_then(|r| serde_json::from_str(&r.config_json).ok())
        .unwrap_or_else(|| json!({}));
    let token_set = row
        .as_ref()
        .map(|r| r.secrets_enc.as_ref().is_some_and(|s| !s.is_empty()))
        .unwrap_or(false);

    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut public_config = stored.clone();

    if let Some(body) = submitted {
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
                let mut cfg = parse_cert_stored(&body);
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
                let cfg = parse_cilium_lb_stored(&body);
                if let Err(e) = validate_cilium_lb(&cfg, &network_mode) {
                    errors.extend(e);
                }
                public_config = public_cilium_lb_config(&cfg);
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
                        live_cert_manager(&kc, issuer).await
                    }
                    CILIUM_LB_ID => live_cilium_lb(&kc).await,
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

    Ok(json!({
        "id": entry.id,
        "name": entry.name,
        "summary": entry.summary,
        "fields": entry.fields,
        "status": status,
        "ok": errors.is_empty(),
        "errors": errors,
        "warnings": warnings,
        "config": public_config,
        "token_set": token_set,
        "job_id": installing,
        "error": row.as_ref().and_then(|r| r.error.clone()),
        "installed_at": row.as_ref().and_then(|r| r.installed_at.clone()),
        "updated_at": row.as_ref().map(|r| r.updated_at.clone()),
        "live": live,
        "cluster_cni": cluster_cni,
        "network_mode": network_mode,
        "cert_manager_version": if addon == CERT_MANAGER_ID { Some(CERT_MANAGER_VERSION) } else { None },
    }))
}

pub async fn list_addons(state: &AppState, cluster_id: &str) -> ApiResult<Vec<Value>> {
    let (cni, _) = cluster_net(state, cluster_id).await?;
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
            let (cni, mode) = cluster_net(state, cluster_id).await?;
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
        other => Err(AppError::bad(format!("unknown addon {other}"))),
    }
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

async fn install_cilium_lb(
    state: &AppState,
    cluster_id: &str,
    kc: &Path,
    log_path: &str,
    stored: &Value,
) -> anyhow::Result<()> {
    let (cni, mode) = cluster_net(state, cluster_id)
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
        });
        assert!(v.get("api_token").is_none());
        assert_eq!(v["email"], "ops@example.com");
        assert_eq!(v["issuer"], "letsencrypt-cloudflare");
    }

    #[test]
    fn catalog_has_nfs_and_cert_manager() {
        let ids: Vec<_> = catalog().iter().map(|e| e.id).collect();
        assert_eq!(ids, ["nfs", "cert-manager", "cilium-lb"]);
        assert_eq!(catalog()[2].requires_cni, Some("cilium"));
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
}
