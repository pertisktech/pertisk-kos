//! Full-pane panel TUI for Proxmox Serial (xterm.js).
//!
//! Renders into a ratatui `TestBackend`, then paints only the rows that
//! changed (absolute CUP, no DEC synchronized updates — those blank Proxmox).
//! Cursor is forced off around every write and parked at a stable in-frame
//! cell so asynchronous console output cannot scroll past the footer.

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

/// Status refresh interval. Keep this slow: Proxmox Serial often ignores
/// DECTCEM (`?25l`), so any full-frame paint walks a visible cursor.
const REFRESH_MS: u64 = 5000;

/// How often to force a full Serial repaint even when nothing changed.
/// Proxmox xterm.js starts blank when you open/switch the console; without a
/// periodic dump the client never sees the last frame. Keep this >> 1 —
/// every-tick invalidate made the pane look like a blinking cursor.
const FORCE_REPAINT_EVERY: u32 = 2;

/// Hide cursor + disable blink (xterm / Proxmox Serial).
///
/// Do **not** send DECSCUSR (`CSI 2 SP q`) — Proxmox Serial / xterm.js often
/// fails to parse the space, and a literal `q` appears on screen.
/// Do **not** send OSC 12 (`ESC ] 12 ; … BEL`) — Proxmox prints `#1a1a1a`
/// and rings the bell.
const CURSOR_OFF: &str = "\x1b[?25l\x1b[?12l";
/// Park at a stable in-frame cell. CUP positions beyond the terminal bounds
/// clamp to the bottom-right; the next kernel/direct console write then wraps
/// and scrolls beneath `[ END LOGS ]`.
const CURSOR_PARK: &str = "\x1b[1;1H";

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
        writer.invalidate();
        dump_frame(&mut writer, terminal.backend(), true)?;
    }

    while !stop.load(Ordering::SeqCst) {
        let steps = (REFRESH_MS / 100).max(1);
        for _ in 0..steps {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            // Keep cursor parked off-screen between paints. Hide alone is not
            // enough on Proxmox Serial (DECTCEM often ignored).
            paint_console(format!("{CURSOR_OFF}{CURSOR_PARK}").as_bytes());
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
        dump_frame(&mut writer, terminal.backend(), true)?;
    }

    paint_console(b"\x1b[?25h\x1b[?12h");
    Ok(())
}

fn build_skin(_width: u16, _height: u16, _source: &str, utf8: bool) -> panels::Skin {
    let chrome = theme::chrome(utf8);
    panels::Skin {
        theme: theme::active(),
        chrome,
        background: theme::background(),
        mgmt_url: crate::dashboard::mgmt_public_url(),
    }
}

