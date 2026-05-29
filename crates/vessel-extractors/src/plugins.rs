use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use vessel_core::models::{ChannelMetadata, InputKind, InputRef, VideoMetadata};
use vessel_core::{Result, VesselError};

use crate::traits::{ExtractContext, ExtractRequest, ExtractedItem, Extractor, SupportLevel};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginLoadError {
    pub plugin_path: PathBuf,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadedPlugin {
    pub id: String,
    pub version: String,
    pub source_dir: PathBuf,
    pub extractors: Vec<String>,
    pub providers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedProvider {
    pub plugin_id: String,
    pub provider_id: String,
    pub purpose: String,
    pub value: String,
}

pub struct PluginCatalog {
    plugins: Vec<LoadedPlugin>,
    extractors: Vec<Arc<dyn Extractor>>,
    providers: Vec<LoadedProviderEntry>,
    errors: Vec<PluginLoadError>,
}

impl Default for PluginCatalog {
    fn default() -> Self {
        Self {
            plugins: Vec::new(),
            extractors: Vec::new(),
            providers: Vec::new(),
            errors: Vec::new(),
        }
    }
}

impl PluginCatalog {
    pub fn plugins(&self) -> &[LoadedPlugin] {
        &self.plugins
    }

    pub fn errors(&self) -> &[PluginLoadError] {
        &self.errors
    }

    pub fn extractors(&self) -> Vec<Arc<dyn Extractor>> {
        self.extractors.clone()
    }

    pub fn providers(&self) -> Vec<String> {
        self.providers
            .iter()
            .map(|provider| format!("{}.{}", provider.plugin_id, provider.provider_id))
            .collect()
    }

    pub fn resolve_provider(
        &self,
        reference: &str,
        expected_purpose: &str,
    ) -> Result<Option<ResolvedProvider>> {
        if reference.trim().is_empty() {
            return Ok(None);
        }
        let (plugin_id, provider_id) = reference.split_once('.').ok_or_else(|| {
            VesselError::Config(format!(
                "provider reference `{reference}` must use the form <plugin>.<provider>"
            ))
        })?;
        let provider = self
            .providers
            .iter()
            .find(|provider| provider.plugin_id == plugin_id && provider.provider_id == provider_id)
            .ok_or_else(|| {
                VesselError::Config(format!("provider reference `{reference}` was not found"))
            })?;
        if provider.purpose != expected_purpose {
            return Err(VesselError::Config(format!(
                "provider `{reference}` does not serve purpose `{expected_purpose}`"
            )));
        }
        let value = provider.resolve()?;
        Ok(Some(ResolvedProvider {
            plugin_id: provider.plugin_id.clone(),
            provider_id: provider.provider_id.clone(),
            purpose: provider.purpose.clone(),
            value,
        }))
    }
}

pub fn load_plugins(directories: &[PathBuf]) -> PluginCatalog {
    let mut catalog = PluginCatalog::default();
    for directory in directories {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let manifest_path = path.join("plugin.toml");
            if !manifest_path.exists() {
                continue;
            }
            match load_one_plugin(&path, &manifest_path) {
                Ok(loaded) => {
                    catalog.extractors.extend(loaded.extractors);
                    catalog.providers.extend(loaded.providers);
                    catalog.plugins.push(loaded.plugin);
                }
                Err(err) => catalog.errors.push(PluginLoadError {
                    plugin_path: manifest_path,
                    message: err,
                }),
            }
        }
    }
    catalog
}

struct LoadedPluginArtifacts {
    plugin: LoadedPlugin,
    extractors: Vec<Arc<dyn Extractor>>,
    providers: Vec<LoadedProviderEntry>,
}

fn load_one_plugin(directory: &Path, manifest_path: &Path) -> std::result::Result<LoadedPluginArtifacts, String> {
    let raw = fs::read_to_string(manifest_path)
        .map_err(|err| format!("failed to read plugin manifest: {err}"))?;
    let manifest = toml::from_str::<PluginManifest>(&raw)
        .map_err(|err| format!("failed to parse plugin manifest: {err}"))?;

    let mut extractor_names = Vec::new();
    let mut extractors: Vec<Arc<dyn Extractor>> = Vec::new();
    for extractor in manifest.extractors.iter().cloned() {
        let loaded = load_fixture_extractor(&manifest.id, directory, extractor)?;
        extractor_names.push(loaded.name().to_owned());
        extractors.push(Arc::new(loaded));
    }

    let mut provider_names = Vec::new();
    let mut providers = Vec::new();
    for provider in manifest.providers.iter().cloned() {
        provider_names.push(format!("{}.{}", manifest.id, provider.id));
        providers.push(LoadedProviderEntry::from_manifest(&manifest.id, directory, provider)?);
    }

    Ok(LoadedPluginArtifacts {
        plugin: LoadedPlugin {
            id: manifest.id,
            version: manifest.version,
            source_dir: directory.to_path_buf(),
            extractors: extractor_names,
            providers: provider_names,
        },
        extractors,
        providers,
    })
}

fn load_fixture_extractor(
    plugin_id: &str,
    directory: &Path,
    manifest: ExtractorPluginManifest,
) -> std::result::Result<FixtureExtractorPlugin, String> {
    if manifest.kind != "fixture" {
        return Err(format!(
            "unsupported extractor plugin kind `{}`; only `fixture` is supported",
            manifest.kind
        ));
    }
    let fixture_path = directory.join(&manifest.fixtures);
    let fixture_raw = fs::read_to_string(&fixture_path)
        .map_err(|err| format!("failed to read extractor fixture file {}: {err}", fixture_path.display()))?;
    let fixtures = serde_json::from_str::<FixtureData>(&fixture_raw)
        .map_err(|err| format!("failed to parse extractor fixture file {}: {err}", fixture_path.display()))?;
    Ok(FixtureExtractorPlugin {
        plugin_id: plugin_id.to_owned(),
        extractor_id: manifest.id,
        display_name: manifest.name,
        match_contains: manifest.match_contains,
        support: manifest.support_level.unwrap_or(SupportLevelConfig::Generic).into(),
        fixtures,
    })
}

#[derive(Debug, Clone, Deserialize)]
struct PluginManifest {
    id: String,
    version: String,
    #[serde(default)]
    extractors: Vec<ExtractorPluginManifest>,
    #[serde(default)]
    providers: Vec<ProviderPluginManifest>,
}

#[derive(Debug, Clone, Deserialize)]
struct ExtractorPluginManifest {
    id: String,
    name: String,
    kind: String,
    fixtures: String,
    #[serde(default)]
    match_contains: Vec<String>,
    #[serde(default)]
    support_level: Option<SupportLevelConfig>,
}

#[derive(Debug, Clone, Deserialize)]
struct ProviderPluginManifest {
    id: String,
    purpose: String,
    kind: String,
    env: Option<String>,
    path: Option<String>,
    value: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum SupportLevelConfig {
    Generic,
    Native,
}

impl From<SupportLevelConfig> for SupportLevel {
    fn from(value: SupportLevelConfig) -> Self {
        match value {
            SupportLevelConfig::Generic => SupportLevel::Generic,
            SupportLevelConfig::Native => SupportLevel::Native,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
struct FixtureData {
    #[serde(default)]
    videos: BTreeMap<String, VideoMetadata>,
    #[serde(default)]
    channels: BTreeMap<String, ChannelMetadata>,
}

#[derive(Debug, Clone)]
struct FixtureExtractorPlugin {
    plugin_id: String,
    extractor_id: String,
    display_name: String,
    match_contains: Vec<String>,
    support: SupportLevel,
    fixtures: FixtureData,
}

#[async_trait]
impl Extractor for FixtureExtractorPlugin {
    fn name(&self) -> &str {
        &self.display_name
    }

    fn supports(&self, input: &InputRef) -> SupportLevel {
        if self
            .match_contains
            .iter()
            .any(|matcher| input.raw.contains(matcher))
        {
            self.support
        } else {
            SupportLevel::Unsupported
        }
    }

    async fn extract(&self, request: ExtractRequest, _ctx: ExtractContext) -> Result<ExtractedItem> {
        if is_channel_input(&request.input) {
            if let Some(channel) = self
                .fixtures
                .channels
                .get(&request.input.raw)
                .cloned()
                .or_else(|| self.fixtures.channels.values().next().cloned())
            {
                return Ok(ExtractedItem::Channel(channel));
            }
        }
        if let Some(video) = self
            .fixtures
            .videos
            .get(&request.input.raw)
            .cloned()
            .or_else(|| lookup_video_fixture(&self.fixtures.videos, &request.input))
        {
            return Ok(ExtractedItem::Video(video));
        }
        Err(VesselError::Unsupported(format!(
            "plugin extractor {}.{} has no fixture for `{}`",
            self.plugin_id, self.extractor_id, request.input.raw
        )))
    }
}

fn lookup_video_fixture(
    videos: &BTreeMap<String, VideoMetadata>,
    input: &InputRef,
) -> Option<VideoMetadata> {
    match input.kind {
        InputKind::VideoId => videos
            .get(&input.raw)
            .cloned()
            .or_else(|| videos.values().find(|video| video.video_id == input.raw).cloned()),
        InputKind::Url => videos
            .get(&input.raw)
            .cloned()
            .or_else(|| videos.values().find(|video| video.url == input.raw).cloned()),
        _ => None,
    }
}

fn is_channel_input(input: &InputRef) -> bool {
    matches!(input.kind, InputKind::ChannelId)
        || (matches!(input.kind, InputKind::Url)
            && (input.raw.contains("/channel/")
                || input.raw.contains("/@")
                || input.raw.contains("/c/")
                || input.raw.contains("/user/")))
}

#[derive(Debug, Clone)]
struct LoadedProviderEntry {
    plugin_id: String,
    provider_id: String,
    purpose: String,
    resolver: ProviderResolver,
}

impl LoadedProviderEntry {
    fn from_manifest(
        plugin_id: &str,
        directory: &Path,
        manifest: ProviderPluginManifest,
    ) -> std::result::Result<Self, String> {
        let resolver = match manifest.kind.as_str() {
            "env" => ProviderResolver::Env(
                manifest
                    .env
                    .ok_or_else(|| format!("provider {}.{} is missing `env`", plugin_id, manifest.id))?,
            ),
            "file" => {
                let relative = manifest.path.ok_or_else(|| {
                    format!("provider {}.{} is missing `path`", plugin_id, manifest.id)
                })?;
                ProviderResolver::File(directory.join(relative))
            }
            "static" => ProviderResolver::Static(
                manifest.value.ok_or_else(|| {
                    format!("provider {}.{} is missing `value`", plugin_id, manifest.id)
                })?,
            ),
            other => {
                return Err(format!(
                    "unsupported provider plugin kind `{other}`; supported kinds are env, file, static"
                ))
            }
        };
        Ok(Self {
            plugin_id: plugin_id.to_owned(),
            provider_id: manifest.id,
            purpose: manifest.purpose,
            resolver,
        })
    }

    fn resolve(&self) -> Result<String> {
        self.resolver.resolve().map_err(|err| {
            VesselError::Config(format!(
                "provider {}.{} failed: {err}",
                self.plugin_id, self.provider_id
            ))
        })
    }
}

#[derive(Debug, Clone)]
enum ProviderResolver {
    Env(String),
    File(PathBuf),
    Static(String),
}

impl ProviderResolver {
    fn resolve(&self) -> std::result::Result<String, String> {
        match self {
            ProviderResolver::Env(name) => std::env::var(name)
                .map_err(|_| format!("environment variable `{name}` is not set")),
            ProviderResolver::File(path) => fs::read_to_string(path)
                .map(|value| value.trim().to_owned())
                .map_err(|err| format!("failed to read {}: {err}", path.display())),
            ProviderResolver::Static(value) => Ok(value.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::load_plugins;
    use std::fs;
    use std::path::PathBuf;
    use time::OffsetDateTime;
    use vessel_core::models::{Availability, InputKind, InputRef, Platform, VideoMetadata};

    #[test]
    fn loads_fixture_plugin_and_resolves_provider() {
        let root = temp_plugin_root("fixture");
        let plugin_dir = root.join("demo");
        fs::create_dir_all(plugin_dir.join("fixtures")).unwrap();
        fs::write(
            plugin_dir.join("plugin.toml"),
            r#"
id = "demo"
version = "0.1.0"

[[extractors]]
id = "fixture"
name = "demo fixture"
kind = "fixture"
fixtures = "fixtures/videos.json"
match_contains = ["fixture://demo"]

[[providers]]
id = "po_token"
purpose = "youtube.po_token"
kind = "static"
value = "token123"
"#,
        )
        .unwrap();
        fs::write(
            plugin_dir.join("fixtures/videos.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "videos": {
                    "fixture://demo/video": sample_video("fixture-video", "fixture://demo/video")
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let catalog = load_plugins(std::slice::from_ref(&root));
        assert_eq!(catalog.plugins().len(), 1);
        assert!(catalog.errors().is_empty());
        let extractor = catalog.extractors().pop().unwrap();
        assert_eq!(extractor.name(), "demo fixture");
        assert_eq!(
            extractor.supports(&InputRef {
                raw: "fixture://demo/video".to_owned(),
                kind: InputKind::Url,
            }),
            super::SupportLevel::Generic
        );
        let provider = catalog
            .resolve_provider("demo.po_token", "youtube.po_token")
            .unwrap()
            .unwrap();
        assert_eq!(provider.value, "token123");
    }

    #[test]
    fn isolates_bad_plugin_manifest() {
        let root = temp_plugin_root("bad");
        let bad_dir = root.join("broken");
        fs::create_dir_all(&bad_dir).unwrap();
        fs::write(bad_dir.join("plugin.toml"), "id = ").unwrap();

        let catalog = load_plugins(&[root]);
        assert_eq!(catalog.plugins().len(), 0);
        assert_eq!(catalog.errors().len(), 1);
    }

    fn temp_plugin_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("vessel-plugin-test-{label}-{}", uuid::Uuid::now_v7()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn sample_video(video_id: &str, url: &str) -> VideoMetadata {
        VideoMetadata {
            platform: Platform::YouTube,
            video_id: video_id.to_owned(),
            channel_id: Some("fixture-channel".to_owned()),
            url: url.to_owned(),
            title: Some("Fixture Video".to_owned()),
            description: Some("fixture".to_owned()),
            duration_seconds: Some(1),
            upload_date: Some("20260101".to_owned()),
            release_timestamp: None,
            tags: Vec::new(),
            categories: Vec::new(),
            primary_category: None,
            view_count: Some(1),
            like_count: None,
            comment_count: None,
            availability: Availability::Public,
            formats: Vec::new(),
            subtitles: Vec::new(),
            thumbnails: Vec::new(),
            fetched_at: OffsetDateTime::UNIX_EPOCH,
            raw: serde_json::Value::Null,
        }
    }
}
