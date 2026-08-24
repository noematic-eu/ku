use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use super::leftovers::is_safe_to_delete;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElevateMethod {
    MacOsascript,
    Pkexec,
    SudoCached,
    Sudo,
}

impl ElevateMethod {
    pub fn label(self) -> &'static str {
        match self {
            Self::MacOsascript => "macOS administrator dialog",
            Self::Pkexec => "polkit (pkexec)",
            Self::SudoCached => "sudo (cached)",
            Self::Sudo => "sudo",
        }
    }
}

pub fn detect() -> Option<ElevateMethod> {
    if super::running_as_root() {
        return None;
    }
    #[cfg(target_os = "macos")]
    {
        if path_exists("/usr/bin/osascript") {
            return Some(ElevateMethod::MacOsascript);
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        if which("pkexec") {
            return Some(ElevateMethod::Pkexec);
        }
        if sudo_noninteractive() {
            return Some(ElevateMethod::SudoCached);
        }
        if which("sudo") {
            return Some(ElevateMethod::Sudo);
        }
    }
    #[cfg(target_os = "macos")]
    {
        if which("sudo") {
            return Some(ElevateMethod::Sudo);
        }
    }
    None
}

fn path_exists(p: &str) -> bool {
    Path::new(p).exists()
}

fn which(bin: &str) -> bool {
    Command::new("which")
        .arg(bin)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(not(target_os = "macos"))]
fn sudo_noninteractive() -> bool {
    Command::new("sudo")
        .args(["-n", "true"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryKind {
    Elevated(ElevateMethod),
    /// Already root: chflags/chmod then `rm -rf` (no extra prompt).
    Forced,
}

impl RetryKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Elevated(m) => m.label(),
            Self::Forced => "root retry (cleared flags)",
        }
    }
}

/// Elevate if needed, otherwise retry as root with flags cleared.
pub fn retry(paths: &[PathBuf]) -> Result<RetryKind> {
    if super::running_as_root() {
        force_remove(paths)?;
        Ok(RetryKind::Forced)
    } else {
        Ok(RetryKind::Elevated(remove(paths)?))
    }
}

/// Delete `paths` after an OS privilege prompt. Only leftover paths already
/// accepted by `is_safe_to_delete` are sent to the helper.
pub fn remove(paths: &[PathBuf]) -> Result<ElevateMethod> {
    let method = detect().context("no privilege helper (pkexec/sudo/osascript)")?;
    let paths = safe_existing(paths)?;
    match method {
        ElevateMethod::MacOsascript => osascript_rm(&paths)?,
        ElevateMethod::Pkexec => run_shell(Some("pkexec"), &delete_shell(&paths))?,
        ElevateMethod::SudoCached | ElevateMethod::Sudo => {
            run_shell(Some("sudo"), &delete_shell(&paths))?;
        }
    }
    Ok(method)
}

/// Already privileged: drop uchg/schg, add write, then `rm -rf`.
pub fn force_remove(paths: &[PathBuf]) -> Result<()> {
    let paths = safe_existing(paths)?;
    run_shell(None, &delete_shell(&paths))
}

fn safe_existing(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let paths: Vec<PathBuf> = paths
        .iter()
        .filter(|p| p.exists() && is_safe_to_delete(p))
        .cloned()
        .collect();
    if paths.is_empty() {
        bail!("no remaining safe paths to delete");
    }
    Ok(paths)
}

fn sh_single_quote(s: &str) -> String {
    let mut out = String::from("'");
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

fn delete_shell(paths: &[PathBuf]) -> String {
    let list = paths
        .iter()
        .map(|p| sh_single_quote(&p.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ");
    #[cfg(target_os = "macos")]
    {
        format!(
            "/usr/bin/chflags -R nouchg,noschg,nouappnd,nosappnd -- {list} >/dev/null 2>&1; \
             /bin/chmod -R u+w -- {list} >/dev/null 2>&1; \
             /bin/rm -rf -- {list}"
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        format!("/bin/chmod -R u+w -- {list} >/dev/null 2>&1; /bin/rm -rf -- {list}")
    }
}

fn run_shell(helper: Option<&str>, script: &str) -> Result<()> {
    let mut cmd = match helper {
        None => {
            let mut c = Command::new("/bin/sh");
            c.arg("-c").arg(script);
            c
        }
        Some("sudo") => {
            let mut c = Command::new("sudo");
            c.arg("--").arg("/bin/sh").arg("-c").arg(script);
            c
        }
        Some("pkexec") => {
            let mut c = Command::new("pkexec");
            c.arg("/bin/sh").arg("-c").arg(script);
            c
        }
        Some(other) => bail!("unknown helper {other}"),
    };
    cmd.stdin(Stdio::null());
    let label = helper.unwrap_or("sh");
    let out = cmd.output().with_context(|| format!("running {label}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim();
        if err.is_empty() {
            bail!("{label} failed with status {}", out.status);
        }
        bail!("{label}: {err}");
    }
    Ok(())
}

fn osascript_rm(paths: &[PathBuf]) -> Result<()> {
    let script = applescript_rm(paths);
    let out = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .context("running osascript")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim();
        if err.contains("User canceled") || err.contains("(-128)") {
            bail!("authentication canceled");
        }
        bail!("osascript: {err}");
    }
    Ok(())
}

pub fn applescript_rm(paths: &[PathBuf]) -> String {
    let mut p = String::new();
    for (i, path) in paths.iter().enumerate() {
        if i > 0 {
            p.push_str(" & space & ");
        }
        let posix = path
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        p.push_str("quoted form of \"");
        p.push_str(&posix);
        p.push('"');
    }
    format!(
        "set p to {p}\n\
         do shell script \"/usr/bin/chflags -R nouchg,noschg,nouappnd,nosappnd -- \" & p \
         & \"; /bin/chmod -R u+w -- \" & p \
         & \"; /bin/rm -rf -- \" & p with administrator privileges"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applescript_quotes_paths() {
        let s = applescript_rm(&[PathBuf::from("/Library/Application Support/Foo")]);
        assert!(s.contains("quoted form of \"/Library/Application Support/Foo\""));
        assert!(s.contains("with administrator privileges"));
        assert!(s.contains("/bin/rm -rf --"));
        assert!(s.contains("chflags -R nouchg"));
    }

    #[test]
    fn shell_quotes_spaces_and_quotes() {
        let s = delete_shell(&[PathBuf::from("/tmp/a b"), PathBuf::from("/tmp/it's")]);
        assert!(s.contains("'/tmp/a b'"));
        assert!(s.contains("'/tmp/it'\\''s'"));
        assert!(s.contains("/bin/rm -rf --"));
    }

    #[test]
    fn force_remove_deletes_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("gone");
        std::fs::write(&p, b"x").unwrap();
        #[cfg(target_os = "macos")]
        {
            let _ = Command::new("/usr/bin/chflags")
                .args(["uchg", p.to_str().unwrap()])
                .status();
        }
        force_remove(&[p.clone()]).unwrap();
        assert!(!p.exists());
    }
}
