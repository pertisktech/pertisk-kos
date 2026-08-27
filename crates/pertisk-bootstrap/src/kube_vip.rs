//! kube-vip ARP/ND VIP static pod for multi-CP HA.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::json;

pub const DEFAULT_KUBE_VIP_IMAGE: &str = "ghcr.io/kube-vip/kube-vip:v0.8.9";
pub const DEFAULT_KUBE_VIP_INTERFACE: &str = "eth0";

/// NIC that holds `advertise_ip`, else the first physical NIC, else eth0.
pub fn detect_vip_interface(advertise_ip: &str) -> String {
    pertisk_net::iface_holding_ipv4(advertise_ip)
        .or_else(pertisk_net::first_physical_iface)
        .unwrap_or_else(|| DEFAULT_KUBE_VIP_INTERFACE.to_string())
}

/// Write kube-vip when `vip` is set and differs from this node's advertise IP.
/// Optional `vip6` adds an IPv6 VIP (ND) alongside the IPv4 VIP when dual-stack HA.
pub fn maybe_write_kube_vip(
    manifests_dir: &Path,
    vip: Option<&str>,
    vip6: Option<&str>,
    advertise_ip: &str,
    interface: &str,
    nodename: &str,
) -> Result<bool> {
    let vip = vip.map(str::trim).filter(|s| !s.is_empty());
    let vip6 = vip6.map(str::trim).filter(|s| !s.is_empty());
    let Some(vip) = vip else {
        remove_kube_vip(manifests_dir);
        return Ok(false);
    };
    if vip == advertise_ip && vip6.is_none() {
        remove_kube_vip(manifests_dir);
        return Ok(false);
    }
    write_kube_vip(manifests_dir, vip, vip6, interface, nodename)?;
    Ok(true)
}

pub fn write_kube_vip(
    manifests_dir: &Path,
    vip: &str,
    vip6: Option<&str>,
    interface: &str,
    nodename: &str,
) -> Result<()> {
    fs::create_dir_all(manifests_dir)?;
    let iface = if interface.is_empty() {
        DEFAULT_KUBE_VIP_INTERFACE
    } else {
        interface
    };
    let (address, vip_cidr) = match vip6.map(str::trim).filter(|s| !s.is_empty()) {
        Some(v6) => (format!("{vip},{v6}"), "32,128".to_string()),
        None => (vip.to_string(), "32".to_string()),
    };
    // Shape matches `kube-vip manifest pod --controlplane --arp --leaderElection`
    // (v0.8.9). hostAliases is required: kube-vip talks to https://kubernetes:6443
    // using certs from the mounted admin.conf; without the alias, hostNetwork DNS
    // (e.g. 1.1.1.1) cannot resolve `kubernetes` and the VIP is never announced.
    // Use only vip_cidr — vip_subnet alongside it yields "10.x.x.x32" in v0.8.x.
    //
    // vip_nodename must be a stable hostname. fieldRef spec.nodeName is empty
    // until kubelet registers the Node — after a cluster reboot that race
    // leaves kube-vip unable to take the lease even when local apiserver is up.
    let nodename = nodename.trim();
    let nodename_env = if nodename.is_empty() {
        json!({
            "name": "vip_nodename",
            "valueFrom": { "fieldRef": { "fieldPath": "spec.nodeName" } }
        })
    } else {
        json!({ "name": "vip_nodename", "value": nodename })
    };
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
                    nodename_env,
                    { "name": "vip_interface", "value": iface },
                    { "name": "vip_cidr", "value": vip_cidr },
                    { "name": "dns_mode", "value": "first" },
                    { "name": "cp_enable", "value": "true" },
                    { "name": "cp_namespace", "value": "kube-system" },
                    { "name": "vip_leaderelection", "value": "true" },
                    { "name": "vip_leasename", "value": "plndr-cp-lock" },
                    { "name": "vip_leaseduration", "value": "15" },
                    { "name": "vip_renewdeadline", "value": "10" },
                    { "name": "vip_retryperiod", "value": "2" },
                    { "name": "address", "value": address },
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

/// IPv6 VIP from an existing kube-vip manifest (`address: v4,v6`).
pub fn vip6_from_manifest(yaml: &str) -> Option<String> {
    for line in yaml.lines() {
        let t = line.trim().trim_start_matches("- ").trim();
        let Some(v) = t.strip_prefix("value:") else {
            continue;
        };
        let v = v.trim().trim_matches('"').trim_matches('\'');
        if let Some((_, v6)) = v.split_once(',') {
            if v6.contains(':') {
                return Some(v6.trim().to_string());
            }
        }
    }
    None
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
        write_kube_vip(dir.path(), "10.1.1.200", None, "eth0", "lab-cp-1").unwrap();
        let y = fs::read_to_string(dir.path().join("kube-vip.yaml")).unwrap();
        assert!(y.contains("hostAliases") || y.contains("hostaliases"));
        assert!(y.contains("kubernetes"));
        assert!(y.contains("127.0.0.1"));
        assert!(y.contains("10.1.1.200"));
        assert!(y.contains("vip_arp"));
        assert!(y.contains("vip_cidr"));
        assert!(y.contains("lab-cp-1"));
        assert!(!y.contains("vip_subnet"));
    }

    #[test]
    fn kube_vip_dual_stack_address() {
        let dir = tempdir().unwrap();
        write_kube_vip(
            dir.path(),
            "10.1.1.200",
            Some("fd00:1::210"),
            "ens18",
            "lab-cp-1",
        )
        .unwrap();
        let y = fs::read_to_string(dir.path().join("kube-vip.yaml")).unwrap();
        assert!(y.contains("10.1.1.200,fd00:1::210") || y.contains("fd00:1::210"));
        assert!(y.contains("32,128"));
        assert!(y.contains("ens18"));
        assert_eq!(vip6_from_manifest(&y).as_deref(), Some("fd00:1::210"));
    }
}
