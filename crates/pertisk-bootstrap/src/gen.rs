//! Offline `pertiskctl gen config` helpers.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use pertisk_config::{
    Cluster, CniMode, Dashboard, Interface, Machine, MachineConfig, MachineType, Network,
    CONFIG_VERSION,
};

use crate::token::generate_bootstrap_token;

pub struct GenConfigOutput {
    pub cluster_name: String,
    pub endpoint: String,
    pub token: String,
    pub controlplane_yaml: String,
    pub worker_yaml: String,
}

/// Generate controlplane + worker machine configs (CA filled after bootstrap).
pub fn gen_config(
    cluster_name: &str,
    endpoint: &str,
    kubernetes_version: &str,
    pod_subnet: &str,
    service_subnet: &str,
) -> Result<GenConfigOutput> {
    let token = generate_bootstrap_token();
    let endpoint = normalize_endpoint(endpoint);

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
            dashboard: Some(Dashboard::builtin()),
        },
        cluster: Some(Cluster {
            endpoint: endpoint.clone(),
            token: Some(token.clone()),
            ca: None,
            ca_key: None,
            sa_key: None,
            pod_subnet: Some(pod_subnet.into()),
            service_subnet: Some(service_subnet.into()),
            kubernetes_version: Some(kubernetes_version.into()),
            pod_cidr: None,
            cni: CniMode::None,
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
            dashboard: Some(Dashboard::builtin()),
        },
        cluster: Some(Cluster {
            endpoint: endpoint.clone(),
            token: Some(token.clone()),
            ca: None, // fill via `pertiskctl join-config` after bootstrap
            ca_key: None,
            sa_key: None,
            pod_subnet: Some(pod_subnet.into()),
            service_subnet: Some(service_subnet.into()),
            kubernetes_version: Some(kubernetes_version.into()),
            pod_cidr: None,
            cni: CniMode::None,
        }),
    };

    Ok(GenConfigOutput {
        cluster_name: cluster_name.into(),
        endpoint,
        token,
        controlplane_yaml: serde_yaml::to_string(&cp)?,
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
             5) Apply worker.yaml to each worker; install CNI (examples/cni/)\n",
            gen.cluster_name, gen.endpoint, gen.token
        ),
    )?;
    Ok(())
}

fn normalize_endpoint(endpoint: &str) -> String {
    let e = endpoint.trim();
    if e.starts_with("https://") || e.starts_with("http://") {
        e.to_string()
    } else {
        format!("https://{e}")
    }
}

/// Patch worker YAML with CA PEM from bootstrap (keeps existing token).
/// Fills built-in dashboard when the file omits it (same as apply).
pub fn patch_worker_ca(worker_yaml: &str, ca_pem: &str) -> Result<String> {
    let mut cfg = MachineConfig::from_yaml(worker_yaml)?;
    cfg.resolve_dashboard(None);
    if let Some(ref mut c) = cfg.cluster {
        c.ca = Some(ca_pem.trim().to_string());
    }
    Ok(serde_yaml::to_string(&cfg)?)
}
