//! Console dashboard palette and frame glyphs (feedo-style named themes).
//!
//! Only the 16 base ANSI colors are used. Proxmox Serial / xterm.js renders
//! those reliably, while 256-color and truecolor SGR often arrive mangled.
//!
//! Status accents use **base** ANSI red/yellow/green (SGR 31/33/32), not the
//! bright set (91–93). Proxmox Serial / xterm.js often drops bold+bright
//! combos while still painting plain `kubelet`-style labels — so `absent` /
//! `up` must stay on the simple codes or they look uncolored.
//!
//! Select with `PERTISK_DASHBOARD_THEME=…` (see `by_name`) and
//! `PERTISK_DASHBOARD_BORDER=auto|ascii|light|heavy|double|rounded`.
//!
//! Wild Cherry palette inspired by
//! <https://github.com/lysyi3m/macos-terminal-themes/blob/master/themes/WildCherry.terminal>.

use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub name: &'static str,
    /// Panel frame.
    pub border: Color,
    /// Panel title text.
    pub title: Color,
    /// Field names (`cpu`, `mem`).
    pub label: Color,
    /// Field values (hostname, IPs, sizes).
    pub value: Color,
    /// Log body text.
    pub log: Color,
    pub ok: Color,
    pub warn: Color,
    pub err: Color,
}

/// Shared status accents — base ANSI (30–37). Avoid `Light*` + bold: Serial
/// often paints Magenta labels but strips `\x1b[0;1;91m` status spans.
const STATUS_OK: Color = Color::Green;
const STATUS_WARN: Color = Color::Yellow;
const STATUS_ERR: Color = Color::Red;

/// Dracula-ish: magenta frames, cyan titles.
pub const DRACULA: Theme = Theme {
    name: "dracula",
    border: Color::Magenta,
    title: Color::LightCyan,
    label: Color::DarkGray,
    value: Color::White,
    log: Color::Gray,
    ok: STATUS_OK,
    warn: STATUS_WARN,
    err: STATUS_ERR,
};

/// Nord: cool blue frames.
pub const NORD: Theme = Theme {
    name: "nord",
    border: Color::Blue,
    title: Color::LightBlue,
    label: Color::DarkGray,
    value: Color::White,
    log: Color::Gray,
    ok: STATUS_OK,
    warn: STATUS_WARN,
    err: STATUS_ERR,
};

/// Gruvbox: warm yellow frames.
pub const GRUVBOX: Theme = Theme {
    name: "gruvbox",
    border: Color::Yellow,
    title: Color::LightYellow,
    label: Color::DarkGray,
    value: Color::White,
    log: Color::Gray,
    ok: STATUS_OK,
    warn: STATUS_WARN,
    err: STATUS_ERR,
};

/// Wild Cherry — pink/magenta chrome, cyan text (macOS Terminal theme).
///
/// ANSI mapping from the published `.terminal` profile: cherry cursor/magenta
/// accents, light-cyan body text, dark purple background (terminal-side).
pub const WILD_CHERRY: Theme = Theme {
    name: "wild-cherry",
    border: Color::LightMagenta,
    title: Color::LightCyan,
    label: Color::Magenta,
    value: Color::White,
    log: Color::LightCyan,
    ok: STATUS_OK,
    warn: STATUS_WARN,
    err: STATUS_ERR,
};

/// Tokyo Night — deep blue frames, cyan titles.
pub const TOKYO_NIGHT: Theme = Theme {
    name: "tokyo-night",
    border: Color::LightBlue,
    title: Color::Cyan,
    label: Color::DarkGray,
    value: Color::White,
    log: Color::Gray,
    ok: STATUS_OK,
    warn: STATUS_WARN,
    err: STATUS_ERR,
};

/// Catppuccin Mocha — mauve frames, soft cyan titles.
pub const CATPPUCCIN: Theme = Theme {
    name: "catppuccin",
    border: Color::Magenta,
    title: Color::LightCyan,
    label: Color::DarkGray,
    value: Color::White,
    log: Color::Gray,
    ok: STATUS_OK,
    warn: STATUS_WARN,
    err: STATUS_ERR,
};

/// Solarized Dark — base blue frames, cyan titles.
pub const SOLARIZED: Theme = Theme {
    name: "solarized",
    border: Color::Blue,
    title: Color::Cyan,
    label: Color::DarkGray,
    value: Color::LightCyan,
    log: Color::Gray,
    ok: STATUS_OK,
    warn: STATUS_WARN,
    err: STATUS_ERR,
};

/// Cyberpunk — hot magenta frames, yellow titles.
pub const CYBERPUNK: Theme = Theme {
    name: "cyberpunk",
    border: Color::LightMagenta,
    title: Color::LightYellow,
    label: Color::Magenta,
    value: Color::White,
    log: Color::Gray,
    ok: STATUS_OK,
    warn: STATUS_WARN,
    err: STATUS_ERR,
};

