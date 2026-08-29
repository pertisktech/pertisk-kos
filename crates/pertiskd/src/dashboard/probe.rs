//! One-shot console size for Proxmox Serial and ESXi / VGA.
//!
//! Interactive CSI probes (`18 t`, cursor-extent, UTF-8 glyph) often leave
//! xterm.js in a cleared/broken state, so they are **off by default**. Size
//! comes from:
//! 1. `PERTISK_DASHBOARD_COLS` / `_ROWS` (or `COLUMNS` / `LINES`)
//! 2. Optional live probe when `PERTISK_DASHBOARD_PROBE=1`
//! 3. Winsize of the **paint target** (`pertisk.dashboard.console` / ttyS0 /
//!    ttyAMA0 / console) — never VGA `tty0` (taller framebuffer scrolls Serial
//!    so logs appear at the top on first paint)
//! 4. Safe **80×24** fallback
//!
//! Refresh follows the same paint-target path so a later VGA ioctl cannot grow
//! the Serial frame past the xterm.js pane.

use std::os::unix::io::RawFd;
use std::time::{Duration, Instant};

pub const FALLBACK_COLS: u16 = pertisk_config::Dashboard::DEFAULT_COLS;
pub const FALLBACK_ROWS: u16 = pertisk_config::Dashboard::DEFAULT_ROWS;
const MIN_COLS: u16 = 40;
const MIN_ROWS: u16 = 12;
/// Reject probe results above this — oversized frames wrap and look blank.
/// Raised for Host Client / fbcon (was 160×50; that still looked ~¼-screen).
const SAFE_MAX_COLS: u16 = 240;
const SAFE_MAX_ROWS: u16 = 80;
const MAX_COLS: u16 = 300;
const MAX_ROWS: u16 = 120;
const REPLY_TIMEOUT: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConsoleCaps {
    pub cols: u16,
    pub rows: u16,
    /// Terminal decoded a 3-byte glyph as one cell — box drawing is safe.
    pub utf8: bool,
    /// Where `cols`/`rows` came from, for the startup log line.
    pub source: &'static str,
}

impl Default for ConsoleCaps {
    fn default() -> Self {
        Self {
            cols: FALLBACK_COLS,
            rows: FALLBACK_ROWS,
            utf8: false,
            source: "default",
        }
    }
}

