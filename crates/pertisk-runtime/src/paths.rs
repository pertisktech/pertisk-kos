//! Default filesystem layout for containerd on Pertisk.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct RuntimePaths {
    pub binary: PathBuf,
    pub config: PathBuf,
    pub root: PathBuf,
    pub state: PathBuf,
    pub socket: PathBuf,
}

impl Default for RuntimePaths {
    fn default() -> Self {
        Self {
            binary: PathBuf::from("/usr/local/bin/containerd"),
            config: PathBuf::from("/etc/containerd/config.toml"),
            root: PathBuf::from("/var/lib/containerd"),
            state: PathBuf::from("/run/containerd"),
            socket: PathBuf::from("/run/containerd/containerd.sock"),
        }
    }
}

impl RuntimePaths {
    pub fn with_prefix(prefix: impl AsRef<Path>) -> Self {
        let p = prefix.as_ref();
        Self {
            binary: p.join("usr/local/bin/containerd"),
            config: p.join("etc/containerd/config.toml"),
            root: p.join("var/lib/containerd"),
            state: p.join("run/containerd"),
            socket: p.join("run/containerd/containerd.sock"),
        }
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        if let Some(parent) = self.config.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::create_dir_all(&self.root)?;
        std::fs::create_dir_all(&self.state)?;
        Ok(())
    }
}
