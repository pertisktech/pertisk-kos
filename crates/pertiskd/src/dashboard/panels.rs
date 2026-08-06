//! Serial console TUI — layout/borders adapted from ptkube-dashboard,
//! styling adapted from feedo's themed-panel approach.
//!
//! Frame glyphs come from `theme::Chrome`, chosen by the startup UTF-8 probe.
//! Color is carried in cell styles and emitted as 16-color SGR by
//! `tui::encode_frame`.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::dashboard::snapshot::{format_bytes, format_kib, StatusSnapshot};
use crate::dashboard::theme::{Chrome, Theme};

/// Everything a frame needs to draw itself.
#[derive(Debug, Clone)]
pub struct Skin {
    pub theme: Theme,
    pub chrome: Chrome,
    /// Public web management URL (from `MGMT_PUBLIC_URL` / dashboard config).
    pub mgmt_url: Option<String>,
}

/// (node, network, mid) — grow with the pane so a small font shows more.
///
/// Everything left after these three goes to logs. Cap the top so a 50-row
/// console still keeps a tall log pane.
///
/// `extra_node_line` adds a second body row for the management URL.
pub fn top_box_heights(frame_h: u16, extra_node_line: bool) -> (u16, u16, u16) {
    let node = if extra_node_line { 4u16 } else { 3u16 };
    // Keep a taller logs pane — on small 80×22 frames the old budget left
    // ~4 hard-to-read log lines; reserve at least 10 rows for the logs frame.
    let log_reserve = match frame_h {
        0..=24 => 8,
        25..=36 => 10,
        _ => 14,
    };
    let budget = frame_h.saturating_sub(node + log_reserve);
    // cpu + memory + ≥1 disk need mid body ≥3 → panel height ≥5.
    let mid = match frame_h {
        0..=24 => 5,
        25..=36 => 6,
        _ => 8,
    }
    .min(budget)
    .max(5);
    let net = match frame_h {
        0..=24 => 4,
        25..=30 => 5,
        31..=40 => 7,
        41..=56 => 9,
        _ => 11,
    }
    .min(budget.saturating_sub(mid))
    .max(4);
    (node, net, mid)
}

/// Log content rows (inside frame borders).
pub fn log_inner_height(frame_h: u16) -> u16 {
    log_inner_height_for(frame_h, false)
}

pub fn log_inner_height_for(frame_h: u16, extra_node_line: bool) -> u16 {
    let (node, net, mid) = top_box_heights(frame_h, extra_node_line);
    frame_h
        .saturating_sub(node + net + mid)
        .saturating_sub(2) // top + bottom border
        .max(1)
}

/// Raw ring lines to pull for `rows` display rows — wrapping expands them.
pub fn log_tail_count(rows: usize) -> usize {
    rows.saturating_mul(3).clamp(8, 200)
}

fn panel<'a>(title: &'a str, skin: &Skin) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_set(skin.chrome.set)
        .border_style(skin.theme.border_style())
        .title(Span::styled(format!(" {title} "), skin.theme.title_style()))
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

/// `[████░░░░░░]  42%` — fill colored by utilization.
fn meter_spans(percent: u16, width: usize, skin: &Skin) -> Vec<Span<'static>> {
    let theme = &skin.theme;
    let bar_w = width.max(4);
    let filled = ((percent as usize).saturating_mul(bar_w) / 100).min(bar_w);
    vec![
        Span::styled("[", theme.meter_track_style()),
        Span::styled(
            skin.chrome.meter_fill.repeat(filled),
            theme.meter_style(percent),
        ),
        Span::styled(
            skin.chrome.meter_track.repeat(bar_w - filled),
            theme.meter_track_style(),
        ),
        Span::styled("]", theme.meter_track_style()),
        Span::styled(
            format!(" {:>3}%", percent.min(100)),
            theme.meter_style(percent),
        ),
    ]
}

fn label(text: &str, theme: &Theme) -> Span<'static> {
    Span::styled(text.to_string(), theme.label_style())
}

fn value(text: impl Into<String>, theme: &Theme) -> Span<'static> {
    Span::styled(text.into(), theme.value_style())
}

fn render_into(frame: &mut Frame, area: Rect, title: &str, lines: Vec<Line<'static>>, skin: &Skin) {
    render_block(frame, area, panel(title, skin), lines);
}

fn render_block(frame: &mut Frame, area: Rect, block: Block, lines: Vec<Line<'static>>) {
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let max_w = inner.width as usize;
    let max_h = inner.height as usize;
    let clipped: Vec<Line> = lines
        .into_iter()
        .take(max_h)
        .map(|line| truncate_line(line, max_w))
        .collect();
    // No wrap — wrapping spills into the next panel.
    frame.render_widget(Paragraph::new(clipped), inner);
}

