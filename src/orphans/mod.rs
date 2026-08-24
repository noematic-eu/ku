mod elevate;
mod fda;
mod identity;
mod ignore;
mod installed;
mod leftovers;

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use identity::{extract_bundle_id, is_apple_id, last_component, slug, tokens_match, vendor_prefix};
use installed::{InstalledApp, inventory_cancellable};
use leftovers::{Candidate, PathKind, is_safe_to_delete};

pub use elevate::{
    ElevateMethod, RetryKind, detect as elevate_detect, force_remove, remove as elevate_remove,
    retry as elevate_retry,
};
pub use fda::{
    app_hint as fda_app_hint, has_full_disk_access, missing as fda_missing, needed as fda_needed,
    open_settings as open_fda_settings,
};
pub use ignore::{IgnoreRule, add_ignore, apply_ignore, is_ignored, remove_ignore};
pub use leftovers::PathKind as OrphanPathKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

impl Confidence {
    pub fn label(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }

    /// High first.
    pub fn rank(self) -> u8 {
        match self {
            Self::High => 0,
            Self::Medium => 1,
            Self::Low => 2,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct OrphanPath {
    pub path: PathBuf,
    pub kind: PathKind,
    pub size: u64,
    pub mtime: Option<i64>,
    pub location: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OrphanApp {
    pub id: String,
    pub name: String,
    pub confidence: Confidence,
    pub reason: String,
    pub size: u64,
    pub paths: Vec<OrphanPath>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OrphanReport {
    pub installed_apps: usize,
    pub apps: Vec<OrphanApp>,
}

impl OrphanReport {
    pub fn total_size(&self) -> u64 {
        self.apps.iter().map(|a| a.size).sum()
    }

    pub fn find_app(&self, key: &str) -> Option<&OrphanApp> {
        let k = key.trim();
        self.apps.iter().find(|a| {
            a.id.eq_ignore_ascii_case(k)
                || a.name.eq_ignore_ascii_case(k)
                || slug(&a.id) == slug(k)
                || slug(&a.name) == slug(k)
        })
    }

    pub fn find_path(&self, path: &Path) -> Option<(&OrphanApp, &OrphanPath)> {
        let target = path.to_string_lossy();
        for app in &self.apps {
            for p in &app.paths {
                if p.path == path || p.path.to_string_lossy() == target {
                    return Some((app, p));
                }
            }
        }
        None
    }

    pub fn flatten_rows(&self) -> Vec<OrphanRow> {
        let mut rows = Vec::new();
        for app in &self.apps {
            for p in &app.paths {
                rows.push(OrphanRow {
                    app_id: app.id.clone(),
                    app_name: app.name.clone(),
                    path: p.path.clone(),
                    size: p.size,
                    confidence: app.confidence,
                    reason: app.reason.clone(),
                    related: app.paths.len(),
                    deleted: false,
                    mtime: p.mtime,
                });
            }
        }
        rows.sort_by(|a, b| b.size.cmp(&a.size));
        rows
    }
}

#[derive(Debug, Clone)]
pub struct OrphanRow {
    pub app_id: String,
    pub app_name: String,
    pub path: PathBuf,
    pub size: u64,
    pub confidence: Confidence,
    pub reason: String,
    pub related: usize,
    pub deleted: bool,
    pub mtime: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrphanSort {
    Size,
    Level,
    Age,
}

impl OrphanSort {
    pub fn next(self) -> Self {
        match self {
            Self::Size => Self::Level,
            Self::Level => Self::Age,
            Self::Age => Self::Size,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Size => "size",
            Self::Level => "level",
            Self::Age => "age",
        }
    }
}

pub fn sort_rows(rows: &mut [OrphanRow], sort: OrphanSort, reverse: bool) {
    rows.sort_by(|a, b| {
        let ord = match sort {
            OrphanSort::Size => b.size.cmp(&a.size),
            OrphanSort::Level => a
                .confidence
                .rank()
                .cmp(&b.confidence.rank())
                .then_with(|| b.size.cmp(&a.size)),
            OrphanSort::Age => match (a.mtime, b.mtime) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
            .then_with(|| b.size.cmp(&a.size)),
        };
        if reverse { ord.reverse() } else { ord }
    });
}

pub fn scan() -> Result<OrphanReport> {
    scan_with_cancel(&std::sync::atomic::AtomicBool::new(false))
}

pub fn scan_with_cancel(cancel: &std::sync::atomic::AtomicBool) -> Result<OrphanReport> {
    use std::sync::atomic::Ordering;
    if cancel.load(Ordering::Relaxed) {
        bail!("cancelled");
    }
    let installed = inventory_cancellable(cancel);
    if cancel.load(Ordering::Relaxed) {
        bail!("cancelled");
    }
    let candidates = leftovers::collect_cancellable(cancel);
    if cancel.load(Ordering::Relaxed) {
        bail!("cancelled");
    }
    Ok(match_orphans(&installed, candidates))
}

fn match_orphans(installed: &[InstalledApp], candidates: Vec<Candidate>) -> OrphanReport {
    let mut groups: BTreeMap<String, Vec<Candidate>> = BTreeMap::new();
    for c in candidates {
        groups.entry(c.app_key.clone()).or_default().push(c);
    }

    let mut apps = Vec::new();
    for (key, paths) in groups {
        if key.is_empty() {
            continue;
        }
        if let Some(id) = extract_bundle_id(&key)
            && is_apple_id(&id)
        {
            continue;
        }
        if is_still_installed(installed, &key) {
            continue;
        }
        if vendor_still_installed(installed, &key) {
            continue;
        }
        let (confidence, reason) = score(&key, &paths);
        let name = display_name(&key, &paths);
        let size = paths.iter().map(|p| p.size).sum();
        let paths = paths
            .into_iter()
            .map(|c| OrphanPath {
                path: c.path,
                kind: c.kind,
                size: c.size,
                mtime: c.mtime,
                location: c.location,
            })
            .collect();
        apps.push(OrphanApp {
            id: key,
            name,
            confidence,
            reason,
            size,
            paths,
        });
    }
    apps.sort_by(|a, b| b.size.cmp(&a.size));
    OrphanReport {
        installed_apps: installed.len(),
        apps,
    }
}

fn is_still_installed(installed: &[InstalledApp], key: &str) -> bool {
    let key_slug = slug(key);
    if key_slug == "homebrew"
        && (std::path::Path::new("/opt/homebrew").exists()
            || std::path::Path::new("/usr/local/Homebrew").exists())
    {
        return true;
    }
    installed.iter().any(|a| {
        if a.matches_key(key) {
            return true;
        }
        if let Some(id) = extract_bundle_id(key)
            && (a.matches_key(&id) || a.matches_key(last_component(&id)))
        {
            return true;
        }
        if tokens_match(key, &a.id) || tokens_match(key, &a.name) {
            return true;
        }
        if a.aliases.iter().any(|alias| tokens_match(key, alias)) {
            return true;
        }
        if key_slug.len() >= 5 {
            let id_slug = slug(&a.id);
            let name_slug = slug(&a.name);
            let last_slug = slug(last_component(&a.id));
            if id_slug.contains(&key_slug) || name_slug.contains(&key_slug) {
                return true;
            }
            if common_prefix_len(&key_slug, &name_slug) >= 5
                || common_prefix_len(&key_slug, &id_slug) >= 5
                || common_prefix_len(&key_slug, &last_slug) >= 5
            {
                return true;
            }
        }
        false
    })
}

fn common_prefix_len(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
}

fn vendor_still_installed(installed: &[InstalledApp], key: &str) -> bool {
    let Some(id) = extract_bundle_id(key) else {
        return false;
    };
    let Some(prefix) = vendor_prefix(&id) else {
        return false;
    };
    if is_apple_id(&prefix) {
        return true;
    }
    installed.iter().any(|a| {
        a.id.to_ascii_lowercase()
            .starts_with(&prefix.to_ascii_lowercase())
            && a.id.to_ascii_lowercase() != id.to_ascii_lowercase()
    })
}

fn score(key: &str, paths: &[Candidate]) -> (Confidence, String) {
    let has_bundle = extract_bundle_id(key).is_some()
        || paths.iter().any(|p| extract_bundle_id(&p.name).is_some());
    let has_container = paths.iter().any(|p| p.location.contains("Containers"));
    let has_prefs = paths.iter().any(|p| p.location.contains("Preferences"));
    if has_bundle && (has_container || has_prefs) {
        (
            Confidence::High,
            "bundle id present in leftover data, no matching installed app".into(),
        )
    } else if has_bundle {
        (
            Confidence::High,
            "reverse-dns leftover, no matching installed app".into(),
        )
    } else if paths.iter().any(|p| {
        matches!(
            p.location.as_str(),
            "Application Support" | ".config" | ".local/share" | "flatpak" | "snap"
        )
    }) {
        (
            Confidence::Medium,
            "named data directory with no matching installed app".into(),
        )
    } else {
        (
            Confidence::Low,
            "cache/log-style leftover, no matching installed app".into(),
        )
    }
}

fn display_name(key: &str, paths: &[Candidate]) -> String {
    if let Some(id) = extract_bundle_id(key) {
        let last = last_component(&id);
        if last.len() >= 3 {
            return last.to_string();
        }
        return id;
    }
    paths
        .first()
        .map(|p| p.name.trim_end_matches(".plist").to_string())
        .unwrap_or_else(|| key.to_string())
}

pub fn resolve_delete_targets(
    report: &OrphanReport,
    target: &str,
    all: bool,
) -> Result<Vec<PathBuf>> {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        bail!("--rm requires a file, directory, or app id");
    }
    let as_path = PathBuf::from(trimmed);
    if as_path.exists() || trimmed.contains('/') || trimmed.starts_with('~') {
        let path = ignore::expand_tilde(&as_path);
        if let Some((app, item)) = report.find_path(&path) {
            if all {
                return Ok(app.paths.iter().map(|p| p.path.clone()).collect());
            }
            return Ok(vec![item.path.clone()]);
        }
        // Allow deleting a file inside an orphan dir.
        for app in &report.apps {
            for p in &app.paths {
                if path.starts_with(&p.path) {
                    if all {
                        return Ok(app.paths.iter().map(|x| x.path.clone()).collect());
                    }
                    return Ok(vec![path]);
                }
            }
        }
        bail!(
            "{} is not an identified leftover — refuse to delete unknown paths",
            path.display()
        );
    }
    let Some(app) = report.find_app(trimmed) else {
        bail!("no leftover app matches `{trimmed}`");
    };
    if !all {
        bail!(
            "`{trimmed}` is an app id with {} leftover path(s). Pass --all to delete every related file/dir, or --rm <path> for a single item.",
            app.paths.len()
        );
    }
    Ok(app.paths.iter().map(|p| p.path.clone()).collect())
}

#[derive(Debug, Clone)]
pub struct DeleteFail {
    pub path: PathBuf,
    pub error: String,
    pub permission: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DeleteOutcome {
    pub removed: Vec<PathBuf>,
    pub failed: Vec<DeleteFail>,
}

impl DeleteOutcome {
    pub fn permission_denied(&self) -> bool {
        self.failed.iter().any(|f| f.permission)
    }

    pub fn incomplete(&self) -> bool {
        !self.failed.is_empty()
    }

    pub fn print_cli(&self) {
        println!("removed {} path(s)", self.removed.len());
        for p in &self.removed {
            println!("  {}", p.display());
        }
        if self.failed.is_empty() {
            return;
        }
        eprintln!("could not delete {} path(s):", self.failed.len());
        for f in self.failed.iter().take(20) {
            eprintln!("  {} ({})", f.path.display(), f.error);
        }
        if self.failed.len() > 20 {
            eprintln!("  … {} more", self.failed.len() - 20);
        }
        if self.permission_denied() {
            if crate::orphans::fda_missing() {
                eprintln!(
                    "macOS Full Disk Access is off — add {} in System Settings → Privacy,",
                    crate::orphans::fda_app_hint()
                );
                eprintln!("then relaunch ku (sudo does not bypass this).  ku orphans --fda");
            } else if !running_as_root() {
                eprintln!("often a permission issue — press e in the TUI, or: sudo ku");
            }
        }
    }
}

pub fn running_as_root() -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

pub fn delete_targets(paths: &[PathBuf]) -> DeleteOutcome {
    let mut out = DeleteOutcome::default();
    for path in paths {
        if !is_safe_to_delete(path) && path.exists() {
            out.failed.push(DeleteFail {
                path: path.clone(),
                error: "protected path".into(),
                permission: true,
            });
            continue;
        }
        let mut local = Vec::new();
        let gone = delete_tree(path, &mut local);
        local.retain(|f| f.path.exists());
        if gone && !path.exists() {
            out.removed.push(path.clone());
        } else if path.exists() {
            if local.is_empty() {
                out.failed.push(DeleteFail {
                    path: path.clone(),
                    error: "still present".into(),
                    permission: true,
                });
            } else {
                out.failed.extend(local);
            }
        }
    }
    out.failed.sort_by(|a, b| a.path.cmp(&b.path));
    out.failed.dedup_by(|a, b| a.path == b.path);
    out
}

fn delete_tree(path: &Path, failed: &mut Vec<DeleteFail>) -> bool {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return true,
        Err(e) => {
            failed.push(fail(path, &e));
            return false;
        }
    };
    let is_dir = meta.file_type().is_dir() && !meta.file_type().is_symlink();
    if is_dir {
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                delete_tree(&entry.path(), failed);
            }
        }
        match fs::remove_dir(path) {
            Ok(()) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
            Err(e) => {
                failed.push(fail(path, &e));
                false
            }
        }
    } else {
        match fs::remove_file(path) {
            Ok(()) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
            Err(e) => {
                failed.push(fail(path, &e));
                false
            }
        }
    }
}

fn fail(path: &Path, err: &std::io::Error) -> DeleteFail {
    DeleteFail {
        path: path.to_path_buf(),
        error: err.to_string(),
        permission: is_permission(err),
    }
}

fn is_permission(err: &std::io::Error) -> bool {
    if err.kind() == std::io::ErrorKind::PermissionDenied {
        return true;
    }
    matches!(err.raw_os_error(), Some(libc::EACCES | libc::EPERM))
}

pub fn search_query(row: &OrphanRow) -> String {
    row.path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if !row.app_name.is_empty() {
                row.app_name.clone()
            } else {
                row.app_id.clone()
            }
        })
}

pub fn google_search_url(query: &str) -> String {
    format!(
        "https://www.google.com/search?q={}",
        urlencode_query(query.trim())
    )
}

fn urlencode_query(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn open_google_search(query: &str) -> Result<()> {
    let q = query.trim();
    if q.is_empty() {
        bail!("empty search query");
    }
    let url = google_search_url(q);
    let mut child = if cfg!(target_os = "macos") {
        Command::new("open").arg(&url).spawn()
    } else if cfg!(target_os = "windows") {
        Command::new("cmd").args(["/C", "start", "", &url]).spawn()
    } else {
        Command::new("xdg-open").arg(&url).spawn()
    }
    .with_context(|| format!("opening {url}"))?;
    let _ = child.stdout.take();
    let _ = child.stderr.take();
    Ok(())
}

pub fn confirm_yes(prompt: &str) -> Result<bool> {
    eprint!("{prompt} [y/N] ");
    let _ = io::stderr().flush();
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes" | "delete"))
}

pub fn confirm_delete(paths: &[PathBuf], prompt: &str, interactive: bool) -> Result<bool> {
    if paths.is_empty() {
        return Ok(false);
    }
    eprintln!("{prompt}");
    for p in paths {
        eprintln!("  {}", p.display());
    }
    if !interactive {
        bail!("refusing to delete without confirmation (-i)");
    }
    eprint!("Type `delete` to confirm: ");
    let _ = io::stderr().flush();
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(line.trim() == "delete")
}

pub fn print_table(report: &OrphanReport) {
    if report.apps.is_empty() {
        println!(
            "no leftover app data found ({} installed apps scanned)",
            report.installed_apps
        );
        return;
    }
    println!(
        "{} leftover app(s), {}  ({} installed apps)",
        report.apps.len(),
        crate::utils::format_bytes(report.total_size()),
        report.installed_apps
    );
    println!();
    for app in &report.apps {
        println!(
            "{:<8}  {:>10}  {:<28}  {}",
            app.confidence.label(),
            crate::utils::format_bytes(app.size),
            crate::utils::truncate_ellipsis(&app.id, 28),
            app.name
        );
        println!("          {}", app.reason);
        for p in &app.paths {
            println!(
                "            {:>10}  {}",
                crate::utils::format_bytes(p.size),
                p.path.display()
            );
        }
        println!();
    }
    println!("dry-run only. delete with:");
    println!("  ku orphans --rm <file-or-dir>");
    println!("  ku orphans --rm <app-id> --all");
    println!("hide a false positive: ku orphans --ignore <app-id-or-path>");
}

#[cfg(test)]
mod tests {
    use super::*;
    use leftovers::PathKind;

    fn cand(key: &str, name: &str, location: &str) -> Candidate {
        Candidate {
            path: PathBuf::from(format!("/tmp/orphan-test/{name}")),
            name: name.into(),
            kind: PathKind::Dir,
            size: 100,
            mtime: None,
            location: location.into(),
            app_key: key.into(),
        }
    }

    #[test]
    fn bundle_without_app_is_high() {
        let report = match_orphans(
            &[],
            vec![cand("com.example.gone", "com.example.gone", "Containers")],
        );
        assert_eq!(report.apps.len(), 1);
        assert_eq!(report.apps[0].confidence, Confidence::High);
        assert_eq!(report.apps[0].id, "com.example.gone");
    }

    #[test]
    fn name_prefix_links_vendor_folder() {
        let installed = vec![
            InstalledApp {
                id: "com.brave.Browser".into(),
                name: "Brave Browser".into(),
                aliases: vec![],
                source: "t".into(),
                path: None,
            }
            .clone(),
        ];
        // with_aliases not called — matches_key on "BraveSoftware" fails; prefix "brave" hits.
        let installed = {
            let mut a = installed;
            a[0] = InstalledApp {
                id: "com.brave.Browser".into(),
                name: "Brave Browser".into(),
                aliases: vec![
                    "bravebrowser".into(),
                    "combravebrowser".into(),
                    "browser".into(),
                ],
                source: "t".into(),
                path: None,
            };
            a
        };
        let report = match_orphans(
            &installed,
            vec![cand(
                "BraveSoftware",
                "BraveSoftware",
                "Application Support",
            )],
        );
        assert!(report.apps.is_empty());
    }

    #[test]
    fn reason_owns_propellerhead_support() {
        let installed = vec![InstalledApp {
            id: "se.propellerheads.reason".into(),
            name: "Reason".into(),
            aliases: vec!["propellerheads".into(), "propellerhead".into()],
            source: "applications".into(),
            path: None,
        }];
        let report = match_orphans(
            &installed,
            vec![cand(
                "Propellerhead Software",
                "Propellerhead Software",
                "Application Support",
            )],
        );
        assert!(
            report.apps.is_empty(),
            "got {:?}",
            report.apps.iter().map(|a| &a.id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn steam_game_is_not_orphan() {
        let installed = vec![InstalledApp {
            id: "steam:Snacktorio".into(),
            name: "Snacktorio".into(),
            aliases: vec!["snacktorio".into()],
            source: "steam".into(),
            path: None,
        }];
        let report = match_orphans(
            &installed,
            vec![cand("Snacktorio", "Snacktorio", "Application Support")],
        );
        assert!(report.apps.is_empty());
    }

    #[test]
    fn installed_bundle_is_not_orphan() {
        let installed = vec![InstalledApp {
            id: "com.example.gone".into(),
            name: "Gone".into(),
            aliases: vec!["gone".into()],
            source: "t".into(),
            path: None,
        }];
        let report = match_orphans(
            &installed,
            vec![cand("com.example.gone", "com.example.gone", "Containers")],
        );
        assert!(report.apps.is_empty());
    }

    #[test]
    fn apple_ids_skipped() {
        let report = match_orphans(
            &[],
            vec![cand("com.apple.Safari", "com.apple.Safari", "Containers")],
        );
        assert!(report.apps.is_empty());
    }

    #[test]
    fn rm_without_target_errors() {
        let report = OrphanReport {
            installed_apps: 0,
            apps: vec![],
        };
        let err = resolve_delete_targets(&report, "", false).unwrap_err();
        assert!(err.to_string().contains("requires"));
    }

    #[test]
    fn app_id_requires_all() {
        let report = match_orphans(
            &[],
            vec![cand("com.example.gone", "com.example.gone", "Containers")],
        );
        let err = resolve_delete_targets(&report, "com.example.gone", false).unwrap_err();
        assert!(err.to_string().contains("--all"));
        let paths = resolve_delete_targets(&report, "com.example.gone", true).unwrap();
        assert_eq!(paths.len(), 1);
    }

    #[test]
    fn delete_missing_is_success() {
        let outcome = delete_targets(&[PathBuf::from("/tmp/ku-orphan-does-not-exist-xyz")]);
        assert!(outcome.failed.is_empty());
    }

    #[test]
    fn delete_protected_is_failure() {
        let outcome = delete_targets(&[PathBuf::from("/")]);
        assert!(outcome.incomplete());
        assert!(outcome.permission_denied());
    }

    fn row(id: &str, conf: Confidence, size: u64, mtime: Option<i64>) -> OrphanRow {
        OrphanRow {
            app_id: id.into(),
            app_name: id.into(),
            path: PathBuf::from(format!("/tmp/{id}")),
            size,
            confidence: conf,
            reason: String::new(),
            related: 1,
            deleted: false,
            mtime,
        }
    }

    #[test]
    fn sort_level_then_oldest() {
        let mut rows = vec![
            row("low-new", Confidence::Low, 9, Some(1_700_000_000)),
            row("high-old", Confidence::High, 1, Some(1_000_000_000)),
            row("high-new", Confidence::High, 2, Some(1_800_000_000)),
        ];
        sort_rows(&mut rows, OrphanSort::Level, false);
        assert_eq!(rows[0].app_id, "high-new");
        assert_eq!(rows[1].app_id, "high-old");
        sort_rows(&mut rows, OrphanSort::Age, false);
        assert_eq!(rows[0].app_id, "high-old");
    }

    #[test]
    fn google_url_encodes_spaces() {
        let url = google_search_url("Dionic Software");
        assert!(url.contains("q=Dionic+Software"));
        assert!(url.starts_with("https://www.google.com/search?"));
    }
}
