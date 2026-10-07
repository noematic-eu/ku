use std::path::Path;
use std::sync::{Arc, Mutex, TryLockError};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};

use crate::collector::Snapshot;
use crate::collector::growth::{GrowthRow, PathSize};
use crate::utils::parse_duration_window;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS metrics (
    ts INTEGER NOT NULL,
    cpu REAL,
    mem_used INTEGER,
    mem_total INTEGER,
    swap_used INTEGER,
    swap_total INTEGER,
    load1 REAL,
    load5 REAL,
    load15 REAL,
    process_count INTEGER
);
CREATE INDEX IF NOT EXISTS idx_metrics_ts ON metrics(ts);

CREATE TABLE IF NOT EXISTS disks (
    ts INTEGER NOT NULL,
    mount TEXT NOT NULL,
    fs TEXT,
    total INTEGER,
    used INTEGER,
    available INTEGER
);
CREATE INDEX IF NOT EXISTS idx_disks_ts ON disks(ts);

CREATE TABLE IF NOT EXISTS processes (
    ts INTEGER NOT NULL,
    pid INTEGER NOT NULL,
    name TEXT NOT NULL,
    user TEXT,
    cpu REAL,
    mem INTEGER,
    virt INTEGER,
    status TEXT,
    cmd TEXT
);
CREATE INDEX IF NOT EXISTS idx_proc_ts ON processes(ts);
CREATE INDEX IF NOT EXISTS idx_proc_name ON processes(name);

CREATE TABLE IF NOT EXISTS dirs (
    ts INTEGER NOT NULL,
    path TEXT NOT NULL,
    size INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_dirs_ts ON dirs(ts);
CREATE INDEX IF NOT EXISTS idx_dirs_path ON dirs(path);
"#;

#[derive(Clone)]
pub struct Storage {
    inner: Arc<Mutex<Connection>>,
}

/// The single sqlite connection is held elsewhere, usually by `VACUUM`.
/// UI reads treat this as "keep the previous result" instead of waiting.
#[derive(Debug)]
pub struct DbBusy;

impl std::fmt::Display for DbBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("database busy")
    }
}

impl std::error::Error for DbBusy {}

pub fn is_db_busy(err: &anyhow::Error) -> bool {
    err.is::<DbBusy>()
}

#[derive(Debug, Clone)]
pub struct ProcessAgg {
    pub name: String,
    pub avg_cpu: f64,
    pub max_cpu: f64,
    pub avg_mem: u64,
    pub max_mem: u64,
    pub samples: u64,
}

#[derive(Debug, Clone)]
pub struct LeakSuspect {
    pub pid: i64,
    pub name: String,
    pub min_mem: u64,
    pub max_mem: u64,
    pub samples: u64,
}

