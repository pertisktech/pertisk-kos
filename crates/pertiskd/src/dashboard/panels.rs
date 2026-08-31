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
/// Summary: PERTISK|KUBERNETES band + NETWORK (v4 + v6 + gw/dns).
const SUMMARY_HEIGHT: u16 = 10;
const FOOTER_HEIGHT: u16 = 1;
/// Below this width, collapse the three info columns into one SYSTEM box.
/// Keep three columns at classic 80-col console sizes.
const WIDE_SUMMARY_MIN_COLS: u16 = 80;
/// Bordered logs need top rule + ≥1 body row (footer hostname is the bottom).
const MIN_LOG_HEIGHT: u16 = 2;

fn dashboard_heights(frame_h: u16) -> (u16, u16, u16, u16) {
    let header = HEADER_HEIGHT.min(frame_h);
    let footer = FOOTER_HEIGHT.min(frame_h.saturating_sub(header));
    let available = frame_h.saturating_sub(header + footer);
    let min_logs = MIN_LOG_HEIGHT.min(available);
    let summary =
        SUMMARY_HEIGHT
            .min(available.saturating_sub(min_logs))
            .max(if available > min_logs {
                3.min(available.saturating_sub(min_logs))
            } else {
                0
            });
    let logs = available.saturating_sub(summary);
    (header, summary, logs, footer)
}

/// Log content rows inside the logs frame (excludes top title rule).
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

fn panel<'a>(title: &'a str, skin: &Skin, borders: Borders) -> Block<'a> {
    // Horizontal rules only — vertical `|` sides crowd Serial and look noisy.
    Block::default()
        .borders(borders)
        .border_set(skin.chrome.set)
        .border_style(skin.theme.border_style())
        .title(Span::styled(format!(" {title} "), skin.theme.title_style()))
}

fn rule_glyph(skin: &Skin) -> char {
    skin.chrome.set.horizontal_top.chars().next().unwrap_or('=')
}

fn render_block(frame: &mut Frame, area: Rect, block: Block<'_>, lines: Vec<Line<'static>>) {
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
    frame.render_widget(Paragraph::new(clipped), inner);
}

fn render_into(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    lines: Vec<Line<'static>>,
    skin: &Skin,
    borders: Borders,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    render_block(frame, area, panel(title, skin, borders), lines);
}

fn draw_node(frame: &mut Frame, area: Rect, snap: &StatusSnapshot, skin: &Skin) {
    let theme = &skin.theme;
    let ready = if snap.ready { "true" } else { "false" };
    let lines = vec![
        Line::from(vec![
            label("TYPE       ", theme),
            value(snap.machine_type.clone(), theme),
        ]),
        Line::from(vec![
            label("READY      ", theme),
            Span::styled(ready, theme.ready_style(snap.ready)),
        ]),
        service_line("CONTAINERD ", &snap.containerd, snap.containerd_pid, theme),
        service_line("KUBELET    ", &snap.kubelet, snap.kubelet_pid, theme),
    ];
    // TOP only — NETWORK's title rule separates the bands (no orphan `---`).
    render_into(frame, area, "PERTISK", lines, skin, Borders::TOP);
}

fn draw_kubernetes(frame: &mut Frame, area: Rect, snap: &StatusSnapshot, skin: &Skin) {
    let theme = &skin.theme;
    // Short labels leave room for full CIDRs / endpoint on an 80-col console.
    let lines = vec![
        Line::from(vec![
            label("VER  ", theme),
            value(snap.kubernetes_version.clone(), theme),
        ]),
        Line::from(vec![
            label("EP   ", theme),
            value(snap.cluster_endpoint.clone(), theme),
        ]),
        Line::from(vec![label("CNI  ", theme), value(snap.cni.clone(), theme)]),
        Line::from(vec![
            label("POD  ", theme),
            value(snap.pod_cidr.clone(), theme),
        ]),
        Line::from(vec![
            label("SVC  ", theme),
            value(snap.service_subnet.clone(), theme),
        ]),
    ];
    render_into(frame, area, "KUBERNETES", lines, skin, Borders::TOP);
}

