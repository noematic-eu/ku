use chrono::{Local, TimeZone};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table};

use ratatui::widgets::{Clear, Wrap};

use ratatui::widgets::TableState;

use crate::app::{App, GrowthMode};
use crate::collector::growth::{ContribChange, GrowthExplain};
use crate::hits::{Hit, register_table_rows};
use crate::orphans::{self, DeleteFail, OrphanRow};
use crate::ui::widgets::bordered;
use crate::ui::{centered, popup_block};
use crate::utils::{format_bytes, format_bytes_signed};

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.growth_mode == GrowthMode::Orphans {
        draw_orphans(frame, app, area);
        return;
    }
    if app.growth_mode == GrowthMode::Allowlist {
        draw_allowlist(frame, app, area);
        return;
    }
    let theme = app.theme;
    let split = Layout::vertical([Constraint::Length(3), Constraint::Min(3)]);
    let [info, table_area] = area.layout(&split);

    let last = app
        .last_growth_ts
        .and_then(|ts| Local.timestamp_opt(ts, 0).single())
        .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| "never".into());
    let hint = if app.growth_rows.is_empty() {
        "waiting for two snapshots — first scan runs in the background (default 5 min)".into()
    } else {
        format!(
            "enter why   e {}   t {}   h / [ ] window   r reload   / filter   o orphans",
            crate::utils::reveal_shortcut_label(),
            app.growth_limit.label(),
        )
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
    let rows: Vec<Row> = app
        .visible_growth_rows()
        .into_iter()
        .map(|r| {
            let color = if r.is_gone {
                theme.green
            } else if r.is_new {
                theme.accent
            } else if r.abs_delta > 0 {
                theme.red
            } else if r.abs_delta < 0 {
                theme.green
            } else {
                theme.muted
            };
            let rel = if r.is_gone {
                "gone".into()
            } else if r.is_new {
                "new".into()
            } else {
                r.rel_delta
                    .map(|v| format!("{v:+.1}%"))
                    .unwrap_or_else(|| "—".into())
            };
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
    let total = app.growth_rows.len();
    let table_title = format!(
        "{} movers over {}  ({shown}/{total})  t to cycle",
        app.growth_limit.label(),
        app.growth_window.label(),
    );
    let table = Table::new(rows, widths)
        .header(header)
        .block(bordered(theme, &table_title))
        .row_highlight_style(theme.highlight())
        .highlight_symbol("▌ ");
    let offset = app.growth_state.offset();
    app.list_viewport_rows = register_table_rows(
        &mut app.hits,
        table_area,
        offset,
        shown,
        Some(Hit::GrowthLimit),
    );
    frame.render_stateful_widget(table, table_area, &mut app.growth_state);
}

fn draw_orphans(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = app.theme;
    let split = Layout::vertical([Constraint::Length(3), Constraint::Min(3)]);
    let [info, table_area] = area.layout(&split);
    let deleted = app.orphan_rows.iter().filter(|r| r.deleted).count();
    let remaining = app.orphan_rows.len().saturating_sub(deleted);
    let ignored = app.config.orphans.ignore.len();
    let count = if deleted == 0 && ignored == 0 {
        format!("{remaining} leftover path(s)")
    } else if ignored == 0 {
        format!("{remaining} leftover  ·  {deleted} deleted")
    } else {
        format!("{remaining} leftover  ·  {ignored} ignored")
    };
    let mut info_spans = vec![
        Span::styled("orphans ", theme.title_style()),
        Span::raw(count),
        Span::raw("  "),
    ];
    if app.orphan_scanning {
        info_spans.extend(crate::ui::widgets::busy_spans(
            app.anim_tick,
            "scanning leftover app data",
            Some("esc cancel"),
            theme.muted_style(),
        ));
    } else if orphans::fda_missing() {
        info_spans.push(Span::styled(
            format!(
                "Full Disk Access off — add {}  ·  F opens Settings",
                orphans::fda_app_hint()
            ),
            Style::default().fg(theme.yellow),
        ));
    } else if app.orphan_rows.is_empty() {
        info_spans.push(Span::styled(
            "no leftovers found  ·  o movers  u allowlist  r rescan",
            theme.muted_style(),
        ));
    } else {
        info_spans.push(Span::styled(
            format!(
                "o movers  e {}  i ignore  I app  u allowlist  d delete  a all related",
                crate::utils::reveal_shortcut_label()
            ),
            theme.muted_style(),
        ));
    }
    let info_line = Paragraph::new(vec![Line::from(info_spans)])
        .block(bordered(theme, "growth · leftover app data"));
    frame.render_widget(info_line, info);

    let q = app.disk_filter.clone();
    let rows: Vec<Row> = app
        .orphan_rows
        .iter()
        .filter(|r| {
            q.is_empty()
                || r.path.to_string_lossy().to_ascii_lowercase().contains(&q)
                || r.app_id.to_ascii_lowercase().contains(&q)
                || r.app_name.to_ascii_lowercase().contains(&q)
        })
        .map(|r| {
            let color = match r.confidence {
                crate::orphans::Confidence::High => theme.red,
                crate::orphans::Confidence::Medium => theme.yellow,
                crate::orphans::Confidence::Low => theme.muted,
            };
            let style = if r.deleted {
                Style::default()
                    .fg(theme.muted)
                    .add_modifier(Modifier::CROSSED_OUT)
            } else {
                Style::default()
            };
            let conf = if r.deleted {
                "done"
            } else {
                r.confidence.label()
            };
            Row::new([
                Cell::from(conf.to_string()).style(if r.deleted {
                    style
                } else {
                    Style::default().fg(color)
                }),
                Cell::from(crate::utils::truncate_ellipsis(&r.app_id, 28)),
                Cell::from(format_bytes(r.size)),
                Cell::from(crate::utils::format_mtime_ago(r.mtime)),
                Cell::from(r.path.display().to_string()),
            ])
            .style(style)
        })
        .collect();
    let shown = rows.len();
    let header = Row::new(["conf", "app", "size", "age", "path"]).style(theme.title_style());
    let widths = [
        Constraint::Length(8),
        Constraint::Length(24),
        Constraint::Length(10),
        Constraint::Length(6),
        Constraint::Fill(4),
    ];
    let dir = match (app.orphan_sort, app.orphan_sort_rev) {
        (crate::orphans::OrphanSort::Age, false) => "oldest",
        (crate::orphans::OrphanSort::Age, true) => "newest",
        (crate::orphans::OrphanSort::Level, false) => "high-first",
        (crate::orphans::OrphanSort::Level, true) => "low-first",
        (_, false) => "largest",
        (_, true) => "smallest",
    };
    let title = format!(
        "leftovers  sort:{} ({dir})  s cycle  S reverse",
        app.orphan_sort.label(),
    );
    let table = Table::new(rows, widths)
        .header(header)
        .block(bordered(theme, &title))
        .row_highlight_style(theme.highlight())
        .highlight_symbol("▌ ");
    let offset = app.growth_state.offset();
    app.list_viewport_rows = register_table_rows(
        &mut app.hits,
        table_area,
        offset,
        shown,
        Some(Hit::CycleSort),
    );
    frame.render_stateful_widget(table, table_area, &mut app.growth_state);
}

fn draw_allowlist(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = app.theme;
    let split = Layout::vertical([Constraint::Length(3), Constraint::Min(3)]);
    let [info, table_area] = area.layout(&split);
    let n = app.config.orphans.ignore.len();
    let mut info_spans = vec![
        Span::styled("allowlist ", theme.title_style()),
        Span::raw(format!("{n} ignored")),
        Span::raw("  "),
    ];
    if n == 0 {
        info_spans.push(Span::styled(
            "empty  ·  i on a leftover to hide it  ·  u leftovers",
            theme.muted_style(),
        ));
    } else {
        info_spans.push(Span::styled(
            "enter/d/x remove  ·  c clear all  ·  u leftovers",
            theme.muted_style(),
        ));
    }
    frame.render_widget(
        Paragraph::new(vec![Line::from(info_spans)])
            .block(bordered(theme, "growth · ignored leftovers")),
        info,
    );

    let rows: Vec<Row> = app
        .config
        .orphans
        .ignore
        .iter()
        .map(|raw| {
            let (kind, value) = match orphans::IgnoreRule::parse(raw) {
                Some(rule) => (rule.kind_label().to_string(), rule.display()),
                None => ("raw".into(), raw.clone()),
            };
            Row::new([
                Cell::from(kind).style(theme.title_style()),
                Cell::from(value),
            ])
        })
        .collect();
    let shown = rows.len();
    let header = Row::new(["kind", "rule"]).style(theme.title_style());
    let table = Table::new(rows, [Constraint::Length(8), Constraint::Fill(1)])
        .header(header)
        .block(bordered(
            theme,
            "persisted in config.toml  [orphans] ignore",
        ))
        .row_highlight_style(theme.highlight())
        .highlight_symbol("▌ ");
    let offset = app.growth_state.offset();
    app.list_viewport_rows = register_table_rows(&mut app.hits, table_area, offset, shown, None);
    frame.render_stateful_widget(table, table_area, &mut app.growth_state);
}

pub fn draw_explain(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    expl: &GrowthExplain,
    depth: usize,
) {
    let h = (10u16.saturating_add(expl.rows.len() as u16).min(28))
        .min(area.height.saturating_sub(2))
        .max(12);
    let popup = centered(area, 92, h);
    app.hits.push(area, Hit::OverlayDismiss);
    app.hits.push(popup, Hit::OverlayDismiss);
    frame.render_widget(Clear, popup);
    let title = format!("why  {}", expl.path);
    let block = popup_block(app, &title);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let split = Layout::vertical([Constraint::Length(4), Constraint::Min(5)]);
    let [info_area, table_area] = inner.layout(&split);
    let parent_delta = format_bytes_signed(expl.abs_delta);
    let back = if depth > 1 { "esc back" } else { "esc close" };
    let caption = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("size ", app.theme.muted_style()),
            Span::raw(format_bytes(expl.size)),
            Span::styled("   Δ ", app.theme.muted_style()),
            Span::raw(parent_delta),
            Span::styled(
                format!("   {} child(ren)", expl.rows.len()),
                app.theme.muted_style(),
            ),
        ]),
        Line::from(Span::styled(expl.source.caption(), app.theme.muted_style())),
        Line::from(Span::styled(
            format!(
                "enter drill into dir  ·  e {}  ·  {back}",
                crate::utils::reveal_shortcut_label()
            ),
            app.theme.muted_style(),
        )),
    ]);
    frame.render_widget(caption, info_area);

    let header = Row::new(["type", "name", "size", "Δ", "why"]).style(app.theme.title_style());
    let rows: Vec<Row> = expl
        .rows
        .iter()
        .map(|r| {
            let color = match r.change {
                ContribChange::New | ContribChange::Grew => app.theme.red,
                ContribChange::Gone | ContribChange::Shrunk => app.theme.green,
                ContribChange::Now => app.theme.accent,
                ContribChange::Unchanged => app.theme.muted,
            };
            let kind = r.kind.map(|k| k.label()).unwrap_or("?");
            let delta = r
                .abs_delta
                .map(format_bytes_signed)
                .unwrap_or_else(|| "—".into());
            Row::new([
                Cell::from(kind.to_string()),
                Cell::from(r.name.clone()),
                Cell::from(format_bytes(r.size)),
                Cell::from(delta).style(Style::default().fg(color)),
                Cell::from(r.change.label().to_string()).style(Style::default().fg(color)),
            ])
        })
        .collect();
    let widths = [
        Constraint::Length(6),
        Constraint::Fill(4),
        Constraint::Length(12),
        Constraint::Length(12),
        Constraint::Length(8),
    ];
    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(app.theme.highlight())
        .highlight_symbol("▌ ");
    let mut state = TableState::default().with_selected(Some(expl.selected));
    frame.render_stateful_widget(table, table_area, &mut state);
}

