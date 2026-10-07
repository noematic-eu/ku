use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use crate::app::App;
use crate::collector::AlertLevel;
use crate::collector::disk::DiskAlertLevel;
use crate::ui::widgets::{bordered, spark, usage_gauge};
use crate::utils::{format_bytes, format_percent, format_uptime};

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = app.theme;
    let snap = &app.snapshot;
    let layout = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(5),
        Constraint::Min(6),
    ]);
    let [cpu_row, mem_row, meta_row, bottom] = area.layout(&layout);

    let cpu_pct = f64::from(snap.cpu.global);
    let cpu_color = theme.usage_color(cpu_pct, 70.0, 90.0);
    draw_usage(
        frame,
        cpu_row,
        theme,
        "cpu",
        format!("{:>5}  {}", format_percent(cpu_pct).trim(), snap.cpu.brand),
        cpu_pct,
        cpu_color,
        &snap.cpu_history,
    );

    let mem_pct = snap.memory.used_pct();
    let mem_color = theme.usage_color(mem_pct, 80.0, 95.0);
    draw_usage(
        frame,
        mem_row,
        theme,
        "memory",
        format!(
            "{}  {} / {}   swap {} / {}",
            format_percent(mem_pct).trim(),
            format_bytes(snap.memory.used),
            format_bytes(snap.memory.total),
            format_bytes(snap.memory.swap_used),
            format_bytes(snap.memory.swap_total),
        ),
        mem_pct,
        mem_color,
        &snap.mem_history,
    );

    let cores: Vec<Line> = snap
        .cpu
        .cores
        .iter()
        .map(|c| {
            let pct = f64::from(c.usage);
            let color = theme.usage_color(pct, 70.0, 90.0);
            let filled = (pct / 10.0).round() as usize;
            let bar: String = (0..10)
                .map(|i| if i < filled { '█' } else { '░' })
                .collect();
            Line::from(vec![
                Span::styled(format!("{:>3} ", c.name), theme.muted_style()),
                Span::styled(bar, Style::default().fg(color)),
                Span::styled(format!(" {:>4.0}%", c.usage), Style::default().fg(color)),
            ])
        })
        .collect();
    let meta = bordered(theme, "load / host");
    let meta_inner = meta.inner(meta_row);
    frame.render_widget(meta, meta_row);
    let meta_split = Layout::horizontal([Constraint::Percentage(42), Constraint::Percentage(58)]);
    let [load_area, core_area] = meta_inner.layout(&meta_split);
    let load = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("load  ", theme.muted_style()),
            Span::raw(format!(
                "{:.2}  {:.2}  {:.2}",
                snap.load.one, snap.load.five, snap.load.fifteen
            )),
        ]),
        Line::from(vec![
            Span::styled("up    ", theme.muted_style()),
            Span::raw(format_uptime(snap.uptime_secs)),
            Span::styled("   procs ", theme.muted_style()),
            Span::raw(snap.process_count.to_string()),
            Span::styled("   zomb ", theme.muted_style()),
            Span::styled(
                snap.zombie_count.to_string(),
                if snap.zombie_count > 0 {
                    Style::default().fg(theme.red)
                } else {
                    Style::default().fg(theme.fg)
                },
            ),
        ]),
        Line::from(vec![
            Span::styled("os    ", theme.muted_style()),
            Span::raw(snap.os.clone()),
        ]),
    ]);
    frame.render_widget(load, load_area);
    frame.render_widget(Paragraph::new(cores).wrap(Wrap { trim: true }), core_area);

    if app.focused {
        draw_disk_preview(frame, app, bottom);
    } else {
        let bottom_split =
            Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)]);
        let [disks, alerts]: [Rect; 2] = bottom.layout(&bottom_split);
        draw_disk_preview(frame, app, disks);
        draw_alerts(frame, app, alerts);
    }
}

