//! containerd supervisor for Pertisk KOS (M3).

mod config;
mod process;
mod paths;

pub use config::write_containerd_config;
pub use paths::RuntimePaths;
pub use process::{ContainerdHandle, RuntimeError, start_containerd};
