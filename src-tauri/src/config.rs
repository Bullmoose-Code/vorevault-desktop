//! Read/write config.json at the platform default location.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::rules::WatchRule;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub rules: Vec<WatchRule>,
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
            rules: Vec::new(),
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
    let mut config: Config = match serde_json::from_slice::<Config>(&bytes) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("config.json is corrupt: {} — backing up + resetting", e);
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let backup = dir.join(format!("config.json.broken-{}", ts));
            let _ = std::fs::rename(&path, &backup);
            return Ok(Config::default());
        }
    };

    if migrate_legacy_watch_folder(&mut config) {
        save_to(&config, dir)?;
    }

    Ok(config)
}

/// If the loaded config has a legacy `watch_folder` set and no rules,
/// promote it to a single default rule. Returns true if migration ran
/// (caller should re-save). No-op otherwise.
fn migrate_legacy_watch_folder(config: &mut Config) -> bool {
    if !config.rules.is_empty() {
        return false;
    }
    let Some(legacy_path) = config.watch_folder.take() else {
        return false;
    };
    if legacy_path.is_empty() {
        return false;
    }
    config.rules.push(WatchRule {
        id: uuid::Uuid::new_v4().to_string(),
        path: legacy_path,
        vault_folder_id: None,
        vault_folder_label: None,
        tags: vec![],
    });
    true
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
        assert!(c.rules.is_empty());
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
            rules: vec![crate::rules::WatchRule {
                id: "rule-1".to_string(),
                path: "/tmp/foo".to_string(),
                vault_folder_id: Some("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa".to_string()),
                vault_folder_label: Some("Games / Apex".to_string()),
                tags: vec!["apex".to_string(), "clips".to_string()],
            }],
            watch_folder: None,
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
        fs::write(dir.path().join("config.json"), r#"{"debounce_ms":1234}"#).unwrap();
        let c = load_from(dir.path()).unwrap();
        assert_eq!(c.debounce_ms, 1234);
        assert!(c.watch_recursive);
        assert!(c.scan_existing_on_pick);
        assert!(c.rules.is_empty());
    }

    #[test]
    fn migration_promotes_legacy_watch_folder_to_a_single_rule() {
        let dir = TempDir::new().unwrap();
        // v0.5.x-shape config on disk: watch_folder set, rules absent.
        fs::write(
            dir.path().join("config.json"),
            r#"{"watch_folder":"/home/ryan/clips","notifications_enabled":true}"#,
        )
        .unwrap();

        let c = load_from(dir.path()).unwrap();

        assert_eq!(c.rules.len(), 1, "expected one migrated rule");
        let r = &c.rules[0];
        assert_eq!(r.path, "/home/ryan/clips");
        assert_eq!(r.vault_folder_id, None);
        assert_eq!(r.vault_folder_label, None);
        assert!(r.tags.is_empty());
        assert!(!r.id.is_empty(), "migrated rule should have a stable id");

        assert_eq!(c.watch_folder, None, "legacy field cleared after migration");
    }

    #[test]
    fn migration_persists_to_disk_so_it_only_runs_once() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("config.json"),
            r#"{"watch_folder":"/home/ryan/clips"}"#,
        )
        .unwrap();

        let first = load_from(dir.path()).unwrap();
        let migrated_id = first.rules[0].id.clone();

        // Second load: file should already be in the new shape; no re-migration.
        let second = load_from(dir.path()).unwrap();
        assert_eq!(second.rules.len(), 1);
        assert_eq!(
            second.rules[0].id, migrated_id,
            "rule id must be stable across loads"
        );
        assert_eq!(second.watch_folder, None);
    }

    #[test]
    fn migration_is_no_op_when_rules_already_present() {
        let dir = TempDir::new().unwrap();
        let existing = Config {
            rules: vec![crate::rules::WatchRule {
                id: "fixed-id-1".to_string(),
                path: "/already/configured".to_string(),
                vault_folder_id: Some("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa".to_string()),
                vault_folder_label: Some("Games / Apex".to_string()),
                tags: vec!["apex".to_string()],
            }],
            watch_folder: None,
            watch_recursive: true,
            scan_existing_on_pick: true,
            debounce_ms: 5000,
            notifications_enabled: true,
            first_launch_done: true,
        };
        save_to(&existing, dir.path()).unwrap();

        let loaded = load_from(dir.path()).unwrap();
        assert_eq!(loaded.rules.len(), 1);
        assert_eq!(loaded.rules[0].id, "fixed-id-1");
    }

    #[test]
    fn migration_no_op_when_no_legacy_and_no_rules() {
        let dir = TempDir::new().unwrap();
        // Empty config — fresh install case.
        fs::write(dir.path().join("config.json"), r#"{}"#).unwrap();
        let c = load_from(dir.path()).unwrap();
        assert!(c.rules.is_empty());
        assert!(c.watch_folder.is_none());
    }
}
