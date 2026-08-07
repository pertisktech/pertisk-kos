//! Control-plane bootstrap for Pertisk KOS (Talos-shaped static pods).

mod addons;
mod api;
mod coredns;
mod gen;
mod join;
mod kube_vip;
mod kubeconfig;
mod manifests;
mod paths;
mod pki;
pub mod static_pods;
mod token;

use std::fs;
use std::net::UdpSocket;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use pertisk_config::{MachineConfig, MachineType};
use tracing::info;

pub use gen::{
    gen_config, gen_config_ha, gen_config_ha_with_network, gen_config_with_network,
    patch_controlplane_secrets, patch_worker_ca, write_gen_config, write_gen_config_ha,
    GenConfigHaOutput, GenConfigOutput, GenNetworkOpts,
};
pub use join::{get_join_config, join_control_plane, JoinConfigResult, JoinControlPlaneResult};
pub use kubeconfig::{rename_kubeconfig_context, sanitize_kubeconfig};
pub use paths::BootstrapPaths;
pub use token::generate_bootstrap_token;

pub const DEFAULT_K8S_VERSION: &str = "v1.36.3";
pub const DEFAULT_ETCD_IMAGE: &str = "registry.k8s.io/etcd:3.5.16-0";
pub const DEFAULT_POD_SUBNET: &str = "10.244.0.0/16";
pub const DEFAULT_SERVICE_SUBNET: &str = "10.96.0.0/12";

pub struct BootstrapResult {
    pub already_bootstrapped: bool,
    pub message: String,
    pub admin_kubeconfig: String,
    pub ca_pem: String,
}

