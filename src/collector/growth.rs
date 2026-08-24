use std::collections::{HashMap, HashSet};
use std::path::Path;

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
    "proc",
    "sys",
    "dev",
];

const MAX_ENTRIES_PER_ROOT: usize = 80_000;
const MAX_DEPTH: usize = 12;

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

pub fn scan_paths(paths: &[String]) -> Vec<PathSize> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for raw in paths {
        let path = Path::new(raw);
        if !path.exists() {
            continue;
        }
        let key = path.to_string_lossy().into_owned();
        if seen.insert(key.clone()) {
            out.push(PathSize {
                path: key,
                size: dir_size(path),
            });
        }
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                let child = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') && name != ".local" {
                    continue;
                }
                let child_key = child.to_string_lossy().into_owned();
                if seen.insert(child_key.clone()) {
                    let size = if child.is_dir() {
                        dir_size(&child)
                    } else {
                        entry.metadata().map(|m| m.len()).unwrap_or(0)
                    };
                    out.push(PathSize {
                        path: child_key,
                        size,
                    });
                }
            }
        }
    }
    out
}

pub fn dir_size(path: &Path) -> u64 {
    let mut total = 0u64;
    let mut counted = 0usize;
    let walker = WalkDir::new(path)
        .follow_links(false)
        .max_depth(MAX_DEPTH)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !SKIP_DIR_NAMES.iter().any(|s| name.eq_ignore_ascii_case(s))
        });
    for entry in walker.flatten() {
        counted += 1;
        if counted > MAX_ENTRIES_PER_ROOT {
            break;
        }
        if entry.file_type().is_file()
            && let Ok(meta) = entry.metadata()
        {
            total = total.saturating_add(meta.len());
        }
    }
    total
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
            if skip_name(&name) {
                continue;
            }
            let child = entry.path();
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
        let sizes = scan_paths(&[dir.path().to_string_lossy().into_owned()]);
        assert!(sizes.iter().any(|s| s.size >= 150));
        assert!(sizes.iter().any(|s| s.path.ends_with("sub")));
    }
}
