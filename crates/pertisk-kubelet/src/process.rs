//! Spawn and babysit kubelet.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use pertisk_config::{Cluster, MachineConfig, MachineType};
use thiserror::Error;
use tracing::{info, warn};

use crate::cni::{ensure_cni_mode, DEFAULT_POD_CIDR};
use crate::config::{write_bootstrap_kubeconfig, write_kubelet_config};
use crate::log_tee::{ensure_var_log, kubelet_log_path, spawn_stderr_tee, LineSink};
use crate::paths::KubeletPaths;

#[derive(Debug, Error)]
pub enum KubeletError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("kubelet binary not found at {0}")]
    MissingBinary(String),
    #[error("{0}")]
    Msg(String),
}

pub struct KubeletHandle {
    pub paths: KubeletPaths,
    child: Child,
    log_sink: Option<LineSink>,
}

impl KubeletHandle {
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn is_alive(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(None) => true,
            Ok(Some(status)) => {
                warn!(status = ?status, "kubelet exited");
                false
            }
            Err(err) => {
                warn!(error = %err, "kubelet wait failed");
                false
            }
        }
    }

    pub fn ensure_alive(&mut self, cfg: &MachineConfig) -> Result<(), KubeletError> {
        if self.is_alive() {
            return Ok(());
        }
        warn!("restarting kubelet");
        let restarted = start_kubelet_with_sink(&self.paths, cfg, self.log_sink.clone())?;
        self.child = restarted.child;
        Ok(())
    }

    pub fn stop(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Prepare configs and spawn kubelet. Requires `cluster` in machine config.
pub fn start_kubelet(
    paths: &KubeletPaths,
    cfg: &MachineConfig,
) -> Result<KubeletHandle, KubeletError> {
    start_kubelet_with_sink(paths, cfg, None)
}

/// Like [`start_kubelet`], teeing stderr to `log_sink` (caller applies prefix).
pub fn start_kubelet_with_sink(
    paths: &KubeletPaths,
    cfg: &MachineConfig,
    log_sink: Option<LineSink>,
) -> Result<KubeletHandle, KubeletError> {
    start_kubelet_inner(paths, cfg, log_sink, Duration::from_secs(60))
}

/// One spawn + liveness check. The supervise loop calls this so DHCP is not blocked.
pub fn try_start_kubelet_with_sink(
    paths: &KubeletPaths,
    cfg: &MachineConfig,
    log_sink: Option<LineSink>,
) -> Result<KubeletHandle, KubeletError> {
    start_kubelet_inner(paths, cfg, log_sink, Duration::ZERO)
}

fn start_kubelet_inner(
    paths: &KubeletPaths,
    cfg: &MachineConfig,
    log_sink: Option<LineSink>,
    retry_budget: Duration,
) -> Result<KubeletHandle, KubeletError> {
    if !paths.binary.exists() {
        return Err(KubeletError::MissingBinary(
            paths.binary.display().to_string(),
        ));
    }

    let cluster = cfg
        .cluster
        .as_ref()
        .ok_or_else(|| KubeletError::Msg("cluster config required for kubelet".into()))?;

    prepare_kubelet(paths, cfg, cluster)?;
    ensure_var_log();

    let container_runtime_endpoint = "unix:///run/containerd/containerd.sock";
    info!(bin = %paths.binary.display(), cni = %cluster.cni.as_str(), "starting kubelet");

    let mut cmd = kubelet_command(paths, cfg, cluster, container_runtime_endpoint);

    // containerd publishes its socket before the CRI plugin serves Version();
    // kubelet then exits with "server is not initialized yet" and pertiskd
    // used to leave kubelet=absent with no retry.
    let deadline = Instant::now() + retry_budget;
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        let mut child = cmd.spawn()?;
        if let Some(stderr) = child.stderr.take() {
            spawn_stderr_tee(stderr, kubelet_log_path(), "kubelet", log_sink.clone());
        }

        std::thread::sleep(Duration::from_millis(400));

        let mut handle = KubeletHandle {
            paths: paths.clone(),
            child,
            log_sink: log_sink.clone(),
        };
        if handle.is_alive() {
            info!(pid = handle.pid(), attempt, "kubelet started");
            return Ok(handle);
        }
        if Instant::now() >= deadline {
            return Err(KubeletError::Msg(
                "kubelet exited immediately (CRI not ready)".into(),
            ));
        }
        warn!(
            attempt,
            "kubelet exited immediately; retrying after CRI settles"
        );
        std::thread::sleep(Duration::from_millis(500));
        cmd = kubelet_command(paths, cfg, cluster, container_runtime_endpoint);
    }
}

