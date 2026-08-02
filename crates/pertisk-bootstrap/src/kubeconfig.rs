//! Render kubeconfig YAML with embedded PEM credentials.

use base64::{engine::general_purpose::STANDARD as B64, Engine};

pub fn render_kubeconfig(
    server: &str,
    ca_crt: &str,
    client_crt: &str,
    client_key: &str,
    user: &str,
) -> String {
    let ca = B64.encode(ca_crt.as_bytes());
    let cert = B64.encode(client_crt.as_bytes());
    let key = B64.encode(client_key.as_bytes());
    format!(
        r#"apiVersion: v1
kind: Config
clusters:
- cluster:
    certificate-authority-data: {ca}
    server: {server}
  name: pertisk
users:
- name: {user}
  user:
    client-certificate-data: {cert}
    client-key-data: {key}
contexts:
- context:
    cluster: pertisk
    user: {user}
  name: pertisk
current-context: pertisk
"#
    )
}

/// Fix a historical raw-string bug that appended a lone `"` after current-context.
pub fn sanitize_kubeconfig(kc: &str) -> String {
    let mut s = kc.trim_end().to_string();
    if s.ends_with('"') {
        let without = s.trim_end_matches('"').trim_end();
        if without.ends_with("current-context: pertisk") {
            s = without.to_string();
        }
    }
    if s.is_empty() {
        return s;
    }
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_has_no_trailing_quote() {
        let kc = render_kubeconfig(
            "https://10.0.0.1:6443",
            "-----BEGIN CERTIFICATE-----\nCA\n-----END CERTIFICATE-----\n",
            "-----BEGIN CERTIFICATE-----\nCRT\n-----END CERTIFICATE-----\n",
            "-----BEGIN PRIVATE KEY-----\nKEY\n-----END PRIVATE KEY-----\n",
            "kubernetes-admin",
        );
        assert!(kc.contains("current-context: pertisk\n"));
        assert!(!kc.trim_end().ends_with('"'));
        assert!(kc.ends_with('\n'));
    }

    #[test]
    fn sanitize_strips_legacy_quote() {
        let bad = "current-context: pertisk\n\"\n";
        assert_eq!(sanitize_kubeconfig(bad), "current-context: pertisk\n");
    }
}
