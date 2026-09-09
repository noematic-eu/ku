use std::path::Path;
use std::sync::mpsc::{self, TryRecvError};
use std::time::{Duration, Instant};

use sysinfo::Disks;

use crate::config::DiskConfig;
use crate::utils::{inode_usage, percent};

const DISK_REFRESH: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Default)]
pub struct DiskSnapshot {
    pub name: String,
    pub mount: String,
    pub fs: String,
    pub kind: String,
    pub total: u64,
    pub available: u64,
    pub used: u64,
    pub inodes_used: Option<u64>,
    pub inodes_total: Option<u64>,
    pub removable: bool,
    pub read_only: bool,
}

impl DiskSnapshot {
    pub fn used_pct(&self) -> f64 {
        percent(self.used, self.total)
    }

    pub fn inode_pct(&self) -> Option<f64> {
        Some(percent(self.inodes_used?, self.inodes_total?))
    }
}

/// Background disk list so a stuck volume (Time Machine, autofs, SMB)
/// cannot stall the collector — and freeze the TUI.
pub struct Sampler {
    last: Vec<DiskSnapshot>,
    rx: Option<mpsc::Receiver<Vec<DiskSnapshot>>>,
    last_ok: Option<Instant>,
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler {
    pub fn new() -> Self {
        Self {
            last: Vec::new(),
            rx: None,
            last_ok: None,
        }
    }

    pub fn tick(&mut self) {
        self.poll();
        // A wedged refresh keeps `rx` occupied: we never kick again, and
        // `last` stays at the previous good list (or empty). The UI "stale"
        // badge keys off snapshot time, which still moves with CPU/mem, so
        // disks can be frozen without that badge. Accepted: one leaked
        // `ku-disks` thread rather than stacking hung IOKit/statvfs calls.
        if self.rx.is_some() {
            return;
        }
        let due = self
            .last_ok
            .map(|t| t.elapsed() >= DISK_REFRESH)
            .unwrap_or(true);
        if due {
            self.kick();
        }
    }

    /// Block up to `timeout` for the first list. Never waits on a hung refresh.
    pub fn wait(&mut self, timeout: Duration) {
        let start = Instant::now();
        self.tick();
        while start.elapsed() < timeout {
            self.poll();
            if !self.last.is_empty() || self.rx.is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
    }

    pub fn snapshots(&self) -> &[DiskSnapshot] {
        &self.last
    }

    fn poll(&mut self) {
        let Some(rx) = &self.rx else {
            return;
        };
        match rx.try_recv() {
            Ok(list) => {
                self.last = list;
                self.last_ok = Some(Instant::now());
                self.rx = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.rx = None;
            }
        }
    }

    fn kick(&mut self) {
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let spawned = std::thread::Builder::new()
            .name("ku-disks".into())
            .spawn(move || {
                let sys_disks = Disks::new_with_refreshed_list();
                let _ = tx.send(collect(&sys_disks));
            });
        if spawned.is_err() {
            self.rx = None;
        }
    }
}

pub fn collect(disks: &Disks) -> Vec<DiskSnapshot> {
    let mut out: Vec<DiskSnapshot> = disks
        .list()
        .iter()
        .filter(|disk| !skip_mount(&disk.file_system().to_string_lossy(), disk.mount_point()))
        .filter(|disk| disk.total_space() > 0)
        .map(|disk| {
            let total = disk.total_space();
            let available = disk.available_space();
            let used = total.saturating_sub(available);
            let mount = disk.mount_point().to_string_lossy().into_owned();
            let inodes = inode_usage(disk.mount_point());
            DiskSnapshot {
                name: disk.name().to_string_lossy().into_owned(),
                mount,
                fs: disk.file_system().to_string_lossy().into_owned(),
                kind: format!("{:?}", disk.kind()),
                total,
                available,
                used,
                inodes_used: inodes.map(|v| v.0),
                inodes_total: inodes.map(|v| v.1),
                removable: disk.is_removable(),
                read_only: disk.is_read_only(),
            }
        })
        .collect();
    out.sort_by(|a, b| a.mount.cmp(&b.mount));
    out
}

pub(crate) fn skip_mount(fs: &str, mount: &Path) -> bool {
    let fs = fs.to_ascii_lowercase();
    if matches!(
        fs.as_str(),
        "autofs" | "devfs" | "devtmpfs" | "proc" | "sysfs" | "ramfs" | "overlay"
    ) {
        return true;
    }
    mount == Path::new("/dev")
        || mount == Path::new("/System/Volumes/Data/home")
        || mount.starts_with("/proc")
        || mount.starts_with("/sys")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskAlertLevel {
    Ok,
    Warning,
    Critical,
}

pub fn alert_level(pct: f64, cfg: &DiskConfig) -> DiskAlertLevel {
    if pct >= f64::from(cfg.critical_threshold) {
        DiskAlertLevel::Critical
    } else if pct >= f64::from(cfg.warning_threshold) {
        DiskAlertLevel::Warning
    } else {
        DiskAlertLevel::Ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds() {
        let cfg = DiskConfig {
            warning_threshold: 80,
            critical_threshold: 90,
            watched_paths: vec![],
            snapshot_interval: 300,
        };
        assert_eq!(alert_level(10.0, &cfg), DiskAlertLevel::Ok);
        assert_eq!(alert_level(80.0, &cfg), DiskAlertLevel::Warning);
        assert_eq!(alert_level(95.0, &cfg), DiskAlertLevel::Critical);
    }

    #[test]
    fn skips_virtual_and_automounts() {
        assert!(skip_mount("autofs", Path::new("/System/Volumes/Data/home")));
        assert!(skip_mount("devfs", Path::new("/dev")));
        assert!(skip_mount("apfs", Path::new("/dev")));
        assert!(!skip_mount("apfs", Path::new("/")));
        assert!(!skip_mount("apfs", Path::new("/System/Volumes/Data")));
        assert!(!skip_mount("ext4", Path::new("/")));
    }
}
