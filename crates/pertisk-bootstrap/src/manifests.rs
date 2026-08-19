//! Apply multi-document Kubernetes YAML via the local apiserver client.

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::api::KubeClient;
use crate::ensure_created;

pub fn apply_yaml_documents(client: &KubeClient, yaml: &str) -> Result<()> {
    for doc in yaml.split("---") {
        let trimmed = doc.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Skip comment-only leading docs.
        let value: serde_yaml::Value = match serde_yaml::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if value.is_null() {
            continue;
        }
        let json: Value = serde_json::to_value(value).context("yaml → json")?;
        let kind = json
            .get("kind")
            .and_then(|k| k.as_str())
            .context("manifest missing kind")?;
        let name = json
            .pointer("/metadata/name")
            .and_then(|n| n.as_str())
            .context("manifest missing metadata.name")?;
        let ns = json.pointer("/metadata/namespace").and_then(|n| n.as_str());
        let path = collection_path(kind, ns)?;
        ensure_created(client, path, &json.to_string(), name)?;
    }
    Ok(())
}

fn collection_path(kind: &str, namespace: Option<&str>) -> Result<&'static str> {
    Ok(match (kind, namespace) {
        ("ServiceAccount", Some("kube-system")) => "/api/v1/namespaces/kube-system/serviceaccounts",
        ("ConfigMap", Some("kube-system")) => "/api/v1/namespaces/kube-system/configmaps",
        ("Service", Some("kube-system")) => "/api/v1/namespaces/kube-system/services",
        ("Deployment", Some("kube-system")) => "/apis/apps/v1/namespaces/kube-system/deployments",
        ("RoleBinding", Some("kube-system")) => {
            "/apis/rbac.authorization.k8s.io/v1/namespaces/kube-system/rolebindings"
        }
        ("ClusterRole", _) => "/apis/rbac.authorization.k8s.io/v1/clusterroles",
        ("ClusterRoleBinding", _) => "/apis/rbac.authorization.k8s.io/v1/clusterrolebindings",
        ("APIService", _) => "/apis/apiregistration.k8s.io/v1/apiservices",
        (k, ns) => bail!("unsupported bootstrap manifest kind={k} ns={ns:?}"),
    })
}
