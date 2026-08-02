//! Default paths for kubelet on Pertisk.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct KubeletPaths {
    pub binary: PathBuf,
    pub config: PathBuf,
    pub kubeconfig: PathBuf,
    /// Bootstrap-token kubeconfig used until TLS bootstrap writes `kubeconfig`.
    pub bootstrap_kubeconfig: PathBuf,
    pub root_dir: PathBuf,
    pub cni_bin: PathBuf,
    pub cni_conf: PathBuf,
    pub ca_file: PathBuf,
}

impl Default for KubeletPaths {
    fn default() -> Self {
        Self {
            binary: PathBuf::from("/usr/local/bin/kubelet"),
            config: PathBuf::from("/var/lib/kubelet/config.yaml"),
            kubeconfig: PathBuf::from("/var/lib/kubelet/kubeconfig"),
            bootstrap_kubeconfig: PathBuf::from("/var/lib/kubelet/bootstrap-kubeconfig"),
            root_dir: PathBuf::from("/var/lib/kubelet"),
            cni_bin: PathBuf::from("/opt/cni/bin"),
            cni_conf: PathBuf::from("/etc/cni/net.d"),
            ca_file: PathBuf::from("/var/lib/kubelet/ca.crt"),
        }
    }
}

impl KubeletPaths {
    pub fn with_prefix(prefix: impl AsRef<Path>) -> Self {
        let p = prefix.as_ref();
        Self {
            binary: p.join("usr/local/bin/kubelet"),
            config: p.join("var/lib/kubelet/config.yaml"),
            kubeconfig: p.join("var/lib/kubelet/kubeconfig"),
            bootstrap_kubeconfig: p.join("var/lib/kubelet/bootstrap-kubeconfig"),
            root_dir: p.join("var/lib/kubelet"),
            cni_bin: p.join("opt/cni/bin"),
            cni_conf: p.join("etc/cni/net.d"),
            ca_file: p.join("var/lib/kubelet/ca.crt"),
        }
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root_dir)?;
        std::fs::create_dir_all(&self.cni_bin)?;
        std::fs::create_dir_all(&self.cni_conf)?;
        if let Some(parent) = self.config.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(())
    }
}
