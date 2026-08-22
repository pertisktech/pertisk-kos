//! Cluster overview: component / package versions.

use serde::Serialize;

use crate::routes::nodes::NodeOut;

/// Pins must match `pertisk-bootstrap` / `pertisk-runtime` image defaults.
pub const DEFAULT_ETCD_IMAGE: &str = "registry.k8s.io/etcd:3.5.16-0";
pub const DEFAULT_PAUSE_IMAGE: &str = "registry.k8s.io/pause:3.10";
pub const DEFAULT_KUBE_VIP_IMAGE: &str = "ghcr.io/kube-vip/kube-vip:v0.8.9";

#[derive(Debug, Clone, Serialize)]
pub struct NodeComponentVersion {
    pub name: String,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ComponentVersion {
    pub id: String,
    pub name: String,
    /// Uniform running version, `"mixed"`, or `"—"`.
    pub version: String,
    /// Cluster Kubernetes spec, or last successful OS A/B upgrade for this cluster.
    pub desired: Option<String>,
    pub source: String,
    pub mixed: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<NodeComponentVersion>,
}

pub struct ClusterVersionCtx<'a> {
    pub k8s_version: &'a str,
    pub cni: &'a str,
    pub catalog_os: Option<&'a str>,
    pub has_vip: bool,
}

pub fn summarize(cluster: &ClusterVersionCtx<'_>, nodes: &[NodeOut]) -> Vec<ComponentVersion> {
    let mut out = Vec::new();

    out.push(from_nodes(
        "kubernetes",
        "Kubernetes",
        nodes,
        |n| n.k8s_version.as_deref(),
        Some(cluster.k8s_version),
    ));
    out.push(from_nodes(
        "os",
        "OS",
        nodes,
        |n| n.os_version.as_deref(),
        cluster.catalog_os,
    ));
    out.push(from_nodes(
        "kernel",
        "Kernel",
        nodes,
        |n| n.kernel_version.as_deref(),
        None,
    ));
    out.push(from_nodes(
        "containerd",
        "containerd",
        nodes,
        |n| n.container_runtime.as_deref(),
        None,
    ));

    out.push(ComponentVersion {
        id: "cni".into(),
        name: "CNI".into(),
        version: nonempty(cluster.cni).unwrap_or_else(|| "—".into()),
        desired: None,
        source: "cluster".into(),
        mixed: false,
        nodes: Vec::new(),
    });

    out.push(pinned("etcd", "etcd", DEFAULT_ETCD_IMAGE));
    out.push(pinned("pause", "pause", DEFAULT_PAUSE_IMAGE));
    if cluster.has_vip {
        out.push(pinned("kube-vip", "kube-vip", DEFAULT_KUBE_VIP_IMAGE));
    }

    out
}

fn pinned(id: &str, name: &str, image: &str) -> ComponentVersion {
    ComponentVersion {
        id: id.into(),
        name: name.into(),
        version: image_tag(image),
        desired: None,
        source: "image".into(),
        mixed: false,
        nodes: Vec::new(),
    }
}

fn from_nodes(
    id: &str,
    name: &str,
    nodes: &[NodeOut],
    pick: fn(&NodeOut) -> Option<&str>,
    desired: Option<&str>,
) -> ComponentVersion {
    let breakdown: Vec<NodeComponentVersion> = nodes
        .iter()
        .map(|n| NodeComponentVersion {
            name: n.name.clone(),
            version: nonempty_opt(pick(n)),
        })
        .collect();
    let mut unique: Vec<String> = Vec::new();
    for row in &breakdown {
        if let Some(v) = row.version.as_deref() {
            if !unique.iter().any(|x| x == v) {
                unique.push(v.to_string());
            }
        }
    }
    let mixed = unique.len() > 1;
    let version = match unique.len() {
        0 => "—".to_string(),
        1 => unique[0].clone(),
        _ => "mixed".to_string(),
    };
    ComponentVersion {
        id: id.into(),
        name: name.into(),
        version,
        desired: nonempty_opt(desired),
        source: "nodes".into(),
        mixed,
        nodes: if mixed { breakdown } else { Vec::new() },
    }
}

fn nonempty(s: &str) -> Option<String> {
    nonempty_opt(Some(s))
}

fn nonempty_opt(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string())
}

fn image_tag(image: &str) -> String {
    match image.rsplit_once(':') {
        Some((_, tag)) if !tag.is_empty() && !tag.contains('/') => tag.to_string(),
        _ => image.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, k8s: Option<&str>, os: Option<&str>) -> NodeOut {
        NodeOut {
            id: name.into(),
            cluster_id: "c1".into(),
            name: name.into(),
            role: "worker".into(),
            vmid: None,
            ip: None,
            ip6: None,
            k8s_version: k8s.map(|s| s.into()),
            os_version: os.map(|s| s.into()),
            kernel_version: Some("6.6.142-0-virt".into()),
            container_runtime: Some("2.3.4".into()),
            memory: None,
            cores: None,
            disk_gb: None,
            ak_public_b64: None,
            ak_enrolled_at: None,
            source: "proxmox".into(),
            status: "ready".into(),
            availability: "online".into(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn mixed_kubernetes_and_catalog_os() {
        let nodes = vec![
            node("cp-1", Some("v1.36.3"), Some("0.2.87")),
            node("wk-1", Some("v1.36.2"), Some("0.2.87")),
        ];
        let rows = summarize(
            &ClusterVersionCtx {
                k8s_version: "v1.36.3",
                cni: "cilium",
                catalog_os: Some("0.2.90"),
                has_vip: true,
            },
            &nodes,
        );
        let k8s = rows.iter().find(|r| r.id == "kubernetes").unwrap();
        assert!(k8s.mixed);
        assert_eq!(k8s.version, "mixed");
        assert_eq!(k8s.desired.as_deref(), Some("v1.36.3"));
        assert_eq!(k8s.nodes.len(), 2);

        let os = rows.iter().find(|r| r.id == "os").unwrap();
        assert!(!os.mixed);
        assert_eq!(os.version, "0.2.87");
        assert_eq!(os.desired.as_deref(), Some("0.2.90"));

        let etcd = rows.iter().find(|r| r.id == "etcd").unwrap();
        assert_eq!(etcd.version, "3.5.16-0");
        assert!(rows.iter().any(|r| r.id == "kube-vip"));
        assert_eq!(
            rows.iter().find(|r| r.id == "cni").unwrap().version,
            "cilium"
        );
    }

    #[test]
    fn image_tag_strips_registry() {
        assert_eq!(image_tag(DEFAULT_PAUSE_IMAGE), "3.10");
        assert_eq!(image_tag(DEFAULT_KUBE_VIP_IMAGE), "v0.8.9");
    }
}