fn draw_node(frame: &mut Frame, area: Rect, snap: &StatusSnapshot, skin: &Skin) {
    let theme = &skin.theme;
    let ready = if snap.ready { "ready" } else { "not-ready" };
    let mut lines = vec![Line::from(vec![
        Span::styled(snap.hostname.clone(), theme.title_style()),
        label("  v", theme),
        value(snap.version.clone(), theme),
        label("  ", theme),
        value(snap.machine_type.clone(), theme),
        label("  ", theme),
        Span::styled(ready.to_string(), theme.ready_style(snap.ready)),
    ])];
    if let Some(url) = skin.mgmt_url.as_deref() {
        lines.push(Line::from(vec![
            label("mgmt ", theme),
            value(url.to_string(), theme),
        ]));
    }
    render_into(frame, area, "node", lines, skin);
}

fn draw_network(frame: &mut Frame, area: Rect, snap: &StatusSnapshot, skin: &Skin) {
    let theme = &skin.theme;
    let max_h = area.height.saturating_sub(2) as usize;
    let mut lines: Vec<Line> = Vec::new();

    // Always reserve the last 1–2 rows for cluster + Kubernetes/cni so Cilium
    // noise (already filtered) cannot push the summary off-screen.
    let summary_rows = if max_h >= 3 { 2 } else { 1 };
    let iface_budget = max_h.saturating_sub(summary_rows);

    // Primary node IP line first.
    let node_line = if snap.node_iface.is_empty() && snap.node_ip == "-" {
        Line::from(Span::styled("(no node ip)", theme.warn_style()))
    } else {
        Line::from(vec![
            label("node ", theme),
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
    if iface_budget > 0 {
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
    let extra_slots = iface_budget.saturating_sub(lines.len());
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

    // Cluster summary — always when there is room (budget reserved above).
    if lines.len() < max_h {
        if summary_rows >= 2 && lines.len() + 2 <= max_h {
            lines.push(Line::from(vec![
                label("cluster ", theme),
                value(snap.cluster_endpoint.clone(), theme),
            ]));
            lines.push(Line::from(vec![
                label("Kubernetes ", theme),
                value(snap.kubernetes_version.clone(), theme),
                label("  cni ", theme),
                value(snap.cni.clone(), theme),
                label("  pod ", theme),
                value(snap.pod_cidr.clone(), theme),
            ]));
        } else {
            lines.push(Line::from(vec![
                label("cluster ", theme),
                value(snap.cluster_endpoint.clone(), theme),
                label("  Kubernetes ", theme),
                value(snap.kubernetes_version.clone(), theme),
                label("  cni ", theme),
                value(snap.cni.clone(), theme),
            ]));
        }
    }
    render_into(frame, area, "network", lines, skin);
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

fn draw_mid(frame: &mut Frame, area: Rect, snap: &StatusSnapshot, skin: &Skin) {
    let theme = &skin.theme;
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    let inner_h = cols[0].height.saturating_sub(2) as usize;
    let inner_w = cols[0].width.saturating_sub(2) as usize;
    // Scale the meter with the pane so GiB values are not cut to `~`.
    let meter_w = (inner_w / 4).clamp(8, 24);

    let mut resources: Vec<Line> = Vec::new();

    // cpu  [████░░░░]  42%  4c  load 0.35
    let mut cpu_spans = vec![label("cpu     ", theme)];
    cpu_spans.extend(meter_spans(snap.cpu_usage_pct, meter_w, skin));
    cpu_spans.push(label(" ", theme));
    cpu_spans.push(value(format!("{}c", snap.cpu_cores), theme));
    if snap.load_1m > 0.0 || snap.cpu_cores > 0 {
        cpu_spans.push(label("  load ", theme));
        cpu_spans.push(value(format!("{:.2}", snap.load_1m), theme));
    }
    resources.push(Line::from(cpu_spans));

    // memory  [██████░░]  62%  5.0/8.0 GiB
    let mem_pct = pct(snap.mem_used_kb(), snap.mem_total_kb);
    let mut mem_spans = vec![label("memory  ", theme)];
    mem_spans.extend(meter_spans(mem_pct, meter_w, skin));
    mem_spans.push(label(" ", theme));
    mem_spans.push(value(
        format!(
            "{}/{}",
            format_kib(snap.mem_used_kb()),
            format_kib(snap.mem_total_kb)
        ),
        theme,
    ));
    resources.push(Line::from(mem_spans));

    // disk rows — first line labeled "disk"; extras use the volume name.
    let disk_slots = inner_h.saturating_sub(resources.len()).max(1);
    for (i, d) in snap.disks.iter().take(disk_slots).enumerate() {
        let dp = pct(d.used_bytes, d.total_bytes);
        let row_label = if i == 0 {
            "disk    ".to_string()
        } else {
            format!("{:<8}", truncate_label(&d.label, 8))
        };
        let mut spans = vec![label(&row_label, theme)];
        spans.extend(meter_spans(dp, meter_w, skin));
        spans.push(label(" ", theme));
        spans.push(value(
            format!(
                "{}/{}",
                format_bytes(d.used_bytes),
                format_bytes(d.total_bytes)
            ),
            theme,
        ));
        if i == 0 {
            spans.push(label("  ", theme));
            spans.push(value(d.label.clone(), theme));
        }
        resources.push(Line::from(spans));
    }
    if snap.disks.len() > disk_slots {
        resources.push(Line::from(Span::styled(
            format!("… +{} disks", snap.disks.len() - disk_slots),
            theme.label_style(),
        )));
    }
    render_into(frame, cols[0], "resources", resources, skin);

    let mut svc = vec![
        service_line("containerd ", &snap.containerd, snap.containerd_pid, theme),
        service_line("kubelet    ", &snap.kubelet, snap.kubelet_pid, theme),
    ];
    if !snap.boot_slot.is_empty() && svc.len() < inner_h {
        svc.push(Line::from(vec![
            label("boot ", theme),
            value(snap.boot_slot.clone(), theme),
            label(" ok=", theme),
            Span::styled(snap.boot_ok.to_string(), theme.ready_style(snap.boot_ok)),
            label(" att=", theme),
            value(snap.boot_attempts.to_string(), theme),
        ]));
    }
    render_into(frame, cols[1], "services", svc, skin);
}

fn truncate_label(s: &str, max: usize) -> String {
    let cleaned = cell_clean(s);
    if cleaned.chars().count() <= max {
        return cleaned;
    }
    cleaned.chars().take(max).collect()
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

/// Manual ASCII logs frame (from ptkube) — avoids Block/content collision on
/// serial, and lets each wrapped line carry its own severity color.
fn draw_logs(frame: &mut Frame, area: Rect, recent: &[String], skin: &Skin) {
    let theme = &skin.theme;
    let glyphs = skin.chrome.set;
    let area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: area.height.max(3),
    };
    if area.width < 4 || area.height < 3 {
        return;
    }

    let border_style = theme.border_style();
    let buf = frame.buffer_mut();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.reset();
                cell.set_char(' ');
            }
        }
    }

    let x0 = area.left();
    let x1 = area.right().saturating_sub(1);
    let y0 = area.top();
    let y1 = area.bottom().saturating_sub(1);

    let frame_cell = |buf: &mut ratatui::buffer::Buffer, x: u16, y: u16, glyph: &str| {
        if let Some(c) = buf.cell_mut((x, y)) {
            c.set_symbol(glyph);
            c.set_style(border_style);
        }
    };
    frame_cell(buf, x0, y0, glyphs.top_left);
    frame_cell(buf, x1, y0, glyphs.top_right);
    frame_cell(buf, x0, y1, glyphs.bottom_left);
    frame_cell(buf, x1, y1, glyphs.bottom_right);
    for x in (x0 + 1)..x1 {
        frame_cell(buf, x, y0, glyphs.horizontal_top);
        frame_cell(buf, x, y1, glyphs.horizontal_bottom);
    }
    for y in (y0 + 1)..y1 {
        frame_cell(buf, x0, y, glyphs.vertical_left);
        frame_cell(buf, x1, y, glyphs.vertical_right);
    }

    let title = " logs ";
    let title_style = theme.title_style();
    let title_x = x0.saturating_add(2);
    for (i, ch) in title.chars().enumerate() {
        let x = title_x + i as u16;
        if x >= x1 {
            break;
        }
        if let Some(c) = buf.cell_mut((x, y0)) {
            c.set_char(ch);
            c.set_style(title_style);
        }
    }

    let inner_x = x0 + 1;
    let inner_y = y0 + 1;
    let inner_w = x1.saturating_sub(inner_x);
    // Fill every row between the borders — earlier builds left a blank spacer
    // that ate one log line on every frame.
    let inner_h = y1.saturating_sub(inner_y);
    if inner_w == 0 || inner_h == 0 {
        return;
    }
    let cols = inner_w as usize;
    let rows = inner_h as usize;

    for (i, text) in log_body(recent, cols, rows).iter().enumerate() {
        let y = inner_y + i as u16;
        let style = theme.log_line_style(text);
        for (j, ch) in text.chars().enumerate() {
            let x = inner_x + j as u16;
            if x >= x1 {
                break;
            }
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_char(ch);
                cell.set_style(style);
            }
        }
    }
}

/// Render node → network (IPs) → resources|services → logs.
pub fn render_themed(frame: &mut Frame, snap: &StatusSnapshot, recent: &[String], skin: &Skin) {
    let area = frame.area();
    let extra = skin.mgmt_url.is_some();
    let (node_h, net_h, mid_h) = top_box_heights(area.height, extra);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(node_h),
            Constraint::Length(net_h),
            Constraint::Length(mid_h),
            Constraint::Min(6),
        ])
        .split(area);

    draw_node(frame, rows[0], snap, skin);
    draw_network(frame, rows[1], snap, skin);
    draw_mid(frame, rows[2], snap, skin);
    draw_logs(frame, rows[3], recent, skin);
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_grows_with_taller_consoles() {
        let (n24, net24, mid24) = top_box_heights(24, false);
        let (n50, net50, mid50) = top_box_heights(50, false);
        assert_eq!(n24, 3);
        assert_eq!(n50, 3);
        assert_eq!(top_box_heights(24, true).0, 4);
        assert!(net50 > net24, "network should grow: {net50} vs {net24}");
        assert!(mid50 >= mid24);
        assert!(mid24 >= 5, "mid needs room for cpu/memory/disk");
        // Most of a tall pane still goes to logs.
        assert!(log_inner_height(50) >= 20);
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
