mod dashboard;
mod disk;
mod growth;
mod help;
mod processes;
mod settings;
mod widgets;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Tabs};

use crate::app::{App, Overlay, View};
use crate::collector::AlertLevel;
use crate::hits::{Hit, tab_label, tab_rects};
use crate::utils::format_uptime;

pub fn draw(frame: &mut Frame, app: &mut App) {
    app.hits.clear();
    let theme = app.theme;
    let layout = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(5),
        Constraint::Length(1),
    ]);
    let [header, body, footer] = frame.area().layout(&layout);

    draw_header(frame, app, header);
    match app.view {
        View::Dashboard => dashboard::draw(frame, app, body),
        View::Disk => disk::draw(frame, app, body),
        View::Growth => growth::draw(frame, app, body),
        View::Processes => processes::draw(frame, app, body),
        View::Settings => settings::draw(frame, app, body),
        View::Help => help::draw(frame, app, body),
    }
    draw_footer(frame, app, footer);

    match app.overlay.clone() {
        Overlay::None => {}
        Overlay::ProcessActions => processes::draw_actions(frame, app, body),
        Overlay::ConfirmKill { pid, name, force } => {
            draw_confirm(
                frame,
                body,
                app,
                if force {
                    format!("SIGKILL {name} ({pid})? [y/n]")
                } else {
                    format!("SIGTERM {name} ({pid})? [y/n]")
                },
            );
        }
        Overlay::Inspect(info) => processes::draw_inspect(frame, app, body, &info),
        Overlay::Renice { pid, buf } => {
            draw_prompt(frame, body, app, format!("renice {pid}"), &buf);
        }
        Overlay::DiskDetail(idx) => disk::draw_detail(frame, app, body, idx),
    }

    let _ = theme;
}

fn draw_header(frame: &mut Frame, app: &mut App, area: Rect) {
    let titles: Vec<Line> = View::ALL
        .iter()
        .enumerate()
        .map(|(i, v)| Line::from(tab_label(i, *v)))
        .collect();
    let selected = View::ALL.iter().position(|v| *v == app.view).unwrap_or(0);
    let tabs = Tabs::new(titles)
        .select(selected)
        .highlight_style(
            Style::default()
                .fg(app.theme.highlight_fg)
                .add_modifier(Modifier::BOLD),
        )
        .style(app.theme.muted_style())
        .divider(Span::styled("│", app.theme.muted_style()));

    let host = if app.snapshot.hostname.is_empty() {
        "…"
    } else {
        app.snapshot.hostname.as_str()
    };
    let title = Line::from(vec![
        Span::styled(" ku ", app.theme.title_style()),
        Span::styled(host, Style::default().fg(app.theme.fg)),
        Span::raw("  "),
        Span::styled(
            format_uptime(app.snapshot.uptime_secs),
            app.theme.muted_style(),
        ),
        Span::raw("  "),
        Span::styled(
            app.snapshot.collected_at.format("%H:%M:%S").to_string(),
            app.theme.muted_style(),
        ),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(title);
    let inner = block.inner(area);
    for (rect, view) in tab_rects(inner) {
        app.hits.push(rect, Hit::Tab(view));
    }
    frame.render_widget(block, area);
    frame.render_widget(tabs, inner);
}

fn draw_footer(frame: &mut Frame, app: &mut App, area: Rect) {
    let mut spans = vec![
        Span::styled(" tab ", app.theme.muted_style()),
        Span::raw("views  "),
        Span::styled("j/k ", app.theme.muted_style()),
        Span::raw("move  "),
        Span::styled("/ ", app.theme.muted_style()),
        Span::raw("filter  "),
        Span::styled("s ", app.theme.muted_style()),
        Span::raw("sort  "),
        Span::styled("? ", app.theme.muted_style()),
        Span::raw("help  "),
        Span::styled("q ", app.theme.muted_style()),
        Span::raw("quit"),
    ];
    if app.filter_editing {
        spans = vec![
            Span::styled(" filter: ", app.theme.title_style()),
            Span::styled(
                format!("{}_", app.filter_input),
                Style::default().fg(app.theme.fg),
            ),
            Span::styled("  enter apply  esc cancel", app.theme.muted_style()),
        ];
    } else if !app.status_line().is_empty() {
        spans.push(Span::raw("  │  "));
        spans.push(Span::styled(
            app.status_line(),
            Style::default().fg(app.theme.accent),
        ));
    } else {
        let crit = app
            .alerts()
            .iter()
            .filter(|a| a.level == AlertLevel::Critical)
            .count();
        let warn = app.alerts().len().saturating_sub(crit);
        if crit + warn > 0 {
            spans.push(Span::raw("  │  "));
            if crit > 0 {
                spans.push(Span::styled(
                    format!("{crit} critical"),
                    Style::default()
                        .fg(app.theme.red)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            if warn > 0 {
                if crit > 0 {
                    spans.push(Span::raw("  "));
                }
                spans.push(Span::styled(
                    format!("{warn} warning"),
                    Style::default().fg(app.theme.yellow),
                ));
            }
        }
    }
    let help = " help ";
    let quit = " quit ";
    let right_w = (help.len() + quit.len()) as u16;
    if area.width > right_w + 8 {
        let split = Layout::horizontal([Constraint::Min(8), Constraint::Length(right_w)]);
        let [left, right] = area.layout(&split);
        let help_rect = Rect {
            x: right.x,
            y: right.y,
            width: help.len() as u16,
            height: 1,
        };
        let quit_rect = Rect {
            x: right.x.saturating_add(help.len() as u16),
            y: right.y,
            width: quit.len() as u16,
            height: 1,
        };
        app.hits.push(help_rect, Hit::FooterHelp);
        app.hits.push(quit_rect, Hit::FooterQuit);
        frame.render_widget(Paragraph::new(Line::from(spans)), left);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(help, app.theme.title_style()),
                Span::styled(quit, Style::default().fg(app.theme.red)),
            ])),
            right,
        );
    } else {
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }
}

pub fn popup_block<'a>(app: &App, title: &'a str) -> Block<'a> {
    Block::default()
        .title(Span::styled(format!(" {title} "), app.theme.title_style()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.accent))
}

pub fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2).max(1));
    let height = height.min(area.height.saturating_sub(2).max(1));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn draw_confirm(frame: &mut Frame, area: Rect, app: &mut App, text: String) {
    let popup = centered(area, (text.len() as u16).saturating_add(8).min(72), 7);
    app.hits.push(area, Hit::OverlayNo);
    frame.render_widget(Clear, popup);
    let block = popup_block(app, "confirm");
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let split = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ]);
    let [msg, _, btns] = inner.layout(&split);
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(app.theme.yellow)),
        msg,
    );
    let btn_split = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]);
    let [yes, no] = btns.layout(&btn_split);
    app.hits.push(popup, Hit::OverlayDismiss);
    app.hits.push(yes, Hit::OverlayYes);
    app.hits.push(no, Hit::OverlayNo);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " [y] yes ",
            app.theme.title_style(),
        ))),
        yes,
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " [n] cancel ",
            Style::default().fg(app.theme.red),
        ))),
        no,
    );
}

fn draw_prompt(frame: &mut Frame, area: Rect, app: &mut App, title: String, buf: &str) {
    let popup = centered(area, 40, 5);
    app.hits.push(area, Hit::OverlayDismiss);
    frame.render_widget(Clear, popup);
    let block = popup_block(app, &title);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    frame.render_widget(Paragraph::new(format!("{buf}_")), inner);
    app.hits.push(popup, Hit::OverlayDismiss);
}
