//! Serial console TUI — layout/borders adapted from ptkube-dashboard,
//! styling adapted from feedo's themed-panel approach.
//!
//! Frame glyphs come from `theme::Chrome`, chosen by the startup UTF-8 probe.
//! Color is carried in cell styles and emitted as 16-color SGR by
//! `tui::encode_frame`.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::dashboard::snapshot::StatusSnapshot;
use crate::dashboard::theme::{Chrome, Theme};

/// Everything a frame needs to draw itself.
#[derive(Debug, Clone)]
pub struct Skin {
    pub theme: Theme,
    pub chrome: Chrome,
    pub background: Option<ratatui::style::Color>,
    /// Public web management URL (from `MGMT_PUBLIC_URL` / dashboard config).
    pub mgmt_url: Option<String>,
}

const HEADER_HEIGHT: u16 = 1;
const SUMMARY_HEIGHT: u16 = 6;
const FOOTER_HEIGHT: u16 = 1;
const MIN_LOG_HEIGHT: u16 = 2;

fn dashboard_heights(frame_h: u16) -> (u16, u16, u16, u16) {
    let header = HEADER_HEIGHT.min(frame_h);
    let footer = FOOTER_HEIGHT.min(frame_h.saturating_sub(header));
    let available = frame_h.saturating_sub(header + footer);
    let logs = MIN_LOG_HEIGHT.min(available);
    let summary = SUMMARY_HEIGHT.min(available.saturating_sub(logs));
    let logs = available.saturating_sub(summary);
    (header, summary, logs, footer)
}

/// Log content rows below the section title.
pub fn log_inner_height(frame_h: u16) -> u16 {
    let (_, _, logs, _) = dashboard_heights(frame_h);
    logs.saturating_sub(1).max(1)
}

pub fn log_inner_height_for(frame_h: u16, _extra_node_line: bool) -> u16 {
    log_inner_height(frame_h)
}

/// Raw ring lines to pull for `rows` display rows — wrapping expands them.
pub fn log_tail_count(rows: usize) -> usize {
    rows.saturating_mul(3).clamp(8, 200)
}

/// Printable ASCII only — one byte per terminal cell, so byte length equals
/// column count. Used for log text, which can carry arbitrary UTF-8.
fn ascii_clean(s: &str) -> String {
    s.chars()
        .map(|c| if c == '\t' { ' ' } else { c })
        .filter(|c| c.is_ascii() && !c.is_control())
        .collect()
}

/// Printable ASCII plus box-drawing and block elements (U+2500–U+259F).
///
/// Those are all single-column, and `tui::encode_frame` degrades them to
/// ASCII when the console turned out not to speak UTF-8.
fn cell_clean(s: &str) -> String {
    s.chars()
        .map(|c| if c == '\t' { ' ' } else { c })
        .filter(|c| (c.is_ascii() && !c.is_control()) || matches!(c, '\u{2500}'..='\u{259f}'))
        .collect()
}

/// Clip a styled line to `width` columns, keeping per-span colors.
fn truncate_line(line: Line<'static>, width: usize) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for span in line.spans {
        if used >= width {
            break;
        }
        let text = cell_clean(&span.content);
        if text.is_empty() {
            continue;
        }
        let cols = text.chars().count();
        let remaining = width - used;
        if cols <= remaining {
            used += cols;
            out.push(Span::styled(text, span.style));
        } else {
            let mut cut: String = text.chars().take(remaining.saturating_sub(1)).collect();
            cut.push('~');
            used = width;
            out.push(Span::styled(cut, span.style));
        }
    }
    Line::from(out)
}