/// Gauge stays under half the row. A `│` sits in the middle, and the history
/// uses every remaining column so it meets the right border.
fn draw_usage(
    frame: &mut Frame,
    area: Rect,
    theme: crate::theme::Theme,
    title: &str,
    label: String,
    pct: f64,
    color: Color,
    history: &[u64],
) {
    let block = bordered(theme, title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let gauge_w = inner.width.saturating_sub(1) / 2;
    if gauge_w == 0 || inner.height == 0 {
        return;
    }
    let split = Layout::horizontal([
        Constraint::Length(gauge_w),
        Constraint::Length(1),
        Constraint::Fill(1),
    ]);
    let [gauge_area, sep_area, spark_area] = inner.layout(&split);
    frame.render_widget(usage_gauge(label, pct / 100.0, color, theme), gauge_area);
    frame.render_widget(
        Paragraph::new(Span::styled("│", Style::default().fg(theme.border))),
        sep_area,
    );
    draw_history(frame, spark_area, history, color);
}

fn draw_history(frame: &mut Frame, area: Rect, data: &[u64], color: Color) {
    if area.width == 0 || area.height == 0 || data.is_empty() {
        return;
    }
    let width = area.width as usize;
    let visible = &data[data.len().saturating_sub(width)..];
    let n = visible.len() as u16;
    let spark_area = Rect {
        x: area.x + area.width - n,
        y: area.y,
        width: n,
        height: area.height,
    };
    frame.render_widget(spark(visible, color), spark_area);
}

fn draw_disk_preview(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = app.theme;
    let block = bordered(theme, "volumes");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    for i in 0..app.snapshot.disks.len().min(inner.height as usize) {
        app.hits.push(
            Rect {
                x: inner.x,
                y: inner.y.saturating_add(i as u16),
                width: inner.width,
                height: 1,
            },
            crate::hits::Hit::DashDisk(i),
        );
    }
    let lines: Vec<Line> = app
        .snapshot
        .disks
        .iter()
        .take(inner.height as usize)
        .map(|d| {
            let pct = d.used_pct();
            let level = app.disk_alert(pct);
            let color = match level {
                DiskAlertLevel::Critical => theme.red,
                DiskAlertLevel::Warning => theme.yellow,
                DiskAlertLevel::Ok => theme.green,
            };
            let filled = (pct / 5.0).round().clamp(0.0, 20.0) as usize;
            let bar: String = (0..20)
                .map(|i| if i < filled { '━' } else { '─' })
                .collect();
            Line::from(vec![
                Span::styled(
                    format!("{:<16}", trunc(&d.mount, 16)),
                    Style::default().fg(theme.fg),
                ),
                Span::styled(
                    format!(" {:>5}", format_percent(pct).trim()),
                    Style::default().fg(color),
                ),
                Span::raw(" "),
                Span::styled(bar, Style::default().fg(color)),
                Span::styled(
                    format!("  {} / {}", format_bytes(d.used), format_bytes(d.total)),
                    theme.muted_style(),
                ),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_alerts(frame: &mut Frame, app: &App, area: Rect) {
    let theme = app.theme;
    let block = bordered(theme, "alerts");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if app.alerts().is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "no active alerts",
                theme.muted_style(),
            ))),
            inner,
        );
        return;
    }
    let lines: Vec<Line> = app
        .alerts()
        .iter()
        .map(|a| {
            let (tag, color) = match a.level {
                AlertLevel::Critical => ("CRIT", theme.red),
                AlertLevel::Warning => ("WARN", theme.yellow),
            };
            Line::from(vec![
                Span::styled(
                    format!(" {tag} "),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::raw(a.message.clone()),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn trunc(s: &str, n: usize) -> String {
    crate::utils::truncate_ellipsis(s, n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::config::Config;
    use crate::storage::Storage;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn harness() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let db = Storage::open(&dir.path().join("ku.db")).unwrap();
        let app = App::new(
            Config::default(),
            dir.path().join("config.toml"),
            dir.path().to_path_buf(),
            db,
        );
        (dir, app)
    }

    #[test]
    fn usage_bars_reach_the_right_edge() {
        let (_dir, mut app) = harness();
        app.snapshot.cpu.global = 100.0;
        app.snapshot.cpu.brand = "cpu".into();
        app.snapshot.memory.total = 1;
        app.snapshot.memory.used = 1;
        app.snapshot.cpu_history = vec![10, 100];
        app.snapshot.mem_history = vec![100];
        let width = 80u16;
        let backend = TestBackend::new(width, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw(frame, &mut app, frame.area()))
            .unwrap();
        let buf = terminal.backend().buffer();
        let inner_w = width - 2;
        let gauge_bg = app.theme.gauge_bg;
        for y in [1u16, 4] {
            let sep = (1..width - 1)
                .find(|x| buf[(*x, y)].symbol() == "│")
                .unwrap_or_else(|| panic!("missing separator on row {y}"));
            let gauge_w = sep - 1;
            assert!(
                gauge_w * 2 < inner_w,
                "gauge width {gauge_w} is not under half of {inner_w}"
            );
            let gauge_end = &buf[(sep - 1, y)];
            assert_eq!(gauge_end.symbol(), "█", "gauge stops before the separator");
            assert_eq!(gauge_end.bg, gauge_bg);
            assert_ne!(buf[(width - 2, y)].symbol(), " ", "history stops early");
        }
    }
}
