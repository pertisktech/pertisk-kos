//! Offline `pertiskctl gen config` helpers.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use pertisk_config::{
    Cluster, CniMode, Dashboard, Interface, Machine, MachineConfig, MachineKubelet, MachineType,
    Network, NetworkMode, CONFIG_VERSION,
};

use crate::token::generate_bootstrap_token;

pub struct GenConfigOutput {
    pub cluster_name: String,
    pub endpoint: String,
    pub token: String,
    pub controlplane_yaml: String,
    pub worker_yaml: String,
}

pub struct GenConfigHaOutput {
    pub cluster_name: String,
    pub endpoint: String,
    pub token: String,
    pub controlplane_yamls: Vec<(String, String)>, // (filename, yaml)
    pub worker_yaml: String,
}

fn normalize_endpoint(endpoint: &str) -> String {
    let e = endpoint.trim();
    if e.starts_with("https://") || e.starts_with("http://") {
        e.to_string()
    } else {
        format!("https://{e}")
    }
}

fn endpoint_host(endpoint: &str) -> String {
    endpoint
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or(endpoint)
        .split(':')
        .next()
        .unwrap_or(endpoint)
        .to_string()
}

pub struct GenNetworkOpts {
    pub dual_stack: bool,
    pub pod_cidr_ipv6: Option<String>,
    pub service_cidr_ipv6: Option<String>,
    pub vip6: Option<String>,
    /// Optional kubelet maxPods (`machine.kubelet.extraConfig.maxPods`).
    pub max_pods: Option<u32>,
    /// Public mgmt URL for `machine.dashboard.mgmt_url` (serial console).
    pub mgmt_url: Option<String>,
}

impl Default for GenNetworkOpts {
    fn default() -> Self {
        Self {
            dual_stack: false,
            pod_cidr_ipv6: None,
            service_cidr_ipv6: None,
            vip6: None,
            max_pods: None,
            mgmt_url: None,
        }
    }
}

fn kubelet_opts(max_pods: Option<u32>) -> Option<MachineKubelet> {
    max_pods.map(MachineKubelet::with_max_pods)
}

fn dashboard_for_gen(net: &GenNetworkOpts) -> Option<Dashboard> {
    Some(Dashboard::builtin_with_mgmt_url(net.mgmt_url.as_deref()))
}

fn base_cluster(
    cluster_name: &str,
    endpoint: String,
    token: String,
    pod_subnet: &str,
    service_subnet: &str,
    kubernetes_version: &str,
    cert_sans: Vec<String>,
    net: &GenNetworkOpts,
) -> Cluster {
    let (network_mode, vip6) = if net.dual_stack {
        (
            NetworkMode::DualStack,
            net.vip6.clone().filter(|s| !s.trim().is_empty()),
        )
    } else {
        (NetworkMode::Ipv4, None)
    };
    let mut cert_sans = cert_sans;
    if let Some(ref v6) = vip6 {
        let v = v6.trim();
        if !v.is_empty() && !cert_sans.iter().any(|s| s == v) {
            cert_sans.push(v.to_string());
        }
    }
    Cluster {
        name: Some(cluster_name.into()),
        endpoint,
        token: Some(token),
        ca: None,
        ca_key: None,
        sa_key: None,
        network: Some(Cluster::network_from_cidrs(
            pod_subnet,
            service_subnet,
            net.dual_stack,
            net.pod_cidr_ipv6.as_deref(),
            net.service_cidr_ipv6.as_deref(),
        )),
        pod_subnet: None,
        service_subnet: None,
        pod_cidr_ipv6: None,
        service_cidr_ipv6: None,
        network_mode,
        vip6,
        kubernetes_version: Some(kubernetes_version.into()),
        pod_cidr: None,
        cni: CniMode::None,
        cert_sans,
    }
}

/// Generate controlplane + worker machine configs (CA filled after bootstrap).
pub fn gen_config(
    cluster_name: &str,
    endpoint: &str,
    kubernetes_version: &str,
    pod_subnet: &str,
    service_subnet: &str,
) -> Result<GenConfigOutput> {
    gen_config_with_network(
        cluster_name,
        endpoint,
        kubernetes_version,
        pod_subnet,
        service_subnet,
        &GenNetworkOpts::default(),
    )
}

