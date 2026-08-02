//! Tee child stderr to a log file and optional line sink.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::ChildStderr;
use std::sync::Arc;
use std::thread;

/// Optional callback for each stderr line (e.g. console log ring).
pub type LineSink = Arc<dyn Fn(&str) + Send + Sync>;

pub fn spawn_stderr_tee(
    stderr: ChildStderr,
    log_path: PathBuf,
    prefix: &'static str,
    sink: Option<LineSink>,
) {
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = thread::Builder::new()
        .name(format!("{prefix}-log"))
        .spawn(move || {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .ok();
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                if let Some(ref mut f) = file {
                    let _ = writeln!(f, "{line}");
                }
                if let Some(ref sink) = sink {
                    sink(&line);
                }
            }
        });
}

pub fn containerd_log_path() -> PathBuf {
    PathBuf::from("/var/log/containerd.log")
}

pub fn ensure_var_log() {
    let _ = std::fs::create_dir_all("/var/log");
}

#[allow(dead_code)]
pub fn path_str(path: &Path) -> &str {
    path.to_str().unwrap_or("")
}