pub fn draw_orphan_inspect(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    row: &OrphanRow,
    related: usize,
) {
    let popup = centered(area, 78, 15);
    app.hits.push(area, Hit::OverlayDismiss);
    app.hits.push(popup, Hit::OverlayDismiss);
    frame.render_widget(Clear, popup);
    let title = format!("orphan {} ({})", row.app_name, row.app_id);
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
        kv("path", row.path.display().to_string()),
        kv("size", format_bytes(row.size)),
        kv("age", crate::utils::format_mtime_ago(row.mtime)),
        kv("confidence", row.confidence.label().into()),
        kv("reason", row.reason.clone()),
        kv(
            "related",
            format!("{related} leftover path(s) for this app"),
        ),
        kv(
            "status",
            if row.deleted {
                "deleted".into()
            } else {
                "present".into()
            },
        ),
        Line::from(""),
        Line::from(Span::styled(
            if row.deleted {
                format!(
                    "already deleted  ·  e {}  ·  w google  ·  i ignore  ·  esc close",
                    crate::utils::reveal_shortcut_label()
                )
            } else {
                format!(
                    "e {} · i ignore path · I ignore app · w google · d delete · a all related · esc close",
                    crate::utils::reveal_shortcut_label()
                )
            },
            app.theme.muted_style(),
        )),
    ];
    frame.render_widget(
        ratatui::widgets::Paragraph::new(text).wrap(Wrap { trim: true }),
        inner,
    );
}

