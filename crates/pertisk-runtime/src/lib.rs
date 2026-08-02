//! containerd supervisor for Pertisk KOS (M3).

mod config;
mod log_tee;
mod paths;
mod process;

pub use config::write_containerd_config;
pub use log_tee::LineSink;
pub use paths::RuntimePaths;
pub use process::{start_containerd, start_containerd_with_sink, ContainerdHandle, RuntimeError};