fn config_signature(caps: &probe::ConsoleCaps, skin: &panels::Skin) -> String {
    format!(
        "{}|{}|{}|{}|{}|{:?}|{}",
        caps.cols,
        caps.rows,
        caps.utf8,
        skin.theme.name,
        skin.chrome.name,
        skin.background,
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

/// Serial dump for Proxmox xterm.js.
///
/// Prefer **per-row CUP** with an immediate off-screen park after each row.
/// A home+\r\n walk leaves a visible cursor marching top→bottom when Proxmox
/// ignores DECTCEM. Full home-walk is kept only as a fallback path via
/// [`FrameWriter::invalidate`] (console reconnect).
#[derive(Default)]
struct FrameWriter {
    last_rows: Vec<String>,
    force_full: bool,
}

impl FrameWriter {
    /// Forget the last frame so the next encode always emits (console reconnect).
    fn invalidate(&mut self) {
        self.last_rows.clear();
        self.force_full = true;
    }

    fn encode(&mut self, buf: &ratatui::buffer::Buffer, ascii_only: bool) -> Option<String> {
        let area = buf.area();
        let rows: Vec<String> = (0..area.height)
            .map(|y| encode_row(buf, y, ascii_only))
            .collect();
        let full = self.force_full || self.last_rows.len() != rows.len();
        if !full && rows == self.last_rows {
            return None;
        }

        let mut out = String::with_capacity((area.width as usize + 24) * area.height as usize);
        out.push_str(CURSOR_OFF);
        if full {
            // Address every row independently. This avoids newline/wrap races
            // and lets every reconnect receive a complete coherent frame.
            for (i, row) in rows.iter().enumerate() {
                out.push_str(CURSOR_OFF);
                out.push_str(&format!("\x1b[{};1H", i + 1));
                out.push_str(row);
                out.push_str("\x1b[0m\x1b[K");
                out.push_str(CURSOR_OFF);
                out.push_str(CURSOR_PARK);
            }
            // The browser terminal can be taller than the probed frame. Clear
            // stale text after the footer without clearing the dashboard.
            out.push_str(&format!("\x1b[{};{}H\x1b[0J", area.height, area.width));
        } else {
            // Incremental: only rewrite dirty rows, park after each so a failed
            // hide never leaves the cursor mid-panel.
            for (i, row) in rows.iter().enumerate() {
                if self.last_rows.get(i) == Some(row) {
                    continue;
                }
                let y = i as u16 + 1;
                out.push_str(CURSOR_OFF);
                out.push_str(&format!("\x1b[{y};1H"));
                out.push_str(row);
                out.push_str("\x1b[0m\x1b[K");
                out.push_str(CURSOR_OFF);
                out.push_str(CURSOR_PARK);
            }
        }
        out.push_str(CURSOR_OFF);
        out.push_str(CURSOR_PARK);
        out.push_str(CURSOR_OFF);

        self.last_rows = rows;
        self.force_full = false;
        Some(out)
    }
}

fn encode_row(buf: &ratatui::buffer::Buffer, y: u16, ascii_only: bool) -> String {
    let area = buf.area();
    let mut out = String::with_capacity(area.width as usize + 16);
    let mut current: Option<(ratatui::style::Color, ratatui::style::Color, bool)> = None;
    // Never write the terminal's final column. Serial VTs commonly auto-wrap
    // immediately after that cell, making one horizontal rule look like two.
    // FrameWriter follows each row with EL, which clears this reserved cell.
    for x in 0..area.width.saturating_sub(1) {
        let cell = &buf[(x, y)];
        let want = (cell.fg, cell.bg, cell.modifier.contains(Modifier::BOLD));
        if current != Some(want) {
            out.push_str(&sgr(want.0, want.1, want.2));
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
        // Block and box-drawing glyphs -> solid ASCII rule.
        '▀' | '▄' | '─' | '━' | '═' | '░' => '=',
        '█' | '▓' | '▒' | '│' | '┃' | '║' | '▌' | '▐' => '|',
        '┌' | '┐' | '└' | '┘' | '┏' | '┓' | '┗' | '┛' | '╔' | '╗' | '╚' | '╝' | '╭' | '╮' | '╯'
        | '╰' | '▛' | '▜' | '▙' | '▟' | '▖' | '▗' | '▘' | '▝' => '+',
        '├' | '┤' | '┬' | '┴' | '┼' => '+',
        _ => ' ',
    }
}

fn sgr(fg: ratatui::style::Color, bg: ratatui::style::Color, bold: bool) -> String {
    let mut codes = vec!["0".to_string()];
    if bold {
        codes.push("1".to_string());
    }
    if let Some(code) = theme::ansi_fg(fg) {
        codes.push(code.to_string());
    }
    match bg {
        ratatui::style::Color::Rgb(red, green, blue) => {
            // Linux VT and some serial parsers do not understand truecolor.
            // `48;2;30;30;46` is then parsed as separate SGR codes and the
            // final 46 turns the background cyan. Indexed color is unambiguous.
            codes.push(format!("48;5;{}", rgb_to_xterm256(red, green, blue)));
        }
        color => {
            if let Some(code) = theme::ansi_bg(color) {
                codes.push(code.to_string());
            }
        }
    }
    format!("\x1b[{}m", codes.join(";"))
}

fn rgb_to_xterm256(red: u8, green: u8, blue: u8) -> u8 {
    let cube_levels = [0u8, 95, 135, 175, 215, 255];
    let mut best_index = 16u8;
    let mut best_distance = u32::MAX;

    for (red_index, &cube_red) in cube_levels.iter().enumerate() {
        for (green_index, &cube_green) in cube_levels.iter().enumerate() {
            for (blue_index, &cube_blue) in cube_levels.iter().enumerate() {
                let distance = color_distance(red, green, blue, cube_red, cube_green, cube_blue);
                if distance < best_distance {
                    best_distance = distance;
                    best_index = 16 + 36 * red_index as u8 + 6 * green_index as u8 + blue_index as u8;
                }
            }
        }
    }

    for gray_index in 0..24u8 {
        let gray = 8 + gray_index * 10;
        let distance = color_distance(red, green, blue, gray, gray, gray);
        if distance < best_distance {
            best_distance = distance;
            best_index = 232 + gray_index;
        }
    }

    best_index
}

fn color_distance(red: u8, green: u8, blue: u8, other_red: u8, other_green: u8, other_blue: u8) -> u32 {
    let red_delta = i32::from(red) - i32::from(other_red);
    let green_delta = i32::from(green) - i32::from(other_green);
    let blue_delta = i32::from(blue) - i32::from(other_blue);
    (3 * red_delta * red_delta + 6 * green_delta * green_delta + blue_delta * blue_delta) as u32
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
            uptime_secs: 93_784,
            process_count: 42,
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
            service_subnet: "10.96.0.0/12".into(),
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
            background: None,
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
    fn encoded_rows_reserve_last_column_to_prevent_wrap() {
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
                assert_eq!(cols, width.saturating_sub(1) as usize, "{} row {i} is {cols} columns", chrome.name);
            }
        }
    }

    #[test]
    fn summary_headings_span_the_width() {
        let rows = demo_rows(160, 24, theme::ASCII);
        let summary_top = strip_escapes(&rows[1]);
        assert!(summary_top.starts_with("PERTISK"), "left heading missing: {summary_top:?}");
        assert!(summary_top.contains("KUBERNETES"), "center heading missing: {summary_top:?}");
        assert!(summary_top.contains("NETWORK"), "right heading missing: {summary_top:?}");
    }

    #[test]
    fn compact_summary_shows_full_endpoint_and_node_ip() {
        let rendered = demo_rows(80, 24, theme::ASCII)
            .into_iter()
            .map(|row| strip_escapes(&row))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("eth0 192.168.1.50/24"), "node IP hidden: {rendered}");
        assert!(rendered.contains("https://10.0.0.1:6443"), "endpoint clipped: {rendered}");
        assert!(rendered.contains("10.244.0.0/16"), "pod subnet hidden: {rendered}");
        assert!(rendered.contains("10.96.0.0/12"), "service subnet hidden: {rendered}");
    }

    #[test]
    fn compact_dashboard_uses_cockpit_header_and_dedicated_footer() {
        let rows: Vec<String> = demo_rows(80, 24, theme::ASCII)
            .into_iter()
            .map(|row| strip_escapes(&row))
            .collect();

        assert!(rows[0].starts_with(" PERTISK pertisk-node-01  v0.1.0"), "header: {:?}", rows[0]);
        assert!(rows[0].contains("| READY | CPU 37% RAM 62% LOAD 0.42"), "header: {:?}", rows[0]);
        assert!(rows[1].contains("[ SYSTEM ] controlplane"), "system row: {:?}", rows[1]);
        assert!(rows[6].contains("BOOT") && rows[6].contains("slot A"), "boot row: {:?}", rows[6]);
        assert!(rows[7].contains("[ LOGS ]"), "log header: {:?}", rows[7]);
        assert!(rows[23].contains("[ END LOGS ]") && rows[23].contains("refresh 5s"), "footer: {:?}", rows[23]);
    }

    #[test]
    fn logs_do_not_overlap_compact_summary_at_minimum_height() {
        let rows: Vec<String> = demo_rows(80, 8, theme::ASCII)
            .into_iter()
            .map(|row| strip_escapes(&row))
            .collect();
        assert!(rows[1].contains("[ SYSTEM ]"));
        assert!(rows[2].contains("NODE") && rows[2].contains("192.168.1.50/24"));
        assert!(rows[3].contains("ENDPOINT") && rows[3].contains("10.0.0.1:6443"));
        assert!(rows[1..5].iter().all(|row| !row.contains("INFO")));
        assert!(rows[5].contains("[ LOGS ]"));
        assert!(rows[6].contains("INFO node ready"));
        assert!(rows[7].contains("[ END LOGS ]") && rows[7].contains("pertisk-node-01"));
        assert!(rows.iter().all(|row| !row.contains("F1:SUMMARY")));
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
    fn dashboard_has_plain_background_and_clear_boundaries() {
        let out = render_demo(80, 24, theme::ASCII);
        assert!(!out.contains(";40m") && !out.contains(";45m") && !out.contains(";105m"));
        assert!(out.contains("[ SYSTEM ]"), "system boundary missing: {out:?}");
        assert!(out.contains("[ LOGS ]"), "log start boundary missing: {out:?}");
        assert!(out.contains("[ END LOGS ]"), "log end boundary missing: {out:?}");
        assert!(!out.contains("F1:SUMMARY"), "obsolete footer action present: {out:?}");
    }

    #[test]
    fn configured_hex_background_reaches_indexed_wire_without_cyan_fallback() {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let skin = panels::Skin {
            theme: theme::CATPPUCCIN,
            chrome: theme::LINE,
            background: Some(ratatui::style::Color::Rgb(30, 30, 46)),
            mgmt_url: None,
        };
        terminal
            .draw(|frame| panels::render_themed(frame, &demo_snapshot(), &demo_logs(), &skin))
            .unwrap();
        let out = FrameWriter::default()
            .encode(terminal.backend().buffer(), true)
            .expect("frame");

        assert!(
            out.contains("48;5;234m"),
            "missing nearest indexed color for #1E1E2E: {out:?}"
        );
        assert!(!out.contains("48;2;30;30;46m"), "unsafe truecolor sequence: {out:?}");
    }

    #[test]
    fn rgb_background_uses_nearest_xterm_palette_color() {
        assert_eq!(rgb_to_xterm256(30, 30, 46), 234);
        assert_eq!(rgb_to_xterm256(255, 0, 0), 196);
        assert_eq!(rgb_to_xterm256(255, 255, 255), 231);
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
            background: None,
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
            out.contains("ptkos.apps.thaidevops.co") && !out.contains("F1:SUMMARY"),
            "mgmt URL missing: {out:?}"
        );
    }

    #[test]
    fn frame_always_paints_something() {
        let out = render_demo(80, 24, theme::ASCII);
        assert!(out.contains("pertisk-node-01"));
        assert!(out.contains("NODE") && out.contains("ENDPOINT"));
        assert!(!out.contains("?2026h"), "synchronized update begin present");
    }

    #[test]
    fn cursor_stays_hidden() {
        let out = render_demo(80, 24, theme::ASCII);
        assert!(out.contains("\x1b[?25l"));
        assert!(!out.contains("\x1b[?25h"));
        // Never park past the terminal bounds: terminals clamp that address to
        // the bottom-right, where asynchronous console output causes scrolling.
        assert!(
            out.contains("\x1b[1;1H"),
            "expected stable cursor park: {out:?}"
        );
        assert!(
            !out.contains("\x1b[999;999H"),
            "must not park at the clamped bottom-right edge"
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
        let out = writer
            .encode(terminal.backend().buffer(), true)
            .expect("reconnect must get a full frame");
        assert!(out.contains("pertisk-node-01"));
        assert!(
            out.contains("\x1b[24;80H\x1b[0J"),
            "full repaint must clear stale text below the footer: {out:?}"
        );
    }

    #[test]
    fn changed_log_repaints_rows_by_address() {
        let mut writer = FrameWriter::default();
        let first = draw_demo(80, 24, theme::ASCII, &demo_logs());
        let full = writer.encode(first.backend().buffer(), true).unwrap();
        assert!(full.contains("\x1b[1;1H"), "first row missing: {full:?}");
        assert!(full.contains("\x1b[24;1H"), "last row missing: {full:?}");

        let mut logs = demo_logs();
        logs.push("INFO one more line".into());
        let second = draw_demo(80, 24, theme::ASCII, &logs);
        let out = writer.encode(second.backend().buffer(), true).unwrap();
        assert!(
            !out.contains("\x1b[H"),
            "incremental paint must not home-walk: {out:?}"
        );
        assert!(out.contains("one more line"), "new log missing: {out:?}");
        assert!(
            !out.contains("[ LOGS ]"),
            "incremental log update must not repaint the separator: {out:?}"
        );
        assert!(
            out.contains("\x1b[1;1H"),
            "incremental must use the stable cursor park"
        );
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
        for chrome in [
            theme::ASCII,
            theme::PROPORTIONAL,
            theme::LIGHT,
            theme::DOUBLE,
            theme::ROUNDED,
        ] {
            let rows = demo_rows(160, 26, chrome);
            let out = rows.join("\n");
            for title in ["PERTISK", "KUBERNETES", "NETWORK", "LOGS"] {
                assert!(out.contains(title), "missing panel {title}");
            }
            assert!(
                out.contains("CPU") && out.contains("RAM") && !out.contains("F1:SUMMARY"),
                "header/footer metrics missing: {out}"
            );
        }
    }

    #[test]
    fn unicode_glyphs_degrade_to_ascii() {
        assert_eq!(glyph("─", true), "=");
        assert_eq!(glyph("▄", true), "=");
        assert_eq!(glyph("▀", true), "=");
        assert_eq!(glyph("█", true), "|");
        assert_eq!(glyph("╔", true), "+");
        assert_eq!(glyph("║", true), "|");
        assert_eq!(glyph("─", false), "─");
        assert_eq!(glyph("▄", false), "▄");
    }

    #[test]
    fn every_chrome_uses_one_ascii_hyphen_horizontal_rule() {
        for chrome in [
            theme::ASCII,
            theme::LINE,
            theme::LIGHT,
            theme::ROUNDED,
            theme::HEAVY,
            theme::DOUBLE,
            theme::PROPORTIONAL,
        ] {
            let rows = demo_rows(80, 24, chrome)
                .into_iter()
                .map(|row| strip_escapes(&row))
                .collect::<Vec<_>>();
            let log_rule = rows[7].strip_prefix("[ LOGS ] ").unwrap_or("");
            assert!(
                !log_rule.is_empty() && log_rule.chars().all(|ch| ch == '-'),
                "wrong log separator for {}: {:?}",
                chrome.name,
                rows[7]
            );
            assert!(
                !rows.join("\n").chars().any(|ch| matches!(ch, '─' | '━' | '═' | '▀' | '▄' | '█')),
                "Unicode rule rendered for {}",
                chrome.name
            );
            assert!(!rows[8].starts_with('-'), "double log rule rendered for {}", chrome.name);
        }
    }
}
