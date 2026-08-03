//! kube-vip ARP VIP static pod for multi-CP HA.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::json;

pub const DEFAULT_KUBE_VIP_IMAGE: &str = "ghcr.io/kube-vip/kube-vip:v0.8.9";
pub const DEFAULT_KUBE_VIP_INTERFACE: &str = "eth0";

/// Write kube-vip when `vip` is set and differs from this node's advertise IP.
pub fn maybe_write_kube_vip(
    manifests_dir: &Path,
    vip: Option<&str>,
    advertise_ip: &str,
    interface: &str,
) -> Result<bool> {
    let Some(vip) = vip.map(str::trim).filter(|s| !s.is_empty()) else {
        remove_kube_vip(manifests_dir);
        return Ok(false);
    };
    if vip == advertise_ip {
        remove_kube_vip(manifests_dir);
        return Ok(false);
    }
    write_kube_vip(manifests_dir, vip, interface)?;
    Ok(true)
}

pub fn write_kube_vip(manifests_dir: &Path, vip: &str, interface: &str) -> Result<()> {
    fs::create_dir_all(manifests_dir)?;
    let iface = if interface.is_empty() {
        DEFAULT_KUBE_VIP_INTERFACE
    } else {
        interface
    };
    // Shape matches `kube-vip manifest pod --controlplane --arp --leaderElection`
    // (v0.8.9). hostAliases is required: kube-vip talks to https://kubernetes:6443
    // using certs from the mounted admin.conf; without the alias, hostNetwork DNS
    // (e.g. 1.1.1.1) cannot resolve `kubernetes` and the VIP is never announced.
    // Use only vip_cidr — vip_subnet alongside it yields "10.x.x.x32" in v0.8.x.
    let pod = json!({
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": {
            "name": "kube-vip",
            "namespace": "kube-system",
            "labels": { "component": "kube-vip", "tier": "control-plane" }
        },
        "spec": {
            "hostNetwork": true,
            "hostAliases": [{
                "ip": "127.0.0.1",
                "hostnames": ["kubernetes"]
            }],
            "priorityClassName": "system-node-critical",
            "containers": [{
                "name": "kube-vip",
                "image": DEFAULT_KUBE_VIP_IMAGE,
                "imagePullPolicy": "IfNotPresent",
                "args": ["manager"],
                "env": [
                    { "name": "vip_arp", "value": "true" },
                    { "name": "port", "value": "6443" },
                    { "name": "vip_nodename", "valueFrom": { "fieldRef": { "fieldPath": "spec.nodeName" } } },
                    { "name": "vip_interface", "value": iface },
                    { "name": "vip_cidr", "value": "32" },
                    { "name": "dns_mode", "value": "first" },
                    { "name": "cp_enable", "value": "true" },
                    { "name": "cp_namespace", "value": "kube-system" },
                    { "name": "vip_leaderelection", "value": "true" },
                    { "name": "vip_leasename", "value": "plndr-cp-lock" },
                    { "name": "vip_leaseduration", "value": "15" },
                    { "name": "vip_renewdeadline", "value": "10" },
                    { "name": "vip_retryperiod", "value": "2" },
                    { "name": "address", "value": vip },
                    { "name": "prometheus_server", "value": ":2112" }
                ],
                "securityContext": {
                    "capabilities": {
                        "add": ["NET_ADMIN", "NET_RAW"]
                    }
                },
                "volumeMounts": [{
                    "name": "kubeconfig",
                    "mountPath": "/etc/kubernetes/admin.conf"
                }]
            }],
            "volumes": [{
                "name": "kubeconfig",
                "hostPath": {
                    "path": "/etc/kubernetes/admin.conf"
                }
            }]
        }
    });
    let path = manifests_dir.join("kube-vip.yaml");
    let s = serde_yaml::to_string(&pod).context("serialize kube-vip")?;
    fs::write(&path, s).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn remove_kube_vip(manifests_dir: &Path) {
    let _ = fs::remove_file(manifests_dir.join("kube-vip.yaml"));
}

/// VIP address from `cluster.endpoint` when it is an IP (not a DNS name alone).
pub fn vip_from_endpoint_host(endpoint_host: &str) -> Option<&str> {
    let h = endpoint_host.trim();
    if h.parse::<std::net::IpAddr>().is_ok() {
        Some(h)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn kube_vip_sets_host_aliases_and_cidr() {
        let dir = tempdir().unwrap();
        write_kube_vip(dir.path(), "10.1.1.200", "eth0").unwrap();
        let y = fs::read_to_string(dir.path().join("kube-vip.yaml")).unwrap();
        assert!(y.contains("hostAliases") || y.contains("hostaliases"));
        assert!(y.contains("kubernetes"));
        assert!(y.contains("127.0.0.1"));
        assert!(y.contains("10.1.1.200"));
        assert!(y.contains("vip_arp"));
        assert!(y.contains("vip_cidr"));
        assert!(!y.contains("vip_subnet"));
    }
}