/// Bootstrap the first control plane from applied machine config.
pub fn bootstrap_control_plane(
    state_root: &Path,
    cfg: &MachineConfig,
    advertise_address: Option<&str>,
) -> Result<BootstrapResult> {
    if cfg.machine.machine_type != MachineType::Controlplane {
        bail!("bootstrap requires machine.type: controlplane");
    }
    let cluster = cfg
        .cluster
        .as_ref()
        .context("cluster block required for bootstrap")?;

    let paths = BootstrapPaths::default_state(state_root);
    if paths.is_bootstrapped() {
        // Re-publish live paths (reboot / re-bootstrap repair).
        if let Err(err) = restore_control_plane(state_root) {
            tracing::warn!(error = %err, "restore after already-bootstrapped failed");
            repair_kubeconfig_files(&paths)?;
        }
        let admin = read_admin_kubeconfig(state_root)?;
        let ca = fs::read_to_string(paths.pki().join("ca.crt")).unwrap_or_default();
        return Ok(BootstrapResult {
            already_bootstrapped: true,
            message: "already bootstrapped".into(),
            admin_kubeconfig: admin,
            ca_pem: ca,
        });
    }

    paths.ensure_dirs()?;
    let advertise = advertise_address
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .or_else(detect_advertise_ip)
        .context("could not determine advertise address; pass Bootstrap.advertise_address")?;
    let hostname = cfg
        .machine
        .network
        .hostname
        .clone()
        .unwrap_or_else(|| "pertisk-cp-1".into());
    let endpoint_host = endpoint_host(&cluster.endpoint);
    let k8s_ver = cluster
        .kubernetes_version
        .as_deref()
        .unwrap_or(DEFAULT_K8S_VERSION);
    let service_subnet = cluster
        .service_subnet
        .as_deref()
        .unwrap_or(DEFAULT_SERVICE_SUBNET);
    let service_cidr = cluster.service_cluster_ip_range();
    let cluster_cidr = cluster.cluster_cidr();
    let sans = cluster.pki_extra_sans();

    info!(%advertise, %hostname, %k8s_ver, "generating control-plane PKI");
    let kubernetes_svc_ip = kubernetes_service_ip(service_subnet);
    let pki = pki::generate_pki_with_optional_existing(
        &advertise,
        &hostname,
        &endpoint_host,
        &kubernetes_svc_ip,
        &sans,
        cluster.ca.as_deref(),
        cluster.ca_key.as_deref(),
        cluster.sa_key.as_deref(),
    )?;
    pki::write_pki(&paths.pki(), &paths.etcd_pki(), &pki)?;

    let kc_name = cluster
        .name
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("pertisk");
    let admin = kubeconfig::render_kubeconfig(
        &cluster.endpoint,
        &pki.ca_crt,
        &pki.admin_crt,
        &pki.admin_key,
        "kubernetes-admin",
        kc_name,
    );
    let local_admin = kubeconfig::render_kubeconfig(
        "https://127.0.0.1:6443",
        &pki.ca_crt,
        &pki.admin_crt,
        &pki.admin_key,
        "kubernetes-admin",
        kc_name,
    );
    let cm_conf = kubeconfig::render_kubeconfig(
        "https://127.0.0.1:6443",
        &pki.ca_crt,
        &pki.cm_crt,
        &pki.cm_key,
        "system:kube-controller-manager",
        kc_name,
    );
    let sched_conf = kubeconfig::render_kubeconfig(
        "https://127.0.0.1:6443",
        &pki.ca_crt,
        &pki.sched_crt,
        &pki.sched_key,
        "system:kube-scheduler",
        kc_name,
    );
    let kubelet_conf = kubeconfig::render_kubeconfig(
        "https://127.0.0.1:6443",
        &pki.ca_crt,
        &pki.kubelet_crt,
        &pki.kubelet_key,
        &format!("system:node:{hostname}"),
        kc_name,
    );

    fs::write(paths.admin_kubeconfig(), &admin)?;
    fs::write(paths.admin_local_kubeconfig(), &local_admin)?;
    fs::write(
        paths.kubeconfig_dir().join("controller-manager.conf"),
        &cm_conf,
    )?;
    fs::write(paths.kubeconfig_dir().join("scheduler.conf"), &sched_conf)?;
    fs::write(paths.kubeconfig_dir().join("kubelet.conf"), &kubelet_conf)?;

    // Live paths used by static pods.
    let live = Path::new("/etc/kubernetes");
    fs::create_dir_all(live)?;
    fs::write(live.join("admin.conf"), &local_admin)?;
    fs::write(live.join("controller-manager.conf"), &cm_conf)?;
    fs::write(live.join("scheduler.conf"), &sched_conf)?;
    fs::write(live.join("kubelet.conf"), &kubelet_conf)?;
    // Kubelet is usually already running with a bootstrap-token kubeconfig from
    // apply; publish cert credentials where it actually reads them.
    publish_kubelet_credentials(&kubelet_conf, &pki.ca_crt)?;

    let pki_live = live.join("pki");
    let etcd_initial_cluster = format!("{hostname}=https://{advertise}:2380");
    static_pods::write_static_pods(
        &paths.manifests(),
        &static_pods::StaticPodParams {
            advertise_ip: &advertise,
            hostname: &hostname,
            kubernetes_version: k8s_ver,
            etcd_image: DEFAULT_ETCD_IMAGE,
            service_cidr: &service_cidr,
            pod_subnet: &cluster_cidr,
            pki_host_path: "/etc/kubernetes/pki",
            etcd_initial_cluster: &etcd_initial_cluster,
            etcd_initial_cluster_state: "new",
        },
    )?;
    let vip = kube_vip::vip_from_endpoint_host(&endpoint_host);
    let _ = kube_vip::maybe_write_kube_vip(
        &paths.manifests(),
        vip,
        cluster.vip6.as_deref(),
        &advertise,
        kube_vip::DEFAULT_KUBE_VIP_INTERFACE,
    )?;
    paths.link_live()?;
    // Ensure pki is present at live path (symlink or copy).
    if !pki_live.exists() {
        copy_dir(&paths.pki(), &pki_live)?;
    }

    fs::create_dir_all("/var/lib/etcd").ok();
    fs::write(
        paths.marker(),
        format!("bootstrapped at {}\n", chrono_like_now()),
    )?;

    // Block until token Secret + join RBAC + CP role are in place. Fire-and-forget
    // races HA joins (etcd/apiserver flaps) and leaves workers Unauthorized.
    let admin_path = paths.admin_kubeconfig();
    let token = cluster.token.clone();
    let node_name = hostname.clone();
    if let Err(err) = finalize_bootstrap_when_ready(&admin_path, token.as_deref(), &node_name) {
        tracing::warn!(
            error = %err,
            "post-bootstrap API finalize incomplete; apply token Secret / node-rbac / CP label manually if needed"
        );
    }

    Ok(BootstrapResult {
        already_bootstrapped: false,
        message: format!(
            "control-plane bootstrapped advertise={advertise}; ensure containerd can pull registry.k8s.io images"
        ),
        admin_kubeconfig: admin,
        ca_pem: pki.ca_crt,
    })
}

