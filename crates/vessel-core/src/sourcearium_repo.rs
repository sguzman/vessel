use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::models::{ChannelMetadata, VideoMetadata};
use crate::{
    AcquisitionV1, Result, SourceIdentityV1, SourceariumArtifactV1, TranscriptCandidate,
    TranscriptDerivation, VesselError, VideoSelection, YoutubeSourcePolicyV1,
};

#[derive(Debug, Clone, Serialize)]
pub struct SourceariumInventoryReport {
    pub youtube_policies: usize,
    pub artifacts: usize,
    pub by_family: BTreeMap<String, usize>,
    pub by_kind: BTreeMap<String, usize>,
    pub by_derivation: BTreeMap<String, usize>,
    pub by_language: BTreeMap<String, usize>,
    pub invalid_artifacts: Vec<String>,
}

pub fn inventory_sourcearium_repository(
    sourcearium_root: &Path,
) -> Result<SourceariumInventoryReport> {
    let sources_root = sourcearium_root.join("sources");
    if !sources_root.is_dir() {
        return Err(corpus_error(format!(
            "Sourcearium sources directory is missing: {}",
            sources_root.display()
        )));
    }

    let youtube_policies = discover_youtube_sources(sourcearium_root)?.len();
    let mut files = Vec::new();
    collect_files(&sources_root, &mut files)?;
    files.sort();

    let mut artifacts = 0usize;
    let mut by_family = BTreeMap::new();
    let mut by_kind = BTreeMap::new();
    let mut by_derivation = BTreeMap::new();
    let mut by_language = BTreeMap::new();
    let mut invalid_artifacts = Vec::new();

    for path in files {
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        if path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("README.md"))
        {
            continue;
        }

        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) => {
                invalid_artifacts.push(format!("{}: {error}", path.display()));
                continue;
            }
        };
        let (artifact, _) = match SourceariumArtifactV1::parse_markdown(&raw) {
            Ok(parsed) => parsed,
            Err(error) => {
                invalid_artifacts.push(format!("{}: {error}", path.display()));
                continue;
            }
        };

        artifacts += 1;
        increment_inventory(&mut by_family, &artifact.source.family);
        increment_inventory(&mut by_kind, &artifact.kind);
        increment_inventory(&mut by_derivation, &artifact.representation.derivation);
        if let Some(language) = artifact.representation.language.as_deref() {
            increment_inventory(&mut by_language, language);
        }
    }

    Ok(SourceariumInventoryReport {
        youtube_policies,
        artifacts,
        by_family,
        by_kind,
        by_derivation,
        by_language,
        invalid_artifacts,
    })
}

fn increment_inventory(map: &mut BTreeMap<String, usize>, key: &str) {
    *map.entry(key.to_owned()).or_insert(0) += 1;
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceariumValidationReport {
    pub valid: bool,
    pub policies_validated: usize,
    pub artifacts_validated: usize,
    pub errors: Vec<String>,
}

pub fn validate_sourcearium_repository(
    sourcearium_root: &Path,
) -> Result<SourceariumValidationReport> {
    let sources_root = sourcearium_root.join("sources");
    if !sources_root.is_dir() {
        return Err(corpus_error(format!(
            "Sourcearium sources directory is missing: {}",
            sources_root.display()
        )));
    }

    let mut errors = Vec::new();
    let policies_validated = match discover_youtube_sources(sourcearium_root) {
        Ok(sources) => sources.len(),
        Err(error) => {
            errors.push(error.to_string());
            0
        }
    };

    let mut files = Vec::new();
    collect_files(&sources_root, &mut files)?;
    files.sort();

    let mut artifact_paths_by_id = BTreeMap::<String, PathBuf>::new();
    let mut artifacts_validated = 0usize;

    for path in files {
        if let Some(extension) = path.extension().and_then(|value| value.to_str()) {
            let extension = extension.to_ascii_lowercase();
            if matches!(extension.as_str(), "pdf" | "epub" | "doc" | "docx") {
                errors.push(format!(
                    "forbidden binary document format under sources/: {}",
                    path.display()
                ));
                continue;
            }
        }

        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        if path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("README.md"))
        {
            continue;
        }

        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) => {
                errors.push(format!("{}: not valid UTF-8 text: {error}", path.display()));
                continue;
            }
        };

        let (artifact, body) = match SourceariumArtifactV1::parse_markdown(&raw) {
            Ok(parsed) => parsed,
            Err(error) => {
                errors.push(format!("{}: {error}", path.display()));
                continue;
            }
        };

        if let Some(previous) =
            artifact_paths_by_id.insert(artifact.artifact_id.clone(), path.clone())
        {
            errors.push(format!(
                "duplicate Sourcearium artifact_id {:?}: {} and {}",
                artifact.artifact_id,
                previous.display(),
                path.display()
            ));
        }

        if let Err(error) = validate_artifact_body(&artifact, &body) {
            errors.push(format!("{}: {error}", path.display()));
            continue;
        }

        artifacts_validated += 1;
    }

    Ok(SourceariumValidationReport {
        valid: errors.is_empty(),
        policies_validated,
        artifacts_validated,
        errors,
    })
}

