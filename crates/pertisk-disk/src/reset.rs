//! Soft node reset: clear STATE identity + EPHEMERAL runtime data (keep GPT).

use std::fs;
use std::path::Path;

use thiserror::Error;
use tracing::{info, warn};

use crate::state::{StateVolume, DEFAULT_CONFIG_NAME};

#[derive(Debug, Error)]
pub enum ResetError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Msg(String),
}

#[derive(Debug, Clone)]
pub struct SoftResetResult {
    pub cleared: Vec<String>,
    pub warnings: Vec<String>,
}

/// Clear node identity and runtime data without GPT wipe.
///
/// Removes:
/// - STATE config / machine / secrets / kubernetes bootstrap marker
/// - live `/etc/kubernetes` (Linux only)
/// - EPHEMERAL runtime trees under `/var` (Linux only)
///
/// Does **not** touch EFI / BOOT_A / BOOT_B / META or run `sgdisk --zap-all`.
pub fn soft_reset(state_root: &Path) -> Result<SoftResetResult, ResetError> {
    if !state_root.is_dir() {
        return Err(ResetError::Msg(format!(
            "STATE root missing: {}",
            state_root.display()
        )));
    }

    let mut cleared = Vec::new();
    let mut warnings = Vec::new();

    // --- STATE ---
    for name in [
        DEFAULT_CONFIG_NAME,
        "machine",
        "secrets",
        "kubernetes",
        "log",
    ] {
        let p = state_root.join(name);
        match remove_path(&p) {
            Ok(true) => cleared.push(format!("state:{name}")),
            Ok(false) => {}
            Err(err) => warnings.push(format!("state:{name}: {err}")),
        }
    }
    // Recreate empty STATE layout (config must be re-applied after reboot).
    let vol = StateVolume {
        root: state_root.to_path_buf(),
        source: crate::state::StateSource::Directory,
    };
    if let Err(err) = vol.ensure_layout() {
        warnings.push(format!("recreate STATE layout: {err}"));
    } else {
        cleared.push("state:layout".into());
    }

    // Runtime clears are Linux-node only (avoid host damage in macOS/dev unit tests).
    #[cfg(target_os = "linux")]
    {
        use std::path::PathBuf;

        match remove_path(Path::new("/etc/kubernetes")) {
            Ok(true) => cleared.push("live:/etc/kubernetes".into()),
            Ok(false) => {}
            Err(err) => warnings.push(format!("live:/etc/kubernetes: {err}")),
        }

        for rel in [
            "lib/kubelet",
            "lib/containerd",
            "lib/etcd",
            "lib/cni",
            "lib/pertisk",
            "log/pods",
            "log/containers",
            "log/kubelet.log",
            "log/containerd.log",
        ] {
            let p = PathBuf::from("/var").join(rel);
            match remove_path(&p) {
                Ok(true) => cleared.push(format!("var:{rel}")),
                Ok(false) => {}
                Err(err) => warnings.push(format!("var:{rel}: {err}")),
            }
        }

        // Drop CNI net.d configs only — never delete image-baked CNI binaries.
        match clear_dir_contents(Path::new("/etc/cni/net.d")) {
            Ok(n) if n > 0 => cleared.push(format!("cni:/etc/cni/net.d ({n} entries)")),
            Ok(_) => {}
            Err(err) => warnings.push(format!("cni:/etc/cni/net.d: {err}")),
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        warnings.push("runtime clears skipped (non-Linux build)".into());
    }

    info!(
        cleared = cleared.len(),
        warnings = warnings.len(),
        "soft reset completed"
    );
    for w in &warnings {
        warn!(%w, "soft reset warning");
    }

    Ok(SoftResetResult { cleared, warnings })
}

fn remove_path(path: &Path) -> Result<bool, std::io::Error> {
    if !path.exists() {
        return Ok(false);
    }
    if path.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(true)
}

#[cfg(target_os = "linux")]
fn clear_dir_contents(dir: &Path) -> Result<usize, std::io::Error> {
    if !dir.is_dir() {
        return Ok(0);
    }
    let mut n = 0;
    for ent in fs::read_dir(dir)? {
        let ent = ent?;
        let p = ent.path();
        if p.is_dir() {
            fs::remove_dir_all(&p)?;
        } else {
            fs::remove_file(&p)?;
        }
        n += 1;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn clears_state_tree() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("machine")).unwrap();
        fs::create_dir_all(root.join("secrets")).unwrap();
        fs::write(root.join("config.yaml"), "version: v1alpha1\n").unwrap();
        fs::create_dir_all(root.join("kubernetes")).unwrap();
        fs::write(root.join("kubernetes/BOOTSTRAPPED"), "1").unwrap();

        let out = soft_reset(root).unwrap();
        assert!(!root.join("config.yaml").exists());
        assert!(!root.join("kubernetes").exists());
        assert!(root.join("machine").is_dir());
        assert!(root.join("secrets").is_dir());
        assert!(out.cleared.iter().any(|c| c.starts_with("state:")));
    }
}
