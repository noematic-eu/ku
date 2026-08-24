use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
use unicode_width::UnicodeWidthStr;

const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];

pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else if value >= 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

pub fn format_bytes_signed(delta: i64) -> String {
    if delta == 0 {
        return "0 B".to_string();
    }
    let sign = if delta > 0 { "+" } else { "-" };
    format!("{sign}{}", format_bytes(delta.unsigned_abs()))
}

pub fn format_percent(pct: f64) -> String {
    if pct.is_nan() {
        return "—".to_string();
    }
    format!("{pct:5.1}%")
}

pub fn format_mtime_ago(mtime: Option<i64>) -> String {
    let Some(ts) = mtime else {
        return "—".into();
    };
    let now = chrono::Local::now().timestamp();
    let days = (now - ts).max(0) / 86_400;
    if days == 0 {
        "today".into()
    } else if days == 1 {
        "1d".into()
    } else if days < 45 {
        format!("{days}d")
    } else if days < 548 {
        format!("{}mo", days / 30)
    } else {
        format!("{}y", days / 365)
    }
}

pub fn format_uptime(secs: u64) -> String {
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let mins = (secs % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h {mins}m")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else {
        format!("{mins}m")
    }
}

pub fn percent(used: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        (used as f64 / total as f64) * 100.0
    }
}

pub fn parse_size(input: &str) -> Option<u64> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let split = trimmed
        .find(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(trimmed.len());
    let (num, unit) = trimmed.split_at(split);
    let value: f64 = num.trim().parse().ok()?;
    if value < 0.0 {
        return None;
    }
    let mul = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "k" | "kb" | "kib" => 1024.0,
        "m" | "mb" | "mib" => 1024.0 * 1024.0,
        "g" | "gb" | "gib" => 1024.0 * 1024.0 * 1024.0,
        "t" | "tb" | "tib" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        "%" => return None,
        _ => return None,
    };
    Some((value * mul) as u64)
}

pub fn parse_duration_window(input: &str) -> Option<i64> {
    let trimmed = input.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        return None;
    }
    let split = trimmed
        .find(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(trimmed.len());
    let (num, unit) = trimmed.split_at(split);
    let value: i64 = num.trim().parse().ok()?;
    let secs = match unit {
        "s" => value,
        "m" => value * 60,
        "h" => value * 3_600,
        "d" => value * 86_400,
        _ => return None,
    };
    Some(secs)
}

pub fn truncate_ellipsis(s: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if s.width() <= max_width {
        return s.to_string();
    }
    if max_width <= 1 {
        return "…".to_string();
    }
    let mut out = String::new();
    let mut width = 0usize;
    for ch in s.chars() {
        let ch_w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_w + 1 > max_width {
            break;
        }
        out.push(ch);
        width += ch_w;
    }
    out.push('…');
    out
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProcessFilter {
    pub text: String,
    pub cpu_min: Option<f32>,
    pub mem_min: Option<u64>,
    pub user: Option<String>,
}

impl ProcessFilter {
    pub fn parse(input: &str) -> Self {
        let mut filter = ProcessFilter::default();
        let mut text_parts = Vec::new();
        for token in input.split_whitespace() {
            let lower = token.to_ascii_lowercase();
            if let Some(rest) = lower.strip_prefix("cpu>")
                && let Ok(v) = rest.parse::<f32>()
            {
                filter.cpu_min = Some(v);
                continue;
            }
            if let Some(rest) = lower.strip_prefix("cpu>=")
                && let Ok(v) = rest.parse::<f32>()
            {
                filter.cpu_min = Some(v);
                continue;
            }
            if let Some(rest) = lower.strip_prefix("mem>")
                && let Some(v) = parse_mem_token(rest)
            {
                filter.mem_min = Some(v);
                continue;
            }
            if let Some(rest) = lower.strip_prefix("mem>=")
                && let Some(v) = parse_mem_token(rest)
            {
                filter.mem_min = Some(v);
                continue;
            }
            if let Some(rest) = token.strip_prefix("user:") {
                filter.user = Some(rest.to_string());
                continue;
            }
            if let Some(rest) = token.strip_prefix("user=") {
                filter.user = Some(rest.to_string());
                continue;
            }
            text_parts.push(token);
        }
        filter.text = text_parts.join(" ").to_ascii_lowercase();
        filter
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
            && self.cpu_min.is_none()
            && self.mem_min.is_none()
            && self.user.is_none()
    }

    pub fn matches(&self, pid: u32, name: &str, user: &str, cmd: &str, cpu: f32, mem: u64) -> bool {
        if let Some(min) = self.cpu_min
            && cpu < min
        {
            return false;
        }
        if let Some(min) = self.mem_min
            && mem < min
        {
            return false;
        }
        if let Some(ref want) = self.user
            && !user.eq_ignore_ascii_case(want)
        {
            return false;
        }
        if self.text.is_empty() {
            return true;
        }
        let hay = format!("{pid} {name} {user} {cmd}").to_ascii_lowercase();
        hay.contains(&self.text)
    }
}

fn parse_mem_token(token: &str) -> Option<u64> {
    if let Some(stripped) = token.strip_suffix('%') {
        let _ = stripped;
        return parse_size(token.trim_end_matches('%')).or_else(|| {
            token
                .trim_end_matches('%')
                .parse::<f64>()
                .ok()
                .map(|p| (p / 100.0 * 16.0 * 1024.0 * 1024.0 * 1024.0) as u64)
        });
    }
    parse_size(token)
}

#[cfg(unix)]
pub fn inode_usage(path: &std::path::Path) -> Option<(u64, u64)> {
    let cstr = std::ffi::CString::new(path.to_string_lossy().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(cstr.as_ptr(), &mut stat) };
    if rc != 0 {
        return None;
    }
    let total = stat.f_files as u64;
    if total == 0 {
        return None;
    }
    let free = stat.f_ffree as u64;
    Some((total.saturating_sub(free), total))
}

#[cfg(not(unix))]
pub fn inode_usage(_path: &std::path::Path) -> Option<(u64, u64)> {
    None
}

pub fn file_manager_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Finder"
    } else if cfg!(target_os = "windows") {
        "Explorer"
    } else {
        "file manager"
    }
}

