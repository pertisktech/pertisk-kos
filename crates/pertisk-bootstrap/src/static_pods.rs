//! kubeadm-shaped static pod manifests for the control plane.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::json;

pub struct StaticPodParams<'a> {
    pub advertise_ip: &'a str,
    pub kubernetes_version: &'a str,
    pub etcd_image: &'a str,
    pub service_cidr: &'a str,
    pub pod_subnet: &'a str,
    pub pki_host_path: &'a str,
}

pub fn write_static_pods(manifests_dir: &Path, p: &StaticPodParams<'_>) -> Result<()> {
    fs::create_dir_all(manifests_dir)?;
    let k8s_ver = p.kubernetes_version.trim_start_matches('v');
    let ver = if p.kubernetes_version.starts_with('v') {
        p.kubernetes_version.to_string()
    } else {
        format!("v{}", p.kubernetes_version)
    };
    let _ = k8s_ver;

    write_yaml(
        &manifests_dir.join("etcd.yaml"),
        &etcd_pod(p.advertise_ip, p.etcd_image, p.pki_host_path),
    )?;
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

fn write_yaml(path: &Path, value: &serde_json::Value) -> Result<()> {
    let s = serde_yaml::to_string(value).context("serialize pod")?;
    fs::write(path, s).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn etcd_pod(advertise_ip: &str, image: &str, pki: &str) -> serde_json::Value {
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
                "image": image,
                "command": [
                    "etcd",
                    "--advertise-client-urls=https://127.0.0.1:2379",
                    "--cert-file=/etc/kubernetes/pki/etcd/server.crt",
                    "--client-cert-auth=true",
                    "--data-dir=/var/lib/etcd",
                    "--initial-advertise-peer-urls=https://127.0.0.1:2380",
                    "--initial-cluster=etcd=https://127.0.0.1:2380",
                    format!("--listen-client-urls=https://127.0.0.1:2379,https://{advertise_ip}:2379"),
                    "--listen-metrics-urls=http://127.0.0.1:2381",
                    "--listen-peer-urls=https://127.0.0.1:2380",
                    "--name=etcd",
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
                { "name": "pki", "hostPath": { "path": pki, "type": "Directory" } }
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
                    "--tls-private-key-file=/etc/kubernetes/pki/apiserver.key"
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
                    // File hostPath — avoids subPath (needs `mount`) and dir
                    // mounts that expose broken absolute PKI symlinks.
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
