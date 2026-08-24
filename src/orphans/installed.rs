use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use super::identity::{last_component, slug, tokens};

#[derive(Debug, Clone)]
pub struct InstalledApp {
    pub id: String,
    pub name: String,
    pub aliases: Vec<String>,
    #[allow(dead_code)]
    pub source: String,
    #[allow(dead_code)]
    pub path: Option<PathBuf>,
}

impl InstalledApp {
    fn with_aliases(mut self) -> Self {
        let mut extra = vec![
            slug(&self.id),
            slug(&self.name),
            slug(last_component(&self.id)),
        ];
        extra.extend(tokens(&self.name));
        extra.extend(tokens(&self.id.replace('.', " ")));
        extra.retain(|s| s.len() >= 3);
        self.aliases.extend(extra);
        self.aliases.sort();
        self.aliases.dedup();
        self
    }

    pub fn matches_key(&self, key: &str) -> bool {
        let key_l = key.to_ascii_lowercase();
        if self.id.eq_ignore_ascii_case(&key_l) {
            return true;
        }
        let key_slug = slug(key);
        if key_slug.len() < 3 {
            return false;
        }
        self.aliases.iter().any(|a| a == &key_slug)
            || (looks_strict_id(&key_l) && self.id.eq_ignore_ascii_case(&key_l))
    }
}

fn looks_strict_id(s: &str) -> bool {
    s.contains('.')
}

#[allow(dead_code)]
pub fn inventory() -> Vec<InstalledApp> {
    inventory_cancellable(&AtomicBool::new(false))
}

pub fn inventory_cancellable(cancel: &AtomicBool) -> Vec<InstalledApp> {
    let mut apps = Vec::new();
    if cancel.load(Ordering::Relaxed) {
        return apps;
    }
    #[cfg(target_os = "macos")]
    collect_macos(&mut apps, cancel);
    #[cfg(not(target_os = "macos"))]
    collect_linux(&mut apps, cancel);
    collect_steam(&mut apps, cancel);
    apps
}

#[cfg(target_os = "macos")]
fn collect_macos(apps: &mut Vec<InstalledApp>, cancel: &AtomicBool) {
    let mut roots = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/System/Applications"),
        PathBuf::from("/System/Applications/Utilities"),
        PathBuf::from("/Applications/Utilities"),
    ];
    if let Some(home) = crate::paths::default_home() {
        roots.push(home.join("Applications"));
    }
    for root in roots {
        walk_apps(&root, apps, 0, cancel);
    }
}

#[cfg(target_os = "macos")]
fn walk_apps(dir: &Path, apps: &mut Vec<InstalledApp>, depth: u32, cancel: &AtomicBool) {
    if depth > 3 || cancel.load(Ordering::Relaxed) {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.ends_with(".app") && path.is_dir() {
            if let Some(app) = read_macos_app(&path) {
                apps.push(app);
            }
            continue;
        }
        if path.is_dir() && !name.starts_with('.') {
            walk_apps(&path, apps, depth + 1, cancel);
        }
    }
}

#[cfg(target_os = "macos")]
fn read_macos_app(bundle: &Path) -> Option<InstalledApp> {
    let plist = bundle.join("Contents").join("Info.plist");
    let (id, name, copyright) = read_info_plist(&plist)?;
    let display = bundle
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.clone());
    let mut aliases = vec![slug(&display)];
    aliases.extend(tokens(&copyright));
    Some(
        InstalledApp {
            id,
            name: if name.is_empty() {
                display.clone()
            } else {
                name
            },
            aliases,
            source: "applications".into(),
            path: Some(bundle.to_path_buf()),
        }
        .with_aliases(),
    )
}

#[cfg(target_os = "macos")]
fn read_info_plist(path: &Path) -> Option<(String, String, String)> {
    let bytes = fs::read(path).ok()?;
    if bytes.starts_with(b"bplist") {
        return plutil_strings(path);
    }
    let xml = String::from_utf8_lossy(&bytes);
    let id = plist_xml_string(&xml, "CFBundleIdentifier")?;
    let name = plist_xml_string(&xml, "CFBundleDisplayName")
        .or_else(|| plist_xml_string(&xml, "CFBundleName"))
        .unwrap_or_default();
    let copyright = plist_xml_string(&xml, "NSHumanReadableCopyright").unwrap_or_default();
    Some((id, name, copyright))
}