impl Storage {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
            crate::paths::chown_to_invoker(parent);
        }
        let conn =
            Connection::open(path).with_context(|| format!("opening sqlite {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(2))?;
        // Must be set before CREATE TABLE on a new file. Existing DBs pick
        // this up on the next full VACUUM.
        conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
        conn.execute_batch(SCHEMA)?;
        crate::paths::chown_to_invoker(path);
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            crate::paths::chown_to_invoker(&path.with_file_name(format!("{name}-wal")));
            crate::paths::chown_to_invoker(&path.with_file_name(format!("{name}-shm")));
        }
        Ok(Self {
            inner: Arc::new(Mutex::new(conn)),
        })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.inner
            .lock()
            .map_err(|_| anyhow::anyhow!("sqlite mutex poisoned"))
    }

    #[cfg(test)]
    pub(crate) fn lock_for_test(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.inner.lock().expect("sqlite mutex poisoned")
    }

    /// UI reads use this. `VACUUM` holds [`lock`](Self::lock) for the whole
    /// rewrite; blocking here would freeze the TUI.
    fn try_lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        match self.inner.try_lock() {
            Ok(guard) => Ok(guard),
            Err(TryLockError::WouldBlock) => Err(DbBusy.into()),
            Err(TryLockError::Poisoned(_)) => anyhow::bail!("sqlite mutex poisoned"),
        }
    }

    /// Metrics, disks, and processes share `snap.collected_at`, in one
    /// transaction, so a delayed write still records one sample.
    pub fn insert_snapshot(&self, snap: &Snapshot) -> Result<()> {
        let ts = snap.collected_at.timestamp();
        let conn = self.lock()?;
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO metrics (ts, cpu, mem_used, mem_total, swap_used, swap_total, load1, load5, load15, process_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                ts,
                snap.cpu.global as f64,
                snap.memory.used as i64,
                snap.memory.total as i64,
                snap.memory.swap_used as i64,
                snap.memory.swap_total as i64,
                snap.load.one,
                snap.load.five,
                snap.load.fifteen,
                snap.process_count as i64,
            ],
        )?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO disks (ts, mount, fs, total, used, available) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for disk in &snap.disks {
                stmt.execute(params![
                    ts,
                    disk.mount,
                    disk.fs,
                    disk.total as i64,
                    disk.used as i64,
                    disk.available as i64,
                ])?;
            }
        }
        {
            let mut stmt = tx.prepare(
                "INSERT INTO processes (ts, pid, name, user, cpu, mem, virt, status, cmd)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for proc in snap
                .processes
                .iter()
                .filter(|p| p.cpu >= 0.3 || p.mem >= 8 * 1024 * 1024)
            {
                stmt.execute(params![
                    ts,
                    proc.pid as i64,
                    proc.name,
                    proc.user,
                    proc.cpu as f64,
                    proc.mem as i64,
                    proc.virt as i64,
                    proc.status,
                    crate::utils::truncate_ellipsis(&proc.cmd, 240),
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn insert_dirs(&self, sizes: &[PathSize]) -> Result<()> {
        let conn = self.lock()?;
        let latest = max_dir_ts(&conn)?;
        write_dirs(&conn, sizes, next_dir_ts(latest))
    }

    /// Insert only when `expected` is still `MAX(ts)`. A scan that read an
    /// older snapshot must not overwrite one that landed while it was running.
    pub fn insert_dirs_if_current(
        &self,
        sizes: &[PathSize],
        expected: Option<i64>,
    ) -> Result<bool> {
        let conn = self.lock()?;
        let latest = max_dir_ts(&conn)?;
        if latest != expected {
            return Ok(false);
        }
        write_dirs(&conn, sizes, next_dir_ts(latest))?;
        Ok(true)
    }

    pub fn prune(&self, retention_days: u32) -> Result<()> {
        let cutoff = chrono::Local::now().timestamp() - i64::from(retention_days) * 86_400;
        let conn = self.lock()?;
        for table in ["metrics", "disks", "processes", "dirs"] {
            conn.execute(
                &format!("DELETE FROM {table} WHERE ts < ?1"),
                params![cutoff],
            )?;
        }
        reclaim(&conn, false)?;
        Ok(())
    }

    pub fn stats(&self) -> Result<DbStats> {
        let conn = self.lock()?;
        db_stats(&conn)
    }

    /// Reclaim unused pages. `force` always runs a full `VACUUM`.
    pub fn compact(&self, force: bool) -> Result<CompactReport> {
        let conn = self.lock()?;
        let before = db_stats(&conn)?;
        let vacuumed = reclaim(&conn, force)?;
        let after = db_stats(&conn)?;
        if vacuumed {
            tracing::info!(
                before = before.bytes(),
                after = after.bytes(),
                freed = before.wasted(),
                "compacted history.db"
            );
        }
        Ok(CompactReport {
            before,
            after,
            vacuumed,
        })
    }

    pub fn top_processes(&self, window: &str, limit: usize) -> Result<Vec<ProcessAgg>> {
        let secs = parse_duration_window(window).unwrap_or(300);
        let since = chrono::Local::now().timestamp() - secs;
        let conn = self.try_lock()?;
        let mut stmt = conn.prepare(
            "SELECT name, AVG(cpu), MAX(cpu), AVG(mem), MAX(mem), COUNT(*)
             FROM processes
             WHERE ts >= ?1
             GROUP BY name
             ORDER BY AVG(cpu) DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![since, limit as i64], |row| {
            Ok(ProcessAgg {
                name: row.get(0)?,
                avg_cpu: row.get(1)?,
                max_cpu: row.get(2)?,
                avg_mem: row.get::<_, f64>(3)? as u64,
                max_mem: row.get::<_, i64>(4)? as u64,
                samples: row.get::<_, i64>(5)? as u64,
            })
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    pub fn leak_suspects(&self, window: &str, limit: usize) -> Result<Vec<LeakSuspect>> {
        let secs = parse_duration_window(window).unwrap_or(3600);
        let since = chrono::Local::now().timestamp() - secs;
        let conn = self.try_lock()?;
        let mut stmt = conn.prepare(
            "SELECT pid, name, MIN(mem), MAX(mem), COUNT(*)
             FROM processes
             WHERE ts >= ?1
             GROUP BY pid, name
             HAVING COUNT(*) >= 5
                AND MAX(mem) > MIN(mem) * 1.5
                AND MAX(mem) - MIN(mem) > 52428800
             ORDER BY (MAX(mem) - MIN(mem)) DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![since, limit as i64], |row| {
            Ok(LeakSuspect {
                pid: row.get(0)?,
                name: row.get(1)?,
                min_mem: row.get::<_, i64>(2)? as u64,
                max_mem: row.get::<_, i64>(3)? as u64,
                samples: row.get::<_, i64>(4)? as u64,
            })
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    pub fn growth_snapshots(&self, window_secs: i64) -> Result<(Vec<PathSize>, Vec<PathSize>)> {
        let now = chrono::Local::now().timestamp();
        let target = now - window_secs;
        let conn = self.try_lock()?;
        let latest: Option<i64> = conn
            .query_row("SELECT MAX(ts) FROM dirs", [], |row| row.get(0))
            .optional()?
            .flatten();
        let Some(latest) = latest else {
            return Ok((Vec::new(), Vec::new()));
        };
        let previous: Option<i64> = conn
            .query_row(
                "SELECT MAX(ts) FROM dirs WHERE ts <= ?1",
                params![target],
                |row| row.get(0),
            )
            .optional()?
            .flatten()
            .or_else(|| {
                conn.query_row("SELECT MIN(ts) FROM dirs", [], |row| row.get(0))
                    .optional()
                    .ok()
                    .flatten()
                    .flatten()
            });
        let current = load_dirs(&conn, latest)?;
        let previous = match previous {
            Some(ts) if ts != latest => load_dirs(&conn, ts)?,
            _ => Vec::new(),
        };
        Ok((current, previous))
    }

    pub fn growth_for_window(&self, window_secs: i64) -> Result<Vec<GrowthRow>> {
        let (current, previous) = self.growth_snapshots(window_secs)?;
        Ok(crate::collector::growth::compute_deltas(
            &current, &previous,
        ))
    }

    pub fn latest_dir_snapshot(&self) -> Result<(Option<i64>, Vec<PathSize>)> {
        let conn = self.lock()?;
        let latest = max_dir_ts(&conn)?;
        match latest {
            Some(ts) => Ok((Some(ts), load_dirs(&conn, ts)?)),
            None => Ok((None, Vec::new())),
        }
    }

    pub fn last_growth_ts(&self) -> Result<Option<i64>> {
        let conn = self.try_lock()?;
        let ts: Option<i64> = conn
            .query_row("SELECT MAX(ts) FROM dirs", [], |row| row.get(0))
            .optional()?
            .flatten();
        Ok(ts)
    }

    pub fn sample_count(&self) -> Result<u64> {
        let conn = self.lock()?;
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM metrics", [], |row| row.get(0))?;
        Ok(n as u64)
    }
}

/// Logical SQLite file size (page_count × page_size), not including WAL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DbStats {
    pub page_count: u64,
    pub freelist: u64,
    pub page_size: u64,
}

impl DbStats {
    pub fn bytes(self) -> u64 {
        self.page_count.saturating_mul(self.page_size)
    }

    pub fn wasted(self) -> u64 {
        self.freelist.saturating_mul(self.page_size)
    }

    pub fn should_compact(self) -> bool {
        const MIN_WASTED: u64 = 8 * 1024 * 1024;
        const MIN_FILE: u64 = 1024 * 1024;
        self.wasted() >= MIN_WASTED
            || (self.bytes() >= MIN_FILE
                && self.freelist * 2 > self.page_count
                && self.freelist > 64)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactReport {
    pub before: DbStats,
    pub after: DbStats,
    pub vacuumed: bool,
}

fn db_stats(conn: &Connection) -> Result<DbStats> {
    let page_count: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
    let freelist: i64 = conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
    let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
    Ok(DbStats {
        page_count: page_count.max(0) as u64,
        freelist: freelist.max(0) as u64,
        page_size: page_size.max(0) as u64,
    })
}

/// Give free pages back to the OS. Incremental first (cheap); full VACUUM
/// only when the file is still mostly empty or `force` is set.
fn reclaim(conn: &Connection, force: bool) -> Result<bool> {
    conn.execute_batch("PRAGMA incremental_vacuum")?;
    let mid = db_stats(conn)?;
    if !force && !mid.should_compact() {
        return Ok(false);
    }
    if mid.freelist == 0 && !force {
        return Ok(false);
    }
    conn.execute_batch("VACUUM")?;
    let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)");
    Ok(true)
}

fn max_dir_ts(conn: &Connection) -> Result<Option<i64>> {
    Ok(conn
        .query_row("SELECT MAX(ts) FROM dirs", [], |row| row.get(0))
        .optional()?
        .flatten())
}

/// Two scans can finish in the same second. Sharing `ts` would concatenate
/// both snapshots into one growth row set.
fn next_dir_ts(latest: Option<i64>) -> i64 {
    let now = chrono::Local::now().timestamp();
    match latest {
        Some(prev) if now <= prev => prev.saturating_add(1),
        _ => now,
    }
}

fn write_dirs(conn: &Connection, sizes: &[PathSize], ts: i64) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare("INSERT INTO dirs (ts, path, size) VALUES (?1, ?2, ?3)")?;
        for row in sizes {
            stmt.execute(params![ts, row.path, row.size as i64])?;
        }
    }
    tx.commit()?;
    Ok(())
}

fn load_dirs(conn: &Connection, ts: i64) -> Result<Vec<PathSize>> {
    let mut stmt = conn.prepare("SELECT path, size FROM dirs WHERE ts = ?1")?;
    let rows = stmt.query_map(params![ts], |row| {
        Ok(PathSize {
            path: row.get(0)?,
            size: row.get::<_, i64>(1)? as u64,
        })
    })?;
    Ok(rows.filter_map(Result::ok).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::growth::PathSize;
    use tempfile::tempdir;

    #[test]
    fn roundtrip_dirs_and_growth() {
        let dir = tempdir().unwrap();
        let db = Storage::open(&dir.path().join("ku.db")).unwrap();
        db.insert_dirs(&[PathSize {
            path: "/var/log".into(),
            size: 100,
        }])
        .unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        db.insert_dirs(&[PathSize {
            path: "/var/log".into(),
            size: 180,
        }])
        .unwrap();
        let rows = db.growth_for_window(0).unwrap();
        assert!(rows.iter().any(|r| r.path == "/var/log"));
    }

    #[test]
    fn insert_dirs_rejects_a_stale_snapshot_and_does_not_share_ts() {
        let dir = tempdir().unwrap();
        let db = Storage::open(&dir.path().join("ku.db")).unwrap();
        db.insert_dirs(&[PathSize {
            path: "/home/a".into(),
            size: 1,
        }])
        .unwrap();
        let (first_ts, _) = db.latest_dir_snapshot().unwrap();
        db.insert_dirs(&[PathSize {
            path: "/home/a".into(),
            size: 2,
        }])
        .unwrap();
        let wrote = db
            .insert_dirs_if_current(
                &[PathSize {
                    path: "/home/a".into(),
                    size: 3,
                }],
                first_ts,
            )
            .unwrap();
        assert!(!wrote);
        let (latest_ts, rows) = db.latest_dir_snapshot().unwrap();
        assert_ne!(latest_ts, first_ts);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].size, 2);
        let distinct: i64 = db
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(DISTINCT ts) FROM dirs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(distinct, 2);
    }

    #[test]
    fn should_compact_when_file_is_mostly_free() {
        let bloated = DbStats {
            page_count: 10_000,
            freelist: 8_000,
            page_size: 4096,
        };
        assert!(bloated.should_compact());
        assert_eq!(bloated.wasted(), 8_000 * 4096);
        let tiny = DbStats {
            page_count: 20,
            freelist: 15,
            page_size: 4096,
        };
        assert!(!tiny.should_compact());
        let healthy = DbStats {
            page_count: 10_000,
            freelist: 10,
            page_size: 4096,
        };
        assert!(!healthy.should_compact());
    }

    #[test]
    fn compact_reclaims_deleted_pages() {
        let dir = tempdir().unwrap();
        let db = Storage::open(&dir.path().join("ku.db")).unwrap();
        let rows: Vec<PathSize> = (0..400)
            .map(|i| PathSize {
                path: format!("/var/log/ku-test-{i:04}"),
                size: i * 1024,
            })
            .collect();
        db.insert_dirs(&rows).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(2));
        db.prune(0).unwrap();
        let before = db.stats().unwrap();
        let report = db.compact(true).unwrap();
        assert!(report.vacuumed);
        assert_eq!(report.after.freelist, 0);
        assert!(report.after.bytes() <= before.bytes());
        assert!(report.after.bytes() <= report.before.bytes());
    }

    #[test]
    fn insert_snapshot_uses_one_collected_at() {
        use crate::collector::{DiskSnapshot, ProcessSnapshot};

        let dir = tempdir().unwrap();
        let db = Storage::open(&dir.path().join("ku.db")).unwrap();
        let ts = 1_700_000_000i64;
        let collected_at = chrono::TimeZone::timestamp_opt(&chrono::Local, ts, 0)
            .single()
            .unwrap();
        let snap = Snapshot {
            collected_at,
            disks: vec![DiskSnapshot {
                mount: "/".into(),
                fs: "apfs".into(),
                total: 100,
                used: 40,
                available: 60,
                ..DiskSnapshot::default()
            }],
            processes: vec![ProcessSnapshot {
                pid: 42,
                name: "ku".into(),
                user: "me".into(),
                cpu: 1.5,
                mem: 10_000_000,
                ..ProcessSnapshot::default()
            }],
            process_count: 1,
            ..Snapshot::default()
        };
        db.insert_snapshot(&snap).unwrap();
        let conn = db.lock().unwrap();
        for table in ["metrics", "disks", "processes"] {
            let got: i64 = conn
                .query_row(&format!("SELECT ts FROM {table}"), [], |row| row.get(0))
                .unwrap();
            assert_eq!(got, collected_at.timestamp(), "{table}");
        }
    }

    #[test]
    fn reads_fail_fast_while_the_connection_is_held() {
        use std::sync::mpsc;
        use std::time::Duration;

        let dir = tempdir().unwrap();
        let db = Storage::open(&dir.path().join("ku.db")).unwrap();
        let db2 = db.clone();
        let _guard = db.lock().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send((db2.top_processes("5m", 10), db2.growth_for_window(3600)));
        });
        let (top, growth) = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("read blocked on the database lock");
        assert!(is_db_busy(&top.unwrap_err()));
        assert!(is_db_busy(&growth.unwrap_err()));
    }
}
