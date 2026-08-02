//! Full-pane panel TUI for Proxmox Serial (xterm.js).
//!
//! Size is pinned once (ptkube-style). Re-probing every tick with CPR
//! (`CSI 999;999H`) makes the cursor blink and breaks width when the
//! reply does not match the real pane. Frame dump keeps the cursor hidden.

use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use pertisk_api::SharedState;
use pertisk_config::MachineConfig;
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use tracing::info;

use crate::dashboard::panels;
use crate::dashboard::snapshot::StatusSnapshot;
use crate::log_ring::LogRing;

/// Safe defaults for Proxmox Serial / xterm.js (same as ptkube-dashboard).
const PIN_WIDTH: u16 = 80;
const PIN_HEIGHT: u16 = 24;
const REFRESH_MS: u64 = 2000;

/// Hide cursor + disable blink (xterm).
const CURSOR_OFF: &[u8] = b"\x1b[?25l\x1b[?12l";

pub fn tui_available() -> bool {
    true
}

pub fn run_tui_loop(
    stop: Arc<AtomicBool>,
    cfg: Option<MachineConfig>,
    state: SharedState,
    state_root: PathBuf,
    logs: LogRing,
) -> Result<(), String> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_tui_inner(stop, cfg, state, state_root, logs)
    }));
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(err),
        Err(_) => Err("TUI paniced".into()),
    }
}

fn run_tui_inner(
    stop: Arc<AtomicBool>,
    cfg: Option<MachineConfig>,
    state: SharedState,
    state_root: PathBuf,
    logs: LogRing,
) -> Result<(), String> {
    let (width, height) = pinned_size();
    let mut terminal = Terminal::new(TestBackend::new(width, height))
        .map_err(|e| format!("terminal init: {e}"))?;

    info!(width, height, "console TUI started (pinned size)");
    // One-shot status on stderr before we own the screen (not every refresh).
    let _ = writeln!(
        io::stderr(),
        "pertiskd: console TUI {width}x{height} (pinned)"
    );

    // Clear once, hide cursor. Do not re-clear every tick (causes blink).
    let _ = io::stderr().write_all(b"\x1b[2J\x1b[H");
    let _ = io::stderr().write_all(CURSOR_OFF);
    let _ = io::stderr().flush();

    while !stop.load(Ordering::SeqCst) {
        let snap = StatusSnapshot::collect(cfg.as_ref(), &state, &state_root);
        let log_lines = panels::log_inner_height(height).max(2) as usize;
        let recent = logs.tail(log_lines);
        terminal
            .draw(|frame| panels::render(frame, &snap, &recent, width))
            .map_err(|e| format!("TUI draw: {e}"))?;
        dump_frame(terminal.backend())?;

        let steps = (REFRESH_MS / 100).max(1);
        for _ in 0..steps {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    let _ = io::stderr().write_all(b"\x1b[?25h\x1b[?12h");
    let _ = io::stderr().flush();
    Ok(())
}

/// Pin console size — env override, else 80×24.
///
/// Proxmox Serial size probes are unreliable; a wider dump than the pane
/// wraps lines and destroys ASCII borders. Matching ptkube: always pin.
fn pinned_size() -> (u16, u16) {
    let cols = std::env::var("PERTISK_DASHBOARD_COLS")
        .ok()
        .and_then(|s| s.parse().ok())
        .or_else(|| std::env::var("COLUMNS").ok().and_then(|s| s.parse().ok()))
        .unwrap_or(PIN_WIDTH);
    let rows = std::env::var("PERTISK_DASHBOARD_ROWS")
        .ok()
        .and_then(|s| s.parse().ok())
        .or_else(|| std::env::var("LINES").ok().and_then(|s| s.parse().ok()))
        .unwrap_or(PIN_HEIGHT);
    // Keep within a range that fits typical Serial panes without wrapping.
    (
        cols.clamp(60, 100),
        rows.clamp(20, 40),
    )
}

fn dump_frame(backend: &TestBackend) -> Result<(), String> {
    let buf = backend.buffer();
    let area = buf.area();
    let mut out = String::with_capacity((area.width as usize + 3) * area.height as usize + 32);
    // Home + hide cursor (never park at bottom-right — that blinks).
    out.push_str("\x1b[H\x1b[?25l\x1b[?12l");
    for y in 0..area.height {
        for x in 0..area.width {
            let sym = buf[(x, y)].symbol();
            let ch = sym.chars().next().unwrap_or(' ');
            out.push(if ch == '\0' || !ch.is_ascii() {
                ' '
            } else if ch.is_control() {
                ' '
            } else {
                ch
            });
        }
        // Clear remainder of the real terminal line (handles wider panes).
        out.push_str("\x1b[K");
        if y + 1 < area.height {
            out.push_str("\r\n");
        }
    }
    // Leave cursor at home, hidden — not on the last cell.
    out.push_str("\x1b[H\x1b[?25l\x1b[?12l");
    io::stderr()
        .write_all(out.as_bytes())
        .map_err(|e| format!("TUI write: {e}"))?;
    io::stderr().flush().map_err(|e| format!("TUI flush: {e}"))?;
    Ok(())
}
