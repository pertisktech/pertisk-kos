//! Spawn and babysit kubelet.

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use pertisk_config::{Cluster, MachineConfig};
use thiserror::Error;
use tracing::{info, warn};

use crate::cni::ensure_loopback_cni;
use crate::config::{write_kubeconfig, write_kubelet_config};
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
        let restarted = start_kubelet(&self.paths, cfg)?;
        self.child = restarted.child;
        Ok(())
    }

    pub fn stop(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Prepare configs and spawn kubelet. Requires `cluster` in machine config.
pub fn start_kubelet(paths: &KubeletPaths, cfg: &MachineConfig) -> Result<KubeletHandle, KubeletError> {
    if !paths.binary.exists() {
        return Err(KubeletError::MissingBinary(paths.binary.display().to_string()));
    }

    let cluster = cfg
        .cluster
        .as_ref()
        .ok_or_else(|| KubeletError::Msg("cluster config required for kubelet".into()))?;

    prepare_kubelet(paths, cfg, cluster)?;

    let container_runtime_endpoint = "unix:///run/containerd/containerd.sock";
    info!(bin = %paths.binary.display(), "starting kubelet");

    let child = Command::new(&paths.binary)
        .arg(format!("--config={}", paths.config.display()))
        .arg(format!("--kubeconfig={}", paths.kubeconfig.display()))
        .arg(format!("--container-runtime-endpoint={container_runtime_endpoint}"))
        .arg(format!("--root-dir={}", paths.root_dir.display()))
        .arg("--v=2")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()?;

    // Give kubelet a moment; registration is async against the API server.
    std::thread::sleep(Duration::from_millis(200));

    let mut handle = KubeletHandle {
        paths: paths.clone(),
        child,
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
    ensure_loopback_cni(paths)?;
    // Static pod manifest dir expected by config.
    std::fs::create_dir_all("/etc/kubernetes/manifests").ok();
    Ok(())
}
