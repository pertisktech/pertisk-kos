//! Minimal CNI loopback drop-in so kubelet can start without a full CNI plugin set.

use std::fs;
use std::io::Write;
use std::path::Path;

use crate::paths::KubeletPaths;
use crate::process::KubeletError;

/// Write `/etc/cni/net.d/99-loopback.conf` (requires `loopback` binary in cni bin later).
pub fn ensure_loopback_cni(paths: &KubeletPaths) -> Result<(), KubeletError> {
    paths.ensure_dirs()?;
    let conf = paths.cni_conf.join("99-loopback.conf");
    write_loopback_conf(&conf)?;
    tracing::info!(path = %conf.display(), "ensured loopback CNI config");
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_loopback() {
        let dir = std::env::temp_dir().join(format!("pertisk-cni-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let paths = KubeletPaths::with_prefix(&dir);
        ensure_loopback_cni(&paths).unwrap();
        assert!(paths.cni_conf.join("99-loopback.conf").exists());
    }
}
