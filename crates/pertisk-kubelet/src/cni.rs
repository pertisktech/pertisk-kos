//! CNI drop-ins: loopback + optional bridge, or external cluster CNI.

use std::fs;
use std::io::Write;
use std::path::Path;

use pertisk_config::CniMode;

use crate::paths::KubeletPaths;
use crate::process::KubeletError;

/// Default node pod CIDR when `cluster.podCidr` is unset (Flannel-style /24).
pub const DEFAULT_POD_CIDR: &str = "10.244.0.0/24";

const BRIDGE_CONFLIST: &str = "10-pertisk.conflist";
const LOOPBACK_CONF: &str = "99-loopback.conf";

/// Apply CNI config for the selected mode.
pub fn ensure_cni_mode(
    paths: &KubeletPaths,
    mode: CniMode,
    pod_cidr: &str,
) -> Result<(), KubeletError> {
    paths.ensure_dirs()?;
    write_loopback_conf(&paths.cni_conf.join(LOOPBACK_CONF))?;
    enable_ip_forward();

    match mode {
        CniMode::Bridge => {
            write_bridge_conflist(&paths.cni_conf.join(BRIDGE_CONFLIST), pod_cidr)?;
            tracing::info!(
                conf = %paths.cni_conf.display(),
                pod_cidr,
                "ensured CNI configs (bridge + loopback)"
            );
        }
        CniMode::None => {
            // Remove built-in bridge so cluster CNI (Flannel/Cilium) owns net.d.
            let bridge = paths.cni_conf.join(BRIDGE_CONFLIST);
            if bridge.exists() {
                fs::remove_file(&bridge)?;
                tracing::info!(path = %bridge.display(), "removed built-in bridge CNI config");
            }
            tracing::info!(
                conf = %paths.cni_conf.display(),
                "CNI mode=none; expecting cluster CNI DaemonSet"
            );
        }
    }
    Ok(())
}

/// Write loopback + bridge CNI configs (legacy helper).
pub fn ensure_cni(paths: &KubeletPaths, pod_cidr: &str) -> Result<(), KubeletError> {
    ensure_cni_mode(paths, CniMode::Bridge, pod_cidr)
}

/// Back-compat alias.
pub fn ensure_loopback_cni(paths: &KubeletPaths) -> Result<(), KubeletError> {
    ensure_cni(paths, DEFAULT_POD_CIDR)
}

fn write_loopback_conf(path: &Path) -> Result<(), KubeletError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = r#"{
  "cniVersion": "1.0.0",
  "name": "lo",
  "type": "loopback"
}
"#;
    let mut f = fs::File::create(path)?;
    f.write_all(body.as_bytes())?;
    Ok(())
}

fn write_bridge_conflist(path: &Path, pod_cidr: &str) -> Result<(), KubeletError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = format!(
        r#"{{
  "cniVersion": "1.0.0",
  "name": "pertisk",
  "plugins": [
    {{
      "type": "bridge",
      "bridge": "cni0",
      "isGateway": true,
      "ipMasq": true,
      "hairpinMode": true,
      "ipam": {{
        "type": "host-local",
        "ranges": [
          [{{ "subnet": "{pod_cidr}" }}]
        ],
        "routes": [{{ "dst": "0.0.0.0/0" }}]
      }}
    }},
    {{
      "type": "portmap",
      "capabilities": {{ "portMappings": true }}
    }}
  ]
}}
"#
    );
    let mut f = fs::File::create(path)?;
    f.write_all(body.as_bytes())?;
    Ok(())
}

fn enable_ip_forward() {
    #[cfg(target_os = "linux")]
    {
        if let Err(err) = fs::write("/proc/sys/net/ipv4/ip_forward", b"1") {
            tracing::warn!(error = %err, "failed to enable ipv4 ip_forward");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_bridge_and_loopback() {
        let dir = std::env::temp_dir().join(format!("pertisk-cni-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let paths = KubeletPaths::with_prefix(&dir);
        ensure_cni(&paths, "10.244.1.0/24").unwrap();
        assert!(paths.cni_conf.join(LOOPBACK_CONF).exists());
        let bridge = fs::read_to_string(paths.cni_conf.join(BRIDGE_CONFLIST)).unwrap();
        assert!(bridge.contains("10.244.1.0/24"));
        assert!(bridge.contains("\"type\": \"bridge\""));
    }

    #[test]
    fn none_removes_bridge() {
        let dir = std::env::temp_dir().join(format!("pertisk-cni-none-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let paths = KubeletPaths::with_prefix(&dir);
        ensure_cni_mode(&paths, CniMode::Bridge, "10.244.0.0/24").unwrap();
        assert!(paths.cni_conf.join(BRIDGE_CONFLIST).exists());
        ensure_cni_mode(&paths, CniMode::None, "10.244.0.0/24").unwrap();
        assert!(!paths.cni_conf.join(BRIDGE_CONFLIST).exists());
        assert!(paths.cni_conf.join(LOOPBACK_CONF).exists());
    }
}