fn kubelet_command(
    paths: &KubeletPaths,
    cfg: &MachineConfig,
    cluster: &Cluster,
    container_runtime_endpoint: &str,
) -> Command {
    let mut cmd = Command::new(&paths.binary);
    cmd.arg(format!("--config={}", paths.config.display()))
        .arg(format!("--kubeconfig={}", paths.kubeconfig.display()))
        .arg(format!(
            "--container-runtime-endpoint={container_runtime_endpoint}"
        ))
        .arg(format!("--root-dir={}", paths.root_dir.display()))
        .arg("--v=2")
        .env("SSL_CERT_FILE", "/etc/ssl/certs/ca-certificates.crt")
        .env("PATH", "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    // Workers: exchange bootstrap token for a node client cert.
    if cfg.machine.machine_type != MachineType::Controlplane && paths.bootstrap_kubeconfig.is_file()
    {
        cmd.arg(format!(
            "--bootstrap-kubeconfig={}",
            paths.bootstrap_kubeconfig.display()
        ));
    }

    // hostnameOverride is NOT a KubeletConfiguration field — must be a flag.
    if let Some(name) = cfg.machine.network.hostname.as_deref() {
        if !name.is_empty() {
            cmd.arg(format!("--hostname-override={name}"));
        }
    }
    // Dual-stack requires explicit --node-ip=v4,v6 or kubelet stays IPv4-only.
    // IPv4-only still needs --node-ip when the guest has no default route
    // (isolated Proxmox bridge / no gateway): kubelet otherwise never
    // registers and static-pod apiserver looks down (`node not found`).
    if cluster.is_dual_stack() {
        if let Some(node_ip) = dual_stack_node_ip_for(cfg) {
            info!(%node_ip, "kubelet dual-stack --node-ip");
            cmd.arg(format!("--node-ip={node_ip}"));
        } else {
            warn!("dual-stack enabled but no v4+v6 on primary iface; kubelet may stay IPv4-only");
        }
    } else if let Some(node_ip) = ipv4_node_ip_for(cfg) {
        info!(%node_ip, "kubelet --node-ip");
        cmd.arg(format!("--node-ip={node_ip}"));
    }
    // Bridge mode needs an explicit pod CIDR; cluster CNI assigns via Node.Spec.PodCIDR.
    if cluster.cni == pertisk_config::CniMode::Bridge {
        let pod_cidr = cluster.pod_cidr.as_deref().unwrap_or(DEFAULT_POD_CIDR);
        cmd.arg(format!("--pod-cidr={pod_cidr}"));
    }
    cmd
}

fn prepare_kubelet(
    paths: &KubeletPaths,
    cfg: &MachineConfig,
    cluster: &Cluster,
) -> Result<(), KubeletError> {
    let is_cp = cfg.machine.machine_type == MachineType::Controlplane;
    let live_cp = Path::new("/etc/kubernetes/kubelet.conf");
    // Prefer live CP cert kubeconfig; also accept an already-published
    // /var/lib/kubelet/kubeconfig (EPHEMERAL) so a late restore still works.
    let have_cp_creds = live_cp.is_file() || (is_cp && paths.kubeconfig.is_file());
    let tls_bootstrap = !is_cp || !have_cp_creds;

    // Apiserver proxies logs/exec with its kubelet-client cert; kubelet must
    // trust that cert via authentication.x509.clientCAFile or requests are 401.
    ensure_client_ca(paths, cluster)?;

    write_kubelet_config(
        paths,
        cfg.machine.network.hostname.as_deref(),
        tls_bootstrap,
        cfg.machine.max_pods(),
    )?;

    if is_cp && have_cp_creds {
        // Cert kubeconfig from bootstrap / restore — no token bootstrap.
        if live_cp.is_file() {
            fs::copy(live_cp, &paths.kubeconfig)?;
        }
        let _ = fs::remove_file(&paths.bootstrap_kubeconfig);
    } else if !is_cp {
        // Keep an already-issued client-cert kubeconfig across reboot. Deleting it
        // forced TLS bootstrap every start; after the join token expired the Node
        // stayed issued in the API but kubelet never became Ready.
        write_bootstrap_kubeconfig(paths, cluster)?;
        if !keep_or_recover_issued_kubeconfig(paths, cluster) {
            let _ = fs::remove_file(&paths.kubeconfig);
        }
    } else {
        // Control-plane without restored credentials: do not delete an existing
        // kubeconfig; try bootstrap as last resort so apply-before-bootstrap works.
        write_bootstrap_kubeconfig(paths, cluster)?;
        if !paths.kubeconfig.is_file() {
            // Kubelet --kubeconfig must exist even when bootstrapping.
            fs::copy(&paths.bootstrap_kubeconfig, &paths.kubeconfig)?;
        }
        warn!(
            "control-plane starting without /etc/kubernetes/kubelet.conf; restore may be missing"
        );
    }
    let pod_cidr = cluster.pod_cidr.as_deref().unwrap_or(DEFAULT_POD_CIDR);
    ensure_cni_mode(paths, cluster.cni, pod_cidr)?;
    std::fs::create_dir_all("/etc/kubernetes/manifests").ok();
    Ok(())
}

fn kubeconfig_has_client_cert(raw: &str) -> bool {
    let has_cert = raw.contains("client-certificate-data:") || raw.contains("client-certificate:");
    if !has_cert {
        return false;
    }
    !(raw.contains("name: kubelet-bootstrap") && raw.contains("token:"))
}

/// Reuse the issued node cert after shutdown/boot (kubeconfig and/or kubelet PKI).
fn keep_or_recover_issued_kubeconfig(paths: &KubeletPaths, cluster: &Cluster) -> bool {
    if paths.kubeconfig.is_file() {
        if let Ok(raw) = fs::read_to_string(&paths.kubeconfig) {
            if kubeconfig_has_client_cert(&raw) {
                info!("keeping issued kubelet kubeconfig");
                return true;
            }
        }
    }
    recover_issued_kubeconfig_from_pki(paths, cluster)
}

fn recover_issued_kubeconfig_from_pki(paths: &KubeletPaths, cluster: &Cluster) -> bool {
    let pki = paths.root_dir.join("pki");
    let pem = pki.join("kubelet-client-current.pem");
    let crt = pki.join("kubelet-client.crt");
    let key = pki.join("kubelet-client.key");
    let (cert_path, key_path) = if pem.is_file() && key.is_file() {
        (pem.clone(), key)
    } else if pem.is_file() {
        (pem.clone(), pem)
    } else if crt.is_file() && key.is_file() {
        (crt, key)
    } else {
        return false;
    };
    let endpoint = cluster.endpoint.trim();
    if endpoint.is_empty() {
        return false;
    }
    let ca_block = if paths.ca_file.is_file() {
        format!("    certificate-authority: {}\n", paths.ca_file.display())
    } else {
        String::new()
    };
    let body = format!(
        r#"apiVersion: v1
kind: Config
clusters:
- cluster:
{ca_block}    server: {endpoint}
  name: default-cluster
users:
- name: default-auth
  user:
    client-certificate: {cert}
    client-key: {key}
contexts:
- context:
    cluster: default-cluster
    user: default-auth
  name: default-auth
current-context: default-auth
"#,
        ca_block = ca_block,
        endpoint = endpoint,
        cert = cert_path.display(),
        key = key_path.display(),
    );
    match fs::write(&paths.kubeconfig, body) {
        Ok(()) => {
            info!("recovered issued kubelet kubeconfig from PKI");
            true
        }
        Err(err) => {
            warn!(error = %err, "failed to recover kubelet kubeconfig from PKI");
            false
        }
    }
}

fn ipv4_node_ip_for(cfg: &MachineConfig) -> Option<String> {
    let skip = cluster_vip_skips(cfg);
    let skip_refs: Vec<&str> = skip.iter().map(|s| s.as_str()).collect();
    let iface = cfg
        .machine
        .network
        .interfaces
        .iter()
        .map(|i| i.interface.as_str())
        .find(|n| !n.is_empty() && *n != "lo")
        .unwrap_or("eth0");
    if !pertisk_net::is_virtual_iface(iface) {
        if let Ok(addrs) = pertisk_net::list_addresses(iface) {
            if let Some(ip) =
                pertisk_net::pick_node_ipv4(addrs.iter().map(|s| s.as_str()), &skip_refs)
            {
                return Some(ip);
            }
        }
    }
    pertisk_net::first_global_ipv4_skip(&skip_refs)
}

fn cluster_vip_skips(cfg: &MachineConfig) -> Vec<String> {
    let Some(cluster) = cfg.cluster.as_ref() else {
        return Vec::new();
    };
    let rest = cluster
        .endpoint
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let hostport = rest.split('/').next().unwrap_or(rest);
    let host = if let Some(inner) = hostport.strip_prefix('[') {
        inner.split(']').next().unwrap_or(inner)
    } else {
        hostport.split(':').next().unwrap_or(hostport)
    };
    if host.parse::<std::net::Ipv4Addr>().is_ok() {
        vec![host.to_string()]
    } else {
        Vec::new()
    }
}

fn dual_stack_node_ip_for(cfg: &MachineConfig) -> Option<String> {
    let iface = cfg
        .machine
        .network
        .interfaces
        .iter()
        .map(|i| i.interface.as_str())
        .find(|n| !n.is_empty() && *n != "lo")
        .unwrap_or("eth0");
    pertisk_net::dual_stack_node_ip(iface).or_else(|| {
        // Fall back to any non-loopback iface that already has v4+v6 (ULA).
        let ifaces = pertisk_net::list_interfaces().ok()?;
        for name in ifaces {
            if name == "lo" {
                continue;
            }
            if let Some(ip) = pertisk_net::dual_stack_node_ip(&name) {
                return Some(ip);
            }
        }
        None
    })
}

/// Ensure `/var/lib/kubelet/ca.crt` exists for `authentication.x509.clientCAFile`.
fn ensure_client_ca(paths: &KubeletPaths, cluster: &Cluster) -> Result<(), KubeletError> {
    paths.ensure_dirs()?;
    if paths.ca_file.is_file() {
        return Ok(());
    }
    // Prefer live control-plane CA, then cluster config CA blob.
    let live_ca = Path::new("/etc/kubernetes/pki/ca.crt");
    if live_ca.is_file() {
        fs::copy(live_ca, &paths.ca_file)?;
        return Ok(());
    }
    if let Some(ca) = &cluster.ca {
        let mut f = fs::File::create(&paths.ca_file)?;
        f.write_all(ca.as_bytes())?;
        if !ca.ends_with('\n') {
            f.write_all(b"\n")?;
        }
        return Ok(());
    }
    warn!(
        path = %paths.ca_file.display(),
        "kubelet client CA missing; apiserver log/exec proxy may return 401"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pertisk-kubelet-process-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn issued_kubeconfig_is_kept() {
        assert!(kubeconfig_has_client_cert(
            "users:\n- name: default-auth\n  user:\n    client-certificate: /var/lib/kubelet/pki/kubelet-client-current.pem\n"
        ));
        assert!(!kubeconfig_has_client_cert(
            "users:\n- name: kubelet-bootstrap\n  user:\n    token: \"abc.def\"\n"
        ));
    }

    #[test]
    fn recover_kubeconfig_from_current_pem() {
        let dir = temp_dir();
        let paths = KubeletPaths::with_prefix(&dir);
        paths.ensure_dirs().unwrap();
        fs::create_dir_all(paths.root_dir.join("pki")).unwrap();
        fs::write(
            paths.root_dir.join("pki/kubelet-client-current.pem"),
            "CERT\n",
        )
        .unwrap();
        fs::write(paths.root_dir.join("pki/kubelet-client.key"), "KEY\n").unwrap();
        fs::write(&paths.ca_file, "CA\n").unwrap();
        let cluster = Cluster {
            name: Some("lab".into()),
            endpoint: "https://10.1.1.254:6443".into(),
            token: Some("expired.token".into()),
            ca: None,
            ca_key: None,
            sa_key: None,
            network: None,
            pod_subnet: None,
            service_subnet: None,
            pod_cidr_ipv6: None,
            service_cidr_ipv6: None,
            network_mode: Default::default(),
            vip6: None,
            kubernetes_version: None,
            pod_cidr: None,
            cni: Default::default(),
            cert_sans: vec![],
        };
        assert!(recover_issued_kubeconfig_from_pki(&paths, &cluster));
        let kc = fs::read_to_string(&paths.kubeconfig).unwrap();
        assert!(kubeconfig_has_client_cert(&kc));
        assert!(kc.contains("https://10.1.1.254:6443"));
        assert!(kc.contains("kubelet-client-current.pem"));
        assert!(kc.contains("kubelet-client.key"));
        assert!(!kc.contains("client-certificate: /var/lib/kubelet/pki/kubelet-client.key"));
        let _ = fs::remove_dir_all(&dir);
    }
}
