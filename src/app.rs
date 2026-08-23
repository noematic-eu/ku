use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::widgets::TableState;
use sysinfo::Users;

use crate::collector::disk::alert_level;
use crate::collector::growth::GrowthRow;
use crate::collector::process::{self, InspectInfo};
use crate::collector::{Alert, Snapshot};
use crate::config::Config;
use crate::hits::{Hit, HitMap, SettingField};
use crate::storage::{LeakSuspect, ProcessAgg, Storage};
use crate::theme::Theme;
use crate::utils::ProcessFilter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Dashboard,
    Disk,
    Growth,
    Processes,
    Settings,
    Help,
}

impl View {
    pub const ALL: [View; 6] = [
        View::Dashboard,
        View::Disk,
        View::Growth,
        View::Processes,
        View::Settings,
        View::Help,
    ];

    pub fn title(self) -> &'static str {
        match self {
            View::Dashboard => "dash",
            View::Disk => "disk",
            View::Growth => "grow",
            View::Processes => "proc",
            View::Settings => "cfg",
            View::Help => "help",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            View::Dashboard => "Dashboard",
            View::Disk => "Disk",
            View::Growth => "Growth",
            View::Processes => "Processes",
            View::Settings => "Settings",
            View::Help => "Help",
        }
    }

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|v| *v == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    pub fn prev(self) -> Self {
        let i = Self::ALL.iter().position(|v| *v == self).unwrap_or(0);
        Self::ALL[(i + Self::ALL.len() - 1) % Self::ALL.len()]
    }

    pub fn from_digit(c: char) -> Option<Self> {
        match c {
            '1' => Some(View::Dashboard),
            '2' => Some(View::Disk),
            '3' => Some(View::Growth),
            '4' => Some(View::Processes),
            '5' => Some(View::Settings),
            '6' => Some(View::Help),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcSort {
    Cpu,
    Mem,
    Pid,
    Name,
    Io,
}

impl ProcSort {
    fn next(self) -> Self {
        match self {
            Self::Cpu => Self::Mem,
            Self::Mem => Self::Pid,
            Self::Pid => Self::Name,
            Self::Name => Self::Io,
            Self::Io => Self::Cpu,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Mem => "mem",
            Self::Pid => "pid",
            Self::Name => "name",
            Self::Io => "io",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskSort {
    Mount,
    UsedPct,
    Size,
    Free,
}

impl DiskSort {
    fn next(self) -> Self {
        match self {
            Self::UsedPct => Self::Size,
            Self::Size => Self::Free,
            Self::Free => Self::Mount,
            Self::Mount => Self::UsedPct,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Mount => "mount",
            Self::UsedPct => "used%",
            Self::Size => "size",
            Self::Free => "free",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrowthWindow {
    H1,
    H6,
    D1,
    D7,
}

impl GrowthWindow {
    pub fn label(self) -> &'static str {
        match self {
            Self::H1 => "1h",
            Self::H6 => "6h",
            Self::D1 => "24h",
            Self::D7 => "7d",
        }
    }

    pub fn secs(self) -> i64 {
        match self {
            Self::H1 => 3600,
            Self::H6 => 6 * 3600,
            Self::D1 => 86400,
            Self::D7 => 7 * 86400,
        }
    }

    fn next(self) -> Self {
        match self {
            Self::H1 => Self::H6,
            Self::H6 => Self::D1,
            Self::D1 => Self::D7,
            Self::D7 => Self::H1,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Overlay {
    None,
    ProcessActions,
    ConfirmKill { pid: u32, name: String, force: bool },
    Inspect(InspectInfo),
    Renice { pid: u32, buf: String },
    DiskDetail(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcMode {
    Live,
    History,
}

pub struct App {
    pub config: Config,
    pub config_path: PathBuf,
    pub data_dir: PathBuf,
    pub theme: Theme,
    pub view: View,
    pub snapshot: Snapshot,
    pub storage: Storage,
    pub should_quit: bool,
    pub filter_input: String,
    pub filter_editing: bool,
    pub filter: ProcessFilter,
    pub disk_filter: String,
    pub proc_sort: ProcSort,
    pub disk_sort: DiskSort,
    pub sort_desc: bool,
    pub proc_state: TableState,
    pub disk_state: TableState,
    pub growth_state: TableState,
    pub history_state: TableState,
    pub overlay: Overlay,
    pub action_cursor: usize,
    pub status: String,
    pub status_at: Instant,
    pub proc_mode: ProcMode,
    pub history_window_idx: usize,
    pub growth_window: GrowthWindow,
    pub growth_rows: Vec<GrowthRow>,
    pub history_rows: Vec<ProcessAgg>,
    pub leak_suspects: Vec<LeakSuspect>,
    pub last_growth_ts: Option<i64>,
    pub focused: bool,
    pub hits: HitMap,
    last_click: Option<(Hit, Instant)>,
    visible_proc: Vec<usize>,
    visible_disk: Vec<usize>,
    users: Users,
}

impl App {
    pub fn new(config: Config, config_path: PathBuf, data_dir: PathBuf, storage: Storage) -> Self {
        let theme = Theme::from_name(&config.general.theme);
        let mut app = Self {
            config,
            config_path,
            data_dir,
            theme,
            view: View::Dashboard,
            snapshot: Snapshot::default(),
            storage,
            should_quit: false,
            filter_input: String::new(),
            filter_editing: false,
            filter: ProcessFilter::default(),
            disk_filter: String::new(),
            proc_sort: ProcSort::Cpu,
            disk_sort: DiskSort::UsedPct,
            sort_desc: true,
            proc_state: TableState::default().with_selected(0),
            disk_state: TableState::default().with_selected(0),
            growth_state: TableState::default().with_selected(0),
            history_state: TableState::default().with_selected(0),
            overlay: Overlay::None,
            action_cursor: 0,
            status: "collecting metrics…".into(),
            status_at: Instant::now(),
            proc_mode: ProcMode::Live,
            history_window_idx: 0,
            growth_window: GrowthWindow::H1,
            growth_rows: Vec::new(),
            history_rows: Vec::new(),
            leak_suspects: Vec::new(),
            last_growth_ts: None,
            focused: false,
            hits: HitMap::default(),
            last_click: None,
            visible_proc: Vec::new(),
            visible_disk: Vec::new(),
            users: Users::new_with_refreshed_list(),
        };
        app.refresh_derived();
        app
    }

    pub fn apply_snapshot(&mut self, snapshot: Snapshot) {
        self.snapshot = snapshot;
        self.refresh_derived();
        if self.proc_mode == ProcMode::History {
            self.reload_history();
        }
        if self.view == View::Growth {
            self.reload_growth();
        }
    }

    pub fn refresh_derived(&mut self) {
        self.visible_disk = self
            .snapshot
            .disks
            .iter()
            .enumerate()
            .filter(|(_, d)| {
                self.disk_filter.is_empty()
                    || d.mount.to_ascii_lowercase().contains(&self.disk_filter)
                    || d.fs.to_ascii_lowercase().contains(&self.disk_filter)
                    || d.name.to_ascii_lowercase().contains(&self.disk_filter)
            })
            .map(|(i, _)| i)
            .collect();
        self.sort_disks();

        self.visible_proc = self
            .snapshot
            .processes
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                self.filter
                    .matches(p.pid, &p.name, &p.user, &p.cmd, p.cpu, p.mem)
            })
            .map(|(i, _)| i)
            .collect();
        self.sort_procs();
        clamp_sel(&mut self.disk_state, self.visible_disk.len());
        clamp_sel(&mut self.proc_state, self.visible_proc.len());
        clamp_sel(&mut self.growth_state, self.growth_rows.len());
        clamp_sel(&mut self.history_state, self.history_rows.len());
    }

    fn sort_disks(&mut self) {
        let disks = &self.snapshot.disks;
        let sort = self.disk_sort;
        let desc = self.sort_desc;
        self.visible_disk.sort_by(|&a, &b| {
            let da = &disks[a];
            let db = &disks[b];
            let ord = match sort {
                DiskSort::Mount => da.mount.cmp(&db.mount),
                DiskSort::UsedPct => da
                    .used_pct()
                    .partial_cmp(&db.used_pct())
                    .unwrap_or(std::cmp::Ordering::Equal),
                DiskSort::Size => da.total.cmp(&db.total),
                DiskSort::Free => da.available.cmp(&db.available),
            };
            if desc { ord.reverse() } else { ord }
        });
    }

    fn sort_procs(&mut self) {
        let procs = &self.snapshot.processes;
        let sort = self.proc_sort;
        let desc = self.sort_desc;
        self.visible_proc.sort_by(|&a, &b| {
            let pa = &procs[a];
            let pb = &procs[b];
            let ord = match sort {
                ProcSort::Cpu => pa
                    .cpu
                    .partial_cmp(&pb.cpu)
                    .unwrap_or(std::cmp::Ordering::Equal),
                ProcSort::Mem => pa.mem.cmp(&pb.mem),
                ProcSort::Pid => pa.pid.cmp(&pb.pid),
                ProcSort::Name => pa
                    .name
                    .to_ascii_lowercase()
                    .cmp(&pb.name.to_ascii_lowercase()),
                ProcSort::Io => (pa.io_read + pa.io_write).cmp(&(pb.io_read + pb.io_write)),
            };
            if desc { ord.reverse() } else { ord }
        });
    }

    pub fn visible_disks(&self) -> impl Iterator<Item = (usize, &crate::collector::DiskSnapshot)> {
        self.visible_disk
            .iter()
            .map(|&i| (i, &self.snapshot.disks[i]))
    }

    pub fn visible_procs(
        &self,
    ) -> impl Iterator<Item = (usize, &crate::collector::ProcessSnapshot)> {
        self.visible_proc
            .iter()
            .map(|&i| (i, &self.snapshot.processes[i]))
    }

    pub fn selected_disk_index(&self) -> Option<usize> {
        self.disk_state
            .selected()
            .and_then(|i| self.visible_disk.get(i).copied())
    }

    pub fn selected_proc(&self) -> Option<&crate::collector::ProcessSnapshot> {
        let i = self.proc_state.selected()?;
        let idx = *self.visible_proc.get(i)?;
        self.snapshot.processes.get(idx)
    }

    pub fn alerts(&self) -> &[Alert] {
        &self.snapshot.alerts
    }

    pub fn history_window(&self) -> &str {
        self.config
            .processes
            .history_window
            .get(self.history_window_idx)
            .map(String::as_str)
            .unwrap_or("5m")
    }

    pub fn reload_growth(&mut self) {
        match self.storage.growth_for_window(self.growth_window.secs()) {
            Ok(rows) => self.growth_rows = rows,
            Err(err) => self.flash(format!("growth query failed: {err}")),
        }
        self.last_growth_ts = self.storage.last_growth_ts().ok().flatten();
        clamp_sel(&mut self.growth_state, self.growth_rows.len());
    }

    pub fn reload_history(&mut self) {
        let window = self.history_window().to_string();
        match self.storage.top_processes(&window, 80) {
            Ok(rows) => self.history_rows = rows,
            Err(err) => self.flash(format!("history query failed: {err}")),
        }
        self.leak_suspects = self.storage.leak_suspects(&window, 12).unwrap_or_default();
        clamp_sel(&mut self.history_state, self.history_rows.len());
    }

    pub fn flash(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
        self.status_at = Instant::now();
    }

    pub fn status_line(&self) -> String {
        if self.status_at.elapsed().as_secs() < 6 {
            self.status.clone()
        } else {
            String::new()
        }
    }

    pub fn switch_view(&mut self, view: View) {
        self.overlay = Overlay::None;
        self.filter_editing = false;
        if self.view == view {
            return;
        }
        self.view = view;
        if view == View::Growth {
            self.reload_growth();
        }
    }

    pub fn handle_event(&mut self, event: Event) -> Result<bool> {
        match event {
            Event::Key(key) => self.handle_key(key),
            Event::Mouse(mouse) => self.handle_mouse(mouse),
            _ => Ok(false),
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        if key.kind != KeyEventKind::Press {
            return Ok(false);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
        {
            self.should_quit = true;
            return Ok(true);
        }
        if self.filter_editing {
            self.handle_filter_key(key);
            return Ok(self.should_quit);
        }
        if !matches!(self.overlay, Overlay::None) {
            self.handle_overlay_key(key)?;
            return Ok(self.should_quit);
        }
        self.handle_global_key(key)?;
        Ok(self.should_quit)
    }

    fn handle_filter_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.filter_editing = false;
                self.filter_input.clear();
            }
            KeyCode::Enter => {
                self.filter_editing = false;
                let q = self.filter_input.to_ascii_lowercase();
                match self.view {
                    View::Disk | View::Growth => self.disk_filter = q,
                    _ => {
                        self.filter = ProcessFilter::parse(&self.filter_input);
                    }
                }
                self.refresh_derived();
            }
            KeyCode::Backspace => {
                self.filter_input.pop();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.filter_input.push(c);
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.filter_input.clear();
            }
            _ => {}
        }
    }

    fn handle_overlay_key(&mut self, key: KeyEvent) -> Result<()> {
        match self.overlay.clone() {
            Overlay::None => {}
            Overlay::ProcessActions => match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('a') => {
                    self.overlay = Overlay::None;
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.action_cursor = (self.action_cursor + 1) % 4;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.action_cursor = (self.action_cursor + 3) % 4;
                }
                KeyCode::Enter
                | KeyCode::Char('1')
                | KeyCode::Char('2')
                | KeyCode::Char('3')
                | KeyCode::Char('4') => {
                    let idx = match key.code {
                        KeyCode::Char('1') => 0,
                        KeyCode::Char('2') => 1,
                        KeyCode::Char('3') => 2,
                        KeyCode::Char('4') => 3,
                        _ => self.action_cursor,
                    };
                    self.run_proc_action(idx)?;
                }
                _ => {}
            },
            Overlay::ConfirmKill { pid, name, force } => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    match process::signal(pid, force) {
                        Ok(()) => self.flash(format!(
                            "sent {} to {name} ({pid})",
                            if force { "SIGKILL" } else { "SIGTERM" }
                        )),
                        Err(err) => self.flash(format!("{err}")),
                    }
                    self.overlay = Overlay::None;
                }
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Char('q') => {
                    self.overlay = Overlay::None;
                }
                _ => {}
            },
            Overlay::Inspect(_) | Overlay::DiskDetail(_) => {
                if matches!(
                    key.code,
                    KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char('i')
                ) {
                    self.overlay = Overlay::None;
                }
            }
            Overlay::Renice { pid, mut buf } => match key.code {
                KeyCode::Esc => self.overlay = Overlay::None,
                KeyCode::Backspace => {
                    buf.pop();
                    self.overlay = Overlay::Renice { pid, buf };
                }
                KeyCode::Enter => {
                    match buf.parse::<i32>() {
                        Ok(nice) => match process::renice(pid, nice) {
                            Ok(()) => self.flash(format!("renice {pid} -> {nice}")),
                            Err(err) => self.flash(format!("{err}")),
                        },
                        Err(_) => self.flash("invalid nice value"),
                    }
                    self.overlay = Overlay::None;
                }
                KeyCode::Char(c) if c.is_ascii_digit() || c == '-' => {
                    buf.push(c);
                    self.overlay = Overlay::Renice { pid, buf };
                }
                _ => {}
            },
        }
        Ok(())
    }

    fn run_proc_action(&mut self, idx: usize) -> Result<()> {
        let Some(proc) = self.selected_proc().cloned() else {
            self.overlay = Overlay::None;
            return Ok(());
        };
        match idx {
            0 => {
                self.overlay = Overlay::ConfirmKill {
                    pid: proc.pid,
                    name: proc.name,
                    force: false,
                };
            }
            1 => {
                self.overlay = Overlay::ConfirmKill {
                    pid: proc.pid,
                    name: proc.name,
                    force: true,
                };
            }
            2 => {
                self.overlay = Overlay::Renice {
                    pid: proc.pid,
                    buf: "0".into(),
                };
            }
            3 => match process::inspect(proc.pid, &self.users) {
                Ok(info) => self.overlay = Overlay::Inspect(info),
                Err(err) => {
                    self.flash(format!("{err}"));
                    self.overlay = Overlay::None;
                }
            },
            _ => self.overlay = Overlay::None,
        }
        Ok(())
    }

    fn handle_global_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('?') => self.switch_view(View::Help),
            KeyCode::Tab => self.switch_view(self.view.next()),
            KeyCode::BackTab => self.switch_view(self.view.prev()),
            KeyCode::Char(c) if c.is_ascii_digit() && View::from_digit(c).is_some() => {
                self.switch_view(View::from_digit(c).unwrap());
            }
            KeyCode::Char('t') if self.view == View::Settings => {
                self.cycle_setting(SettingField::Theme)?;
            }
            KeyCode::Char('/') => {
                self.filter_editing = true;
                self.filter_input.clear();
            }
            KeyCode::Char('r') => {
                self.flash("refresh requested");
                if self.view == View::Growth {
                    self.reload_growth();
                }
                if self.proc_mode == ProcMode::History {
                    self.reload_history();
                }
            }
            KeyCode::Char('s') => {
                match self.view {
                    View::Processes => self.proc_sort = self.proc_sort.next(),
                    View::Disk => self.disk_sort = self.disk_sort.next(),
                    _ => self.sort_desc = !self.sort_desc,
                }
                self.refresh_derived();
            }
            KeyCode::Char('S') => {
                self.sort_desc = !self.sort_desc;
                self.refresh_derived();
            }
            KeyCode::Char('f') => self.focused = !self.focused,
            KeyCode::Char('h') if self.view == View::Processes => {
                self.proc_mode = match self.proc_mode {
                    ProcMode::Live => ProcMode::History,
                    ProcMode::History => ProcMode::Live,
                };
                if self.proc_mode == ProcMode::History {
                    self.reload_history();
                }
            }
            KeyCode::Char('h') if self.view == View::Growth => {
                self.growth_window = self.growth_window.next();
                self.reload_growth();
            }
            KeyCode::Char('[') if self.view == View::Growth => {
                self.growth_window = match self.growth_window {
                    GrowthWindow::H1 => GrowthWindow::D7,
                    GrowthWindow::H6 => GrowthWindow::H1,
                    GrowthWindow::D1 => GrowthWindow::H6,
                    GrowthWindow::D7 => GrowthWindow::D1,
                };
                self.reload_growth();
            }
            KeyCode::Char(']') if self.view == View::Growth => {
                self.growth_window = self.growth_window.next();
                self.reload_growth();
            }
            KeyCode::Char('n')
                if self.view == View::Processes && self.proc_mode == ProcMode::History =>
            {
                let n = self.config.processes.history_window.len().max(1);
                self.history_window_idx = (self.history_window_idx + 1) % n;
                self.reload_history();
            }
            KeyCode::Char('a')
                if self.view == View::Processes && self.proc_mode == ProcMode::Live =>
            {
                if self.selected_proc().is_some() {
                    self.action_cursor = 0;
                    self.overlay = Overlay::ProcessActions;
                }
            }
            KeyCode::Enter => self.handle_enter()?,
            KeyCode::Down | KeyCode::Char('j') => self.move_sel(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_sel(-1),
            KeyCode::PageDown => self.move_sel(10),
            KeyCode::PageUp => self.move_sel(-10),
            KeyCode::Home | KeyCode::Char('g') => self.select_edge(true),
            KeyCode::End | KeyCode::Char('G') => self.select_edge(false),
            KeyCode::Esc => {
                self.filter = ProcessFilter::default();
                self.disk_filter.clear();
                self.filter_input.clear();
                self.refresh_derived();
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> Result<bool> {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.handle_click(mouse.column, mouse.row, false)?;
            }
            MouseEventKind::Down(MouseButton::Right) => {
                self.handle_click(mouse.column, mouse.row, true)?;
            }
            MouseEventKind::ScrollUp => {
                if matches!(self.overlay, Overlay::None) && !self.filter_editing {
                    self.move_sel(-1);
                }
            }
            MouseEventKind::ScrollDown => {
                if matches!(self.overlay, Overlay::None) && !self.filter_editing {
                    self.move_sel(1);
                }
            }
            _ => {}
        }
        Ok(self.should_quit)
    }

    fn handle_click(&mut self, x: u16, y: u16, right: bool) -> Result<()> {
        let Some(hit) = self.hits.at(x, y) else {
            return Ok(());
        };
        let now = Instant::now();
        let double = self
            .last_click
            .is_some_and(|(prev, at)| prev == hit && now.duration_since(at).as_millis() < 400);
        self.last_click = Some((hit, now));

        if self.filter_editing && !matches!(hit, Hit::Tab(_) | Hit::FooterHelp | Hit::FooterQuit) {
            return Ok(());
        }

        match hit {
            Hit::Tab(view) => self.switch_view(view),
            Hit::FooterHelp => self.switch_view(View::Help),
            Hit::FooterQuit => self.should_quit = true,
            Hit::CycleSort => {
                match self.view {
                    View::Processes => self.proc_sort = self.proc_sort.next(),
                    View::Disk => self.disk_sort = self.disk_sort.next(),
                    _ => self.sort_desc = !self.sort_desc,
                }
                self.refresh_derived();
            }
            Hit::GrowthWindow => {
                self.growth_window = self.growth_window.next();
                self.reload_growth();
            }
            Hit::Setting(field) => self.cycle_setting(field)?,
            Hit::DashDisk(idx) => {
                self.switch_view(View::Disk);
                if let Some(pos) = self.visible_disk.iter().position(|i| *i == idx) {
                    self.disk_state.select(Some(pos));
                }
                self.overlay = Overlay::DiskDetail(idx);
            }
            Hit::TableRow(i) => self.handle_table_click(i, right, double)?,
            Hit::OverlayAction(i) => self.run_proc_action(i)?,
            Hit::OverlayYes => {
                if let Overlay::ConfirmKill { pid, name, force } = self.overlay.clone() {
                    match process::signal(pid, force) {
                        Ok(()) => self.flash(format!(
                            "sent {} to {name} ({pid})",
                            if force { "SIGKILL" } else { "SIGTERM" }
                        )),
                        Err(err) => self.flash(format!("{err}")),
                    }
                    self.overlay = Overlay::None;
                }
            }
            Hit::OverlayNo | Hit::OverlayDismiss => self.overlay = Overlay::None,
        }
        Ok(())
    }

    fn handle_table_click(&mut self, index: usize, right: bool, double: bool) -> Result<()> {
        {
            let (state, len) = self.active_table();
            if index >= len {
                return Ok(());
            }
            state.select(Some(index));
        }
        if right && self.view == View::Processes && self.proc_mode == ProcMode::Live {
            if self.selected_proc().is_some() {
                self.action_cursor = 0;
                self.overlay = Overlay::ProcessActions;
            }
            return Ok(());
        }
        if double {
            self.handle_enter()?;
        }
        Ok(())
    }

    fn cycle_setting(&mut self, field: SettingField) -> Result<()> {
        match field {
            SettingField::Theme => {
                self.config.general.theme = if self.config.is_light() {
                    "dark".into()
                } else {
                    "light".into()
                };
                self.theme = Theme::from_name(&self.config.general.theme);
                self.persist_config(&format!("theme → {}", self.config.general.theme));
            }
            SettingField::Refresh => {
                self.config.general.refresh_interval =
                    next_choice(self.config.general.refresh_interval, &[1_u64, 2, 5, 10]);
                self.persist_config(&format!(
                    "refresh → {}s (restart to apply)",
                    self.config.general.refresh_interval
                ));
            }
            SettingField::Retention => {
                self.config.general.history_retention_days = next_choice(
                    self.config.general.history_retention_days,
                    &[1_u32, 7, 14, 30],
                );
                self.persist_config(&format!(
                    "retention → {}d",
                    self.config.general.history_retention_days
                ));
            }
            SettingField::Warn => {
                self.config.disk.warning_threshold =
                    next_choice(self.config.disk.warning_threshold, &[70_u8, 80, 85, 90]);
                self.config.normalize();
                self.persist_config(&format!(
                    "disk warning → {}%",
                    self.config.disk.warning_threshold
                ));
            }
            SettingField::Crit => {
                self.config.disk.critical_threshold =
                    next_choice(self.config.disk.critical_threshold, &[85_u8, 90, 95, 99]);
                self.config.normalize();
                self.persist_config(&format!(
                    "disk critical → {}%",
                    self.config.disk.critical_threshold
                ));
            }
            SettingField::Snapshot => {
                self.config.disk.snapshot_interval =
                    next_choice(self.config.disk.snapshot_interval, &[60_u64, 120, 300, 600]);
                self.persist_config(&format!(
                    "growth scan → {}s (restart to apply)",
                    self.config.disk.snapshot_interval
                ));
            }
        }
        Ok(())
    }

    fn persist_config(&mut self, msg: &str) {
        match self.config.save(&self.config_path) {
            Ok(()) => self.flash(msg.to_string()),
            Err(err) => self.flash(format!("config save failed: {err}")),
        }
    }

    fn handle_enter(&mut self) -> Result<()> {
        match self.view {
            View::Disk => {
                if let Some(idx) = self.selected_disk_index() {
                    self.overlay = Overlay::DiskDetail(idx);
                }
            }
            View::Processes if self.proc_mode == ProcMode::Live => {
                if let Some(proc) = self.selected_proc() {
                    match process::inspect(proc.pid, &self.users) {
                        Ok(info) => self.overlay = Overlay::Inspect(info),
                        Err(err) => self.flash(format!("{err}")),
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn move_sel(&mut self, delta: i32) {
        let (state, len) = self.active_table();
        move_table(state, len, delta);
    }

    fn select_edge(&mut self, start: bool) {
        let (state, len) = self.active_table();
        if len == 0 {
            state.select(None);
        } else if start {
            state.select(Some(0));
        } else {
            state.select(Some(len - 1));
        }
    }

    fn active_table(&mut self) -> (&mut TableState, usize) {
        match self.view {
            View::Disk => (&mut self.disk_state, self.visible_disk.len()),
            View::Growth => (&mut self.growth_state, self.growth_rows.len()),
            View::Processes if self.proc_mode == ProcMode::History => {
                (&mut self.history_state, self.history_rows.len())
            }
            View::Processes => (&mut self.proc_state, self.visible_proc.len()),
            _ => (&mut self.proc_state, self.visible_proc.len()),
        }
    }

    pub fn disk_alert(&self, pct: f64) -> crate::collector::disk::DiskAlertLevel {
        alert_level(pct, &self.config.disk)
    }
}

fn clamp_sel(state: &mut TableState, len: usize) {
    if len == 0 {
        state.select(None);
        return;
    }
    match state.selected() {
        Some(i) if i < len => {}
        Some(_) => state.select(Some(len - 1)),
        None => state.select(Some(0)),
    }
}

fn next_choice<T: Copy + PartialEq>(current: T, options: &[T]) -> T {
    match options.iter().position(|v| *v == current) {
        Some(i) => options[(i + 1) % options.len()],
        None => options.first().copied().unwrap_or(current),
    }
}

fn move_table(state: &mut TableState, len: usize, delta: i32) {
    if len == 0 {
        state.select(None);
        return;
    }
    let cur = state.selected().unwrap_or(0) as i32;
    let next = (cur + delta).rem_euclid(len as i32) as usize;
    state.select(Some(next));
}