pub fn gen_config_with_network(
    cluster_name: &str,
    endpoint: &str,
    kubernetes_version: &str,
    pod_subnet: &str,
    service_subnet: &str,
    net: &GenNetworkOpts,
) -> Result<GenConfigOutput> {
    let token = generate_bootstrap_token();
    let endpoint = normalize_endpoint(endpoint);
    let host = endpoint_host(&endpoint);
    let cert_sans = if host.parse::<std::net::IpAddr>().is_ok() {
        vec![host]
    } else {
        vec![]
    };

    let cp = MachineConfig {
        version: CONFIG_VERSION.into(),
        machine: Machine {
            machine_type: MachineType::Controlplane,
            network: Network {
                hostname: Some(format!("{cluster_name}-cp-1")),
                interfaces: vec![Interface {
                    interface: "eth0".into(),
                    dhcp: true,
                    addresses: vec![],
                    gateway: None,
                }],
                nameservers: vec!["1.1.1.1".into()],
            },
            install: None,
            dashboard: dashboard_for_gen(net),
            kubelet: kubelet_opts(net.max_pods),
        },
        cluster: Some(base_cluster(
            cluster_name,
            endpoint.clone(),
            token.clone(),
            pod_subnet,
            service_subnet,
            kubernetes_version,
            cert_sans.clone(),
            net,
        )),
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
            dashboard: dashboard_for_gen(net),
            kubelet: kubelet_opts(net.max_pods),
        },
        cluster: Some(base_cluster(
            cluster_name,
            endpoint.clone(),
            token.clone(),
            pod_subnet,
            service_subnet,
            kubernetes_version,
            cert_sans,
            net,
        )),
    };

    Ok(GenConfigOutput {
        cluster_name: cluster_name.into(),
        endpoint,
        token,
        controlplane_yaml: serde_yaml::to_string(&cp)?,
        worker_yaml: serde_yaml::to_string(&worker)?,
    })
}

/// Generate N control-plane YAMLs + one worker template for HA labs.
pub fn gen_config_ha(
    cluster_name: &str,
    endpoint: &str,
    controlplanes: u32,
    kubernetes_version: &str,
    pod_subnet: &str,
    service_subnet: &str,
) -> Result<GenConfigHaOutput> {
    gen_config_ha_with_network(
        cluster_name,
        endpoint,
        controlplanes,
        kubernetes_version,
        pod_subnet,
        service_subnet,
        &GenNetworkOpts::default(),
    )
}

pub fn gen_config_ha_with_network(
    cluster_name: &str,
    endpoint: &str,
    controlplanes: u32,
    kubernetes_version: &str,
    pod_subnet: &str,
    service_subnet: &str,
    net: &GenNetworkOpts,
) -> Result<GenConfigHaOutput> {
    if controlplanes == 0 {
        anyhow::bail!("controlplanes must be >= 1");
    }
    let token = generate_bootstrap_token();
    let endpoint = normalize_endpoint(endpoint);
    let host = endpoint_host(&endpoint);
    let mut cert_sans = Vec::new();
    if host.parse::<std::net::IpAddr>().is_ok() {
        cert_sans.push(host);
    }

    let mut controlplane_yamls = Vec::new();
    for i in 1..=controlplanes {
        let cp = MachineConfig {
            version: CONFIG_VERSION.into(),
            machine: Machine {
                machine_type: MachineType::Controlplane,
                network: Network {
                    hostname: Some(format!("{cluster_name}-cp-{i}")),
                    interfaces: vec![Interface {
                        interface: "eth0".into(),
                        dhcp: true,
                        addresses: vec![],
                        gateway: None,
                    }],
                    nameservers: vec!["1.1.1.1".into()],
                },
                install: None,
                dashboard: dashboard_for_gen(net),
                kubelet: kubelet_opts(net.max_pods),
            },
            cluster: Some(base_cluster(
                cluster_name,
                endpoint.clone(),
                token.clone(),
                pod_subnet,
                service_subnet,
                kubernetes_version,
                cert_sans.clone(),
                net,
            )),
        };
        let name = if i == 1 {
            "controlplane.yaml".into()
        } else {
            format!("controlplane-{i}.yaml")
        };
        controlplane_yamls.push((name, serde_yaml::to_string(&cp)?));
    }

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
            dashboard: dashboard_for_gen(net),
            kubelet: kubelet_opts(net.max_pods),
        },
        cluster: Some(base_cluster(
            cluster_name,
            endpoint.clone(),
            token.clone(),
            pod_subnet,
            service_subnet,
            kubernetes_version,
            cert_sans,
            net,
        )),
    };

    Ok(GenConfigHaOutput {
        cluster_name: cluster_name.into(),
        endpoint,
        token,
        controlplane_yamls,
        worker_yaml: serde_yaml::to_string(&worker)?,
    })
}

