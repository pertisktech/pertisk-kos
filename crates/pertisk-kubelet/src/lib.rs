//! kubelet supervisor for Pertisk KOS (M3).

mod cni;
mod config;
mod paths;
mod process;

pub use cni::ensure_loopback_cni;
pub use config::{write_kubeconfig, write_kubelet_config};
pub use paths::KubeletPaths;
pub use process::{KubeletError, KubeletHandle, start_kubelet};
