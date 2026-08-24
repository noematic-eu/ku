use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use super::identity::{
    extract_bundle_id, is_apple_id, is_protected_name, is_system_library_support,
    is_system_var_lib, slug,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PathKind {
    File,
    Dir,
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub path: PathBuf,
    pub name: String,
    pub kind: PathKind,
    pub size: u64,
    pub mtime: Option<i64>,
    pub location: String,
    pub app_key: String,
}

#[allow(dead_code)]
pub fn collect() -> Vec<Candidate> {
    collect_cancellable(&AtomicBool::new(false))
}

pub fn collect_cancellable(cancel: &AtomicBool) -> Vec<Candidate> {
    let mut out = Vec::new();
    for (root, location, as_files) in roots() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if !root.exists() {
            continue;
        }
        scan_root(&root, &location, as_files, &mut out, cancel);
    }
    out
}

fn roots() -> Vec<(PathBuf, String, bool)> {
    let mut roots = Vec::new();
    if let Some(home) = crate::paths::default_home() {
        #[cfg(target_os = "macos")]
        {
            let lib = home.join("Library");
            roots.push((
                lib.join("Application Support"),
                "Application Support".into(),
                false,
            ));
            roots.push((lib.join("Caches"), "Caches".into(), false));
            roots.push((lib.join("Containers"), "Containers".into(), false));
            roots.push((
                lib.join("Group Containers"),
                "Group Containers".into(),
                false,
            ));
            roots.push((lib.join("Preferences"), "Preferences".into(), true));
            roots.push((lib.join("Logs"), "Logs".into(), false));
            roots.push((
                lib.join("Saved Application State"),
                "Saved Application State".into(),
                false,
            ));
            roots.push((lib.join("HTTPStorages"), "HTTPStorages".into(), false));
            roots.push((lib.join("WebKit"), "WebKit".into(), false));
        }
        #[cfg(not(target_os = "macos"))]
        {
            roots.push((home.join(".config"), ".config".into(), false));
            roots.push((home.join(".local/share"), ".local/share".into(), false));
            roots.push((home.join(".cache"), ".cache".into(), false));
            roots.push((home.join(".local/state"), ".local/state".into(), false));
            roots.push((home.join(".var/app"), "flatpak".into(), false));
            roots.push((home.join("snap"), "snap".into(), false));
        }
    }
    #[cfg(target_os = "macos")]
    {
        roots.push((
            PathBuf::from("/Library/Application Support"),
            "/Library/Application Support".into(),
            false,
        ));
        roots.push((
            PathBuf::from("/Library/Caches"),
            "/Library/Caches".into(),
            false,
        ));
        roots.push((
            PathBuf::from("/Library/Logs"),
            "/Library/Logs".into(),
            false,
        ));
    }
    #[cfg(not(target_os = "macos"))]
    {
        roots.push((PathBuf::from("/var/lib"), "/var/lib".into(), false));
    }
    roots
}

fn scan_root(
    root: &Path,
    location: &str,
    as_files: bool,
    out: &mut Vec<Candidate>,
    cancel: &AtomicBool,
) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        if skip_entry(location, &name) {
            continue;
        }
        let is_dir = path.is_dir();
        if as_files && is_dir {
            continue;
        }
        if !as_files && !is_dir {
            continue;
        }
        let kind = if is_dir {
            PathKind::Dir
        } else {
            PathKind::File
        };
        let size = if is_dir {
            dir_size(&path, cancel)
        } else {
            entry.metadata().map(|m| m.len()).unwrap_or(0)
        };
        let mtime = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64);
        let app_key = app_key_for(&name);
        out.push(Candidate {
            path,
            name,
            kind,
            size,
            mtime,
            location: location.to_string(),
            app_key,
        });
    }
}

fn skip_entry(location: &str, name: &str) -> bool {
    if is_protected_name(name) {
        return true;
    }
    if let Some(id) = extract_bundle_id(name)
        && is_apple_id(&id)
    {
        return true;
    }
    if (location.contains("/Library") || location == "Application Support")
        && is_system_library_support(name)
    {
        return true;
    }
    if location == "/var/lib" && is_system_var_lib(name) {
        return true;
    }
    if location == ".local/share"
        && matches!(
            slug(name).as_str(),
            "applications" | "fonts" | "flatpak" | "Trash" | "trash" | "ku" | "recently-used.xbel"
        )
    {
        return true;
    }
    if location == ".config" && matches!(slug(name).as_str(), "ku" | "systemd" | "userdirs") {
        return true;
    }
    if location.contains("Caches") && is_toolchain_cache(name) {
        return true;
    }
    false
}

fn is_toolchain_cache(name: &str) -> bool {
    matches!(
        slug(name).as_str(),
        "gobuild"
            | "pip"
            | "nodegyp"
            | "npm"
            | "yarn"
            | "pnpm"
            | "cargo"
            | "gradle"
            | "maven"
            | "cocoapods"
            | "composer"
            | "typescript"
            | "fanal"
            | "ccache"
            | "cython"
            | "homebrew"
    )
}

fn app_key_for(name: &str) -> String {
    if let Some(id) = extract_bundle_id(name) {
        return id;
    }
    let stem = name
        .trim_end_matches(".plist")
        .trim_end_matches(".savedState");
    if let Some(id) = extract_bundle_id(stem) {
        return id;
    }
    stem.to_string()
}

fn dir_size(path: &Path, cancel: &AtomicBool) -> u64 {
    let mut total = 0u64;
    let mut n = 0usize;
    for entry in walkdir::WalkDir::new(path)
        .follow_links(false)
        .max_depth(10)
        .into_iter()
        .flatten()
    {
        n += 1;
        if n > 80_000 || (n % 256 == 0 && cancel.load(Ordering::Relaxed)) {
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

pub fn is_safe_to_delete(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let components = canon.components().count();
    if components < 4 {
        return false;
    }
    let s = canon.to_string_lossy();
    let s = s.trim_end_matches('/');
    let forbidden = [
        "",
        "/",
        "/Library",
        "/private/Library",
        "/var",
        "/private/var",
        "/var/lib",
        "/private/var/lib",
        "/usr",
        "/System",
        "/Applications",
        "/home",
        "/Users",
        "/opt",
        "/tmp",
        "/private/tmp",
    ];
    if forbidden.iter().any(|f| s == *f) {
        return false;
    }
    if let Some(home) = crate::paths::default_home() {
        if canon == home {
            return false;
        }
        for tail in [
            "Library",
            "Library/Application Support",
            "Library/Caches",
            "Library/Preferences",
            "Library/Containers",
            ".config",
            ".local",
            ".local/share",
            ".cache",
        ] {
            if canon == home.join(tail) {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_from_names() {
        assert_eq!(app_key_for("com.foo.bar.plist"), "com.foo.bar");
        assert_eq!(app_key_for("Slack"), "Slack");
        assert_eq!(app_key_for("ABCDEFGHIJ.com.foo.bar"), "com.foo.bar");
    }

    #[test]
    fn refuses_shallow_paths() {
        assert!(!is_safe_to_delete(Path::new("/")));
        assert!(!is_safe_to_delete(Path::new("/Library")));
        assert!(!is_safe_to_delete(Path::new("/var/lib")));
    }
}
