//! Start and babysit containerd + kubelet (+ QEMU guest agent when present).

use anyhow::Result;
use pertisk_config::{MachineConfig, MachineType};
use pertisk_kubelet::{
    ensure_kubelet_version, start_kubelet_with_sink, try_start_kubelet_with_sink, KubeletHandle,
    KubeletPaths,
};
use pertisk_runtime::{start_containerd_with_sink, ContainerdHandle, RuntimePaths};
use tracing::{info, warn};

use crate::guest_agent::{self, GuestAgentHandle};
use crate::log_ring::LogRing;

fn has_global_ipv4() -> bool {
    let ifaces = pertisk_net::list_interfaces().unwrap_or_default();
    for name in ifaces {
        let Ok(addrs) = pertisk_net::list_addresses(&name) else {
            continue;
        };
        if addrs.iter().any(|a| {
            let ip = a.split('/').next().unwrap_or(a.as_str());
            ip.contains('.') && !ip.starts_with("127.") && !ip.starts_with("169.254.")
        }) {
            return true;
        }
    }
    false
}

/// Static pods (etcd) advertise the DHCP address. Starting kubelet before the
/// NIC has an address leaves HA members unable to form quorum after a cluster
/// reboot (`127.0.0.1:6443 connection refused`, nodes never register).
fn wait_for_ipv4(timeout: std::time::Duration) {
    let deadline = std::time::Instant::now() + timeout;
    if has_global_ipv4() {
        return;
    }
    info!(
        timeout_s = timeout.as_secs(),
        "waiting for guest IPv4 before kubelet"
    );
    loop {
        if has_global_ipv4() {
            info!("guest IPv4 is up; starting kubelet");
            return;
        }
        if std::time::Instant::now() >= deadline {
            warn!(
                timeout_s = timeout.as_secs(),
                "no guest IPv4 yet; starting kubelet anyway"
            );
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
}

pub struct NodeServices {
    pub containerd: Option<ContainerdHandle>,
    pub kubelet: Option<KubeletHandle>,
    pub guest_agent: Option<GuestAgentHandle>,
    kubelet_retry_warn_at: Option<std::time::Instant>,
}

impl NodeServices {
    pub fn empty(guest_agent: Option<GuestAgentHandle>) -> Self {
        Self {
            containerd: None,
            kubelet: None,
            guest_agent,
            kubelet_retry_warn_at: None,
        }
    }

    /// Attempt to start runtime services. Missing binaries are soft-warned.
    pub fn start(cfg: &MachineConfig, logs: &LogRing) -> Result<Self> {
        Self::start_with_guest_agent(cfg, logs, guest_agent::start())
    }

    /// Like [`start`] but reuses an already-running qemu-ga (early boot).
    pub fn start_with_guest_agent(
        cfg: &MachineConfig,
        logs: &LogRing,
        guest_agent: Option<GuestAgentHandle>,
    ) -> Result<Self> {
        let mut services = Self {
            containerd: None,
            kubelet: None,
            guest_agent,
            kubelet_retry_warn_at: None,
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
            wait_for_ipv4(std::time::Duration::from_secs(90));
            if !has_global_ipv4() {
                warn!("deferring kubelet until guest IPv4 is up");
            } else {
                match start_kubelet_with_sink(
                    &KubeletPaths::default(),
                    cfg,
                    Some(logs.sink("kubelet")),
                ) {
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
            }
        } else if cfg.cluster.is_none() {
            info!("no cluster config; kubelet not started");
        }

        Ok(services)
    }

    pub fn ensure_alive(&mut self, cfg: &MachineConfig) {
        // Cilium Bidirectional hostPath can tear down host /proc; heal before
        // restarting runtimes that depend on /proc/<pid>/ns/*.
        if let Err(err) = crate::linux::ensure_proc_readable() {
            warn!(error = %err, "ensure /proc failed");
        }
        if let Some(ref mut ga) = self.guest_agent {
            ga.ensure_alive();
        }
        if let Some(ref mut cd) = self.containerd {
            if let Err(err) = cd.ensure_alive() {
                warn!(error = %err, "containerd restart failed");
            }
        }
        // After reboot kubelet can lose the CRI race (containerd socket exists,
        // plugin not ready). Start/retry even when the handle was never stored.
        if self.kubelet.is_none() && self.containerd.is_some() && cfg.cluster.is_some() {
            if has_global_ipv4() {
                match try_start_kubelet_with_sink(
                    &KubeletPaths::default(),
                    cfg,
                    Some(crate::log_ring().sink("kubelet")),
                ) {
                    Ok(handle) => {
                        info!(pid = handle.pid(), "kubelet started after earlier failure");
                        self.kubelet = Some(handle);
                        self.kubelet_retry_warn_at = None;
                    }
                    Err(err) => {
                        let now = std::time::Instant::now();
                        let noisy = self.kubelet_retry_warn_at.is_none_or(|t| {
                            now.duration_since(t) >= std::time::Duration::from_secs(5)
                        });
                        if noisy {
                            warn!(error = %err, "kubelet retry failed");
                            self.kubelet_retry_warn_at = Some(now);
                        }
                    }
                }
            }
        }
        if let Some(ref mut kl) = self.kubelet {
            if let Err(err) = kl.ensure_alive(cfg) {
                warn!(error = %err, "kubelet restart failed");
            }
        }
    }

    /// After `pertiskctl apply`, start kubelet if cluster config is now present.
    /// When `kubernetesVersion` changes: bump static-pod images (CP) and replace kubelet.
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

        let mut kubelet_needs_restart = false;
        if let Some(cluster) = cfg.cluster.as_ref() {
            if let Some(ver) = cluster
                .kubernetes_version
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                if matches!(cfg.machine.machine_type, MachineType::Controlplane) {
                    match pertisk_bootstrap::static_pods::bump_control_plane_images(
                        std::path::Path::new("/etc/kubernetes/manifests"),
                        ver,
                    ) {
                        Ok(0) => {}
                        Ok(n) => {
                            info!(files = n, version = %ver, "bumped control-plane static pod images")
                        }
                        Err(err) => warn!(error = %err, "static pod image bump failed"),
                    }
                }
                match ensure_kubelet_version(&KubeletPaths::default(), ver) {
                    Ok(true) => {
                        info!(version = %ver, "kubelet binary upgraded; will restart");
                        kubelet_needs_restart = true;
                    }
                    Ok(false) => {}
                    Err(err) => warn!(error = %err, "kubelet binary upgrade failed"),
                }
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
        } else if kubelet_needs_restart {
            self.restart_kubelet(cfg, logs);
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
