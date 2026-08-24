use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Result;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::widgets::TableState;
use sysinfo::Users;

use crate::collector::disk::alert_level;
use crate::collector::growth::{ContribKind, GrowthExplain, GrowthRow};
use crate::collector::process::{self, InspectInfo};
use crate::collector::{Alert, Snapshot};
use crate::config::Config;
use crate::hits::{Hit, HitMap, SettingField};
use crate::orphans::{self, OrphanApp, OrphanReport, OrphanRow, OrphanSort};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrowthLimit {
    Top50,
    All,
}

impl GrowthLimit {
    pub fn label(self) -> &'static str {
        match self {
            Self::Top50 => "top 50",
            Self::All => "all",
        }
    }

    pub fn cap(self) -> Option<usize> {
        match self {
            Self::Top50 => Some(50),
            Self::All => None,
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Top50 => Self::All,
            Self::All => Self::Top50,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrowthMode {
    Movers,
    Orphans,
    Allowlist,
}

#[derive(Debug, Clone)]
pub enum Overlay {
    None,
    ProcessActions,
    ConfirmKill {
        pid: u32,
        name: String,
        force: bool,
    },
    Inspect(InspectInfo),
    Renice {
        pid: u32,
        buf: String,
    },
    DiskDetail(usize),
    InspectOrphan {
        row: OrphanRow,
        related: usize,
    },
    ConfirmOrphanDelete {
        paths: Vec<PathBuf>,
        label: String,
    },
    OrphanDeleteReport {
        removed: usize,
        failed: Vec<orphans::DeleteFail>,
        hint_sudo: bool,
    },
    ConfirmClearIgnore,
    GrowthExplain {
        stack: Vec<GrowthExplain>,
    },
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
    pub shutting_down: bool,
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
    pub growth_limit: GrowthLimit,
    pub growth_mode: GrowthMode,
    pub growth_rows: Vec<GrowthRow>,
    pub orphan_all: Vec<OrphanRow>,
    pub orphan_rows: Vec<OrphanRow>,
    pub orphan_apps: Vec<OrphanApp>,
    pub orphan_sort: OrphanSort,
    pub orphan_sort_rev: bool,
    pub orphan_scanning: bool,
    pub anim_tick: u64,
    orphan_scan_id: u64,
    orphan_cancel: Arc<AtomicBool>,
    orphan_inbox: Arc<Mutex<Option<(u64, Result<OrphanReport, String>)>>>,
    pub history_rows: Vec<ProcessAgg>,
    pub leak_suspects: Vec<LeakSuspect>,
    pub last_growth_ts: Option<i64>,
    pub focused: bool,
    pub hits: HitMap,
    pub list_viewport_rows: usize,
    last_click: Option<(Hit, Instant)>,
    pending_ncdu: Option<PathBuf>,
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
            shutting_down: false,
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
            growth_limit: GrowthLimit::Top50,
            growth_mode: GrowthMode::Movers,
            growth_rows: Vec::new(),
            orphan_all: Vec::new(),
            orphan_rows: Vec::new(),
            orphan_apps: Vec::new(),
            orphan_sort: OrphanSort::Size,
            orphan_sort_rev: false,
            orphan_scanning: false,
            anim_tick: 0,
            orphan_scan_id: 0,
            orphan_cancel: Arc::new(AtomicBool::new(false)),
            orphan_inbox: Arc::new(Mutex::new(None)),
            history_rows: Vec::new(),
            leak_suspects: Vec::new(),
            last_growth_ts: None,
            focused: false,
            hits: HitMap::default(),
            list_viewport_rows: 10,
            last_click: None,
            pending_ncdu: None,
            visible_proc: Vec::new(),
            visible_disk: Vec::new(),
            users: Users::new_with_refreshed_list(),
        };
        if orphans::fda_missing() {
            app.status = format!(
                "Full Disk Access off — add {} in Privacy, then relaunch. F opens Settings",
                orphans::fda_app_hint()
            );
        }
        app.refresh_derived();
        app
    }

    fn open_full_disk_access(&mut self) {
        match orphans::open_fda_settings() {
            Ok(()) => self.flash(format!(
                "System Settings → Full Disk Access — enable {}",
                orphans::fda_app_hint()
            )),
            Err(err) => self.flash(format!("{err}")),
        }
    }

    pub fn apply_snapshot(&mut self, snapshot: Snapshot) {
        self.snapshot = snapshot;
        self.poll_orphan_scan();
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
        let growth_len = match self.growth_mode {
            GrowthMode::Orphans => self.filtered_orphan_len(),
            GrowthMode::Allowlist => self.config.orphans.ignore.len(),
            GrowthMode::Movers => self.filtered_growth_len(),
        };
        clamp_sel(&mut self.growth_state, growth_len);
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
        let n = self.growth_len();
        clamp_sel(&mut self.growth_state, n);
    }

    pub fn is_busy(&self) -> bool {
        self.orphan_scanning || self.shutting_down
    }

    pub fn begin_shutdown(&mut self) {
        if self.shutting_down {
            return;
        }
        self.shutting_down = true;
        self.should_quit = true;
        self.cancel_orphan_scan();
        self.flash("closing…");
    }

    pub fn tick_anim(&mut self) {
        self.anim_tick = self.anim_tick.wrapping_add(1);
    }

    pub fn poll_orphan_scan(&mut self) {
        let taken = self.orphan_inbox.lock().ok().and_then(|mut g| g.take());
        let Some((id, res)) = taken else {
            return;
        };
        if id != self.orphan_scan_id {
            return;
        }
        self.orphan_scanning = false;
        match res {
            Ok(report) => {
                let n = report.apps.len();
                self.orphan_all = report.flatten_rows();
                self.orphan_apps = report.apps;
                self.rebuild_visible_orphans();
                let hidden = self.orphan_all.len().saturating_sub(self.orphan_rows.len());
                if hidden > 0 {
                    self.flash(format!("{n} leftover app(s)  ·  {hidden} ignored"));
                } else {
                    self.flash(format!("{n} leftover app(s)"));
                }
            }
            Err(err) if err == "cancelled" => {
                self.flash("scan cancelled");
            }
            Err(err) => self.flash(format!("orphan scan failed: {err}")),
        }
        let n = self.growth_len();
        clamp_sel(&mut self.growth_state, n);
    }

    pub fn start_orphan_scan(&mut self) {
        self.orphan_cancel.store(true, Ordering::Relaxed);
        self.orphan_scan_id = self.orphan_scan_id.wrapping_add(1);
        let id = self.orphan_scan_id;
        let cancel = Arc::new(AtomicBool::new(false));
        self.orphan_cancel = cancel.clone();
        self.orphan_scanning = true;
        let inbox = self.orphan_inbox.clone();
        std::thread::spawn(move || {
            let res = orphans::scan_with_cancel(&cancel).map_err(|e| e.to_string());
            if let Ok(mut slot) = inbox.lock() {
                *slot = Some((id, res));
            }
        });
        self.flash("scanning leftover app data…");
    }

    pub fn cancel_orphan_scan(&mut self) {
        if !self.orphan_scanning {
            return;
        }
        self.orphan_cancel.store(true, Ordering::Relaxed);
        self.orphan_scan_id = self.orphan_scan_id.wrapping_add(1);
        self.orphan_scanning = false;
        self.flash("scan cancelled");
    }

    pub fn toggle_orphan_mode(&mut self) {
        if self.view == View::Growth
            && matches!(
                self.growth_mode,
                GrowthMode::Orphans | GrowthMode::Allowlist
            )
        {
            self.growth_mode = GrowthMode::Movers;
            let n = self.growth_len();
            clamp_sel(&mut self.growth_state, n);
            return;
        }
        self.switch_view(View::Growth);
        self.growth_mode = GrowthMode::Orphans;
        self.start_orphan_scan();
        let n = self.growth_len();
        clamp_sel(&mut self.growth_state, n);
    }

    pub fn toggle_orphan_allowlist(&mut self) {
        self.switch_view(View::Growth);
        if self.growth_mode == GrowthMode::Allowlist {
            self.growth_mode = GrowthMode::Orphans;
        } else {
            self.growth_mode = GrowthMode::Allowlist;
        }
        let n = self.growth_len();
        clamp_sel(&mut self.growth_state, n);
    }

    fn growth_len(&self) -> usize {
        match self.growth_mode {
            GrowthMode::Orphans => self.filtered_orphan_len(),
            GrowthMode::Allowlist => self.config.orphans.ignore.len(),
            GrowthMode::Movers => self.filtered_growth_len(),
        }
    }

    fn rebuild_visible_orphans(&mut self) {
        let rules = &self.config.orphans.ignore;
        self.orphan_rows = self
            .orphan_all
            .iter()
            .filter(|r| !orphans::is_ignored(rules, &r.app_id, &r.path))
            .cloned()
            .collect();
        self.sort_orphan_rows();
        if matches!(
            self.growth_mode,
            GrowthMode::Orphans | GrowthMode::Allowlist
        ) {
            let n = self.growth_len();
            clamp_sel(&mut self.growth_state, n);
        }
    }

    fn ignore_orphan_path(&mut self, path: &std::path::Path) {
        self.add_orphan_ignore(&path.to_string_lossy());
    }

    fn ignore_orphan_app(&mut self, app_id: &str) {
        self.add_orphan_ignore(app_id);
    }

    fn add_orphan_ignore(&mut self, raw: &str) {
        let from = self.growth_state.selected().unwrap_or(0);
        match orphans::add_ignore(&mut self.config.orphans.ignore, raw) {
            Ok(true) => {
                self.persist_config(&format!("ignored {raw}"));
                self.rebuild_visible_orphans();
                if self.growth_mode == GrowthMode::Orphans {
                    self.select_next_alive_orphan(from);
                }
            }
            Ok(false) => self.flash("already ignored"),
            Err(err) => self.flash(err.to_string()),
        }
    }

    fn unignore_selected(&mut self) {
        let Some(i) = self.growth_state.selected() else {
            return;
        };
        if i >= self.config.orphans.ignore.len() {
            return;
        }
        let rule = self.config.orphans.ignore[i].clone();
        if orphans::remove_ignore(&mut self.config.orphans.ignore, &rule) {
            self.persist_config(&format!("unignored {rule}"));
            self.rebuild_visible_orphans();
        }
    }

    fn clear_orphan_ignore(&mut self) {
        let n = self.config.orphans.ignore.len();
        self.config.orphans.ignore.clear();
        self.overlay = Overlay::None;
        if n == 0 {
            self.flash("allowlist empty");
            return;
        }
        self.persist_config(&format!("cleared {n} allowlist entries"));
        self.rebuild_visible_orphans();
    }

    fn growth_row_visible(&self, path: &str) -> bool {
        self.disk_filter.is_empty() || path.to_ascii_lowercase().contains(&self.disk_filter)
    }

    pub fn visible_growth_rows(&self) -> Vec<&GrowthRow> {
        let capped = self
            .growth_rows
            .iter()
            .take(self.growth_limit.cap().unwrap_or(usize::MAX));
        capped
            .filter(|r| self.growth_row_visible(&r.path))
            .collect()
    }

    fn filtered_growth_len(&self) -> usize {
        self.visible_growth_rows().len()
    }

    pub fn cycle_growth_limit(&mut self) {
        self.growth_limit = self.growth_limit.next();
        let n = self.growth_len();
        clamp_sel(&mut self.growth_state, n);
        let shown = n;
        let total = self.growth_rows.len();
        self.flash(format!(
            "growth {} ({shown}/{total})",
            self.growth_limit.label()
        ));
    }

    fn orphan_row_visible(&self, row: &OrphanRow) -> bool {
        let q = &self.disk_filter;
        q.is_empty()
            || row.path.to_string_lossy().to_ascii_lowercase().contains(q)
            || row.app_id.to_ascii_lowercase().contains(q)
            || row.app_name.to_ascii_lowercase().contains(q)
    }

    fn filtered_orphan_len(&self) -> usize {
        self.orphan_rows
            .iter()
            .filter(|r| self.orphan_row_visible(r))
            .count()
    }

    pub fn selected_orphan(&self) -> Option<&OrphanRow> {
        let i = self.growth_state.selected()?;
        self.orphan_rows
            .iter()
            .filter(|r| self.orphan_row_visible(r))
            .nth(i)
    }

    pub fn selected_growth(&self) -> Option<&GrowthRow> {
        let i = self.growth_state.selected()?;
        let cap = self.growth_limit.cap().unwrap_or(usize::MAX);
        self.growth_rows
            .iter()
            .take(cap)
            .filter(|r| self.growth_row_visible(&r.path))
            .nth(i)
    }

    fn reveal_path(&mut self, path: &Path) {
        if path.as_os_str().is_empty() {
            self.flash("empty path");
            return;
        }
        if !path.exists() {
            self.flash(format!("{} no longer exists", path.display()));
            return;
        }
        if crate::utils::ncdu_available() {
            self.pending_ncdu = Some(path.to_path_buf());
            return;
        }
        match crate::utils::reveal_in_file_manager(path) {
            Ok(()) => self.flash(format!(
                "{}: {}",
                crate::utils::file_manager_label(),
                path.display()
            )),
            Err(err) => self.flash(format!("{err}")),
        }
    }

    pub fn take_pending_ncdu(&mut self) -> Option<PathBuf> {
        self.pending_ncdu.take()
    }

    fn reveal_selected_growth_path(&mut self) {
        match self.growth_mode {
            GrowthMode::Movers => {
                if let Some(row) = self.selected_growth() {
                    let path = PathBuf::from(&row.path);
                    self.reveal_path(&path);
                } else {
                    self.flash("no folder selected");
                }
            }
            GrowthMode::Orphans => {
                if let Some(row) = self.selected_orphan() {
                    let path = row.path.clone();
                    self.reveal_path(&path);
                } else {
                    self.flash("no path selected");
                }
            }
            GrowthMode::Allowlist => {}
        }
    }

    fn orphan_related_paths(&self, app_id: &str) -> Vec<PathBuf> {
        self.orphan_rows
            .iter()
            .filter(|r| r.app_id == app_id && !r.deleted)
            .map(|r| r.path.clone())
            .collect()
    }

    pub fn sort_orphan_rows(&mut self) {
        orphans::sort_rows(
            &mut self.orphan_rows,
            self.orphan_sort,
            self.orphan_sort_rev,
        );
    }

    fn cycle_orphan_sort(&mut self) {
        self.orphan_sort = self.orphan_sort.next();
        self.orphan_sort_rev = false;
        self.sort_orphan_rows();
        let dir = if self.orphan_sort == OrphanSort::Age {
            "oldest first"
        } else {
            "interesting first"
        };
        self.flash(format!("sort {} ({dir})", self.orphan_sort.label()));
    }

    fn retry_orphan_delete_elevated(&mut self, failed: &[orphans::DeleteFail]) {
        let paths: Vec<PathBuf> = failed.iter().map(|f| f.path.clone()).collect();
        let result = orphans::elevate_retry(&paths);
        let gone: Vec<PathBuf> = paths.iter().filter(|p| !p.exists()).cloned().collect();
        let still: Vec<_> = paths.iter().filter(|p| p.exists()).cloned().collect();
        if !gone.is_empty() {
            self.mark_orphans_deleted(&gone);
        }
        let how = result.as_ref().map(|k| k.label()).unwrap_or("retry");
        if still.is_empty() {
            self.flash(format!("removed {} path(s) via {how}", gone.len()));
            self.overlay = Overlay::None;
            return;
        }
        let blocked = if orphans::running_as_root() {
            "still present (SIP / Full Disk Access)"
        } else {
            "still present"
        };
        match result {
            Err(err) if gone.is_empty() => self.flash(format!("{err}")),
            _ => self.flash(format!("{} still blocked after {how}", still.len())),
        }
        self.overlay = Overlay::OrphanDeleteReport {
            removed: gone.len(),
            hint_sudo: !orphans::running_as_root(),
            failed: still
                .into_iter()
                .map(|path| orphans::DeleteFail {
                    path,
                    error: blocked.into(),
                    permission: true,
                })
                .collect(),
        };
    }

    fn finish_orphan_delete(&mut self, paths: &[PathBuf], label: &str) {
        let outcome = orphans::delete_targets(paths);
        if !outcome.removed.is_empty() {
            self.mark_orphans_deleted(&outcome.removed);
        }
        if outcome.incomplete() {
            let n_fail = outcome.failed.len();
            self.flash(format!(
                "partial delete: {} removed, {n_fail} failed ({label})",
                outcome.removed.len()
            ));
            self.overlay = Overlay::OrphanDeleteReport {
                removed: outcome.removed.len(),
                hint_sudo: outcome.permission_denied() && !orphans::running_as_root(),
                failed: outcome.failed,
            };
        } else {
            self.flash(format!(
                "removed {} path(s) ({label})",
                outcome.removed.len()
            ));
            self.overlay = Overlay::None;
        }
    }

    fn mark_orphans_deleted(&mut self, paths: &[PathBuf]) {
        let from = self.growth_state.selected().unwrap_or(0);
        for row in &mut self.orphan_rows {
            if paths
                .iter()
                .any(|p| p == &row.path || row.path.starts_with(p))
            {
                row.deleted = true;
            }
        }
        self.select_next_alive_orphan(from);
    }

    fn select_next_alive_orphan(&mut self, from: usize) {
        let len = self.orphan_rows.len();
        if len == 0 {
            self.growth_state.select(None);
            return;
        }
        let start = (from + 1).min(len);
        for i in start..len {
            if !self.orphan_rows[i].deleted {
                self.growth_state.select(Some(i));
                return;
            }
        }
        for i in 0..start {
            if !self.orphan_rows[i].deleted {
                self.growth_state.select(Some(i));
                return;
            }
        }
        self.growth_state.select(Some(from.min(len - 1)));
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
        if self.shutting_down {
            return Ok(false);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
        {
            self.begin_shutdown();
            return Ok(false);
        }
        if self.filter_editing {
            self.handle_filter_key(key);
            return Ok(false);
        }
        if !matches!(self.overlay, Overlay::None) {
            self.handle_overlay_key(key)?;
            return Ok(false);
        }
        self.handle_global_key(key)?;
        Ok(false)
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
            Overlay::OrphanDeleteReport { failed, .. } => match key.code {
                KeyCode::Char('e') | KeyCode::Char('E') => {
                    self.retry_orphan_delete_elevated(&failed);
                }
                KeyCode::Char('f') | KeyCode::Char('F') if orphans::fda_needed() => {
                    self.open_full_disk_access();
                }
                KeyCode::Esc
                | KeyCode::Enter
                | KeyCode::Char('q')
                | KeyCode::Char('y')
                | KeyCode::Char('n') => {
                    self.overlay = Overlay::None;
                }
                _ => {}
            },
            Overlay::ConfirmOrphanDelete { paths, label } => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    self.finish_orphan_delete(&paths, &label);
                }
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Char('q') => {
                    self.overlay = Overlay::None;
                }
                _ => {}
            },
            Overlay::ConfirmClearIgnore => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    self.clear_orphan_ignore();
                }
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Char('q') => {
                    self.overlay = Overlay::None;
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
            Overlay::InspectOrphan { row, .. } => match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                    self.overlay = Overlay::None;
                }
                KeyCode::Char('e') => {
                    self.reveal_path(&row.path);
                }
                KeyCode::Char('i') => {
                    self.overlay = Overlay::None;
                    self.ignore_orphan_path(&row.path);
                }
                KeyCode::Char('I') => {
                    self.overlay = Overlay::None;
                    self.ignore_orphan_app(&row.app_id);
                }
                KeyCode::Char('w') => {
                    let q = orphans::search_query(&row);
                    match orphans::open_google_search(&q) {
                        Ok(()) => self.flash(format!("google: {q}")),
                        Err(err) => self.flash(format!("{err}")),
                    }
                }
                KeyCode::Char('d') if !row.deleted => {
                    self.overlay = Overlay::ConfirmOrphanDelete {
                        paths: vec![row.path.clone()],
                        label: row.path.display().to_string(),
                    };
                }
                KeyCode::Char('a') => {
                    let id = row.app_id.clone();
                    let paths = self.orphan_related_paths(&id);
                    if !paths.is_empty() {
                        self.overlay = Overlay::ConfirmOrphanDelete {
                            paths,
                            label: format!("ALL leftovers for {id}"),
                        };
                    }
                }
                _ => {}
            },
            Overlay::GrowthExplain { mut stack } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    stack.pop();
                    self.overlay = if stack.is_empty() {
                        Overlay::None
                    } else {
                        Overlay::GrowthExplain { stack }
                    };
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if let Some(frame) = stack.last_mut() {
                        let last = frame.rows.len().saturating_sub(1);
                        if frame.selected < last {
                            frame.selected += 1;
                        }
                    }
                    self.overlay = Overlay::GrowthExplain { stack };
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if let Some(frame) = stack.last_mut() {
                        frame.selected = frame.selected.saturating_sub(1);
                    }
                    self.overlay = Overlay::GrowthExplain { stack };
                }
                KeyCode::Enter => {
                    let child = stack.last().and_then(|f| f.rows.get(f.selected)).cloned();
                    self.overlay = Overlay::GrowthExplain { stack };
                    if let Some(row) = child
                        && row.kind == Some(ContribKind::Dir)
                        && row.change != crate::collector::growth::ContribChange::Gone
                    {
                        self.push_growth_explain(row.path, row.size, row.abs_delta.unwrap_or(0));
                    }
                }
                KeyCode::Char('e') => {
                    let path = stack.last().and_then(|f| {
                        f.rows
                            .get(f.selected)
                            .map(|r| PathBuf::from(&r.path))
                            .or_else(|| Some(PathBuf::from(&f.path)))
                    });
                    self.overlay = Overlay::GrowthExplain { stack };
                    if let Some(path) = path {
                        self.reveal_path(&path);
                    }
                }
                _ => {
                    self.overlay = Overlay::GrowthExplain { stack };
                }
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
            KeyCode::Char('q') => self.begin_shutdown(),
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
                if self.view == View::Growth
                    && matches!(
                        self.growth_mode,
                        GrowthMode::Orphans | GrowthMode::Allowlist
                    )
                {
                    self.start_orphan_scan();
                } else if self.view == View::Growth {
                    self.reload_growth();
                }
                if self.proc_mode == ProcMode::History {
                    self.reload_history();
                }
            }
            KeyCode::Char('o') if self.view == View::Growth || self.view == View::Dashboard => {
                self.toggle_orphan_mode();
            }
            KeyCode::Char('u')
                if self.view == View::Growth
                    && matches!(
                        self.growth_mode,
                        GrowthMode::Orphans | GrowthMode::Allowlist
                    ) =>
            {
                self.toggle_orphan_allowlist();
            }
            KeyCode::Char('i')
                if self.view == View::Growth && self.growth_mode == GrowthMode::Orphans =>
            {
                if let Some(row) = self.selected_orphan() {
                    let path = row.path.clone();
                    self.ignore_orphan_path(&path);
                }
            }
            KeyCode::Char('I')
                if self.view == View::Growth && self.growth_mode == GrowthMode::Orphans =>
            {
                if let Some(row) = self.selected_orphan() {
                    let id = row.app_id.clone();
                    self.ignore_orphan_app(&id);
                }
            }
            KeyCode::Char('x') | KeyCode::Delete | KeyCode::Backspace
                if self.view == View::Growth && self.growth_mode == GrowthMode::Allowlist =>
            {
                self.unignore_selected();
            }
            KeyCode::Char('d')
                if self.view == View::Growth && self.growth_mode == GrowthMode::Allowlist =>
            {
                self.unignore_selected();
            }
            KeyCode::Char('s') => {
                if self.view == View::Growth && self.growth_mode == GrowthMode::Orphans {
                    self.cycle_orphan_sort();
                } else {
                    match self.view {
                        View::Processes => self.proc_sort = self.proc_sort.next(),
                        View::Disk => self.disk_sort = self.disk_sort.next(),
                        _ => self.sort_desc = !self.sort_desc,
                    }
                    self.refresh_derived();
                }
            }
            KeyCode::Char('S') => {
                if self.view == View::Growth && self.growth_mode == GrowthMode::Orphans {
                    self.orphan_sort_rev = !self.orphan_sort_rev;
                    self.sort_orphan_rows();
                } else {
                    self.sort_desc = !self.sort_desc;
                    self.refresh_derived();
                }
            }
            KeyCode::Char('F')
                if self.view == View::Growth
                    && matches!(
                        self.growth_mode,
                        GrowthMode::Orphans | GrowthMode::Allowlist
                    )
                    && orphans::fda_needed() =>
            {
                self.open_full_disk_access();
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
            KeyCode::Char('h')
                if self.view == View::Growth && self.growth_mode == GrowthMode::Movers =>
            {
                self.growth_window = self.growth_window.next();
                self.reload_growth();
            }
            KeyCode::Char('t')
                if self.view == View::Growth && self.growth_mode == GrowthMode::Movers =>
            {
                self.cycle_growth_limit();
            }
            KeyCode::Char('e')
                if self.view == View::Growth
                    && matches!(self.growth_mode, GrowthMode::Movers | GrowthMode::Orphans) =>
            {
                self.reveal_selected_growth_path();
            }
            KeyCode::Char('w')
                if self.view == View::Growth && self.growth_mode == GrowthMode::Orphans =>
            {
                if let Some(row) = self.selected_orphan() {
                    let q = orphans::search_query(row);
                    match orphans::open_google_search(&q) {
                        Ok(()) => self.flash(format!("google: {q}")),
                        Err(err) => self.flash(format!("{err}")),
                    }
                }
            }
            KeyCode::Char('d')
                if self.view == View::Growth && self.growth_mode == GrowthMode::Orphans =>
            {
                if let Some(row) = self.selected_orphan()
                    && !row.deleted
                {
                    self.overlay = Overlay::ConfirmOrphanDelete {
                        paths: vec![row.path.clone()],
                        label: row.path.display().to_string(),
                    };
                }
            }
            KeyCode::Char('a')
                if self.view == View::Growth && self.growth_mode == GrowthMode::Orphans =>
            {
                if let Some(row) = self.selected_orphan() {
                    let id = row.app_id.clone();
                    let paths = self.orphan_related_paths(&id);
                    if !paths.is_empty() {
                        self.overlay = Overlay::ConfirmOrphanDelete {
                            paths,
                            label: format!("ALL leftovers for {id}"),
                        };
                    }
                }
            }
            KeyCode::Char('[')
                if self.view == View::Growth && self.growth_mode == GrowthMode::Movers =>
            {
                self.growth_window = match self.growth_window {
                    GrowthWindow::H1 => GrowthWindow::D7,
                    GrowthWindow::H6 => GrowthWindow::H1,
                    GrowthWindow::D1 => GrowthWindow::H6,
                    GrowthWindow::D7 => GrowthWindow::D1,
                };
                self.reload_growth();
            }
            KeyCode::Char(']')
                if self.view == View::Growth && self.growth_mode == GrowthMode::Movers =>
            {
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
            KeyCode::PageDown => self.move_sel(self.page_delta()),
            KeyCode::PageUp => self.move_sel(-self.page_delta()),
            KeyCode::Home | KeyCode::Char('g') => self.select_edge(true),
            KeyCode::End | KeyCode::Char('G') => self.select_edge(false),
            KeyCode::Esc if self.orphan_scanning => {
                self.cancel_orphan_scan();
            }
            KeyCode::Char('c') if self.orphan_scanning => {
                self.cancel_orphan_scan();
            }
            KeyCode::Char('c')
                if self.view == View::Growth && self.growth_mode == GrowthMode::Allowlist =>
            {
                if self.config.orphans.ignore.is_empty() {
                    self.flash("allowlist empty");
                } else {
                    self.overlay = Overlay::ConfirmClearIgnore;
                }
            }
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
        Ok(false)
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
            Hit::FooterQuit => self.begin_shutdown(),
            Hit::CycleSort => {
                if self.view == View::Growth && self.growth_mode == GrowthMode::Orphans {
                    self.cycle_orphan_sort();
                } else {
                    match self.view {
                        View::Processes => self.proc_sort = self.proc_sort.next(),
                        View::Disk => self.disk_sort = self.disk_sort.next(),
                        _ => self.sort_desc = !self.sort_desc,
                    }
                    self.refresh_derived();
                }
            }
            Hit::GrowthWindow => {
                self.growth_window = self.growth_window.next();
                self.reload_growth();
            }
            Hit::GrowthLimit => {
                self.cycle_growth_limit();
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
            Hit::OverlayYes => match self.overlay.clone() {
                Overlay::ConfirmKill { pid, name, force } => {
                    match process::signal(pid, force) {
                        Ok(()) => self.flash(format!(
                            "sent {} to {name} ({pid})",
                            if force { "SIGKILL" } else { "SIGTERM" }
                        )),
                        Err(err) => self.flash(format!("{err}")),
                    }
                    self.overlay = Overlay::None;
                }
                Overlay::ConfirmOrphanDelete { paths, label } => {
                    self.finish_orphan_delete(&paths, &label);
                }
                Overlay::OrphanDeleteReport { failed, .. } => {
                    self.retry_orphan_delete_elevated(&failed);
                }
                Overlay::ConfirmClearIgnore => self.clear_orphan_ignore(),
                _ => {}
            },
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
            SettingField::PageJump => {
                self.config.general.page_jump =
                    next_choice(self.config.general.page_jump, &[0, 5, 10, 20, 40]);
                self.persist_config(&format!(
                    "page_jump → {}",
                    if self.config.general.page_jump == 0 {
                        "auto".into()
                    } else {
                        self.config.general.page_jump.to_string()
                    }
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

    fn open_growth_explain(&mut self) {
        let Some(row) = self.selected_growth() else {
            self.flash("no folder selected");
            return;
        };
        let path = row.path.clone();
        let size = row.size;
        let delta = row.abs_delta;
        self.overlay = Overlay::None;
        self.push_growth_explain(path, size, delta);
    }

    fn push_growth_explain(&mut self, path: String, size: u64, abs_delta: i64) {
        match self.storage.growth_snapshots(self.growth_window.secs()) {
            Ok((current, previous)) => {
                let frame =
                    crate::collector::growth::explain(&path, size, abs_delta, &current, &previous);
                match &mut self.overlay {
                    Overlay::GrowthExplain { stack } => stack.push(frame),
                    _ => {
                        self.overlay = Overlay::GrowthExplain { stack: vec![frame] };
                    }
                }
            }
            Err(err) => self.flash(format!("{err}")),
        }
    }

    fn handle_enter(&mut self) -> Result<()> {
        match self.view {
            View::Growth if self.growth_mode == GrowthMode::Movers => {
                self.open_growth_explain();
            }
            View::Growth if self.growth_mode == GrowthMode::Orphans => {
                if let Some(row) = self.selected_orphan() {
                    self.overlay = Overlay::InspectOrphan {
                        related: row.related,
                        row: row.clone(),
                    };
                }
            }
            View::Growth if self.growth_mode == GrowthMode::Allowlist => {
                self.unignore_selected();
            }
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

    pub fn page_delta(&self) -> i32 {
        let jump = self.config.general.page_jump;
        if jump > 0 {
            return i32::from(jump);
        }
        let vis = self.list_viewport_rows.max(1);
        ((vis * 80) / 100).max(1) as i32
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
            View::Growth if self.growth_mode == GrowthMode::Orphans => {
                let n = self.filtered_orphan_len();
                (&mut self.growth_state, n)
            }
            View::Growth if self.growth_mode == GrowthMode::Allowlist => {
                let n = self.config.orphans.ignore.len();
                (&mut self.growth_state, n)
            }
            View::Growth => {
                let n = self.filtered_growth_len();
                (&mut self.growth_state, n)
            }
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
    let last = (len - 1) as i32;
    let cur = state.selected().unwrap_or(0) as i32;
    let next = cur + delta;
    let idx = if next < 0 {
        0
    } else if next > last {
        0
    } else {
        next as usize
    };
    state.select(Some(idx));
}

#[cfg(test)]
mod nav_tests {
    use super::move_table;
    use ratatui::widgets::TableState;

    #[test]
    fn up_at_top_stays() {
        let mut state = TableState::default().with_selected(0);
        move_table(&mut state, 20, -1);
        assert_eq!(state.selected(), Some(0));
        move_table(&mut state, 20, -10);
        assert_eq!(state.selected(), Some(0));
    }

    #[test]
    fn down_at_bottom_wraps_to_top() {
        let mut state = TableState::default().with_selected(19);
        move_table(&mut state, 20, 1);
        assert_eq!(state.selected(), Some(0));
    }

    #[test]
    fn down_in_middle_steps() {
        let mut state = TableState::default().with_selected(5);
        move_table(&mut state, 20, 3);
        assert_eq!(state.selected(), Some(8));
    }
}