fn draw_network(frame: &mut Frame, area: Rect, snap: &StatusSnapshot, skin: &Skin) {
    let theme = &skin.theme;
    let body_h = area.height.saturating_sub(1) as usize;
    let max_w = area.width.max(1) as usize;
    let mut lines: Vec<Line> = Vec::new();

    let gw_dns = format!("gw {}  dns {}", snap.gateway_display(), snap.dns_display());
    let reserve_gw = if body_h >= 3 { 1 } else { 0 };
    let addr_budget = body_h.saturating_sub(reserve_gw);

    if snap.node_iface.is_empty() && snap.node_ip == "-" {
        if addr_budget > 0 {
            lines.push(Line::from(Span::styled("(no node ip)", theme.warn_style())));
        }
    } else {
        let primary_row = snap
            .net_rows
            .iter()
            .find(|r| row_iface_name(r) == snap.node_iface);
        let expanded = match primary_row {
            Some(row) => expand_iface_row(row),
            None => expand_node_ip(&snap.node_iface, &snap.node_ip),
        };
        for text in wrap_addr_block(pick_dual_stack_lines(expanded), max_w)
            .into_iter()
            .take(addr_budget.max(1))
        {
            lines.push(Line::from(value(text, theme)));
        }
    }
    if reserve_gw > 0 && lines.len() < body_h {
        lines.push(Line::from(vec![value(gw_dns, theme)]));
    }

    // TOP only — logs title rule separates the bands (no orphan `---`).
    render_into(frame, area, "NETWORK", lines, skin, Borders::TOP);
}

/// Keep one IPv4 line and one IPv6 line (GUA preferred over ULA).
fn pick_dual_stack_lines(expanded: Vec<String>) -> Vec<String> {
    let mut v4: Option<String> = None;
    let mut v6_gua: Option<String> = None;
    let mut v6_other: Option<String> = None;
    for line in expanded {
        let token = line.split_whitespace().last().unwrap_or("");
        let ip = token.split('/').next().unwrap_or(token);
        if looks_like_ipv6(ip) {
            if is_ipv6_ula_or_ll(ip) {
                v6_other.get_or_insert(line);
            } else {
                v6_gua.get_or_insert(line);
            }
        } else if looks_like_ipv4(ip) {
            v4.get_or_insert(line);
        }
    }
    let mut out = Vec::new();
    if let Some(line) = v4 {
        out.push(line);
    }
    if let Some(line) = v6_gua.or(v6_other) {
        out.push(line);
    }
    out
}

fn looks_like_ipv4(ip: &str) -> bool {
    ip.contains('.') && !ip.contains(':')
}

fn looks_like_ipv6(ip: &str) -> bool {
    ip.contains(':')
}

fn is_ipv6_ula_or_ll(ip: &str) -> bool {
    let ip = ip.to_ascii_lowercase();
    ip.starts_with("fe80:") || ip.starts_with("fc") || ip.starts_with("fd")
}

/// Hard-wrap address lines so a 39-char IPv6 GUA is not clipped with `~`.
fn wrap_addr_block(lines: Vec<String>, width: usize) -> Vec<String> {
    if width == 0 {
        return lines;
    }
    let mut out = Vec::new();
    for line in lines {
        let chars: Vec<char> = line.chars().collect();
        if chars.len() <= width {
            out.push(line);
            continue;
        }
        // Prefer breaking after the iface prefix ("eth0 ") on the first chunk.
        let mut start = 0usize;
        while start < chars.len() {
            let end = (start + width).min(chars.len());
            out.push(chars[start..end].iter().collect());
            start = end;
        }
    }
    out
}

fn row_iface_name(row: &str) -> &str {
    row.split_whitespace().next().unwrap_or("")
}

/// Split `eth0 UP 10.1.1.1/24 v6a v6b` into one line per address so dual-stack
/// IPv6 is not truncated off a single NETWORK row.
fn expand_iface_row(row: &str) -> Vec<String> {
    let mut parts = row.split_whitespace();
    let Some(iface) = parts.next() else {
        return vec![row.to_string()];
    };
    let rest: Vec<&str> = parts.collect();
    // Skip operstate token when present (UP/DOWN/UNKNOWN).
    let addrs: Vec<&str> = match rest.first() {
        Some(s) if looks_like_operstate(s) => rest[1..].to_vec(),
        _ => rest,
    };
    let addrs: Vec<&str> = addrs
        .into_iter()
        .filter(|a| *a != "(no" && *a != "ipv4)" && *a != "ip)")
        .collect();
    if addrs.is_empty() {
        return vec![iface.to_string()];
    }
    let mut out = Vec::with_capacity(addrs.len());
    out.push(format!("{iface} {}", addrs[0]));
    let pad = " ".repeat(iface.chars().count().saturating_add(1));
    for addr in &addrs[1..] {
        out.push(format!("{pad}{addr}"));
    }
    out
}

