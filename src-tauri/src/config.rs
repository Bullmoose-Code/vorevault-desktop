//! Read/write config.json at the platform default location.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub watch_folder: Option<String>,
    pub watch_recursive: bool,
    pub scan_existing_on_pick: bool,
    pub debounce_ms: u64,
    pub notifications_enabled: bool,
    pub first_launch_done: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            watch_folder: None,
            watch_recursive: true,
            scan_existing_on_pick: true,
            debounce_ms: 5000,
            notifications_enabled: true,
            first_launch_done: false,
        }
    }
}

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    NoConfigDir,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(e) => write!(f, "config io: {}", e),
            ConfigError::NoConfigDir => write!(f, "could not resolve platform config dir"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Resolve the config directory: `<dirs::config_dir>/VoreVault/`.
/// Creates the directory if it doesn't exist.
pub fn config_dir() -> Result<PathBuf, ConfigError> {
    let base = dirs::config_dir().ok_or(ConfigError::NoConfigDir)?;
    let dir = base.join("VoreVault");
    std::fs::create_dir_all(&dir).map_err(ConfigError::Io)?;
    Ok(dir)
}

/// Load config from `<config_dir>/config.json`. Returns `Default::default()`
/// if the file doesn't exist. If the file exists but is corrupt JSON, backs
/// it up to `config.json.broken-<timestamp>` and returns defaults.
pub fn load() -> Result<Config, ConfigError> {
    load_from(&config_dir()?)
}

pub fn load_from(dir: &Path) -> Result<Config, ConfigError> {
    let path = dir.join("config.json");
    if !path.exists() {
        return Ok(Config::default());
    }
    let bytes = std::fs::read(&path).map_err(ConfigError::Io)?;
    match serde_json::from_slice::<Config>(&bytes) {
        Ok(c) => Ok(c),
        Err(e) => {
            log::warn!("config.json is corrupt: {} — backing up + resetting", e);
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let backup = dir.join(format!("config.json.broken-{}", ts));
            let _ = std::fs::rename(&path, &backup);
            Ok(Config::default())
        }
    }
}

/// Save config atomically: write to `<dir>/config.json.tmp`, then rename to
/// `<dir>/config.json`.
pub fn save(config: &Config) -> Result<(), ConfigError> {
    save_to(config, &config_dir()?)
}

pub fn save_to(config: &Config, dir: &Path) -> Result<(), ConfigError> {
    let final_path = dir.join("config.json");
    let tmp_path = dir.join("config.json.tmp");
    let json = serde_json::to_vec_pretty(config).expect("Config is always serializable");
    std::fs::write(&tmp_path, &json).map_err(ConfigError::Io)?;
    std::fs::rename(&tmp_path, &final_path).map_err(ConfigError::Io)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn default_config_has_expected_values() {
        let c = Config::default();
        assert_eq!(c.watch_folder, None);
        assert!(c.watch_recursive);
        assert!(c.scan_existing_on_pick);
        assert_eq!(c.debounce_ms, 5000);
    }

    #[test]
    fn load_returns_defaults_when_file_missing() {
        let dir = TempDir::new().unwrap();
        let c = load_from(dir.path()).unwrap();
        assert_eq!(c, Config::default());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = TempDir::new().unwrap();
        let original = Config {
            watch_folder: Some("/tmp/foo".to_string()),
            watch_recursive: true,
            scan_existing_on_pick: false,
            debounce_ms: 3000,
            notifications_enabled: false,
            first_launch_done: true,
        };
        save_to(&original, dir.path()).unwrap();
        let loaded = load_from(dir.path()).unwrap();
        assert_eq!(loaded, original);
    }

    #[test]
    fn first_launch_done_defaults_to_false() {
        assert!(!Config::default().first_launch_done);
    }

    #[test]
    fn load_existing_config_without_first_launch_field_defaults_to_false() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("config.json"),
            r#"{"watch_folder":"/foo","notifications_enabled":true}"#,
        )
        .unwrap();
        let c = load_from(dir.path()).unwrap();
        assert!(!c.first_launch_done);
    }

    #[test]
    fn save_writes_atomically_via_tmp_file() {
        let dir = TempDir::new().unwrap();
        let cfg = Config::default();
        save_to(&cfg, dir.path()).unwrap();
        assert!(dir.path().join("config.json").exists());
        assert!(!dir.path().join("config.json.tmp").exists());
    }

    #[test]
    fn load_with_corrupt_json_backs_up_and_returns_defaults() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("config.json"), "not json {").unwrap();
        let c = load_from(dir.path()).unwrap();
        assert_eq!(c, Config::default());
        let entries: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        let has_broken_backup = entries.iter().any(|n| n.starts_with("config.json.broken-"));
        assert!(
            has_broken_backup,
            "expected a config.json.broken-* backup, got {:?}",
            entries
        );
    }

    #[test]
    fn load_accepts_partial_json_and_fills_defaults() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("config.json"), r#"{"watch_folder":"/foo"}"#).unwrap();
        let c = load_from(dir.path()).unwrap();
        assert_eq!(c.watch_folder, Some("/foo".to_string()));
        assert!(c.watch_recursive);
        assert!(c.scan_existing_on_pick);
        assert_eq!(c.debounce_ms, 5000);
    }
}
