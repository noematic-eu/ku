use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};

use crate::collector::growth::{GrowthRow, PathSize};
use crate::collector::{DiskSnapshot, ProcessSnapshot, Snapshot};
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

    pub fn insert_metrics(&self, snap: &Snapshot) -> Result<()> {
        let ts = snap.collected_at.timestamp();
        let conn = self.lock()?;
        conn.execute(
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
        Ok(())
    }

    pub fn insert_disks(&self, disks: &[DiskSnapshot]) -> Result<()> {
        let ts = chrono::Local::now().timestamp();
        let conn = self.lock()?;
        let tx = conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO disks (ts, mount, fs, total, used, available) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for disk in disks {
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
        tx.commit()?;
        Ok(())
    }

    pub fn insert_processes(&self, processes: &[ProcessSnapshot]) -> Result<()> {
        let ts = chrono::Local::now().timestamp();
        let conn = self.lock()?;
        let tx = conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO processes (ts, pid, name, user, cpu, mem, virt, status, cmd)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for proc in processes
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
        let ts = chrono::Local::now().timestamp();
        let conn = self.lock()?;
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

    pub fn prune(&self, retention_days: u32) -> Result<()> {
        let cutoff = chrono::Local::now().timestamp() - i64::from(retention_days) * 86_400;
        let conn = self.lock()?;
        for table in ["metrics", "disks", "processes", "dirs"] {
            conn.execute(
                &format!("DELETE FROM {table} WHERE ts < ?1"),
                params![cutoff],
            )?;
        }
        Ok(())
    }

    pub fn top_processes(&self, window: &str, limit: usize) -> Result<Vec<ProcessAgg>> {
        let secs = parse_duration_window(window).unwrap_or(300);
        let since = chrono::Local::now().timestamp() - secs;
        let conn = self.lock()?;
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
        let conn = self.lock()?;
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
        let conn = self.lock()?;
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

    pub fn last_growth_ts(&self) -> Result<Option<i64>> {
        let conn = self.lock()?;
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
}