fn expand_node_ip(iface: &str, node_ip: &str) -> Vec<String> {
    let addrs: Vec<&str> = node_ip
        .split_whitespace()
        .filter(|a| *a != "(no" && *a != "ipv4)")
        .collect();
    if addrs.is_empty() {
        return if iface.is_empty() {
            vec![node_ip.to_string()]
        } else {
            vec![format!("{iface} {node_ip}")]
        };
    }
    let label = if iface.is_empty() { "" } else { iface };
    let mut out = Vec::with_capacity(addrs.len());
    if label.is_empty() {
        out.push(addrs[0].to_string());
        out.extend(addrs[1..].iter().map(|a| (*a).to_string()));
    } else {
        out.push(format!("{label} {}", addrs[0]));
        let pad = " ".repeat(label.chars().count().saturating_add(1));
        for addr in &addrs[1..] {
            out.push(format!("{pad}{addr}"));
        }
    }
    out
}

fn looks_like_operstate(s: &str) -> bool {
    matches!(
        s.to_ascii_uppercase().as_str(),
        "UP" | "DOWN" | "UNKNOWN" | "LOWERLAYERDOWN" | "DORMANT" | "?"
    )
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
    // Six body lines fit SUMMARY_HEIGHT (10) with top+bottom rules.
    let lines = vec![
        Line::from(vec![
            label("TYPE       ", theme),
            value(snap.machine_type.clone(), theme),
            label("  BOOT ", theme),
            value(format!("slot {} [{boot}]", snap.boot_slot), theme),
        ]),
        Line::from(vec![label("NODE       ", theme), value(node, theme)]),
        Line::from(vec![
            label("GW         ", theme),
            value(snap.gateway_display(), theme),
            label("  DNS ", theme),
            value(snap.dns_display(), theme),
        ]),
        Line::from(vec![
            label("ENDPOINT   ", theme),
            value(snap.cluster_endpoint.clone(), theme),
        ]),
        Line::from(vec![
            label("POD        ", theme),
            value(snap.pod_cidr.clone(), theme),
            label("  SVC ", theme),
            value(snap.service_subnet.clone(), theme),
            label("  CNI ", theme),
            value(snap.cni.clone(), theme),
        ]),
        Line::from(vec![
            label("SERVICES   containerd ", theme),
            Span::styled(
                format!("[{}]", snap.containerd),
                theme.status_style(&snap.containerd),
            ),
            label("  kubelet ", theme),
            Span::styled(
                format!("[{}]", snap.kubelet),
                theme.status_style(&snap.kubelet),
            ),
        ]),
    ];
    render_into(
        frame,
        area,
        "SYSTEM",
        lines,
        skin,
        Borders::TOP | Borders::BOTTOM,
    );
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
        Span::styled(
            format!("{}%", snap.cpu_usage_pct),
            theme.meter_style(snap.cpu_usage_pct),
        ),
        label(" RAM ", theme),
        Span::styled(format!("{mem_pct}%"), theme.meter_style(mem_pct)),
        label(" LOAD ", theme),
        value(format!("{:.2}", snap.load_1m), theme),
        label(" | UP ", theme),
        value(format_uptime(snap.uptime_secs), theme),
    ]);
    frame.render_widget(
        Paragraph::new(truncate_line(line, area.width as usize)),
        area,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect, snap: &StatusSnapshot, skin: &Skin) {
    if area.width == 0 {
        return;
    }
    let theme = &skin.theme;
    // Match panel titles (` NETWORK `, ` logs `) — spaced name on the rule.
    let left = format!(" {} ", snap.hostname);
    let right = match skin.mgmt_url.as_deref() {
        Some(url) => format!(" {url} "),
        None => String::new(),
    };
    let fill = rule_glyph(skin)
        .to_string()
        .repeat((area.width as usize).saturating_sub(left.chars().count() + right.chars().count()));
    let line = Line::from(vec![
        Span::styled(left, theme.title_style()),
        Span::styled(fill, theme.border_style()),
        Span::styled(right, theme.value_style()),
    ]);
    frame.render_widget(
        Paragraph::new(truncate_line(line, area.width as usize)),
        area,
    );
}

