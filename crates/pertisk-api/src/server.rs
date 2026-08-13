//! gRPC MachineService implementation (mTLS-capable).

use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, MutexGuard};
use std::task::{Context, Poll};

use pertisk_bootstrap::{
    bootstrap_control_plane, default_restore_identity, detect_advertise_ip, etcd_restore,
    etcd_snapshot, get_join_config, join_control_plane, read_admin_kubeconfig,
};
use pertisk_config::MachineConfig;
use pertisk_proto::machine_service_server::{MachineService, MachineServiceServer};
use pertisk_proto::{
    ApplyConfigurationRequest, ApplyConfigurationResponse, AttestRequest, AttestResponse,
    BootstrapRequest, BootstrapResponse, ContainerInfo, ContainersRequest, ContainersResponse,
    DiskInspectRequest, DiskInspectResponse, DiskVolume, EtcdRestoreRequest, EtcdRestoreResponse,
    EtcdSnapshotRequest, EtcdSnapshotResponse, GetJoinConfigRequest, GetJoinConfigResponse,
    GrowDiskRequest, GrowDiskResponse, HealthRequest, HealthResponse, JoinControlPlaneRequest,
    JoinControlPlaneResponse, KubeconfigRequest, KubeconfigResponse, LogsRequest, LogsResponse,
    MarkBootGoodRequest, MarkBootGoodResponse, NetInspectRequest, NetInspectResponse, NetInterface,
    PcrValue, QuoteRequest, QuoteResponse, RebootRequest, RebootResponse, ResetRequest,
    ResetResponse, ServiceListRequest, ServiceListResponse, ServiceStatus, ShutdownRequest,
    ShutdownResponse, UpgradeRequest, UpgradeResponse, UpgradeStatusRequest,
    UpgradeStatusResponse, ValidateConfigurationResponse, VersionRequest, VersionResponse,
};
use pertisk_update::{apply_bundle, mark_boot_good, BootMeta, SlotLayout};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::Stream;
use tonic::transport::{Certificate, Identity, Server, ServerTlsConfig};
use tonic::{Request, Response, Status};
use tracing::{info, warn};

use crate::logs::{follow_logs, follow_source, tail_logs};
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

