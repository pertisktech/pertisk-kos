//! kubeadm-shaped static pod manifests for the control plane.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::json;

pub struct StaticPodParams<'a> {
    pub advertise_ip: &'a str,
    pub hostname: &'a str,
    pub kubernetes_version: &'a str,
    pub etcd_image: &'a str,
    pub service_cidr: &'a str,
    pub pod_subnet: &'a str,
    pub pki_host_path: &'a str,
    /// e.g. `cp-1=https://10.0.0.1:2380` or multi-member CSV for joiners.
    pub etcd_initial_cluster: &'a str,
    /// `new` (first CP) or `existing` (joined CP).
    pub etcd_initial_cluster_state: &'a str,
}

pub fn write_static_pods(manifests_dir: &Path, p: &StaticPodParams<'_>) -> Result<()> {
    fs::create_dir_all(manifests_dir)?;
    let ver = if p.kubernetes_version.starts_with('v') {
        p.kubernetes_version.to_string()
    } else {
        format!("v{}", p.kubernetes_version)
    };

    write_yaml(&manifests_dir.join("etcd.yaml"), &etcd_pod(p))?;
    write_yaml(
        &manifests_dir.join("kube-apiserver.yaml"),
        &apiserver_pod(p.advertise_ip, &ver, p.service_cidr, p.pki_host_path),
    )?;
    write_yaml(
        &manifests_dir.join("kube-controller-manager.yaml"),
        &controller_manager_pod(&ver, p.pod_subnet, p.service_cidr, p.pki_host_path),
    )?;
    write_yaml(
        &manifests_dir.join("kube-scheduler.yaml"),
        &scheduler_pod(&ver, p.pki_host_path),
    )?;
    Ok(())
}

/// Rewrite control-plane static-pod image tags to `kubernetes_version`.
/// Leaves etcd / kube-vip untouched (cluster membership + VIP must not churn).
/// Returns how many manifest files were updated.
pub fn bump_control_plane_images(manifests_dir: &Path, kubernetes_version: &str) -> Result<usize> {
    let ver = normalize_k8s_version(kubernetes_version);
    let files = [
        "kube-apiserver.yaml",
        "kube-controller-manager.yaml",
        "kube-scheduler.yaml",
    ];
    let mut changed = 0usize;
    for name in files {
        let path = manifests_dir.join(name);
        if !path.is_file() {
            continue;
        }
        let old = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let new = bump_registry_k8s_image_tags(&old, &ver);
        if new != old {
            fs::write(&path, new).with_context(|| format!("write {}", path.display()))?;
            changed += 1;
        }
    }
    Ok(changed)
}

fn normalize_k8s_version(v: &str) -> String {
    let v = v.trim();
    if v.starts_with('v') {
        v.to_string()
    } else {
        format!("v{v}")
    }
}

