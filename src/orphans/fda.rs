use anyhow::{Result, bail};

/// Whether this process can read TCC-protected user data.
///
/// Full Disk Access is a Privacy setting on the *host app* (Terminal, iTerm,
/// Ghostty…), not on the `ku` binary. `sudo` does not grant it.
pub fn has_full_disk_access() -> bool {
    #[cfg(target_os = "macos")]
    {
        probe_macos()
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

#[cfg(target_os = "macos")]
fn probe_macos() -> bool {
    use std::fs::File;
    use std::io::ErrorKind;

    let Some(home) = crate::paths::default_home() else {
        return true;
    };
    let tcc = home
        .join("Library")
        .join("Application Support")
        .join("com.apple.TCC")
        .join("TCC.db");
    let blocked = |err: &std::io::Error| {
        err.kind() == ErrorKind::PermissionDenied
            || matches!(err.raw_os_error(), Some(libc::EPERM | libc::EACCES))
    };
    match File::open(&tcc) {
        Ok(_) => true,
        Err(err) if blocked(&err) => false,
        Err(_) => {
            for rel in ["Library/Safari", "Library/Mail", "Library/Accounts"] {
                let p = home.join(rel);
                if !p.exists() {
                    continue;
                }
                return match std::fs::read_dir(&p) {
                    Ok(_) => true,
                    Err(err) if blocked(&err) => false,
                    Err(_) => true,
                };
            }
            true
        }
    }
}

/// Open System Settings on the Full Disk Access pane.
pub fn open_settings() -> Result<()> {
    #[cfg(not(target_os = "macos"))]
    {
        bail!("Full Disk Access is a macOS setting");
    }
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        let urls = [
            "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_AllFiles",
            "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles",
        ];
        let mut last = None;
        for url in urls {
            match Command::new("/usr/bin/open").arg(url).status() {
                Ok(st) if st.success() => return Ok(()),
                Ok(st) => last = Some(format!("open {url} failed ({st})")),
                Err(err) => last = Some(err.to_string()),
            }
        }
        bail!(
            "{}",
            last.unwrap_or_else(|| "could not open System Settings".into())
        );
    }
}

pub fn app_hint() -> String {
    label_term_program(std::env::var("TERM_PROGRAM").ok().as_deref())
}

pub fn label_term_program(program: Option<&str>) -> String {
    match program {
        Some("Apple_Terminal") => "Terminal.app".into(),
        Some("iTerm.app") => "iTerm".into(),
        Some("ghostty") => "Ghostty".into(),
        Some("vscode") | Some("vscodevim") => "Visual Studio Code".into(),
        Some("WezTerm") => "WezTerm".into(),
        Some("Alacritty") => "Alacritty".into(),
        Some("WarpTerminal") => "Warp".into(),
        Some("kitty") => "kitty".into(),
        Some(other) => other.to_string(),
        None => "your terminal app".into(),
    }
}

pub fn needed() -> bool {
    cfg!(target_os = "macos")
}

pub fn missing() -> bool {
    needed() && !has_full_disk_access()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_known_terminals() {
        assert_eq!(label_term_program(Some("Apple_Terminal")), "Terminal.app");
        assert_eq!(label_term_program(Some("iTerm.app")), "iTerm");
        assert_eq!(label_term_program(None), "your terminal app");
    }

    #[test]
    fn linux_is_granted() {
        #[cfg(not(target_os = "macos"))]
        assert!(has_full_disk_access());
    }
}
