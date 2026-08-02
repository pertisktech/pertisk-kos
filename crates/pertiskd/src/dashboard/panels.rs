//! Serial console TUI — layout/borders adapted from ptkube-dashboard.
//!
//! ASCII borders only. Content is clipped to `block.inner`; logs paints its
//! frame by hand so the last line never sits on the bottom border.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::dashboard::snapshot::{format_bytes, format_kib, StatusSnapshot};

const ASCII_BORDER: border::Set = border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

/// (node, network, mid) — leave room for logs.
pub fn top_box_heights(frame_h: u16) -> (u16, u16, u16) {
    // Match ptkube sizing for 80x24 serial panes.
    if frame_h <= 24 {
        (3, 5, 5)
    } else if frame_h <= 28 {
        (3, 6, 5)
    } else {
        (3, 7, 5)
    }
}

/// Log content rows (inside frame, with one blank row above bottom border).
pub fn log_inner_height(frame_h: u16) -> u16 {
    let (node, net, mid) = top_box_heights(frame_h);
    // logs area height - top border - bottom border - blank row before bottom
    frame_h
        .saturating_sub(node + net + mid)
        .saturating_sub(3)
        .max(1)
}

fn panel(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_set(ASCII_BORDER)
        .title(Span::styled(
            format!(" {title} "),
            Style::default().add_modifier(Modifier::BOLD),
        ))
}

/// Truncate to ASCII columns (ptkube truncate_cols).
fn truncate_cols(s: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let chars: Vec<char> = s
        .chars()
        .map(|c| {
            if c.is_ascii() && !c.is_control() {
                c
            } else {
                '?'
            }
        })
        .collect();
    if chars.len() <= width {
        chars.into_iter().collect()
    } else if width == 1 {
        "~".into()
    } else {
        chars
            .iter()
            .take(width - 1)
            .chain(std::iter::once(&'~'))
            .collect()
    }
}

fn pct(used: u64, total: u64) -> u16 {
    if total == 0 {
        return 0;
    }
    ((used.saturating_mul(100)) / total).min(100) as u16
}

fn meter(percent: u16, width: usize) -> String {
    let bar_w = width.max(4);
    let filled = ((percent as usize).saturating_mul(bar_w) / 100).min(bar_w);
    let empty = bar_w - filled;
    format!(
        "[{}{}] {:>3}%",
        "|".repeat(filled),
        "-".repeat(empty),
        percent.min(100)
    )
}

fn render_into(frame: &mut Frame, area: Rect, title: &str, lines: Vec<Line<'static>>) {
    let block = panel(title);
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
        .map(|line| {
            let text: String = line
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect();
            Line::from(truncate_cols(&text, max_w))
        })
        .collect();
    // No wrap — wrapping spills into the next panel.
    frame.render_widget(Paragraph::new(clipped), inner);
}

fn draw_node(frame: &mut Frame, area: Rect, snap: &StatusSnapshot) {
    let ready = if snap.ready { "ready" } else { "not-ready" };
    let lines = vec![Line::from(format!(
        "{}  v{}  {}  {}  cpu {}c",
        snap.hostname, snap.version, snap.machine_type, ready, snap.cpu_cores
    ))];
    render_into(frame, area, "node", lines);
}

fn draw_network(frame: &mut Frame, area: Rect, snap: &StatusSnapshot) {
    let max_h = area.height.saturating_sub(2) as usize;
    let mut lines: Vec<Line> = Vec::new();

    if snap.net_rows.is_empty() {
        lines.push(Line::from("(no addresses)"));
    } else {
        for row in snap.net_rows.iter().take(max_h.saturating_sub(1).max(1)) {
            lines.push(Line::from(row.clone()));
        }
    }

    if lines.len() < max_h {
        lines.push(Line::from(format!(
            "cluster {}  cni {}  pod {}",
            snap.cluster_endpoint, snap.cni, snap.pod_cidr
        )));
    }
    render_into(frame, area, "network", lines);
}

