//! Control-plane bootstrap for Pertisk KOS (static pods).

mod addons;
mod api;
mod coredns;
mod etcd_backup;
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

pub use etcd_backup::{
    default_restore_identity, etcd_restore, etcd_snapshot, EtcdRestoreResult, EtcdSnapshotResult,
};
pub use gen::{
    gen_config, gen_config_ha, gen_config_ha_with_network, gen_config_with_network,
    patch_controlplane_secrets, patch_worker_ca, write_gen_config, write_gen_config_ha,
    GenConfigHaOutput, GenConfigOutput, GenNetworkOpts,
};
pub use join::{get_join_config, join_control_plane, JoinConfigResult, JoinControlPlaneResult};
pub use kubeconfig::{kubeconfig_has_client_cert, rename_kubeconfig_context, sanitize_kubeconfig};
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
        let expected = advertise_address.map(str::trim).filter(|s| !s.is_empty());
        if let Some(want) = expected {
            if let Some(stored) = paths.read_advertise() {
                if stored != want {
                    tracing::warn!(
                        stored,
                        want,
                        "advertise IP changed after bootstrap; rebasing control plane"
                    );
                    if let Err(err) = rebase_advertise_address(state_root, want) {
                        tracing::warn!(error = %err, "rebase after IP change failed");
                    }
                }
            } else {
                // Legacy STATE without advertise file — compare live detection.
                if let Some(live) = detect_advertise_ip() {
                    if live != want {
                        tracing::warn!(
                            live,
                            want,
                            "legacy STATE advertise missing; rebasing onto requested IP"
                        );
                        if let Err(err) = rebase_advertise_address(state_root, want) {
                            tracing::warn!(error = %err, "rebase after IP change failed");
                        }
                    }
                }
                let _ = paths.write_advertise(want);
            }
        }
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
    let service_subnet = cluster.ipv4_service_subnet();
    let service_cidr = cluster.service_cluster_ip_range();
    let cluster_cidr = cluster.cluster_cidr();
    let sans = cluster.pki_extra_sans();

    info!(%advertise, %hostname, %k8s_ver, "generating control-plane PKI");
    let kubernetes_svc_ip = kubernetes_service_ip(&service_subnet);
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
    paths.write_advertise(&advertise)?;
    fs::write(
        paths.marker(),
        format!("bootstrapped at {}\n", chrono_like_now()),
    )?;

    // Block until token Secret + join RBAC + CP role are in place. Returning ok
    // while finalize failed left lab-up waiting on :6443 forever (silent miss).
    let admin_path = paths.admin_kubeconfig();
    let token = cluster.token.clone();
    let node_name = hostname.clone();
    let defer_addons = matches!(cluster.cni, pertisk_config::CniMode::None);
    finalize_bootstrap_when_ready(&admin_path, token.as_deref(), &node_name, defer_addons)
        .with_context(|| {
            format!(
                "post-bootstrap finalize failed (advertise={advertise}). \
Check containerd can pull registry.k8s.io/kube-apiserver + etcd images \
(pertiskctl -e {advertise}:50000 logs containerd -n 80)"
            )
        })?;

    Ok(BootstrapResult {
        already_bootstrapped: false,
        message: format!("control-plane bootstrapped advertise={advertise}"),
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
    // Ask pertiskd to restart kubelet so it picks up cert credentials before
    // join finalize waits for the Node object (otherwise CP stays unregistered).
    request_kubelet_reload();
    info!("published kubelet credentials under /var/lib/kubelet");
    Ok(())
}

/// Sentinel watched by pertiskd's supervise loop (`/run/pertisk/kubelet-reload`).
pub const KUBELET_RELOAD_FLAG: &str = "/run/pertisk/kubelet-reload";

pub fn request_kubelet_reload() {
    let path = Path::new(KUBELET_RELOAD_FLAG);
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(path, b"1\n");
}

/// Returns true once if a reload was requested (consumes the flag).
pub fn take_kubelet_reload_request() -> bool {
    let path = Path::new(KUBELET_RELOAD_FLAG);
    if !path.is_file() {
        return false;
    }
    let _ = fs::remove_file(path);
    true
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
    if let Err(err) = maybe_rebase_advertise(state_root) {
        tracing::warn!(error = %err, "advertise rebase on restore failed");
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

const LIVE_KUBELET_DIR: &str = "/var/lib/kubelet";

/// Copy issued worker kubelet credentials onto STATE so a reboot can restore them.
///
/// `/var/lib/kubelet` is on EPHEMERAL (or tmpfs). After shutdown/boot the Node
/// object is still issued, but kubelet used to wipe kubeconfig and fail TLS
/// bootstrap once the join token expired.
pub fn snapshot_worker_kubelet(state_root: &Path) -> Result<bool> {
    snapshot_worker_kubelet_from(state_root, Path::new(LIVE_KUBELET_DIR))
}

pub fn snapshot_worker_kubelet_from(state_root: &Path, live: &Path) -> Result<bool> {
    let paths = BootstrapPaths::default_state(state_root);
    if paths.is_bootstrapped() {
        return Ok(false);
    }
    let live_kc = live.join("kubeconfig");
    let Ok(raw) = fs::read_to_string(&live_kc) else {
        return Ok(false);
    };
    if !kubeconfig::kubeconfig_has_client_cert(&raw) {
        return Ok(false);
    }
    let dest = paths.kubelet_runtime();
    fs::create_dir_all(&dest)?;
    fs::copy(&live_kc, dest.join("kubeconfig"))
        .with_context(|| format!("snapshot {}", live_kc.display()))?;
    let live_ca = live.join("ca.crt");
    if live_ca.is_file() {
        let _ = fs::copy(&live_ca, dest.join("ca.crt"));
    }
    let live_pki = live.join("pki");
    if live_pki.is_dir() {
        copy_dir(&live_pki, &dest.join("pki"))?;
    }
    Ok(true)
}

/// Restore worker kubelet client certs from STATE before kubelet starts.
pub fn restore_worker_kubelet(state_root: &Path) -> Result<bool> {
    restore_worker_kubelet_into(state_root, Path::new(LIVE_KUBELET_DIR))
}

pub fn restore_worker_kubelet_into(state_root: &Path, live: &Path) -> Result<bool> {
    let paths = BootstrapPaths::default_state(state_root);
    if paths.is_bootstrapped() {
        return Ok(false);
    }
    let src = paths.kubelet_runtime();
    let src_kc = src.join("kubeconfig");
    if !src_kc.is_file() {
        return Ok(false);
    }
    let Ok(raw) = fs::read_to_string(&src_kc) else {
        return Ok(false);
    };
    if !kubeconfig::kubeconfig_has_client_cert(&raw) {
        return Ok(false);
    }
    fs::create_dir_all(live)?;
    let live_kc = live.join("kubeconfig");
    if live_kc.is_file() {
        if let Ok(existing) = fs::read_to_string(&live_kc) {
            if kubeconfig::kubeconfig_has_client_cert(&existing) {
                return Ok(false);
            }
        }
    }
    fs::copy(&src_kc, &live_kc).with_context(|| format!("restore {}", live_kc.display()))?;
    let src_ca = src.join("ca.crt");
    if src_ca.is_file() {
        let _ = fs::copy(&src_ca, live.join("ca.crt"));
    }
    let src_pki = src.join("pki");
    if src_pki.is_dir() {
        copy_dir(&src_pki, &live.join("pki"))?;
    }
    info!(
        state = %src.display(),
        "restored worker kubelet credentials from STATE"
    );
    Ok(true)
}

/// If DHCP/IPAM handed out a new address after reboot, rewrite static pods + serving certs.
///
/// Returns `true` when the advertise address was rewritten.
pub fn maybe_rebase_advertise(state_root: &Path) -> Result<bool> {
    let paths = BootstrapPaths::default_state(state_root);
    if !paths.is_bootstrapped() {
        return Ok(false);
    }
    let Some(detected) = detect_advertise_ip() else {
        let _ = ensure_etcd_listen_all(&paths);
        return Ok(false);
    };
    rebase_advertise_address(state_root, &detected)
}

/// Whether `detected` should replace `stored` as the node's advertise IP.
/// Skips no-op and VIP (kube-vip on the same NIC can win `detect_advertise_ip`).
pub fn pick_rebase_ip(stored: &str, detected: &str, vip: Option<&str>) -> Option<String> {
    let stored = stored.trim();
    let detected = detected.trim();
    if detected.is_empty() || detected == stored {
        return None;
    }
    if vip
        .map(str::trim)
        .is_some_and(|v| !v.is_empty() && v == detected)
    {
        return None;
    }
    Some(detected.to_string())
}

/// Rewrite control-plane manifests, serving certs, and kubeconfig for `new_ip`.
pub fn rebase_advertise_address(state_root: &Path, new_ip: &str) -> Result<bool> {
    let paths = BootstrapPaths::default_state(state_root);
    if !paths.is_bootstrapped() {
        return Ok(false);
    }
    let new_ip = new_ip.trim();
    if new_ip.is_empty() {
        return Ok(false);
    }
    let stored = paths.read_advertise().unwrap_or_default();
    let admin_raw = fs::read_to_string(paths.admin_kubeconfig()).unwrap_or_default();
    let server_host = kubeconfig::kubeconfig_server_host(&admin_raw);
    let vip = server_host
        .as_deref()
        .filter(|h| !h.is_empty() && *h != stored.as_str());
    if pick_rebase_ip(&stored, new_ip, vip).is_none() {
        let changed = ensure_etcd_listen_all(&paths)?;
        return Ok(changed);
    }

    info!(from = %stored, to = %new_ip, "rebasing control-plane advertise address");
    rewrite_ip_in_dir(&paths.manifests(), &stored, new_ip)?;
    let _ = ensure_etcd_listen_all(&paths);

    let etcd_yaml = fs::read_to_string(paths.manifests().join("etcd.yaml")).unwrap_or_default();
    let api_yaml =
        fs::read_to_string(paths.manifests().join("kube-apiserver.yaml")).unwrap_or_default();
    let hostname = flag_value(&etcd_yaml, "--name=").unwrap_or_else(|| "pertisk-cp".into());
    let endpoint_host = server_host
        .clone()
        .filter(|h| h != new_ip)
        .or_else(|| vip.map(|s| s.to_string()))
        .unwrap_or_else(|| new_ip.to_string());
    let svc_cidr = flag_value(&api_yaml, "--service-cluster-ip-range=")
        .unwrap_or_else(|| DEFAULT_SERVICE_SUBNET.to_string());
    let kubernetes_svc_ip = kubernetes_service_ip(&svc_cidr);
    let mut extra = Vec::new();
    if !stored.is_empty() {
        extra.push(stored.clone());
    }
    pki::reissue_serving_certs(
        &paths.pki(),
        &paths.etcd_pki(),
        new_ip,
        &hostname,
        &endpoint_host,
        &kubernetes_svc_ip,
        &extra,
    )?;

    if server_host.as_deref() == Some(stored.as_str()) && !stored.is_empty() {
        let rewritten =
            kubeconfig::rewrite_kubeconfig_server(&admin_raw, &format!("https://{new_ip}:6443"));
        fs::write(paths.admin_kubeconfig(), rewritten)
            .with_context(|| format!("write {}", paths.admin_kubeconfig().display()))?;
    }
    paths.write_advertise(new_ip)?;
    request_kubelet_reload();
    Ok(true)
}

/// Replace `old` with `new` only at IP token boundaries (`10.1.1.1` must not match `10.1.1.10`).
pub fn replace_ip_token(haystack: &str, old: &str, new: &str) -> String {
    if old.is_empty() || old == new {
        return haystack.to_string();
    }
    let mut out = String::with_capacity(haystack.len());
    let mut rest = haystack;
    while let Some(i) = rest.find(old) {
        let before_ok = i == 0 || !rest.as_bytes()[i - 1].is_ascii_digit();
        let after_idx = i + old.len();
        let after_ok = after_idx >= rest.len()
            || !rest
                .as_bytes()
                .get(after_idx)
                .is_some_and(|b| b.is_ascii_digit());
        out.push_str(&rest[..i]);
        if before_ok && after_ok {
            out.push_str(new);
        } else {
            out.push_str(old);
        }
        rest = &rest[after_idx..];
    }
    out.push_str(rest);
    out
}

fn rewrite_ip_in_dir(dir: &Path, old: &str, new: &str) -> Result<()> {
    if old.is_empty() || !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };
        let next = replace_ip_token(&raw, old, new);
        if next != raw {
            fs::write(&path, next).with_context(|| format!("write {}", path.display()))?;
        }
    }
    Ok(())
}

fn ensure_etcd_listen_all(paths: &BootstrapPaths) -> Result<bool> {
    let path = paths.manifests().join("etcd.yaml");
    if !path.is_file() {
        return Ok(false);
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    const WANT: &str = "--listen-client-urls=https://0.0.0.0:2379";
    if raw.contains(WANT) && !raw.contains("--listen-client-urls=https://127.0.0.1:2379") {
        return Ok(false);
    }
    let mut out = String::with_capacity(raw.len());
    for line in raw.lines() {
        if line.contains("--listen-client-urls=") {
            let pad = line.len() - line.trim_start().len();
            let trimmed = line.trim_end();
            let comma = if trimmed.ends_with(',') { "," } else { "" };
            out.push_str(&" ".repeat(pad));
            out.push_str(WANT);
            out.push_str(comma);
            out.push('\n');
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if out == raw {
        return Ok(false);
    }
    fs::write(&path, out).with_context(|| format!("write {}", path.display()))?;
    Ok(true)
}

fn flag_value(yaml: &str, flag: &str) -> Option<String> {
    for line in yaml.lines() {
        let Some(i) = line.find(flag) else {
            continue;
        };
        let rest = line[i + flag.len()..].trim();
        let val = rest
            .trim_start_matches('"')
            .trim_end_matches(['"', ',', ' ']);
        if !val.is_empty() {
            return Some(val.to_string());
        }
    }
    None
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

pub fn detect_advertise_ip() -> Option<String> {
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
///
/// When `defer_addons` is true (`cluster.cni: none`), skip CoreDNS/metrics-server —
/// they need a cluster CNI and otherwise spam kubelet with
/// "failed to find network info for sandbox".
fn finalize_timeout() -> Duration {
    // Soft-reset used to wipe /var/lib/containerd; first bootstrap then waits on
    // registry.k8s.io pulls. Match lab-up BOOTSTRAP_TIMEOUT default (600s).
    let secs = std::env::var("PERTISK_BOOTSTRAP_FINALIZE_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(600);
    Duration::from_secs(secs.max(60))
}

pub(crate) fn finalize_bootstrap_when_ready(
    admin_kubeconfig: &Path,
    token: Option<&str>,
    node_name: &str,
    defer_addons: bool,
) -> Result<()> {
    let deadline = Instant::now() + finalize_timeout();
    while Instant::now() < deadline {
        if apiserver_tcp_ready(Duration::from_secs(2)) {
            break;
        }
        thread::sleep(Duration::from_secs(3));
    }
    if !apiserver_tcp_ready(Duration::from_secs(2)) {
        bail!(
            "apiserver not reachable on 127.0.0.1:6443 within {}s \
(static pods not up — usually slow/failed registry.k8s.io image pulls)",
            finalize_timeout().as_secs()
        );
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
    // Basic addons need pod networking. With cluster.cni:none, lab-up/helm installs
    // Cilium/Calico/Flannel first, then CoreDNS + metrics-server.
    if defer_addons {
        info!("deferring CoreDNS/metrics-server until cluster CNI is installed (cni: none)");
    } else if let Err(err) = addons::ensure_basic_addons(&client) {
        tracing::warn!(
            error = %err,
            "basic addons incomplete; apply examples/dns/coredns.yaml and examples/addons/metrics-server.yaml"
        );
    }
    info!(node = %node_name, "post-bootstrap API finalize complete");
    Ok(())
}

pub(crate) fn ensure_created(
    client: &api::KubeClient,
    path: &str,
    body: &str,
    name: &str,
) -> Result<()> {
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
        match client.get(&path) {
            Ok((status, _)) if status == 200 => break,
            Ok((status, _)) => {
                tracing::debug!(status, node = %node_name, "node not registered yet");
            }
            Err(err) => {
                tracing::warn!(error = %err, node = %node_name, "get node failed; retrying");
            }
        }
        thread::sleep(Duration::from_secs(3));
    }
    let (status, body) = client
        .get(&path)
        .with_context(|| format!("get node {node_name}"))?;
    if status != 200 {
        bail!(
            "node {node_name} not registered before timeout (HTTP {status}). \
Check kubelet logs on that node; if STATE was reused after a failed create, \
soft-reset it (pertiskctl reset --force) and re-join"
        );
    }
    // Already labeled (retry after a previous partial finalize) — still ensure taint.
    if node_has_control_plane_label(&body) {
        ensure_control_plane_taint(client, &path, node_name);
        info!(node = %node_name, "control-plane label already present");
        return Ok(());
    }

    // Label via merge-patch (empty-string role labels are reliable this way).
    let label_patch = serde_json::json!({
        "metadata": {
            "labels": {
                "node-role.kubernetes.io/control-plane": ""
            }
        }
    });
    let (status, resp) = client
        .patch_merge(&path, &label_patch.to_string())
        .with_context(|| format!("patch label on {node_name}"))?;
    if status != 200 {
        bail!("label node {node_name} failed HTTP {status}: {resp}");
    }

    ensure_control_plane_taint(client, &path, node_name);

    // Verify label stuck (node recreate / patch no-op races).
    // JSON Pointer: `/` in a key must be escaped as `~1` (RFC 6901).
    let (status, body) = client
        .get(&path)
        .with_context(|| format!("re-get node {node_name}"))?;
    if status != 200 {
        bail!("re-get node {node_name} after label failed HTTP {status}");
    }
    if !node_has_control_plane_label(&body) {
        bail!("node {node_name} missing control-plane label after patch");
    }
    info!(node = %node_name, "labeled control-plane (+ NoSchedule taint)");
    Ok(())
}

fn node_has_control_plane_label(body: &str) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return false;
    };
    v.pointer("/metadata/labels/node-role.kubernetes.io~1control-plane")
        .is_some()
}

/// Best-effort NoSchedule taint. Label alone is enough for `ROLES=control-plane`;
/// taint failures must not fail join retries (strategic-merge edge cases).
fn ensure_control_plane_taint(client: &api::KubeClient, path: &str, node_name: &str) {
    let taint_patch = serde_json::json!({
        "spec": {
            "taints": [{
                "key": "node-role.kubernetes.io/control-plane",
                "effect": "NoSchedule"
            }]
        }
    });
    match client.patch_strategic(path, &taint_patch.to_string()) {
        Ok((status, _)) if status == 200 => {}
        Ok((status, resp)) => {
            tracing::warn!(
                node = %node_name,
                status,
                body = %resp,
                "control-plane taint patch failed (label may still be set)"
            );
        }
        Err(err) => {
            tracing::warn!(
                node = %node_name,
                error = %err,
                "control-plane taint patch error (label may still be set)"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_plane_label_json_pointer_escapes_slash() {
        let v = serde_json::json!({
            "metadata": {
                "labels": {
                    "node-role.kubernetes.io/control-plane": "",
                    "kubernetes.io/hostname": "cp-3"
                }
            }
        });
        // Unescaped `/` walks nested objects and misses the label key.
        assert!(v
            .pointer("/metadata/labels/node-role.kubernetes.io/control-plane")
            .is_none());
        assert!(v
            .pointer("/metadata/labels/node-role.kubernetes.io~1control-plane")
            .is_some());
    }

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
        assert!(g.controlplane_yaml.contains("podSubnets:"));
        assert!(g.controlplane_yaml.contains("10.244.0.0/16"));
        assert!(g.controlplane_yaml.contains("serviceSubnets:"));
        assert!(g.controlplane_yaml.contains("10.96.0.0/12"));
        assert!(!g.controlplane_yaml.contains("podSubnet:"));
        assert!(g.worker_yaml.contains("worker"));
        assert!(g.worker_yaml.contains("dashboard"));
        assert!(g.worker_yaml.contains("catppuccin"));
    }

    #[test]
    fn gen_config_dual_stack_emits_ipv6_subnets() {
        let net = GenNetworkOpts {
            dual_stack: true,
            ..Default::default()
        };
        let g = gen_config_with_network(
            "lab",
            "https://10.1.1.10:6443",
            DEFAULT_K8S_VERSION,
            "10.10.0.0/16",
            DEFAULT_SERVICE_SUBNET,
            &net,
        )
        .unwrap();
        assert!(g.controlplane_yaml.contains("networkMode: dual-stack"));
        assert!(g.controlplane_yaml.contains("10.10.0.0/16"));
        assert!(g.controlplane_yaml.contains("2001:db8:10:0::/56"));
        assert!(g.controlplane_yaml.contains("2001:db8:96:1::/112"));
    }

    #[test]
    fn gen_config_includes_mgmt_url() {
        let net = GenNetworkOpts {
            mgmt_url: Some("https://mgmt.example.com".into()),
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
        assert!(g
            .controlplane_yaml
            .contains("mgmt_url: https://mgmt.example.com"));
        assert!(g.worker_yaml.contains("mgmt_url: https://mgmt.example.com"));
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

    #[test]
    fn replace_ip_token_avoids_prefix_matches() {
        let s = "https://10.1.1.1:2379 https://10.1.1.10:2379";
        let out = replace_ip_token(s, "10.1.1.1", "10.1.1.9");
        assert_eq!(out, "https://10.1.1.9:2379 https://10.1.1.10:2379");
    }

    #[test]
    fn pick_rebase_skips_same_and_vip() {
        assert_eq!(pick_rebase_ip("10.1.1.10", "10.1.1.10", None), None);
        assert_eq!(
            pick_rebase_ip("10.1.1.10", "10.1.1.99", Some("10.1.1.99")),
            None
        );
        assert_eq!(
            pick_rebase_ip("10.1.1.10", "10.1.1.40", Some("10.1.1.99")).as_deref(),
            Some("10.1.1.40")
        );
    }

    #[test]
    fn rebase_rewrites_manifests_and_kubeconfig() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path();
        let paths = BootstrapPaths::default_state(state);
        paths.ensure_dirs().unwrap();
        let pki =
            pki::generate_pki("10.1.1.10", "lab-cp-1", "10.1.1.10", "10.96.0.1", &[]).unwrap();
        pki::write_pki(&paths.pki(), &paths.etcd_pki(), &pki).unwrap();
        static_pods::write_static_pods(
            &paths.manifests(),
            &static_pods::StaticPodParams {
                advertise_ip: "10.1.1.10",
                hostname: "lab-cp-1",
                kubernetes_version: DEFAULT_K8S_VERSION,
                etcd_image: DEFAULT_ETCD_IMAGE,
                service_cidr: DEFAULT_SERVICE_SUBNET,
                pod_subnet: DEFAULT_POD_SUBNET,
                pki_host_path: "/etc/kubernetes/pki",
                etcd_initial_cluster: "lab-cp-1=https://10.1.1.10:2380",
                etcd_initial_cluster_state: "new",
            },
        )
        .unwrap();
        let admin = kubeconfig::render_kubeconfig(
            "https://10.1.1.10:6443",
            &pki.ca_crt,
            &pki.admin_crt,
            &pki.admin_key,
            "kubernetes-admin",
            "lab",
        );
        fs::write(paths.admin_kubeconfig(), admin).unwrap();
        fs::write(paths.marker(), "bootstrapped\n").unwrap();
        paths.write_advertise("10.1.1.10").unwrap();

        assert!(rebase_advertise_address(state, "10.1.1.40").unwrap());
        let etcd = fs::read_to_string(paths.manifests().join("etcd.yaml")).unwrap();
        assert!(etcd.contains("https://10.1.1.40:2380"));
        assert!(etcd.contains("0.0.0.0:2379"));
        assert!(!etcd.contains("https://10.1.1.10:2380"));
        let admin = fs::read_to_string(paths.admin_kubeconfig()).unwrap();
        assert!(admin.contains("https://10.1.1.40:6443"));
        assert_eq!(paths.read_advertise().as_deref(), Some("10.1.1.40"));
        assert!(paths.pki().join("apiserver.crt").is_file());
    }

    #[test]
    fn worker_kubelet_snapshot_roundtrip() {
        let state = tempfile::tempdir().unwrap();
        let live = tempfile::tempdir().unwrap();
        let live_dir = live.path();
        fs::create_dir_all(live_dir.join("pki")).unwrap();
        fs::write(
            live_dir.join("kubeconfig"),
            "users:\n- name: default-auth\n  user:\n    client-certificate: /var/lib/kubelet/pki/kubelet-client-current.pem\n",
        )
        .unwrap();
        fs::write(live_dir.join("ca.crt"), "CA\n").unwrap();
        fs::write(live_dir.join("pki/kubelet-client-current.pem"), "CERT\n").unwrap();

        assert!(snapshot_worker_kubelet_from(state.path(), live_dir).unwrap());

        let empty = tempfile::tempdir().unwrap();
        assert!(restore_worker_kubelet_into(state.path(), empty.path()).unwrap());
        let kc = fs::read_to_string(empty.path().join("kubeconfig")).unwrap();
        assert!(kubeconfig_has_client_cert(&kc));
        assert_eq!(
            fs::read_to_string(empty.path().join("pki/kubelet-client-current.pem")).unwrap(),
            "CERT\n"
        );
    }
}
