use sysinfo::Disks;

use crate::config::DiskConfig;
use crate::utils::{inode_usage, percent};

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

pub fn collect(disks: &Disks) -> Vec<DiskSnapshot> {
    let mut out: Vec<DiskSnapshot> = disks
        .list()
        .iter()
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
        .filter(|d| d.total > 0)
        .collect();
    out.sort_by(|a, b| a.mount.cmp(&b.mount));
    out
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
}
