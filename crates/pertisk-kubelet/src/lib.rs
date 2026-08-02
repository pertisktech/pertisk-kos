//! kubelet supervisor for Pertisk KOS (M3).

mod cni;
mod config;
mod log_tee;
mod paths;
mod process;

pub use cni::{ensure_cni, ensure_cni_mode, ensure_loopback_cni, DEFAULT_POD_CIDR};
pub use config::{write_bootstrap_kubeconfig, write_kubeconfig, write_kubelet_config};
pub use log_tee::LineSink;
pub use paths::KubeletPaths;
pub use process::{start_kubelet, start_kubelet_with_sink, KubeletError, KubeletHandle};
