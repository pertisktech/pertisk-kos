//! Start and babysit containerd + kubelet.

use anyhow::Result;
use pertisk_config::MachineConfig;
use pertisk_kubelet::{start_kubelet_with_sink, KubeletHandle, KubeletPaths};
use pertisk_runtime::{start_containerd_with_sink, ContainerdHandle, RuntimePaths};
use tracing::{info, warn};

use crate::log_ring::LogRing;

pub struct NodeServices {
    pub containerd: Option<ContainerdHandle>,
    pub kubelet: Option<KubeletHandle>,
}

impl NodeServices {
    /// Attempt to start runtime services. Missing binaries are soft-warned.
    pub fn start(cfg: &MachineConfig, logs: &LogRing) -> Result<Self> {
        let mut services = Self {
            containerd: None,
            kubelet: None,
        };

        match start_containerd_with_sink(&RuntimePaths::default(), Some(logs.sink("containerd"))) {
            Ok(handle) => {
                info!(pid = handle.pid(), "containerd running");
                services.containerd = Some(handle);
            }
            Err(pertisk_runtime::RuntimeError::MissingBinary(path)) => {
                warn!(%path, "containerd binary missing; skip runtime");
            }
            Err(err) => {
                warn!(error = %err, "containerd failed to start");
            }
        }

        if services.containerd.is_some() && cfg.cluster.is_some() {
            match start_kubelet_with_sink(&KubeletPaths::default(), cfg, Some(logs.sink("kubelet")))
            {
                Ok(handle) => {
                    info!(pid = handle.pid(), "kubelet running");
                    services.kubelet = Some(handle);
                }
                Err(pertisk_kubelet::KubeletError::MissingBinary(path)) => {
                    warn!(%path, "kubelet binary missing; skip kubelet");
                }
                Err(err) => {
                    warn!(error = %err, "kubelet failed to start");
                }
            }
        } else if cfg.cluster.is_none() {
            info!("no cluster config; kubelet not started");
        }

        Ok(services)
    }

    pub fn ensure_alive(&mut self, cfg: &MachineConfig) {
        if let Some(ref mut cd) = self.containerd {
            if let Err(err) = cd.ensure_alive() {
                warn!(error = %err, "containerd restart failed");
            }
        }
        if let Some(ref mut kl) = self.kubelet {
            if let Err(err) = kl.ensure_alive(cfg) {
                warn!(error = %err, "kubelet restart failed");
            }
        }
    }

    /// After `pertiskctl apply`, start kubelet if cluster config is now present.
    pub fn on_config_reload(&mut self, cfg: &MachineConfig, logs: &crate::log_ring::LogRing) {
        if self.containerd.is_none() {
            match start_containerd_with_sink(
                &RuntimePaths::default(),
                Some(logs.sink("containerd")),
            ) {
                Ok(handle) => {
                    info!(pid = handle.pid(), "containerd started after config reload");
                    self.containerd = Some(handle);
                }
                Err(err) => warn!(error = %err, "containerd start after reload failed"),
            }
        }
        if self.kubelet.is_none() && self.containerd.is_some() && cfg.cluster.is_some() {
            match start_kubelet_with_sink(&KubeletPaths::default(), cfg, Some(logs.sink("kubelet")))
            {
                Ok(handle) => {
                    info!(pid = handle.pid(), "kubelet started after config reload");
                    self.kubelet = Some(handle);
                }
                Err(err) => warn!(error = %err, "kubelet start after reload failed"),
            }
        } else if cfg.cluster.is_none() {
            info!("config reload: still no cluster block; kubelet not started");
        }
    }

    /// After bootstrap, restart kubelet so it loads cert credentials.
    pub fn restart_kubelet(&mut self, cfg: &MachineConfig, logs: &crate::log_ring::LogRing) {
        if let Some(kl) = self.kubelet.take() {
            info!(pid = kl.pid(), "stopping kubelet for credential reload");
            kl.stop();
        }
        if self.containerd.is_none() {
            warn!("kubelet restart skipped; containerd not running");
            return;
        }
        if cfg.cluster.is_none() {
            warn!("kubelet restart skipped; no cluster config");
            return;
        }
        match start_kubelet_with_sink(&KubeletPaths::default(), cfg, Some(logs.sink("kubelet"))) {
            Ok(handle) => {
                info!(pid = handle.pid(), "kubelet restarted after bootstrap");
                self.kubelet = Some(handle);
            }
            Err(err) => warn!(error = %err, "kubelet restart after bootstrap failed"),
        }
    }

    pub fn status_summary(&mut self) -> String {
        let (cd, kl, _, _) = self.status_parts();
        format!("containerd={cd} kubelet={kl}")
    }

    pub fn status_parts(&mut self) -> (&'static str, &'static str, u32, u32) {
        let (cd, cd_pid) = if let Some(h) = self.containerd.as_mut() {
            if h.is_healthy() {
                ("up", h.pid())
            } else {
                ("down", 0)
            }
        } else {
            ("absent", 0)
        };
        let (kl, kl_pid) = if let Some(h) = self.kubelet.as_mut() {
            if h.is_alive() {
                ("up", h.pid())
            } else {
                ("down", 0)
            }
        } else {
            ("absent", 0)
        };
        (cd, kl, cd_pid, kl_pid)
    }
}
