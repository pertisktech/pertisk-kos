//! Additional control-plane join + join-config export for HA.

use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use etcd_client::{Certificate as EtcdCert, Client, ConnectOptions, Identity as EtcdIdentity};
use pertisk_config::{
    Cluster, ClusterNetwork, CniMode, Dashboard, Interface, Machine, MachineConfig, MachineType,
    Network, CONFIG_VERSION,
};
use tracing::info;

use crate::kube_vip;
use crate::kubeconfig;
use crate::paths::BootstrapPaths;
use crate::pki;
use crate::static_pods;
use crate::{
    chrono_like_now, copy_dir, detect_advertise_ip, endpoint_host, finalize_bootstrap_when_ready,
    kubernetes_service_ip, publish_kubelet_credentials, DEFAULT_ETCD_IMAGE, DEFAULT_K8S_VERSION,
};

pub struct JoinControlPlaneResult {
    pub already_joined: bool,
    pub message: String,
}

pub struct JoinConfigResult {
    pub worker_yaml: String,
    pub controlplane_yaml: String,
    pub etcd_endpoints: Vec<String>,
    pub ca_pem: String,
}

/// Join this node as an additional stacked control plane.
///
/// Requires applied machine config with `ca`/`caKey`/`saKey` (from GetJoinConfig).
/// `etcd_endpoints` are existing member client URLs (e.g. `https://cp1:2379`).
pub async fn join_control_plane(
    state_root: &Path,
    cfg: &MachineConfig,
    advertise_address: Option<&str>,
    etcd_endpoints: &[String],
) -> Result<JoinControlPlaneResult> {
    if cfg.machine.machine_type != MachineType::Controlplane {
        bail!("join-controlplane requires machine.type: controlplane");
    }
    let cluster = cfg
        .cluster
        .as_ref()
        .context("cluster block required for join-controlplane")?;
    let ca = cluster
        .ca
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .context("cluster.ca required for control-plane join")?;
    let ca_key = cluster
        .ca_key
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .context("cluster.caKey required for control-plane join")?;
    let sa_key = cluster
        .sa_key
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .context("cluster.saKey required for control-plane join")?;
    if etcd_endpoints.is_empty() {
        bail!("etcd_endpoints required (existing member https://<ip>:2379)");
    }

    let paths = BootstrapPaths::default_state(state_root);
    let hostname = cfg
        .machine
        .network
        .hostname
        .clone()
        .unwrap_or_else(|| "pertisk-cp".into());
    if paths.is_bootstrapped() {
        let _ = crate::restore_control_plane(state_root);
        // Prior joins could write BOOTSTRAPPED before the CP role label stuck
        // (especially CP3). Re-run finalize so retries actually fix ROLES=<none>.
        // Give pertiskd a beat to honor the kubelet-reload flag from restore.
        thread::sleep(Duration::from_secs(5));
        let admin_path = paths.admin_kubeconfig();
        let defer_addons = matches!(cluster.cni, pertisk_config::CniMode::None);
        finalize_bootstrap_when_ready(&admin_path, None, &hostname, defer_addons).with_context(
            || format!("already-joined control-plane missing finalize/label for {hostname}"),
        )?;
        return Ok(JoinControlPlaneResult {
            already_joined: true,
            message: "already bootstrapped / joined".into(),
        });
    }

    paths.ensure_dirs()?;
    let advertise = advertise_address
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .or_else(detect_advertise_ip)
        .context("could not determine advertise address")?;
    let endpoint_host = endpoint_host(&cluster.endpoint);
    let k8s_ver = cluster
        .kubernetes_version
        .as_deref()
        .unwrap_or(DEFAULT_K8S_VERSION);
    let service_subnet = cluster.ipv4_service_subnet();
    let service_cidr = cluster.service_cluster_ip_range();
    let cluster_cidr = cluster.cluster_cidr();
    let sans = cluster.pki_extra_sans();
    let kubernetes_svc_ip = kubernetes_service_ip(&service_subnet);

    info!(%advertise, %hostname, "joining control-plane (shared CA)");
    let pki = pki::generate_pki_from_existing(
        ca,
        ca_key,
        Some(sa_key),
        &advertise,
        &hostname,
        &endpoint_host,
        &kubernetes_svc_ip,
        &sans,
    )?;
    pki::write_pki(&paths.pki(), &paths.etcd_pki(), &pki)?;

    // MemberAdd against an existing etcd using the new peer's client certs.
    let peer_url = format!("https://{advertise}:2380");
    let initial_cluster = etcd_member_add(
        etcd_endpoints,
        &hostname,
        &peer_url,
        &pki.ca_crt,
        &pki.etcd_crt,
        &pki.etcd_key,
    )
    .await?;

    let kc_name = cluster
        .name
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("pertisk");
    let admin = kubeconfig::render_kubeconfig(
        &cluster.endpoint,
        &pki.ca_crt,
        &pki.admin_crt,
        &pki.admin_key,
        "kubernetes-admin",
        kc_name,
    );
    let local_admin = kubeconfig::render_kubeconfig(
        "https://127.0.0.1:6443",
        &pki.ca_crt,
        &pki.admin_crt,
        &pki.admin_key,
        "kubernetes-admin",
        kc_name,
    );
    let cm_conf = kubeconfig::render_kubeconfig(
        "https://127.0.0.1:6443",
        &pki.ca_crt,
        &pki.cm_crt,
        &pki.cm_key,
        "system:kube-controller-manager",
        kc_name,
    );
    let sched_conf = kubeconfig::render_kubeconfig(
        "https://127.0.0.1:6443",
        &pki.ca_crt,
        &pki.sched_crt,
        &pki.sched_key,
        "system:kube-scheduler",
        kc_name,
    );
    let kubelet_conf = kubeconfig::render_kubeconfig(
        "https://127.0.0.1:6443",
        &pki.ca_crt,
        &pki.kubelet_crt,
        &pki.kubelet_key,
        &format!("system:node:{hostname}"),
        kc_name,
    );

    fs::write(paths.admin_kubeconfig(), &admin)?;
    fs::write(paths.admin_local_kubeconfig(), &local_admin)?;
    fs::write(
        paths.kubeconfig_dir().join("controller-manager.conf"),
        &cm_conf,
    )?;
    fs::write(paths.kubeconfig_dir().join("scheduler.conf"), &sched_conf)?;
    fs::write(paths.kubeconfig_dir().join("kubelet.conf"), &kubelet_conf)?;

    let live = Path::new("/etc/kubernetes");
    fs::create_dir_all(live)?;
    fs::write(live.join("admin.conf"), &local_admin)?;
    fs::write(live.join("controller-manager.conf"), &cm_conf)?;
    fs::write(live.join("scheduler.conf"), &sched_conf)?;
    fs::write(live.join("kubelet.conf"), &kubelet_conf)?;
    publish_kubelet_credentials(&kubelet_conf, &pki.ca_crt)?;

    let pki_live = live.join("pki");
    static_pods::write_static_pods(
        &paths.manifests(),
        &static_pods::StaticPodParams {
            advertise_ip: &advertise,
            hostname: &hostname,
            kubernetes_version: k8s_ver,
            etcd_image: DEFAULT_ETCD_IMAGE,
            service_cidr: &service_cidr,
            pod_subnet: &cluster_cidr,
            pki_host_path: "/etc/kubernetes/pki",
            etcd_initial_cluster: &initial_cluster,
            etcd_initial_cluster_state: "existing",
        },
    )?;
    let vip = kube_vip::vip_from_endpoint_host(&endpoint_host);
    let _ = kube_vip::maybe_write_kube_vip(
        &paths.manifests(),
        vip,
        cluster.vip6.as_deref(),
        &advertise,
        kube_vip::DEFAULT_KUBE_VIP_INTERFACE,
    )?;
    paths.link_live()?;
    if !pki_live.exists() {
        copy_dir(&paths.pki(), &pki_live)?;
    }

    fs::create_dir_all("/var/lib/etcd").ok();
    // Marker after join material is on disk so reboot restore works; finalize may
    // still fail (slow CP3). Retries hit already_joined and re-run finalize/label.
    fs::write(
        paths.marker(),
        format!("joined control-plane at {}\n", chrono_like_now()),
    )?;

    // Cert kubeconfig is on disk; wait briefly for pertiskd to restart kubelet
    // (via /run/pertisk/kubelet-reload) so the Node object appears before finalize.
    info!("waiting for kubelet credential reload before finalize");
    thread::sleep(Duration::from_secs(8));

    // Label this CP node once local apiserver is up (skip token/RBAC/addons).
    // Must succeed: unlabeled joined CPs show ROLES=<none> in `kubectl get nodes`.
    let admin_path = paths.admin_kubeconfig();
    let node_name = hostname.clone();
    let defer_addons = matches!(cluster.cni, pertisk_config::CniMode::None);
    finalize_bootstrap_when_ready(&admin_path, None, &node_name, defer_addons)
        .with_context(|| format!("post-join finalize failed for {node_name}"))?;

    Ok(JoinControlPlaneResult {
        already_joined: false,
        message: format!(
            "control-plane joined advertise={advertise} etcd_initial_cluster={initial_cluster}"
        ),
    })
}

