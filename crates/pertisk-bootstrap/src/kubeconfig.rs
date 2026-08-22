//! Render kubeconfig YAML with embedded PEM credentials.

use base64::{engine::general_purpose::STANDARD as B64, Engine};

pub fn render_kubeconfig(
    server: &str,
    ca_crt: &str,
    client_crt: &str,
    client_key: &str,
    user: &str,
    cluster_name: &str,
) -> String {
    let ca = B64.encode(ca_crt.as_bytes());
    let cert = B64.encode(client_crt.as_bytes());
    let key = B64.encode(client_key.as_bytes());
    let name = sanitize_context_name(cluster_name);
    format!(
        r#"apiVersion: v1
kind: Config
clusters:
- cluster:
    certificate-authority-data: {ca}
    server: {server}
  name: {name}
users:
- name: {user}
  user:
    client-certificate-data: {cert}
    client-key-data: {key}
contexts:
- context:
    cluster: {name}
    user: {user}
  name: {name}
current-context: {name}
"#
    )
}

fn sanitize_context_name(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return "pertisk".into();
    }
    trimmed
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Fix a historical raw-string bug that appended a lone `"` after current-context.
pub fn sanitize_kubeconfig(kc: &str) -> String {
    let mut s = kc.trim_end().to_string();
    if s.ends_with('"') {
        let without = s.trim_end_matches('"').trim_end();
        if without
            .lines()
            .last()
            .is_some_and(|l| l.starts_with("current-context:"))
        {
            s = without.to_string();
        }
    }
    if s.is_empty() {
        return s;
    }
    s.push('\n');
    s
}

