//! PID 1 supervise loop: reap zombies, babysit services, honor API power actions.

use anyhow::Result;
use pertisk_api::{apply_loki_push, apply_prom_push, PowerAction, SharedState};
use pertisk_config::MachineConfig;
use tracing::{info, warn};

use crate::services::NodeServices;

/// Block forever until stop signal or API-requested power action.
pub fn supervise(
    cfg: Option<MachineConfig>,
    mut services: NodeServices,
    state: SharedState,
) -> Result<()> {
    #[cfg(unix)]
    {
        unix_impl::supervise(cfg, &mut services, state)
    }
    #[cfg(not(unix))]
    {
        let _ = (cfg, services, state);
        anyhow::bail!("pertiskd supervise loop requires Unix")
    }
}

#[cfg(unix)]
mod unix_impl {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    use nix::sys::signal::{sigaction, SaFlags, SigAction, SigHandler, SigSet, Signal};
    use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
    use nix::unistd::Pid;

    static STOP: AtomicBool = AtomicBool::new(false);

    extern "C" fn handle_stop(_: nix::libc::c_int) {
        STOP.store(true, Ordering::SeqCst);
    }

    extern "C" fn handle_chld(_: nix::libc::c_int) {}

    pub fn supervise(
        mut cfg: Option<MachineConfig>,
        services: &mut NodeServices,
        state: SharedState,
    ) -> Result<()> {
        install_handlers()?;
        refresh_state(services, &state);
        info!(status = %services.status_summary(), "supervise loop entered");

        let mut dhcp_retry_at = std::time::Instant::now();
        let mut last_rebase_at = std::time::Instant::now() - std::time::Duration::from_secs(30);
        let mut dhcp_ok = iface_has_address(cfg.as_ref());

        while !STOP.load(Ordering::SeqCst) {
            reap_zombies();

            let reload = state
                .lock()
                .map(|mut s| {
                    let r = s.config_reload;
                    if r {
                        s.config_reload = false;
                    }
                    r
                })
                .unwrap_or(false);
            if reload {
                let (path, state_root) = state
                    .lock()
                    .map(|s| (s.config_path.clone(), s.state_root.clone()))
                    .unwrap_or_default();
                match std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|y| MachineConfig::from_yaml(&y).ok())
                {
                    Some(new_cfg) => {
                        info!(
                            machine_type = ?new_cfg.machine.machine_type,
                            has_cluster = new_cfg.cluster.is_some(),
                            "reloading machine config after apply"
                        );
                        crate::dashboard::apply_config(Some(&new_cfg));
                        let dual = new_cfg
                            .cluster
                            .as_ref()
                            .map(|c| c.is_dual_stack())
                            .unwrap_or(false);
                        crate::sysctl::apply_ipv6_policy(dual);
                        if let Some(name) = new_cfg.machine.network.hostname.as_deref() {
                            if let Err(err) = crate::hostname::set_hostname(name) {
                                warn!(error = %err, "hostname apply on reload failed");
                            }
                        }
                        if let Err(err) = pertisk_net::apply_network(&new_cfg.machine.network) {
                            warn!(error = %err, "network apply on config reload failed");
                        }
                        services.on_config_reload(&new_cfg, crate::log_ring());
                        apply_loki_push(Some(&new_cfg), &state_root);
                        apply_prom_push(Some(&new_cfg), state.clone());
                        // Dual-stack needs kubelet --node-ip=v4,v6 after ULA/SLAAC lands.
                        if dual && new_cfg.cluster.is_some() {
                            info!("restarting kubelet after dual-stack network apply");
                            services.restart_kubelet(&new_cfg, crate::log_ring());
                        }
                        cfg = Some(new_cfg);
                    }
                    None => warn!(path = %path.display(), "config reload failed to parse"),
                }
            }

            let kubelet_reload = state
                .lock()
                .map(|mut s| {
                    let r = s.kubelet_reload;
                    if r {
                        s.kubelet_reload = false;
                    }
                    r
                })
                .unwrap_or(false)
                || pertisk_bootstrap::take_kubelet_reload_request();
            if kubelet_reload {
                if let Some(ref c) = cfg {
                    info!("restarting kubelet after bootstrap");
                    services.restart_kubelet(c, crate::log_ring());
                } else {
                    warn!("kubelet reload requested but no machine config loaded");
                }
            }

            if let Some(ref cfg) = cfg {
                services.ensure_alive(cfg);
            }
            refresh_state(services, &state);

            // Keep trying DHCP if we came up before the NIC/carrier/server was ready.
            // First boot has no machine config yet (apply needs :50000) — still retry
            // default DHCP so AHV IPAM / slow virtio can bind an address.
            if dhcp_ok && last_rebase_at.elapsed() >= std::time::Duration::from_secs(30) {
                last_rebase_at = std::time::Instant::now();
                let state_root = state
                    .lock()
                    .map(|s| s.state_root.clone())
                    .unwrap_or_default();
                if !state_root.as_os_str().is_empty() {
                    match pertisk_bootstrap::maybe_rebase_advertise(&state_root) {
                        Ok(true) => info!("control-plane rebased onto new guest IP"),
                        Ok(false) => {}
                        Err(err) => warn!(error = %err, "advertise rebase failed"),
                    }
                }
            }
            if !dhcp_ok && std::time::Instant::now() >= dhcp_retry_at {
                let mut ok = false;
                match pertisk_net::apply_provider_netcfg() {
                    Ok(true) => ok = true,
                    other => {
                        if let Err(err) = other {
                            warn!(error = %err, "provider netcfg retry failed");
                        }
                        let net = cfg
                            .as_ref()
                            .map(|c| c.machine.network.clone())
                            .unwrap_or_else(pertisk_config::Network::dhcp_default);
                        match pertisk_net::apply_network(&net) {
                            Ok(()) => ok = true,
                            Err(err) => {
                                warn!(error = %err, "DHCP retry failed");
                            }
                        }
                    }
                }
                if ok {
                    dhcp_ok = iface_has_address(cfg.as_ref());
                    if dhcp_ok {
                        info!("network retry succeeded");
                        last_rebase_at =
                            std::time::Instant::now() - std::time::Duration::from_secs(30);
                    } else {
                        warn!("network retry produced no address; will try again");
                    }
                }
                if !dhcp_ok {
                    dhcp_retry_at = std::time::Instant::now() + std::time::Duration::from_secs(15);
                }
            }

            let power = state.lock().map(|s| s.power).unwrap_or(PowerAction::None);
            match power {
                PowerAction::Reboot => {
                    info!("executing API reboot");
                    stop_services(services);
                    do_reboot()?;
                    break;
                }
                PowerAction::Shutdown => {
                    info!("executing API shutdown");
                    stop_services(services);
                    do_shutdown()?;
                    break;
                }
                PowerAction::None => {}
            }

            std::thread::sleep(std::time::Duration::from_millis(500));
        }