fn collect_files(directory: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_files(&path, output)?;
        } else {
            output.push(path);
        }
    }
    Ok(())
}

fn validate_artifact_body(artifact: &SourceariumArtifactV1, body: &str) -> Result<()> {
    if artifact.kind != "transcript" {
        return Ok(());
    }

    if body.trim().is_empty() {
        return Err(corpus_error("transcript body must not be empty"));
    }

    if artifact.representation.timestamps != Some(true) {
        return Ok(());
    }

    let mut previous = None;
    let mut segments = 0usize;
    for line in body.lines().filter(|line| !line.trim().is_empty()) {
        let seconds = parse_sourcearium_timestamp(line).ok_or_else(|| {
            corpus_error(format!(
                "timestamped transcript line does not begin with [HH:MM:SS]: {line:?}"
            ))
        })?;
        if let Some(previous) = previous
            && seconds < previous
        {
            return Err(corpus_error("transcript timestamps must be monotonic"));
        }
        previous = Some(seconds);
        segments += 1;
    }

    if segments == 0 {
        return Err(corpus_error("timestamped transcript contains no segments"));
    }

    Ok(())
}

fn parse_sourcearium_timestamp(line: &str) -> Option<u64> {
    let close = line.find(']')?;
    if !line.starts_with('[') || close < 8 {
        return None;
    }
    let timestamp = &line[1..close];
    let mut parts = timestamp.split(':');
    let hours = parts.next()?.parse::<u64>().ok()?;
    let minutes = parts.next()?.parse::<u64>().ok()?;
    let seconds = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() || minutes >= 60 || seconds >= 60 {
        return None;
    }
    Some(hours * 3_600 + minutes * 60 + seconds)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceariumPruneCandidate {
    pub source_key: String,
    pub video_id: String,
    pub artifact_id: String,
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceariumPrunePlan {
    pub configured_sources: usize,
    pub inspected_artifacts: usize,
    pub preserved_unresolved: usize,
    pub candidates: Vec<SourceariumPruneCandidate>,
}

pub fn plan_sourcearium_prune(sourcearium_root: &Path) -> Result<SourceariumPrunePlan> {
    let sources = discover_youtube_sources(sourcearium_root)?;
    let mut inspected_artifacts = 0usize;
    let mut preserved_unresolved = 0usize;
    let mut candidates = Vec::new();

    for source in &sources {
        let transcripts_dir = source.source_dir.join("transcripts");
        if !transcripts_dir.exists() {
            continue;
        }

        let mut paths = fs::read_dir(&transcripts_dir)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        paths.sort();

        for path in paths {
            if !path.is_file() || path.extension().and_then(|value| value.to_str()) != Some("md") {
                continue;
            }
            if path
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case("README.md"))
            {
                continue;
            }

            let raw = fs::read_to_string(&path)?;
            let (artifact, _) = SourceariumArtifactV1::parse_markdown(&raw)
                .map_err(|error| corpus_error(format!("{}: {error}", path.display())))?;

            if artifact.source.family != "youtube"
                || artifact.source.kind != "video"
                || artifact.kind != "transcript"
            {
                return Err(corpus_error(format!(
                    "{} is under a YouTube transcript directory but is not a YouTube video transcript",
                    path.display()
                )));
            }

            let expected_artifact_id = format!("youtube:video:{}:transcript", artifact.source.id);
            if artifact.artifact_id != expected_artifact_id {
                return Err(corpus_error(format!(
                    "{} has artifact_id {:?}; expected {:?}",
                    path.display(),
                    artifact.artifact_id,
                    expected_artifact_id
                )));
            }

            inspected_artifacts += 1;
            let selection = source
                .policy
                .select_video(&artifact.source.id, artifact.source.published.as_deref())?;

            let reason = match selection {
                VideoSelection::ExplicitlyExcluded => Some("explicitly_excluded"),
                VideoSelection::BeforeCutoff => Some("before_cutoff"),
                VideoSelection::PublicationDateUnresolved => {
                    preserved_unresolved += 1;
                    None
                }
                VideoSelection::Included | VideoSelection::ExplicitlyIncluded => None,
            };

            if let Some(reason) = reason {
                candidates.push(SourceariumPruneCandidate {
                    source_key: source.policy.source_key.clone(),
                    video_id: artifact.source.id.clone(),
                    artifact_id: artifact.artifact_id,
                    path,
                    reason: reason.into(),
                });
            }
        }
    }

    candidates.sort_by(|left, right| left.path.cmp(&right.path));

    Ok(SourceariumPrunePlan {
        configured_sources: sources.len(),
        inspected_artifacts,
        preserved_unresolved,
        candidates,
    })
}

pub fn apply_sourcearium_prune(
    sourcearium_root: &Path,
    plan: &SourceariumPrunePlan,
) -> Result<usize> {
    let sources_root = sourcearium_root.join("sources").canonicalize()?;
    let mut validated_paths = Vec::with_capacity(plan.candidates.len());

    for candidate in &plan.candidates {
        let canonical_path = candidate.path.canonicalize()?;
        if !canonical_path.starts_with(&sources_root) {
            return Err(corpus_error(format!(
                "refusing to prune path outside Sourcearium sources tree: {}",
                candidate.path.display()
            )));
        }

        let raw = fs::read_to_string(&canonical_path)?;
        let (artifact, _) = SourceariumArtifactV1::parse_markdown(&raw)
            .map_err(|error| corpus_error(format!("{}: {error}", canonical_path.display())))?;
        if artifact.artifact_id != candidate.artifact_id || artifact.source.id != candidate.video_id
        {
            return Err(corpus_error(format!(
                "refusing stale prune candidate {}; artifact identity changed",
                candidate.path.display()
            )));
        }

        validated_paths.push(canonical_path);
    }

    for path in &validated_paths {
        fs::remove_file(path)?;
    }

    Ok(validated_paths.len())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceariumYoutubeSource {
    pub source_dir: PathBuf,
    pub policy_path: PathBuf,
    pub policy: YoutubeSourcePolicyV1,
}

pub fn discover_youtube_sources(sourcearium_root: &Path) -> Result<Vec<SourceariumYoutubeSource>> {
    let youtube_root = sourcearium_root.join("sources").join("youtube");
    if !youtube_root.exists() {
        return Ok(Vec::new());
    }
    if !youtube_root.is_dir() {
        return Err(corpus_error(format!(
            "Sourcearium YouTube root is not a directory: {}",
            youtube_root.display()
        )));
    }

    let mut source_dirs = fs::read_dir(&youtube_root)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    source_dirs.retain(|path| path.is_dir());
    source_dirs.sort();

    let mut sources = Vec::new();
    for source_dir in source_dirs {
        let policy_path = source_dir.join("source.toml");
        if !policy_path.exists() {
            continue;
        }

        let dir_key = source_dir
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| {
                corpus_error(format!(
                    "Sourcearium source directory is not valid UTF-8: {}",
                    source_dir.display()
                ))
            })?;

        let raw = fs::read_to_string(&policy_path)?;
        let policy = YoutubeSourcePolicyV1::parse_toml(&raw)
            .map_err(|error| corpus_error(format!("{}: {error}", policy_path.display())))?;

        if policy.source_key != dir_key {
            return Err(corpus_error(format!(
                "{} declares source_key {:?}, but directory key is {:?}",
                policy_path.display(),
                policy.source_key,
                dir_key
            )));
        }

        sources.push(SourceariumYoutubeSource {
            source_dir,
            policy_path,
            policy,
        });
    }

    Ok(sources)
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExistingSourceariumArtifact {
    pub path: PathBuf,
    pub artifact: SourceariumArtifactV1,
}

pub fn load_youtube_transcript_artifact(
    source: &SourceariumYoutubeSource,
    video_id: &str,
) -> Result<Option<ExistingSourceariumArtifact>> {
    let artifact_id = format!("youtube:video:{video_id}:transcript");
    let transcripts_dir = source.source_dir.join("transcripts");
    let Some(path) = find_artifact_path(&transcripts_dir, &artifact_id)? else {
        return Ok(None);
    };
    let raw = fs::read_to_string(&path)?;
    let (artifact, _) = SourceariumArtifactV1::parse_markdown(&raw)
        .map_err(|error| corpus_error(format!("{}: {error}", path.display())))?;
    Ok(Some(ExistingSourceariumArtifact { path, artifact }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterializeStatus {
    Created,
    Updated,
    Unchanged,
    PreservedStronger,
    PreservedUnknownDerivation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializeResult {
    pub path: PathBuf,
    pub status: MaterializeStatus,
}

pub fn materialize_youtube_transcript(
    sourcearium_root: &Path,
    source: &SourceariumYoutubeSource,
    video: &VideoMetadata,
    channel: Option<&ChannelMetadata>,
    candidate: &TranscriptCandidate,
) -> Result<MaterializeResult> {
    candidate.validate()?;

    let youtube_root = sourcearium_root.join("sources").join("youtube");
    if !source.source_dir.starts_with(&youtube_root) {
        return Err(corpus_error(format!(
            "source directory {} is outside Sourcearium YouTube root {}",
            source.source_dir.display(),
            youtube_root.display()
        )));
    }

    let artifact_id = format!("youtube:video:{}:transcript", video.video_id);
    let body = candidate.render_body()?;
    let representation = candidate.to_sourcearium_representation()?;
    let acquisition_method = match candidate.derivation {
        TranscriptDerivation::CreatorSubtitles | TranscriptDerivation::PlatformAutoCaption => {
            "platform_caption_fetch"
        }
        TranscriptDerivation::LocalAsr => "local_asr",
    };

    let mut extensions = BTreeMap::new();
    if let Some(handle) = channel.and_then(|channel| channel.handle.as_deref()) {
        let mut youtube = toml::Table::new();
        youtube.insert(
            "channel_handle".into(),
            toml::Value::String(handle.to_owned()),
        );
        extensions.insert("youtube".into(), youtube);
    }

    let artifact = SourceariumArtifactV1 {
        schema: 1,
        artifact_id: artifact_id.clone(),
        kind: "transcript".into(),
        title: video.title.clone(),
        source: SourceIdentityV1 {
            family: "youtube".into(),
            kind: "video".into(),
            id: video.video_id.clone(),
            url: Some(video.url.clone()),
            creator: channel.and_then(|channel| channel.title.clone()),
            creator_id: video
                .channel_id
                .clone()
                .or_else(|| channel.map(|channel| channel.channel_id.clone())),
            published: normalized_publication_date(video.upload_date.as_deref()),
        },
        representation,
        acquisition: AcquisitionV1 {
            producer: "vessel".into(),
            producer_version: None,
            acquired_at: None,
            method: Some(acquisition_method.into()),
        },
        extensions,
    };
    let rendered = artifact.to_markdown(&body)?;

    let transcripts_dir = source.source_dir.join("transcripts");
    fs::create_dir_all(&transcripts_dir)?;
    let existing = find_artifact_path(&transcripts_dir, &artifact_id)?;

    let path = if let Some(path) = existing {
        let current = fs::read_to_string(&path)?;
        if current == rendered {
            return Ok(MaterializeResult {
                path,
                status: MaterializeStatus::Unchanged,
            });
        }

        let (current_artifact, current_body) = SourceariumArtifactV1::parse_markdown(&current)?;
        if same_textual_representation(&current_artifact, &current_body, &artifact, &body) {
            return Ok(MaterializeResult {
                path,
                status: MaterializeStatus::Unchanged,
            });
        }

        let Some(current_rank) = derivation_rank(&current_artifact.representation.derivation)
        else {
            return Ok(MaterializeResult {
                path,
                status: MaterializeStatus::PreservedUnknownDerivation,
            });
        };
        let candidate_rank = candidate.derivation.quality_rank();
        if current_rank > candidate_rank {
            return Ok(MaterializeResult {
                path,
                status: MaterializeStatus::PreservedStronger,
            });
        }
        path
    } else {
        transcripts_dir.join(new_transcript_filename(video))
    };

    let status = if path.exists() {
        MaterializeStatus::Updated
    } else {
        MaterializeStatus::Created
    };
    atomic_write(&path, rendered.as_bytes())?;

    Ok(MaterializeResult { path, status })
}

fn find_artifact_path(directory: &Path, artifact_id: &str) -> Result<Option<PathBuf>> {
    if !directory.exists() {
        return Ok(None);
    }

    let mut matches = Vec::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if !path.is_file() || path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }

        let raw = fs::read_to_string(&path)?;
        let (artifact, _) = SourceariumArtifactV1::parse_markdown(&raw)
            .map_err(|error| corpus_error(format!("{}: {error}", path.display())))?;
        if artifact.artifact_id == artifact_id {
            matches.push(path);
        }
    }

    matches.sort();
    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.into_iter().next()),
        _ => Err(corpus_error(format!(
            "duplicate Sourcearium artifact_id {artifact_id:?}: {}",
            matches
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

fn new_transcript_filename(video: &VideoMetadata) -> String {
    let date = normalized_publication_date(video.upload_date.as_deref())
        .unwrap_or_else(|| "undated".into());
    let slug = slugify(video.title.as_deref().unwrap_or("video"));
    format!("{date}__{}__{slug}.md", video.video_id)
}

fn normalized_publication_date(value: Option<&str>) -> Option<String> {
    let value = value?;
    if value.len() >= 10
        && value.as_bytes().get(4) == Some(&b'-')
        && value.as_bytes().get(7) == Some(&b'-')
    {
        return Some(value[..10].to_owned());
    }
    if value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Some(format!("{}-{}-{}", &value[..4], &value[4..6], &value[6..8]));
    }
    Some(value.to_owned())
}

fn slugify(value: &str) -> String {
    let mut slug = String::new();
    let mut separator_pending = false;

    for ch in value.chars() {
        if ch.is_alphanumeric() {
            if separator_pending && !slug.is_empty() {
                slug.push('-');
            }
            separator_pending = false;
            for lower in ch.to_lowercase() {
                slug.push(lower);
            }
        } else if !slug.is_empty() {
            separator_pending = true;
        }

        if slug.chars().count() >= 80 {
            break;
        }
    }

    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "video".into()
    } else {
        slug
    }
}

fn same_textual_representation(
    current: &SourceariumArtifactV1,
    current_body: &str,
    candidate: &SourceariumArtifactV1,
    candidate_body: &str,
) -> bool {
    current.representation == candidate.representation
        && current_body.trim_end_matches('\n') == candidate_body.trim_end_matches('\n')
}

fn derivation_rank(value: &str) -> Option<u8> {
    match value {
        "creator_subtitles" => Some(TranscriptDerivation::CreatorSubtitles.quality_rank()),
        "platform_auto_caption" => Some(TranscriptDerivation::PlatformAutoCaption.quality_rank()),
        "local_asr" => Some(TranscriptDerivation::LocalAsr.quality_rank()),
        _ => None,
    }
}

fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| corpus_error(format!("artifact path has no parent: {}", path.display())))?;
    fs::create_dir_all(parent)?;

    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            corpus_error(format!(
                "artifact filename is not valid UTF-8: {}",
                path.display()
            ))
        })?;
    let temporary = parent.join(format!(".{file_name}.{}.tmp", uuid::Uuid::new_v4()));

    let result = (|| -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(VesselError::Io)
}

fn corpus_error(message: impl Into<String>) -> VesselError {
    VesselError::Corpus(message.into())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::Value;
    use time::OffsetDateTime;
    use uuid::Uuid;

    use crate::models::{Availability, Platform};

    use super::*;

    fn temp_sourcearium() -> PathBuf {
        let root = std::env::temp_dir().join(format!("vessel-sourcearium-test-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("sources").join("youtube")).expect("create source root");
        root
    }

    fn write_policy(root: &Path, key: &str, declared_key: &str) {
        let dir = root.join("sources").join("youtube").join(key);
        fs::create_dir_all(&dir).expect("create source dir");
        fs::write(
            dir.join("source.toml"),
            format!(
                r#"schema = 1
family = "youtube"
source_key = "{declared_key}"

[channel]
input = "https://www.youtube.com/@{key}"
"#
            ),
        )
        .expect("write policy");
    }

    #[test]
    fn discovers_sources_in_deterministic_order() {
        let root = temp_sourcearium();
        write_policy(&root, "zeta", "zeta");
        write_policy(&root, "alpha", "alpha");

        let sources = discover_youtube_sources(&root).expect("discover");
        let keys = sources
            .iter()
            .map(|source| source.policy.source_key.as_str())
            .collect::<Vec<_>>();
        assert_eq!(keys, vec!["alpha", "zeta"]);

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn ignores_directories_without_source_policy() {
        let root = temp_sourcearium();
        fs::create_dir_all(root.join("sources").join("youtube").join("notes"))
            .expect("create notes dir");
        write_policy(&root, "alpha", "alpha");

        let sources = discover_youtube_sources(&root).expect("discover");
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].policy.source_key, "alpha");

        fs::remove_dir_all(root).expect("cleanup");
    }

    fn sample_video() -> VideoMetadata {
        VideoMetadata {
            platform: Platform::YouTube,
            video_id: "abc123".into(),
            channel_id: Some("UCexample".into()),
            url: "https://www.youtube.com/watch?v=abc123".into(),
            title: Some("A Useful Video".into()),
            description: None,
            duration_seconds: Some(60),
            upload_date: Some("20260918".into()),
            release_timestamp: None,
            tags: Vec::new(),
            categories: Vec::new(),
            primary_category: None,
            view_count: None,
            like_count: None,
            comment_count: None,
            availability: Availability::Public,
            formats: Vec::new(),
            subtitles: Vec::new(),
            thumbnails: Vec::new(),
            fetched_at: OffsetDateTime::UNIX_EPOCH,
            raw: Value::Null,
        }
    }

    fn sample_channel() -> ChannelMetadata {
        ChannelMetadata {
            platform: Platform::YouTube,
            channel_id: "UCexample".into(),
            handle: Some("@example".into()),
            url: "https://www.youtube.com/@example".into(),
            title: Some("Example Channel".into()),
            description: None,
            subscriber_count: None,
            video_count: None,
            view_count: None,
            avatar_url: None,
            banner_url: None,
            fetched_at: OffsetDateTime::UNIX_EPOCH,
            raw: Value::Null,
        }
    }

    fn sample_candidate(derivation: TranscriptDerivation) -> TranscriptCandidate {
        TranscriptCandidate {
            derivation,
            language: Some("en".into()),
            timestamps: true,
            engine: (derivation == TranscriptDerivation::LocalAsr).then(|| "whisper-candle".into()),
            model: (derivation == TranscriptDerivation::LocalAsr).then(|| "small.en".into()),
            segments: vec![crate::TranscriptSegment {
                start_seconds: Some(3),
                text: "Hello corpus.".into(),
            }],
        }
    }

    #[test]
    fn materialization_preserves_channel_creator_provenance() {
        let root = temp_sourcearium();
        write_policy(&root, "alpha", "alpha");
        let source = discover_youtube_sources(&root).unwrap().remove(0);
        let video = sample_video();
        let channel = sample_channel();
        let candidate = sample_candidate(TranscriptDerivation::CreatorSubtitles);

        let result =
            materialize_youtube_transcript(&root, &source, &video, Some(&channel), &candidate)
                .expect("materialize");
        let raw = fs::read_to_string(&result.path).unwrap();
        let (artifact, _) = SourceariumArtifactV1::parse_markdown(&raw).unwrap();

        assert_eq!(artifact.source.creator.as_deref(), Some("Example Channel"));
        assert_eq!(artifact.source.creator_id.as_deref(), Some("UCexample"));
        assert_eq!(
            artifact.extensions["youtube"]["channel_handle"].as_str(),
            Some("@example")
        );

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn materialization_is_noop_when_bytes_are_unchanged() {
        let root = temp_sourcearium();
        write_policy(&root, "alpha", "alpha");
        let source = discover_youtube_sources(&root).unwrap().remove(0);
        let video = sample_video();
        let candidate = sample_candidate(TranscriptDerivation::CreatorSubtitles);

        let first = materialize_youtube_transcript(&root, &source, &video, None, &candidate)
            .expect("first");
        assert_eq!(first.status, MaterializeStatus::Created);
        assert!(
            first
                .path
                .ends_with("2026-09-18__abc123__a-useful-video.md")
        );

        let second = materialize_youtube_transcript(&root, &source, &video, None, &candidate)
            .expect("second");
        assert_eq!(second.status, MaterializeStatus::Unchanged);
        assert_eq!(second.path, first.path);

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn mutable_source_metadata_does_not_churn_unchanged_transcript() {
        let root = temp_sourcearium();
        write_policy(&root, "alpha", "alpha");
        let source = discover_youtube_sources(&root).unwrap().remove(0);
        let mut video = sample_video();
        let channel = sample_channel();
        let candidate = sample_candidate(TranscriptDerivation::CreatorSubtitles);

        let first =
            materialize_youtube_transcript(&root, &source, &video, Some(&channel), &candidate)
                .expect("first");
        let original = fs::read_to_string(&first.path).unwrap();

        video.title = Some("Changed display title".into());
        let mut changed_channel = channel.clone();
        changed_channel.title = Some("Changed channel title".into());
        changed_channel.handle = Some("@changed".into());

        let second = materialize_youtube_transcript(
            &root,
            &source,
            &video,
            Some(&changed_channel),
            &candidate,
        )
        .expect("second");

        assert_eq!(second.status, MaterializeStatus::Unchanged);
        assert_eq!(fs::read_to_string(&second.path).unwrap(), original);

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn stronger_transcript_replaces_weaker_without_renaming() {
        let root = temp_sourcearium();
        write_policy(&root, "alpha", "alpha");
        let source = discover_youtube_sources(&root).unwrap().remove(0);
        let mut video = sample_video();

        let weak = sample_candidate(TranscriptDerivation::LocalAsr);
        let first = materialize_youtube_transcript(&root, &source, &video, None, &weak).unwrap();

        video.title = Some("Renamed Upstream Title".into());
        let strong = sample_candidate(TranscriptDerivation::CreatorSubtitles);
        let second = materialize_youtube_transcript(&root, &source, &video, None, &strong).unwrap();

        assert_eq!(second.status, MaterializeStatus::Updated);
        assert_eq!(second.path, first.path);
        let raw = fs::read_to_string(&second.path).unwrap();
        let (artifact, _) = SourceariumArtifactV1::parse_markdown(&raw).unwrap();
        assert_eq!(artifact.representation.derivation, "creator_subtitles");

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn weaker_transcript_never_downgrades_stronger_existing_artifact() {
        let root = temp_sourcearium();
        write_policy(&root, "alpha", "alpha");
        let source = discover_youtube_sources(&root).unwrap().remove(0);
        let video = sample_video();

        let strong = sample_candidate(TranscriptDerivation::CreatorSubtitles);
        let first = materialize_youtube_transcript(&root, &source, &video, None, &strong).unwrap();
        let original = fs::read_to_string(&first.path).unwrap();

        let weak = sample_candidate(TranscriptDerivation::LocalAsr);
        let second = materialize_youtube_transcript(&root, &source, &video, None, &weak).unwrap();
        assert_eq!(second.status, MaterializeStatus::PreservedStronger);
        assert_eq!(fs::read_to_string(&second.path).unwrap(), original);

        fs::remove_dir_all(root).expect("cleanup");
    }

    fn write_policy_with_selection(
        root: &Path,
        key: &str,
        cutoff: Option<&str>,
        include: &[&str],
        exclude: &[&str],
    ) {
        let dir = root.join("sources").join("youtube").join(key);
        fs::create_dir_all(&dir).expect("create source dir");

        let cutoff = cutoff
            .map(|value| format!("published_on_or_after = \"{value}\"\n"))
            .unwrap_or_default();
        let include = include
            .iter()
            .map(|value| format!("\"{value}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let exclude = exclude
            .iter()
            .map(|value| format!("\"{value}\""))
            .collect::<Vec<_>>()
            .join(", ");

        fs::write(
            dir.join("source.toml"),
            format!(
                r#"schema = 1
family = "youtube"
source_key = "{key}"

[channel]
input = "https://www.youtube.com/@{key}"

[selection]
{cutoff}include_video_ids = [{include}]
exclude_video_ids = [{exclude}]
"#
            ),
        )
        .expect("write policy");
    }

    #[test]
    fn prune_plan_only_includes_locally_provable_policy_exclusions() {
        let root = temp_sourcearium();
        write_policy_with_selection(
            &root,
            "alpha",
            Some("2026-09-19"),
            &["explicit-keep"],
            &["explicit-drop"],
        );
        let source = discover_youtube_sources(&root).unwrap().remove(0);

        let mut old_video = sample_video();
        old_video.video_id = "old-video".into();
        old_video.url = "https://www.youtube.com/watch?v=old-video".into();
        old_video.upload_date = Some("20260918".into());

        let mut explicit_keep = sample_video();
        explicit_keep.video_id = "explicit-keep".into();
        explicit_keep.url = "https://www.youtube.com/watch?v=explicit-keep".into();
        explicit_keep.upload_date = Some("20200101".into());

        let mut explicit_drop = sample_video();
        explicit_drop.video_id = "explicit-drop".into();
        explicit_drop.url = "https://www.youtube.com/watch?v=explicit-drop".into();
        explicit_drop.upload_date = None;

        let candidate = sample_candidate(TranscriptDerivation::CreatorSubtitles);
        materialize_youtube_transcript(&root, &source, &old_video, None, &candidate).unwrap();
        materialize_youtube_transcript(&root, &source, &explicit_keep, None, &candidate).unwrap();
        materialize_youtube_transcript(&root, &source, &explicit_drop, None, &candidate).unwrap();

        let plan = plan_sourcearium_prune(&root).expect("plan");
        assert_eq!(plan.inspected_artifacts, 3);
        assert_eq!(plan.preserved_unresolved, 0);
        assert_eq!(plan.candidates.len(), 2);
        assert!(plan.candidates.iter().any(|candidate| {
            candidate.video_id == "old-video" && candidate.reason == "before_cutoff"
        }));
        assert!(plan.candidates.iter().any(|candidate| {
            candidate.video_id == "explicit-drop" && candidate.reason == "explicitly_excluded"
        }));
        assert!(
            !plan
                .candidates
                .iter()
                .any(|candidate| candidate.video_id == "explicit-keep")
        );

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn prune_apply_validates_entire_plan_before_removing_anything() {
        let root = temp_sourcearium();
        write_policy_with_selection(&root, "alpha", None, &[], &["drop-a", "drop-b"]);
        let source = discover_youtube_sources(&root).unwrap().remove(0);
        let candidate = sample_candidate(TranscriptDerivation::CreatorSubtitles);

        let mut video_a = sample_video();
        video_a.video_id = "drop-a".into();
        video_a.url = "https://www.youtube.com/watch?v=drop-a".into();
        let result_a =
            materialize_youtube_transcript(&root, &source, &video_a, None, &candidate).unwrap();

        let mut video_b = sample_video();
        video_b.video_id = "drop-b".into();
        video_b.url = "https://www.youtube.com/watch?v=drop-b".into();
        let result_b =
            materialize_youtube_transcript(&root, &source, &video_b, None, &candidate).unwrap();

        let mut plan = plan_sourcearium_prune(&root).expect("plan");
        assert_eq!(plan.candidates.len(), 2);
        plan.candidates[1].artifact_id = "youtube:video:wrong:transcript".into();

        assert!(apply_sourcearium_prune(&root, &plan).is_err());
        assert!(result_a.path.exists());
        assert!(result_b.path.exists());

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn prune_apply_removes_only_planned_artifacts() {
        let root = temp_sourcearium();
        write_policy_with_selection(&root, "alpha", None, &[], &["drop"]);
        let source = discover_youtube_sources(&root).unwrap().remove(0);

        let mut drop_video = sample_video();
        drop_video.video_id = "drop".into();
        drop_video.url = "https://www.youtube.com/watch?v=drop".into();

        let mut keep_video = sample_video();
        keep_video.video_id = "keep".into();
        keep_video.url = "https://www.youtube.com/watch?v=keep".into();

        let candidate = sample_candidate(TranscriptDerivation::CreatorSubtitles);
        let drop_result =
            materialize_youtube_transcript(&root, &source, &drop_video, None, &candidate).unwrap();
        let keep_result =
            materialize_youtube_transcript(&root, &source, &keep_video, None, &candidate).unwrap();

        let plan = plan_sourcearium_prune(&root).expect("plan");
        assert_eq!(plan.candidates.len(), 1);
        assert_eq!(apply_sourcearium_prune(&root, &plan).expect("apply"), 1);
        assert!(!drop_result.path.exists());
        assert!(keep_result.path.exists());

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn repository_inventory_summarizes_materialized_artifacts() {
        let root = temp_sourcearium();
        write_policy(&root, "alpha", "alpha");
        let source = discover_youtube_sources(&root).unwrap().remove(0);
        let video = sample_video();
        let candidate = sample_candidate(TranscriptDerivation::PlatformAutoCaption);

        materialize_youtube_transcript(&root, &source, &video, None, &candidate)
            .expect("materialize");

        let inventory = inventory_sourcearium_repository(&root).expect("inventory");
        assert_eq!(inventory.youtube_policies, 1);
        assert_eq!(inventory.artifacts, 1);
        assert_eq!(inventory.by_family.get("youtube"), Some(&1));
        assert_eq!(inventory.by_kind.get("transcript"), Some(&1));
        assert_eq!(
            inventory.by_derivation.get("platform_auto_caption"),
            Some(&1)
        );
        assert_eq!(inventory.by_language.get("en"), Some(&1));
        assert!(inventory.invalid_artifacts.is_empty());

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn repository_validator_ignores_source_tree_readmes() {
        let root = temp_sourcearium();
        fs::write(
            root.join("sources").join("README.md"),
            "# Source documentation\n",
        )
        .unwrap();

        let report = validate_sourcearium_repository(&root).unwrap();
        assert!(report.valid);
        assert_eq!(report.artifacts_validated, 0);

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn repository_validator_detects_duplicate_artifact_ids() {
        let root = temp_sourcearium();
        write_policy(&root, "alpha", "alpha");
        let source = discover_youtube_sources(&root).unwrap().remove(0);
        let video = sample_video();
        let candidate = sample_candidate(TranscriptDerivation::CreatorSubtitles);
        let first =
            materialize_youtube_transcript(&root, &source, &video, None, &candidate).unwrap();

        let duplicate = source.source_dir.join("transcripts").join("duplicate.md");
        fs::copy(&first.path, &duplicate).unwrap();

        let report = validate_sourcearium_repository(&root).unwrap();
        assert!(!report.valid);
        assert!(
            report
                .errors
                .iter()
                .any(|error| error.contains("duplicate Sourcearium artifact_id"))
        );

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn repository_validator_rejects_non_monotonic_timestamp_body() {
        let root = temp_sourcearium();
        write_policy(&root, "alpha", "alpha");
        let source = discover_youtube_sources(&root).unwrap().remove(0);
        let video = sample_video();
        let candidate = sample_candidate(TranscriptDerivation::CreatorSubtitles);
        let materialized =
            materialize_youtube_transcript(&root, &source, &video, None, &candidate).unwrap();
        let raw = fs::read_to_string(&materialized.path).unwrap();
        let raw = raw.replace(
            "[00:00:03] Hello corpus.",
            "[00:00:10] Later.\n\n[00:00:09] Earlier.",
        );
        fs::write(&materialized.path, raw).unwrap();

        let report = validate_sourcearium_repository(&root).unwrap();
        assert!(!report.valid);
        assert!(
            report
                .errors
                .iter()
                .any(|error| error.contains("timestamps must be monotonic"))
        );

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn repository_validator_rejects_binary_document_formats() {
        let root = temp_sourcearium();
        fs::write(root.join("sources").join("book.pdf"), b"%PDF").unwrap();

        let report = validate_sourcearium_repository(&root).unwrap();
        assert!(!report.valid);
        assert!(
            report
                .errors
                .iter()
                .any(|error| error.contains("forbidden binary document format"))
        );

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn rejects_source_key_directory_mismatch() {
        let root = temp_sourcearium();
        write_policy(&root, "folder-key", "different-key");

        let error = discover_youtube_sources(&root).expect_err("mismatch must fail");
        assert!(error.to_string().contains("directory key"));

        fs::remove_dir_all(root).expect("cleanup");
    }
}
