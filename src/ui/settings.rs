use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::hits::{Hit, SettingField};
use crate::ui::widgets::bordered;

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = app.theme;
    let block = bordered(theme, "settings  ·  click a value to change");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = setting_rows(app);
    let mut lines = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let y = inner.y.saturating_add(i as u16);
        match row {
            SettingRow::Header(title) => {
                lines.push(Line::from(Span::styled(
                    format!(" {title}"),
                    theme.title_style(),
                )));
            }
            SettingRow::Blank => lines.push(Line::from("")),
            SettingRow::Note(text) => {
                lines.push(Line::from(Span::styled(text.clone(), theme.muted_style())));
            }
            SettingRow::Field {
                field,
                key,
                value,
                live,
            } => {
                if y < inner.bottom() {
                    app.hits.push(
                        Rect {
                            x: inner.x,
                            y,
                            width: inner.width,
                            height: 1,
                        },
                        Hit::Setting(*field),
                    );
                }
                let value_style = if *live {
                    theme.title_style()
                } else {
                    Style::default().fg(theme.fg)
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("  {key:<28}"), theme.muted_style()),
                    Span::styled(value.clone(), value_style),
                    Span::styled("  ‹ click ›", theme.muted_style()),
                ]));
            }
        }
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

enum SettingRow {
    Header(&'static str),
    Blank,
    Note(String),
    Field {
        field: SettingField,
        key: &'static str,
        value: String,
        live: bool,
    },
}

fn setting_rows(app: &App) -> Vec<SettingRow> {
    let cfg = &app.config;
    vec![
        SettingRow::Header("paths"),
        SettingRow::Note(format!("  config    {}", app.config_path.display())),
        SettingRow::Note(format!("  data dir  {}", app.data_dir.display())),
        SettingRow::Blank,
        SettingRow::Header("general"),
        SettingRow::Field {
            field: SettingField::Refresh,
            key: "refresh_interval",
            value: format!("{}s", cfg.general.refresh_interval),
            live: false,
        },
        SettingRow::Field {
            field: SettingField::Theme,
            key: "theme",
            value: cfg.general.theme.clone(),
            live: true,
        },
        SettingRow::Field {
            field: SettingField::Retention,
            key: "history_retention_days",
            value: cfg.general.history_retention_days.to_string(),
            live: true,
        },
        SettingRow::Blank,
        SettingRow::Header("disk"),
        SettingRow::Field {
            field: SettingField::Warn,
            key: "warning_threshold",
            value: format!("{}%", cfg.disk.warning_threshold),
            live: true,
        },
        SettingRow::Field {
            field: SettingField::Crit,
            key: "critical_threshold",
            value: format!("{}%", cfg.disk.critical_threshold),
            live: true,
        },
        SettingRow::Field {
            field: SettingField::Snapshot,
            key: "snapshot_interval",
            value: format!("{}s", cfg.disk.snapshot_interval),
            live: false,
        },
        SettingRow::Blank,
        SettingRow::Header("processes"),
        SettingRow::Note("  history_window is edited in config.toml".into()),
        SettingRow::Blank,
        SettingRow::Note("  Theme, thresholds and retention apply immediately.".into()),
        SettingRow::Note("  Interval changes are saved; restart ku to apply.".into()),
        SettingRow::Note("  t toggles theme · values are written to config.toml".into()),
    ]
}
