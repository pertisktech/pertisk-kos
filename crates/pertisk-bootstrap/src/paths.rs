//! On-disk layout for Kubernetes control-plane material.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct BootstrapPaths {
    pub root: PathBuf,
}

impl BootstrapPaths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Default: `/var/lib/pertisk/kubernetes` (STATE-backed) + live `/etc/kubernetes`.
    pub fn default_state(state_root: &Path) -> Self {
        Self::new(state_root.join("kubernetes"))
    }

    pub fn pki(&self) -> PathBuf {
        self.root.join("pki")
    }
    pub fn etcd_pki(&self) -> PathBuf {
        self.pki().join("etcd")
    }
    pub fn manifests(&self) -> PathBuf {
        self.root.join("manifests")
    }
    pub fn kubeconfig_dir(&self) -> PathBuf {
        self.root.join("kubeconfig")
    }
    pub fn admin_kubeconfig(&self) -> PathBuf {
        self.kubeconfig_dir().join("admin.conf")
    }
    pub fn marker(&self) -> PathBuf {
        self.root.join("BOOTSTRAPPED")
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        for d in [
            self.pki(),
            self.etcd_pki(),
            self.manifests(),
            self.kubeconfig_dir(),
        ] {
            fs::create_dir_all(&d).with_context(|| format!("mkdir {}", d.display()))?;
        }
        Ok(())
    }

    pub fn is_bootstrapped(&self) -> bool {
        self.marker().is_file() && self.admin_kubeconfig().is_file()
    }

    /// Publish into `/etc/kubernetes` for kubelet staticPodPath.
    pub fn link_live(&self) -> Result<()> {
        let live = Path::new("/etc/kubernetes");
        fs::create_dir_all(live).ok();
        for name in ["pki", "manifests"] {
            let src = self.root.join(name);
            let dst = live.join(name);
            if dst.exists() {
                let _ = fs::remove_dir_all(&dst);
                let _ = fs::remove_file(&dst);
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::symlink;
                if let Err(err) = symlink(&src, &dst) {
                    tracing::warn!(
                        error = %err,
                        from = %src.display(),
                        to = %dst.display(),
                        "symlink failed; copying tree"
                    );
                    copy_dir(&src, &dst)?;
                }
            }
            #[cfg(not(unix))]
            {
                copy_dir(&src, &dst)?;
            }
        }
        let admin_live = live.join("admin.conf");
        fs::copy(self.admin_kubeconfig(), &admin_live)
            .with_context(|| format!("copy admin.conf → {}", admin_live.display()))?;
        Ok(())
    }
}

fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            fs::copy(entry.path(), to)?;
        }
    }
    Ok(())
}
