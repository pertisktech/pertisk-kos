//! gRPC MachineService implementation (mTLS-capable).

use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::MutexGuard;

use pertisk_bootstrap::{
    bootstrap_control_plane, get_join_config, join_control_plane, read_admin_kubeconfig,
};
use pertisk_config::MachineConfig;
use pertisk_proto::machine_service_server::{MachineService, MachineServiceServer};
use pertisk_proto::{
    ApplyConfigurationRequest, ApplyConfigurationResponse, BootstrapRequest, BootstrapResponse,
    GetJoinConfigRequest, GetJoinConfigResponse, HealthRequest, HealthResponse,
    JoinControlPlaneRequest, JoinControlPlaneResponse, KubeconfigRequest, KubeconfigResponse,
    LogsRequest, LogsResponse, MarkBootGoodRequest, MarkBootGoodResponse, RebootRequest,
    RebootResponse, ServiceListRequest, ServiceListResponse, ServiceStatus, ShutdownRequest,
    ShutdownResponse, UpgradeRequest, UpgradeResponse, UpgradeStatusRequest, UpgradeStatusResponse,
    ValidateConfigurationResponse, VersionRequest, VersionResponse,
};
use pertisk_update::{apply_bundle, mark_boot_good, BootMeta, SlotLayout};
use tonic::transport::{Certificate, Identity, Server, ServerTlsConfig};
use tonic::{Request, Response, Status};
use tracing::info;

use crate::logs::tail_logs;
use crate::state::{PowerAction, SharedState};

/// Default management listen address.
pub const DEFAULT_LISTEN: &str = "0.0.0.0:50000";
/// Default Prometheus metrics listen address.
pub const DEFAULT_METRICS_LISTEN: &str = "0.0.0.0:50001";

#[derive(Debug, Clone)]
pub struct TlsPaths {
    pub ca_cert: PathBuf,
    pub server_cert: PathBuf,
    pub server_key: PathBuf,
}

#[derive(Clone)]
struct MachineSvc {
    state: SharedState,
}

#[allow(clippy::result_large_err)]
fn lock(state: &SharedState) -> Result<MutexGuard<'_, crate::state::NodeState>, Status> {
    state
        .lock()
        .map_err(|_| Status::internal("node state lock poisoned"))
}

#[tonic::async_trait]
impl MachineService for MachineSvc {
    async fn version(
        &self,
        _request: Request<VersionRequest>,
    ) -> Result<Response<VersionResponse>, Status> {
        let st = lock(&self.state)?;
        Ok(Response::new(VersionResponse {
            version: st.version.clone(),
            api_version: st.api_version.clone(),
            platform: st.platform.clone(),
        }))
    }

    async fn health(
        &self,
        _request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        let st = lock(&self.state)?;
        Ok(Response::new(HealthResponse {
            ready: st.ready,
            message: st.message.clone(),
            containerd: st.containerd.clone(),
            kubelet: st.kubelet.clone(),
        }))
    }

    async fn validate_configuration(
        &self,
        request: Request<ApplyConfigurationRequest>,
    ) -> Result<Response<ValidateConfigurationResponse>, Status> {
        let yaml = request.into_inner().yaml;
        match MachineConfig::from_yaml(&yaml) {
            Ok(_) => Ok(Response::new(ValidateConfigurationResponse {
                ok: true,
                message: "configuration valid".into(),
            })),
            Err(err) => Ok(Response::new(ValidateConfigurationResponse {
                ok: false,
                message: err.to_string(),
            })),
        }
    }