async fn etcd_member_add(
    endpoints: &[String],
    name: &str,
    peer_url: &str,
    ca_pem: &str,
    cert_pem: &str,
    key_pem: &str,
) -> Result<String> {
    let ca = EtcdCert::from_pem(ca_pem.as_bytes());
    let identity = EtcdIdentity::from_pem(cert_pem.as_bytes(), key_pem.as_bytes());
    let opts = ConnectOptions::new().with_tls(
        etcd_client::TlsOptions::new()
            .ca_certificate(ca)
            .identity(identity),
    );

    // Retry: first CP etcd may still be starting when lab races ahead.
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 1..=45 {
        match Client::connect(endpoints, Some(opts.clone())).await {
            Ok(mut client) => match client.member_add([peer_url.to_string()], None).await {
                Ok(_) => {
                    let list = client.member_list().await.context("etcd member_list")?;
                    let mut parts = Vec::new();
                    for m in list.members() {
                        let peer = m
                            .peer_urls()
                            .first()
                            .cloned()
                            .unwrap_or_else(|| peer_url.to_string());
                        let n = m.name();
                        if n.is_empty() {
                            if m.peer_urls().iter().any(|u| u == peer_url) {
                                parts.push(format!("{name}={peer_url}"));
                            } else {
                                parts.push(format!("{}={peer}", m.id()));
                            }
                        } else {
                            parts.push(format!("{n}={peer}"));
                        }
                    }
                    if !parts.iter().any(|p| p.starts_with(&format!("{name}="))) {
                        parts.push(format!("{name}={peer_url}"));
                    }
                    parts.sort();
                    parts.dedup();
                    let initial = parts.join(",");
                    info!(%initial, attempt, "etcd MemberAdd ok");
                    return Ok(initial);
                }
                Err(err) => {
                    last_err = Some(anyhow::anyhow!("member_add: {err}"));
                }
            },
            Err(err) => {
                last_err = Some(anyhow::anyhow!("connect: {err}"));
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("etcd MemberAdd failed")))
}

/// Export worker + controlplane-join YAML (with shared secrets) from a bootstrapped CP.
pub fn get_join_config(
    state_root: &Path,
    applied: &MachineConfig,
    cluster_name: &str,
    controlplane_index: u32,
) -> Result<JoinConfigResult> {
    let paths = BootstrapPaths::default_state(state_root);
    if !paths.is_bootstrapped() {
        bail!("node is not bootstrapped; run bootstrap first");
    }
    let cluster = applied
        .cluster
        .as_ref()
        .context("applied config missing cluster")?;
    let ca = fs::read_to_string(paths.pki().join("ca.crt")).context("read ca.crt")?;
    let ca_key = fs::read_to_string(paths.pki().join("ca.key")).context("read ca.key")?;
    let sa_key = fs::read_to_string(paths.pki().join("sa.key")).context("read sa.key")?;
    let token = cluster
        .token
        .clone()
        .context("applied config missing cluster.token")?;
    let endpoint = cluster.endpoint.clone();
    let cert_sans = cluster.cert_sans.clone();
    let network = Some(ClusterNetwork {
        pod_subnets: cluster.effective_pod_subnets(),
        service_subnets: cluster.effective_service_subnets(),
    });
    let network_mode = cluster.network_mode;
    let vip6 = cluster.vip6.clone();
    let k8s_ver = cluster
        .kubernetes_version
        .clone()
        .unwrap_or_else(|| DEFAULT_K8S_VERSION.into());
    let cni = cluster.cni;

    let advertise = detect_advertise_ip().unwrap_or_else(|| "127.0.0.1".into());
    let etcd_endpoints = vec![format!("https://{advertise}:2379")];

    let cp_hostname = if controlplane_index == 0 {
        format!("{cluster_name}-cp-join")
    } else {
        format!("{cluster_name}-cp-{controlplane_index}")
    };

    let cp = MachineConfig {
        version: CONFIG_VERSION.into(),
        machine: Machine {
            machine_type: MachineType::Controlplane,
            network: Network {
                hostname: Some(cp_hostname),
                interfaces: vec![Interface {
                    interface: "eth0".into(),
                    dhcp: true,
                    addresses: vec![],
                    gateway: None,
                }],
                nameservers: vec!["1.1.1.1".into()],
            },
            install: None,
            dashboard: applied
                .machine
                .dashboard
                .clone()
                .or_else(|| Some(Dashboard::builtin())),
            kubelet: applied.machine.kubelet.clone(),
        },
        cluster: Some(Cluster {
            name: Some(cluster_name.into()),
            endpoint: endpoint.clone(),
            token: Some(token.clone()),
            ca: Some(ca.clone()),
            ca_key: Some(ca_key),
            sa_key: Some(sa_key),
            network: network.clone(),
            pod_subnet: None,
            service_subnet: None,
            pod_cidr_ipv6: None,
            service_cidr_ipv6: None,
            network_mode,
            vip6: vip6.clone(),
            kubernetes_version: Some(k8s_ver.clone()),
            pod_cidr: None,
            cni,
            cert_sans: cert_sans.clone(),
        }),
    };

    let worker = MachineConfig {
        version: CONFIG_VERSION.into(),
        machine: Machine {
            machine_type: MachineType::Worker,
            network: Network {
                hostname: Some(format!("{cluster_name}-wk-1")),
                interfaces: vec![Interface {
                    interface: "eth0".into(),
                    dhcp: true,
                    addresses: vec![],
                    gateway: None,
                }],
                nameservers: vec!["1.1.1.1".into()],
            },
            install: None,
            dashboard: applied
                .machine
                .dashboard
                .clone()
                .or_else(|| Some(Dashboard::builtin())),
            kubelet: applied.machine.kubelet.clone(),
        },
        cluster: Some(Cluster {
            name: Some(cluster_name.into()),
            endpoint: endpoint.clone(),
            token: Some(token),
            ca: Some(ca.clone()),
            ca_key: None,
            sa_key: None,
            network,
            pod_subnet: None,
            service_subnet: None,
            pod_cidr_ipv6: None,
            service_cidr_ipv6: None,
            network_mode,
            vip6,
            kubernetes_version: Some(k8s_ver),
            pod_cidr: None,
            cni: CniMode::None,
            cert_sans,
        }),
    };

    Ok(JoinConfigResult {
        worker_yaml: serde_yaml::to_string(&worker)?,
        controlplane_yaml: serde_yaml::to_string(&cp)?,
        etcd_endpoints,
        ca_pem: ca,
    })
}
