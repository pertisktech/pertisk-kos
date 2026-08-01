//! kubelet supervisor for Pertisk KOS (M3).

mod cni;
mod config;
mod paths;
mod process;

pub use cni::{ensure_cni, ensure_cni_mode, ensure_loopback_cni, DEFAULT_POD_CIDR};
pub use config::{write_kubeconfig, write_kubelet_config};
pub use paths::KubeletPaths;
pub use process::{start_kubelet, KubeletError, KubeletHandle};
