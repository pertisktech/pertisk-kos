//! Kubelet config + bootstrap kubeconfig generation.

use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use pertisk_config::Cluster;

use crate::paths::KubeletPaths;
use crate::process::KubeletError;

/// Write KubeletConfiguration (v1beta1) with CIS-aligned defaults.
///
/// `tls_bootstrap`: workers need certificate rotation so a bootstrap token can
/// be exchanged for a `system:node:` client cert.
pub fn write_kubelet_config(
    paths: &KubeletPaths,
    _hostname: Option<&str>,
    tls_bootstrap: bool,
) -> Result<(), KubeletError> {
    paths.ensure_dirs()?;
    let rotate = if tls_bootstrap { "true" } else { "false" };

    let body = format!(
        r#"apiVersion: kubelet.config.k8s.io/v1beta1
kind: KubeletConfiguration
authentication:
  anonymous:
    enabled: false
  webhook:
    enabled: true
  x509:
    clientCAFile: {ca_file}
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
# Unified cgroup v2 (mounted by pertiskd at /sys/fs/cgroup).
cgroupRoot: /
failSwapOn: false
clusterDomain: cluster.local
clusterDNS:
  - 10.96.0.10
serializeImagePulls: false
staticPodPath: "/etc/kubernetes/manifests"
# Workers: rotateCertificates + bootstrap-kubeconfig → node client cert.
# Control-plane: cert kubeconfig from pertiskctl bootstrap (no rotation needed).
rotateCertificates: {rotate}
serverTLSBootstrap: false
# Pods that set spec.hostUsers (Flannel/charts) need this; GA in 1.36 but
# explicit so a mismatched/older kubelet binary does not reject the field.
featureGates:
  UserNamespacesSupport: true
"#,
        ca_file = paths.ca_file.display(),
        rotate = rotate,
    );

    let mut f = fs::File::create(&paths.config)?;
    f.write_all(body.as_bytes())?;
    restrict_file_mode(&paths.config)?;
    tracing::info!(path = %paths.config.display(), tls_bootstrap, "wrote kubelet config");
    Ok(())
}

/// Write bootstrap-token kubeconfig (TLS bootstrap input).
pub fn write_bootstrap_kubeconfig(
    paths: &KubeletPaths,
    cluster: &Cluster,
) -> Result<(), KubeletError> {
    write_token_kubeconfig(paths, &paths.bootstrap_kubeconfig, cluster)
}

/// Write token kubeconfig to the main kubeconfig path (legacy / tests).
pub fn write_kubeconfig(paths: &KubeletPaths, cluster: &Cluster) -> Result<(), KubeletError> {
    write_token_kubeconfig(paths, &paths.kubeconfig, cluster)
}

fn write_token_kubeconfig(
    paths: &KubeletPaths,
    dest: &Path,
    cluster: &Cluster,
) -> Result<(), KubeletError> {
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

    let mut f = fs::File::create(dest)?;
    f.write_all(body.as_bytes())?;
    restrict_file_mode(dest)?;
    if paths.ca_file.exists() {
        restrict_file_mode(&paths.ca_file)?;
    }
    tracing::info!(path = %dest.display(), "wrote kubelet token kubeconfig");
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
            name: None,
            endpoint: "https://10.0.0.1:6443".into(),
            token: Some("abc.def".into()),
            ca: Some("-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n".into()),
            ca_key: None,
            sa_key: None,
            pod_subnet: None,
            service_subnet: None,
            pod_cidr_ipv6: None,
            service_cidr_ipv6: None,
            network_mode: Default::default(),
            vip6: None,
            kubernetes_version: None,
            pod_cidr: Some("10.244.0.0/24".into()),
            cni: Default::default(),
            cert_sans: vec![],
        };
        write_kubeconfig(&paths, &cluster).unwrap();
        write_kubelet_config(&paths, Some("node-1"), true).unwrap();
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
        assert!(cfg.contains("UserNamespacesSupport: true"));
        assert!(cfg.contains("clientCAFile:"));
        assert!(cfg.contains(&paths.ca_file.display().to_string()));
    }

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pertisk-kubelet-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