/// Replace the `server:` URL (indent preserved).
pub fn rewrite_kubeconfig_server(kc: &str, url: &str) -> String {
    let mut out = String::with_capacity(kc.len() + url.len());
    for line in kc.lines() {
        let t = line.trim_start();
        if t.starts_with("server:") {
            let pad = line.len() - t.len();
            out.push_str(&" ".repeat(pad));
            out.push_str("server: ");
            out.push_str(url.trim());
            out.push('\n');
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// True when kubeconfig holds a node client cert (TLS bootstrap already issued).
/// Token-only bootstrap kubeconfigs must not match.
pub fn kubeconfig_has_client_cert(kc: &str) -> bool {
    let has_cert = kc.contains("client-certificate-data:") || kc.contains("client-certificate:");
    if !has_cert {
        return false;
    }
    !(kc.contains("name: kubelet-bootstrap") && kc.contains("token:"))
}

/// Host from a kubeconfig `server:` line (`https://10.0.0.1:6443` → `10.0.0.1`).
pub fn kubeconfig_server_host(kc: &str) -> Option<String> {
    for line in kc.lines() {
        let t = line.trim_start();
        let Some(rest) = t.strip_prefix("server:") else {
            continue;
        };
        let server = rest.trim();
        let hostport = server
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        if let Some(rest) = hostport.strip_prefix('[') {
            return Some(rest.split(']').next().unwrap_or(rest).to_string());
        }
        let host = hostport.split(':').next().unwrap_or(hostport).trim();
        if !host.is_empty() {
            return Some(host.to_string());
        }
    }
    None
}

/// Rewrite kubeconfig cluster/context names to `cluster_name` (for mgmt download).
pub fn rename_kubeconfig_context(kc: &str, cluster_name: &str) -> String {
    let name = sanitize_context_name(cluster_name);
    let mut out = String::with_capacity(kc.len() + name.len());
    let mut in_clusters = false;
    let mut in_contexts = false;
    let mut saw_cluster_entry_name = false;
    let mut saw_context_entry_name = false;

    for line in kc.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("clusters:") {
            in_clusters = true;
            in_contexts = false;
            saw_cluster_entry_name = false;
        } else if trimmed.starts_with("users:") {
            in_clusters = false;
            in_contexts = false;
        } else if trimmed.starts_with("contexts:") {
            in_clusters = false;
            in_contexts = true;
            saw_context_entry_name = false;
        } else if trimmed.starts_with("current-context:") {
            out.push_str("current-context: ");
            out.push_str(&name);
            out.push('\n');
            continue;
        } else if in_clusters && !saw_cluster_entry_name && trimmed.starts_with("name:") {
            // top-level cluster entry name (not nested under cluster:)
            let indent = line.len() - trimmed.len();
            if indent == 2 {
                out.push_str("  name: ");
                out.push_str(&name);
                out.push('\n');
                saw_cluster_entry_name = true;
                continue;
            }
        } else if in_contexts {
            if trimmed.starts_with("cluster:") {
                let indent = line.len() - trimmed.len();
                out.push_str(&" ".repeat(indent));
                out.push_str("cluster: ");
                out.push_str(&name);
                out.push('\n');
                continue;
            }
            if !saw_context_entry_name && trimmed.starts_with("name:") {
                let indent = line.len() - trimmed.len();
                if indent == 2 {
                    out.push_str("  name: ");
                    out.push_str(&name);
                    out.push('\n');
                    saw_context_entry_name = true;
                    continue;
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    if !kc.ends_with('\n') && out.ends_with('\n') {
        // keep trailing newline convention from sanitize
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_uses_cluster_name() {
        let kc = render_kubeconfig(
            "https://10.0.0.1:6443",
            "-----BEGIN CERTIFICATE-----\nCA\n-----END CERTIFICATE-----\n",
            "-----BEGIN CERTIFICATE-----\nCRT\n-----END CERTIFICATE-----\n",
            "-----BEGIN PRIVATE KEY-----\nKEY\n-----END PRIVATE KEY-----\n",
            "kubernetes-admin",
            "lab-ha-dual-stack",
        );
        assert!(kc.contains("  name: lab-ha-dual-stack\n"));
        assert!(kc.contains("    cluster: lab-ha-dual-stack\n"));
        assert!(kc.contains("current-context: lab-ha-dual-stack\n"));
        assert!(!kc.contains("name: pertisk"));
        assert!(!kc.trim_end().ends_with('"'));
        assert!(kc.ends_with('\n'));
    }

    #[test]
    fn issued_kubeconfig_detected() {
        assert!(kubeconfig_has_client_cert(
            "users:\n- name: default-auth\n  user:\n    client-certificate: /var/lib/kubelet/pki/kubelet-client-current.pem\n"
        ));
        assert!(kubeconfig_has_client_cert(
            "user:\n    client-certificate-data: QQ==\n    client-key-data: QQ==\n"
        ));
        assert!(!kubeconfig_has_client_cert(
            "users:\n- name: kubelet-bootstrap\n  user:\n    token: \"abc.def\"\n"
        ));
    }

    #[test]
    fn sanitize_strips_legacy_quote() {
        let bad = "current-context: pertisk\n\"\n";
        assert_eq!(sanitize_kubeconfig(bad), "current-context: pertisk\n");
        let bad2 = "current-context: my-cluster\n\"\n";
        assert_eq!(sanitize_kubeconfig(bad2), "current-context: my-cluster\n");
    }

    #[test]
    fn rename_rewrites_pertisk() {
        let src = r#"apiVersion: v1
kind: Config
clusters:
- cluster:
    server: https://10.0.0.1:6443
  name: pertisk
users:
- name: kubernetes-admin
  user: {}
contexts:
- context:
    cluster: pertisk
    user: kubernetes-admin
  name: pertisk
current-context: pertisk
"#;
        let out = rename_kubeconfig_context(src, "ui-cluster");
        assert!(out.contains("  name: ui-cluster\n"));
        assert!(out.contains("    cluster: ui-cluster\n"));
        assert!(out.contains("current-context: ui-cluster\n"));
        assert!(!out.contains("pertisk"));
    }

    #[test]
    fn rewrite_server_and_host() {
        let src = "clusters:\n- cluster:\n    server: https://10.0.0.1:6443\n  name: lab\n";
        let out = rewrite_kubeconfig_server(src, "https://10.0.0.9:6443");
        assert!(out.contains("server: https://10.0.0.9:6443\n"));
        assert_eq!(kubeconfig_server_host(&out).as_deref(), Some("10.0.0.9"));
    }
}