pub fn read_admin_kubeconfig(state_root: &Path) -> Result<String> {
    let paths = BootstrapPaths::default_state(state_root);
    let raw = fs::read_to_string(paths.admin_kubeconfig())
        .context("admin kubeconfig missing; bootstrap first")?;
    Ok(kubeconfig::sanitize_kubeconfig(&raw))
}

/// Rewrite kubeconfig files that still have the trailing-`"` render bug.
fn repair_kubeconfig_files(paths: &BootstrapPaths) -> Result<()> {
    let candidates = [
        paths.admin_kubeconfig(),
        paths.kubeconfig_dir().join("controller-manager.conf"),
        paths.kubeconfig_dir().join("scheduler.conf"),
        paths.kubeconfig_dir().join("kubelet.conf"),
        Path::new("/etc/kubernetes/admin.conf").to_path_buf(),
        Path::new("/etc/kubernetes/controller-manager.conf").to_path_buf(),
        Path::new("/etc/kubernetes/scheduler.conf").to_path_buf(),
        Path::new("/etc/kubernetes/kubelet.conf").to_path_buf(),
    ];
    for path in candidates {
        if !path.is_file() {
            continue;
        }
        let raw = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let fixed = kubeconfig::sanitize_kubeconfig(&raw);
        if fixed != raw {
            info!(path = %path.display(), "repairing kubeconfig trailing quote");
            let _ = fs::write(&path, fixed);
        }
    }
    // Keep live kubelet credentials in sync on re-bootstrap / repair.
    if let Ok(kc) = fs::read_to_string("/etc/kubernetes/kubelet.conf") {
        let ca = fs::read_to_string(paths.pki().join("ca.crt")).unwrap_or_default();
        let _ = publish_kubelet_credentials(&kc, &ca);
    }
    Ok(())
}

/// Install cert kubeconfig where the running kubelet reads it.
pub fn publish_kubelet_credentials(kubelet_conf: &str, ca_crt: &str) -> Result<()> {
    let root = Path::new("/var/lib/kubelet");
    fs::create_dir_all(root).ok();
    fs::write(root.join("kubeconfig"), kubelet_conf)
        .context("write /var/lib/kubelet/kubeconfig")?;
    if !ca_crt.is_empty() {
        fs::write(root.join("ca.crt"), ca_crt).context("write /var/lib/kubelet/ca.crt")?;
    }
    info!("published kubelet credentials under /var/lib/kubelet");
    Ok(())
}

pub fn read_ca_pem(state_root: &Path) -> Result<String> {
    let paths = BootstrapPaths::default_state(state_root);
    fs::read_to_string(paths.pki().join("ca.crt")).context("ca.crt missing; bootstrap first")
}

/// Re-publish `/etc/kubernetes` + kubelet cert credentials after reboot.
///
/// Root `/etc` is ephemeral; PKI/manifests/kubeconfigs live on STATE. Without
/// this, CP kubelet starts in bootstrap-token mode, deletes its cert kubeconfig,
/// and exits immediately.
pub fn restore_control_plane(state_root: &Path) -> Result<bool> {
    let paths = BootstrapPaths::default_state(state_root);
    if !paths.is_bootstrapped() {
        return Ok(false);
    }
    repair_kubeconfig_files(&paths)?;
    paths.link_live()?;
    let kubelet_conf = paths.kubeconfig_dir().join("kubelet.conf");
    let kubelet_conf = fs::read_to_string(&kubelet_conf)
        .with_context(|| format!("read {}", kubelet_conf.display()))?;
    let ca = fs::read_to_string(paths.pki().join("ca.crt")).unwrap_or_default();
    publish_kubelet_credentials(&kubelet_conf, &ca)?;
    // Static pods also need these under the live path (link_live copies them).
    info!(
        state = %paths.root.display(),
        "restored control-plane /etc/kubernetes + kubelet credentials from STATE"
    );
    Ok(true)
}

pub(crate) fn endpoint_host(endpoint: &str) -> String {
    endpoint
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or(endpoint)
        .split(':')
        .next()
        .unwrap_or(endpoint)
        .to_string()
}

pub(crate) fn detect_advertise_ip() -> Option<String> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("1.1.1.1:80").ok()?;
    let ip = sock.local_addr().ok()?.ip();
    if ip.is_loopback() || ip.is_unspecified() {
        return None;
    }
    Some(ip.to_string())
}