    async fn apply_configuration(
        &self,
        request: Request<ApplyConfigurationRequest>,
    ) -> Result<Response<ApplyConfigurationResponse>, Status> {
        let yaml = request.into_inner().yaml;

        let path = {
            let st = lock(&self.state)?;
            st.config_path.clone()
        };

        // Merge onto on-disk config so partial YAML (dashboard-only, etc.)
        // does not wipe machine.type / network / cluster and break kubelet.
        let previous_raw = fs::read_to_string(&path).ok();
        let mut cfg = MachineConfig::from_yaml_merged(yaml.as_str(), previous_raw.as_deref())
            .map_err(|e| Status::invalid_argument(e.to_string()))?;

        let previous = previous_raw
            .as_deref()
            .and_then(|raw| MachineConfig::from_yaml(raw).ok());
        // gen config omits dashboard — preserve the on-disk section (or write
        // built-ins) so apply does not wipe console theme/size after reboot.
        cfg.resolve_dashboard(previous.as_ref());

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| Status::internal(e.to_string()))?;
        }
        let serialized =
            serde_yaml::to_string(&cfg).map_err(|e| Status::internal(e.to_string()))?;
        fs::write(&path, &serialized).map_err(|e| Status::internal(e.to_string()))?;
        // Ensure apply survives reboot (STATE may be on slow storage).
        if let Ok(f) = fs::File::open(&path) {
            let _ = f.sync_all();
        }
        if let Ok(mut st) = self.state.lock() {
            st.config_reload = true;
            st.message = "configuration applied".into();
        }

        info!(path = %path.display(), "configuration applied");
        Ok(Response::new(ApplyConfigurationResponse {
            ok: true,
            message: "configuration written; runtime will reload (reboot still needed for install/network edge cases)".into(),
            path: path.display().to_string(),
        }))
    }

    async fn reboot(
        &self,
        _request: Request<RebootRequest>,
    ) -> Result<Response<RebootResponse>, Status> {
        let mut st = lock(&self.state)?;
        st.power = PowerAction::Reboot;
        st.ready = false;
        st.message = "reboot requested".into();
        info!("reboot accepted via API");
        Ok(Response::new(RebootResponse {
            accepted: true,
            message: "reboot scheduled".into(),
        }))
    }

    async fn shutdown(
        &self,
        _request: Request<ShutdownRequest>,
    ) -> Result<Response<ShutdownResponse>, Status> {
        let mut st = lock(&self.state)?;
        st.power = PowerAction::Shutdown;
        st.ready = false;
        st.message = "shutdown requested".into();
        info!("shutdown accepted via API");
        Ok(Response::new(ShutdownResponse {
            accepted: true,
            message: "shutdown scheduled".into(),
        }))
    }

    async fn service_list(
        &self,
        _request: Request<ServiceListRequest>,
    ) -> Result<Response<ServiceListResponse>, Status> {
        let st = lock(&self.state)?;
        let services = vec![
            ServiceStatus {
                name: "pertiskd".into(),
                state: "up".into(),
                pid: std::process::id(),
            },
            ServiceStatus {
                name: "containerd".into(),
                state: st.containerd.clone(),
                pid: st.containerd_pid,
            },
            ServiceStatus {
                name: "kubelet".into(),
                state: st.kubelet.clone(),
                pid: st.kubelet_pid,
            },
        ];
        Ok(Response::new(ServiceListResponse { services }))
    }

    async fn upgrade(
        &self,
        request: Request<UpgradeRequest>,
    ) -> Result<Response<UpgradeResponse>, Status> {
        let req = request.into_inner();
        let (state_root, trust_key) = {
            let st = lock(&self.state)?;
            (st.state_root.clone(), st.trust_public_key.clone())
        };

        if !trust_key.exists() {
            return Err(Status::failed_precondition(format!(
                "upgrade trust key missing at {}",
                trust_key.display()
            )));
        }

        let layout = SlotLayout::new(state_root, trust_key);
        let result = apply_bundle(&layout, std::path::Path::new(&req.bundle_path))
            .map_err(|e| Status::invalid_argument(e.to_string()))?;

        if req.reboot {
            let mut st = lock(&self.state)?;
            st.power = PowerAction::Reboot;
            st.ready = false;
            st.message = format!("upgrade to {} staged; rebooting", result.version);
        }

        Ok(Response::new(UpgradeResponse {
            ok: true,
            message: format!(
                "staged slot {} version {}; bootloader_updated={}; reboot required to activate",
                result.target_slot, result.version, result.bootloader_updated
            ),
            target_slot: result.target_slot.to_string(),
            version: result.version,
        }))
    }

    async fn mark_boot_good(
        &self,
        _request: Request<MarkBootGoodRequest>,
    ) -> Result<Response<MarkBootGoodResponse>, Status> {
        let state_root = {
            let st = lock(&self.state)?;
            st.state_root.clone()
        };
        let meta = mark_boot_good(&state_root).map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(MarkBootGoodResponse {
            ok: true,
            message: "current boot marked good".into(),
            active_slot: meta.active.to_string(),
        }))
    }

    async fn upgrade_status(
        &self,
        _request: Request<UpgradeStatusRequest>,
    ) -> Result<Response<UpgradeStatusResponse>, Status> {
        let state_root = {
            let st = lock(&self.state)?;
            st.state_root.clone()
        };
        let meta = BootMeta::load(&state_root).map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(UpgradeStatusResponse {
            active_slot: meta.active.to_string(),
            next_slot: meta.next.to_string(),
            previous_good: meta.previous_good.to_string(),
            boot_attempts: meta.boot_attempts,
            boot_ok: meta.boot_ok,
            active_version: meta.active_version.unwrap_or_default(),
            pending_version: meta.pending_version.unwrap_or_default(),
        }))
    }

    async fn logs(&self, request: Request<LogsRequest>) -> Result<Response<LogsResponse>, Status> {
        let req = request.into_inner();
        let state_root = {
            let st = lock(&self.state)?;
            st.state_root.clone()
        };
        let service = if req.service.is_empty() {
            "pertiskd".to_string()
        } else {
            req.service
        };
        let tail = tail_logs(&state_root, &service, req.tail_lines)
            .map_err(|e| Status::invalid_argument(e.to_string()))?;
        Ok(Response::new(LogsResponse {
            service: tail.service,
            lines: tail.lines,
            source: tail.source,
        }))
    }

    async fn bootstrap(
        &self,
        request: Request<BootstrapRequest>,
    ) -> Result<Response<BootstrapResponse>, Status> {
        let advertise = request.into_inner().advertise_address;
        let (state_root, config_path) = {
            let st = lock(&self.state)?;
            (st.state_root.clone(), st.config_path.clone())
        };
        let yaml = fs::read_to_string(&config_path).map_err(|e| {
            Status::failed_precondition(format!(
                "read config {}: {e}; apply a controlplane config first",
                config_path.display()
            ))
        })?;
        let cfg =
            MachineConfig::from_yaml(&yaml).map_err(|e| Status::invalid_argument(e.to_string()))?;
        let adv = if advertise.is_empty() {
            None
        } else {
            Some(advertise.as_str())
        };
        let result = bootstrap_control_plane(&state_root, &cfg, adv)
            .map_err(|e| Status::internal(e.to_string()))?;
        {
            let mut st = lock(&self.state)?;
            st.message = result.message.clone();
            // Always bounce kubelet so it picks up cert credentials under
            // /var/lib/kubelet (apply may have started it with a join token).
            st.kubelet_reload = true;
        }
        Ok(Response::new(BootstrapResponse {
            ok: true,
            message: result.message,
            already_bootstrapped: result.already_bootstrapped,
        }))
    }

    async fn kubeconfig(
        &self,
        _request: Request<KubeconfigRequest>,
    ) -> Result<Response<KubeconfigResponse>, Status> {
        let state_root = {
            let st = lock(&self.state)?;
            st.state_root.clone()
        };
        let kubeconfig = read_admin_kubeconfig(&state_root)
            .map_err(|e| Status::failed_precondition(e.to_string()))?;
        Ok(Response::new(KubeconfigResponse { kubeconfig }))
    }

    async fn join_control_plane(
        &self,
        request: Request<JoinControlPlaneRequest>,
    ) -> Result<Response<JoinControlPlaneResponse>, Status> {
        let req = request.into_inner();
        let (state_root, config_path) = {
            let st = lock(&self.state)?;
            (st.state_root.clone(), st.config_path.clone())
        };
        let yaml = fs::read_to_string(&config_path).map_err(|e| {
            Status::failed_precondition(format!(
                "read config {}: {e}; apply a controlplane join config first",
                config_path.display()
            ))
        })?;
        let cfg =
            MachineConfig::from_yaml(&yaml).map_err(|e| Status::invalid_argument(e.to_string()))?;
        let adv = if req.advertise_address.is_empty() {
            None
        } else {
            Some(req.advertise_address.as_str())
        };
        let result = join_control_plane(&state_root, &cfg, adv, &req.etcd_endpoints)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        {
            let mut st = lock(&self.state)?;
            st.message = result.message.clone();
            st.kubelet_reload = true;
        }
        Ok(Response::new(JoinControlPlaneResponse {
            ok: true,
            message: result.message,
            already_joined: result.already_joined,
        }))
    }

    async fn get_join_config(
        &self,
        request: Request<GetJoinConfigRequest>,
    ) -> Result<Response<GetJoinConfigResponse>, Status> {
        let req = request.into_inner();
        let (state_root, config_path) = {
            let st = lock(&self.state)?;
            (st.state_root.clone(), st.config_path.clone())
        };
        let yaml = fs::read_to_string(&config_path).map_err(|e| {
            Status::failed_precondition(format!("read config {}: {e}", config_path.display()))
        })?;
        let cfg =
            MachineConfig::from_yaml(&yaml).map_err(|e| Status::invalid_argument(e.to_string()))?;
        let cluster_name = if req.cluster_name.is_empty() {
            let host = cfg
                .machine
                .network
                .hostname
                .as_deref()
                .unwrap_or("pertisk");
            strip_cp_suffix(host)
        } else {
            req.cluster_name.clone()
        };
        let idx = if req.controlplane {
            if req.controlplane_index == 0 {
                2
            } else {
                req.controlplane_index
            }
        } else {
            0
        };
        let result = get_join_config(&state_root, &cfg, &cluster_name, idx)
            .map_err(|e| Status::failed_precondition(e.to_string()))?;
        let controlplane_yaml = if req.controlplane {
            result.controlplane_yaml
        } else {
            String::new()
        };
        Ok(Response::new(GetJoinConfigResponse {
            ok: true,
            message: "ok".into(),
            worker_yaml: result.worker_yaml,
            controlplane_yaml,
            etcd_endpoints: result.etcd_endpoints,
            ca_pem: result.ca_pem,
        }))
    }
}