pub fn ncdu_available() -> bool {
    static AVAIL: OnceLock<bool> = OnceLock::new();
    *AVAIL.get_or_init(|| find_in_path("ncdu").is_some())
}

/// Short label for the growth inspect shortcut (`ncdu` or `reveal`).
pub fn reveal_shortcut_label() -> &'static str {
    if ncdu_available() { "ncdu" } else { "reveal" }
}

pub fn find_in_path(bin: &str) -> Option<PathBuf> {
    find_in_path_from(bin, std::env::var_os("PATH")?.as_os_str())
}

fn find_in_path_from(bin: &str, path_var: &OsStr) -> Option<PathBuf> {
    for dir in std::env::split_paths(path_var) {
        let candidate = dir.join(bin);
        if is_executable(&candidate) {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let exe = dir.join(format!("{bin}.exe"));
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = path.metadata() else {
        return false;
    };
    meta.is_file() && meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Directory ncdu should scan: `path` if it is a directory, otherwise its parent.
///
/// Only walks up when metadata confirms a non-directory. A failed `is_dir()`
/// check must not promote `/Users/me/work` to `/Users/me`.
pub fn inspect_dir(path: &Path) -> PathBuf {
    ncdu_scan_dir(path).unwrap_or_else(|_| path.to_path_buf())
}

pub fn ncdu_scan_dir(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        bail!("empty path");
    }
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    let meta =
        std::fs::metadata(&abs).with_context(|| format!("{} no longer exists", abs.display()))?;
    let dir = if meta.is_dir() {
        abs
    } else {
        abs.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .ok_or_else(|| anyhow::anyhow!("no parent directory for {}", abs.display()))?
    };
    Ok(dir.canonicalize().unwrap_or(dir))
}

/// Run `ncdu` on `path` (blocking). Caller must have left the TUI first.
///
/// `current_dir` + `.` so ncdu 2 cannot fall back to $HOME / cwd if the
/// positional path is ignored.
pub fn run_ncdu(path: &Path) -> Result<()> {
    let dir = ncdu_scan_dir(path)?;
    let bin = find_in_path("ncdu").unwrap_or_else(|| PathBuf::from("ncdu"));
    Command::new(&bin)
        .current_dir(&dir)
        .arg("--ignore-config")
        .arg("--")
        .arg(".")
        .status()
        .with_context(|| format!("running ncdu in {}", dir.display()))?;
    Ok(())
}

/// Open `path` in the desktop file manager (Finder, Explorer, Nautilus…).
///
/// Directories are opened so their contents are visible. Files are revealed
/// (selected in the parent folder) when the OS supports it.
pub fn reveal_in_file_manager(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() {
        bail!("empty path");
    }
    if !path.exists() {
        bail!("{} no longer exists", path.display());
    }

    let label = file_manager_label();
    if cfg!(target_os = "macos") {
        if path.is_dir() {
            spawn_gui("open", &[path.as_os_str()], label)
        } else {
            spawn_gui(
                "open",
                &[std::ffi::OsStr::new("-R"), path.as_os_str()],
                label,
            )
        }
    } else if cfg!(target_os = "windows") {
        let select = format!("/select,{}", path.display());
        spawn_gui("explorer", &[std::ffi::OsStr::new(&select)], label)
    } else {
        let target = if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(path)
                .to_path_buf()
        };
        spawn_gui("xdg-open", &[target.as_os_str()], label)
    }
}

fn spawn_gui(program: &str, args: &[&std::ffi::OsStr], label: &str) -> Result<()> {
    let status = match sudo_user() {
        Some(user) => {
            let mut cmd = gui_command("sudo");
            cmd.arg("-n").arg("-u").arg(&user).arg(program).args(args);
            match cmd.status() {
                Ok(st) if st.success() => return Ok(()),
                Ok(_) | Err(_) => gui_command(program).args(args).status(),
            }
        }
        None => gui_command(program).args(args).status(),
    }
    .with_context(|| format!("opening {label} ({program})"))?;
    if status.success() {
        Ok(())
    } else {
        bail!("could not open in {label} ({status})")
    }
}

fn gui_command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd
}

