//! containerd supervisor for Pertisk KOS (M3).

mod config;
mod paths;
mod process;

pub use config::write_containerd_config;
pub use paths::RuntimePaths;
pub use process::{start_containerd, ContainerdHandle, RuntimeError};
