use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Clear, List, ListItem, Paragraph, Row, Table, Wrap};

use crate::app::{App, ProcMode};
use crate::collector::process::InspectInfo;
use crate::hits::{Hit, register_table_rows};
use crate::ui::widgets::bordered;
use crate::ui::{centered, popup_block};
use crate::utils::{format_bytes, format_percent, format_uptime};

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    match app.proc_mode {
        ProcMode::Live => draw_live(frame, app, area),
        ProcMode::History => draw_history(frame, app, area),
    }
}

fn draw_live(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = app.theme;
    let header = Row::new([
        "pid", "user", "cpu", "mem", "virt", "io r/w", "stat", "command",
    ])
    .style(theme.title_style());
    let rows: Vec<Row> = app
        .visible_procs()
        .map(|(_, p)| {
            let cpu_color = theme.usage_color(f64::from(p.cpu), 50.0, 90.0);
            let style = if p.is_zombie {
                Style::default().fg(theme.red)
            } else {
                Style::default()
            };
            Row::new([
                Cell::from(p.pid.to_string()),
                Cell::from(p.user.clone()),
                Cell::from(format!("{:>5.1}", p.cpu)).style(Style::default().fg(cpu_color)),
                Cell::from(format_bytes(p.mem)),
                Cell::from(format_bytes(p.virt)),
                Cell::from(format!(
                    "{}/{}",
                    format_bytes(p.io_read),
                    format_bytes(p.io_write)
                )),
                Cell::from(short_status(&p.status)),
                Cell::from(p.cmd.clone()),
            ])
            .style(style)
        })
        .collect();
    let filter = if app.filter.is_empty() {
        "off".into()
    } else {
        format!(
            "{} cpu>{} mem>{} user:{}",
            app.filter.text,
            app.filter
                .cpu_min
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".into()),
            app.filter
                .mem_min
                .map(format_bytes)
                .unwrap_or_else(|| "-".into()),
            app.filter.user.clone().unwrap_or_else(|| "-".into())
        )
    };
    let title = format!(
        "processes live  {}  sort:{}{}  filter:[{}]  a:actions  h:history  enter:inspect",
        app.visible_procs().count(),
        app.proc_sort.label(),
        if app.sort_desc { "↓" } else { "↑" },
        filter
    );
    let widths = [
        Constraint::Length(7),
        Constraint::Length(10),
        Constraint::Length(6),
        Constraint::Length(9),
        Constraint::Length(9),
        Constraint::Length(15),
        Constraint::Length(6),
        Constraint::Fill(1),
    ];
    let table = Table::new(rows, widths)
        .header(header)
        .block(bordered(theme, &title))
        .row_highlight_style(theme.highlight())
        .highlight_symbol("▌ ");
    let offset = app.proc_state.offset();
    let len = app.visible_procs().count();
    register_table_rows(&mut app.hits, area, offset, len, Some(Hit::CycleSort));
    frame.render_stateful_widget(table, area, &mut app.proc_state);
}

fn draw_history(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = app.theme;
    let split = Layout::vertical([Constraint::Percentage(72), Constraint::Percentage(28)]);
    let [table_area, leaks] = area.layout(&split);

    let header = Row::new([
        "name", "avg cpu", "max cpu", "avg mem", "max mem", "samples",
    ])
    .style(theme.title_style());
    let rows: Vec<Row> = app
        .history_rows
        .iter()
        .map(|r| {
            Row::new([
                Cell::from(r.name.clone()),
                Cell::from(format!("{:>6.1}%", r.avg_cpu)),
                Cell::from(format!("{:>6.1}%", r.max_cpu)),
                Cell::from(format_bytes(r.avg_mem)),
                Cell::from(format_bytes(r.max_mem)),
                Cell::from(r.samples.to_string()),
            ])
        })
        .collect();
    let title = format!(
        "process history  window:{}  n:next window  h:live",
        app.history_window()
    );
    let widths = [
        Constraint::Fill(2),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(12),
        Constraint::Length(12),
        Constraint::Length(8),
    ];
    let table = Table::new(rows, widths)
        .header(header)
        .block(bordered(theme, &title))
        .row_highlight_style(theme.highlight())
        .highlight_symbol("▌ ");
    let offset = app.history_state.offset();
    let len = app.history_rows.len();
    register_table_rows(&mut app.hits, table_area, offset, len, None);
    frame.render_stateful_widget(table, table_area, &mut app.history_state);

    let leak_lines: Vec<Line> = if app.leak_suspects.is_empty() {
        vec![Line::from(Span::styled(
            "no memory-growth suspects in this window",
            theme.muted_style(),
        ))]
    } else {
        app.leak_suspects
            .iter()
            .map(|l| {
                Line::from(vec![
                    Span::styled(format!("{:>7} ", l.pid), theme.muted_style()),
                    Span::styled(format!("{:<20}", l.name), Style::default().fg(theme.fg)),
                    Span::styled(
                        format!(
                            "  {} → {}  ({} samples)",
                            format_bytes(l.min_mem),
                            format_bytes(l.max_mem),
                            l.samples
                        ),
                        Style::default().fg(theme.yellow),
                    ),
                ])
            })
            .collect()
    };
    frame.render_widget(
        Paragraph::new(leak_lines).block(bordered(theme, "memory leak suspects")),
        leaks,
    );
}

