use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub disk: DiskConfig,
    #[serde(default)]
    pub processes: ProcessConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    #[serde(default = "default_refresh")]
    pub refresh_interval: u64,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_retention")]
    pub history_retention_days: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskConfig {
    #[serde(default = "default_warn")]
    pub warning_threshold: u8,
    #[serde(default = "default_crit")]
    pub critical_threshold: u8,
    #[serde(default)]
    pub watched_paths: Vec<String>,
    #[serde(default = "default_snapshot_interval")]
    pub snapshot_interval: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessConfig {
    #[serde(default = "default_windows")]
    pub history_window: Vec<String>,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            refresh_interval: default_refresh(),
            theme: default_theme(),
            history_retention_days: default_retention(),
        }
    }
}

impl Default for DiskConfig {
    fn default() -> Self {
        Self {
            warning_threshold: default_warn(),
            critical_threshold: default_crit(),
            watched_paths: Vec::new(),
            snapshot_interval: default_snapshot_interval(),
        }
    }
}

impl Default for ProcessConfig {
    fn default() -> Self {
        Self {
            history_window: default_windows(),
        }
    }
}

fn default_refresh() -> u64 {
    2
}
fn default_theme() -> String {
    "dark".into()
}
fn default_retention() -> u32 {
    7
}
fn default_warn() -> u8 {
    80
}
fn default_crit() -> u8 {
    90
}
fn default_snapshot_interval() -> u64 {
    300
}
fn default_windows() -> Vec<String> {
    vec!["1m".into(), "5m".into(), "1h".into(), "24h".into()]
}

impl Config {
    pub fn load(explicit: Option<&Path>) -> Result<(Self, PathBuf)> {
        let path = match explicit {
            Some(p) => p.to_path_buf(),
            None => default_config_path(),
        };
        if path.exists() {
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("reading config {}", path.display()))?;
            let mut cfg: Config = toml::from_str(&raw)
                .with_context(|| format!("parsing config {}", path.display()))?;
            cfg.normalize();
            return Ok((cfg, path));
        }
        let mut cfg = Config::default();
        cfg.normalize();
        if explicit.is_none() {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).ok();
                crate::paths::chown_to_invoker(parent);
            }
            if fs::write(&path, cfg.to_toml()).is_ok() {
                crate::paths::chown_to_invoker(&path);
            }
        }
        Ok((cfg, path))
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_else(|_| String::new())
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
            crate::paths::chown_to_invoker(parent);
        }
        fs::write(path, self.to_toml())
            .with_context(|| format!("writing config {}", path.display()))?;
        crate::paths::chown_to_invoker(path);
        Ok(())
    }

    pub fn normalize(&mut self) {
        if self.general.refresh_interval == 0 {
            self.general.refresh_interval = 1;
        }
        self.general.history_retention_days = self.general.history_retention_days.clamp(1, 30);
        self.disk.warning_threshold = self.disk.warning_threshold.min(99);
        self.disk.critical_threshold = self
            .disk
            .critical_threshold
            .max(self.disk.warning_threshold)
            .min(100);
        if self.disk.snapshot_interval < 30 {
            self.disk.snapshot_interval = 30;
        }
        if self.disk.watched_paths.is_empty() {
            self.disk.watched_paths = auto_watched_paths();
        }
        if self.processes.history_window.is_empty() {
            self.processes.history_window = default_windows();
        }
        let theme = self.general.theme.to_ascii_lowercase();
        self.general.theme = if theme == "light" { "light" } else { "dark" }.into();
    }

    pub fn is_light(&self) -> bool {
        self.general.theme.eq_ignore_ascii_case("light")
    }
}

pub fn default_config_path() -> PathBuf {
    crate::paths::default_config_path()
}

pub fn default_data_dir() -> PathBuf {
    crate::paths::default_data_dir()
}

pub fn auto_watched_paths() -> Vec<String> {
    let mut paths = vec!["/var/log".to_string(), "/tmp".to_string()];
    if let Some(home) = crate::paths::default_home() {
        paths.push(home.to_string_lossy().into_owned());
    }
    for extra in ["/var/lib/docker", "/opt/homebrew", "/usr/local"] {
        paths.push(extra.to_string());
    }
    paths.retain(|p| Path::new(p).exists());
    paths.sort();
    paths.dedup();
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_spec_example() {
        let raw = r#"
[general]
refresh_interval = 2
theme = "dark"
history_retention_days = 7

[disk]
warning_threshold = 80
critical_threshold = 90
watched_paths = ["/var/log", "/tmp"]

[processes]
history_window = ["1m", "5m", "1h", "24h"]
"#;
        let mut cfg: Config = toml::from_str(raw).unwrap();
        cfg.normalize();
        assert_eq!(cfg.general.refresh_interval, 2);
        assert_eq!(cfg.disk.critical_threshold, 90);
        assert_eq!(cfg.processes.history_window.len(), 4);
    }

    #[test]
    fn clamps_retention_and_thresholds() {
        let mut cfg = Config::default();
        cfg.general.history_retention_days = 99;
        cfg.disk.warning_threshold = 95;
        cfg.disk.critical_threshold = 70;
        cfg.normalize();
        assert_eq!(cfg.general.history_retention_days, 30);
        assert_eq!(cfg.disk.critical_threshold, 95);
    }

    #[test]
    fn save_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut cfg = Config::default();
        cfg.general.theme = "light".into();
        cfg.save(&path).unwrap();
        let (loaded, _) = Config::load(Some(&path)).unwrap();
        assert_eq!(loaded.general.theme, "light");
    }
}
