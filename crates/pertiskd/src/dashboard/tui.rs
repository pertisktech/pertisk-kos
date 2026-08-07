//! Full-pane panel TUI for Proxmox Serial (xterm.js).
//!
//! Renders into a ratatui `TestBackend`, then paints only the rows that
//! changed (absolute CUP, no DEC synchronized updates — those blank Proxmox).
//! Cursor is forced off around every write so a slow serial line does not
//! show it walking through the node panel / mid-screen.

use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use pertisk_api::SharedState;
use pertisk_config::MachineConfig;
use ratatui::backend::TestBackend;
use ratatui::style::Modifier;
use ratatui::Terminal;
use tracing::info;

use crate::dashboard::snapshot::StatusSnapshot;
use crate::dashboard::{panels, probe, theme};
use crate::log_ring::LogRing;

const REFRESH_MS: u64 = 2000;

/// How often to force a full Serial repaint even when nothing changed.
/// Proxmox xterm.js starts blank when you open/switch the console; without a
/// periodic dump the client never sees the last frame. Keep this >> 1 —
/// every-tick invalidate made the pane look like a blinking cursor.
const FORCE_REPAINT_EVERY: u32 = 15;

/// Hide cursor + disable blink (xterm / Proxmox Serial).
///
/// Do **not** send DECSCUSR (`CSI 2 SP q`) — Proxmox Serial / xterm.js often
/// fails to parse the space, and a literal `q` appears on screen.
/// Re-assert often: xterm.js re-enables the cursor on focus / some CSI.
const CURSOR_OFF: &str = "\x1b[?25l\x1b[?12l";

/// Clear any stuck synchronized-update mode from a previous build.
const UNSYNC: &str = "\x1b[?2026l";

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
    // Safety net: apply built-ins / YAML even if the caller forgot.
    sync_dashboard_env(cfg.as_ref(), &state, &state_root);
    let mut caps = probe::detect();
    let (mut width, mut height) = (caps.cols, caps.rows);
    let mut skin = build_skin(width, height, caps.source, caps.utf8);
    let mut terminal = Terminal::new(TestBackend::new(width, height))
        .map_err(|e| format!("terminal init: {e}"))?;

    info!(
        width,
        height,
        size_source = caps.source,
        utf8 = caps.utf8,
        theme = skin.theme.name,
        border = skin.chrome.name,
        "console TUI started"
    );
    info!(
        "console TUI {width}x{height} ({}) theme={} border={}",
        caps.source, skin.theme.name, skin.chrome.name
    );

    // Never `\x1b[2J` — a clear that races boot `eprintln!` / failed paint
    // leaves Proxmox Serial blank. Full-frame home+\r\n paint overwrites.
    paint_console(UNSYNC.as_bytes());
    paint_console(CURSOR_OFF.as_bytes());

    let mut writer = FrameWriter::default();
    let mut last_sig = config_signature(&caps, &skin);
    let mut ticks: u32 = 0;
    // Paint immediately so Serial is never left empty after cursor-off.
    {
        let snap = StatusSnapshot::collect(cfg.as_ref(), &state, &state_root);
        let log_rows = panels::log_inner_height_for(height, skin.mgmt_url.is_some()).max(2) as usize;
        let recent = logs.tail(panels::log_tail_count(log_rows));
        terminal
            .draw(|frame| panels::render_themed(frame, &snap, &recent, &skin))
            .map_err(|e| format!("TUI draw: {e}"))?;
        dump_frame(&mut writer, terminal.backend(), skin.chrome.ascii_only)?;
    }

    while !stop.load(Ordering::SeqCst) {
        let steps = (REFRESH_MS / 100).max(1);
        for _ in 0..steps {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            // Proxmox Serial / xterm.js often re-shows the cursor on focus;
            // keep asserting hide between paints.
            paint_console(CURSOR_OFF.as_bytes());
            thread::sleep(Duration::from_millis(100));
        }
        if stop.load(Ordering::SeqCst) {
            break;
        }

        // Pick up `pertiskctl apply` / STATE config written after early start.
        sync_dashboard_env(cfg.as_ref(), &state, &state_root);
        caps = probe::detect_refresh(caps);
        let next_skin = build_skin(caps.cols, caps.rows, caps.source, caps.utf8);
        let sig = config_signature(&caps, &next_skin);
        // Always refresh mgmt_url — apply can set it without changing theme/size,
        // and the old signature ignored that field so the node panel never updated.
        let mgmt_changed = skin.mgmt_url != next_skin.mgmt_url;
        if sig != last_sig || mgmt_changed {
            width = caps.cols;
            height = caps.rows;
            skin = next_skin;
            terminal = Terminal::new(TestBackend::new(width, height))
                .map_err(|e| format!("terminal resize: {e}"))?;
            writer = FrameWriter::default();
            last_sig = sig;
            info!(
                width,
                height,
                theme = skin.theme.name,
                border = skin.chrome.name,
                utf8 = caps.utf8,
                mgmt_url = skin.mgmt_url.as_deref().unwrap_or(""),
                "console TUI reloaded from config"
            );
        }

        let snap = StatusSnapshot::collect(cfg.as_ref(), &state, &state_root);
        let log_rows = panels::log_inner_height_for(height, skin.mgmt_url.is_some()).max(2) as usize;
        let recent = logs.tail(panels::log_tail_count(log_rows));
        terminal
            .draw(|frame| panels::render_themed(frame, &snap, &recent, &skin))
            .map_err(|e| format!("TUI draw: {e}"))?;
        ticks = ticks.wrapping_add(1);
        // Periodic re-send so a newly opened Proxmox Serial tab is not blank.
        if ticks % FORCE_REPAINT_EVERY == 0 {
            writer.invalidate();
        }
        dump_frame(&mut writer, terminal.backend(), skin.chrome.ascii_only)?;
    }

    paint_console(b"\x1b[?25h\x1b[?12h");
    Ok(())
}

