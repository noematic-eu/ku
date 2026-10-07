use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use walkdir::WalkDir;

const SKIP_DIR_NAMES: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    ".Trash",
    "Caches",
    ".cache",
    ".npm",
    ".cargo",
    "Library",
    "Mobile Documents",
    "iCloud Drive",
    "proc",
    "sys",
    "dev",
];

const MAX_ENTRIES_PER_ROOT: usize = 80_000;
const MAX_DEPTH: usize = 12;
const SCAN_BUDGET: Duration = Duration::from_secs(20);
const DIR_BUDGET: Duration = Duration::from_secs(8);

/// Bumps once per directory listing so a slow child is not first on every scan.
static CHILD_ROTATION: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, PartialEq)]
pub struct PathSize {
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GrowthRow {
    pub path: String,
    pub size: u64,
    pub abs_delta: i64,
    pub rel_delta: Option<f64>,
    pub is_new: bool,
    pub is_gone: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContribKind {
    File,
    Dir,
}

impl ContribKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Dir => "dir",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContribChange {
    Grew,
    Shrunk,
    New,
    Gone,
    Unchanged,
    /// Current size only; this child was not in the previous scan.
    Now,
}

impl ContribChange {
    pub fn label(self) -> &'static str {
        match self {
            Self::Grew => "grew",
            Self::Shrunk => "shrunk",
            Self::New => "new",
            Self::Gone => "gone",
            Self::Unchanged => "same",
            Self::Now => "now",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExplainSource {
    /// Direct children were stored in the growth snapshots (real Δ).
    Snapshot,
    /// Listed the folder now. Δ only if that child path was snapshotted.
    Live,
}

impl ExplainSource {
    pub fn caption(self) -> &'static str {
        match self {
            Self::Snapshot => "why it changed  ·  dir = recursive size  ·  file = itself",
            Self::Live => {
                "largest entries now (no child history)  ·  dir = recursive  ·  file = itself"
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Contribution {
    pub path: String,
    pub name: String,
    pub kind: Option<ContribKind>,
    pub size: u64,
    pub abs_delta: Option<i64>,
    pub change: ContribChange,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GrowthExplain {
    pub path: String,
    pub size: u64,
    pub abs_delta: i64,
    pub source: ExplainSource,
    pub rows: Vec<Contribution>,
    pub selected: usize,
}

/// What to copy from the previous snapshot after a scan that did not finish.
///
/// A finished directory listing must not keep the whole root: names missing
/// from that listing were deleted. Only paths we did not actually revisit stay.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Unfinished {
    /// Child we did not finish. Keep this path and everything under it.
    pub exact: Vec<String>,
    /// `read_dir` stopped early. Keep rows strictly under the path, not the path itself.
    pub unlisted: Vec<String>,
    /// The root was never opened. Keep the path and everything under it.
    pub untouched: Vec<String>,
    /// Watched roots measured completely. Their previous rows are stale.
    pub finished: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScanResult {
    /// Directories and files that were measured completely.
    pub sizes: Vec<PathSize>,
    /// A directory hit the time or entry cap. Its size is not in `sizes`.
    pub truncated: bool,
    pub keep: Unfinished,
}

pub fn scan_paths(paths: &[String]) -> ScanResult {
    scan_paths_until(paths, Instant::now() + SCAN_BUDGET)
}

fn scan_paths_until(paths: &[String], deadline: Instant) -> ScanResult {
    scan_paths_limited(paths, deadline, MAX_ENTRIES_PER_ROOT)
}

fn scan_paths_limited(paths: &[String], deadline: Instant, max_entries: usize) -> ScanResult {
    let mut sizes = Vec::new();
    let mut keep = Unfinished::default();
    let mut truncated = false;
    for raw in paths {
        if Instant::now() >= deadline {
            truncated = true;
            keep.untouched.push(raw.clone());
            continue;
        }
        let path = Path::new(raw);
        if !path.exists() || is_cloud_placeholder(path) {
            continue;
        }
        let measured = measure_root(path, deadline, max_entries);
        truncated |= measured.truncated;
        sizes.extend(measured.sizes);
        keep.exact.extend(measured.exact);
        if let Some(path) = measured.unlisted {
            keep.unlisted.push(path);
        }
        if let Some(path) = measured.untouched {
            keep.untouched.push(path);
        }
        if let Some(path) = measured.finished {
            keep.finished.push(path);
        }
    }
    ScanResult {
        sizes: dedupe_paths(sizes),
        truncated,
        keep,
    }
}

fn dedupe_paths(sizes: Vec<PathSize>) -> Vec<PathSize> {
    let mut seen = HashSet::new();
    sizes
        .into_iter()
        .filter(|row| seen.insert(row.path.clone()))
        .collect()
}

struct Measured {
    sizes: Vec<PathSize>,
    exact: Vec<String>,
    unlisted: Option<String>,
    untouched: Option<String>,
    finished: Option<String>,
    truncated: bool,
}

/// Size a watched directory from its children. Files are measured before
/// directory walks, and the walk order rotates so one slow child is not first
/// on every scan. The root total is stored only when every child finished.
fn measure_root(path: &Path, deadline: Instant, max_entries: usize) -> Measured {
    let key = path.to_string_lossy().into_owned();
    if path.is_file() {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        return Measured {
            sizes: vec![PathSize {
                path: key.clone(),
                size,
            }],
            exact: Vec::new(),
            unlisted: None,
            untouched: None,
            finished: Some(key),
            truncated: false,
        };
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return Measured {
            sizes: Vec::new(),
            exact: Vec::new(),
            unlisted: None,
            untouched: Some(key),
            finished: None,
            truncated: true,
        };
    };

    let mut sizes = Vec::new();
    let mut exact = Vec::new();
    let mut total = 0u64;
    let mut listing_done = true;
    let mut files = Vec::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        if Instant::now() >= deadline {
            listing_done = false;
            break;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if skip_name(&name) || is_cloud_placeholder(&entry.path()) {
            continue;
        }
        let child = entry.path();
        if child.is_dir() {
            dirs.push(child);
        } else {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            files.push((child, size));
        }
    }
    for (child, size) in files {
        total = total.saturating_add(size);
        sizes.push(PathSize {
            path: child.to_string_lossy().into_owned(),
            size,
        });
    }
    if !dirs.is_empty() {
        let rot = CHILD_ROTATION.fetch_add(1, Ordering::Relaxed) % dirs.len();
        dirs.rotate_left(rot);
    }
    let mut dirs_done = true;
    for (index, child) in dirs.iter().enumerate() {
        if Instant::now() >= deadline {
            dirs_done = false;
            for rest in dirs.iter().skip(index) {
                exact.push(rest.to_string_lossy().into_owned());
            }
            break;
        }
        let child_key = child.to_string_lossy().into_owned();
        // One folder gets at most DIR_BUDGET. The rest of the scan stays
        // available for siblings and the other watched roots.
        let child_deadline = Instant::now()
            .checked_add(DIR_BUDGET)
            .map(|cap| cap.min(deadline))
            .unwrap_or(deadline);
        let walked = dir_size_until(child, child_deadline, max_entries);
        if walked.truncated {
            dirs_done = false;
            exact.push(child_key);
            continue;
        }
        total = total.saturating_add(walked.size);
        sizes.push(PathSize {
            path: child_key,
            size: walked.size,
        });
    }
    let complete = listing_done && dirs_done;
    if complete {
        sizes.push(PathSize {
            path: key.clone(),
            size: total,
        });
    }
    Measured {
        sizes,
        exact,
        // Names we never saw are not deletions. The root total is still omitted.
        unlisted: if listing_done {
            None
        } else {
            Some(key.clone())
        },
        untouched: None,
        finished: if complete { Some(key) } else { None },
        truncated: !complete,
    }
}

/// Keep the previous size of every path this scan did not revisit.
/// Rows under `keep.finished` stay dropped: that root was measured completely.
pub fn merge_unfinished(
    mut current: Vec<PathSize>,
    keep: &Unfinished,
    previous: &[PathSize],
) -> Vec<PathSize> {
    let mut have: HashSet<String> = current.iter().map(|row| row.path.clone()).collect();
    for prev in previous {
        if have.contains(&prev.path) {
            continue;
        }
        if keep
            .finished
            .iter()
            .any(|root| path_under(root, &prev.path))
        {
            continue;
        }
        let retain = keep.exact.iter().any(|root| path_under(root, &prev.path))
            || keep
                .untouched
                .iter()
                .any(|root| path_under(root, &prev.path))
            || keep
                .unlisted
                .iter()
                .any(|root| strict_under(root, &prev.path));
        if retain {
            have.insert(prev.path.clone());
            current.push(prev.clone());
        }
    }
    current
}

fn strict_under(root: &str, path: &str) -> bool {
    path_under(root, path) && path.trim_end_matches('/') != root.trim_end_matches('/')
}

fn path_under(root: &str, path: &str) -> bool {
    let root = root.trim_end_matches('/');
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}

pub fn dir_size(path: &Path) -> u64 {
    dir_size_until(path, Instant::now() + DIR_BUDGET, MAX_ENTRIES_PER_ROOT).size
}

struct Walked {
    size: u64,
    truncated: bool,
}

fn dir_size_until(path: &Path, deadline: Instant, max_entries: usize) -> Walked {
    let mut total = 0u64;
    let mut counted = 0usize;
    let walker = WalkDir::new(path)
        .follow_links(false)
        .max_depth(MAX_DEPTH)
        .into_iter()
        .filter_entry(|e| !skip_walk_entry(e));
    for entry in walker.flatten() {
        if Instant::now() >= deadline {
            return Walked {
                size: total,
                truncated: true,
            };
        }
        counted += 1;
        if counted > max_entries {
            return Walked {
                size: total,
                truncated: true,
            };
        }
        if is_cloud_placeholder(entry.path()) {
            continue;
        }
        if entry.file_type().is_file()
            && let Ok(meta) = entry.metadata()
        {
            total = total.saturating_add(meta.len());
        }
    }
    Walked {
        size: total,
        truncated: false,
    }
}

fn skip_walk_entry(e: &walkdir::DirEntry) -> bool {
    let name = e.file_name().to_string_lossy();
    SKIP_DIR_NAMES.iter().any(|s| name.eq_ignore_ascii_case(s)) || is_cloud_placeholder(e.path())
}

#[cfg(target_os = "macos")]
fn is_cloud_placeholder(path: &Path) -> bool {
    use std::os::macos::fs::MetadataExt;
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    // UF_DATALESS: content lives in iCloud and is not on disk. Statting it
    // can hang while macOS tries to materialize the file.
    const UF_DATALESS: u32 = 0x4000_0000;
    (meta.st_flags() & UF_DATALESS) != 0
}

#[cfg(not(target_os = "macos"))]
fn is_cloud_placeholder(_path: &Path) -> bool {
    false
}

pub fn compute_deltas(current: &[PathSize], previous: &[PathSize]) -> Vec<GrowthRow> {
    let prev: HashMap<&str, u64> = previous.iter().map(|p| (p.path.as_str(), p.size)).collect();
    let curr: HashMap<&str, u64> = current.iter().map(|p| (p.path.as_str(), p.size)).collect();
    let mut rows: Vec<GrowthRow> = current
        .iter()
        .map(|p| match prev.get(p.path.as_str()) {
            Some(&old) => {
                let abs = p.size as i64 - old as i64;
                let rel = if old == 0 {
                    if p.size == 0 { 0.0 } else { 100.0 }
                } else {
                    abs as f64 / old as f64 * 100.0
                };
                GrowthRow {
                    path: p.path.clone(),
                    size: p.size,
                    abs_delta: abs,
                    rel_delta: Some(rel),
                    is_new: false,
                    is_gone: false,
                }
            }
            None => GrowthRow {
                path: p.path.clone(),
                size: p.size,
                abs_delta: p.size as i64,
                rel_delta: None,
                is_new: true,
                is_gone: false,
            },
        })
        .collect();
    for p in previous {
        if curr.contains_key(p.path.as_str()) {
            continue;
        }
        rows.push(GrowthRow {
            path: p.path.clone(),
            size: p.size,
            abs_delta: -(p.size as i64),
            rel_delta: Some(-100.0),
            is_new: false,
            is_gone: true,
        });
    }
    rows.sort_by_key(|r| std::cmp::Reverse(r.abs_delta.unsigned_abs()));
    rows
}

pub fn is_direct_child(parent: &str, path: &str) -> bool {
    let parent = parent.trim_end_matches('/');
    if parent.is_empty() {
        return path.starts_with('/') && path.len() > 1 && !path[1..].contains('/');
    }
    let Some(rest) = path.strip_prefix(parent) else {
        return false;
    };
    let Some(rest) = rest.strip_prefix('/') else {
        return false;
    };
    !rest.is_empty() && !rest.contains('/')
}

fn child_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| path.to_string())
}

fn classify_kind(path: &str) -> Option<ContribKind> {
    let p = Path::new(path);
    if !p.exists() {
        return None;
    }
    Some(if p.is_dir() {
        ContribKind::Dir
    } else {
        ContribKind::File
    })
}

fn change_from_row(row: &GrowthRow) -> ContribChange {
    if row.is_gone {
        ContribChange::Gone
    } else if row.is_new {
        ContribChange::New
    } else if row.abs_delta > 0 {
        ContribChange::Grew
    } else if row.abs_delta < 0 {
        ContribChange::Shrunk
    } else {
        ContribChange::Unchanged
    }
}

fn row_to_contrib(row: GrowthRow) -> Contribution {
    let name = child_name(&row.path);
    let kind = classify_kind(&row.path);
    let change = change_from_row(&row);
    Contribution {
        path: row.path,
        name,
        kind,
        size: row.size,
        abs_delta: Some(row.abs_delta),
        change,
    }
}

fn sort_contribs(rows: &mut [Contribution]) {
    rows.sort_by_key(|r| std::cmp::Reverse(r.abs_delta.unwrap_or(r.size as i64).unsigned_abs()));
}

fn snapshot_children(
    parent: &str,
    current: &[PathSize],
    previous: &[PathSize],
) -> Vec<Contribution> {
    let cur: Vec<PathSize> = current
        .iter()
        .filter(|p| is_direct_child(parent, &p.path))
        .cloned()
        .collect();
    let prev: Vec<PathSize> = previous
        .iter()
        .filter(|p| is_direct_child(parent, &p.path))
        .cloned()
        .collect();
    if cur.is_empty() && prev.is_empty() {
        return Vec::new();
    }
    let mut rows: Vec<Contribution> = compute_deltas(&cur, &prev)
        .into_iter()
        .map(row_to_contrib)
        .collect();
    sort_contribs(&mut rows);
    rows
}

fn skip_name(name: &str) -> bool {
    if name.starts_with('.') && name != ".local" {
        return true;
    }
    SKIP_DIR_NAMES.iter().any(|s| name.eq_ignore_ascii_case(s))
}

fn explain_live(parent: &str, previous: &[PathSize]) -> Vec<Contribution> {
    let prev: HashMap<&str, u64> = previous.iter().map(|p| (p.path.as_str(), p.size)).collect();
    let has_child_history = previous.iter().any(|p| is_direct_child(parent, &p.path));
    let mut seen = HashSet::new();
    let mut rows = Vec::new();
    if let Ok(entries) = std::fs::read_dir(parent) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let child = entry.path();
            if skip_name(&name) || is_cloud_placeholder(&child) {
                continue;
            }
            let key = child.to_string_lossy().into_owned();
            seen.insert(key.clone());
            let is_dir = child.is_dir();
            let size = if is_dir {
                dir_size(&child)
            } else {
                entry.metadata().map(|m| m.len()).unwrap_or(0)
            };
            let (change, abs_delta) = if let Some(&old) = prev.get(key.as_str()) {
                let d = size as i64 - old as i64;
                let ch = if d > 0 {
                    ContribChange::Grew
                } else if d < 0 {
                    ContribChange::Shrunk
                } else {
                    ContribChange::Unchanged
                };
                (ch, Some(d))
            } else if has_child_history {
                (ContribChange::New, Some(size as i64))
            } else {
                (ContribChange::Now, None)
            };
            rows.push(Contribution {
                path: key,
                name,
                kind: Some(if is_dir {
                    ContribKind::Dir
                } else {
                    ContribKind::File
                }),
                size,
                abs_delta,
                change,
            });
        }
    }
    if has_child_history {
        for p in previous {
            if is_direct_child(parent, &p.path) && seen.insert(p.path.clone()) {
                rows.push(Contribution {
                    path: p.path.clone(),
                    name: child_name(&p.path),
                    kind: classify_kind(&p.path),
                    size: p.size,
                    abs_delta: Some(-(p.size as i64)),
                    change: ContribChange::Gone,
                });
            }
        }
    }
    sort_contribs(&mut rows);
    rows
}

pub fn explain(
    path: &str,
    size: u64,
    abs_delta: i64,
    current: &[PathSize],
    previous: &[PathSize],
) -> GrowthExplain {
    let snap = snapshot_children(path, current, previous);
    let (source, rows) = if snap.is_empty() {
        (ExplainSource::Live, explain_live(path, previous))
    } else {
        (ExplainSource::Snapshot, snap)
    };
    GrowthExplain {
        path: path.to_string(),
        size,
        abs_delta,
        source,
        rows,
        selected: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn deltas_grow_and_shrink() {
        let prev = vec![
            PathSize {
                path: "/var/log".into(),
                size: 1000,
            },
            PathSize {
                path: "/tmp".into(),
                size: 500,
            },
        ];
        let now = vec![
            PathSize {
                path: "/var/log".into(),
                size: 1500,
            },
            PathSize {
                path: "/tmp".into(),
                size: 200,
            },
            PathSize {
                path: "/new".into(),
                size: 80,
            },
        ];
        let rows = compute_deltas(&now, &prev);
        let log = rows.iter().find(|r| r.path == "/var/log").unwrap();
        assert_eq!(log.abs_delta, 500);
        assert!((log.rel_delta.unwrap() - 50.0).abs() < f64::EPSILON);
        let tmp = rows.iter().find(|r| r.path == "/tmp").unwrap();
        assert_eq!(tmp.abs_delta, -300);
        let new = rows.iter().find(|r| r.path == "/new").unwrap();
        assert!(new.is_new);
        assert_eq!(new.abs_delta, 80);
        assert!(!rows.iter().any(|r| r.is_gone));
    }

    #[test]
    fn deltas_include_vanished_and_rank_by_contribution() {
        let prev = vec![
            PathSize {
                path: "/keep".into(),
                size: 100,
            },
            PathSize {
                path: "/gone".into(),
                size: 400,
            },
        ];
        let now = vec![
            PathSize {
                path: "/keep".into(),
                size: 110,
            },
            PathSize {
                path: "/new".into(),
                size: 250,
            },
        ];
        let rows = compute_deltas(&now, &prev);
        assert_eq!(rows[0].path, "/gone");
        assert!(rows[0].is_gone);
        assert_eq!(rows[0].abs_delta, -400);
        assert_eq!(rows[1].path, "/new");
        assert!(rows[1].is_new);
        assert_eq!(rows[1].abs_delta, 250);
        assert_eq!(rows[2].path, "/keep");
        assert_eq!(rows[2].abs_delta, 10);
    }

    #[test]
    fn direct_child_one_level_only() {
        assert!(is_direct_child("/home/u", "/home/u/work"));
        assert!(!is_direct_child("/home/u", "/home/u"));
        assert!(!is_direct_child("/home/u", "/home/u/work/dev"));
        assert!(!is_direct_child("/home/u", "/home/other"));
        assert!(is_direct_child("/", "/tmp"));
        assert!(!is_direct_child("/", "/tmp/foo"));
    }

    #[test]
    fn explain_uses_snapshot_children_for_delta() {
        let prev = vec![
            PathSize {
                path: "/data".into(),
                size: 300,
            },
            PathSize {
                path: "/data/a".into(),
                size: 100,
            },
            PathSize {
                path: "/data/gone".into(),
                size: 80,
            },
        ];
        let now = vec![
            PathSize {
                path: "/data".into(),
                size: 400,
            },
            PathSize {
                path: "/data/a".into(),
                size: 250,
            },
            PathSize {
                path: "/data/new".into(),
                size: 70,
            },
        ];
        let expl = explain("/data", 400, 100, &now, &prev);
        assert_eq!(expl.source, ExplainSource::Snapshot);
        assert_eq!(expl.rows[0].name, "a");
        assert_eq!(expl.rows[0].abs_delta, Some(150));
        assert_eq!(expl.rows[0].change, ContribChange::Grew);
        let gone = expl.rows.iter().find(|r| r.name == "gone").unwrap();
        assert_eq!(gone.change, ContribChange::Gone);
        let new = expl.rows.iter().find(|r| r.name == "new").unwrap();
        assert_eq!(new.change, ContribChange::New);
    }

    #[test]
    fn explain_live_marks_file_vs_recursive_dir() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), vec![0u8; 40]).unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("b.txt"), vec![0u8; 80]).unwrap();
        let expl = explain(&dir.path().to_string_lossy(), 0, 0, &[], &[]);
        assert_eq!(expl.source, ExplainSource::Live);
        let file = expl.rows.iter().find(|r| r.name == "a.txt").unwrap();
        assert_eq!(file.kind, Some(ContribKind::File));
        assert_eq!(file.change, ContribChange::Now);
        let nested = expl.rows.iter().find(|r| r.name == "sub").unwrap();
        assert_eq!(nested.kind, Some(ContribKind::Dir));
        assert!(nested.size >= 80);
    }

    #[test]
    fn scan_counts_files() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), vec![0u8; 100]).unwrap();
        let child = dir.path().join("sub");
        fs::create_dir(&child).unwrap();
        fs::write(child.join("b.txt"), vec![0u8; 50]).unwrap();
        let scan = scan_paths(&[dir.path().to_string_lossy().into_owned()]);
        assert!(!scan.truncated);
        assert!(scan.sizes.iter().any(|s| s.size >= 150));
        assert!(scan.sizes.iter().any(|s| s.path.ends_with("sub")));
    }

    #[test]
    fn exhausted_budget_marks_scan_truncated() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), vec![0u8; 10]).unwrap();
        let past = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now);
        let scan = scan_paths_until(&[dir.path().to_string_lossy().into_owned()], past);
        assert!(scan.truncated);
        assert!(scan.sizes.is_empty());
    }

    #[test]
    fn partial_scan_keeps_finished_entries_only() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("ok.txt"), vec![0u8; 10]).unwrap();
        let deep = dir.path().join("deep");
        fs::create_dir(&deep).unwrap();
        fs::write(deep.join("a"), vec![0u8; 1]).unwrap();
        fs::write(deep.join("b"), vec![0u8; 1]).unwrap();
        fs::write(deep.join("c"), vec![0u8; 1]).unwrap();
        let root = dir.path().to_string_lossy().into_owned();
        let scan = scan_paths_limited(&[root.clone()], Instant::now() + SCAN_BUDGET, 1);
        assert!(scan.truncated);
        assert!(
            scan.sizes
                .iter()
                .any(|s| s.path.ends_with("ok.txt") && s.size == 10)
        );
        assert!(
            !scan
                .sizes
                .iter()
                .any(|s| s.path.ends_with("/deep") || s.path.ends_with("\\deep"))
        );
        assert!(!scan.sizes.iter().any(|s| s.path == root));
        let deep_key = deep.to_string_lossy().into_owned();
        assert!(scan.keep.exact.iter().any(|p| p == &deep_key));
        assert!(!scan.keep.exact.iter().any(|p| p == &root));
        assert!(scan.keep.unlisted.is_empty());
        assert!(scan.keep.untouched.is_empty());
        assert!(scan.keep.finished.is_empty());
    }

    fn sample_previous() -> Vec<PathSize> {
        vec![
            PathSize {
                path: "/home".into(),
                size: 500,
            },
            PathSize {
                path: "/home/deep".into(),
                size: 99,
            },
            PathSize {
                path: "/home/deep/old".into(),
                size: 7,
            },
            PathSize {
                path: "/home/gone".into(),
                size: 5,
            },
            PathSize {
                path: "/tmp".into(),
                size: 1,
            },
        ]
    }

    #[test]
    fn merge_unfinished_keeps_skipped_and_drops_deleted() {
        let current = vec![PathSize {
            path: "/home/ok.txt".into(),
            size: 10,
        }];
        let previous = sample_previous();
        // Listing finished. Only `/home/deep` was not measured, so a sibling
        // that the listing no longer contains is a deletion.
        let listed = Unfinished {
            exact: vec!["/home/deep".into()],
            ..Unfinished::default()
        };
        let merged = merge_unfinished(current.clone(), &listed, &previous);
        assert!(
            merged
                .iter()
                .any(|s| s.path == "/home/ok.txt" && s.size == 10)
        );
        assert!(
            merged
                .iter()
                .any(|s| s.path == "/home/deep" && s.size == 99)
        );
        assert!(
            merged
                .iter()
                .any(|s| s.path == "/home/deep/old" && s.size == 7)
        );
        assert!(!merged.iter().any(|s| s.path == "/home"));
        assert!(!merged.iter().any(|s| s.path == "/home/gone"));
        assert!(!merged.iter().any(|s| s.path == "/tmp"));

        let interrupted = Unfinished {
            unlisted: vec!["/home".into()],
            ..Unfinished::default()
        };
        let merged = merge_unfinished(current, &interrupted, &previous);
        assert!(merged.iter().any(|s| s.path == "/home/gone" && s.size == 5));
        assert!(
            merged
                .iter()
                .any(|s| s.path == "/home/deep" && s.size == 99)
        );
        assert!(!merged.iter().any(|s| s.path == "/home"));
        assert!(!merged.iter().any(|s| s.path == "/tmp"));
    }

    #[test]
    fn finished_root_drops_rows_kept_by_an_unfinished_parent() {
        let current = vec![
            PathSize {
                path: "/home/deep".into(),
                size: 40,
            },
            PathSize {
                path: "/home/deep/a".into(),
                size: 3,
            },
        ];
        let keep = Unfinished {
            exact: vec!["/home/deep".into()],
            untouched: vec!["/home".into()],
            finished: vec!["/home/deep".into()],
            ..Unfinished::default()
        };
        let merged = merge_unfinished(current, &keep, &sample_previous());
        assert!(
            merged
                .iter()
                .any(|s| s.path == "/home/deep" && s.size == 40)
        );
        assert!(
            merged
                .iter()
                .any(|s| s.path == "/home/deep/a" && s.size == 3)
        );
        assert!(!merged.iter().any(|s| s.path == "/home/deep/old"));
        assert!(merged.iter().any(|s| s.path == "/home" && s.size == 500));
        assert!(merged.iter().any(|s| s.path == "/home/gone" && s.size == 5));
        assert!(!merged.iter().any(|s| s.path == "/tmp"));
    }
}
