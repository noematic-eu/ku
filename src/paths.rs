use std::path::{Path, PathBuf};

/// Home / config / data for the *invoking* user.
///
/// Under `sudo`, `dirs::*` would resolve to root (`/var/root`, `/root`). We prefer
/// `SUDO_USER` so config and SQLite stay in the original account.
#[derive(Debug, Clone)]
pub struct UserPaths {
    pub home: PathBuf,
    pub config_file: PathBuf,
    pub data_dir: PathBuf,
}

pub fn user_paths() -> UserPaths {
    if let Some(invoker) = invoking_user() {
        return paths_for_home(&invoker.home);
    }
    fallback_paths()
}

pub fn default_config_path() -> PathBuf {
    user_paths().config_file
}

pub fn default_data_dir() -> PathBuf {
    user_paths().data_dir
}

pub fn default_home() -> Option<PathBuf> {
    invoking_user().map(|u| u.home).or_else(dirs::home_dir)
}

pub fn paths_for_home(home: &Path) -> UserPaths {
    #[cfg(target_os = "macos")]
    {
        let app_support = home.join("Library").join("Application Support").join("ku");
        UserPaths {
            home: home.to_path_buf(),
            config_file: app_support.join("config.toml"),
            data_dir: app_support,
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        UserPaths {
            home: home.to_path_buf(),
            config_file: home.join(".config").join("ku").join("config.toml"),
            data_dir: home.join(".local").join("share").join("ku"),
        }
    }
}

fn fallback_paths() -> UserPaths {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let config_file = dirs::config_dir()
        .unwrap_or_else(|| home.join(".config"))
        .join("ku")
        .join("config.toml");
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| home.join(".local").join("share"))
        .join("ku");
    UserPaths {
        home,
        config_file,
        data_dir,
    }
}

#[derive(Debug, Clone)]
struct Invoker {
    home: PathBuf,
    uid: u32,
    gid: u32,
}

fn invoking_user() -> Option<Invoker> {
    #[cfg(unix)]
    {
        if !is_root() {
            return None;
        }
        let user = std::env::var("SUDO_USER").ok()?;
        if user.is_empty() || user == "root" {
            return None;
        }
        let pw = lookup_passwd(&user);
        let home = std::env::var("SUDO_HOME")
            .ok()
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .or_else(|| pw.as_ref().map(|p| p.home.clone()))?;
        let uid = std::env::var("SUDO_UID")
            .ok()
            .and_then(|s| s.parse().ok())
            .or_else(|| pw.as_ref().map(|p| p.uid))?;
        let gid = std::env::var("SUDO_GID")
            .ok()
            .and_then(|s| s.parse().ok())
            .or_else(|| pw.as_ref().map(|p| p.gid))?;
        Some(Invoker { home, uid, gid })
    }
    #[cfg(not(unix))]
    {
        None
    }
}

#[cfg(unix)]
fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

#[cfg(unix)]
#[derive(Clone)]
struct PasswdInfo {
    home: PathBuf,
    uid: u32,
    gid: u32,
}

#[cfg(unix)]
fn lookup_passwd(name: &str) -> Option<PasswdInfo> {
    use std::ffi::{CStr, CString};

    let cname = CString::new(name).ok()?;
    let mut pwd = unsafe { std::mem::zeroed::<libc::passwd>() };
    let mut buf = vec![0u8; 4096];
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    let rc = unsafe {
        libc::getpwnam_r(
            cname.as_ptr(),
            &mut pwd,
            buf.as_mut_ptr().cast::<libc::c_char>(),
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() {
        return None;
    }
    let home = unsafe { CStr::from_ptr(pwd.pw_dir) }
        .to_string_lossy()
        .into_owned();
    if home.is_empty() {
        return None;
    }
    Some(PasswdInfo {
        home: PathBuf::from(home),
        uid: pwd.pw_uid,
        gid: pwd.pw_gid,
    })
}

/// If running via `sudo`, chown `path` (and its `ku/` parent) back to SUDO_USER
/// so later non-root runs can still write history and config.
pub fn chown_to_invoker(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::chown;
        let Some(invoker) = invoking_user() else {
            return;
        };
        let _ = chown(path, Some(invoker.uid), Some(invoker.gid));
        if let Some(parent) = path.parent()
            && parent.file_name().is_some_and(|n| n == "ku")
        {
            let _ = chown(parent, Some(invoker.uid), Some(invoker.gid));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_paths_match_platform() {
        let paths = paths_for_home(Path::new("/Users/alice"));
        #[cfg(target_os = "macos")]
        {
            assert_eq!(
                paths.config_file,
                PathBuf::from("/Users/alice/Library/Application Support/ku/config.toml")
            );
            assert_eq!(
                paths.data_dir,
                PathBuf::from("/Users/alice/Library/Application Support/ku")
            );
        }
        #[cfg(not(target_os = "macos"))]
        {
            assert_eq!(
                paths.config_file,
                PathBuf::from("/Users/alice/.config/ku/config.toml")
            );
            assert_eq!(
                paths.data_dir,
                PathBuf::from("/Users/alice/.local/share/ku")
            );
        }
    }

    #[test]
    fn non_root_is_not_a_sudo_invoker() {
        if !cfg!(unix) {
            return;
        }
        let root = unsafe { libc::geteuid() == 0 };
        if !root {
            assert!(invoking_user().is_none());
        }
    }
}