/// No chrome color except status — safest on stubborn serial consoles.
pub const MONO: Theme = Theme {
    name: "mono",
    border: Color::Reset,
    title: Color::Reset,
    label: Color::Reset,
    value: Color::Reset,
    log: Color::Reset,
    ok: STATUS_OK,
    warn: STATUS_WARN,
    err: STATUS_ERR,
};

/// Every named palette (for docs + tests).
pub const ALL: &[Theme] = &[
    DRACULA,
    NORD,
    GRUVBOX,
    WILD_CHERRY,
    TOKYO_NIGHT,
    CATPPUCCIN,
    SOLARIZED,
    CYBERPUNK,
    MONO,
];

pub fn by_name(name: &str) -> Theme {
    match name.trim().to_ascii_lowercase().replace('_', "-").as_str() {
        "nord" => NORD,
        "gruvbox" => GRUVBOX,
        "wild-cherry" | "wildcherry" | "cherry" => WILD_CHERRY,
        "tokyo-night" | "tokyonight" | "tokyo" => TOKYO_NIGHT,
        "catppuccin" | "catppuccin-mocha" | "mocha" => CATPPUCCIN,
        "solarized" | "solarized-dark" => SOLARIZED,
        "cyberpunk" | "cyber" => CYBERPUNK,
        "mono" | "none" | "plain" => MONO,
        _ => CATPPUCCIN,
    }
}

/// Theme from `PERTISK_DASHBOARD_THEME`, else Catppuccin (install default).
pub fn active() -> Theme {
    std::env::var("PERTISK_DASHBOARD_THEME")
        .ok()
        .map(|n| by_name(&n))
        .unwrap_or(CATPPUCCIN)
}

impl Theme {
    pub fn border_style(&self) -> Style {
        Style::default().fg(self.border)
    }

    pub fn title_style(&self) -> Style {
        Style::default().fg(self.title).add_modifier(Modifier::BOLD)
    }

    pub fn label_style(&self) -> Style {
        Style::default().fg(self.label)
    }

    pub fn value_style(&self) -> Style {
        Style::default().fg(self.value)
    }

    pub fn log_style(&self) -> Style {
        Style::default().fg(self.log)
    }

    pub fn ok_style(&self) -> Style {
        // No BOLD — keeps SGR as `\x1b[0;32m` (same shape as label colors).
        Style::default().fg(self.ok)
    }

    pub fn warn_style(&self) -> Style {
        Style::default().fg(self.warn)
    }

    pub fn err_style(&self) -> Style {
        Style::default().fg(self.err)
    }

    pub fn ready_style(&self, ready: bool) -> Style {
        if ready {
            self.ok_style()
        } else {
            self.warn_style()
        }
    }

    /// `up`/`ready` → green, `fail`/`down`/`absent` → red, otherwise amber.
    pub fn status_style(&self, status: &str) -> Style {
        match classify(status) {
            Health::Up => self.ok_style(),
            Health::Down => self.err_style(),
            Health::Unknown => self.warn_style(),
        }
    }

    /// Meter fill reddens as utilization climbs.
    pub fn meter_style(&self, percent: u16) -> Style {
        if percent >= 90 {
            Style::default().fg(self.err)
        } else if percent >= 70 {
            Style::default().fg(self.warn)
        } else {
            Style::default().fg(self.ok)
        }
    }

    pub fn meter_track_style(&self) -> Style {
        Style::default().fg(self.label)
    }

