use anyhow::{Context, Result};
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::util::{atomic_write, timestamp_slug};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_ime_auto")]
    pub ime_auto: bool,
    #[serde(default)]
    pub sync_dir: Option<PathBuf>,
    #[serde(default)]
    pub sync_git: bool,
    #[serde(default = "default_auto_sync")]
    pub auto_sync: bool,
    #[serde(default)]
    pub device_id: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            ime_auto: default_ime_auto(),
            sync_dir: None,
            sync_git: false,
            auto_sync: true,
            device_id: None,
        }
    }
}

fn default_theme() -> String {
    "auto".to_string()
}

fn default_ime_auto() -> bool {
    true
}

fn default_auto_sync() -> bool {
    true
}

#[derive(Debug, Clone)]
pub struct Paths {
    pub root: PathBuf,
    pub config: PathBuf,
    pub db: PathBuf,
    pub backups: PathBuf,
    pub log: PathBuf,
}

impl Paths {
    pub fn discover(data_dir: Option<PathBuf>) -> Result<Self> {
        let root = match data_dir {
            Some(path) => path,
            None => {
                let base = BaseDirs::new()
                    .context("could not locate a home directory; pass --data-dir explicitly")?;
                base.data_dir().join("eword")
            }
        };
        fs::create_dir_all(&root)
            .with_context(|| format!("creating data directory {}", root.display()))?;
        let backups = root.join("backups");
        fs::create_dir_all(&backups)
            .with_context(|| format!("creating backup directory {}", backups.display()))?;
        Ok(Self {
            config: root.join("config.toml"),
            db: root.join("eword.db"),
            log: root.join("eword.log"),
            backups,
            root,
        })
    }

    pub fn default_export(&self) -> PathBuf {
        self.root
            .join(format!("eword-export-{}.json", timestamp_slug()))
    }

    pub fn default_backup(&self) -> PathBuf {
        self.backups.join(format!("eword-{}.db", timestamp_slug()))
    }
}

pub fn load(path: &Path) -> Result<Config> {
    if !path.exists() {
        return Ok(Config::default());
    }
    let text =
        fs::read_to_string(path).with_context(|| format!("reading config {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing config {}", path.display()))
}

pub fn save(path: &Path, config: &Config) -> Result<()> {
    let text = toml::to_string_pretty(config).context("serializing config")?;
    atomic_write(path, text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_settings_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let config = Config {
            sync_dir: Some(PathBuf::from(r"C:\Users\test\eword-sync")),
            sync_git: true,
            device_id: Some("pc-a".to_string()),
            ..Config::default()
        };
        save(&path, &config).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.sync_dir, config.sync_dir);
        assert!(loaded.sync_git);
        assert!(loaded.auto_sync);
        assert_eq!(loaded.device_id.as_deref(), Some("pc-a"));
    }

    #[test]
    fn save_replaces_existing_config_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut config = Config::default();
        assert!(config.ime_auto);
        save(&path, &config).unwrap();
        config.theme = "dark".to_string();
        save(&path, &config).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.theme, "dark");
    }
}
