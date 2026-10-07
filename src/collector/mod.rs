pub mod cpu;
pub mod disk;
pub mod growth;
pub mod memory;
pub mod process;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, Users};
use tokio::sync::watch;
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::storage::Storage;

pub use cpu::CpuSnapshot;
pub use disk::DiskSnapshot;
pub use memory::MemorySnapshot;
pub use process::{InspectInfo, ProcessSnapshot};

const HISTORY_LEN: usize = 180;

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub collected_at: DateTime<Local>,
    pub hostname: String,
    pub os: String,
    pub kernel: String,
    pub uptime_secs: u64,
    pub load: LoadSnapshot,
    pub cpu: CpuSnapshot,
    pub memory: MemorySnapshot,
    pub disks: Vec<DiskSnapshot>,
    pub processes: Vec<ProcessSnapshot>,
    pub alerts: Vec<Alert>,
    pub cpu_history: Vec<u64>,
    pub mem_history: Vec<u64>,
    pub process_count: usize,
    pub zombie_count: usize,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            collected_at: Local::now(),
            hostname: String::new(),
            os: String::new(),
            kernel: String::new(),
            uptime_secs: 0,
            load: LoadSnapshot::default(),
            cpu: CpuSnapshot::default(),
            memory: MemorySnapshot::default(),
            disks: Vec::new(),
            processes: Vec::new(),
            alerts: Vec::new(),
            cpu_history: Vec::new(),
            mem_history: Vec::new(),
            process_count: 0,
            zombie_count: 0,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct LoadSnapshot {
    pub one: f64,
    pub five: f64,
    pub fifteen: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertLevel {
    Warning,
    Critical,
}

#[derive(Debug, Clone)]
pub struct Alert {
    pub level: AlertLevel,
    pub message: String,
}

pub struct Collector {
    sys: System,
    disk_sampler: disk::Sampler,
    users: Users,
    cpu_hist: VecDeque<u64>,
    mem_hist: VecDeque<u64>,
    config: Config,
    ticks: u64,
}

impl Collector {
    pub fn new(config: Config) -> Self {
        let mut sys = System::new();
        sys.refresh_cpu_all();
        sys.refresh_memory();
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::everything().without_tasks(),
        );
        let mut disk_sampler = disk::Sampler::new();
        // Brief wait so the first frame often has volumes. Never block on a
        // hung CFURL/statvfs (Time Machine, autofs, sleeping disks).
        disk_sampler.wait(Duration::from_millis(400));
        Self {
            sys,
            disk_sampler,
            users: Users::new_with_refreshed_list(),
            cpu_hist: VecDeque::with_capacity(HISTORY_LEN),
            mem_hist: VecDeque::with_capacity(HISTORY_LEN),
            config,
            ticks: 0,
        }
    }

    pub fn wait_disks(&mut self, timeout: Duration) {
        self.disk_sampler.wait(timeout);
    }

    pub fn collect(&mut self) -> Snapshot {
        self.sys.refresh_cpu_all();
        self.sys.refresh_memory();
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::everything().without_tasks(),
        );
        self.disk_sampler.tick();
        if self.ticks.is_multiple_of(15) {
            self.users.refresh();
        }
        self.ticks += 1;

        let cpu = cpu::collect(&self.sys);
        let memory = memory::collect(&self.sys);
        let disks = self.disk_sampler.snapshots().to_vec();
        let processes = process::collect(&self.sys, &self.users);
        let load = System::load_average();
        let zombie_count = processes.iter().filter(|p| p.is_zombie).count();

        push_hist(&mut self.cpu_hist, cpu.global.clamp(0.0, 100.0) as u64);
        push_hist(
            &mut self.mem_hist,
            memory.used_pct().clamp(0.0, 100.0) as u64,
        );

        let alerts = build_alerts(&self.config, &memory, &disks, zombie_count);

        Snapshot {
            collected_at: Local::now(),
            hostname: System::host_name().unwrap_or_else(|| "unknown".into()),
            os: System::long_os_version().unwrap_or_else(|| System::name().unwrap_or_default()),
            kernel: System::kernel_version().unwrap_or_default(),
            uptime_secs: System::uptime(),
            load: LoadSnapshot {
                one: load.one,
                five: load.five,
                fifteen: load.fifteen,
            },
            cpu,
            memory,
            disks,
            process_count: processes.len(),
            zombie_count,
            processes,
            alerts,
            cpu_history: self.cpu_hist.iter().copied().collect(),
            mem_history: self.mem_hist.iter().copied().collect(),
        }
    }
}