/// First usable address in the service CIDR (kubeadm: `kubernetes` Service ClusterIP).
/// When `service_cidr` is dual-stack (`v4,v6`), uses the IPv4 range only.
pub(crate) fn kubernetes_service_ip(service_cidr: &str) -> String {
    let cidr = service_cidr
        .split(',')
        .next()
        .unwrap_or(service_cidr)
        .split('/')
        .next()
        .unwrap_or("10.96.0.0");
    if let Ok(std::net::IpAddr::V4(v4)) = cidr.parse() {
        let o = v4.octets();
        // network + 1 → 10.96.0.1 for 10.96.0.0/12
        return std::net::Ipv4Addr::new(o[0], o[1], o[2], o[3].saturating_add(1)).to_string();
    }
    "10.96.0.1".into()
}

pub(crate) fn chrono_like_now() -> String {
    use std::time::SystemTime;
    match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => format!("{}s", d.as_secs()),
        Err(_) => "unknown".into(),
    }
}

pub(crate) fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            fs::copy(entry.path(), to)?;
        }
    }
    Ok(())
}

fn apiserver_tcp_ready(timeout: Duration) -> bool {
    use std::net::{SocketAddr, TcpStream};
    let addr: SocketAddr = "127.0.0.1:6443".parse().expect("valid addr");
    TcpStream::connect_timeout(&addr, timeout).is_ok()
}

/// After apiserver is healthy: bootstrap-token Secret, join RBAC, CP node label.
pub(crate) fn finalize_bootstrap_when_ready(
    admin_kubeconfig: &Path,
    token: Option<&str>,
    node_name: &str,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(300);
    while Instant::now() < deadline {
        if apiserver_tcp_ready(Duration::from_secs(2)) {
            break;
        }
        thread::sleep(Duration::from_secs(3));
    }
    if !apiserver_tcp_ready(Duration::from_secs(2)) {
        bail!("apiserver not reachable on 127.0.0.1:6443 within timeout");
    }

    let kc = fs::read_to_string(admin_kubeconfig).context("read admin kubeconfig")?;
    let (ca, cert, key) = api::credentials_from_kubeconfig(&kc)?;
    let client = api::KubeClient::local(&ca, &cert, &key)?;

    // Apiserver can accept TCP before controller-manager has created kube-system.
    ensure_kube_system(&client, deadline)?;

    // Persist token YAML for debugging / manual re-apply.
    if let Some(token) = token {
        let Some((id, secret)) = token::split_token(token) else {
            bail!("invalid bootstrap token format");
        };
        let dir = Path::new("/var/lib/pertisk/kubernetes");
        fs::create_dir_all(dir).ok();
        let yaml = format!(
            "apiVersion: v1\nkind: Secret\nmetadata:\n  name: bootstrap-token-{id}\n  namespace: kube-system\ntype: bootstrap.kubernetes.io/token\nstringData:\n  description: pertisk bootstrap token\n  token-id: \"{id}\"\n  token-secret: \"{secret}\"\n  usage-bootstrap-authentication: \"true\"\n  usage-bootstrap-signing: \"true\"\n  auth-extra-groups: system:bootstrappers:kubeadm:default-node-token\n"
        );
        fs::write(dir.join("bootstrap-token-secret.yaml"), &yaml)?;

        let secret_name = format!("bootstrap-token-{id}");
        let body = serde_json::json!({
            "apiVersion": "v1",
            "kind": "Secret",
            "metadata": {
                "name": &secret_name,
                "namespace": "kube-system"
            },
            "type": "bootstrap.kubernetes.io/token",
            "stringData": {
                "description": "pertisk bootstrap token",
                "token-id": id,
                "token-secret": secret,
                "usage-bootstrap-authentication": "true",
                "usage-bootstrap-signing": "true",
                "auth-extra-groups": "system:bootstrappers:kubeadm:default-node-token"
            }
        })
        .to_string();
        // Retry: early apiserver can accept TCP then flap while etcd settles.
        let mut last_err = None;
        while Instant::now() < deadline {
            match ensure_created(
                &client,
                "/api/v1/namespaces/kube-system/secrets",
                &body,
                &secret_name,
            ) {
                Ok(()) => {
                    last_err = None;
                    break;
                }
                Err(err) => {
                    tracing::warn!(error = %err, "bootstrap-token Secret create failed; retrying");
                    last_err = Some(err);
                    thread::sleep(Duration::from_secs(3));
                }
            }
        }
        if let Some(err) = last_err {
            return Err(err).context("bootstrap-token Secret not created within finalize timeout");
        }
        // Confirm readable (create 201 alone is not enough if a later flap drops etcd).
        let get_path = format!("/api/v1/namespaces/kube-system/secrets/{secret_name}");
        let (gstatus, _) = client.get(&get_path)?;
        if gstatus != 200 {
            bail!("bootstrap-token Secret {secret_name} missing after create (HTTP {gstatus})");
        }
    }

    ensure_node_join_rbac(&client)?;
    // Fresh deadline: waiting for local apiserver/images can consume most of the
    // original window; CP3 join especially was left unlabeled (looked like a worker).
    let label_deadline = Instant::now() + Duration::from_secs(300);
    ensure_control_plane_node_role(&client, node_name, label_deadline)?;
    // Basic addons: CoreDNS + metrics-server (usable cluster after CNI is up).
    if let Err(err) = addons::ensure_basic_addons(&client) {
        tracing::warn!(
            error = %err,
            "basic addons incomplete; apply examples/dns/coredns.yaml and examples/addons/metrics-server.yaml"
        );
    }
    info!(node = %node_name, "post-bootstrap API finalize complete");
    Ok(())
}