pub fn draw_actions(frame: &mut Frame, app: &mut App, area: Rect) {
    let items = ["kill (SIGTERM)", "kill -9 (SIGKILL)", "renice", "inspect"];
    let list_items: Vec<ListItem> = items
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let marker = if i == app.action_cursor { "▸" } else { " " };
            let style = if i == app.action_cursor {
                app.theme.highlight()
            } else {
                Style::default().fg(app.theme.fg)
            };
            ListItem::new(format!(" {marker} {}. {label}", i + 1)).style(style)
        })
        .collect();
    let popup = centered(area, 36, 8);
    app.hits.push(area, Hit::OverlayDismiss);
    frame.render_widget(Clear, popup);
    let pid = app
        .selected_proc()
        .map(|p| format!("{} ({})", p.name, p.pid))
        .unwrap_or_default();
    let title = format!("actions {pid}");
    let list = List::new(list_items).block(popup_block(app, &title));
    let inner = popup_block(app, &title).inner(popup);
    for i in 0..items.len() {
        app.hits.push(
            Rect {
                x: inner.x,
                y: inner.y.saturating_add(i as u16),
                width: inner.width,
                height: 1,
            },
            Hit::OverlayAction(i),
        );
    }
    frame.render_widget(list, popup);
}

pub fn draw_inspect(frame: &mut Frame, app: &mut App, area: Rect, info: &InspectInfo) {
    let popup = centered(area, 78, 16);
    app.hits.push(area, Hit::OverlayDismiss);
    app.hits.push(popup, Hit::OverlayDismiss);
    frame.render_widget(Clear, popup);
    let title = format!("inspect {} ({})", info.name, info.pid);
    let block = popup_block(app, &title);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let kv = |k: &str, v: String| {
        Line::from(vec![
            Span::styled(format!("{k:<12}"), app.theme.muted_style()),
            Span::raw(v),
        ])
    };
    let text = vec![
        kv("user", info.user.clone()),
        kv("status", info.status.clone()),
        kv(
            "cpu/mem",
            format!(
                "{}  rss {}  virt {}",
                format_percent(f64::from(info.cpu)).trim(),
                format_bytes(info.mem),
                format_bytes(info.virt)
            ),
        ),
        kv(
            "parent",
            info.parent
                .map(|p| p.to_string())
                .unwrap_or_else(|| "—".into()),
        ),
        kv("runtime", format_uptime(info.run_time)),
        kv("exe", info.exe.clone()),
        kv("cwd", info.cwd.clone()),
        kv("cmd", info.cmd.clone()),
        kv(
            "open files",
            info.open_files
                .map(|n| n.to_string())
                .unwrap_or_else(|| "n/a".into()),
        ),
        kv("environ", format!("{} vars", info.environ_count)),
        Line::from(Span::styled(
            "esc to close",
            app.theme.muted_style().add_modifier(Modifier::ITALIC),
        )),
    ];
    frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), inner);
}

fn short_status(status: &str) -> String {
    let s = status.to_ascii_lowercase();
    if s.contains("zombie") {
        "zomb".into()
    } else if s.contains("run") {
        "run".into()
    } else if s.contains("sleep") {
        "slp".into()
    } else if s.contains("stop") {
        "stop".into()
    } else if s.contains("idle") {
        "idle".into()
    } else {
        status.chars().take(4).collect()
    }
}