fn push_hist(buf: &mut VecDeque<u64>, value: u64) {
    if buf.len() == HISTORY_LEN {
        buf.pop_front();
    }
    buf.push_back(value);
}

fn build_alerts(
    config: &Config,
    memory: &MemorySnapshot,
    disks: &[DiskSnapshot],
    zombie_count: usize,
) -> Vec<Alert> {
    let mut alerts = Vec::new();
    let mem_pct = memory.used_pct();
    if mem_pct >= 95.0 {
        alerts.push(Alert {
            level: AlertLevel::Critical,
            message: format!("memory {mem_pct:.0}% used"),
        });
    } else if mem_pct >= 85.0 {
        alerts.push(Alert {
            level: AlertLevel::Warning,
            message: format!("memory {mem_pct:.0}% used"),
        });
    }
    if memory.swap_total > 0 && memory.swap_pct() >= 80.0 {
        alerts.push(Alert {
            level: AlertLevel::Warning,
            message: format!("swap {pct:.0}% used", pct = memory.swap_pct()),
        });
    }
    for disk in disks {
        match disk::alert_level(disk.used_pct(), &config.disk) {
            disk::DiskAlertLevel::Critical => alerts.push(Alert {
                level: AlertLevel::Critical,
                message: format!("disk {} {:.0}% full", disk.mount, disk.used_pct()),
            }),
            disk::DiskAlertLevel::Warning => alerts.push(Alert {
                level: AlertLevel::Warning,
                message: format!("disk {} {:.0}% used", disk.mount, disk.used_pct()),
            }),
            disk::DiskAlertLevel::Ok => {}
        }
        if let Some(pct) = disk.inode_pct()
            && pct >= f64::from(config.disk.warning_threshold)
        {
            alerts.push(Alert {
                level: if pct >= f64::from(config.disk.critical_threshold) {
                    AlertLevel::Critical
                } else {
                    AlertLevel::Warning
                },
                message: format!("inodes {} {pct:.0}% used", disk.mount),
            });
        }
    }
    if zombie_count > 0 {
        alerts.push(Alert {
            level: AlertLevel::Warning,
            message: format!("{zombie_count} zombie process(es)"),
        });
    }
    alerts
}

