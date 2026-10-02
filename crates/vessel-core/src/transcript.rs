use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{
    Result, SourceariumArtifactV1, TextRepresentationV1, VesselError, YoutubeTranscriptPolicyV1,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptDerivation {
    CreatorSubtitles,
    PlatformAutoCaption,
    LocalAsr,
}

impl TranscriptDerivation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CreatorSubtitles => "creator_subtitles",
            Self::PlatformAutoCaption => "platform_auto_caption",
            Self::LocalAsr => "local_asr",
        }
    }

    pub const fn quality_rank(self) -> u8 {
        match self {
            Self::CreatorSubtitles => 3,
            Self::PlatformAutoCaption => 2,
            Self::LocalAsr => 1,
        }
    }

    pub fn allowed_by(self, policy: &YoutubeTranscriptPolicyV1) -> bool {
        match self {
            Self::CreatorSubtitles => policy.allow_creator_subtitles,
            Self::PlatformAutoCaption => policy.allow_auto_captions,
            Self::LocalAsr => policy.allow_local_asr,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptRequest {
    pub video_id: String,
    pub url: String,
    pub preferred_languages: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeakerAttribution {
    Unresolved,
    ModelMatched,
    HumanConfirmed,
}

impl SpeakerAttribution {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unresolved => "unresolved",
            Self::ModelMatched => "model_matched",
            Self::HumanConfirmed => "human_confirmed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptSpeaker {
    pub diarization_label: String,
    pub identity: Option<String>,
    pub attribution: SpeakerAttribution,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiarizationProvenance {
    pub engine: String,
    pub model: String,
    pub registry_revision: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptSegment {
    pub start_seconds: Option<u64>,
    pub text: String,
    pub speaker: Option<TranscriptSpeaker>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptCandidate {
    pub derivation: TranscriptDerivation,
    pub language: Option<String>,
    pub timestamps: bool,
    pub engine: Option<String>,
    pub model: Option<String>,
    pub diarization: Option<DiarizationProvenance>,
    pub segments: Vec<TranscriptSegment>,
}

impl TranscriptCandidate {
    pub fn from_sourcearium_artifact(artifact: &SourceariumArtifactV1, body: &str) -> Result<Self> {
        if artifact.kind != "transcript" {
            return Err(corpus_error(format!(
                "Sourcearium artifact {:?} is not a transcript",
                artifact.artifact_id
            )));
        }
        let derivation = match artifact.representation.derivation.as_str() {
            "creator_subtitles" => TranscriptDerivation::CreatorSubtitles,
            "platform_auto_caption" => TranscriptDerivation::PlatformAutoCaption,
            "local_asr" => TranscriptDerivation::LocalAsr,
            other => {
                return Err(corpus_error(format!(
                    "unsupported transcript derivation {other:?}"
                )));
            }
        };
        let timestamps = artifact.representation.timestamps.ok_or_else(|| {
            corpus_error("transcript artifact is missing representation.timestamps")
        })?;
        let diarization = artifact
            .extensions
            .get("diarization")
            .map(parse_diarization_extension)
            .transpose()?;
        let segments = parse_sourcearium_transcript_body(body, timestamps)?;

        let candidate = Self {
            derivation,
            language: artifact.representation.language.clone(),
            timestamps,
            engine: artifact.representation.engine.clone(),
            model: artifact.representation.model.clone(),
            diarization,
            segments,
        };
        candidate.validate()?;
        Ok(candidate)
    }

    pub fn validate(&self) -> Result<()> {
        if self.segments.is_empty() {
            return Err(corpus_error("transcript contains no segments"));
        }

        if self.derivation == TranscriptDerivation::LocalAsr {
            require_nonempty("local ASR engine", self.engine.as_deref())?;
            require_nonempty("local ASR model", self.model.as_deref())?;
        }

        if let Some(diarization) = self.diarization.as_ref() {
            require_nonempty("diarization engine", Some(&diarization.engine))?;
            require_nonempty("diarization model", Some(&diarization.model))?;
        }

        let mut previous = None;
        for segment in &self.segments {
            if segment.text.is_empty() {
                return Err(corpus_error("transcript segment text must not be empty"));
            }
            if let Some(speaker) = segment.speaker.as_ref() {
                require_nonempty(
                    "segment speaker diarization label",
                    Some(&speaker.diarization_label),
                )?;
                validate_optional_nonempty(
                    "segment speaker identity",
                    speaker.identity.as_deref(),
                )?;
                if speaker.identity.is_some()
                    && speaker.attribution == SpeakerAttribution::Unresolved
                {
                    return Err(corpus_error(
                        "identified segment speaker cannot have unresolved attribution",
                    ));
                }
            }

            match (self.timestamps, segment.start_seconds) {
                (true, None) => {
                    return Err(corpus_error(
                        "timestamped transcript contains a segment without a timestamp",
                    ));
                }
                (false, Some(_)) => {
                    return Err(corpus_error(
                        "untimestamped transcript contains a timestamped segment",
                    ));
                }
                _ => {}
            }

            if let Some(start) = segment.start_seconds {
                if let Some(previous) = previous
                    && start < previous
                {
                    return Err(corpus_error("transcript timestamps must be monotonic"));
                }
                previous = Some(start);
            }
        }

        Ok(())
    }

    pub fn render_body(&self) -> Result<String> {
        self.validate()?;
        let mut body = String::new();

        for (index, segment) in self.segments.iter().enumerate() {
            if index > 0 {
                body.push('\n');
            }
            if let Some(start) = segment.start_seconds {
                body.push_str(&format_timestamp(start));
                body.push(' ');
            }
            if let Some(speaker) = segment.speaker.as_ref() {
                let label = speaker
                    .identity
                    .as_deref()
                    .unwrap_or(&speaker.diarization_label);
                body.push_str("<speaker:");
                body.push_str(label);
                body.push_str("> ");
            }
            body.push_str(segment.text.trim_end_matches(['\r', '\n']));
            body.push('\n');
        }

        Ok(body)
    }

    pub fn to_sourcearium_representation(&self) -> Result<TextRepresentationV1> {
        self.validate()?;
        Ok(TextRepresentationV1 {
            derivation: self.derivation.as_str().into(),
            language: self.language.clone(),
            timestamps: Some(self.timestamps),
            engine: self.engine.clone(),
            model: self.model.clone(),
        })
    }
}

fn parse_diarization_extension(table: &toml::Table) -> Result<DiarizationProvenance> {
    let engine = table
        .get("engine")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| corpus_error("diarization extension is missing engine"))?;
    let model = table
        .get("model")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| corpus_error("diarization extension is missing model"))?;
    let registry_revision = table
        .get("registry_revision")
        .and_then(toml::Value::as_integer)
        .map(|value| {
            u64::try_from(value)
                .map_err(|_| corpus_error("diarization registry_revision must be non-negative"))
        })
        .transpose()?;
    Ok(DiarizationProvenance {
        engine: engine.to_owned(),
        model: model.to_owned(),
        registry_revision,
    })
}

fn parse_sourcearium_transcript_body(
    body: &str,
    timestamps: bool,
) -> Result<Vec<TranscriptSegment>> {
    let mut segments = Vec::new();
    for raw_segment in body.split("\n\n") {
        let raw_segment = raw_segment.trim_end_matches('\n');
        if raw_segment.trim().is_empty() {
            continue;
        }

        let (start_seconds, rest) = if timestamps {
            let tag = raw_segment.get(..10).ok_or_else(|| {
                corpus_error("timestamped transcript segment is shorter than [HH:MM:SS]")
            })?;
            let start = parse_rendered_timestamp(tag).ok_or_else(|| {
                corpus_error(format!("invalid transcript timestamp prefix {tag:?}"))
            })?;
            let rest = raw_segment
                .get(10..)
                .and_then(|rest| rest.strip_prefix(' '))
                .ok_or_else(|| {
                    corpus_error("timestamped transcript segment must have a space after timestamp")
                })?;
            (Some(start), rest)
        } else {
            (None, raw_segment)
        };

        let (speaker, text) = if let Some(rest) = rest.strip_prefix("<speaker:") {
            let end = rest.find("> ").ok_or_else(|| {
                corpus_error("speaker-tagged transcript segment is missing closing '> '")
            })?;
            let label = &rest[..end];
            if label.trim().is_empty() {
                return Err(corpus_error("speaker tag must not be empty"));
            }
            (
                Some(TranscriptSpeaker {
                    diarization_label: label.to_owned(),
                    identity: None,
                    attribution: SpeakerAttribution::Unresolved,
                }),
                &rest[end + 2..],
            )
        } else {
            (None, rest)
        };

        if text.is_empty() {
            return Err(corpus_error("transcript segment text must not be empty"));
        }
        segments.push(TranscriptSegment {
            start_seconds,
            text: text.to_owned(),
            speaker,
        });
    }
    if segments.is_empty() {
        return Err(corpus_error("transcript contains no segments"));
    }
    Ok(segments)
}

fn parse_rendered_timestamp(value: &str) -> Option<u64> {
    if value.len() != 10 || !value.starts_with('[') || !value.ends_with(']') {
        return None;
    }
    let inner = &value[1..9];
    let mut parts = inner.split(':');
    let hours = parts.next()?.parse::<u64>().ok()?;
    let minutes = parts.next()?.parse::<u64>().ok()?;
    let seconds = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() || minutes >= 60 || seconds >= 60 {
        return None;
    }
    Some(hours * 3_600 + minutes * 60 + seconds)
}

#[async_trait]
pub trait TranscriptProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn derivation(&self) -> TranscriptDerivation;

    async fn acquire(&self, request: &TranscriptRequest) -> Result<Option<TranscriptCandidate>>;
}

fn corpus_error(message: impl Into<String>) -> VesselError {
    VesselError::Corpus(message.into())
}

fn require_nonempty(field: &str, value: Option<&str>) -> Result<()> {
    match value {
        Some(value) if !value.is_empty() => Ok(()),
        _ => Err(corpus_error(format!("{field} is required"))),
    }
}

fn validate_optional_nonempty(field: &str, value: Option<&str>) -> Result<()> {
    if value.is_some_and(|value| value.trim().is_empty()) {
        return Err(corpus_error(format!(
            "{field} must not be empty when present"
        )));
    }
    Ok(())
}

fn format_timestamp(total_seconds: u64) -> String {
    let hours = total_seconds / 3_600;
    let minutes = (total_seconds % 3_600) / 60;
    let seconds = total_seconds % 60;
    format!("[{hours:02}:{minutes:02}:{seconds:02}]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconstructs_materialized_local_asr_candidate() {
        let mut diarization = toml::Table::new();
        diarization.insert("engine".into(), toml::Value::String("sherpa-onnx".into()));
        diarization.insert(
            "model".into(),
            toml::Value::String("segmentation=x;embedding=y".into()),
        );
        let artifact = SourceariumArtifactV1 {
            schema: 1,
            artifact_id: "youtube:video:abc:transcript".into(),
            kind: "transcript".into(),
            title: Some("Example".into()),
            source: crate::SourceIdentityV1 {
                family: "youtube".into(),
                kind: "video".into(),
                id: "abc".into(),
                url: None,
                creator: None,
                creator_id: None,
                published: None,
            },
            representation: TextRepresentationV1 {
                derivation: "local_asr".into(),
                language: Some("en".into()),
                timestamps: Some(true),
                engine: Some("whisper-candle".into()),
                model: Some("small".into()),
            },
            acquisition: crate::AcquisitionV1 {
                producer: "vessel".into(),
                producer_version: None,
                acquired_at: None,
                method: Some("local_asr".into()),
            },
            extensions: std::collections::BTreeMap::from([("diarization".into(), diarization)]),
        };
        let body = "[00:00:03] <speaker:SPEAKER_00> Hello.\n\n[00:01:05] World.\n";
        let candidate =
            TranscriptCandidate::from_sourcearium_artifact(&artifact, body).expect("candidate");
        assert_eq!(candidate.derivation, TranscriptDerivation::LocalAsr);
        assert_eq!(candidate.segments.len(), 2);
        assert_eq!(candidate.segments[0].start_seconds, Some(3));
        assert_eq!(
            candidate.segments[0]
                .speaker
                .as_ref()
                .expect("speaker")
                .diarization_label,
            "SPEAKER_00"
        );
        assert_eq!(candidate.render_body().expect("body"), body);
    }

    #[test]
    fn renders_timed_transcript_deterministically() {
        let candidate = TranscriptCandidate {
            derivation: TranscriptDerivation::CreatorSubtitles,
            language: Some("en".into()),
            timestamps: true,
            engine: None,
            model: None,
            diarization: None,
            segments: vec![
                TranscriptSegment {
                    start_seconds: Some(3),
                    text: "Hello.".into(),
                    speaker: None,
                },
                TranscriptSegment {
                    start_seconds: Some(65),
                    text: "World.".into(),
                    speaker: None,
                },
            ],
        };

        assert_eq!(
            candidate.render_body().expect("body"),
            "[00:00:03] Hello.\n\n[00:01:05] World.\n"
        );
    }

    #[test]
    fn renders_anonymous_and_identified_speakers_without_conflating_them() {
        let candidate = TranscriptCandidate {
            derivation: TranscriptDerivation::LocalAsr,
            language: Some("en".into()),
            timestamps: true,
            engine: Some("whisperx".into()),
            model: Some("large-v3".into()),
            diarization: Some(DiarizationProvenance {
                engine: "pyannote".into(),
                model: "speaker-diarization-community-1".into(),
                registry_revision: None,
            }),
            segments: vec![
                TranscriptSegment {
                    start_seconds: Some(3),
                    text: "Anonymous clip.".into(),
                    speaker: Some(TranscriptSpeaker {
                        diarization_label: "SPEAKER_00".into(),
                        identity: None,
                        attribution: SpeakerAttribution::Unresolved,
                    }),
                },
                TranscriptSegment {
                    start_seconds: Some(8),
                    text: "Known creator.".into(),
                    speaker: Some(TranscriptSpeaker {
                        diarization_label: "SPEAKER_01".into(),
                        identity: Some("creator".into()),
                        attribution: SpeakerAttribution::HumanConfirmed,
                    }),
                },
            ],
        };

        assert_eq!(
            candidate.render_body().expect("body"),
            "[00:00:03] <speaker:SPEAKER_00> Anonymous clip.\n\n[00:00:08] <speaker:creator> Known creator.\n"
        );
    }

    #[test]
    fn rejects_non_monotonic_timestamps() {
        let candidate = TranscriptCandidate {
            derivation: TranscriptDerivation::PlatformAutoCaption,
            language: Some("en".into()),
            timestamps: true,
            engine: None,
            model: None,
            diarization: None,
            segments: vec![
                TranscriptSegment {
                    start_seconds: Some(10),
                    text: "Later".into(),
                    speaker: None,
                },
                TranscriptSegment {
                    start_seconds: Some(9),
                    text: "Earlier".into(),
                    speaker: None,
                },
            ],
        };
        assert!(candidate.validate().is_err());
    }

    #[test]
    fn quality_order_matches_sourcearium_policy() {
        assert!(
            TranscriptDerivation::CreatorSubtitles.quality_rank()
                > TranscriptDerivation::PlatformAutoCaption.quality_rank()
        );
        assert!(
            TranscriptDerivation::PlatformAutoCaption.quality_rank()
                > TranscriptDerivation::LocalAsr.quality_rank()
        );
    }
}