/// Word-wrap to `width`, indenting continuations. Input is ASCII-cleaned, so
/// byte length equals column count.
pub fn wrap_line(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let text = ascii_clean(text);
    if text.is_empty() {
        return Vec::new();
    }
    if text.len() <= width {
        return vec![text];
    }
    let indent = if width > 12 { 2 } else { 0 };
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let budget = if out.is_empty() {
            width
        } else {
            width - indent
        };
        if !cur.is_empty() {
            if cur.len() + 1 + word.len() <= budget {
                cur.push(' ');
                cur.push_str(word);
                continue;
            }
            out.push(std::mem::take(&mut cur));
        }
        let mut rest = word;
        loop {
            let budget = if out.is_empty() {
                width
            } else {
                width - indent
            };
            if rest.len() <= budget {
                break;
            }
            let (head, tail) = rest.split_at(budget);
            out.push(head.to_string());
            rest = tail;
        }
        cur.push_str(rest);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    let pad = " ".repeat(indent);
    out.into_iter()
        .enumerate()
        .map(|(i, s)| if i == 0 { s } else { format!("{pad}{s}") })
        .collect()
}

/// Wrap the ring tail and keep the newest `rows` display lines.
pub fn log_body(recent: &[String], width: usize, rows: usize) -> Vec<String> {
    if width == 0 || rows == 0 {
        return Vec::new();
    }
    let mut wrapped: Vec<String> = Vec::new();
    for line in recent {
        wrapped.extend(wrap_line(line, width));
    }
    let start = wrapped.len().saturating_sub(rows);
    wrapped.split_off(start)
}

fn pct(used: u64, total: u64) -> u16 {
    if total == 0 {
        return 0;
    }
    ((used.saturating_mul(100)) / total).min(100) as u16
}

fn format_uptime(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if days > 0 {
        format!("{days}d{hours}h{minutes}m")
    } else if hours > 0 {
        format!("{hours}h{minutes}m")
    } else {
        format!("{minutes}m")
    }
}

fn label(text: &str, theme: &Theme) -> Span<'static> {
    Span::styled(text.to_string(), theme.label_style())
}

fn value(text: impl Into<String>, theme: &Theme) -> Span<'static> {
    Span::styled(text.into(), theme.value_style())
}

fn section_line(title: impl Into<String>, width: u16, skin: &Skin) -> Line<'static> {
    let title = title.into();
    let title_width = title.chars().count();
    let gap = usize::from(width).saturating_sub(title_width);
    let rule = "-".repeat(gap);
    Line::from(vec![
        Span::styled(title, skin.theme.title_style()),
        Span::styled(rule, skin.theme.border_style()),
    ])
}

fn render_into(frame: &mut Frame, area: Rect, title: &str, lines: Vec<Line<'static>>, skin: &Skin) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let title_area = Rect::new(area.x, area.y, area.width, 1);
    frame.render_widget(
        Paragraph::new(section_line(format!("{title} "), area.width, skin)),
        title_area,
    );
    let body = Rect::new(
        area.x,
        area.y.saturating_add(1),
        area.width,
        area.height.saturating_sub(1),
    );
    let max_w = body.width as usize;
    let max_h = body.height as usize;
    let clipped: Vec<Line> = lines
        .into_iter()
        .take(max_h)
        .map(|line| truncate_line(line, max_w))
        .collect();
    frame.render_widget(Paragraph::new(clipped), body);
}

fn draw_node(frame: &mut Frame, area: Rect, snap: &StatusSnapshot, skin: &Skin) {
    let theme = &skin.theme;
    let ready = if snap.ready { "true" } else { "false" };
    let lines = vec![
        Line::from(vec![label("TYPE       ", theme), value(snap.machine_type.clone(), theme)]),
        Line::from(vec![
            label("READY      ", theme),
            Span::styled(ready, theme.ready_style(snap.ready)),
        ]),
        service_line("CONTAINERD ", &snap.containerd, snap.containerd_pid, theme),
        service_line("KUBELET    ", &snap.kubelet, snap.kubelet_pid, theme),
    ];
    render_into(frame, area, "PERTISK", lines, skin);
}

