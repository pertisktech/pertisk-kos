//! Collect recent log lines for management API `Logs` (unary tail + follow).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

use thiserror::Error;

use crate::containers;

#[derive(Debug, Error)]
pub enum LogsError {
    #[error("unknown service '{0}' (want pertiskd|containerd|kubelet|dmesg|container:<id>)")]
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

/// How to continue after the initial tail when `follow=true`.
#[derive(Debug, Clone)]
pub enum FollowSource {
    /// Append-only file (pertiskd / containerd / kubelet / CRI).
    File { path: PathBuf },
    /// Poll `dmesg -T` for new lines.
    Dmesg,
    /// Nothing to follow (soft-fail / missing file).
    None,
}

/// File path for a named service, if it is file-backed (not dmesg).
pub fn log_file_path(state_root: &Path, service: &str) -> Option<PathBuf> {
    match service {
        "pertiskd" => Some(state_root.join("log/pertiskd.log")),
        "containerd" => Some(PathBuf::from("/var/log/containerd.log")),
        "kubelet" => Some(PathBuf::from("/var/log/kubelet.log")),
        _ => None,
    }
}

/// Tail logs for a named service.
pub fn tail_logs(state_root: &Path, service: &str, tail_lines: u32) -> Result<LogTail, LogsError> {
    let n = normalize_tail(tail_lines);
    if let Some(id) = service.strip_prefix("container:") {
        return tail_container_logs(id, n);
    }
    match service {
        "dmesg" => tail_dmesg(n),
        "pertiskd" | "containerd" | "kubelet" => {
            let path = log_file_path(state_root, service).expect("file-backed service");
            tail_file_or_empty(service, &path, n)
        }
        other => Err(LogsError::UnknownService(other.into())),
    }
}

/// Resolve follow source after a successful `tail_logs`.
pub fn follow_source(state_root: &Path, service: &str) -> Result<FollowSource, LogsError> {
    if let Some(id) = service.strip_prefix("container:") {
        let resolved = containers::resolve_cri_log(id);
        return Ok(match resolved.path {
            Some(p) if Path::new(&p).exists() => FollowSource::File {
                path: PathBuf::from(p),
            },
            _ => FollowSource::None,
        });
    }
    match service {
        "dmesg" => Ok(FollowSource::Dmesg),
        "pertiskd" | "containerd" | "kubelet" => {
            let path = log_file_path(state_root, service).expect("file-backed service");
            Ok(if path.exists() {
                FollowSource::File { path }
            } else {
                FollowSource::None
            })
        }
        other => Err(LogsError::UnknownService(other.into())),
    }
}

/// Blocking follow loop: send chunks of new lines until `cancel` is set or the
/// receiver drops.
pub fn follow_logs(
    source: FollowSource,
    service: &str,
    tx: &mpsc::Sender<LogTail>,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<(), LogsError> {
    match source {
        FollowSource::None => Ok(()),
        FollowSource::File { path } => follow_file(&path, service, tx, cancel),
        FollowSource::Dmesg => follow_dmesg(service, tx, cancel),
    }
}

fn follow_file(
    path: &Path,
    service: &str,
    tx: &mpsc::Sender<LogTail>,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<(), LogsError> {
    let mut f = File::open(path)?;
    let mut pos = f.seek(SeekFrom::End(0))?;
    let mut partial = String::new();
    let source = path.display().to_string();

    while !cancel.load(std::sync::atomic::Ordering::Relaxed) {
        // Truncation / rotation: restart from beginning.
        let meta_len = f.metadata()?.len();
        if meta_len < pos {
            pos = 0;
            partial.clear();
            f.seek(SeekFrom::Start(0))?;
        }

        f.seek(SeekFrom::Start(pos))?;
        let mut buf = String::new();
        let n = f.read_to_string(&mut buf)?;
        if n == 0 {
            std::thread::sleep(Duration::from_millis(400));
            continue;
        }
        pos += n as u64;

        partial.push_str(&buf);
        let mut lines = Vec::new();
        while let Some(idx) = partial.find('\n') {
            let mut line = partial[..idx].to_string();
            if line.ends_with('\r') {
                line.pop();
            }
            lines.push(line);
            partial = partial[idx + 1..].to_string();
        }
        if !lines.is_empty()
            && tx
                .send(LogTail {
                    service: service.into(),
                    source: source.clone(),
                    lines,
                })
                .is_err()
        {
            break;
        }
    }
    Ok(())
}

fn follow_dmesg(
    service: &str,
    tx: &mpsc::Sender<LogTail>,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<(), LogsError> {
    let mut last: Vec<String> = Vec::new();
    while !cancel.load(std::sync::atomic::Ordering::Relaxed) {
        let snap = match Command::new("dmesg").arg("-T").output() {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|l| l.to_string())
                .collect::<Vec<_>>(),
            _ => {
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
        };
        if snap.len() > last.len() && snap[..last.len()] == *last {
            let new_lines: Vec<String> = snap[last.len()..].to_vec();
            if !new_lines.is_empty()
                && tx
                    .send(LogTail {
                        service: service.into(),
                        source: "dmesg".into(),
                        lines: new_lines,
                    })
                    .is_err()
            {
                break;
            }
        } else if snap != last && !last.is_empty() {
            // Buffer rotated — emit last few new-looking lines only if suffix differs.
            let emit = snap
                .iter()
                .rev()
                .take(20)
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>();
            if tx
                .send(LogTail {
                    service: service.into(),
                    source: "dmesg".into(),
                    lines: emit,
                })
                .is_err()
            {
                break;
            }
        }
        last = snap;
        std::thread::sleep(Duration::from_secs(2));
    }
    Ok(())
}

fn tail_container_logs(id: &str, n: usize) -> Result<LogTail, LogsError> {
    let service = format!("container:{}", id.trim());
    let resolved = containers::resolve_cri_log(id);
    let Some(path) = resolved.path.as_deref() else {
        return Ok(LogTail {
            service,
            source: resolved.message.clone(),
            lines: vec![resolved.message],
        });
    };
    let path = Path::new(path);
    if !path.exists() {
        return Ok(LogTail {
            service,
            source: path.display().to_string(),
            lines: vec![format!("(no log file at {})", path.display())],
        });
    }
    let lines = tail_file(path, n)?;
    Ok(LogTail {
        service,
        source: path.display().to_string(),
        lines,
    })
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
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use tempfile::tempdir;

    #[test]
    fn file_backed_paths() {
        let p = log_file_path(Path::new("/system/state"), "pertiskd").unwrap();
        assert!(p.ends_with("log/pertiskd.log"));
        assert!(log_file_path(Path::new("/system/state"), "dmesg").is_none());
    }

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

    #[test]
    fn container_service_soft_fails_without_runtime() {
        let dir = tempdir().unwrap();
        let tail = tail_logs(dir.path(), "container:deadbeef", 10).unwrap();
        assert_eq!(tail.service, "container:deadbeef");
        assert!(!tail.lines.is_empty());
        // No containerd on the unit-test host → soft message, not InvalidArgument.
        assert!(
            tail.lines[0].contains("containerd socket")
                || tail.lines[0].contains("ctr binary")
                || tail.lines[0].contains("no container")
                || tail.lines[0].contains("no log file")
        );
    }

    #[test]
    fn follow_file_emits_appends() {
        use std::sync::Arc;

        let dir = tempdir().unwrap();
        let path = dir.path().join("f.log");
        File::create(&path).unwrap();

        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let path_c = path.clone();
        let cancel_c = Arc::clone(&cancel);
        let handle = thread::spawn(move || follow_file(&path_c, "test", &tx, &cancel_c));

        thread::sleep(Duration::from_millis(100));
        {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            writeln!(f, "hello-follow").unwrap();
        }
        let chunk = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(chunk.lines, vec!["hello-follow".to_string()]);
        cancel.store(true, Ordering::Relaxed);
        let _ = handle.join();
    }
}