fn probe_enabled() -> bool {
    matches!(
        std::env::var("PERTISK_DASHBOARD_PROBE").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

pub fn detect() -> ConsoleCaps {
    let mut caps = ConsoleCaps::default();

    // Operator pin wins immediately — skip interactive queries entirely.
    let env_cols = env_u16("PERTISK_DASHBOARD_COLS").or_else(|| env_u16("COLUMNS"));
    let env_rows = env_u16("PERTISK_DASHBOARD_ROWS").or_else(|| env_u16("LINES"));
    if env_cols.is_some() || env_rows.is_some() {
        if let Some(c) = env_cols {
            caps.cols = c;
        }
        if let Some(r) = env_rows {
            caps.rows = r;
        }
        caps.source = "env";
        caps.utf8 = utf8_from_env(false);
        // Never paint wider/taller than the live pane — oversized frames wrap
        // and look blank on Proxmox Serial.
        clamp_to_tty(&mut caps);
        let (cols, rows) = clamp_measured(caps.cols, caps.rows);
        caps.cols = cols;
        caps.rows = rows;
        clamp_to_paint_target(&mut caps);
        return caps;
    }

    let tty = open_tty();
    if probe_enabled() {
        if let Some(fd) = tty.as_ref().map(Tty::fd) {
            if let Some(mut raw) = RawMode::enter(fd) {
                raw.drain();
                if let Some((rows, cols)) = raw.query_text_area() {
                    if cols <= SAFE_MAX_COLS && rows <= SAFE_MAX_ROWS {
                        caps.rows = rows;
                        caps.cols = cols;
                        caps.source = "csi-18t";
                    }
                }
                if caps.source == "default" {
                    raw.drain();
                    if let Some((rows, cols)) = raw.query_cursor_extent() {
                        if cols <= SAFE_MAX_COLS && rows <= SAFE_MAX_ROWS {
                            caps.rows = rows;
                            caps.cols = cols;
                            caps.source = "cursor-extent";
                        }
                    }
                }
                raw.drain();
                caps.utf8 = raw.query_utf8().unwrap_or(false);
            }
        }
    }

    if caps.source == "default" {
        // Prefer the tty we actually paint to (Serial / dashboard.console).
        // Never adopt VGA/`tty0` via best_winsize — its framebuffer is often
        // much taller than Proxmox xterm.js, and the first oversized CUP scroll
        // leaves the logs panel stuck at the top of the viewport until resize.
        if let Some((rows, cols, src)) = paint_target_winsize() {
            caps.rows = rows;
            caps.cols = cols;
            caps.source = src;
        }
        // else keep FALLBACK 80×24 — safer than guessing VGA geometry.
    }

    caps.utf8 = utf8_from_env(caps.utf8);

    let (cols, rows) = if caps.source == "default" {
        (
            caps.cols.clamp(MIN_COLS, SAFE_MAX_COLS),
            caps.rows.clamp(MIN_ROWS, SAFE_MAX_ROWS),
        )
    } else {
        clamp_measured(caps.cols, caps.rows)
    };
    caps.cols = cols;
    caps.rows = rows;
    // Belt-and-suspenders: never exceed the live Serial pane.
    clamp_to_paint_target(&mut caps);
    caps
}

/// Cheap re-read for the TUI loop: env pins, else refresh the paint-target
/// winsize (Serial / dashboard.console). Do **not** grow to VGA/`tty0` — that
/// reintroduces the first-paint scroll bug on Proxmox Serial.
pub fn detect_refresh(previous: ConsoleCaps) -> ConsoleCaps {
    let env_cols = env_u16("PERTISK_DASHBOARD_COLS").or_else(|| env_u16("COLUMNS"));
    let env_rows = env_u16("PERTISK_DASHBOARD_ROWS").or_else(|| env_u16("LINES"));
    if env_cols.is_some() || env_rows.is_some() {
        let mut caps = previous;
        if let Some(c) = env_cols {
            caps.cols = c;
        }
        if let Some(r) = env_rows {
            caps.rows = r;
        }
        caps.source = "env";
        caps.utf8 = utf8_from_env(caps.utf8);
        clamp_to_tty_paint_target(&mut caps);
        let (cols, rows) = clamp_measured(caps.cols, caps.rows);
        caps.cols = cols;
        caps.rows = rows;
        clamp_to_paint_target(&mut caps);
        return caps;
    }

    let mut caps = previous;
    caps.utf8 = utf8_from_env(caps.utf8);
    if let Some((rows, cols, src)) = paint_target_winsize() {
        if cols != caps.cols || rows != caps.rows {
            caps.cols = cols;
            caps.rows = rows;
            caps.source = src;
        }
    }
    let (cols, rows) = clamp_measured(caps.cols, caps.rows);
    caps.cols = cols;
    caps.rows = rows;
    clamp_to_paint_target(&mut caps);
    caps
}

fn utf8_from_env(fallback: bool) -> bool {
    match std::env::var("PERTISK_DASHBOARD_UTF8").ok().as_deref() {
        Some("1" | "true" | "yes") => true,
        Some("0" | "false" | "no") => false,
        _ => fallback,
    }
}

fn clamp_to_tty_paint_target(caps: &mut ConsoleCaps) {
    let Some((rows, cols, _)) = paint_target_winsize() else {
        return;
    };
    if caps.cols > cols {
        caps.cols = cols;
    }
    if caps.rows > rows {
        caps.rows = rows;
    }
}

fn clamp_to_tty(caps: &mut ConsoleCaps) {
    // Env pins: shrink to paint target when known, else any sane tty (legacy).
    if paint_target_winsize().is_some() {
        clamp_to_tty_paint_target(caps);
        return;
    }
    let Some((rows, cols, _)) = best_winsize() else {
        return;
    };
    if caps.cols > cols {
        caps.cols = cols;
    }
    if caps.rows > rows {
        caps.rows = rows;
    }
}

/// Winsize of the tty we actually paint to (Serial / dashboard.console).
///
/// **Never** fall through to `/dev/console` or VGA `tty0`. On Proxmox those
/// often report a tall framebuffer; when Serial winsize is still 0×0 we used
/// to pick console, paint a 50-row frame, CUP past the xterm.js pane, and the
/// **logs panel scrolled into the visible top** until a later resize.
///
/// If the serial device exists but has no sane winsize yet → 80×24 fallback.
fn paint_target_winsize() -> Option<(u16, u16, &'static str)> {
    let mut paths: Vec<(std::path::PathBuf, &'static str, bool)> = Vec::new();
    if let Some(p) = crate::cmdline::dashboard_console_path() {
        let is_vga = p.ends_with("tty0") || p.ends_with("tty1") || p.ends_with("tty2");
        paths.push((p, "ioctl-dashboard", !is_vga));
    }
    #[cfg(target_arch = "aarch64")]
    {
        paths.push((
            std::path::PathBuf::from("/dev/ttyAMA0"),
            "ioctl-ttyAMA0",
            true,
        ));
    }
    paths.push((std::path::PathBuf::from("/dev/ttyS0"), "ioctl-ttyS0", true));
    // Deliberately omit /dev/console and /dev/tty0.

    for (path, src, serial_like) in paths {
        let Ok(file) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
        else {
            continue;
        };
        use std::os::unix::io::AsRawFd;
        let measured = winsize(file.as_raw_fd());
        match measured {
            Some((rows, cols))
                if cols >= MIN_COLS
                    && rows >= MIN_ROWS
                    && cols <= SAFE_MAX_COLS
                    && rows <= SAFE_MAX_ROWS =>
            {
                // Some hosts report a huge winsize on ttyS0 (copy of VGA). Cap
                // serial-like devices to a pane that fits Proxmox Serial.
                if serial_like && (rows > FALLBACK_ROWS * 2 || cols > FALLBACK_COLS * 2) {
                    return Some((FALLBACK_ROWS, FALLBACK_COLS, "fallback-serial-cap"));
                }
                return Some((rows, cols, src));
            }
            _ if serial_like => {
                // Device is there but winsize unset/junk — do not consult VGA.
                return Some((FALLBACK_ROWS, FALLBACK_COLS, "fallback-serial"));
            }
            _ => continue,
        }
    }
    None
}

/// Shrink caps so the TUI never exceeds the Serial / dashboard console pane.
fn clamp_to_paint_target(caps: &mut ConsoleCaps) {
    apply_paint_target_clamp(caps, paint_target_winsize());
}

fn apply_paint_target_clamp(caps: &mut ConsoleCaps, target: Option<(u16, u16, &'static str)>) {
    let Some((rows, cols, src)) = target else {
        return;
    };
    let mut clamped = false;
    if caps.rows > rows {
        caps.rows = rows;
        clamped = true;
    }
    if caps.cols > cols {
        caps.cols = cols;
        clamped = true;
    }
    if clamped {
        caps.source = src;
    }
}

fn clamp_measured(cols: u16, rows: u16) -> (u16, u16) {
    (
        cols.clamp(10, MAX_COLS.min(SAFE_MAX_COLS)),
        rows.clamp(8, MAX_ROWS.min(SAFE_MAX_ROWS)),
    )
}

/// Largest sane winsize among VGA / console / serial.
///
/// Returns `(rows, cols, source)`.
fn best_winsize() -> Option<(u16, u16, &'static str)> {
    let candidates: &[(&str, &'static str)] = &[
        ("/dev/tty0", "ioctl-tty0"),
        ("/dev/console", "ioctl-console"),
        ("/dev/ttyS0", "ioctl-ttyS0"),
    ];
    let mut best: Option<(u16, u16, u32, &'static str)> = None;
    for &(path, src) in candidates {
        let Ok(file) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
        else {
            continue;
        };
        use std::os::unix::io::AsRawFd;
        let Some((rows, cols)) = winsize(file.as_raw_fd()) else {
            continue;
        };
        if cols < MIN_COLS || rows < MIN_ROWS || cols > SAFE_MAX_COLS || rows > SAFE_MAX_ROWS {
            continue;
        }
        let area = u32::from(cols) * u32::from(rows);
        let replace = match best {
            None => true,
            Some((_, _, a, _)) => area > a,
        };
        if replace {
            best = Some((rows, cols, area, src));
        }
    }
    best.map(|(rows, cols, _, src)| (rows, cols, src))
}

/// Re-query the pane size while the dashboard is running.
#[allow(dead_code)]
pub fn detect_size() -> Option<(u16, u16)> {
    if std::env::var_os("PERTISK_DASHBOARD_COLS").is_some()
        || std::env::var_os("PERTISK_DASHBOARD_ROWS").is_some()
        || !probe_enabled()
    {
        return None;
    }
    let tty = open_tty()?;
    let mut raw = RawMode::enter(tty.fd())?;
    raw.drain();
    let size = raw
        .query_text_area()
        .or_else(|| raw.query_cursor_extent())?;
    let (rows, cols) = size;
    if cols > SAFE_MAX_COLS || rows > SAFE_MAX_ROWS {
        return None;
    }
    let (cols, rows) = clamp_measured(cols, rows);
    Some((rows, cols))
}

fn env_u16(key: &str) -> Option<u16> {
    std::env::var(key).ok()?.trim().parse().ok()
}

enum Tty {
    Stdin,
    Owned(std::fs::File),
}

impl Tty {
    fn fd(&self) -> RawFd {
        use std::os::unix::io::AsRawFd;
        match self {
            Tty::Stdin => 0,
            Tty::Owned(file) => file.as_raw_fd(),
        }
    }
}

fn open_tty() -> Option<Tty> {
    if unsafe { libc::isatty(0) } == 1 {
        return Some(Tty::Stdin);
    }
    for path in ["/dev/ttyS0", "/dev/console"] {
        if let Ok(file) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
        {
            return Some(Tty::Owned(file));
        }
    }
    None
}

fn winsize(fd: RawFd) -> Option<(u16, u16)> {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) } != 0 {
        return None;
    }
    (ws.ws_col > 0 && ws.ws_row > 0).then_some((ws.ws_row, ws.ws_col))
}

#[allow(dead_code)]
fn set_winsize(fd: RawFd, rows: u16, cols: u16) {
    let ws = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &ws) };
}

struct RawMode {
    fd: RawFd,
    saved: libc::termios,
}

impl RawMode {
    fn enter(fd: RawFd) -> Option<Self> {
        let mut saved: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(fd, &mut saved) } != 0 {
            return None;
        }
        let mut raw = saved;
        raw.c_lflag &= !(libc::ICANON | libc::ECHO);
        raw.c_cc[libc::VMIN] = 0;
        raw.c_cc[libc::VTIME] = 0;
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
            return None;
        }
        Some(Self { fd, saved })
    }

    fn write(&self, bytes: &[u8]) -> Option<()> {
        let n = unsafe { libc::write(self.fd, bytes.as_ptr().cast(), bytes.len()) };
        (n == bytes.len() as isize).then_some(())
    }

    fn drain(&mut self) {
        let mut sink = [0u8; 64];
        while self.poll_ready(0) {
            let n = unsafe { libc::read(self.fd, sink.as_mut_ptr().cast(), sink.len()) };
            if n <= 0 {
                break;
            }
        }
    }

    fn poll_ready(&self, timeout_ms: i32) -> bool {
        let mut pfd = libc::pollfd {
            fd: self.fd,
            events: libc::POLLIN,
            revents: 0,
        };
        unsafe { libc::poll(&mut pfd, 1, timeout_ms) > 0 }
    }

    fn read_until(&mut self, terminator: u8) -> Option<Vec<u8>> {
        let deadline = Instant::now() + REPLY_TIMEOUT;
        let mut buf = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || !self.poll_ready(left.as_millis() as i32) {
                return None;
            }
            let mut chunk = [0u8; 64];
            let n = unsafe { libc::read(self.fd, chunk.as_mut_ptr().cast(), chunk.len()) };
            if n <= 0 {
                return None;
            }
            buf.extend_from_slice(&chunk[..n as usize]);
            if buf.contains(&terminator) {
                return Some(buf);
            }
            if buf.len() > 256 {
                return None;
            }
        }
    }

    fn query_text_area(&mut self) -> Option<(u16, u16)> {
        self.write(b"\x1b[18t")?;
        parse_text_area(&self.read_until(b't')?)
    }

    fn query_cursor_extent(&mut self) -> Option<(u16, u16)> {
        self.write(b"\x1b7\x1b[9999;9999H\x1b[6n")?;
        let reply = self.read_until(b'R');
        let _ = self.write(b"\x1b8");
        let (rows, cols) = parse_cursor_pos(&reply?)?;
        (rows > 1 && cols > 1).then_some((rows, cols))
    }

    fn query_utf8(&mut self) -> Option<bool> {
        self.write("\x1b[H\x1b[2K\u{2500}\x1b[6n".as_bytes())?;
        let col = parse_cursor_col(&self.read_until(b'R')?)?;
        Some(col == 2)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &self.saved) };
    }
}

