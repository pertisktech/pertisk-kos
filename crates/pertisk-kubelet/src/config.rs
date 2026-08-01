//! Kubelet config + bootstrap kubeconfig generation.

use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use pertisk_config::Cluster;

use crate::paths::KubeletPaths;
use crate::process::KubeletError;

/// Write KubeletConfiguration (v1beta1) with CIS-aligned defaults.
pub fn write_kubelet_config(
    paths: &KubeletPaths,
    hostname: Option<&str>,
) -> Result<(), KubeletError> {
    paths.ensure_dirs()?;
    let hostname_line = hostname
        .map(|h| format!("hostnameOverride: \"{h}\"\n"))
        .unwrap_or_default();

    let body = format!(
        r#"apiVersion: kubelet.config.k8s.io/v1beta1
kind: KubeletConfiguration
{hostname_line}authentication:
  anonymous:
    enabled: false
  webhook:
    enabled: true
authorization:
  mode: Webhook
# CIS 4.2.4 — disable legacy read-only port
readOnlyPort: 0
# CIS 4.2.5
streamingConnectionIdleTimeout: 5m
# CIS 4.2.6 — requires matching sysctls applied by pertiskd
protectKernelDefaults: true
# CIS 4.2.7
makeIPTablesUtilChains: true
# CIS 4.2.10
rotateCertificates: true
serverTLSBootstrap: true
eventRecordQPS: 5
# CIS 4.2.12 — strong TLS cipher suites
tlsCipherSuites:
  - TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256
  - TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256
  - TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384
  - TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384
  - TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256
  - TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256
cgroupDriver: cgroupfs
failSwapOn: false
clusterDomain: cluster.local
clusterDNS:
  - 10.96.0.10
serializeImagePulls: false
staticPodPath: "/etc/kubernetes/manifests"
"#,
        hostname_line = hostname_line,
    );

    let mut f = fs::File::create(&paths.config)?;
    f.write_all(body.as_bytes())?;
    restrict_file_mode(&paths.config)?;
    tracing::info!(path = %paths.config.display(), "wrote kubelet config");
    Ok(())
}

/// Write bootstrap kubeconfig from cluster join settings.
pub fn write_kubeconfig(paths: &KubeletPaths, cluster: &Cluster) -> Result<(), KubeletError> {
    paths.ensure_dirs()?;

    if let Some(ca) = &cluster.ca {
        let mut f = fs::File::create(&paths.ca_file)?;
        f.write_all(ca.as_bytes())?;
        if !ca.ends_with('\n') {
            f.write_all(b"\n")?;
        }
    }

    let token = cluster
        .token
        .as_deref()
        .ok_or_else(|| KubeletError::Msg("cluster.token is required to start kubelet".into()))?;

    // Embed CA via path reference when present; otherwise insecure-skip (dev only).
    let (ca_block, insecure) = if paths.ca_file.exists() {
        (
            format!("    certificate-authority: {}\n", paths.ca_file.display()),
            "    insecure-skip-tls-verify: false\n",
        )
    } else {
        (String::new(), "    insecure-skip-tls-verify: true\n")
    };

    let body = format!(
        r#"apiVersion: v1
kind: Config
clusters:
- cluster:
{ca_block}{insecure}    server: {endpoint}
  name: pertisk
users:
- name: kubelet-bootstrap
  user:
    token: "{token}"
contexts:
- context:
    cluster: pertisk
    user: kubelet-bootstrap
  name: pertisk
current-context: pertisk
"#,
        ca_block = ca_block,
        insecure = insecure,
        endpoint = cluster.endpoint,
        token = token,
    );

    let mut f = fs::File::create(&paths.kubeconfig)?;
    f.write_all(body.as_bytes())?;
    restrict_file_mode(&paths.kubeconfig)?;
    if paths.ca_file.exists() {
        restrict_file_mode(&paths.ca_file)?;
    }
    tracing::info!(path = %paths.kubeconfig.display(), "wrote kubelet kubeconfig");
    Ok(())
}

fn restrict_file_mode(path: &std::path::Path) -> Result<(), KubeletError> {
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(path, perms)?;
    }
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pertisk_config::Cluster;
    use std::path::PathBuf;

    #[test]
    fn writes_kubeconfig_with_ca() {
        let dir = temp_dir();
        let paths = KubeletPaths::with_prefix(&dir);
        let cluster = Cluster {
            endpoint: "https://10.0.0.1:6443".into(),
            token: Some("abc.def".into()),
            ca: Some("-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n".into()),
            pod_cidr: Some("10.244.0.0/24".into()),
            cni: Default::default(),
        };
        write_kubeconfig(&paths, &cluster).unwrap();
        write_kubelet_config(&paths, Some("node-1")).unwrap();
        assert!(paths.kubeconfig.exists());
        assert!(paths.ca_file.exists());
        let kc = fs::read_to_string(&paths.kubeconfig).unwrap();
        assert!(kc.contains("https://10.0.0.1:6443"));
        assert!(kc.contains("abc.def"));
        let cfg = fs::read_to_string(&paths.config).unwrap();
        assert!(cfg.contains("readOnlyPort: 0"));
        assert!(cfg.contains("protectKernelDefaults: true"));
        assert!(cfg.contains("rotateCertificates: true"));
        assert!(cfg.contains("tlsCipherSuites:"));
    }

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pertisk-kubelet-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