fn draw_kubernetes(frame: &mut Frame, area: Rect, snap: &StatusSnapshot, skin: &Skin) {
    let theme = &skin.theme;
    let lines = vec![
        Line::from(vec![label("VERSION  ", theme), value(snap.kubernetes_version.clone(), theme)]),
        Line::from(vec![label("ENDPOINT ", theme), value(snap.cluster_endpoint.clone(), theme)]),
        Line::from(vec![label("CNI      ", theme), value(snap.cni.clone(), theme)]),
        Line::from(vec![label("POD CIDR ", theme), value(snap.pod_cidr.clone(), theme)]),
    ];
    render_into(frame, area, "KUBERNETES", lines, skin);
}

fn draw_network(frame: &mut Frame, area: Rect, snap: &StatusSnapshot, skin: &Skin) {
    let theme = &skin.theme;
    let max_h = area.height.saturating_sub(1) as usize;
    let mut lines: Vec<Line> = Vec::new();

    // Primary node IP line first.
    let node_line = if snap.node_iface.is_empty() && snap.node_ip == "-" {
        Line::from(Span::styled("(no node ip)", theme.warn_style()))
    } else {
        Line::from(vec![
            label("NODE  ", theme),
            value(
                if snap.node_iface.is_empty() {
                    snap.node_ip.clone()
                } else {
                    format!("{} {}", snap.node_iface, snap.node_ip)
                },
                theme,
            ),
        ])
    };
    if max_h > 0 {
        lines.push(node_line);
    }

    // Extra host iface rows (skip the primary if already shown as node line).
    let extra: Vec<&String> = snap
        .net_rows
        .iter()
        .filter(|r| {
            let name = r.split_whitespace().next().unwrap_or("");
            name != snap.node_iface
        })
        .collect();
    let extra_slots = max_h.saturating_sub(lines.len());
    for row in extra.iter().take(extra_slots) {
        lines.push(net_row_line(row, theme));
    }
    if extra.len() > extra_slots && extra_slots > 0 {
        lines.pop();
        lines.push(Line::from(Span::styled(
            format!("… +{} more", extra.len() - extra_slots + 1),
            theme.label_style(),
        )));
    }

    render_into(frame, area, "NETWORK", lines, skin);
}

/// Interface name dimmed, address highlighted.
fn net_row_line(row: &str, theme: &Theme) -> Line<'static> {
    match row.split_once(char::is_whitespace) {
        Some((iface, rest)) => Line::from(vec![
            label(iface, theme),
            label(" ", theme),
            value(rest.trim_start().to_string(), theme),
        ]),
        None => Line::from(value(row.to_string(), theme)),
    }
}

fn service_line(name: &str, status: &str, pid: u32, theme: &Theme) -> Line<'static> {
    let st = theme.status_style(status);
    let mut spans = vec![
        label(name, theme),
        // Brackets make the health token obvious even when the terminal
        // remaps hues; the fill still carries ok/warn/err color.
        Span::styled(format!("[{status}]"), st),
    ];
    if pid > 0 {
        spans.push(label(&format!(" pid={pid}"), theme));
    }
    Line::from(spans)
}