fn build_skin(_width: u16, _height: u16, _source: &str, utf8: bool) -> panels::Skin {
    let chrome = theme::chrome(utf8);
    panels::Skin {
        theme: theme::active(),
        chrome,
        mgmt_url: crate::dashboard::mgmt_public_url(),
    }
}

fn config_signature(caps: &probe::ConsoleCaps, skin: &panels::Skin) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}",
        caps.cols,
        caps.rows,
        caps.utf8,
        skin.theme.name,
        skin.chrome.name,
        skin.mgmt_url.as_deref().unwrap_or(""),
    )
}

/// Apply YAML from STATE (preferred) or the boot-time cfg into dashboard env.
fn sync_dashboard_env(cfg: Option<&MachineConfig>, state: &SharedState, state_root: &PathBuf) {
    let config_path = if let Ok(st) = state.lock() {
        st.config_path.clone()
    } else {
        state_root.join("config.yaml")
    };
    let disk = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|y| MachineConfig::from_yaml(&y).ok());
    crate::dashboard::apply_config(disk.as_ref().or(cfg));
}

fn dump_frame(
    writer: &mut FrameWriter,
    backend: &TestBackend,
    ascii_only: bool,
) -> Result<(), String> {
    let Some(out) = writer.encode(backend.buffer(), ascii_only) else {
        paint_console(CURSOR_OFF.as_bytes());
        return Ok(());
    };
    paint_console(out.as_bytes());
    Ok(())
}

/// Write the TUI to stderr (usually ttyS0 / Proxmox Serial) and mirror to
/// `/dev/tty0` so ESXi Host Client VGA also shows the dashboard.
fn paint_console(bytes: &[u8]) {
    let _ = io::stderr().write_all(bytes);
    let _ = io::stderr().flush();
    mirror_vga(bytes);
}

fn mirror_vga(bytes: &[u8]) {
    #[cfg(target_os = "linux")]
    {
        use std::fs::OpenOptions;
        use std::os::unix::io::AsRawFd;
        let Ok(mut f) = OpenOptions::new().write(true).open("/dev/tty0") else {
            return;
        };
        // Skip if stderr is already this VT (avoid double paint).
        let tty0 = f.as_raw_fd();
        let same = unsafe {
            let mut s = std::mem::MaybeUninit::<libc::stat>::uninit();
            let mut e = std::mem::MaybeUninit::<libc::stat>::uninit();
            if libc::fstat(tty0, s.as_mut_ptr()) != 0 || libc::fstat(2, e.as_mut_ptr()) != 0 {
                false
            } else {
                let s = s.assume_init();
                let e = e.assume_init();
                s.st_rdev == e.st_rdev && s.st_rdev != 0
            }
        };
        if same {
            return;
        }
        let _ = f.write_all(bytes);
        let _ = f.flush();
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = bytes;
    }
}

