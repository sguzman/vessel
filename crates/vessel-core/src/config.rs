use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Result, VesselError};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub database: DatabaseConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub youtube: YoutubeConfig,
    #[serde(default)]
    pub download: DownloadConfig,
    #[serde(default)]
    pub dataset: DatasetConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    pub level: String,
    pub format: LoggingFormat,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct YoutubeConfig {
    pub po_token_provider: Option<String>,
    pub cookies_from_browser: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadConfig {
    pub archive: bool,
    pub output: String,
    pub concurrent_fragments: usize,
    pub retries: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetConfig {
    pub snapshot_raw_json: bool,
    pub snapshot_unchanged: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LoggingFormat {
    #[default]
    Human,
    Json,
}

#[derive(Debug, Clone)]
pub struct ConfigPaths {
    pub system: PathBuf,
    pub user: PathBuf,
    pub project: PathBuf,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: "sqlite://vessel.sqlite".to_owned(),
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_owned(),
            format: LoggingFormat::Human,
        }
    }
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            archive: true,
            output: "%(channel)s/%(upload_date)s - %(title)s [%(id)s].%(ext)s".to_owned(),
            concurrent_fragments: 8,
            retries: 10,
        }
    }
}

impl Default for DatasetConfig {
    fn default() -> Self {
        Self {
            snapshot_raw_json: true,
            snapshot_unchanged: false,
        }
    }
}

impl Config {
    pub fn default_with_paths() -> (Self, ConfigPaths) {
        (Self::default(), ConfigPaths::discover())
    }
}

impl ConfigPaths {
    pub fn discover() -> Self {
        let project = env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("vessel.toml");
        let user = home_dir()
            .map(|dir| dir.join(".config/vessel/config.toml"))
            .unwrap_or_else(|| PathBuf::from(".config/vessel/config.toml"));
        let system = PathBuf::from("/etc/vessel/config.toml");

        Self {
            system,
            user,
            project,
        }
    }

    pub fn existing_in_precedence(&self) -> Vec<PathBuf> {
        [self.system.clone(), self.user.clone(), self.project.clone()]
            .into_iter()
            .filter(|path| path.exists())
            .collect()
    }
}

pub fn load_config() -> Result<(Config, ConfigPaths, Vec<PathBuf>)> {
    let (mut config, paths) = Config::default_with_paths();
    let existing = paths.existing_in_precedence();

    for path in &existing {
        merge_file(&mut config, path)?;
    }

    Ok((config, paths, existing))
}

fn merge_file(config: &mut Config, path: &Path) -> Result<()> {
    let raw = fs::read_to_string(path)?;
    let next = toml::from_str::<Config>(&raw)
        .map_err(|err| VesselError::Config(format!("{}: {err}", path.display())))?;
    *config = next;
    Ok(())
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}