pub(crate) fn ensure_created(client: &api::KubeClient, path: &str, body: &str, name: &str) -> Result<()> {
    let (status, resp) = client.post_json(path, body)?;
    if status == 201 || status == 200 || status == 409 {
        info!(name, status, "ensured API object");
        Ok(())
    } else {
        bail!("create {name} failed HTTP {status}: {resp}");
    }
}

/// Wait for (or create) `kube-system` so bootstrap-token Secrets can be posted.
fn ensure_kube_system(client: &api::KubeClient, deadline: Instant) -> Result<()> {
    let body = serde_json::json!({
        "apiVersion": "v1",
        "kind": "Namespace",
        "metadata": { "name": "kube-system" }
    })
    .to_string();

    while Instant::now() < deadline {
        let (status, resp) = client.get("/api/v1/namespaces/kube-system")?;
        if status == 200 {
            info!("kube-system namespace ready");
            return Ok(());
        }
        if status == 404 {
            let (cstatus, cresp) = client.post_json("/api/v1/namespaces", &body)?;
            if cstatus == 201 || cstatus == 200 || cstatus == 409 {
                info!(status = cstatus, "ensured kube-system namespace");
                return Ok(());
            }
            tracing::warn!(status = cstatus, body = %cresp, "create kube-system failed; retrying");
        } else {
            tracing::warn!(status, body = %resp, "get kube-system failed; retrying");
        }
        thread::sleep(Duration::from_secs(2));
    }
    bail!("kube-system namespace not available within finalize timeout")
}

fn ensure_node_join_rbac(client: &api::KubeClient) -> Result<()> {
    // Mirrors examples/bootstrap/node-rbac.yaml (kubeadm-shaped).
    let group_bindings = [
        (
            "pertisk:kubelet-bootstrap",
            "system:bootstrappers:kubeadm:default-node-token",
            "system:node-bootstrapper",
        ),
        (
            "pertisk:node-autoapprove-bootstrap",
            "system:bootstrappers:kubeadm:default-node-token",
            "system:certificates.k8s.io:certificatesigningrequests:nodeclient",
        ),
        (
            "pertisk:node-autoapprove-certificate-rotation",
            "system:nodes",
            "system:certificates.k8s.io:certificatesigningrequests:selfnodeclient",
        ),
    ];
    for (name, subject_group, role) in group_bindings {
        let body = serde_json::json!({
            "apiVersion": "rbac.authorization.k8s.io/v1",
            "kind": "ClusterRoleBinding",
            "metadata": { "name": name },
            "subjects": [{
                "kind": "Group",
                "name": subject_group,
                "apiGroup": "rbac.authorization.k8s.io"
            }],
            "roleRef": {
                "apiGroup": "rbac.authorization.k8s.io",
                "kind": "ClusterRole",
                "name": role
            }
        });
        ensure_created(
            client,
            "/apis/rbac.authorization.k8s.io/v1/clusterrolebindings",
            &body.to_string(),
            name,
        )?;
    }

    // Apiserver uses /etc/kubernetes/pki/apiserver.crt (CN=kube-apiserver) as the
    // kubelet client cert. Without this binding, kubectl/ktail logs return
    // Forbidden on nodes/proxy after x509 auth succeeds.
    let kubelet_api = serde_json::json!({
        "apiVersion": "rbac.authorization.k8s.io/v1",
        "kind": "ClusterRoleBinding",
        "metadata": { "name": "system:kube-apiserver-to-kubelet" },
        "subjects": [{
            "kind": "User",
            "name": "kube-apiserver",
            "apiGroup": "rbac.authorization.k8s.io"
        }],
        "roleRef": {
            "apiGroup": "rbac.authorization.k8s.io",
            "kind": "ClusterRole",
            "name": "system:kubelet-api-admin"
        }
    });
    ensure_created(
        client,
        "/apis/rbac.authorization.k8s.io/v1/clusterrolebindings",
        &kubelet_api.to_string(),
        "system:kube-apiserver-to-kubelet",
    )?;
    Ok(())
}

