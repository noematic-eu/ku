use ratatui::style::Style;
use ratatui::widgets::{Block, Borders, Gauge, Sparkline};

use crate::theme::Theme;

pub fn bordered<'a>(theme: Theme, title: &'a str) -> Block<'a> {
    Block::default()
        .title(format!(" {title} "))
        .title_style(theme.title_style())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
}

pub fn usage_gauge(
    label: String,
    ratio: f64,
    color: ratatui::style::Color,
    theme: Theme,
) -> Gauge<'static> {
    Gauge::default()
        .gauge_style(Style::default().fg(color).bg(theme.gauge_bg))
        .ratio(ratio.clamp(0.0, 1.0))
        .label(label)
        .use_unicode(true)
}

pub fn spark(data: &[u64], color: ratatui::style::Color) -> Sparkline<'_> {
    Sparkline::default()
        .data(data)
        .max(100)
        .style(Style::default().fg(color))
}