fn draw_compact_summary(frame: &mut Frame, area: Rect, snap: &StatusSnapshot, skin: &Skin) {
    let theme = &skin.theme;
    let node = if snap.node_iface.is_empty() {
        snap.node_ip.clone()
    } else {
        format!("{} {}", snap.node_iface, snap.node_ip)
    };
    let boot = if snap.boot_ok { "ok" } else { "pending" };
    let lines = vec![
        section_line(
            format!("[ SYSTEM ] {} ", snap.machine_type),
            area.width,
            skin,
        ),
        Line::from(vec![label("NODE       ", theme), value(node, theme)]),
        Line::from(vec![
            label("ENDPOINT   ", theme),
            value(snap.cluster_endpoint.clone(), theme),
        ]),
        Line::from(vec![
            label("K8S        ", theme),
            value(snap.kubernetes_version.clone(), theme),
            label("  CNI ", theme),
            value(snap.cni.clone(), theme),
            label("  POD ", theme),
            value(snap.pod_cidr.clone(), theme),
        ]),
        Line::from(vec![
            label("SERVICES   containerd ", theme),
            Span::styled(format!("[{}]", snap.containerd), theme.status_style(&snap.containerd)),
            label("  kubelet ", theme),
            Span::styled(format!("[{}]", snap.kubelet), theme.status_style(&snap.kubelet)),
        ]),
        Line::from(vec![
            label("BOOT       slot ", theme),
            value(snap.boot_slot.clone(), theme),
            label("  status ", theme),
            Span::styled(format!("[{boot}]"), theme.ready_style(snap.boot_ok)),
            label("  attempts ", theme),
            value(snap.boot_attempts.to_string(), theme),
        ]),
    ];
    let clipped: Vec<Line> = lines
        .into_iter()
        .take(area.height as usize)
        .map(|line| truncate_line(line, area.width as usize))
        .collect();
    frame.render_widget(Paragraph::new(clipped), area);
}

fn draw_header(frame: &mut Frame, area: Rect, snap: &StatusSnapshot, skin: &Skin) {
    let theme = &skin.theme;
    let mem_pct = pct(snap.mem_used_kb(), snap.mem_total_kb);
    let ready = if snap.ready { "READY" } else { "NOT READY" };
    let line = Line::from(vec![
        Span::styled(" PERTISK ", theme.header_style()),
        value(format!("{}  v{}", snap.hostname, snap.version), theme),
        label(" | ", theme),
        Span::styled(ready, theme.ready_style(snap.ready)),
        label(" | CPU ", theme),
        Span::styled(format!("{}%", snap.cpu_usage_pct), theme.meter_style(snap.cpu_usage_pct)),
        label(" RAM ", theme),
        Span::styled(format!("{mem_pct}%"), theme.meter_style(mem_pct)),
        label(" LOAD ", theme),
        value(format!("{:.2}", snap.load_1m), theme),
        label(" | UP ", theme),
        value(format_uptime(snap.uptime_secs), theme),
    ]);
    frame.render_widget(Paragraph::new(truncate_line(line, area.width as usize)), area);
}

fn draw_footer(frame: &mut Frame, area: Rect, snap: &StatusSnapshot, skin: &Skin) {
    if area.width == 0 {
        return;
    }
    let theme = &skin.theme;
    let left = format!("[ END LOGS ]  {}  |  refresh 5s", snap.hostname);
    let right = match skin.mgmt_url.as_deref() {
        Some(url) => format!(" {url} "),
        None => String::new(),
    };
    let fill = "-".repeat(
        (area.width as usize).saturating_sub(left.chars().count() + right.chars().count()),
    );
    let line = Line::from(vec![
        Span::styled(left, theme.title_style()),
        Span::styled(fill, theme.border_style()),
        Span::styled(right, theme.value_style()),
    ]);
    frame.render_widget(Paragraph::new(truncate_line(line, area.width as usize)), area);
}

/// Borderless log surface with a compact section heading.
fn draw_logs(frame: &mut Frame, area: Rect, recent: &[String], skin: &Skin) {
    let theme = &skin.theme;
    if area.width == 0 || area.height < 2 {
        return;
    }
    frame.render_widget(
        Paragraph::new(section_line("[ LOGS ] ", area.width, skin)),
        Rect::new(area.x, area.y, area.width, 1),
    );
    let body = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
    let lines: Vec<Line> = log_body(recent, body.width as usize, body.height as usize)
        .into_iter()
        .map(|text| {
            let style = theme.log_line_style(&text);
            Line::from(Span::styled(text, style))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), body);
}

