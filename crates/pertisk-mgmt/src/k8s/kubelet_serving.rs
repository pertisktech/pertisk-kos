//! Approve kubelet serving CSRs for registered nodes.
//!
//! Kubernetes leaves `kubernetes.io/kubelet-serving` Pending on purpose. First
//! control-plane bootstrap and lab-up approve the initial set; scaled/adopted
//! workers need the same check (signer + system:nodes + Node object exists).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::process::Command;
use tokio::time::sleep;

const SIGNER: &str = "kubernetes.io/kubelet-serving";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServingCsr {
    pub name: String,
    pub node: String,
    pub issued: bool,
    pub terminal: bool,
}

pub fn parse_kubelet_serving_csrs(list: &Value) -> Vec<ServingCsr> {
    let Some(items) = list.get("items").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for csr in items {
        let Some(name) = csr
            .pointer("/metadata/name")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let spec = csr.get("spec").unwrap_or(&Value::Null);
        if spec.get("signerName").and_then(Value::as_str) != Some(SIGNER) {
            continue;
        }
        let username = spec.get("username").and_then(Value::as_str).unwrap_or("");
        let Some(node) = username.strip_prefix("system:node:") else {
            continue;
        };
        let groups_ok = spec
            .get("groups")
            .and_then(Value::as_array)
            .is_some_and(|groups| groups.iter().any(|g| g.as_str() == Some("system:nodes")));
        if !groups_ok {
            continue;
        }
        let conditions = csr
            .pointer("/status/conditions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let terminal = conditions.iter().any(|c| {
            matches!(
                c.get("type").and_then(Value::as_str),
                Some("Approved") | Some("Denied")
            )
        });
        let issued = csr
            .pointer("/status/certificate")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty());
        out.push(ServingCsr {
            name: name.to_string(),
            node: node.to_string(),
            issued,
            terminal,
        });
    }
    out
}

pub fn pending_for_registered<'a>(
    csrs: &'a [ServingCsr],
    registered: &HashSet<String>,
) -> Vec<&'a ServingCsr> {
    csrs.iter()
        .filter(|c| !c.terminal && registered.contains(&c.node))
        .collect()
}

fn node_serving_issued(csrs: &[ServingCsr], node: &str) -> bool {
    csrs.iter()
        .any(|c| c.node == node && c.issued && c.terminal)
}