fn draw_mid(frame: &mut Frame, area: Rect, snap: &StatusSnapshot) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    let mem_pct = pct(snap.mem_used_kb(), snap.mem_total_kb);
    let mut mem = vec![Line::from(format!(
        "mem {} {}/{}",
        meter(mem_pct, 10),
        format_kib(snap.mem_used_kb()),
        format_kib(snap.mem_total_kb)
    ))];
    for d in snap.disks.iter().take(2) {
        let dp = pct(d.used_bytes, d.total_bytes);
        mem.push(Line::from(format!(
            "{} {} {}/{}",
            d.label,
            meter(dp, 10),
            format_bytes(d.used_bytes),
            format_bytes(d.total_bytes)
        )));
    }
    render_into(frame, cols[0], "mem", mem);

    let ctd = if snap.containerd_pid > 0 {
        format!("{} pid={}", snap.containerd, snap.containerd_pid)
    } else {
        snap.containerd.clone()
    };
    let kub = if snap.kubelet_pid > 0 {
        format!("{} pid={}", snap.kubelet, snap.kubelet_pid)
    } else {
        snap.kubelet.clone()
    };
    let mut svc = vec![
        Line::from(format!("containerd  {ctd}")),
        Line::from(format!("kubelet     {kub}")),
    ];
    if !snap.boot_slot.is_empty() {
        svc.push(Line::from(format!(
            "boot {} ok={} att={}",
            snap.boot_slot, snap.boot_ok, snap.boot_attempts
        )));
    }
    render_into(frame, cols[1], "services", svc);
}

/// Manual ASCII logs frame (from ptkube) — avoids Block/content collision on serial.
fn draw_logs(frame: &mut Frame, area: Rect, recent: &[String]) {
    let area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: area.height.max(3),
    };
    if area.width < 4 || area.height < 3 {
        return;
    }

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

    if let Some(c) = buf.cell_mut((x0, y0)) {
        c.set_char('+');
    }
    if let Some(c) = buf.cell_mut((x1, y0)) {
        c.set_char('+');
    }
    if let Some(c) = buf.cell_mut((x0, y1)) {
        c.set_char('+');
    }
    if let Some(c) = buf.cell_mut((x1, y1)) {
        c.set_char('+');
    }
    for x in (x0 + 1)..x1 {
        if let Some(c) = buf.cell_mut((x, y0)) {
            c.set_char('-');
        }
        if let Some(c) = buf.cell_mut((x, y1)) {
            c.set_char('-');
        }
    }
    for y in (y0 + 1)..y1 {
        if let Some(c) = buf.cell_mut((x0, y)) {
            c.set_char('|');
        }
        if let Some(c) = buf.cell_mut((x1, y)) {
            c.set_char('|');
        }
    }

    let title = " logs ";
    let title_x = x0.saturating_add(2);
    for (i, ch) in title.chars().enumerate() {
        let x = title_x + i as u16;
        if x >= x1 {
            break;
        }
        if let Some(c) = buf.cell_mut((x, y0)) {
            c.set_char(ch);
        }
    }

    let inner_x = x0 + 1;
    let inner_y = y0 + 1;
    let inner_w = x1.saturating_sub(inner_x);
    // Blank row above bottom border (ptkube).
    let inner_h = y1.saturating_sub(inner_y).saturating_sub(1);
    if inner_w == 0 || inner_h == 0 {
        return;
    }
    let cols = inner_w as usize;
    let rows = inner_h as usize;
    let body_w = cols.saturating_sub(1); // empty col before right border
    let start = recent.len().saturating_sub(rows);
    for (i, l) in recent.iter().skip(start).take(rows).enumerate() {
        let y = inner_y + i as u16;
        let body = truncate_cols(l, body_w);
        for (j, ch) in body.chars().enumerate() {
            let x = inner_x + j as u16;
            if x >= x1 {
                break;
            }
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_char(ch);
            }
        }
    }
}

/// Render node → network (IPs) → mem|services → logs.
pub fn render(frame: &mut Frame, snap: &StatusSnapshot, recent: &[String], _cols: u16) {
    let area = frame.area();
    let (node_h, net_h, mid_h) = top_box_heights(area.height);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(node_h),
            Constraint::Length(net_h),
            Constraint::Length(mid_h),
            Constraint::Min(4),
        ])
        .split(area);

    draw_node(frame, rows[0], snap);
    draw_network(frame, rows[1], snap);
    draw_mid(frame, rows[2], snap);
    draw_logs(frame, rows[3], recent);
}