        info!("supervise loop exiting");
        stop_services(services);
        Ok(())
    }

    fn refresh_state(services: &mut NodeServices, state: &SharedState) {
        let (cd, kl, cd_pid, kl_pid) = services.status_parts();
        if let Ok(mut st) = state.lock() {
            st.set_runtime_status(cd, kl, cd_pid, kl_pid);
        }
    }

    fn iface_has_address(cfg: Option<&MachineConfig>) -> bool {
        let names: Vec<String> = if let Some(cfg) = cfg {
            cfg.machine
                .network
                .interfaces
                .iter()
                .filter(|i| i.dhcp)
                .map(|i| i.interface.clone())
                .collect()
        } else {
            Vec::new()
        };
        let names = if names.is_empty() {
            pertisk_net::list_interfaces().unwrap_or_default()
        } else {
            names
        };
        for name in names {
            if let Ok(addrs) = pertisk_net::list_addresses(&name) {
                // Need IPv4 for management reachability on typical lab LANs.
                if addrs.iter().any(|a| a.contains('.')) {
                    return true;
                }
            }
        }
        false
    }

    fn stop_services(services: &mut NodeServices) {
        if let Some(cd) = services.containerd.take() {
            cd.stop();
        }
        if let Some(kl) = services.kubelet.take() {
            kl.stop();
        }
        if let Some(ga) = services.guest_agent.take() {
            ga.stop();
        }
    }

    fn do_reboot() -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            use nix::sys::reboot::{reboot, RebootMode};
            let _ = nix::unistd::sync();
            reboot(RebootMode::RB_AUTOBOOT)?;
        }
        #[cfg(not(target_os = "linux"))]
        {
            info!("reboot requested (dev host — exiting process)");
            STOP.store(true, Ordering::SeqCst);
        }
        Ok(())
    }

    fn do_shutdown() -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            use nix::sys::reboot::{reboot, RebootMode};
            let _ = nix::unistd::sync();
            reboot(RebootMode::RB_POWER_OFF)?;
        }
        #[cfg(not(target_os = "linux"))]
        {
            info!("shutdown requested (dev host — exiting process)");
            STOP.store(true, Ordering::SeqCst);
        }
        Ok(())
    }

    fn install_handlers() -> Result<()> {
        let stop = SigAction::new(
            SigHandler::Handler(handle_stop),
            SaFlags::empty(),
            SigSet::empty(),
        );
        let chld = SigAction::new(
            SigHandler::Handler(handle_chld),
            SaFlags::SA_NOCLDSTOP,
            SigSet::empty(),
        );

        unsafe {
            sigaction(Signal::SIGTERM, &stop)?;
            sigaction(Signal::SIGINT, &stop)?;
            sigaction(Signal::SIGQUIT, &stop)?;
            sigaction(Signal::SIGCHLD, &chld)?;
        }
        Ok(())
    }

    fn reap_zombies() {
        loop {
            match waitpid(Pid::from_raw(-1), Some(WaitPidFlag::WNOHANG)) {
                Ok(WaitStatus::Exited(pid, code)) => {
                    info!(pid = pid.as_raw(), code, "reaped child (exit)");
                }
                Ok(WaitStatus::Signaled(pid, sig, _)) => {
                    warn!(pid = pid.as_raw(), signal = %sig, "reaped child (signal)");
                }
                Ok(WaitStatus::StillAlive) | Ok(_) => break,
                Err(nix::errno::Errno::ECHILD) => break,
                Err(err) => {
                    warn!(error = %err, "waitpid failed");
                    break;
                }
            }
        }
    }
}
