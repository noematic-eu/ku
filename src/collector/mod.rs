pub mod cpu;
pub mod disk;
pub mod growth;
pub mod memory;
pub mod process;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use sysinfo::{Disks, ProcessRefreshKind, ProcessesToUpdate, System, Users};
use tokio::sync::watch;
use tracing::{debug, warn};

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
    disks: Disks,
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
        Self {
            sys,
            disks: Disks::new_with_refreshed_list(),
            users: Users::new_with_refreshed_list(),
            cpu_hist: VecDeque::with_capacity(HISTORY_LEN),
            mem_hist: VecDeque::with_capacity(HISTORY_LEN),
            config,
            ticks: 0,
        }
    }

    pub fn collect(&mut self) -> Snapshot {
        self.sys.refresh_cpu_all();
        self.sys.refresh_memory();
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::everything().without_tasks(),
        );
        self.disks.refresh(true);
        if self.ticks.is_multiple_of(15) {
            self.users.refresh();
        }
        self.ticks += 1;

        let cpu = cpu::collect(&self.sys);
        let memory = memory::collect(&self.sys);
        let disks = disk::collect(&self.disks);
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
/// (and delay `q` until the next `.await`). `stop` is checked between steps; the
/// current refresh cannot be interrupted, but the UI no longer waits for it.
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

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let snap = collector.collect();
        if stop.load(Ordering::Relaxed) {
            break;
        }
        if let Err(err) = persist_snapshot(&storage, &snap) {
            warn!(error = %err, "failed to persist snapshot");
        }
        if last_growth.elapsed() >= snapshot_every {
            last_growth = Instant::now();
            let paths = watched.clone();
            let store = storage.clone();
            std::thread::spawn(move || {
                let sizes = growth::scan_paths(&paths);
                if let Err(err) = store.insert_dirs(&sizes) {
                    warn!(error = %err, "failed to persist growth snapshot");
                }
                if let Err(err) = store.prune(retention_days) {
                    warn!(error = %err, "failed to prune history");
                }
            });
        }
        debug!(
            cpu = snap.cpu.global,
            mem = snap.memory.used_pct(),
            procs = snap.process_count,
            "collected snapshot"
        );
        if tx.send(snap).is_err() {
            break;
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
    storage.insert_metrics(snap)?;
    storage.insert_disks(&snap.disks)?;
    storage.insert_processes(&snap.processes)?;
    Ok(())
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
}