/// API listens before STATE is mounted (AHV virtio-scsi can hang). Wait so
/// apply/bootstrap never write `/system/state/config.yaml` on initramfs.
async fn config_path_when_state_mounted(state: &SharedState) -> Result<PathBuf, Status> {
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(90);
    loop {
        {
            let st = lock(state)?;
            if st.state_mounted {
                return Ok(st.config_path.clone());
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(Status::failed_precondition(
                "STATE partition is not mounted yet; retry apply in a few seconds",
            ));
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    }
}

#[tonic::async_trait]
impl MachineService for MachineSvc {
    async fn version(
        &self,
        _request: Request<VersionRequest>,
    ) -> Result<Response<VersionResponse>, Status> {
        let st = lock(&self.state)?;
        let hostname = fs::read_to_string(&st.config_path)
            .ok()
            .and_then(|y| MachineConfig::from_yaml(&y).ok())
            .and_then(|c| c.machine.network.hostname)
            .filter(|h| !h.is_empty())
            .or_else(|| {
                fs::read_to_string("/etc/hostname")
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_default();
        Ok(Response::new(VersionResponse {
            version: st.version.clone(),
            api_version: st.api_version.clone(),
            platform: st.platform.clone(),
            hostname,
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

        let path = config_path_when_state_mounted(&self.state).await?;

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

    type LogsStream =
        Pin<Box<dyn tokio_stream::Stream<Item = Result<LogsResponse, Status>> + Send + 'static>>;

    async fn logs(
        &self,
        request: Request<LogsRequest>,
    ) -> Result<Response<Self::LogsStream>, Status> {
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
        let follow = req.follow;
        let tail_lines = req.tail_lines;

        let (tx, rx) = tokio::sync::mpsc::channel::<Result<LogsResponse, Status>>(16);
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_worker = Arc::clone(&cancel);

        tokio::task::spawn_blocking(move || {
            let send = |tail: crate::logs::LogTail| {
                tx.blocking_send(Ok(LogsResponse {
                    service: tail.service,
                    lines: tail.lines,
                    source: tail.source,
                }))
                .is_ok()
            };

            let initial = match tail_logs(&state_root, &service, tail_lines) {
                Ok(t) => t,
                Err(e) => {
                    let _ = tx.blocking_send(Err(Status::invalid_argument(e.to_string())));
                    return;
                }
            };
            if !send(initial) {
                return;
            }
            if !follow {
                return;
            }

            let source = match follow_source(&state_root, &service) {
                Ok(s) => s,
                Err(e) => {
                    let _ = tx.blocking_send(Err(Status::invalid_argument(e.to_string())));
                    return;
                }
            };

            let (ftx, frx) = mpsc::channel();
            let svc = service.clone();
            let cancel_f = Arc::clone(&cancel_worker);
            let follow_handle = std::thread::spawn(move || {
                let _ = follow_logs(source, &svc, &ftx, &cancel_f);
            });

            while !cancel_worker.load(Ordering::Relaxed) {
                match frx.recv_timeout(std::time::Duration::from_millis(500)) {
                    Ok(chunk) => {
                        if !send(chunk) {
                            break;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            cancel_worker.store(true, Ordering::Relaxed);
            let _ = follow_handle.join();
        });

        // When the client disconnects, drop of `rx` stops the sender; also flip cancel
        // via a wrapper stream that observes drop... ReceiverStream alone is enough:
        // blocking_send fails → worker exits → follow thread sees disconnect / cancel.
        // Set cancel when the async stream is dropped by wrapping.
        let stream = DropCancelStream {
            inner: ReceiverStream::new(rx),
            cancel,
        };

        Ok(Response::new(Box::pin(stream)))
    }

    async fn bootstrap(
        &self,
        request: Request<BootstrapRequest>,
    ) -> Result<Response<BootstrapResponse>, Status> {
        let advertise = request.into_inner().advertise_address;
        let (state_root, config_path) = {
            let path = config_path_when_state_mounted(&self.state).await?;
            let st = lock(&self.state)?;
            (st.state_root.clone(), path)
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
            let path = config_path_when_state_mounted(&self.state).await?;
            let st = lock(&self.state)?;
            (st.state_root.clone(), path)
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
            .map_err(|e| {
                // Even on finalize failure, credentials may already be on disk —
                // restart kubelet so the Node can register for the next retry.
                if let Ok(mut st) = self.state.lock() {
                    st.kubelet_reload = true;
                }
                Status::internal(format!("{e:#}"))
            })?;
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

    async fn grow_disk(
        &self,
        _request: Request<GrowDiskRequest>,
    ) -> Result<Response<GrowDiskResponse>, Status> {
        info!("grow disk (EPHEMERAL) requested via API");
        match pertisk_disk::grow_ephemeral_storage() {
            Ok(r) => {
                let message = match (r.partition_grew, r.filesystem_grew) {
                    (false, false) => {
                        "EPHEMERAL already fills the disk (or kernel still sees old capacity — rescan/reboot)".into()
                    }
                    (true, true) => "grew EPHEMERAL partition and filesystem".into(),
                    (true, false) => "grew EPHEMERAL partition (filesystem unchanged)".into(),
                    (false, true) => "resized EPHEMERAL filesystem".into(),
                };
                info!(
                    partition_grew = r.partition_grew,
                    filesystem_grew = r.filesystem_grew,
                    "{message}"
                );
                Ok(Response::new(GrowDiskResponse {
                    ok: true,
                    message,
                    partition_grew: r.partition_grew,
                    filesystem_grew: r.filesystem_grew,
                }))
            }
            Err(err) => {
                warn!(error = %err, "grow disk failed");
                Ok(Response::new(GrowDiskResponse {
                    ok: false,
                    message: err.to_string(),
                    partition_grew: false,
                    filesystem_grew: false,
                }))
            }
        }
    }

    async fn attest(
        &self,
        _request: Request<AttestRequest>,
    ) -> Result<Response<AttestResponse>, Status> {
        let (state_root, version) = {
            let st = lock(&self.state)?;
            (st.state_root.clone(), st.version.clone())
        };
        let (active_slot, version) = match BootMeta::load(&state_root) {
            Ok(meta) => (
                meta.active.to_string(),
                meta.active_version.unwrap_or(version),
            ),
            Err(_) => ("unknown".into(), version),
        };
        let snap = crate::attest::read_host_pcrs();
        Ok(Response::new(AttestResponse {
            available: snap.available,
            message: snap.message,
            active_slot,
            version,
            pcrs: snap
                .pcrs
                .into_iter()
                .map(|p| PcrValue {
                    index: p.index,
                    algo: p.algo,
                    digest_hex: p.digest_hex,
                })
                .collect(),
        }))
    }

    async fn containers(
        &self,
        _request: Request<ContainersRequest>,
    ) -> Result<Response<ContainersResponse>, Status> {
        let snap = tokio::task::spawn_blocking(crate::containers::list_containers)
            .await
            .map_err(|e| Status::internal(format!("containers task: {e}")))?;
        Ok(Response::new(ContainersResponse {
            available: snap.available,
            message: snap.message,
            containers: snap
                .containers
                .into_iter()
                .map(|c| ContainerInfo {
                    id: c.id,
                    name: c.name,
                    image: c.image,
                    state: c.state,
                    namespace: c.namespace,
                    kind: c.kind,
                    pod_name: c.pod_name,
                    pod_namespace: c.pod_namespace,
                })
                .collect(),
        }))
    }

    async fn quote(
        &self,
        request: Request<QuoteRequest>,
    ) -> Result<Response<QuoteResponse>, Status> {
        let nonce = request.into_inner().nonce;
        let (state_root, version) = {
            let st = lock(&self.state)?;
            (st.state_root.clone(), st.version.clone())
        };
        let (active_slot, version) = match BootMeta::load(&state_root) {
            Ok(meta) => (
                meta.active.to_string(),
                meta.active_version.unwrap_or(version),
            ),
            Err(_) => ("unknown".into(), version),
        };
        let snap = tokio::task::spawn_blocking(move || pertisk_tpm::produce_quote(&nonce))
            .await
            .map_err(|e| Status::internal(format!("quote task: {e}")))?;
        let pcrs = crate::attest::read_host_pcrs()
            .pcrs
            .into_iter()
            .map(|p| PcrValue {
                index: p.index,
                algo: p.algo,
                digest_hex: p.digest_hex,
            })
            .collect();
        Ok(Response::new(QuoteResponse {
            available: snap.available,
            message: snap.message,
            nonce: snap.nonce,
            quoted: snap.quoted,
            signature: snap.signature,
            ak_public: snap.ak_public,
            pcrs,
            active_slot,
            version,
            ek_cert_der: snap.ek.der,
            ek_nv_index: snap.ek.nv_index,
            ek_subject: snap.ek.subject,
            ek_issuer: snap.ek.issuer,
            ek_fingerprint: snap.ek.fingerprint_sha256,
            ek_chain_status: snap.ek.chain_status.as_str().to_string(),
            ek_chain_message: snap.ek.chain_message,
        }))
    }

    async fn etcd_snapshot(
        &self,
        request: Request<EtcdSnapshotRequest>,
    ) -> Result<Response<EtcdSnapshotResponse>, Status> {
        let req = request.into_inner();
        let state_root = {
            let st = lock(&self.state)?;
            st.state_root.clone()
        };
        let out = if req.output_path.is_empty() {
            None
        } else {
            Some(std::path::PathBuf::from(req.output_path))
        };
        let snap = etcd_snapshot(&state_root, out.as_deref()).await;
        Ok(Response::new(EtcdSnapshotResponse {
            available: snap.available,
            message: snap.message,
            path: snap.path,
            size_bytes: snap.size_bytes,
            revision: snap.revision,
        }))
    }

    async fn etcd_restore(
        &self,
        request: Request<EtcdRestoreRequest>,
    ) -> Result<Response<EtcdRestoreResponse>, Status> {
        let req = request.into_inner();
        if req.snapshot_path.is_empty() {
            return Err(Status::invalid_argument("snapshot_path required"));
        }
        let (state_root, config_path) = {
            let st = lock(&self.state)?;
            (st.state_root.clone(), st.config_path.clone())
        };
        let hostname = fs::read_to_string(&config_path)
            .ok()
            .and_then(|y| MachineConfig::from_yaml(&y).ok())
            .and_then(|c| c.machine.network.hostname)
            .unwrap_or_else(|| "pertisk-cp-1".into());
        let advertise = if !req.advertise_address.is_empty() {
            req.advertise_address.clone()
        } else {
            detect_advertise_ip().unwrap_or_else(|| "127.0.0.1".into())
        };
        let (def_name, def_initial, def_peer) = default_restore_identity(&hostname, &advertise);
        let name = if req.member_name.is_empty() {
            def_name
        } else {
            req.member_name
        };
        let initial = if req.initial_cluster.is_empty() {
            def_initial
        } else {
            req.initial_cluster
        };
        let peer = if req.peer_url.is_empty() {
            def_peer
        } else {
            req.peer_url
        };
        let result = etcd_restore(
            &state_root,
            std::path::Path::new(&req.snapshot_path),
            req.force,
            &name,
            &initial,
            &peer,
        )
        .await;
        Ok(Response::new(EtcdRestoreResponse {
            ok: result.ok,
            message: result.message,
        }))
    }

    async fn net_inspect(
        &self,
        _request: Request<NetInspectRequest>,
    ) -> Result<Response<NetInspectResponse>, Status> {
        let snap = tokio::task::spawn_blocking(crate::net_inspect::inspect_net)
            .await
            .map_err(|e| Status::internal(format!("net inspect task: {e}")))?;
        Ok(Response::new(NetInspectResponse {
            available: snap.available,
            message: snap.message,
            interfaces: snap
                .interfaces
                .into_iter()
                .map(|i| NetInterface {
                    name: i.name,
                    operstate: i.operstate,
                    addresses: i.addresses,
                })
                .collect(),
        }))
    }

    async fn disk_inspect(
        &self,
        _request: Request<DiskInspectRequest>,
    ) -> Result<Response<DiskInspectResponse>, Status> {
        let snap = tokio::task::spawn_blocking(crate::disk_inspect::inspect_disks)
            .await
            .map_err(|e| Status::internal(format!("disk inspect task: {e}")))?;
        Ok(Response::new(DiskInspectResponse {
            available: snap.available,
            message: snap.message,
            volumes: snap
                .volumes
                .into_iter()
                .map(|v| DiskVolume {
                    label: v.label,
                    mountpoint: v.mountpoint,
                    device: v.device,
                    mounted: v.mounted,
                    total_bytes: v.total_bytes,
                    used_bytes: v.used_bytes,
                })
                .collect(),
        }))
    }

    async fn reset(
        &self,
        request: Request<ResetRequest>,
    ) -> Result<Response<ResetResponse>, Status> {
        let req = request.into_inner();
        if !req.force {
            return Err(Status::failed_precondition(
                "reset requires force=true (destroys STATE identity + local runtime data; GPT kept)",
            ));
        }
        let state_root = {
            let st = lock(&self.state)?;
            st.state_root.clone()
        };
        let result = tokio::task::spawn_blocking(move || pertisk_disk::soft_reset(&state_root))
            .await
            .map_err(|e| Status::internal(format!("reset task: {e}")))?
            .map_err(|e| Status::internal(e.to_string()))?;

        let mut message = format!(
            "soft reset cleared {} path(s)",
            result.cleared.len()
        );
        if !result.warnings.is_empty() {
            message.push_str(&format!("; {} warning(s)", result.warnings.len()));
        }
        if !result.cleared.is_empty() {
            message.push_str(&format!(": {}", result.cleared.join(", ")));
        }

        let mut reboot_scheduled = false;
        if req.reboot {
            let mut st = lock(&self.state)?;
            st.power = PowerAction::Reboot;
            st.ready = false;
            st.message = "soft reset complete; reboot scheduled".into();
            reboot_scheduled = true;
            message.push_str("; reboot scheduled");
        } else if let Ok(mut st) = self.state.lock() {
            st.message = "soft reset complete (no reboot)".into();
            st.ready = false;
        }

        info!(
            cleared = result.cleared.len(),
            warnings = result.warnings.len(),
            reboot_scheduled,
            "reset accepted via API"
        );
        Ok(Response::new(ResetResponse {
            ok: true,
            message,
            reboot_scheduled,
        }))
    }
}

/// Stops the follow worker when the gRPC client disconnects.
struct DropCancelStream<S> {
    inner: S,
    cancel: Arc<AtomicBool>,
}

impl<S> Drop for DropCancelStream<S> {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl<S: Stream + Unpin> Stream for DropCancelStream<S> {
    type Item = S::Item;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
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
