//! One-shot console capability probe (size + UTF-8) for Proxmox Serial.
//!
//! A serial line carries no `SIGWINCH` and `TIOCGWINSZ` answers with a stale
//! 80x24, so the only source of the real pane size is asking xterm.js with
//! `CSI 18 t`. Probing on every refresh is what made the cursor blink before,
//! so this runs exactly once, before the dashboard takes over the screen.

use std::os::unix::io::RawFd;
use std::time::{Duration, Instant};

pub const FALLBACK_COLS: u16 = 93;
pub const FALLBACK_ROWS: u16 = 25;
/// Soft floor only used when no probe answered — never inflate a real size.
const MIN_COLS: u16 = 40;
const MIN_ROWS: u16 = 12;
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

pub fn detect() -> ConsoleCaps {
    let mut caps = ConsoleCaps::default();

    let tty = open_tty();
    if let Some(fd) = tty.as_ref().map(Tty::fd) {
        if let Some(mut raw) = RawMode::enter(fd) {
            raw.drain();
            // `CSI 18t` is exact and leaves the cursor alone, but xterm.js
            // ships with `windowOptions` disabled and simply will not answer.
            // The cursor-extent trick only needs DSR, which every terminal
            // supports — it is how `resize(1)` has always done this.
            if let Some((rows, cols)) = raw.query_text_area() {
                caps.rows = rows;
                caps.cols = cols;
                caps.source = "csi-18t";
            } else if let Some((rows, cols)) = raw.query_cursor_extent() {
                caps.rows = rows;
                caps.cols = cols;
                caps.source = "cursor-extent";
            }
            caps.utf8 = raw.query_utf8().unwrap_or(false);
        }
        if caps.source == "default" {
            if let Some((rows, cols)) = winsize(fd) {
                caps.rows = rows;
                caps.cols = cols;
                caps.source = "ioctl";
            }
        }
    }

    // Explicit override always wins — the probe can only be wrong.
    if let Some(cols) = env_u16("PERTISK_DASHBOARD_COLS").or_else(|| env_u16("COLUMNS")) {
        caps.cols = cols;
        caps.source = "env";
    }
    if let Some(rows) = env_u16("PERTISK_DASHBOARD_ROWS").or_else(|| env_u16("LINES")) {
        caps.rows = rows;
        caps.source = "env";
    }

    // Never inflate a measured size: drawing 60 columns into a 50-wide pane
    // wraps every row and the right/bottom of the dashboard disappears.
    let (cols, rows) = if caps.source == "default" {
        (caps.cols.clamp(MIN_COLS, MAX_COLS), caps.rows.clamp(MIN_ROWS, MAX_ROWS))
    } else {
        clamp_measured(caps.cols, caps.rows)
    };
    caps.cols = cols;
    caps.rows = rows;

    // Publish the result so child processes and `ip`/`ps` agree with us.
    if let Some(fd) = tty.as_ref().map(Tty::fd) {
        set_winsize(fd, caps.rows, caps.cols);
    }
    caps
}

/// Cap absurd values but do not grow a real pane.
fn clamp_measured(cols: u16, rows: u16) -> (u16, u16) {
    (cols.clamp(10, MAX_COLS), rows.clamp(8, MAX_ROWS))
}

/// Re-query the pane size while the dashboard is running.
///
/// Returns `None` when the size is pinned by the operator, when no terminal
/// answered, or when nothing can be opened. UTF-8 support is not re-probed:
/// it cannot change at runtime, and skipping it halves the round trips.
pub fn detect_size() -> Option<(u16, u16)> {
    if std::env::var_os("PERTISK_DASHBOARD_COLS").is_some()
        || std::env::var_os("PERTISK_DASHBOARD_ROWS").is_some()
    {
        return None;
    }
    let tty = open_tty()?;
    let mut raw = RawMode::enter(tty.fd())?;
    raw.drain();
    // No synchronized-update wrapper — Proxmox xterm.js can hold that buffer
    // forever and leave a blank console.
    let size = raw
        .query_text_area()
        .or_else(|| raw.query_cursor_extent())?;
    let (rows, cols) = size;
    let (cols, rows) = clamp_measured(cols, rows);
    Some((rows, cols))
}

fn env_u16(key: &str) -> Option<u16> {
    std::env::var(key).ok()?.trim().parse().ok()
}

/// The console, either inherited on stdin or opened directly.
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
    // `redirect_stdio_serial` dup2s the O_RDWR ttyS0 handle onto 0/1/2, so
    // stdin is normally both readable and writable here.
    if unsafe { libc::isatty(0) } == 1 {
        return Some(Tty::Stdin);
    }
    for path in ["/dev/ttyS0", "/dev/console"] {
        if let Ok(file) = std::fs::OpenOptions::new().read(true).write(true).open(path) {
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

fn set_winsize(fd: RawFd, rows: u16, cols: u16) {
    let ws = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &ws) };
}

