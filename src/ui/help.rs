use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::ui::widgets::bordered;

pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let theme = app.theme;
    let k = |key: &str, desc: &str| {
        Line::from(vec![
            Span::styled(format!("  {key:<22}"), theme.title_style()),
            Span::raw(desc.to_string()),
        ])
    };
    let lines = vec![
        Line::from(Span::styled(" navigation", theme.title_style())),
        k("Tab / Shift-Tab", "next / previous view"),
        k("1 2 3 4 5 6", "dash disk grow proc cfg help"),
        k("j k ↑ ↓", "move selection"),
        k("g / G  Home / End", "first / last row"),
        k(
            "PgUp / PgDn",
            "page jump (auto ~80% of list, or config page_jump)",
        ),
        k("Enter", "detail (disk / process / growth: why it changed)"),
        k("q  Ctrl-c", "quit (cancels background work)"),
        Line::from(""),
        Line::from(Span::styled(" mouse", theme.title_style())),
        k("click tab", "switch view (dash disk grow proc cfg help)"),
        k("click settings value", "cycle option and save config.toml"),
        k("click row / wheel", "select · double-click for detail"),
        k("right-click process", "actions (kill, renice, inspect)"),
        k("click help / quit", "footer shortcuts"),
        Line::from(""),
        Line::from(Span::styled(" filtering & sort", theme.title_style())),
        k("/", "filter (process: text cpu>2 mem>100M user:root)"),
        k("Esc", "clear filter"),
        k("s / S", "cycle sort column / reverse"),
        k("r", "reload derived views"),
        Line::from(""),
        Line::from(Span::styled(" processes", theme.title_style())),
        k("a", "actions: kill, kill -9, renice, inspect"),
        k("h", "toggle live / historical top"),
        k("n", "next history window (1m 5m 1h 24h)"),
        Line::from(""),
        Line::from(Span::styled(" growth", theme.title_style())),
        k("h  [  ]", "cycle 1h / 6h / 24h / 7d window"),
        k("t", "top 50 contributions (new/gone) or all"),
        k(
            "Enter",
            "why this path changed (child files vs recursive dirs)",
        ),
        k(
            "e",
            "ncdu on the folder if installed, else Finder / file manager",
        ),
        k("o", "leftover data from uninstalled apps"),
        k("esc / c", "cancel an in-progress orphan scan"),
        k("i / I", "orphans: ignore this path / this app (allowlist)"),
        k("u", "orphans: show allowlist (ignored leftovers)"),
        k("d / x", "allowlist: remove selected  ·  c clear all"),
        k("d / a", "orphans: delete path / all related (confirm)"),
        k(
            "e",
            "failed delete: elevate (or retry as root: chflags + rm)",
        ),
        k(
            "F / f",
            "orphans: open macOS Full Disk Access (add the terminal app)",
        ),
        k("w", "orphans: Google search the selected folder/app"),
        k(
            "s / S",
            "orphans: size → level → age (oldest first) / reverse",
        ),
        Line::from(""),
        Line::from(Span::styled(
            "  ku stores snapshots in SQLite under the data directory.",
            theme.muted_style(),
        )),
        Line::from(Span::styled(
            "  Disk growth scans watched_paths in the background (default every 5 minutes).",
            theme.muted_style(),
        )),
    ];
    frame.render_widget(Paragraph::new(lines).block(bordered(theme, "help")), area);
}