/// Dedicated OS thread: sysinfo refreshes are sync and would stall a tokio worker
/// (and delay `q` until the next `.await`). Disk enumeration runs on a side
/// thread so a stuck volume cannot pause CPU/mem snapshots. Snapshot writes run
/// on `ku-persist` so a full `VACUUM` cannot stall the UI. `stop` is checked
/// between steps; the current CPU/process refresh cannot be interrupted.
pub fn run(config: Config, tx: watch::Sender<Snapshot>, storage: Storage, stop: &AtomicBool) {
    if stop.load(Ordering::Relaxed) {
        return;
    }
    let mut collector = Collector::new(config.clone());
    if wait_or_stop(stop, sysinfo::MINIMUM_CPU_UPDATE_INTERVAL) {
        return;
    }

    let interval = Duration::from_secs(config.general.refresh_interval.max(1));
    let mut last_growth = Instant::now()
        .checked_sub(Duration::from_secs(config.disk.snapshot_interval))
        .unwrap_or_else(Instant::now);
    let watched = config.disk.watched_paths.clone();
    let snapshot_every = Duration::from_secs(config.disk.snapshot_interval.max(30));
    let retention_days = config.general.history_retention_days;
    // First pass ~45s after start so the UI is up; then hourly even if growth
    // is slow or skipped.
    const MAINTAIN_EVERY: Duration = Duration::from_secs(3600);
    let mut last_maintain = Instant::now()
        .checked_sub(MAINTAIN_EVERY.saturating_sub(Duration::from_secs(45)))
        .unwrap_or_else(Instant::now);
    let persist = PersistJoin::spawn(storage.clone());
    // One growth scan at a time, prune included. Two scans blocked on the
    // same lock would reread one snapshot and the later insert would replace
    // the newer measurements.
    let growth_running = Arc::new(AtomicBool::new(false));

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let snap = collector.collect();
        if stop.load(Ordering::Relaxed) {
            break;
        }
        debug!(
            cpu = snap.cpu.global,
            mem = snap.memory.used_pct(),
            procs = snap.process_count,
            "collected snapshot"
        );
        // Publish before enqueue. `ku-persist` may block in VACUUM; the
        // header clock must not wait for that lock.
        let published = tx.send(snap.clone()).is_ok();
        persist.slot.push(snap);
        if !published {
            break;
        }
        if last_growth.elapsed() >= snapshot_every
            && growth_running
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            let paths = watched.clone();
            let store = storage.clone();
            let running = Arc::clone(&growth_running);
            match std::thread::Builder::new()
                .name("ku-growth".into())
                .spawn(move || {
                    struct Running(Arc<AtomicBool>);
                    impl Drop for Running {
                        fn drop(&mut self) {
                            self.0.store(false, Ordering::Release);
                        }
                    }
                    let _running = Running(running);
                    let scan = growth::scan_paths(&paths);
                    if scan.truncated {
                        match store.latest_dir_snapshot() {
                            Ok((ts, previous)) => {
                                let merged =
                                    growth::merge_unfinished(scan.sizes, &scan.keep, &previous);
                                warn!(
                                    kept = merged.len(),
                                    exact = scan.keep.exact.len(),
                                    unlisted = scan.keep.unlisted.len(),
                                    untouched = scan.keep.untouched.len(),
                                    "growth scan incomplete; kept prior sizes for unfinished paths"
                                );
                                persist_growth(&store, &merged, ts, true);
                            }
                            Err(err) => {
                                warn!(
                                    error = %err,
                                    "growth scan incomplete; kept the previous snapshot"
                                );
                            }
                        }
                    } else {
                        match store.latest_dir_snapshot() {
                            Ok((ts, _)) => persist_growth(&store, &scan.sizes, ts, true),
                            Err(err) => {
                                warn!(
                                    error = %err,
                                    "could not read the latest growth snapshot"
                                );
                                persist_growth(&store, &scan.sizes, None, false);
                            }
                        }
                    }
                    if let Err(err) = store.prune(retention_days) {
                        warn!(error = %err, "failed to prune history");
                    }
                }) {
                Ok(_) => last_growth = Instant::now(),
                Err(err) => {
                    growth_running.store(false, Ordering::Release);
                    warn!(error = %err, "failed to start growth scan");
                }
            }
        }
        if last_maintain.elapsed() >= MAINTAIN_EVERY {
            last_maintain = Instant::now();
            let store = storage.clone();
            std::thread::spawn(move || {
                if let Err(err) = store.prune(retention_days) {
                    warn!(error = %err, "failed to prune/compact history");
                }
            });
        }
        if wait_or_stop(stop, interval) {
            break;
        }
    }
}

/// Sleep `total` in small slices so shutdown is noticed without waiting the full interval.
/// Returns true if `stop` was set.
fn wait_or_stop(stop: &AtomicBool, total: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < total {
        if stop.load(Ordering::Relaxed) {
            return true;
        }
        let left = total.saturating_sub(start.elapsed());
        std::thread::sleep(left.min(Duration::from_millis(50)));
    }
    stop.load(Ordering::Relaxed)
}

fn persist_snapshot(storage: &Storage, snap: &Snapshot) -> anyhow::Result<()> {
    storage.insert_snapshot(snap)
}

fn persist_growth(
    store: &Storage,
    sizes: &[growth::PathSize],
    expected: Option<i64>,
    check: bool,
) {
    if sizes.is_empty() {
        warn!("growth scan produced no sizes");
        return;
    }
    let result = if check {
        store.insert_dirs_if_current(sizes, expected)
    } else {
        store.insert_dirs(sizes).map(|()| true)
    };
    match result {
        Ok(true) => {}
        Ok(false) => warn!("growth snapshot changed during the scan; dropped this one"),
        Err(err) => warn!(error = %err, "failed to persist growth snapshot"),
    }
}

/// One pending snapshot. A push replaces whatever has not been taken yet,
/// so a long `VACUUM` cannot queue a write per sample.
struct LatestSlot<T> {
    inner: Mutex<SlotInner<T>>,
    cvar: Condvar,
}

struct SlotInner<T> {
    pending: Option<T>,
    dropped: u64,
    closed: bool,
}

impl<T> LatestSlot<T> {
    fn new() -> Self {
        Self {
            inner: Mutex::new(SlotInner {
                pending: None,
                dropped: 0,
                closed: false,
            }),
            cvar: Condvar::new(),
        }
    }

