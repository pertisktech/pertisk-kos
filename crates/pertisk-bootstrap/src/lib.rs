//! Control-plane bootstrap for Pertisk KOS (Talos-shaped static pods).

mod gen;
mod kubeconfig;
mod paths;
mod pki;
mod static_pods;
mod token;

use std::fs;
use std::net::UdpSocket;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use pertisk_config::{MachineConfig, MachineType};
use tracing::info;

pub use gen::{gen_config, patch_worker_ca, write_gen_config, GenConfigOutput};
pub use kubeconfig::sanitize_kubeconfig;
pub use paths::BootstrapPaths;
pub use token::generate_bootstrap_token;

pub const DEFAULT_K8S_VERSION: &str = "v1.32.5";
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
        repair_kubeconfig_files(&paths)?;
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
    let pod_subnet = cluster
        .pod_subnet
        .as_deref()
        .unwrap_or(DEFAULT_POD_SUBNET);
    let service_subnet = cluster
        .service_subnet
        .as_deref()
        .unwrap_or(DEFAULT_SERVICE_SUBNET);

    info!(%advertise, %hostname, %k8s_ver, "generating control-plane PKI");
    let pki = pki::generate_pki(&advertise, &hostname, &endpoint_host)?;
    pki::write_pki(&paths.pki(), &paths.etcd_pki(), &pki)?;

    let admin = kubeconfig::render_kubeconfig(
        &cluster.endpoint,
        &pki.ca_crt,
        &pki.admin_crt,
        &pki.admin_key,
        "kubernetes-admin",
    );
    let local_admin = kubeconfig::render_kubeconfig(
        "https://127.0.0.1:6443",
        &pki.ca_crt,
        &pki.admin_crt,
        &pki.admin_key,
        "kubernetes-admin",
    );
    let cm_conf = kubeconfig::render_kubeconfig(
        "https://127.0.0.1:6443",
        &pki.ca_crt,
        &pki.cm_crt,
        &pki.cm_key,
        "system:kube-controller-manager",
    );
    let sched_conf = kubeconfig::render_kubeconfig(
        "https://127.0.0.1:6443",
        &pki.ca_crt,
        &pki.sched_crt,
        &pki.sched_key,
        "system:kube-scheduler",
    );
    let kubelet_conf = kubeconfig::render_kubeconfig(
        "https://127.0.0.1:6443",
        &pki.ca_crt,
        &pki.kubelet_crt,
        &pki.kubelet_key,
        &format!("system:node:{hostname}"),
    );

    fs::write(paths.admin_kubeconfig(), &admin)?;
    fs::write(paths.kubeconfig_dir().join("controller-manager.conf"), &cm_conf)?;
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
    static_pods::write_static_pods(
        &paths.manifests(),
        &static_pods::StaticPodParams {
            advertise_ip: &advertise,
            kubernetes_version: k8s_ver,
            etcd_image: DEFAULT_ETCD_IMAGE,
            service_cidr: service_subnet,
            pod_subnet,
            pki_host_path: "/etc/kubernetes/pki",
        },
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

    if let Some(token) = cluster.token.as_deref() {
        // Best-effort: create bootstrap token Secret once API is up.
        let admin_path = paths.admin_kubeconfig();
        let token = token.to_string();
        let ca = pki.ca_crt.clone();
        thread::spawn(move || {
            if let Err(err) = ensure_bootstrap_token_when_ready(&admin_path, &token, &ca) {
                tracing::warn!(error = %err, "bootstrap token Secret not created yet; use join-config after API is up");
            }
        });
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
    let raw =
        fs::read_to_string(paths.admin_kubeconfig()).context("admin kubeconfig missing; bootstrap first")?;
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

fn endpoint_host(endpoint: &str) -> String {
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

fn detect_advertise_ip() -> Option<String> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("1.1.1.1:80").ok()?;
    let ip = sock.local_addr().ok()?.ip();
    if ip.is_loopback() || ip.is_unspecified() {
        return None;
    }
    Some(ip.to_string())
}

fn chrono_like_now() -> String {
    use std::time::SystemTime;
    match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => format!("{}s", d.as_secs()),
        Err(_) => "unknown".into(),
    }
}

fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
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

fn ensure_bootstrap_token_when_ready(
    _admin_kubeconfig: &Path,
    token: &str,
    _ca_pem: &str,
) -> Result<()> {
    let Some((id, secret)) = token::split_token(token) else {
        bail!("invalid bootstrap token format");
    };
    // Wait for local apiserver; creating the Secret requires a kube client.
    // For Phase A we write a well-known file the operator/docs can apply, and
    // also attempt a minimal HTTPS POST if possible later.
    let deadline = Instant::now() + Duration::from_secs(180);
    while Instant::now() < deadline {
        if apiserver_tcp_ready(Duration::from_secs(2)) {
            break;
        }
        thread::sleep(Duration::from_secs(3));
    }

    // Persist token material for join-config / manual Secret apply.
    let dir = Path::new("/var/lib/pertisk/kubernetes");
    fs::create_dir_all(dir).ok();
    let body = format!(
        "apiVersion: v1\nkind: Secret\nmetadata:\n  name: bootstrap-token-{id}\n  namespace: kube-system\ntype: bootstrap.kubernetes.io/token\nstringData:\n  description: pertisk bootstrap token\n  token-id: \"{id}\"\n  token-secret: \"{secret}\"\n  usage-bootstrap-authentication: \"true\"\n  usage-bootstrap-signing: \"true\"\n  auth-extra-groups: system:bootstrappers:kubeadm:default-node-token\n"
    );
    fs::write(dir.join("bootstrap-token-secret.yaml"), body)?;
    info!("wrote bootstrap-token-secret.yaml — apply with: kubectl apply -f /var/lib/pertisk/kubernetes/bootstrap-token-secret.yaml");
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
        let out = patch_worker_ca(yaml, "-----BEGIN CERTIFICATE-----\nX\n-----END CERTIFICATE-----")
            .unwrap();
        assert!(out.contains("dashboard"));
        assert!(out.contains("catppuccin"));
        assert!(out.contains("BEGIN CERTIFICATE"));
    }
}