fn ensure_control_plane_node_role(
    client: &api::KubeClient,
    node_name: &str,
    deadline: Instant,
) -> Result<()> {
    let path = format!("/api/v1/nodes/{node_name}");
    while Instant::now() < deadline {
        let (status, _) = client.get(&path)?;
        if status == 200 {
            break;
        }
        thread::sleep(Duration::from_secs(3));
    }
    let (status, _) = client.get(&path)?;
    if status != 200 {
        bail!("node {node_name} not registered before timeout (HTTP {status})");
    }

    // Label via merge-patch (empty-string role labels are reliable this way).
    let label_patch = serde_json::json!({
        "metadata": {
            "labels": {
                "node-role.kubernetes.io/control-plane": ""
            }
        }
    });
    let (status, resp) = client.patch_merge(&path, &label_patch.to_string())?;
    if status != 200 {
        bail!("label node {node_name} failed HTTP {status}: {resp}");
    }

    // Taint via strategic merge (merge key = key+effect).
    let taint_patch = serde_json::json!({
        "spec": {
            "taints": [{
                "key": "node-role.kubernetes.io/control-plane",
                "effect": "NoSchedule"
            }]
        }
    });
    let (status, resp) = client.patch_strategic(&path, &taint_patch.to_string())?;
    if status != 200 {
        bail!("taint node {node_name} failed HTTP {status}: {resp}");
    }

    // Verify label stuck (node recreate / patch no-op races).
    let (status, body) = client.get(&path)?;
    if status != 200 {
        bail!("re-get node {node_name} after label failed HTTP {status}");
    }
    let v: serde_json::Value = serde_json::from_str(&body).context("parse node after label")?;
    let has = v
        .pointer("/metadata/labels/node-role.kubernetes.io/control-plane")
        .is_some();
    if !has {
        bail!("node {node_name} missing control-plane label after patch");
    }
    info!(node = %node_name, "labeled control-plane (+ NoSchedule taint)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gen_config_writes_token() {
        let g = gen_config(
            "lab",
            "https://10.1.1.10:6443",
            DEFAULT_K8S_VERSION,
            DEFAULT_POD_SUBNET,
            DEFAULT_SERVICE_SUBNET,
        )
        .unwrap();
        assert!(g.token.contains('.'));
        assert!(g.controlplane_yaml.contains("controlplane"));
        assert!(g.controlplane_yaml.contains("dashboard"));
        assert!(g.controlplane_yaml.contains("catppuccin"));
        assert!(g.worker_yaml.contains("worker"));
        assert!(g.worker_yaml.contains("dashboard"));
        assert!(g.worker_yaml.contains("catppuccin"));
    }

    #[test]
    fn gen_config_includes_mgmt_url() {
        let net = GenNetworkOpts {
            mgmt_url: Some("https://ptkos.apps.thaidevops.co".into()),
            ..Default::default()
        };
        let g = gen_config_with_network(
            "lab",
            "https://10.1.1.10:6443",
            DEFAULT_K8S_VERSION,
            DEFAULT_POD_SUBNET,
            DEFAULT_SERVICE_SUBNET,
            &net,
        )
        .unwrap();
        assert!(g.controlplane_yaml.contains("mgmt_url: https://ptkos.apps.thaidevops.co"));
        assert!(g.worker_yaml.contains("mgmt_url: https://ptkos.apps.thaidevops.co"));
    }

    #[test]
    fn patch_worker_ca_fills_builtin_dashboard() {
        let yaml = r#"
version: v1alpha1
machine:
  type: worker
  network:
    hostname: wk-1
cluster:
  endpoint: https://10.1.1.10:6443
  token: a.b
"#;
        let out = patch_worker_ca(
            yaml,
            "-----BEGIN CERTIFICATE-----\nX\n-----END CERTIFICATE-----",
        )
        .unwrap();
        assert!(out.contains("dashboard"));
        assert!(out.contains("catppuccin"));
        assert!(out.contains("BEGIN CERTIFICATE"));
    }
}
