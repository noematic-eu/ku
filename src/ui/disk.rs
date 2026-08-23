use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Clear, Paragraph, Row, Table, Wrap};

use crate::app::App;
use crate::collector::disk::DiskAlertLevel;
use crate::hits::{Hit, register_table_rows};
use crate::ui::widgets::bordered;
use crate::ui::{centered, popup_block};
use crate::utils::{format_bytes, format_percent};

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = app.theme;
    let header = Row::new([
        "mount", "fs", "kind", "used", "free", "total", "%", "inodes", "bar",
    ])
    .style(theme.title_style())
    .bottom_margin(0);

    let rows: Vec<Row> = app
        .visible_disks()
        .map(|(_, d)| {
            let pct = d.used_pct();
            let level = app.disk_alert(pct);
            let color = match level {
                DiskAlertLevel::Critical => theme.red,
                DiskAlertLevel::Warning => theme.yellow,
                DiskAlertLevel::Ok => theme.green,
            };
            let filled = (pct / 6.25).round().clamp(0.0, 16.0) as usize;
            let bar: String = (0..16)
                .map(|i| if i < filled { '█' } else { '░' })
                .collect();
            let inodes = match d.inode_pct() {
                Some(p) => format_percent(p).trim().to_string(),
                None => "—".into(),
            };
            Row::new([
                Cell::from(d.mount.clone()),
                Cell::from(d.fs.clone()),
                Cell::from(d.kind.clone()),
                Cell::from(format_bytes(d.used)),
                Cell::from(format_bytes(d.available)),
                Cell::from(format_bytes(d.total)),
                Cell::from(format_percent(pct).trim().to_string())
                    .style(Style::default().fg(color)),
                Cell::from(inodes),
                Cell::from(bar).style(Style::default().fg(color)),
            ])
        })
        .collect();

    let title = format!(
        "disk  sort:{}{}  filter:{}  enter:detail",
        app.disk_sort.label(),
        if app.sort_desc { "↓" } else { "↑" },
        if app.disk_filter.is_empty() {
            "off".into()
        } else {
            app.disk_filter.clone()
        }
    );
    let widths = [
        Constraint::Fill(3),
        Constraint::Length(8),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(7),
        Constraint::Length(8),
        Constraint::Length(17),
    ];
    let table = Table::new(rows, widths)
        .header(header)
        .block(bordered(theme, &title))
        .row_highlight_style(theme.highlight())
        .highlight_symbol("▌ ");
    let offset = app.disk_state.offset();
    let len = app.visible_disks().count();
    register_table_rows(&mut app.hits, area, offset, len, Some(Hit::CycleSort));
    frame.render_stateful_widget(table, area, &mut app.disk_state);
}

pub fn draw_detail(frame: &mut Frame, app: &mut App, area: Rect, idx: usize) {
    let Some(disk) = app.snapshot.disks.get(idx) else {
        return;
    };
    let popup = centered(area, 72, 16);
    app.hits.push(area, Hit::OverlayDismiss);
    app.hits.push(popup, Hit::OverlayDismiss);
    frame.render_widget(Clear, popup);
    let title = format!("volume {}", disk.mount);
    let block = popup_block(app, &title);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let pct = disk.used_pct();
    let inodes = disk
        .inode_pct()
        .map(|p| {
            format!(
                "{}  ({p:.1}%)",
                format_bytes_pair(disk.inodes_used, disk.inodes_total)
            )
        })
        .unwrap_or_else(|| "n/a".into());
    let text = vec![
        Line::from(vec![
            Span::styled("device   ", app.theme.muted_style()),
            Span::raw(disk.name.clone()),
        ]),
        Line::from(vec![
            Span::styled("mount    ", app.theme.muted_style()),
            Span::raw(disk.mount.clone()),
        ]),
        Line::from(vec![
            Span::styled("fs       ", app.theme.muted_style()),
            Span::raw(format!("{}  ({})", disk.fs, disk.kind)),
        ]),
        Line::from(vec![
            Span::styled("size     ", app.theme.muted_style()),
            Span::raw(format!(
                "{} used / {} free / {} total  ({})",
                format_bytes(disk.used),
                format_bytes(disk.available),
                format_bytes(disk.total),
                format_percent(pct).trim()
            )),
        ]),
        Line::from(vec![
            Span::styled("inodes   ", app.theme.muted_style()),
            Span::raw(inodes),
        ]),
        Line::from(vec![
            Span::styled("flags    ", app.theme.muted_style()),
            Span::raw(format!(
                "removable={}  read_only={}",
                disk.removable, disk.read_only
            )),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "esc / enter to close",
            app.theme.muted_style().add_modifier(Modifier::ITALIC),
        )),
    ];
    frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), inner);
}

fn format_bytes_pair(used: Option<u64>, total: Option<u64>) -> String {
    match (used, total) {
        (Some(u), Some(t)) => format!("{u} / {t}"),
        _ => "—".into(),
    }
}