/// Manual ASCII logs frame — top rule only (footer hostname closes the pane).
fn draw_logs(frame: &mut Frame, area: Rect, recent: &[String], skin: &Skin) {
    let theme = &skin.theme;
    let glyphs = skin.chrome.set;
    let area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: area.height.max(2),
    };
    if area.width < 4 || area.height < 2 {
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

    let frame_cell = |buf: &mut ratatui::buffer::Buffer, x: u16, y: u16, glyph: &str| {
        if let Some(c) = buf.cell_mut((x, y)) {
            c.set_symbol(glyph);
            c.set_style(border_style);
        }
    };
    // Top rule only — hostname footer is the bottom edge (no orphan `---`).
    for x in x0..=x1 {
        frame_cell(buf, x, y0, glyphs.horizontal_top);
    }

    let title = " logs ";
    let title_style = theme.title_style();
    // Start at the left edge so the title is ` logs ---` not `- logs ---`.
    let title_x = x0;
    for (i, ch) in title.chars().enumerate() {
        let x = title_x + i as u16;
        if x > x1 {
            break;
        }
        if let Some(c) = buf.cell_mut((x, y0)) {
            c.set_char(ch);
            c.set_style(title_style);
        }
    }

    let inner_x = x0;
    let inner_y = y0 + 1;
    let inner_w = area.width;
    let inner_h = area.bottom().saturating_sub(inner_y);
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
            if x > x1 {
                break;
            }
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_char(ch);
                cell.set_style(style);
            }
        }
    }
}

/// Header + bordered summary columns (or compact SYSTEM) + bordered logs + footer.
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
    if area.width < WIDE_SUMMARY_MIN_COLS {
        draw_compact_summary(frame, rows[1], snap, skin);
    } else {
        // PERTISK | KUBERNETES on top; NETWORK is v4 + v6 + gw/dns.
        let summary = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(6), Constraint::Length(4)])
            .split(rows[1]);
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Fill(1), Constraint::Fill(1)])
            .split(summary[0]);
        draw_node(frame, columns[0], snap, skin);
        draw_kubernetes(frame, columns[1], snap, skin);
        draw_network(frame, summary[1], snap, skin);
    }
    draw_logs(frame, rows[2], recent, skin);
    draw_footer(frame, rows[3], snap, skin);
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_dual_stack_prefers_gua_over_ula() {
        let lines = pick_dual_stack_lines(vec![
            "eth0 192.168.1.50/24".into(),
            "     fd00:a:1:1::32".into(),
            "     2405:9800:b901:194c:be24:11ff:fe91:e066".into(),
        ]);
        assert_eq!(
            lines,
            vec![
                "eth0 192.168.1.50/24".to_string(),
                "     2405:9800:b901:194c:be24:11ff:fe91:e066".to_string(),
            ]
        );
    }

    #[test]
    fn layout_reserves_header_summary_and_footer() {
        // 24: header1 + summary10 + logs12 + footer1
        assert_eq!(dashboard_heights(24), (1, 10, 12, 1));
        // 8: header1 + summary4 + logs2 + footer1 (min panes with top-only logs)
        assert_eq!(dashboard_heights(8), (1, 4, 2, 1));
        assert_eq!(log_inner_height(24), 11);
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

    #[test]
    fn expand_iface_row_puts_each_addr_on_its_own_line() {
        let rows = expand_iface_row(
            "eth0  UP  10.1.1.173/24  fd00:a:1:1::ad  2405:9800:b901:194c:be24:11ff:fe91:e066",
        );
        assert_eq!(
            rows,
            vec![
                "eth0 10.1.1.173/24".to_string(),
                "     fd00:a:1:1::ad".to_string(),
                "     2405:9800:b901:194c:be24:11ff:fe91:e066".to_string(),
            ]
        );
    }

    #[test]
    fn wrap_addr_block_keeps_full_ipv6_gua() {
        let gua = "2405:9800:b901:194c:be24:11ff:fe91:e066";
        assert_eq!(gua.chars().count(), 39);
        let wrapped = wrap_addr_block(vec![format!("eth0 {gua}")], 40);
        let joined: String = wrapped.concat();
        assert!(joined.contains(gua), "GUA must not be clipped: {wrapped:?}");
        let wrapped_narrow = wrap_addr_block(vec![gua.to_string()], 20);
        assert!(wrapped_narrow.len() >= 2);
        assert_eq!(wrapped_narrow.concat(), gua);
    }
}
