use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::models::VideoMetadata;
use crate::{
    AcquisitionV1, Result, SourceIdentityV1, SourceariumArtifactV1, TranscriptCandidate,
    TranscriptDerivation, VesselError, YoutubeSourcePolicyV1,
};

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
            creator: None,
            creator_id: video.channel_id.clone(),
            published: normalized_publication_date(video.upload_date.as_deref()),
        },
        representation,
        acquisition: AcquisitionV1 {
            producer: "vessel".into(),
            producer_version: None,
            acquired_at: None,
            method: Some(acquisition_method.into()),
        },
        extensions: Default::default(),
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

        let (current_artifact, _) = SourceariumArtifactV1::parse_markdown(&current)?;
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
    fn materialization_is_noop_when_bytes_are_unchanged() {
        let root = temp_sourcearium();
        write_policy(&root, "alpha", "alpha");
        let source = discover_youtube_sources(&root).unwrap().remove(0);
        let video = sample_video();
        let candidate = sample_candidate(TranscriptDerivation::CreatorSubtitles);

        let first =
            materialize_youtube_transcript(&root, &source, &video, &candidate).expect("first");
        assert_eq!(first.status, MaterializeStatus::Created);
        assert!(
            first
                .path
                .ends_with("2026-09-18__abc123__a-useful-video.md")
        );

        let second =
            materialize_youtube_transcript(&root, &source, &video, &candidate).expect("second");
        assert_eq!(second.status, MaterializeStatus::Unchanged);
        assert_eq!(second.path, first.path);

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn stronger_transcript_replaces_weaker_without_renaming() {
        let root = temp_sourcearium();
        write_policy(&root, "alpha", "alpha");
        let source = discover_youtube_sources(&root).unwrap().remove(0);
        let mut video = sample_video();

        let weak = sample_candidate(TranscriptDerivation::LocalAsr);
        let first = materialize_youtube_transcript(&root, &source, &video, &weak).unwrap();

        video.title = Some("Renamed Upstream Title".into());
        let strong = sample_candidate(TranscriptDerivation::CreatorSubtitles);
        let second = materialize_youtube_transcript(&root, &source, &video, &strong).unwrap();

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
        let first = materialize_youtube_transcript(&root, &source, &video, &strong).unwrap();
        let original = fs::read_to_string(&first.path).unwrap();

        let weak = sample_candidate(TranscriptDerivation::LocalAsr);
        let second = materialize_youtube_transcript(&root, &source, &video, &weak).unwrap();
        assert_eq!(second.status, MaterializeStatus::PreservedStronger);
        assert_eq!(fs::read_to_string(&second.path).unwrap(), original);

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn repository_validator_detects_duplicate_artifact_ids() {
        let root = temp_sourcearium();
        write_policy(&root, "alpha", "alpha");
        let source = discover_youtube_sources(&root).unwrap().remove(0);
        let video = sample_video();
        let candidate = sample_candidate(TranscriptDerivation::CreatorSubtitles);
        let first = materialize_youtube_transcript(&root, &source, &video, &candidate).unwrap();

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
            materialize_youtube_transcript(&root, &source, &video, &candidate).unwrap();
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