/// Non-canonical, no-echo mode for the duration of the probe.
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

    /// Discard buffered keystrokes so they cannot be parsed as a reply.
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

    /// `CSI 18 t` → `CSI 8 ; rows ; cols t`.
    fn query_text_area(&mut self) -> Option<(u16, u16)> {
        self.write(b"\x1b[18t")?;
        parse_text_area(&self.read_until(b't')?)
    }

    /// Drive the cursor past the far corner and read back where it stopped.
    ///
    /// The terminal clamps the move to its own bounds, so the reported
    /// position *is* the pane size. Needs only DSR (`CSI 6n`), which xterm.js
    /// always answers — unlike `CSI 18t`. The screen is cleared immediately
    /// after the probe, so the cursor excursion is never visible.
    fn query_cursor_extent(&mut self) -> Option<(u16, u16)> {
        self.write(b"\x1b7\x1b[9999;9999H\x1b[6n")?;
        let reply = self.read_until(b'R');
        // Restore the saved cursor even if the read failed.
        self.write(b"\x1b8");
        let (rows, cols) = parse_cursor_pos(&reply?)?;
        (rows > 1 && cols > 1).then_some((rows, cols))
    }

    /// Print one 3-byte glyph at home and see how far the cursor moved.
    ///
    /// Column 2 means the terminal decoded UTF-8 and box drawing will render.
    /// Column 4 means it treated the glyph as three raw bytes — mojibake, and
    /// every column after it would be shifted.
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

pub fn parse_text_area(reply: &[u8]) -> Option<(u16, u16)> {
    let text = String::from_utf8_lossy(reply);
    let start = text.rfind("\u{1b}[8;")? + 4;
    let body = &text[start..];
    let end = body.find('t')?;
    let mut parts = body[..end].split(';');
    let rows = parts.next()?.trim().parse().ok()?;
    let cols = parts.next()?.trim().parse().ok()?;
    Some((rows, cols))
}

/// `CSI row ; col R` → `(row, col)`.
pub fn parse_cursor_pos(reply: &[u8]) -> Option<(u16, u16)> {
    let text = String::from_utf8_lossy(reply);
    let start = text.rfind("\u{1b}[")? + 2;
    let body = &text[start..];
    let end = body.find('R')?;
    let mut parts = body[..end].split(';');
    let row = parts.next()?.trim().parse().ok()?;
    let col = parts.next()?.trim().parse().ok()?;
    Some((row, col))
}

pub fn parse_cursor_col(reply: &[u8]) -> Option<u16> {
    parse_cursor_pos(reply).map(|(_, col)| col)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_area_reply() {
        assert_eq!(parse_text_area(b"\x1b[8;30;120t"), Some((30, 120)));
    }

    #[test]
    fn ignores_noise_before_text_area_reply() {
        assert_eq!(parse_text_area(b"junk\x1b[8;24;80t"), Some((24, 80)));
    }

    #[test]
    fn rejects_malformed_text_area_reply() {
        assert_eq!(parse_text_area(b"\x1b[8;30t"), None);
        assert_eq!(parse_text_area(b"nothing here"), None);
    }

    #[test]
    fn utf8_probe_reads_column_two() {
        assert_eq!(parse_cursor_col(b"\x1b[1;2R"), Some(2));
        // Raw-byte terminal: three cells consumed.
        assert_eq!(parse_cursor_col(b"\x1b[1;4R"), Some(4));
    }

    /// The clamped cursor position after `CSI 9999;9999H` is the pane size.
    #[test]
    fn parses_cursor_extent_reply() {
        assert_eq!(parse_cursor_pos(b"\x1b[30;120R"), Some((30, 120)));
        assert_eq!(parse_cursor_pos(b"\x1b[24;80R"), Some((24, 80)));
    }

    #[test]
    fn rejects_malformed_cursor_reply() {
        assert_eq!(parse_cursor_pos(b"\x1b[30R"), None);
        assert_eq!(parse_cursor_pos(b""), None);
    }

    #[test]
    fn measured_size_is_not_inflated() {
        // A 50-wide pane must stay 50 — bumping to MIN_COLS wraps the frame.
        assert_eq!(clamp_measured(50, 18), (50, 18));
        assert_eq!(clamp_measured(200, 60), (200, 60));
        assert_eq!(clamp_measured(500, 200), (MAX_COLS, MAX_ROWS));
    }
}
