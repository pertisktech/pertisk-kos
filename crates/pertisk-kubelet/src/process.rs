//! Spawn and babysit kubelet.

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use pertisk_config::{Cluster, MachineConfig};
use thiserror::Error;
use tracing::{info, warn};

use crate::cni::{ensure_cni_mode, DEFAULT_POD_CIDR};
use crate::config::{write_kubeconfig, write_kubelet_config};
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
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

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
    write_kubelet_config(paths, cfg.machine.network.hostname.as_deref())?;
    write_kubeconfig(paths, cluster)?;
    let pod_cidr = cluster.pod_cidr.as_deref().unwrap_or(DEFAULT_POD_CIDR);
    ensure_cni_mode(paths, cluster.cni, pod_cidr)?;
    // Static pod manifest dir expected by config.
    std::fs::create_dir_all("/etc/kubernetes/manifests").ok();
    Ok(())
}
