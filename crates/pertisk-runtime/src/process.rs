//! Spawn and babysit the containerd process.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use thiserror::Error;
use tracing::{info, warn};

use crate::config::write_containerd_config;
use crate::paths::RuntimePaths;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("containerd binary not found at {0}")]
    MissingBinary(String),
    #[error("{0}")]
    Msg(String),
}

/// Running containerd child process.
pub struct ContainerdHandle {
    pub paths: RuntimePaths,
    child: Child,
}

impl ContainerdHandle {
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn socket(&self) -> &Path {
        &self.paths.socket
    }

    /// True when the process is still running and the CRI socket exists.
    pub fn is_healthy(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(None) => self.paths.socket.exists(),
            Ok(Some(status)) => {
                warn!(status = ?status, "containerd exited");
                false
            }
            Err(err) => {
                warn!(error = %err, "containerd wait failed");
                false
            }
        }
    }

    /// Restart containerd if it has exited.
    pub fn ensure_alive(&mut self) -> Result<(), RuntimeError> {
        if self.is_healthy() {
            return Ok(());
        }
        warn!("restarting containerd");
        let restarted = start_containerd(&self.paths)?;
        self.child = restarted.child;
        Ok(())
    }

    pub fn stop(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Write config and spawn containerd. Returns error if binary is missing.
pub fn start_containerd(paths: &RuntimePaths) -> Result<ContainerdHandle, RuntimeError> {
    if !paths.binary.exists() {
        return Err(RuntimeError::MissingBinary(paths.binary.display().to_string()));
    }
    write_containerd_config(paths)?;

    info!(bin = %paths.binary.display(), "starting containerd");
    let child = Command::new(&paths.binary)
        .arg("--config")
        .arg(&paths.config)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()?;

    let mut handle = ContainerdHandle {
        paths: paths.clone(),
        child,
    };

    // Brief wait for the socket (non-fatal if slow).
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if handle.paths.socket.exists() {
            info!(pid = handle.pid(), socket = %handle.paths.socket.display(), "containerd ready");
            return Ok(handle);
        }
        if matches!(handle.child.try_wait(), Ok(Some(_))) {
            return Err(RuntimeError::Msg("containerd exited during startup".into()));
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    warn!("containerd socket not ready yet; continuing");
    Ok(handle)
}
