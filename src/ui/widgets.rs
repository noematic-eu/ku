use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Gauge, Sparkline};

use crate::theme::Theme;

const SPIN: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const SPIN_COLORS: &[Color] = &[
    Color::Cyan,
    Color::Magenta,
    Color::Yellow,
    Color::Green,
    Color::Blue,
    Color::LightCyan,
];

pub fn busy_spans(tick: u64, label: &str, hint: Option<&str>, muted: Style) -> Vec<Span<'static>> {
    let i = tick as usize;
    let ch = SPIN[i % SPIN.len()];
    let color = SPIN_COLORS[i % SPIN_COLORS.len()];
    let mut spans = vec![
        Span::styled(
            format!(" {ch} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(label.to_string(), Style::default().fg(color)),
    ];
    if let Some(hint) = hint {
        spans.push(Span::styled(format!("  {hint}"), muted));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_cycles() {
        assert_eq!(SPIN.len(), 10);
        let a = busy_spans(0, "scan", Some("esc cancel"), Style::default());
        let b = busy_spans(1, "scan", Some("esc cancel"), Style::default());
        assert!(a[2].content.contains("esc cancel"));
        let closing = busy_spans(0, "closing…", None, Style::default());
        assert_eq!(closing.len(), 2);
        assert_ne!(a[0].content, b[0].content);
    }
}

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
