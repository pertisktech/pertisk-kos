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
    /// Loopback admin kubeconfig for on-node consumers (kube-vip, static pods).
    /// Distinct from [`Self::admin_kubeconfig`], which may point at the VIP.
    pub fn admin_local_kubeconfig(&self) -> PathBuf {
        self.kubeconfig_dir().join("admin.local.conf")
    }
    pub fn marker(&self) -> PathBuf {
        self.root.join("BOOTSTRAPPED")
    }

    /// Advertise address used when this node was bootstrapped/joined (IPv4).
    pub fn advertise_path(&self) -> PathBuf {
        self.root.join("advertise-address")
    }

    pub fn write_advertise(&self, ip: &str) -> Result<()> {
        fs::create_dir_all(&self.root)?;
        fs::write(self.advertise_path(), format!("{}\n", ip.trim()))
            .with_context(|| format!("write {}", self.advertise_path().display()))?;
        Ok(())
    }

    pub fn read_advertise(&self) -> Option<String> {
        fs::read_to_string(self.advertise_path())
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
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
        // Kubeconfigs live on ephemeral /etc — always re-copy from STATE.
        // Live admin.conf must be loopback so kube-vip can elect before the VIP exists.
        let admin_src = if self.admin_local_kubeconfig().is_file() {
            self.admin_local_kubeconfig()
        } else {
            self.admin_kubeconfig()
        };
        for (src, name) in [
            (admin_src, "admin.conf"),
            (
                self.kubeconfig_dir().join("controller-manager.conf"),
                "controller-manager.conf",
            ),
            (
                self.kubeconfig_dir().join("scheduler.conf"),
                "scheduler.conf",
            ),
            (self.kubeconfig_dir().join("kubelet.conf"), "kubelet.conf"),
        ] {
            if !src.is_file() {
                continue;
            }
            let dst = live.join(name);
            fs::copy(&src, &dst)
                .with_context(|| format!("copy {} → {}", src.display(), dst.display()))?;
        }
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
