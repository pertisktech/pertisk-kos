//! Kernel cmdline helpers (`pertisk.*` tokens).
//!
//! Dotted names like `pertisk.dashboard.disabled=1` are **not** promoted into
//! init's environment by Linux (only `KEY=value` without `.`). We parse
//! `/proc/cmdline` ourselves and also honor underscore env aliases.

use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

/// Resolved dashboard-related cmdline / env settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DashboardCmdline {
    /// `pertisk.dashboard.disabled=1` (or env alias) — skip console TUI.
    pub disabled: bool,
    /// Device name starting with `tty` (e.g. `ttyS0`), without `/dev/`.
    pub console: Option<String>,
}

static CACHED: OnceLock<DashboardCmdline> = OnceLock::new();

/// Cached resolution for the process lifetime (boot path).
pub fn dashboard_settings() -> &'static DashboardCmdline {
    CACHED.get_or_init(resolve_dashboard_settings)
}

/// Force-resolve (tests / after env mutation). Not used on the hot path.
#[cfg(test)]
#[allow(dead_code)]
pub fn resolve_for_test(cmdline: &str) -> DashboardCmdline {
    resolve_from_cmdline_and_env(cmdline, |k| std::env::var(k).ok())
}

fn resolve_dashboard_settings() -> DashboardCmdline {
    let cmdline = read_proc_cmdline().unwrap_or_default();
    resolve_from_cmdline_and_env(&cmdline, |k| std::env::var(k).ok())
}

fn read_proc_cmdline() -> Option<String> {
    fs::read_to_string("/proc/cmdline").ok()
}

fn resolve_from_cmdline_and_env<F>(cmdline: &str, env: F) -> DashboardCmdline
where
    F: Fn(&str) -> Option<String>,
{
    let mut out = DashboardCmdline::default();
    for (key, val) in parse_kv_tokens(cmdline) {
        match key.as_str() {
            "pertisk.dashboard.disabled" => {
                out.disabled = is_truthy(&val);
            }
            "pertisk.dashboard.console" => {
                if let Some(c) = normalize_console(&val) {
                    out.console = Some(c);
                }
            }
            _ => {}
        }
    }

    // Env aliases win only when cmdline omitted that knob (`pertisk.*` is
    // primary; underscore form is for convenience / lab inject).
    if !cmdline_has(cmdline, "pertisk.dashboard.disabled") {
        if let Some(v) = env("PERTISK_DASHBOARD_DISABLED") {
            out.disabled = is_truthy(&v);
        }
    }
    if !cmdline_has(cmdline, "pertisk.dashboard.console") {
        if let Some(v) = env("PERTISK_DASHBOARD_CONSOLE") {
            if let Some(c) = normalize_console(&v) {
                out.console = Some(c);
            }
        }
    }
    out
}

fn cmdline_has(cmdline: &str, key: &str) -> bool {
    parse_kv_tokens(cmdline).any(|(k, _)| k == key)
}

/// Split whitespace-separated `key=value` tokens (last duplicate wins upstream).
fn parse_kv_tokens(cmdline: &str) -> impl Iterator<Item = (String, String)> + '_ {
    cmdline.split_whitespace().filter_map(|tok| {
        let (k, v) = tok.split_once('=')?;
        if k.is_empty() {
            return None;
        }
        Some((k.to_string(), v.to_string()))
    })
}

fn is_truthy(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Accept `ttyS0`, `/dev/ttyS0`, reject non-tty names (must start with tty).
fn normalize_console(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let name = s.strip_prefix("/dev/").unwrap_or(s);
    if !name.starts_with("tty") {
        return None;
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some(name.to_string())
}

/// Absolute path for the configured dashboard console, if any.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn dashboard_console_path() -> Option<PathBuf> {
    dashboard_settings()
        .console
        .as_ref()
        .map(|n| PathBuf::from(format!("/dev/{n}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_disabled_truthy() {
        let d = resolve_from_cmdline_and_env(
            "console=ttyS0 pertisk.dashboard.disabled=1 rdinit=/init",
            |_| None,
        );
        assert!(d.disabled);
        assert!(d.console.is_none());
    }

    #[test]
    fn parses_disabled_falsey() {
        let d = resolve_from_cmdline_and_env("pertisk.dashboard.disabled=0", |_| None);
        assert!(!d.disabled);
    }

    #[test]
    fn parses_console_tty() {
        let d =
            resolve_from_cmdline_and_env("pertisk.dashboard.console=ttyS0 console=tty0", |_| None);
        assert_eq!(d.console.as_deref(), Some("ttyS0"));
    }

    #[test]
    fn rejects_non_tty_console() {
        let d = resolve_from_cmdline_and_env("pertisk.dashboard.console=hvc0", |_| None);
        assert!(d.console.is_none());
    }

    #[test]
    fn accepts_dev_prefix() {
        let d = resolve_from_cmdline_and_env("pertisk.dashboard.console=/dev/ttyAMA0", |_| None);
        assert_eq!(d.console.as_deref(), Some("ttyAMA0"));
    }

    #[test]
    fn last_wins() {
        let d = resolve_from_cmdline_and_env(
            "pertisk.dashboard.disabled=0 pertisk.dashboard.console=tty0 \
             pertisk.dashboard.disabled=true pertisk.dashboard.console=ttyS0",
            |_| None,
        );
        assert!(d.disabled);
        assert_eq!(d.console.as_deref(), Some("ttyS0"));
    }

    #[test]
    fn env_alias_when_cmdline_omits() {
        let d = resolve_from_cmdline_and_env("console=ttyS0 rdinit=/init", |k| match k {
            "PERTISK_DASHBOARD_DISABLED" => Some("yes".into()),
            "PERTISK_DASHBOARD_CONSOLE" => Some("ttyAMA0".into()),
            _ => None,
        });
        assert!(d.disabled);
        assert_eq!(d.console.as_deref(), Some("ttyAMA0"));
    }

    #[test]
    fn cmdline_beats_env() {
        let d = resolve_from_cmdline_and_env(
            "pertisk.dashboard.disabled=0 pertisk.dashboard.console=ttyS0",
            |k| match k {
                "PERTISK_DASHBOARD_DISABLED" => Some("1".into()),
                "PERTISK_DASHBOARD_CONSOLE" => Some("ttyAMA0".into()),
                _ => None,
            },
        );
        assert!(!d.disabled);
        assert_eq!(d.console.as_deref(), Some("ttyS0"));
    }
}