#[cfg(target_os = "macos")]
fn plist_xml_string(xml: &str, key: &str) -> Option<String> {
    let needle = format!("<key>{key}</key>");
    let i = xml.find(&needle)?;
    let rest = xml[i + needle.len()..].trim_start();
    let start = rest.find("<string>")? + 8;
    let end = rest[start..].find("</string>")?;
    let value = rest[start..start + end].trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

#[cfg(target_os = "macos")]
fn plutil_strings(path: &Path) -> Option<(String, String, String)> {
    let out = std::process::Command::new("plutil")
        .args(["-convert", "json", "-o", "-", "--"])
        .arg(path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let id = v.get("CFBundleIdentifier")?.as_str()?.to_string();
    let name = v
        .get("CFBundleDisplayName")
        .or_else(|| v.get("CFBundleName"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let copyright = v
        .get("NSHumanReadableCopyright")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    Some((id, name, copyright))
}

fn collect_steam(apps: &mut Vec<InstalledApp>, cancel: &AtomicBool) {
    let mut roots = Vec::new();
    if let Some(home) = crate::paths::default_home() {
        roots.push(home.join("Library/Application Support/Steam"));
        roots.push(home.join(".local/share/Steam"));
        roots.push(home.join(".steam/steam"));
        roots.push(home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"));
    }
    for root in roots {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let common = root.join("steamapps").join("common");
        let Ok(entries) = fs::read_dir(&common) else {
            continue;
        };
        for entry in entries.flatten() {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.eq_ignore_ascii_case("screenshots") || name.starts_with('.') {
                continue;
            }
            apps.push(
                InstalledApp {
                    id: format!("steam:{name}"),
                    name: name.clone(),
                    aliases: vec![slug(&name)],
                    source: "steam".into(),
                    path: Some(path),
                }
                .with_aliases(),
            );
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn collect_linux(apps: &mut Vec<InstalledApp>, cancel: &AtomicBool) {
    let mut roots = vec![
        PathBuf::from("/usr/share/applications"),
        PathBuf::from("/usr/local/share/applications"),
        PathBuf::from("/var/lib/snapd/desktop/applications"),
        PathBuf::from("/var/lib/flatpak/exports/share/applications"),
    ];
    if let Some(home) = crate::paths::default_home() {
        roots.push(home.join(".local/share/applications"));
        roots.push(home.join(".local/share/flatpak/exports/share/applications"));
    }
    for root in roots {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            if let Some(app) = read_desktop(&path) {
                apps.push(app);
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn read_desktop(path: &Path) -> Option<InstalledApp> {
    let text = fs::read_to_string(path).ok()?;
    let mut in_entry = false;
    let mut name = None;
    let mut exec = None;
    let mut wmclass = None;
    let mut no_display = false;
    let mut hidden = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line.eq_ignore_ascii_case("[Desktop Entry]");
            continue;
        }
        if !in_entry {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            match k {
                "Name" => name = Some(v.to_string()),
                "Exec" => exec = Some(v.to_string()),
                "StartupWMClass" => wmclass = Some(v.to_string()),
                "NoDisplay" if v.eq_ignore_ascii_case("true") => no_display = true,
                "Hidden" if v.eq_ignore_ascii_case("true") => hidden = true,
                _ => {}
            }
        }
    }
    if hidden || no_display {
        return None;
    }
    let name = name?;
    let file_id = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.clone());
    let bin = exec
        .as_deref()
        .and_then(|e| e.split_whitespace().next())
        .and_then(|p| Path::new(p).file_name())
        .map(|s| s.to_string_lossy().into_owned());
    let mut aliases = vec![slug(&file_id)];
    if let Some(b) = &bin {
        aliases.push(slug(b));
    }
    if let Some(w) = &wmclass {
        aliases.push(slug(w));
    }
    Some(
        InstalledApp {
            id: file_id,
            name,
            aliases,
            source: "desktop".into(),
            path: Some(path.to_path_buf()),
        }
        .with_aliases(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_exact_id() {
        let app = InstalledApp {
            id: "com.example.foo".into(),
            name: "Foo".into(),
            aliases: vec![],
            source: "t".into(),
            path: None,
        }
        .with_aliases();
        assert!(app.matches_key("com.example.foo"));
        assert!(app.matches_key("Foo"));
        assert!(!app.matches_key("bar"));
    }
}
