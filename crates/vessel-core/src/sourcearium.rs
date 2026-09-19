use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::{Result, VesselError};

pub const SOURCEARIUM_ARTIFACT_SCHEMA_V1: u8 = 1;
pub const SOURCEARIUM_YOUTUBE_POLICY_SCHEMA_V1: u8 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceariumArtifactV1 {
    pub schema: u8,
    pub artifact_id: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub source: SourceIdentityV1,
    pub representation: TextRepresentationV1,
    pub acquisition: AcquisitionV1,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, toml::Table>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceIdentityV1 {
    pub family: String,
    pub kind: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creator: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creator_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextRepresentationV1 {
    pub derivation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamps: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcquisitionV1 {
    pub producer: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub producer_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acquired_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
}

impl SourceariumArtifactV1 {
    pub fn validate(&self) -> Result<()> {
        if self.schema != SOURCEARIUM_ARTIFACT_SCHEMA_V1 {
            return Err(corpus_error(format!(
                "unsupported Sourcearium artifact schema {}; expected {}",
                self.schema, SOURCEARIUM_ARTIFACT_SCHEMA_V1
            )));
        }
        require_nonempty("artifact_id", &self.artifact_id)?;
        require_token("kind", &self.kind, false)?;

        require_token("source.family", &self.source.family, true)?;
        require_token("source.kind", &self.source.kind, false)?;
        require_nonempty("source.id", &self.source.id)?;
        validate_optional_nonempty("source.url", self.source.url.as_deref())?;
        validate_optional_nonempty("source.creator", self.source.creator.as_deref())?;
        validate_optional_nonempty("source.creator_id", self.source.creator_id.as_deref())?;
        validate_optional_nonempty("source.published", self.source.published.as_deref())?;

        require_token(
            "representation.derivation",
            &self.representation.derivation,
            false,
        )?;
        validate_optional_nonempty(
            "representation.language",
            self.representation.language.as_deref(),
        )?;
        validate_optional_nonempty(
            "representation.engine",
            self.representation.engine.as_deref(),
        )?;
        validate_optional_nonempty(
            "representation.model",
            self.representation.model.as_deref(),
        )?;

        if self.kind == "transcript" && self.representation.timestamps.is_none() {
            return Err(corpus_error(
                "transcript artifacts require representation.timestamps",
            ));
        }

        if self.representation.derivation == "local_asr" {
            require_optional_nonempty(
                "representation.engine",
                self.representation.engine.as_deref(),
            )?;
            require_optional_nonempty(
                "representation.model",
                self.representation.model.as_deref(),
            )?;
        }

        require_token("acquisition.producer", &self.acquisition.producer, true)?;
        validate_optional_nonempty(
            "acquisition.producer_version",
            self.acquisition.producer_version.as_deref(),
        )?;
        validate_optional_nonempty(
            "acquisition.acquired_at",
            self.acquisition.acquired_at.as_deref(),
        )?;
        if let Some(method) = self.acquisition.method.as_deref() {
            require_token("acquisition.method", method, false)?;
        }

        for namespace in self.extensions.keys() {
            require_token("extensions namespace", namespace, true)?;
        }

        Ok(())
    }

    pub fn to_markdown(&self, body: &str) -> Result<String> {
        self.validate()?;
        let front_matter = toml::to_string(self)
            .map_err(|error| corpus_error(format!("failed to serialize Sourcearium v1: {error}")))?;

        let normalized_body = normalize_line_endings(body);
        let normalized_body = normalized_body.trim_end_matches('\n');

        let mut output = String::with_capacity(front_matter.len() + normalized_body.len() + 16);
        output.push_str("+++\n");
        output.push_str(&front_matter);
        if !front_matter.ends_with('\n') {
            output.push('\n');
        }
        output.push_str("+++\n");
        if !normalized_body.is_empty() {
            output.push('\n');
            output.push_str(normalized_body);
            output.push('\n');
        }
        Ok(output)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct YoutubeSourcePolicyV1 {
    pub schema: u8,
    pub family: String,
    pub source_key: String,
    pub channel: YoutubeChannelPolicyV1,
    #[serde(default)]
    pub selection: YoutubeSelectionPolicyV1,
    #[serde(default)]
    pub transcripts: YoutubeTranscriptPolicyV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct YoutubeChannelPolicyV1 {
    pub input: String,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub handle: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct YoutubeSelectionPolicyV1 {
    #[serde(default)]
    pub published_on_or_after: Option<String>,
    #[serde(default)]
    pub include_video_ids: Vec<String>,
    #[serde(default)]
    pub exclude_video_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct YoutubeTranscriptPolicyV1 {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub preferred_languages: Vec<String>,
    #[serde(default = "default_true")]
    pub allow_creator_subtitles: bool,
    #[serde(default = "default_true")]
    pub allow_auto_captions: bool,
    #[serde(default = "default_true")]
    pub allow_local_asr: bool,
}

impl Default for YoutubeTranscriptPolicyV1 {
    fn default() -> Self {
        Self {
            enabled: true,
            preferred_languages: Vec::new(),
            allow_creator_subtitles: true,
            allow_auto_captions: true,
            allow_local_asr: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoSelection {
    Included,
    ExplicitlyIncluded,
    ExplicitlyExcluded,
    BeforeCutoff,
    PublicationDateUnresolved,
}

impl YoutubeSourcePolicyV1 {
    pub fn parse_toml(input: &str) -> Result<Self> {
        let policy: Self = toml::from_str(input)
            .map_err(|error| corpus_error(format!("invalid YouTube source policy TOML: {error}")))?;
        policy.validate()?;
        Ok(policy)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema != SOURCEARIUM_YOUTUBE_POLICY_SCHEMA_V1 {
            return Err(corpus_error(format!(
                "unsupported YouTube source policy schema {}; expected {}",
                self.schema, SOURCEARIUM_YOUTUBE_POLICY_SCHEMA_V1
            )));
        }
        if self.family != "youtube" {
            return Err(corpus_error(format!(
                "YouTube source policy family must be youtube, got {}",
                self.family
            )));
        }

        require_token("source_key", &self.source_key, true)?;
        require_nonempty("channel.input", &self.channel.input)?;
        validate_optional_nonempty("channel.id", self.channel.id.as_deref())?;
        validate_optional_nonempty("channel.handle", self.channel.handle.as_deref())?;

        if let Some(cutoff) = self.selection.published_on_or_after.as_deref()
            && !is_iso_date(cutoff)
        {
            return Err(corpus_error(format!(
                "selection.published_on_or_after must be YYYY-MM-DD, got {cutoff}"
            )));
        }

        ensure_unique("selection.include_video_ids", &self.selection.include_video_ids)?;
        ensure_unique("selection.exclude_video_ids", &self.selection.exclude_video_ids)?;

        let included: HashSet<&str> = self
            .selection
            .include_video_ids
            .iter()
            .map(String::as_str)
            .collect();
        if let Some(conflict) = self
            .selection
            .exclude_video_ids
            .iter()
            .map(String::as_str)
            .find(|id| included.contains(id))
        {
            return Err(corpus_error(format!(
                "video {conflict} appears in both include_video_ids and exclude_video_ids"
            )));
        }

        ensure_unique(
            "transcripts.preferred_languages",
            &self.transcripts.preferred_languages,
        )?;
        for language in &self.transcripts.preferred_languages {
            require_nonempty("transcripts.preferred_languages item", language)?;
        }

        Ok(())
    }

    pub fn select_video(&self, video_id: &str, published_date: Option<&str>) -> Result<VideoSelection> {
        require_nonempty("video_id", video_id)?;

        if self
            .selection
            .exclude_video_ids
            .iter()
            .any(|candidate| candidate == video_id)
        {
            return Ok(VideoSelection::ExplicitlyExcluded);
        }

        if self
            .selection
            .include_video_ids
            .iter()
            .any(|candidate| candidate == video_id)
        {
            return Ok(VideoSelection::ExplicitlyIncluded);
        }

        let Some(cutoff) = self.selection.published_on_or_after.as_deref() else {
            return Ok(VideoSelection::Included);
        };

        let Some(published_date) = published_date else {
            return Ok(VideoSelection::PublicationDateUnresolved);
        };
        if !is_iso_date(published_date) {
            return Err(corpus_error(format!(
                "published date must be normalized to YYYY-MM-DD, got {published_date}"
            )));
        }

        if published_date >= cutoff {
            Ok(VideoSelection::Included)
        } else {
            Ok(VideoSelection::BeforeCutoff)
        }
    }
}

fn corpus_error(message: impl Into<String>) -> VesselError {
    VesselError::Corpus(message.into())
}

fn default_true() -> bool {
    true
}

fn normalize_line_endings(value: &str) -> String {
    value.replace("\r\n", "\n").replace('\r', "\n")
}

fn require_nonempty(field: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(corpus_error(format!("{field} must not be empty")));
    }
    Ok(())
}

fn require_optional_nonempty(field: &str, value: Option<&str>) -> Result<()> {
    let Some(value) = value else {
        return Err(corpus_error(format!("{field} is required")));
    };
    require_nonempty(field, value)
}

fn validate_optional_nonempty(field: &str, value: Option<&str>) -> Result<()> {
    if let Some(value) = value {
        require_nonempty(field, value)?;
    }
    Ok(())
}

fn require_token(field: &str, value: &str, allow_hyphen: bool) -> Result<()> {
    if !is_token(value, allow_hyphen) {
        return Err(corpus_error(format!(
            "{field} must be a lowercase identifier, got {value:?}"
        )));
    }
    Ok(())
}

fn is_token(value: &str, allow_hyphen: bool) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return false;
    }
    chars.all(|ch| {
        ch.is_ascii_lowercase()
            || ch.is_ascii_digit()
            || ch == '_'
            || (allow_hyphen && ch == '-')
    })
}

fn is_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit)
}

fn ensure_unique(field: &str, values: &[String]) -> Result<()> {
    let mut seen = HashSet::new();
    for value in values {
        require_nonempty(&format!("{field} item"), value)?;
        if !seen.insert(value.as_str()) {
            return Err(corpus_error(format!(
                "{field} contains duplicate value {value:?}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_artifact() -> SourceariumArtifactV1 {
        SourceariumArtifactV1 {
            schema: 1,
            artifact_id: "youtube:video:abc123:transcript".into(),
            kind: "transcript".into(),
            title: Some("Example".into()),
            source: SourceIdentityV1 {
                family: "youtube".into(),
                kind: "video".into(),
                id: "abc123".into(),
                url: Some("https://www.youtube.com/watch?v=abc123".into()),
                creator: Some("Example Channel".into()),
                creator_id: Some("UCexample".into()),
                published: Some("2026-09-18".into()),
            },
            representation: TextRepresentationV1 {
                derivation: "platform_auto_caption".into(),
                language: Some("en".into()),
                timestamps: Some(true),
                engine: None,
                model: None,
            },
            acquisition: AcquisitionV1 {
                producer: "vessel".into(),
                producer_version: None,
                acquired_at: None,
                method: Some("platform_caption_fetch".into()),
            },
            extensions: BTreeMap::new(),
        }
    }

    #[test]
    fn artifact_serialization_is_deterministic() {
        let artifact = base_artifact();
        let first = artifact
            .to_markdown("[00:00:03] Hello.\r\n")
            .expect("first serialization");
        let second = artifact
            .to_markdown("[00:00:03] Hello.\n")
            .expect("second serialization");
        assert_eq!(first, second);
        assert!(first.starts_with("+++\nschema = 1\n"));
        assert!(first.ends_with("[00:00:03] Hello.\n"));
    }

    #[test]
    fn local_asr_requires_engine_and_model() {
        let mut artifact = base_artifact();
        artifact.representation.derivation = "local_asr".into();
        assert!(artifact.validate().is_err());

        artifact.representation.engine = Some("whisper-candle".into());
        artifact.representation.model = Some("small.en".into());
        assert!(artifact.validate().is_ok());
    }

    #[test]
    fn policy_defaults_enable_transcripts_and_fallbacks() {
        let policy = YoutubeSourcePolicyV1::parse_toml(
            r#"
schema = 1
family = "youtube"
source_key = "example"

[channel]
input = "https://www.youtube.com/@example"
"#,
        )
        .expect("policy");
        assert!(policy.transcripts.enabled);
        assert!(policy.transcripts.allow_creator_subtitles);
        assert!(policy.transcripts.allow_auto_captions);
        assert!(policy.transcripts.allow_local_asr);
    }

    #[test]
    fn explicit_include_bypasses_cutoff() {
        let policy = YoutubeSourcePolicyV1::parse_toml(
            r#"
schema = 1
family = "youtube"
source_key = "example"

[channel]
input = "@example"

[selection]
published_on_or_after = "2025-01-01"
include_video_ids = ["old-important"]
"#,
        )
        .expect("policy");

        assert_eq!(
            policy
                .select_video("old-important", Some("2019-01-01"))
                .expect("selection"),
            VideoSelection::ExplicitlyIncluded
        );
        assert_eq!(
            policy
                .select_video("ordinary-old", Some("2019-01-01"))
                .expect("selection"),
            VideoSelection::BeforeCutoff
        );
    }

    #[test]
    fn include_exclude_conflict_is_invalid() {
        let error = YoutubeSourcePolicyV1::parse_toml(
            r#"
schema = 1
family = "youtube"
source_key = "example"

[channel]
input = "@example"

[selection]
include_video_ids = ["same"]
exclude_video_ids = ["same"]
"#,
        )
        .expect_err("conflict must fail");
        assert!(error.to_string().contains("both include_video_ids and exclude_video_ids"));
    }
}
