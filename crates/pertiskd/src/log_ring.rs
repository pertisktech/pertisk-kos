//! Bounded in-memory log buffer for the console dashboard and optional file sink.

use std::collections::VecDeque;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::MakeWriter;

const DEFAULT_CAPACITY: usize = 500;

/// Drop ANSI escape sequences and control bytes.
///
/// The console dashboard renders one character per terminal cell, so an
/// embedded `\x1b[32m` would otherwise show up as literal `[32m` text.
/// Child processes (containerd, kubelet) colorize their output too.
pub fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.peek() {
                // CSI: consume through the final byte (@ to ~).
                Some('[') => {
                    chars.next();
                    for next in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&next) {
                            break;
                        }
                    }
                }
                // OSC: consume through BEL or ESC \.
                Some(']') => {
                    chars.next();
                    while let Some(next) = chars.next() {
                        if next == '\u{7}' {
                            break;
                        }
                        if next == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                // Two-character escape.
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
            continue;
        }
        if c == '\t' {
            out.push(' ');
        } else if !c.is_control() {
            out.push(c);
        }
    }
    out
}

/// Shared ring of recent log lines.
#[derive(Clone, Debug)]
pub struct LogRing {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    lines: Mutex<VecDeque<String>>,
    capacity: usize,
    silence_stderr: AtomicBool,
    state_log: Mutex<Option<PathBuf>>,
    /// Partial line buffer for MakeWriter chunks that split mid-line.
    pending: Mutex<String>,
}

impl Default for LogRing {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

impl LogRing {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Inner {
                lines: Mutex::new(VecDeque::with_capacity(capacity)),
                capacity: capacity.max(32),
                silence_stderr: AtomicBool::new(false),
                state_log: Mutex::new(None),
                pending: Mutex::new(String::new()),
            }),
        }
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn set_silence_stderr(&self, silence: bool) {
        self.inner.silence_stderr.store(silence, Ordering::SeqCst);
    }

    pub fn silence_stderr(&self) -> bool {
        self.inner.silence_stderr.load(Ordering::SeqCst)
    }

    /// Best-effort append target: `STATE/log/pertiskd.log`.
    pub fn set_state_root(&self, state_root: &Path) {
        if let Ok(mut g) = self.inner.state_log.lock() {
            *g = Some(state_root.to_path_buf());
        }
    }

    #[allow(dead_code)]
    pub fn push(&self, line: impl Into<String>) {
        self.push_line(line.into(), true, true);
    }

    /// Ring (+ stderr when not silenced); does not write STATE/pertiskd.log
    /// (child tees already write `/var/log/*.log`).
    pub fn push_prefixed(&self, prefix: &str, line: &str) {
        self.push_line(format!("{prefix}: {line}"), true, false);
    }

    fn push_line(&self, line: String, mirror_stderr: bool, to_state_log: bool) {
        let line = strip_ansi(line.trim_end_matches(['\r', '\n']));
        if line.is_empty() {
            return;
        }
        if let Ok(mut q) = self.inner.lines.lock() {
            while q.len() >= self.inner.capacity {
                q.pop_front();
            }
            q.push_back(line.clone());
        }
        if to_state_log {
            if let Ok(guard) = self.inner.state_log.lock() {
                if let Some(ref root) = *guard {
                    let _ = pertisk_api::append_pertiskd_log(root, &line);
                }
            }
        }
        if mirror_stderr && !self.silence_stderr() {
            let _ = writeln!(io::stderr(), "{line}");
        }
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn tail(&self, n: usize) -> Vec<String> {
        let Ok(q) = self.inner.lines.lock() else {
            return Vec::new();
        };
        let n = n.min(q.len());
        q.iter()
            .rev()
            .take(n)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    /// Callback suitable for runtime/kubelet stderr tees.
    pub fn sink(&self, prefix: &'static str) -> Arc<dyn Fn(&str) + Send + Sync> {
        let ring = self.clone();
        Arc::new(move |line: &str| {
            // Child tees already decide stderr policy via silence flag.
            ring.push_prefixed(prefix, line);
        })
    }

    /// Writer factory for `tracing_subscriber::fmt`.
    pub fn make_writer(&self) -> LogRingWriter {
        LogRingWriter {
            ring: self.clone(),
        }
    }
}

/// [`MakeWriter`] that splits on newlines into the ring (and stderr unless silenced).
#[derive(Clone, Debug)]
pub struct LogRingWriter {
    ring: LogRing,
}

impl<'a> MakeWriter<'a> for LogRingWriter {
    type Writer = LogRingIo;

    fn make_writer(&'a self) -> Self::Writer {
        LogRingIo {
            ring: self.ring.clone(),
        }
    }
}

pub struct LogRingIo {
    ring: LogRing,
}

impl Write for LogRingIo {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let text = String::from_utf8_lossy(buf);
        let mut complete = Vec::new();
        {
            let mut pending = self
                .ring
                .inner
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            pending.push_str(&text);
            while let Some(idx) = pending.find('\n') {
                let line = pending[..idx].to_string();
                pending.drain(..=idx);
                complete.push(line);
            }
        }
        for line in complete {
            self.ring.push_line(line, true, true);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::strip_ansi;

    #[test]
    fn removes_sgr_color_codes() {
        assert_eq!(strip_ansi("\u{1b}[32m INFO\u{1b}[0m ready"), " INFO ready");
    }

    #[test]
    fn removes_bare_csi_left_by_earlier_stripping() {
        assert_eq!(strip_ansi("\u{1b}[2K\u{1b}[1;33mwarn\u{1b}[0m"), "warn");
    }

    #[test]
    fn keeps_plain_text_and_tabs() {
        assert_eq!(strip_ansi("kubelet\tup"), "kubelet up");
    }
}