pub fn write_gen_config(out_dir: &Path, gen: &GenConfigOutput) -> Result<()> {
    fs::create_dir_all(out_dir)?;
    fs::write(out_dir.join("controlplane.yaml"), &gen.controlplane_yaml)
        .context("write controlplane.yaml")?;
    fs::write(out_dir.join("worker.yaml"), &gen.worker_yaml).context("write worker.yaml")?;
    fs::write(
        out_dir.join("README.txt"),
        format!(
            "cluster: {}\nendpoint: {}\ntoken: {}\n\n\
             1) Apply controlplane.yaml to the CP Machine API (:50000)\n\
             2) pertiskctl bootstrap -e <cp-ip>:50000\n\
             3) pertiskctl kubeconfig -e <cp-ip>:50000 -f admin.conf\n\
             4) pertiskctl join-config -e <cp-ip>:50000 -f worker.yaml\n\
             5) Apply worker.yaml to each worker (unique hostname); install CNI\n\
             Bootstrap also creates the join token Secret, node-join RBAC, and\n\
             labels the CP node-role.kubernetes.io/control-plane= (kubeadm-shaped).\n",
            gen.cluster_name, gen.endpoint, gen.token
        ),
    )?;
    Ok(())
}

pub fn write_gen_config_ha(out_dir: &Path, gen: &GenConfigHaOutput) -> Result<()> {
    fs::create_dir_all(out_dir)?;
    for (name, yaml) in &gen.controlplane_yamls {
        fs::write(out_dir.join(name), yaml).with_context(|| format!("write {name}"))?;
    }
    fs::write(out_dir.join("worker.yaml"), &gen.worker_yaml).context("write worker.yaml")?;
    fs::write(
        out_dir.join("README.txt"),
        format!(
            "cluster: {}\nendpoint: {}\ntoken: {}\ncontrolplanes: {}\n\n\
             HA (stacked etcd + kube-vip when endpoint VIP ≠ node IP):\n\
             1) Apply controlplane.yaml to CP1; pertiskctl bootstrap -e <cp1>:50000\n\
             2) pertiskctl get-join-config -e <cp1>:50000 --controlplane -o controlplane-join.yaml\n\
             3) For each extra CP: set hostname, apply YAML, join-controlplane --etcd-endpoints https://<cp1>:2379\n\
             4) join-config workers; install CNI with k8sServiceHost=<VIP>\n",
            gen.cluster_name,
            gen.endpoint,
            gen.token,
            gen.controlplane_yamls.len()
        ),
    )?;
    Ok(())
}

/// Patch worker YAML with CA PEM from bootstrap (keeps existing token).
pub fn patch_worker_ca(worker_yaml: &str, ca_pem: &str) -> Result<String> {
    let mut cfg = MachineConfig::from_yaml(worker_yaml)?;
    cfg.resolve_dashboard(None);
    if let Some(ref mut c) = cfg.cluster {
        c.ca = Some(ca_pem.trim().to_string());
    }
    Ok(serde_yaml::to_string(&cfg)?)
}

/// Fill ca/caKey/saKey (+ optional ca-only for workers) into a controlplane join YAML.
pub fn patch_controlplane_secrets(
    cp_yaml: &str,
    ca_pem: &str,
    ca_key_pem: &str,
    sa_key_pem: &str,
) -> Result<String> {
    let mut cfg = MachineConfig::from_yaml(cp_yaml)?;
    cfg.resolve_dashboard(None);
    if let Some(ref mut c) = cfg.cluster {
        c.ca = Some(ca_pem.trim().to_string());
        c.ca_key = Some(ca_key_pem.trim().to_string());
        c.sa_key = Some(sa_key_pem.trim().to_string());
    }
    Ok(serde_yaml::to_string(&cfg)?)
}
