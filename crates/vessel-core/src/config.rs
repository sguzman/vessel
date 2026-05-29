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
    pub project: Option<String>,
    pub root: Option<String>,
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

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeLayout {
    pub project_name: String,
    pub cache_root: PathBuf,
    pub project_root: PathBuf,
    pub database_url: String,
    pub database_path: PathBuf,
    pub download_output: String,
    pub downloads_root: PathBuf,
    pub thumbnails_root: PathBuf,
    pub subtitles_root: PathBuf,
    pub plugins_root: PathBuf,
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
            project: None,
            root: None,
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

pub fn resolve_runtime_layout(
    config: &Config,
    paths: &ConfigPaths,
    cli_project: Option<&str>,
) -> Result<RuntimeLayout> {
    let project_name = cli_project
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| config.dataset.project.clone())
        .unwrap_or_else(default_project_name);
    let project_name = sanitize_project_name(&project_name);

    let cache_root = config
        .dataset
        .root
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            paths.project
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(".cache")
                .join("vessel")
        });
    let project_root = cache_root.join(&project_name);
    let database_path = project_root.join("vessel.sqlite");
    let database_url = if config.database.url == DatabaseConfig::default().url {
        format!("sqlite://{}", database_path.display())
    } else {
        config.database.url.clone()
    };
    let download_output = if config.download.output == DownloadConfig::default().output {
        project_root
            .join("downloads")
            .join("%(channel)s/%(upload_date)s - %(title)s [%(id)s].%(ext)s")
            .to_string_lossy()
            .into_owned()
    } else {
        config.download.output.clone()
    };

    Ok(RuntimeLayout {
        project_name,
        cache_root,
        project_root: project_root.clone(),
        database_url,
        database_path,
        download_output,
        downloads_root: project_root.join("downloads"),
        thumbnails_root: project_root.join("thumbnails"),
        subtitles_root: project_root.join("subtitles"),
        plugins_root: project_root.join("plugins"),
    })
}

fn default_project_name() -> String {
    env::current_dir()
        .ok()
        .and_then(|cwd| cwd.file_name().map(|value| value.to_string_lossy().into_owned()))
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "default".to_owned())
}

fn sanitize_project_name(input: &str) -> String {
    let sanitized = input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_owned();
    if sanitized.is_empty() {
        "default".to_owned()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Config, ConfigPaths, DatabaseConfig, DownloadConfig, resolve_runtime_layout,
    };
    use std::path::PathBuf;

    #[test]
    fn runtime_layout_defaults_under_project_cache_root() {
        let config = Config::default();
        let paths = ConfigPaths {
            system: PathBuf::from("/etc/vessel/config.toml"),
            user: PathBuf::from("/tmp/user-config.toml"),
            project: PathBuf::from("/workspace/demo/vessel.toml"),
        };
        let layout = resolve_runtime_layout(&config, &paths, Some("alpha")).unwrap();
        assert_eq!(layout.project_name, "alpha");
        assert_eq!(layout.project_root, PathBuf::from("/workspace/demo/.cache/vessel/alpha"));
        assert_eq!(layout.database_path, layout.project_root.join("vessel.sqlite"));
        assert!(layout.database_url.ends_with(".cache/vessel/alpha/vessel.sqlite"));
        assert_eq!(
            layout.download_output,
            layout
                .project_root
                .join("downloads/%(channel)s/%(upload_date)s - %(title)s [%(id)s].%(ext)s")
                .to_string_lossy()
                .into_owned()
        );
    }

    #[test]
    fn runtime_layout_honors_explicit_database_url_and_dataset_root() {
        let mut config = Config::default();
        config.database = DatabaseConfig {
            url: "sqlite:///tmp/custom.sqlite".to_owned(),
        };
        config.download = DownloadConfig {
            output: "custom/%(id)s.%(ext)s".to_owned(),
            ..DownloadConfig::default()
        };
        config.dataset.root = Some("/data/vessel-cache".to_owned());
        config.dataset.project = Some("beta".to_owned());
        let paths = ConfigPaths {
            system: PathBuf::from("/etc/vessel/config.toml"),
            user: PathBuf::from("/tmp/user-config.toml"),
            project: PathBuf::from("/workspace/demo/vessel.toml"),
        };
        let layout = resolve_runtime_layout(&config, &paths, None).unwrap();
        assert_eq!(layout.project_name, "beta");
        assert_eq!(layout.cache_root, PathBuf::from("/data/vessel-cache"));
        assert_eq!(layout.project_root, PathBuf::from("/data/vessel-cache/beta"));
        assert_eq!(layout.database_url, "sqlite:///tmp/custom.sqlite");
        assert_eq!(layout.download_output, "custom/%(id)s.%(ext)s");
    }
}