fn parse_text_area(buf: &[u8]) -> Option<(u16, u16)> {
    let s = std::str::from_utf8(buf).ok()?;
    let start = s.rfind("\x1b[8;").or_else(|| s.rfind("[8;"))?;
    let body = &s[start..];
    let body = body
        .strip_prefix("\x1b[8;")
        .or_else(|| body.strip_prefix("[8;"))?;
    let body = body.strip_suffix('t')?;
    let mut parts = body.split(';');
    let rows: u16 = parts.next()?.parse().ok()?;
    let cols: u16 = parts.next()?.parse().ok()?;
    (rows > 0 && cols > 0).then_some((rows, cols))
}

fn parse_cursor_pos(buf: &[u8]) -> Option<(u16, u16)> {
    let s = std::str::from_utf8(buf).ok()?;
    let start = s.rfind("\x1b[").or_else(|| s.rfind('['))?;
    let body = &s[start..];
    let body = body
        .strip_prefix("\x1b[")
        .or_else(|| body.strip_prefix('['))?;
    let body = body.strip_suffix('R')?;
    let mut parts = body.split(';');
    let rows: u16 = parts.next()?.parse().ok()?;
    let cols: u16 = parts.next()?.parse().ok()?;
    (rows > 0 && cols > 0).then_some((rows, cols))
}