pub fn draw_orphan_delete_report(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    removed: usize,
    failed: &[DeleteFail],
    hint_sudo: bool,
) {
    let extra = if orphans::fda_missing() || orphans::running_as_root() {
        3
    } else {
        2
    };
    let lines_n = extra + 6 + failed.len().min(10) as u16;
    let popup = centered(area, 78, lines_n.min(area.height.saturating_sub(2)).max(8));
    app.hits.push(area, Hit::OverlayDismiss);
    app.hits.push(popup, Hit::OverlayDismiss);
    frame.render_widget(Clear, popup);
    let block = popup_block(app, "incomplete delete");
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let mut lines = vec![
        Line::from(vec![
            Span::styled("removed  ", app.theme.muted_style()),
            Span::raw(format!("{removed} path(s)")),
        ]),
        Line::from(vec![
            Span::styled("failed   ", app.theme.muted_style()),
            Span::styled(
                format!("{} path(s)", failed.len()),
                Style::default()
                    .fg(app.theme.red)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
    ];
    for f in failed.iter().take(10) {
        let why = if f.permission {
            "permission denied"
        } else {
            f.error.as_str()
        };
        lines.push(Line::from(vec![
            Span::styled("  • ", Style::default().fg(app.theme.red)),
            Span::raw(crate::utils::truncate_ellipsis(
                &f.path.display().to_string(),
                48,
            )),
            Span::styled(format!("  {why}"), app.theme.muted_style()),
        ]));
    }
    if failed.len() > 10 {
        lines.push(Line::from(Span::styled(
            format!("  … {} more", failed.len() - 10),
            app.theme.muted_style(),
        )));
    }
    lines.push(Line::from(""));
    if orphans::fda_missing() {
        lines.push(Line::from(Span::styled(
            format!(
                "macOS Full Disk Access is off. Add {} in Privacy, then relaunch ku.",
                orphans::fda_app_hint()
            ),
            Style::default().fg(app.theme.yellow),
        )));
        lines.push(Line::from(Span::styled(
            "sudo does not bypass this. f opens System Settings.",
            app.theme.muted_style(),
        )));
    } else if orphans::running_as_root() {
        lines.push(Line::from(Span::styled(
            "Already root. e retries after clearing flags (chflags + rm).",
            Style::default().fg(app.theme.yellow),
        )));
        lines.push(Line::from(Span::styled(
            "If it stays blocked: Full Disk Access (F) or delete in Finder.",
            app.theme.muted_style(),
        )));
    } else if hint_sudo {
        let how = orphans::elevate_detect()
            .map(|m| m.label())
            .unwrap_or("sudo ku");
        lines.push(Line::from(Span::styled(
            format!("Often access rights. Press e to elevate ({how})"),
            Style::default().fg(app.theme.yellow),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "Some files could not be removed. e retries.",
            app.theme.muted_style(),
        )));
    }
    let actions = if orphans::fda_needed() {
        if orphans::running_as_root() {
            "e retry  ·  f Full Disk Access  ·  esc / enter close"
        } else {
            "e elevate  ·  f Full Disk Access  ·  esc / enter close"
        }
    } else if orphans::running_as_root() {
        "e retry  ·  esc / enter close"
    } else {
        "e elevate  ·  esc / enter close"
    };
    lines.push(Line::from(Span::styled(actions, app.theme.muted_style())));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
    let footer = Rect {
        x: inner.x,
        y: inner.y.saturating_add(inner.height.saturating_sub(1)),
        width: inner.width,
        height: 1,
    };
    app.hits.push(footer, Hit::OverlayYes);
}

fn sparkline_delta(delta: i64) -> String {
    if delta == 0 {
        return "────────".into();
    }
    let mag = (delta.abs() as f64).log10().clamp(0.0, 8.0) as usize + 1;
    let ch = if delta > 0 { '▲' } else { '▼' };
    std::iter::repeat_n(ch, mag.min(12)).collect()
}