/// Full-frame Serial dump: home + one `\r\n` per row (same shape as the text
/// banner). Per-row CUP left Proxmox xterm.js blank on some builds; the
/// newline walk is slightly blinkier but always visible with the cursor off.
#[derive(Default)]
struct FrameWriter {
    last: String,
}

impl FrameWriter {
    /// Forget the last frame so the next encode always emits (console reconnect).
    fn invalidate(&mut self) {
        self.last.clear();
    }

    fn encode(&mut self, buf: &ratatui::buffer::Buffer, ascii_only: bool) -> Option<String> {
        let area = buf.area();
        let mut out = String::with_capacity((area.width as usize + 16) * area.height as usize);
        // Hide before home — Proxmox otherwise flashes a blink on the title border.
        out.push_str(CURSOR_OFF);
        out.push_str("\x1b[H");
        out.push_str(CURSOR_OFF);
        for y in 0..area.height {
            // Re-hide each row: xterm.js can re-show the cursor mid-frame.
            out.push_str(CURSOR_OFF);
            out.push_str(&encode_row(buf, y, ascii_only));
            // Last row: no trailing \r\n (avoids scroll). Never CUP back to home.
            if y + 1 < area.height {
                out.push_str("\x1b[0m\x1b[K\r\n");
            } else {
                out.push_str("\x1b[0m\x1b[K");
            }
        }
        // Park bottom-right (not home) so a failed hide does not blink on top.
        out.push_str(CURSOR_OFF);
        out.push_str(&format!("\x1b[{};{}H", area.height, area.width));
        out.push_str(CURSOR_OFF);
        if out == self.last {
            return None;
        }
        self.last = out.clone();
        Some(out)
    }
}

fn encode_row(buf: &ratatui::buffer::Buffer, y: u16, ascii_only: bool) -> String {
    let area = buf.area();
    let mut out = String::with_capacity(area.width as usize + 16);
    let mut current = Some((None, false));
    for x in 0..area.width {
        let cell = &buf[(x, y)];
        let want = (
            theme::ansi_fg(cell.fg),
            cell.modifier.contains(Modifier::BOLD),
        );
        if current != Some(want) {
            out.push_str(&sgr(want.0, want.1));
            current = Some(want);
        }
        out.push_str(&glyph(cell.symbol(), ascii_only));
    }
    out
}

fn glyph(symbol: &str, ascii_only: bool) -> String {
    let ch = symbol.chars().next().unwrap_or(' ');
    if ch.is_control() || ch == '\0' {
        return " ".into();
    }
    if ch.is_ascii() {
        return ch.to_string();
    }
    if ascii_only {
        return ascii_fallback(ch).to_string();
    }
    ch.to_string()
}

fn ascii_fallback(ch: char) -> char {
    match ch {
        '─' | '━' | '═' => '-',
        '│' | '┃' | '║' => '|',
        '█' | '▓' | '▒' => '|',
        '░' => '-',
        '┌' | '┐' | '└' | '┘' | '┏' | '┓' | '┗' | '┛' | '╔' | '╗' | '╚' | '╝' | '╭' | '╮' | '╯'
        | '╰' => '+',
        '├' | '┤' | '┬' | '┴' | '┼' => '+',
        _ => ' ',
    }
}

