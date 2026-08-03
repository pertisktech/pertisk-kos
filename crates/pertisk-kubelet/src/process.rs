//! Spawn and babysit kubelet.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

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
    // Bridge mode needs an explicit pod CIDR; cluster CNI assigns via Node.Spec.PodCIDR.
    if cluster.cni == pertisk_config::CniMode::Bridge {
        let pod_cidr = cluster.pod_cidr.as_deref().unwrap_or(DEFAULT_POD_CIDR);
        cmd.arg(format!("--pod-cidr={pod_cidr}"));
    }

    let mut child = cmd.spawn()?;
    if let Some(stderr) = child.stderr.take() {
        spawn_stderr_tee(stderr, kubelet_log_path(), "kubelet", log_sink.clone());
    }

    // Give kubelet a moment; registration is async against the API server.
    std::thread::sleep(Duration::from_millis(200));

    let mut handle = KubeletHandle {
        paths: paths.clone(),
        child,
        log_sink,
    };
    if !handle.is_alive() {
        return Err(KubeletError::Msg("kubelet exited immediately".into()));
    }
    info!(pid = handle.pid(), "kubelet started");
    Ok(handle)
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
    )?;

    if is_cp && have_cp_creds {
        // Cert kubeconfig from bootstrap / restore — no token bootstrap.
        if live_cp.is_file() {
            fs::copy(live_cp, &paths.kubeconfig)?;
        }
        let _ = fs::remove_file(&paths.bootstrap_kubeconfig);
    } else if !is_cp {
        // Token → CSR → node cert. Remove any stale token kubeconfig so kubelet
        // is forced through --bootstrap-kubeconfig.
        write_bootstrap_kubeconfig(paths, cluster)?;
        let _ = fs::remove_file(&paths.kubeconfig);
    } else {
        // Control-plane without restored credentials: do not delete an existing
        // kubeconfig; try bootstrap as last resort so apply-before-bootstrap works.
        write_bootstrap_kubeconfig(paths, cluster)?;
        if !paths.kubeconfig.is_file() {
            // Kubelet --kubeconfig must exist even when bootstrapping.
            fs::copy(&paths.bootstrap_kubeconfig, &paths.kubeconfig)?;
        }
        warn!("control-plane starting without /etc/kubernetes/kubelet.conf; restore may be missing");
    }
    let pod_cidr = cluster.pod_cidr.as_deref().unwrap_or(DEFAULT_POD_CIDR);
    ensure_cni_mode(paths, cluster.cni, pod_cidr)?;
    std::fs::create_dir_all("/etc/kubernetes/manifests").ok();
    Ok(())
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