async fn kubectl_json(kubeconfig: &Path, args: &[&str]) -> anyhow::Result<Value> {
    let mut cmd = Command::new("kubectl");
    cmd.arg("--kubeconfig").arg(kubeconfig).args(args);
    let out = cmd.output().await?;
    if !out.status.success() {
        anyhow::bail!(
            "kubectl {} failed: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    serde_json::from_slice(&out.stdout).map_err(|e| anyhow::anyhow!("kubectl json: {e}"))
}

async fn list_nodes(kubeconfig: &Path) -> anyhow::Result<HashSet<String>> {
    let doc = kubectl_json(kubeconfig, &["get", "nodes", "-o", "json"]).await?;
    let mut names = HashSet::new();
    if let Some(items) = doc.get("items").and_then(Value::as_array) {
        for item in items {
            if let Some(n) = item.pointer("/metadata/name").and_then(Value::as_str) {
                names.insert(n.to_string());
            }
        }
    }
    Ok(names)
}

async fn list_serving_csrs(kubeconfig: &Path) -> anyhow::Result<Vec<ServingCsr>> {
    let doc = kubectl_json(
        kubeconfig,
        &["get", "csr", "-o", "json", "--request-timeout=15s"],
    )
    .await?;
    Ok(parse_kubelet_serving_csrs(&doc))
}

async fn approve_csr(kubeconfig: &Path, name: &str) -> anyhow::Result<()> {
    let mut cmd = Command::new("kubectl");
    cmd.arg("--kubeconfig").arg(kubeconfig).args([
        "certificate",
        "approve",
        name,
        "--request-timeout=15s",
    ]);
    let out = cmd.output().await?;
    if !out.status.success() {
        anyhow::bail!(
            "kubectl certificate approve {name} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

fn throttle_map() -> &'static Mutex<HashMap<String, Instant>> {
    static M: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Same as [`approve_pending_kubelet_serving_csrs`], skipped if this kubeconfig
/// was scanned in the last 20s (dashboard / cluster GET).
pub async fn approve_pending_kubelet_serving_csrs_throttled(
    kubeconfig: &Path,
) -> anyhow::Result<Vec<String>> {
    let key = kubeconfig.to_string_lossy().to_string();
    if let Ok(mut g) = throttle_map().lock() {
        if let Some(at) = g.get(&key) {
            if at.elapsed() < Duration::from_secs(20) {
                return Ok(Vec::new());
            }
        }
        g.insert(key, Instant::now());
    }
    approve_pending_kubelet_serving_csrs(kubeconfig).await
}

/// Approve Pending kubelet-serving CSRs whose requester Node already exists.
pub async fn approve_pending_kubelet_serving_csrs(
    kubeconfig: &Path,
) -> anyhow::Result<Vec<String>> {
    if !kubeconfig.is_file() {
        return Ok(Vec::new());
    }
    let registered = list_nodes(kubeconfig).await?;
    let csrs = list_serving_csrs(kubeconfig).await?;
    let mut approved = Vec::new();
    for csr in pending_for_registered(&csrs, &registered) {
        approve_csr(kubeconfig, &csr.name).await?;
        approved.push(csr.name.clone());
        tracing::info!(csr = %csr.name, node = %csr.node, "approved kubelet serving CSR");
    }
    Ok(approved)
}

/// Wait until `node` has an issued kubelet serving certificate, approving CSRs
/// from registered nodes along the way.
pub async fn wait_kubelet_serving_cert(
    kubeconfig: &Path,
    node: &str,
    timeout: Duration,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + timeout;
    let mut last_err = None;
    while Instant::now() < deadline {
        match approve_pending_kubelet_serving_csrs(kubeconfig).await {
            Ok(names) if !names.is_empty() => {
                tracing::info!(node, approved = ?names, "approved kubelet serving CSRs");
            }
            Ok(_) => {}
            Err(e) => last_err = Some(e),
        }
        match list_serving_csrs(kubeconfig).await {
            Ok(csrs) if node_serving_issued(&csrs, node) => return Ok(()),
            Ok(_) => {}
            Err(e) => last_err = Some(e),
        }
        sleep(Duration::from_secs(3)).await;
    }
    if let Some(err) = last_err {
        anyhow::bail!("kubelet serving cert for {node} not issued: {err:#}");
    }
    anyhow::bail!(
        "kubelet serving cert for {node} not issued within {}s",
        timeout.as_secs()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn skips_non_serving_and_unregistered() {
        let list = json!({
            "items": [
                {
                    "metadata": { "name": "csr-client" },
                    "spec": {
                        "signerName": "kubernetes.io/kube-apiserver-client-kubelet",
                        "username": "system:node:wk-1",
                        "groups": ["system:nodes"]
                    }
                },
                {
                    "metadata": { "name": "csr-pending" },
                    "spec": {
                        "signerName": "kubernetes.io/kubelet-serving",
                        "username": "system:node:wk-2",
                        "groups": ["system:nodes"]
                    }
                },
                {
                    "metadata": { "name": "csr-issued" },
                    "spec": {
                        "signerName": "kubernetes.io/kubelet-serving",
                        "username": "system:node:wk-3",
                        "groups": ["system:nodes"]
                    },
                    "status": {
                        "conditions": [{ "type": "Approved", "status": "True" }],
                        "certificate": "Y2VydA=="
                    }
                }
            ]
        });
        let parsed = parse_kubelet_serving_csrs(&list);
        assert_eq!(parsed.len(), 2);
        let registered = HashSet::from(["wk-2".into(), "wk-3".into()]);
        let pending = pending_for_registered(&parsed, &registered);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].name, "csr-pending");
        assert!(node_serving_issued(&parsed, "wk-3"));
        assert!(!node_serving_issued(&parsed, "wk-2"));
    }

    #[test]
    fn rejects_missing_system_nodes_group() {
        let list = json!({
            "items": [{
                "metadata": { "name": "csr-bad" },
                "spec": {
                    "signerName": "kubernetes.io/kubelet-serving",
                    "username": "system:node:wk-1",
                    "groups": ["system:authenticated"]
                }
            }]
        });
        assert!(parse_kubelet_serving_csrs(&list).is_empty());
    }
}
