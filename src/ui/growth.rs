use chrono::{Local, TimeZone};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table};

use crate::app::App;
use crate::hits::{Hit, register_table_rows};
use crate::ui::widgets::bordered;
use crate::utils::{format_bytes, format_bytes_signed};

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = app.theme;
    let split = Layout::vertical([Constraint::Length(3), Constraint::Min(3)]);
    let [info, table_area] = area.layout(&split);

    let last = app
        .last_growth_ts
        .and_then(|ts| Local.timestamp_opt(ts, 0).single())
        .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| "never".into());
    let hint = if app.growth_rows.is_empty() {
        "waiting for two snapshots — first scan runs in the background (default 5 min)"
    } else {
        "h / [ ] cycle window   r reload   / filter"
    };
    let info_line = Paragraph::new(vec![Line::from(vec![
        Span::styled("window ", theme.muted_style()),
        Span::styled(app.growth_window.label(), theme.title_style()),
        Span::styled("   last scan ", theme.muted_style()),
        Span::raw(last),
        Span::styled("   paths ", theme.muted_style()),
        Span::raw(app.config.disk.watched_paths.len().to_string()),
        Span::styled(format!("   {hint}"), theme.muted_style()),
    ])])
    .block(bordered(theme, "growth tracking"));
    app.hits.push(info, Hit::GrowthWindow);
    frame.render_widget(info_line, info);

    let header = Row::new(["path", "size", "Δ abs", "Δ rel", "trend"]).style(theme.title_style());
    let q = app.disk_filter.clone();
    let rows: Vec<Row> = app
        .growth_rows
        .iter()
        .filter(|r| q.is_empty() || r.path.to_ascii_lowercase().contains(&q))
        .map(|r| {
            let color = if r.is_new {
                theme.accent
            } else if r.abs_delta > 0 {
                theme.red
            } else if r.abs_delta < 0 {
                theme.green
            } else {
                theme.muted
            };
            let rel = r
                .rel_delta
                .map(|v| format!("{v:+.1}%"))
                .unwrap_or_else(|| if r.is_new { "new".into() } else { "—".into() });
            let spark = sparkline_delta(r.abs_delta);
            Row::new([
                Cell::from(r.path.clone()),
                Cell::from(format_bytes(r.size)),
                Cell::from(format_bytes_signed(r.abs_delta)).style(Style::default().fg(color)),
                Cell::from(rel).style(Style::default().fg(color)),
                Cell::from(spark).style(Style::default().fg(color)),
            ])
        })
        .collect();
    let shown = rows.len();

    let widths = [
        Constraint::Fill(4),
        Constraint::Length(12),
        Constraint::Length(12),
        Constraint::Length(10),
        Constraint::Length(18),
    ];
    let table_title = format!("top movers over {}", app.growth_window.label());
    let table = Table::new(rows, widths)
        .header(header)
        .block(bordered(theme, &table_title))
        .row_highlight_style(theme.highlight())
        .highlight_symbol("▌ ");
    let offset = app.growth_state.offset();
    register_table_rows(
        &mut app.hits,
        table_area,
        offset,
        shown,
        Some(Hit::GrowthWindow),
    );
    frame.render_stateful_widget(table, table_area, &mut app.growth_state);
}

fn sparkline_delta(delta: i64) -> String {
    if delta == 0 {
        return "────────".into();
    }
    let mag = (delta.abs() as f64).log10().clamp(0.0, 8.0) as usize + 1;
    let ch = if delta > 0 { '▲' } else { '▼' };
    std::iter::repeat_n(ch, mag.min(12)).collect()
}
