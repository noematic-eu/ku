use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub fg: Color,
    pub muted: Color,
    pub accent: Color,
    pub green: Color,
    pub yellow: Color,
    pub red: Color,
    pub border: Color,
    pub highlight_bg: Color,
    pub highlight_fg: Color,
    pub gauge_bg: Color,
    pub title: Color,
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            fg: Color::White,
            muted: Color::DarkGray,
            accent: Color::Cyan,
            green: Color::Green,
            yellow: Color::Yellow,
            red: Color::Red,
            border: Color::DarkGray,
            highlight_bg: Color::Rgb(32, 48, 64),
            highlight_fg: Color::Cyan,
            gauge_bg: Color::Rgb(28, 28, 28),
            title: Color::Cyan,
        }
    }

    pub fn light() -> Self {
        Self {
            fg: Color::Black,
            muted: Color::Gray,
            accent: Color::Blue,
            green: Color::Green,
            yellow: Color::Rgb(180, 120, 0),
            red: Color::Red,
            border: Color::Gray,
            highlight_bg: Color::Rgb(210, 230, 245),
            highlight_fg: Color::Blue,
            gauge_bg: Color::Rgb(230, 230, 230),
            title: Color::Blue,
        }
    }

    pub fn from_name(name: &str) -> Self {
        if name.eq_ignore_ascii_case("light") {
            Self::light()
        } else {
            Self::dark()
        }
    }

    pub fn base(&self) -> Style {
        Style::default().fg(self.fg)
    }

    pub fn title_style(&self) -> Style {
        Style::default().fg(self.title).add_modifier(Modifier::BOLD)
    }

    pub fn highlight(&self) -> Style {
        Style::default()
            .bg(self.highlight_bg)
            .fg(self.highlight_fg)
            .add_modifier(Modifier::BOLD)
    }

    pub fn muted_style(&self) -> Style {
        Style::default().fg(self.muted)
    }

    pub fn usage_color(&self, pct: f64, warn: f64, crit: f64) -> Color {
        if pct >= crit {
            self.red
        } else if pct >= warn {
            self.yellow
        } else {
            self.green
        }
    }
}
