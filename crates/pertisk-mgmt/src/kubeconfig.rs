//! Kubeconfig helpers for download / storage.

/// Rewrite kubeconfig cluster/context names to match the UI cluster name.
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
    out
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
