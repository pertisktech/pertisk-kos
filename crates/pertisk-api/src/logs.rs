//! Collect recent log lines for management API `Logs`.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum LogsError {
    #[error("unknown service '{0}' (want pertiskd|containerd|kubelet|dmesg)")]
    UnknownService(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Msg(String),
}

#[derive(Debug, Clone)]
pub struct LogTail {
    pub service: String,
    pub source: String,
    pub lines: Vec<String>,
}

/// Tail logs for a named service.
pub fn tail_logs(state_root: &Path, service: &str, tail_lines: u32) -> Result<LogTail, LogsError> {
    let n = normalize_tail(tail_lines);
    match service {
        "dmesg" => tail_dmesg(n),
        "pertiskd" => {
            let path = state_root.join("log/pertiskd.log");
            tail_file_or_empty("pertiskd", &path, n)
        }
        "containerd" => {
            let path = PathBuf::from("/var/log/containerd.log");
            tail_file_or_empty("containerd", &path, n)
        }
        "kubelet" => {
            let path = PathBuf::from("/var/log/kubelet.log");
            tail_file_or_empty("kubelet", &path, n)
        }
        other => Err(LogsError::UnknownService(other.into())),
    }
}

fn normalize_tail(n: u32) -> usize {
    let n = if n == 0 { 100 } else { n as usize };
    n.min(5000)
}

fn tail_dmesg(n: usize) -> Result<LogTail, LogsError> {
    // Prefer `dmesg` when present; fall back to reading a saved buffer.
    match Command::new("dmesg").arg("-T").output() {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            let lines = last_n_lines(&text, n);
            Ok(LogTail {
                service: "dmesg".into(),
                source: "dmesg".into(),
                lines,
            })
        }
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(LogsError::Msg(format!("dmesg failed: {err}")))
        }
        Err(_) => {
            // Dev hosts / macOS: empty rather than hard-fail.
            Ok(LogTail {
                service: "dmesg".into(),
                source: "dmesg".into(),
                lines: vec!["(dmesg unavailable on this platform)".into()],
            })
        }
    }
}

fn tail_file_or_empty(service: &str, path: &Path, n: usize) -> Result<LogTail, LogsError> {
    if !path.exists() {
        return Ok(LogTail {
            service: service.into(),
            source: path.display().to_string(),
            lines: vec![format!("(no log file at {})", path.display())],
        });
    }
    let lines = tail_file(path, n)?;
    Ok(LogTail {
        service: service.into(),
        source: path.display().to_string(),
        lines,
    })
}

fn tail_file(path: &Path, n: usize) -> Result<Vec<String>, LogsError> {
    let mut f = File::open(path)?;
    let len = f.seek(SeekFrom::End(0))?;
    // Read at most last 512 KiB for efficiency.
    let window = 512 * 1024u64;
    let start = len.saturating_sub(window);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = String::new();
    f.read_to_string(&mut buf)?;
    Ok(last_n_lines(&buf, n))
}

fn last_n_lines(text: &str, n: usize) -> Vec<String> {
    let mut lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    if lines.len() > n {
        lines = lines.split_off(lines.len() - n);
    }
    lines
}

/// Append a line to STATE log/pertiskd.log (best-effort).
pub fn append_pertiskd_log(state_root: &Path, line: &str) -> std::io::Result<()> {
    use std::io::Write;
    let dir = state_root.join("log");
    std::fs::create_dir_all(&dir)?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("pertiskd.log"))?;
    writeln!(f, "{line}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn tails_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("t.log");
        let mut f = File::create(&path).unwrap();
        for i in 0..20 {
            writeln!(f, "line-{i}").unwrap();
        }
        let lines = tail_file(&path, 5).unwrap();
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0], "line-15");
        assert_eq!(lines[4], "line-19");
    }

    #[test]
    fn unknown_service() {
        let dir = tempdir().unwrap();
        assert!(matches!(
            tail_logs(dir.path(), "nope", 10),
            Err(LogsError::UnknownService(_))
        ));
    }
}