/// Proxxx-inspired summary adapted for a non-interactive serial console.
pub fn render_themed(frame: &mut Frame, snap: &StatusSnapshot, recent: &[String], skin: &Skin) {
    let area = frame.area();
    if let Some(background) = skin.background {
        frame.render_widget(
            Block::default().style(ratatui::style::Style::default().bg(background)),
            area,
        );
    }
    let (header_h, summary_h, logs_h, footer_h) = dashboard_heights(area.height);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_h),
            Constraint::Length(summary_h),
            Constraint::Length(logs_h),
            Constraint::Length(footer_h),
        ])
        .split(area);

    draw_header(frame, rows[0], snap, skin);
    if area.width < 120 {
        draw_compact_summary(frame, rows[1], snap, skin);
    } else {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Fill(1),
                Constraint::Length(2),
                Constraint::Fill(1),
                Constraint::Length(2),
                Constraint::Fill(1),
            ])
            .split(rows[1]);
        draw_node(frame, columns[0], snap, skin);
        draw_kubernetes(frame, columns[2], snap, skin);
        draw_network(frame, columns[4], snap, skin);
    }
    draw_logs(frame, rows[2], recent, skin);
    draw_footer(frame, rows[3], snap, skin);
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn talos_layout_reserves_header_summary_and_footer() {
        assert_eq!(dashboard_heights(24), (1, 6, 16, 1));
        assert_eq!(dashboard_heights(8), (1, 4, 2, 1));
        assert_eq!(log_inner_height(24), 15);
        assert_eq!(log_inner_height(8), 1);
    }

    #[test]
    fn uptime_is_compact() {
        assert_eq!(format_uptime(59), "0m");
        assert_eq!(format_uptime(3_660), "1h1m");
        assert_eq!(format_uptime(93_784), "1d2h3m");
    }

    #[test]
    fn wrap_splits_on_word_boundaries_and_indents() {
        let out = wrap_line("alpha beta gamma delta epsilon", 16);
        assert_eq!(out, vec!["alpha beta gamma", "  delta epsilon"]);
    }

    #[test]
    fn wrap_hard_splits_overlong_words() {
        let out = wrap_line("aaaaaaaaaaaaaaaaaaaa", 8);
        assert_eq!(out, vec!["aaaaaaaa", "aaaaaaaa", "aaaa"]);
    }

    #[test]
    fn wrap_keeps_short_line_intact() {
        assert_eq!(wrap_line("ready", 20), vec!["ready"]);
    }

    #[test]
    fn log_body_keeps_newest_rows() {
        let recent = vec![
            "one two three four five".to_string(),
            "six".to_string(),
            "seven".to_string(),
        ];
        let out = log_body(&recent, 12, 2);
        assert_eq!(out, vec!["six", "seven"]);
    }

    #[test]
    fn truncate_line_preserves_span_styles() {
        let theme = crate::dashboard::theme::DRACULA;
        let line = Line::from(vec![
            Span::styled("kubelet ", theme.label_style()),
            Span::styled("up", theme.ok_style()),
        ]);
        let out = truncate_line(line, 32);
        assert_eq!(out.spans[1].content.as_ref(), "up");
        assert_eq!(out.spans[1].style.fg, Some(theme.ok));
    }

    #[test]
    fn truncate_line_marks_clipped_text() {
        let line = Line::from("abcdefgh");
        let out = truncate_line(line, 4);
        let text: String = out.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "abc~");
    }

    /// Block glyphs are three bytes but one column — budget by column.
    #[test]
    fn truncate_line_counts_wide_glyphs_as_one_column() {
        let line = Line::from("████████");
        let out = truncate_line(line, 4);
        let text: String = out.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text.chars().count(), 4);
    }

    #[test]
    fn cell_clean_keeps_meter_glyphs_but_drops_emoji() {
        assert_eq!(cell_clean("█░"), "█░");
        assert_eq!(cell_clean("ok\u{1f600}"), "ok");
    }
}