    /// Color a log line by severity keyword.
    pub fn log_line_style(&self, line: &str) -> Style {
        let lower = line.to_ascii_lowercase();
        if lower.contains("error") || lower.contains("fail") || lower.contains("panic") {
            Style::default().fg(self.err)
        } else if lower.contains("warn") {
            Style::default().fg(self.warn)
        } else {
            self.log_style()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Up,
    Down,
    Unknown,
}

pub fn classify(status: &str) -> Health {
    let s = status.trim().to_ascii_lowercase();
    if matches!(s.as_str(), "up" | "ready" | "ok" | "running" | "active") {
        Health::Up
    } else if s.contains("fail")
        || s.contains("error")
        || s.contains("crash")
        || matches!(s.as_str(), "down" | "dead" | "stopped" | "absent")
    {
        Health::Down
    } else {
        Health::Unknown
    }
}

/// Frame glyph set.
///
/// ASCII `-` is a short centered dash, so a run of them reads as `- - - -`
/// rather than a solid rule. Box-drawing glyphs are designed to join, but
/// they are three bytes each: on a console that is not decoding UTF-8 they
/// become mojibake and shift every column after them. Hence `auto`, which
/// picks based on the startup probe rather than on a guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chrome {
    pub name: &'static str,
    pub set: border::Set,
    /// Every glyph is single-byte ASCII.
    pub ascii_only: bool,
    pub meter_fill: &'static str,
    pub meter_track: &'static str,
}

pub const ASCII: Chrome = Chrome {
    name: "ascii",
    set: border::Set {
        top_left: "+",
        top_right: "+",
        bottom_left: "+",
        bottom_right: "+",
        vertical_left: "|",
        vertical_right: "|",
        horizontal_top: "-",
        horizontal_bottom: "-",
    },
    ascii_only: true,
    meter_fill: "|",
    meter_track: "-",
};

const UNICODE_METER_FILL: &str = "█";
const UNICODE_METER_TRACK: &str = "░";

pub const LIGHT: Chrome = Chrome {
    name: "light",
    set: border::PLAIN,
    ascii_only: false,
    meter_fill: UNICODE_METER_FILL,
    meter_track: UNICODE_METER_TRACK,
};

pub const ROUNDED: Chrome = Chrome {
    name: "rounded",
    set: border::ROUNDED,
    ascii_only: false,
    meter_fill: UNICODE_METER_FILL,
    meter_track: UNICODE_METER_TRACK,
};

pub const HEAVY: Chrome = Chrome {
    name: "heavy",
    set: border::THICK,
    ascii_only: false,
    meter_fill: UNICODE_METER_FILL,
    meter_track: UNICODE_METER_TRACK,
};

pub const DOUBLE: Chrome = Chrome {
    name: "double",
    set: border::DOUBLE,
    ascii_only: false,
    meter_fill: UNICODE_METER_FILL,
    meter_track: UNICODE_METER_TRACK,
};

/// Frame glyphs from `PERTISK_DASHBOARD_BORDER`.
///
/// `auto` follows the UTF-8 probe. Explicit styles (`light`, `double`, …) keep
/// their *shape* even when the probe says the console is not UTF-8 — they fall
/// back to an ASCII stand-in (`=` rules for double, etc.) instead of emitting
/// multi-byte glyphs that shift every column on Serial.
pub fn chrome(utf8: bool) -> Chrome {
    let requested = std::env::var("PERTISK_DASHBOARD_BORDER").unwrap_or_default();
    let force_utf8 = matches!(
        std::env::var("PERTISK_DASHBOARD_UTF8").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    );
    let force_ascii = matches!(
        std::env::var("PERTISK_DASHBOARD_UTF8").as_deref(),
        Ok("0") | Ok("false") | Ok("no")
    );
    let use_unicode = if force_ascii {
        false
    } else {
        utf8 || force_utf8
    };
    match requested.trim().to_ascii_lowercase().as_str() {
        "ascii" | "plain" => ASCII,
        "light" | "unicode" => {
            if use_unicode {
                LIGHT
            } else {
                ASCII
            }
        }
        "rounded" => {
            if use_unicode {
                ROUNDED
            } else {
                ASCII
            }
        }
        "heavy" | "thick" => {
            if use_unicode {
                HEAVY
            } else {
                ASCII_HEAVY
            }
        }
        "double" => {
            if use_unicode {
                DOUBLE
            } else {
                ASCII_DOUBLE
            }
        }
        // auto / empty / unknown → rounded (install default) when UTF-8-safe
        _ if use_unicode => ROUNDED,
        _ => ASCII,
    }
}

/// Solid-looking ASCII stand-in for `border: double` when UTF-8 is unavailable.
pub const ASCII_DOUBLE: Chrome = Chrome {
    name: "double-ascii",
    set: border::Set {
        top_left: "+",
        top_right: "+",
        bottom_left: "+",
        bottom_right: "+",
        vertical_left: "|",
        vertical_right: "|",
        horizontal_top: "=",
        horizontal_bottom: "=",
    },
    ascii_only: true,
    meter_fill: "#",
    meter_track: "-",
};

/// Thicker ASCII stand-in for `border: heavy`.
pub const ASCII_HEAVY: Chrome = Chrome {
    name: "heavy-ascii",
    set: border::Set {
        top_left: "+",
        top_right: "+",
        bottom_left: "+",
        bottom_right: "+",
        vertical_left: "#",
        vertical_right: "#",
        horizontal_top: "=",
        horizontal_bottom: "=",
    },
    ascii_only: true,
    meter_fill: "#",
    meter_track: "-",
};

/// ANSI SGR foreground code, or `None` for `Color::Reset`.
pub fn ansi_fg(color: Color) -> Option<u8> {
    match color {
        Color::Black => Some(30),
        Color::Red => Some(31),
        Color::Green => Some(32),
        Color::Yellow => Some(33),
        Color::Blue => Some(34),
        Color::Magenta => Some(35),
        Color::Cyan => Some(36),
        Color::Gray => Some(37),
        Color::DarkGray => Some(90),
        Color::LightRed => Some(91),
        Color::LightGreen => Some(92),
        Color::LightYellow => Some(93),
        Color::LightBlue => Some(94),
        Color::LightMagenta => Some(95),
        Color::LightCyan => Some(96),
        Color::White => Some(97),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_border_env(border: &str, utf8_env: Option<&str>, f: impl FnOnce()) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::remove_var("PERTISK_DASHBOARD_BORDER");
            std::env::remove_var("PERTISK_DASHBOARD_UTF8");
            std::env::set_var("PERTISK_DASHBOARD_BORDER", border);
            if let Some(v) = utf8_env {
                std::env::set_var("PERTISK_DASHBOARD_UTF8", v);
            }
        }
        f();
        unsafe {
            std::env::remove_var("PERTISK_DASHBOARD_BORDER");
            std::env::remove_var("PERTISK_DASHBOARD_UTF8");
        }
    }

