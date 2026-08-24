//! Spawn and babysit the containerd process.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use thiserror::Error;
use tracing::{info, warn};

use crate::config::write_containerd_config;
use crate::log_tee::{containerd_log_path, ensure_var_log, spawn_stderr_tee, LineSink};
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
    log_sink: Option<LineSink>,
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
        let restarted = start_containerd_with_sink(&self.paths, self.log_sink.clone())?;
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
    start_containerd_with_sink(paths, None)
}

/// Like [`start_containerd`], but tees stderr lines to `log_sink` (prefixed by caller).
pub fn start_containerd_with_sink(
    paths: &RuntimePaths,
    log_sink: Option<LineSink>,
) -> Result<ContainerdHandle, RuntimeError> {
    if !paths.binary.exists() {
        return Err(RuntimeError::MissingBinary(
            paths.binary.display().to_string(),
        ));
    }
    write_containerd_config(paths)?;
    ensure_var_log();

    info!(bin = %paths.binary.display(), "starting containerd");
    let mut child = match Command::new(&paths.binary)
        .arg("--config")
        .arg(&paths.config)
        .env("SSL_CERT_FILE", "/etc/ssl/certs/ca-certificates.crt")
        .env("PATH", "/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            // Linux returns ENOENT when the ELF interpreter (e.g. glibc ld-linux)
            // is missing, not only when the binary path is absent.
            return Err(RuntimeError::Msg(format!(
                "failed to exec {}: {err} (missing binary or dynamic linker/libs — rebuild with fetch-runtime glibc libs)",
                paths.binary.display()
            )));
        }
        Err(err) => return Err(RuntimeError::Io(err)),
    };

    if let Some(stderr) = child.stderr.take() {
        // Caller sink (e.g. LogRing) should apply its own prefix.
        spawn_stderr_tee(
            stderr,
            containerd_log_path(),
            "containerd",
            log_sink.clone(),
        );
    }

    let mut handle = ContainerdHandle {
        paths: paths.clone(),
        child,
        log_sink,
    };

    // Brief wait for the socket (non-fatal if slow). The socket file can exist
    // before the CRI plugin answers Version(); kubelet then exits immediately.
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut socket_seen = None;
    while Instant::now() < deadline {
        if matches!(handle.child.try_wait(), Ok(Some(_))) {
            return Err(RuntimeError::Msg("containerd exited during startup".into()));
        }
        if handle.paths.socket.exists() {
            if socket_seen.is_none() {
                socket_seen = Some(Instant::now());
            }
            // Hold ~2s after the socket appears so CRI can finish init.
            if socket_seen.is_some_and(|t| t.elapsed() >= Duration::from_secs(5)) {
                info!(pid = handle.pid(), socket = %handle.paths.socket.display(), "containerd ready");
                return Ok(handle);
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    if handle.paths.socket.exists() {
        info!(pid = handle.pid(), socket = %handle.paths.socket.display(), "containerd socket up (CRI may still be settling)");
    } else {
        warn!("containerd socket not ready yet; continuing");
    }
    Ok(handle)
}