fn parse_cursor_col(buf: &[u8]) -> Option<u16> {
    parse_cursor_pos(buf).map(|(_, cols)| cols)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_area_reply() {
        assert_eq!(parse_text_area(b"\x1b[8;40;120t"), Some((40, 120)));
    }

    #[test]
    fn ignores_noise_before_text_area_reply() {
        assert_eq!(parse_text_area(b"junk\x1b[8;25;80t"), Some((25, 80)));
    }

    #[test]
    fn rejects_malformed_text_area_reply() {
        assert_eq!(parse_text_area(b"\x1b[8;x;y t"), None);
    }

    #[test]
    fn parses_cursor_extent_reply() {
        assert_eq!(parse_cursor_pos(b"\x1b[24;80R"), Some((24, 80)));
    }

    #[test]
    fn rejects_malformed_cursor_reply() {
        assert_eq!(parse_cursor_pos(b"\x1b[24R"), None);
    }

    #[test]
    fn utf8_probe_reads_column_two() {
        assert_eq!(parse_cursor_col(b"\x1b[1;2R"), Some(2));
        assert_eq!(parse_cursor_col(b"\x1b[1;4R"), Some(4));
    }

    #[test]
    fn measured_size_is_not_inflated() {
        assert_eq!(clamp_measured(50, 20), (50, 20));
        assert_eq!(clamp_measured(5, 5), (10, 8));
    }

    #[test]
    fn paint_target_clamp_shrinks_oversized_vga_frame() {
        let mut caps = ConsoleCaps {
            cols: 200,
            rows: 50,
            utf8: true,
            source: "ioctl-tty0",
        };
        apply_paint_target_clamp(&mut caps, Some((24, 80, "ioctl-ttyS0")));
        assert_eq!(caps.rows, 24);
        assert_eq!(caps.cols, 80);
        assert_eq!(caps.source, "ioctl-ttyS0");
    }

    #[test]
    fn paint_target_clamp_keeps_smaller_frame() {
        let mut caps = ConsoleCaps {
            cols: 40,
            rows: 12,
            utf8: true,
            source: "env",
        };
        apply_paint_target_clamp(&mut caps, Some((24, 80, "ioctl-ttyS0")));
        assert_eq!(caps.rows, 12);
        assert_eq!(caps.cols, 40);
        assert_eq!(caps.source, "env");
    }

    #[test]
    fn serial_fallback_clamp_beats_vga_geometry() {
        let mut caps = ConsoleCaps {
            cols: 200,
            rows: 50,
            utf8: true,
            source: "ioctl-tty0",
        };
        apply_paint_target_clamp(&mut caps, Some((24, 80, "fallback-serial")));
        assert_eq!(caps.rows, 24);
        assert_eq!(caps.cols, 80);
        assert_eq!(caps.source, "fallback-serial");
    }
}