    #[test]
    fn status_up_is_green_fail_is_red() {
        let t = DRACULA;
        assert_eq!(t.status_style("up").fg, Some(Color::Green));
        assert_eq!(t.status_style("ready").fg, Some(Color::Green));
        assert_eq!(t.status_style("absent").fg, Some(Color::Red));
        assert_eq!(t.status_style("CrashLoopBackOff").fg, Some(Color::Red));
        assert_eq!(t.status_style("starting").fg, Some(Color::Yellow));
        // No bold — Serial-safe SGR shape matches labels.
        assert!(!t.status_style("absent").add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn every_theme_uses_base_status_colors() {
        for t in ALL {
            assert_eq!(t.ok, Color::Green, "{} ok", t.name);
            assert_eq!(t.warn, Color::Yellow, "{} warn", t.name);
            assert_eq!(t.err, Color::Red, "{} err", t.name);
            assert_ne!(t.ok, t.warn, "{} ok==warn", t.name);
            assert_ne!(t.ok, t.err, "{} ok==err", t.name);
            assert_ne!(t.warn, t.err, "{} warn==err", t.name);
            // Base SGR 32/33/31 (mono chrome may be Reset, status must not).
            assert_eq!(ansi_fg(t.ok), Some(32), "{} ok SGR", t.name);
            assert_eq!(ansi_fg(t.warn), Some(33), "{} warn SGR", t.name);
            assert_eq!(ansi_fg(t.err), Some(31), "{} err SGR", t.name);
        }
    }

    #[test]
    fn unknown_theme_falls_back_to_catppuccin() {
        assert_eq!(by_name("nope").name, "catppuccin");
        assert_eq!(by_name("NORD").name, "nord");
        assert_eq!(by_name("mono").name, "mono");
        assert_eq!(by_name("wild-cherry").name, "wild-cherry");
        assert_eq!(by_name("WildCherry").name, "wild-cherry");
        assert_eq!(by_name("tokyo_night").name, "tokyo-night");
        assert_eq!(by_name("catppuccin").name, "catppuccin");
        assert_eq!(by_name("cyberpunk").name, "cyberpunk");
        assert_eq!(by_name("solarized").name, "solarized");
    }

    #[test]
    fn mono_theme_has_no_chrome_color() {
        assert_eq!(ansi_fg(MONO.border), None);
        assert_eq!(ansi_fg(MONO.ok), Some(32));
    }

    #[test]
    fn auto_chrome_follows_utf8_probe() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::remove_var("PERTISK_DASHBOARD_BORDER");
            std::env::remove_var("PERTISK_DASHBOARD_UTF8");
        }
        assert_eq!(chrome(true).name, "rounded");
        assert_eq!(chrome(false).name, "ascii");
    }

    #[test]
    fn explicit_double_keeps_shape_without_utf8() {
        with_border_env("double", None, || {
            let c = chrome(false);
            assert_eq!(c.name, "double-ascii");
            assert!(c.ascii_only);
            assert_eq!(c.set.horizontal_top, "=");
        });
    }

    #[test]
    fn explicit_double_uses_unicode_when_utf8() {
        with_border_env("double", None, || {
            let c = chrome(true);
            assert_eq!(c.name, "double");
            assert!(!c.ascii_only);
        });
    }

    #[test]
    fn utf8_env_forces_unicode_double() {
        with_border_env("double", Some("1"), || {
            let c = chrome(false);
            assert_eq!(c.name, "double");
            assert!(!c.ascii_only);
        });
    }

    #[test]
    fn ascii_chrome_glyphs_are_single_byte() {
        let s = ASCII.set;
        for glyph in [
            s.top_left,
            s.top_right,
            s.bottom_left,
            s.bottom_right,
            s.vertical_left,
            s.vertical_right,
            s.horizontal_top,
            s.horizontal_bottom,
        ] {
            assert_eq!(glyph.len(), 1, "{glyph:?} is not one byte");
        }
    }
}