fn strip_cp_suffix(name: &str) -> String {
    // lab-ha-cp-1 → lab-ha
    if let Some(idx) = name.rfind("-cp-") {
        name[..idx].to_string()
    } else if let Some(stripped) = name.strip_suffix("-cp") {
        stripped.to_string()
    } else {
        name.to_string()
    }
}

/// Serve the management API (plaintext or mTLS).
pub async fn serve(
    state: SharedState,
    listen: SocketAddr,
    tls: Option<TlsPaths>,
) -> anyhow::Result<()> {
    let svc = MachineSvc { state };
    let mut builder = Server::builder();

    if let Some(tls_paths) = tls {
        let cert = fs::read(&tls_paths.server_cert)?;
        let key = fs::read(&tls_paths.server_key)?;
        let ca = fs::read(&tls_paths.ca_cert)?;
        let identity = Identity::from_pem(cert, key);
        let client_ca = Certificate::from_pem(ca);
        let tls_config = ServerTlsConfig::new()
            .identity(identity)
            .client_ca_root(client_ca);
        builder = builder.tls_config(tls_config)?;
        info!(%listen, "management API listening (mTLS)");
    } else {
        info!(%listen, "management API listening (plaintext)");
    }

    builder
        .layer(crate::api_metrics::ApiMetricsLayer)
        .add_service(MachineServiceServer::new(svc))
        .serve(listen)
        .await?;
    Ok(())
}
