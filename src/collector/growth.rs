use std::collections::HashMap;
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
                }
            }
            None => GrowthRow {
                path: p.path.clone(),
                size: p.size,
                abs_delta: 0,
                rel_delta: None,
                is_new: true,
            },
        })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.abs_delta.unsigned_abs()));
    rows
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