/// Replace `registry.k8s.io/kube-*:vX.Y.Z` image tags with `ver` (already normalized).
fn bump_registry_k8s_image_tags(yaml: &str, ver: &str) -> String {
    let mut out = String::with_capacity(yaml.len());
    for line in yaml.lines() {
        if let Some(idx) = line.find("registry.k8s.io/kube-") {
            if let Some(colon) = line[idx..].find(':') {
                let abs = idx + colon;
                // Keep prefix through ':'; rewrite tag (stop at whitespace / comment).
                let rest = &line[abs + 1..];
                let tag_end = rest
                    .find(|c: char| c.is_whitespace() || c == '#' || c == '"')
                    .unwrap_or(rest.len());
                out.push_str(&line[..=abs]);
                out.push_str(ver);
                out.push_str(&rest[tag_end..]);
                out.push('\n');
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn write_yaml(path: &Path, value: &serde_json::Value) -> Result<()> {
    let s = serde_yaml::to_string(value).context("serialize pod")?;
    fs::write(path, s).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn etcd_pod(p: &StaticPodParams<'_>) -> serde_json::Value {
    let advertise_ip = p.advertise_ip;
    let name = p.hostname;
    json!({
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": {
            "name": "etcd",
            "namespace": "kube-system",
            "labels": { "component": "etcd", "tier": "control-plane" }
        },
        "spec": {
            "hostNetwork": true,
            "priorityClassName": "system-node-critical",
            "containers": [{
                "name": "etcd",
                "image": p.etcd_image,
                "command": [
                    "etcd",
                    format!("--advertise-client-urls=https://{advertise_ip}:2379,https://127.0.0.1:2379"),
                    "--cert-file=/etc/kubernetes/pki/etcd/server.crt",
                    "--client-cert-auth=true",
                    "--data-dir=/var/lib/etcd",
                    format!("--initial-advertise-peer-urls=https://{advertise_ip}:2380"),
                    format!("--initial-cluster={}", p.etcd_initial_cluster),
                    format!("--initial-cluster-state={}", p.etcd_initial_cluster_state),
                    format!("--listen-client-urls=https://127.0.0.1:2379,https://{advertise_ip}:2379"),
                    "--listen-metrics-urls=http://127.0.0.1:2381",
                    format!("--listen-peer-urls=https://0.0.0.0:2380"),
                    format!("--name={name}"),
                    "--peer-cert-file=/etc/kubernetes/pki/etcd/peer.crt",
                    "--peer-client-cert-auth=true",
                    "--peer-key-file=/etc/kubernetes/pki/etcd/peer.key",
                    "--peer-trusted-ca-file=/etc/kubernetes/pki/etcd/ca.crt",
                    "--snapshot-count=10000",
                    "--key-file=/etc/kubernetes/pki/etcd/server.key",
                    "--trusted-ca-file=/etc/kubernetes/pki/etcd/ca.crt"
                ],
                "volumeMounts": [
                    { "name": "etcd-data", "mountPath": "/var/lib/etcd" },
                    { "name": "pki", "mountPath": "/etc/kubernetes/pki" }
                ]
            }],
            "volumes": [
                { "name": "etcd-data", "hostPath": { "path": "/var/lib/etcd", "type": "DirectoryOrCreate" } },
                { "name": "pki", "hostPath": { "path": p.pki_host_path, "type": "Directory" } }
            ]
        }
    })
}

fn apiserver_pod(
    advertise_ip: &str,
    ver: &str,
    service_cidr: &str,
    pki: &str,
) -> serde_json::Value {
    let image = format!("registry.k8s.io/kube-apiserver:{ver}");
    json!({
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": {
            "name": "kube-apiserver",
            "namespace": "kube-system",
            "labels": { "component": "kube-apiserver", "tier": "control-plane" }
        },
        "spec": {
            "hostNetwork": true,
            "priorityClassName": "system-node-critical",
            "containers": [{
                "name": "kube-apiserver",
                "image": image,
                "command": [
                    "kube-apiserver",
                    format!("--advertise-address={advertise_ip}"),
                    "--allow-privileged=true",
                    "--authorization-mode=Node,RBAC",
                    "--client-ca-file=/etc/kubernetes/pki/ca.crt",
                    "--enable-admission-plugins=NodeRestriction",
                    "--enable-bootstrap-token-auth=true",
                    "--etcd-cafile=/etc/kubernetes/pki/etcd/ca.crt",
                    "--etcd-certfile=/etc/kubernetes/pki/etcd/server.crt",
                    "--etcd-keyfile=/etc/kubernetes/pki/etcd/server.key",
                    "--etcd-servers=https://127.0.0.1:2379",
                    "--kubelet-client-certificate=/etc/kubernetes/pki/apiserver.crt",
                    "--kubelet-client-key=/etc/kubernetes/pki/apiserver.key",
                    "--kubelet-preferred-address-types=InternalIP,Hostname,ExternalIP",
                    "--proxy-client-cert-file=/etc/kubernetes/pki/front-proxy-client.crt",
                    "--proxy-client-key-file=/etc/kubernetes/pki/front-proxy-client.key",
                    "--requestheader-allowed-names=front-proxy-client",
                    "--requestheader-client-ca-file=/etc/kubernetes/pki/front-proxy-ca.crt",
                    "--requestheader-extra-headers-prefix=X-Remote-Extra-",
                    "--requestheader-group-headers=X-Remote-Group",
                    "--requestheader-username-headers=X-Remote-User",
                    "--secure-port=6443",
                    format!("--service-cluster-ip-range={service_cidr}"),
                    "--service-account-issuer=https://kubernetes.default.svc.cluster.local",
                    "--service-account-key-file=/etc/kubernetes/pki/sa.pub",
                    "--service-account-signing-key-file=/etc/kubernetes/pki/sa.key",
                    "--tls-cert-file=/etc/kubernetes/pki/apiserver.crt",
                    "--tls-private-key-file=/etc/kubernetes/pki/apiserver.key",
                    "--feature-gates=UserNamespacesSupport=true"
                ],
                "volumeMounts": [
                    { "name": "pki", "mountPath": "/etc/kubernetes/pki" }
                ]
            }],
            "volumes": [
                { "name": "pki", "hostPath": { "path": pki, "type": "Directory" } }
            ]
        }
    })
}

fn controller_manager_pod(
    ver: &str,
    pod_subnet: &str,
    service_cidr: &str,
    pki: &str,
) -> serde_json::Value {
    let image = format!("registry.k8s.io/kube-controller-manager:{ver}");
    json!({
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": {
            "name": "kube-controller-manager",
            "namespace": "kube-system",
            "labels": { "component": "kube-controller-manager", "tier": "control-plane" }
        },
        "spec": {
            "hostNetwork": true,
            "priorityClassName": "system-node-critical",
            "containers": [{
                "name": "kube-controller-manager",
                "image": image,
                "command": [
                    "kube-controller-manager",
                    "--allocate-node-cidrs=true",
                    "--authentication-kubeconfig=/etc/kubernetes/controller-manager.conf",
                    "--authorization-kubeconfig=/etc/kubernetes/controller-manager.conf",
                    "--bind-address=127.0.0.1",
                    "--client-ca-file=/etc/kubernetes/pki/ca.crt",
                    format!("--cluster-cidr={pod_subnet}"),
                    "--cluster-signing-cert-file=/etc/kubernetes/pki/ca.crt",
                    "--cluster-signing-key-file=/etc/kubernetes/pki/ca.key",
                    "--controllers=*,bootstrapsigner,tokencleaner",
                    "--kubeconfig=/etc/kubernetes/controller-manager.conf",
                    "--leader-elect=true",
                    "--requestheader-client-ca-file=/etc/kubernetes/pki/front-proxy-ca.crt",
                    "--root-ca-file=/etc/kubernetes/pki/ca.crt",
                    "--service-account-private-key-file=/etc/kubernetes/pki/sa.key",
                    format!("--service-cluster-ip-range={service_cidr}"),
                    "--use-service-account-credentials=true"
                ],
                "volumeMounts": [
                    { "name": "pki", "mountPath": "/etc/kubernetes/pki" },
                    {
                        "name": "kubeconfig",
                        "mountPath": "/etc/kubernetes/controller-manager.conf"
                    }
                ]
            }],
            "volumes": [
                { "name": "pki", "hostPath": { "path": pki, "type": "Directory" } },
                {
                    "name": "kubeconfig",
                    "hostPath": {
                        "path": "/etc/kubernetes/controller-manager.conf",
                        "type": "File"
                    }
                }
            ]
        }
    })
}

fn scheduler_pod(ver: &str, _pki: &str) -> serde_json::Value {
    let image = format!("registry.k8s.io/kube-scheduler:{ver}");
    json!({
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": {
            "name": "kube-scheduler",
            "namespace": "kube-system",
            "labels": { "component": "kube-scheduler", "tier": "control-plane" }
        },
        "spec": {
            "hostNetwork": true,
            "priorityClassName": "system-node-critical",
            "containers": [{
                "name": "kube-scheduler",
                "image": image,
                "command": [
                    "kube-scheduler",
                    "--authentication-kubeconfig=/etc/kubernetes/scheduler.conf",
                    "--authorization-kubeconfig=/etc/kubernetes/scheduler.conf",
                    "--bind-address=127.0.0.1",
                    "--kubeconfig=/etc/kubernetes/scheduler.conf",
                    "--leader-elect=true"
                ],
                "volumeMounts": [{
                    "name": "kubeconfig",
                    "mountPath": "/etc/kubernetes/scheduler.conf"
                }]
            }],
            "volumes": [{
                "name": "kubeconfig",
                "hostPath": {
                    "path": "/etc/kubernetes/scheduler.conf",
                    "type": "File"
                }
            }]
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn etcd_pod_uses_node_peer_urls() {
        let dir = tempdir().unwrap();
        write_static_pods(
            dir.path(),
            &StaticPodParams {
                advertise_ip: "10.1.1.10",
                hostname: "lab-cp-1",
                kubernetes_version: "v1.36.3",
                etcd_image: "registry.k8s.io/etcd:3.5.16-0",
                service_cidr: "10.96.0.0/12",
                pod_subnet: "10.244.0.0/16",
                pki_host_path: "/etc/kubernetes/pki",
                etcd_initial_cluster: "lab-cp-1=https://10.1.1.10:2380",
                etcd_initial_cluster_state: "new",
            },
        )
        .unwrap();
        let etcd = fs::read_to_string(dir.path().join("etcd.yaml")).unwrap();
        assert!(etcd.contains("--name=lab-cp-1"));
        assert!(etcd.contains("https://10.1.1.10:2380"));
        assert!(etcd.contains("--initial-cluster-state=new"));
        assert!(etcd.contains("0.0.0.0:2380"));
    }

    #[test]
    fn bump_images_updates_cp_only() {
        let dir = tempdir().unwrap();
        write_static_pods(
            dir.path(),
            &StaticPodParams {
                advertise_ip: "10.1.1.10",
                hostname: "lab-cp-1",
                kubernetes_version: "v1.36.2",
                etcd_image: "registry.k8s.io/etcd:3.5.16-0",
                service_cidr: "10.96.0.0/12",
                pod_subnet: "10.244.0.0/16",
                pki_host_path: "/etc/kubernetes/pki",
                etcd_initial_cluster: "lab-cp-1=https://10.1.1.10:2380",
                etcd_initial_cluster_state: "new",
            },
        )
        .unwrap();
        assert_eq!(bump_control_plane_images(dir.path(), "v1.36.3").unwrap(), 3);
        let api = fs::read_to_string(dir.path().join("kube-apiserver.yaml")).unwrap();
        assert!(api.contains("registry.k8s.io/kube-apiserver:v1.36.3"));
        assert!(!api.contains(":v1.36.2"));
        let etcd = fs::read_to_string(dir.path().join("etcd.yaml")).unwrap();
        assert!(etcd.contains("registry.k8s.io/etcd:3.5.16-0"));
    }
}