    fn push(&self, value: T) {
        let mut guard = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        if guard.closed {
            return;
        }
        if guard.pending.is_some() {
            guard.dropped = guard.dropped.saturating_add(1);
        }
        guard.pending = Some(value);
        self.cvar.notify_one();
    }

    /// `None` once the slot is closed and empty. The dropped count is how
    /// many pushes were overwritten since the previous take.
    fn pop(&self) -> Option<(T, u64)> {
        let mut guard = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        loop {
            if let Some(value) = guard.pending.take() {
                let dropped = std::mem::take(&mut guard.dropped);
                return Some((value, dropped));
            }
            if guard.closed {
                return None;
            }
            guard = self.cvar.wait(guard).unwrap_or_else(|err| err.into_inner());
        }
    }

    fn close(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        guard.closed = true;
        self.cvar.notify_one();
    }
}

fn persist_loop(storage: Storage, slot: Arc<LatestSlot<Snapshot>>) {
    while let Some((snap, dropped)) = slot.pop() {
        if dropped > 0 {
            info!(dropped, "coalesced snapshot writes");
        }
        if let Err(err) = persist_snapshot(&storage, &snap) {
            warn!(error = %err, "failed to persist snapshot");
        }
    }
}

struct PersistJoin {
    slot: Arc<LatestSlot<Snapshot>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl PersistJoin {
    fn spawn(storage: Storage) -> Self {
        let slot = Arc::new(LatestSlot::new());
        let slot_worker = Arc::clone(&slot);
        let handle = match std::thread::Builder::new()
            .name("ku-persist".into())
            .spawn(move || persist_loop(storage, slot_worker))
        {
            Ok(handle) => Some(handle),
            Err(err) => {
                warn!(
                    error = %err,
                    "failed to spawn ku-persist; snapshots stay on screen only"
                );
                slot.close();
                None
            }
        };
        Self { slot, handle }
    }
}

impl Drop for PersistJoin {
    fn drop(&mut self) {
        self.slot.close();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn live_snapshot_has_cpu_and_memory() {
        let mut collector = Collector::new(Config::default());
        let snap = collector.collect();
        assert!(snap.memory.total > 0);
        assert!(!snap.cpu.cores.is_empty());
        assert!(snap.process_count > 0);
    }

    #[test]
    fn wait_or_stop_returns_immediately_when_flagged() {
        let stop = AtomicBool::new(true);
        let start = Instant::now();
        assert!(wait_or_stop(&stop, Duration::from_secs(5)));
        assert!(start.elapsed() < Duration::from_millis(200));
    }

    #[test]
    fn collect_does_not_block_on_disk_sampler() {
        let mut collector = Collector::new(Config::default());
        let start = Instant::now();
        let snap = collector.collect();
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(snap.memory.total > 0);
    }

    fn tagged(uptime_secs: u64) -> Snapshot {
        Snapshot {
            uptime_secs,
            ..Snapshot::default()
        }
    }

    #[test]
    fn latest_slot_keeps_the_newest_snapshot() {
        let slot = LatestSlot::new();
        slot.push(tagged(1));
        slot.push(tagged(2));
        slot.push(tagged(3));
        slot.close();
        let (snap, dropped) = slot.pop().unwrap();
        assert_eq!(snap.uptime_secs, 3);
        assert_eq!(dropped, 2);
        assert!(slot.pop().is_none());
    }

    #[test]
    fn latest_slot_ignores_push_after_close() {
        let slot = LatestSlot::new();
        slot.close();
        slot.push(tagged(1));
        assert!(slot.pop().is_none());
    }

    #[test]
    fn latest_slot_flushes_the_pending_snapshot_on_close() {
        let slot = LatestSlot::new();
        slot.push(tagged(4));
        slot.close();
        let (snap, dropped) = slot.pop().unwrap();
        assert_eq!(snap.uptime_secs, 4);
        assert_eq!(dropped, 0);
        assert!(slot.pop().is_none());
    }

    #[test]
    fn persist_worker_writes_the_coalesced_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::storage::Storage::open(&dir.path().join("ku.db")).unwrap();
        let slot = Arc::new(LatestSlot::new());
        slot.push(tagged(1));
        slot.push(tagged(2));
        slot.push(tagged(3));
        slot.close();
        let db_worker = db.clone();
        let slot_worker = Arc::clone(&slot);
        let worker = std::thread::spawn(move || persist_loop(db_worker, slot_worker));
        worker.join().unwrap();
        assert_eq!(db.sample_count().unwrap(), 1);
    }
}