fn sudo_user() -> Option<String> {
    let user = std::env::var("SUDO_USER").ok()?;
    if user.is_empty() || user == "root" {
        None
    } else {
        Some(user)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_format_scales() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.00 KiB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.00 MiB");
        assert_eq!(format_bytes_signed(-2048), "-2.00 KiB");
        assert_eq!(format_bytes_signed(0), "0 B");
    }

    #[test]
    fn mtime_ago_unknown() {
        assert_eq!(format_mtime_ago(None), "—");
    }

    #[test]
    fn uptime_format() {
        assert_eq!(format_uptime(65), "1m");
        assert_eq!(format_uptime(3700), "1h 1m");
        assert_eq!(format_uptime(90_000), "1d 1h 0m");
    }

    #[test]
    fn parse_sizes() {
        assert_eq!(parse_size("10M"), Some(10 * 1024 * 1024));
        assert_eq!(
            parse_size("1.5G"),
            Some((1.5 * 1024.0 * 1024.0 * 1024.0) as u64)
        );
        assert_eq!(parse_size("512"), Some(512));
        assert!(parse_size("nope").is_none());
    }

    #[test]
    fn parse_windows() {
        assert_eq!(parse_duration_window("1m"), Some(60));
        assert_eq!(parse_duration_window("5m"), Some(300));
        assert_eq!(parse_duration_window("1h"), Some(3600));
        assert_eq!(parse_duration_window("24h"), Some(86400));
        assert_eq!(parse_duration_window("7d"), Some(7 * 86400));
    }

    #[test]
    fn process_filter_tokens() {
        let f = ProcessFilter::parse("nginx cpu>2.5 mem>100M user:root");
        assert_eq!(f.text, "nginx");
        assert_eq!(f.cpu_min, Some(2.5));
        assert_eq!(f.mem_min, Some(100 * 1024 * 1024));
        assert_eq!(f.user.as_deref(), Some("root"));
        assert!(f.matches(
            42,
            "nginx",
            "root",
            "/usr/sbin/nginx",
            10.0,
            200 * 1024 * 1024
        ));
        assert!(!f.matches(
            42,
            "nginx",
            "www",
            "/usr/sbin/nginx",
            10.0,
            200 * 1024 * 1024
        ));
        assert!(!f.matches(42, "sshd", "root", "sshd", 10.0, 200 * 1024 * 1024));
    }

    #[test]
    fn truncate_keeps_width() {
        assert_eq!(truncate_ellipsis("hello", 10), "hello");
        let t = truncate_ellipsis("abcdefghij", 5);
        assert!(t.ends_with('…'));
        assert!(t.width() <= 5);
    }

    #[test]
    fn reveal_rejects_missing_path() {
        let err = reveal_in_file_manager(Path::new("/no/such/ku-reveal-test-path")).unwrap_err();
        assert!(err.to_string().contains("no longer exists"));
        let err = reveal_in_file_manager(Path::new("")).unwrap_err();
        assert!(err.to_string().contains("empty path"));
    }

    #[test]
    fn inspect_dir_uses_parent_for_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        let want = dir.path().canonicalize().unwrap();
        assert_eq!(ncdu_scan_dir(dir.path()).unwrap(), want);
        assert_eq!(ncdu_scan_dir(&file).unwrap(), want);
        let nested = dir.path().join("sub");
        std::fs::create_dir(&nested).unwrap();
        assert_eq!(
            ncdu_scan_dir(&nested).unwrap(),
            nested.canonicalize().unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn find_in_path_sees_executable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("fakebin");
        std::fs::write(&bin, b"#!/bin/sh\n").unwrap();
        let mut perm = std::fs::metadata(&bin).unwrap().permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&bin, perm).unwrap();
        assert_eq!(
            find_in_path_from("fakebin", dir.path().as_os_str()).as_deref(),
            Some(bin.as_path())
        );
        assert!(find_in_path_from("missing", dir.path().as_os_str()).is_none());
    }

    #[test]
    fn file_manager_label_is_os_specific() {
        let label = file_manager_label();
        assert!(!label.is_empty());
        if cfg!(target_os = "macos") {
            assert_eq!(label, "Finder");
        } else if cfg!(target_os = "windows") {
            assert_eq!(label, "Explorer");
        } else {
            assert_eq!(label, "file manager");
        }
    }
}
