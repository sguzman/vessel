use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{Result, TextRepresentationV1, VesselError, YoutubeTranscriptPolicyV1};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptSegment {
    pub start_seconds: Option<u64>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptCandidate {
    pub derivation: TranscriptDerivation,
    pub language: Option<String>,
    pub timestamps: bool,
    pub engine: Option<String>,
    pub model: Option<String>,
    pub segments: Vec<TranscriptSegment>,
}

impl TranscriptCandidate {
    pub fn validate(&self) -> Result<()> {
        if self.segments.is_empty() {
            return Err(corpus_error("transcript contains no segments"));
        }

        if self.derivation == TranscriptDerivation::LocalAsr {
            require_nonempty("local ASR engine", self.engine.as_deref())?;
            require_nonempty("local ASR model", self.model.as_deref())?;
        }

        let mut previous = None;
        for segment in &self.segments {
            if segment.text.is_empty() {
                return Err(corpus_error("transcript segment text must not be empty"));
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
    fn renders_timed_transcript_deterministically() {
        let candidate = TranscriptCandidate {
            derivation: TranscriptDerivation::CreatorSubtitles,
            language: Some("en".into()),
            timestamps: true,
            engine: None,
            model: None,
            segments: vec![
                TranscriptSegment {
                    start_seconds: Some(3),
                    text: "Hello.".into(),
                },
                TranscriptSegment {
                    start_seconds: Some(65),
                    text: "World.".into(),
                },
            ],
        };

        assert_eq!(
            candidate.render_body().expect("body"),
            "[00:00:03] Hello.\n\n[00:01:05] World.\n"
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
            segments: vec![
                TranscriptSegment {
                    start_seconds: Some(10),
                    text: "Later".into(),
                },
                TranscriptSegment {
                    start_seconds: Some(9),
                    text: "Earlier".into(),
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