fn sgr(fg: Option<u8>, bold: bool) -> String {
    match (fg, bold) {
        (None, false) => "\x1b[0m".to_string(),
        (None, true) => "\x1b[0;1m".to_string(),
        (Some(code), false) => format!("\x1b[0;{code}m"),
        (Some(code), true) => format!("\x1b[0;1;{code}m"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dashboard::snapshot::{DiskUsage, StatusSnapshot};

    fn demo_snapshot() -> StatusSnapshot {
        StatusSnapshot {
            hostname: "pertisk-node-01".into(),
            version: "0.1.0".into(),
            ready: true,
            cpu_cores: 4,
            cpu_usage_pct: 37,
            load_1m: 0.42,
            mem_total_kb: 8 * 1024 * 1024,
            mem_available_kb: 3 * 1024 * 1024,
            disks: vec![DiskUsage {
                label: "state".into(),
                total_bytes: 40 * 1024 * 1024 * 1024,
                used_bytes: 9 * 1024 * 1024 * 1024,
            }],
            net_rows: vec!["eth0 UP 192.168.1.50/24".into()],
            node_iface: "eth0".into(),
            node_ip: "192.168.1.50/24".into(),
            machine_type: "controlplane".into(),
            cluster_endpoint: "https://10.0.0.1:6443".into(),
            cni: "flannel".into(),
            pod_cidr: "10.244.0.0/16".into(),
            kubernetes_version: "v1.36.3".into(),
            containerd: "up".into(),
            containerd_pid: 412,
            kubelet: "failed".into(),
            boot_slot: "A".into(),
            boot_ok: true,
            boot_attempts: 1,
            ..Default::default()
        }
    }

    fn demo_logs() -> Vec<String> {
        vec![
            "INFO kubelet starting with a very long argument list that must wrap instead of being cut off".into(),
            "ERROR failed to pull image registry.k8s.io/pause:3.9".into(),
            "INFO node ready".into(),
        ]
    }

    fn draw_demo(
        width: u16,
        height: u16,
        chrome: theme::Chrome,
        logs: &[String],
    ) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let skin = panels::Skin {
            theme: theme::DRACULA,
            chrome,
            mgmt_url: None,
        };
        terminal
            .draw(|f| panels::render_themed(f, &demo_snapshot(), logs, &skin))
            .unwrap();
        terminal
    }

    fn render_demo(width: u16, height: u16, chrome: theme::Chrome) -> String {
        let terminal = draw_demo(width, height, chrome, &demo_logs());
        FrameWriter::default()
            .encode(terminal.backend().buffer(), chrome.ascii_only)
            .expect("first frame is never empty")
    }

    fn demo_rows(width: u16, height: u16, chrome: theme::Chrome) -> Vec<String> {
        let terminal = draw_demo(width, height, chrome, &demo_logs());
        let buf = terminal.backend().buffer();
        (0..height)
            .map(|y| encode_row(buf, y, chrome.ascii_only))
            .collect()
    }

    #[test]
    fn ascii_chrome_dump_is_pure_ascii() {
        let out = render_demo(80, 24, theme::ASCII);
        assert!(out.is_ascii(), "frame dump contained non-ASCII bytes");
    }

    #[test]
    fn dump_only_emits_known_escape_sequences() {
        let out = render_demo(80, 24, theme::ASCII);
        for seq in out.split('\u{1b}').skip(1) {
            let ok = seq.starts_with("[H")
                || seq.starts_with("[K")
                || seq.starts_with("[?25l")
                || seq.starts_with("[?12l")
                || (seq.starts_with('[') && seq[1..].starts_with(|c: char| c.is_ascii_digit()));
            assert!(
                ok,
                "unexpected escape sequence: {:?}",
                &seq[..seq.len().min(12)]
            );
            assert!(
                !seq.starts_with("[2 q") && !seq.contains(" q"),
                "DECSCUSR must not be used (prints literal q on Serial): {seq:?}"
            );
        }
    }

    #[test]
    fn every_row_is_exactly_frame_width() {
        for (chrome, width) in [
            (theme::ASCII, 80u16),
            (theme::LIGHT, 80),
            (theme::DOUBLE, 120),
            (theme::ASCII, 200),
        ] {
            let rows = demo_rows(width, 24, chrome);
            assert_eq!(rows.len(), 24);
            for (i, row) in rows.iter().enumerate() {
                let cols = strip_escapes(row).chars().count();
                assert_eq!(
                    cols, width as usize,
                    "{} row {i} is {cols} columns",
                    chrome.name
                );
            }
        }
    }

    #[test]
    fn panels_span_the_full_width() {
        let rows = demo_rows(160, 24, theme::ASCII);
        let first = strip_escapes(&rows[0]);
        assert!(first.starts_with('+'), "left edge missing: {first:?}");
        assert!(first.ends_with('+'), "right edge missing: {first:?}");
    }

    #[test]
    fn status_colors_reach_the_wire() {
        let out = render_demo(80, 24, theme::ASCII);
        // Base ANSI (no bold/bright) — same SGR shape as Magenta labels.
        assert!(
            out.contains("\x1b[0;32m[up]"),
            "missing green [up]: {out:?}"
        );
        assert!(
            out.contains("\x1b[0;31m[failed]"),
            "missing red [failed]: {out:?}"
        );
    }

    #[test]
    fn absent_status_is_red_on_the_wire() {
        let mut snap = demo_snapshot();
        snap.containerd = "absent".into();
        snap.containerd_pid = 0;
        snap.kubelet = "absent".into();
        snap.kubelet_pid = 0;
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let skin = panels::Skin {
            theme: theme::WILD_CHERRY,
            chrome: theme::ASCII,
            mgmt_url: Some("https://ptkos.apps.thaidevops.co".into()),
        };
        terminal
            .draw(|f| panels::render_themed(f, &snap, &demo_logs(), &skin))
            .unwrap();
        let out = FrameWriter::default()
            .encode(terminal.backend().buffer(), true)
            .expect("frame");
        assert!(
            out.contains("\x1b[0;31m[absent]"),
            "absent must be red: {out:?}"
        );
        assert!(out.contains("kubelet"), "{out:?}");
        assert!(
            out.contains("mgmt") && out.contains("ptkos.apps.thaidevops.co"),
            "mgmt URL missing: {out:?}"
        );
    }

    #[test]
    fn frame_always_paints_something() {
        let out = render_demo(80, 24, theme::ASCII);
        assert!(out.contains("pertisk-node-01"));
        assert!(out.contains(" node "));
        assert!(!out.contains("?2026h"), "synchronized update begin present");
    }

    #[test]
    fn cursor_stays_hidden() {
        let out = render_demo(80, 24, theme::ASCII);
        assert!(out.contains("\x1b[?25l"));
        assert!(!out.contains("\x1b[?25h"));
        // Park bottom-right — not CUP home after paint (blinks on title if hide fails).
        assert!(
            out.contains("\x1b[24;80H"),
            "expected cursor park at bottom-right: {out:?}"
        );
        assert!(
            !out.ends_with("\x1b[H\x1b[?25l\x1b[?12l"),
            "must not leave cursor at home"
        );
    }

    #[test]
    fn identical_frame_emits_nothing() {
        let terminal = draw_demo(80, 24, theme::ASCII, &demo_logs());
        let mut writer = FrameWriter::default();
        assert!(writer.encode(terminal.backend().buffer(), true).is_some());
        assert!(writer.encode(terminal.backend().buffer(), true).is_none());
    }

    #[test]
    fn invalidate_forces_repaint_for_console_reconnect() {
        let terminal = draw_demo(80, 24, theme::ASCII, &demo_logs());
        let mut writer = FrameWriter::default();
        assert!(writer.encode(terminal.backend().buffer(), true).is_some());
        assert!(writer.encode(terminal.backend().buffer(), true).is_none());
        writer.invalidate();
        let out = writer.encode(terminal.backend().buffer(), true);
        assert!(out.is_some(), "reconnect must get a full frame");
        assert!(out.unwrap().contains("pertisk-node-01"));
    }

    #[test]
    fn changed_log_repaints_full_frame() {
        let mut writer = FrameWriter::default();
        let first = draw_demo(80, 24, theme::ASCII, &demo_logs());
        writer.encode(first.backend().buffer(), true).unwrap();

        let mut logs = demo_logs();
        logs.push("INFO one more line".into());
        let second = draw_demo(80, 24, theme::ASCII, &logs);
        let out = writer.encode(second.backend().buffer(), true).unwrap();
        assert!(
            out.contains("pertisk-node-01"),
            "full-frame dump still includes node"
        );
        assert!(out.contains("one more line"), "new log missing: {out:?}");
    }

    fn strip_escapes(line: &str) -> String {
        let mut out = String::new();
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' {
                if chars.peek() == Some(&'[') {
                    chars.next();
                }
                for next in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&next) {
                        break;
                    }
                }
                continue;
            }
            out.push(c);
        }
        out
    }

    #[test]
    fn demo_frame_has_all_panels() {
        for chrome in [theme::ASCII, theme::LIGHT, theme::DOUBLE, theme::ROUNDED] {
            let rows = demo_rows(100, 26, chrome);
            let out = rows.join("\n");
            for title in [" node ", " network ", " resources ", " services ", " logs "] {
                assert!(out.contains(title), "missing panel {title}");
            }
            assert!(
                out.contains("cpu") && out.contains("memory") && out.contains("disk"),
                "resources panel missing cpu/memory/disk: {out}"
            );
            assert!(
                out.contains("Kubernetes"),
                "expected Kubernetes label: {out}"
            );
        }
    }

    #[test]
    fn unicode_glyphs_degrade_to_ascii() {
        assert_eq!(glyph("─", true), "-");
        assert_eq!(glyph("╔", true), "+");
        assert_eq!(glyph("║", true), "|");
        assert_eq!(glyph("─", false), "─");
    }
}
