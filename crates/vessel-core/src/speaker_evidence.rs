use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{Result, VesselError};

pub const SPEAKER_EVIDENCE_SCHEMA_V1: u8 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeakerEvidenceV1 {
    pub schema: u8,
    pub video_id: String,
    pub asr: SpeakerEvidenceProvenance,
    pub diarization: SpeakerEvidenceDiarization,
    #[serde(default)]
    pub segments: Vec<SpeakerEvidenceSegment>,
    #[serde(default)]
    pub speaker_embeddings: Option<BTreeMap<String, Vec<f64>>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeakerEvidenceProvenance {
    pub engine: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeakerEvidenceDiarization {
    pub engine: String,
    pub model: String,
    pub label_scope: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeakerEvidenceSegment {
    pub start: f64,
    pub end: f64,
    pub speaker: String,
}

impl SpeakerEvidenceV1 {
    pub fn validate(&self) -> Result<()> {
        if self.schema != SPEAKER_EVIDENCE_SCHEMA_V1 {
            return Err(evidence_error(format!(
                "unsupported speaker evidence schema {}; expected {}",
                self.schema, SPEAKER_EVIDENCE_SCHEMA_V1
            )));
        }
        require_nonempty("speaker evidence video_id", &self.video_id)?;
        require_nonempty("speaker evidence ASR engine", &self.asr.engine)?;
        require_nonempty("speaker evidence ASR model", &self.asr.model)?;
        require_nonempty(
            "speaker evidence diarization engine",
            &self.diarization.engine,
        )?;
        require_nonempty(
            "speaker evidence diarization model",
            &self.diarization.model,
        )?;
        if self.diarization.label_scope != "file_local" {
            return Err(evidence_error(format!(
                "speaker evidence label_scope must be \"file_local\", got {:?}",
                self.diarization.label_scope
            )));
        }

        for segment in &self.segments {
            if !segment.start.is_finite()
                || !segment.end.is_finite()
                || segment.start < 0.0
                || segment.end <= segment.start
            {
                return Err(evidence_error(format!(
                    "invalid speaker evidence segment {}-{}",
                    segment.start, segment.end
                )));
            }
            require_nonempty("speaker evidence segment label", &segment.speaker)?;
        }

        if let Some(embeddings) = self.speaker_embeddings.as_ref() {
            let mut dimension = None;
            for (label, embedding) in embeddings {
                require_nonempty("speaker embedding label", label)?;
                if embedding.is_empty() {
                    return Err(evidence_error(format!(
                        "speaker embedding {label:?} must not be empty"
                    )));
                }
                if embedding.iter().any(|value| !value.is_finite()) {
                    return Err(evidence_error(format!(
                        "speaker embedding {label:?} contains non-finite values"
                    )));
                }
                let norm = embedding
                    .iter()
                    .map(|value| value * value)
                    .sum::<f64>()
                    .sqrt();
                if norm <= f64::EPSILON {
                    return Err(evidence_error(format!(
                        "speaker embedding {label:?} has zero norm"
                    )));
                }
                if let Some(expected) = dimension {
                    if embedding.len() != expected {
                        return Err(evidence_error(format!(
                            "speaker embedding dimension mismatch: {label:?} has {}, expected {expected}",
                            embedding.len()
                        )));
                    }
                } else {
                    dimension = Some(embedding.len());
                }
            }
        }

        Ok(())
    }

    pub fn embedding_dimension(&self) -> Option<usize> {
        self.speaker_embeddings
            .as_ref()
            .and_then(|embeddings| embeddings.values().next())
            .map(Vec::len)
    }
}

pub fn load_speaker_evidence(path: &Path) -> Result<SpeakerEvidenceV1> {
    let raw = fs::read_to_string(path)?;
    let evidence: SpeakerEvidenceV1 = serde_json::from_str(&raw).map_err(|error| {
        evidence_error(format!(
            "{}: invalid speaker evidence JSON: {error}",
            path.display()
        ))
    })?;
    evidence.validate()?;
    Ok(evidence)
}

fn require_nonempty(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(evidence_error(format!("{field} is required")));
    }
    Ok(())
}

fn evidence_error(message: impl Into<String>) -> VesselError {
    VesselError::Corpus(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SpeakerEvidenceV1 {
        SpeakerEvidenceV1 {
            schema: SPEAKER_EVIDENCE_SCHEMA_V1,
            video_id: "video123".into(),
            asr: SpeakerEvidenceProvenance {
                engine: "whisperx-faster-whisper".into(),
                model: "large-v3".into(),
            },
            diarization: SpeakerEvidenceDiarization {
                engine: "pyannote-audio".into(),
                model: "pyannote/speaker-diarization-community-1".into(),
                label_scope: "file_local".into(),
            },
            segments: vec![SpeakerEvidenceSegment {
                start: 0.0,
                end: 2.5,
                speaker: "SPEAKER_00".into(),
            }],
            speaker_embeddings: None,
        }
    }

    #[test]
    fn anonymous_evidence_validates_without_identity_data() {
        sample().validate().expect("valid evidence");
    }

    #[test]
    fn invalid_label_scope_is_rejected() {
        let mut evidence = sample();
        evidence.diarization.label_scope = "global".into();
        assert!(evidence.validate().is_err());
    }

    #[test]
    fn embedding_dimensions_must_match() {
        let mut evidence = sample();
        evidence.speaker_embeddings = Some(BTreeMap::from([
            ("SPEAKER_00".into(), vec![1.0, 0.0]),
            ("SPEAKER_01".into(), vec![1.0, 0.0, 0.0]),
        ]));
        assert!(evidence.validate().is_err());
    }
}
