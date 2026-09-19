use std::path::Path;

use vessel_core::{
    Result, TranscriptCandidate, TranscriptDerivation, TranscriptSegment, VesselError,
};
use whisper_core::{TranscribeOptions, WhisperModel, device, load_model, transcribe_file};

pub const ENGINE_NAME: &str = "whisper-candle";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsrConfig {
    pub model: String,
    pub device: String,
    pub language: Option<String>,
    pub word_timestamps: bool,
}

impl Default for AsrConfig {
    fn default() -> Self {
        Self {
            model: "small".into(),
            device: "cpu".into(),
            language: None,
            word_timestamps: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct AsrSegment {
    start_seconds: f64,
    text: String,
}

pub struct WhisperCandleBackend {
    config: AsrConfig,
    model: WhisperModel,
}

impl WhisperCandleBackend {
    pub fn load(config: AsrConfig) -> Result<Self> {
        let device = device(&config.device).map_err(|error| {
            asr_error(format!(
                "failed to initialize ASR device {:?}: {error}",
                config.device
            ))
        })?;
        let model = load_model(&config.model, &device).map_err(|error| {
            asr_error(format!(
                "failed to load Whisper model {:?}: {error}",
                config.model
            ))
        })?;
        Ok(Self { config, model })
    }

    pub fn config(&self) -> &AsrConfig {
        &self.config
    }

    pub fn transcribe_path(&mut self, path: &Path) -> Result<TranscriptCandidate> {
        if !path.is_file() {
            return Err(asr_error(format!(
                "ASR input does not exist or is not a file: {}",
                path.display()
            )));
        }

        let mut options = TranscribeOptions::default();
        options.word_timestamps = self.config.word_timestamps;
        options.decode_options.language = self.config.language.clone();
        options.decode_options.without_timestamps = false;
        options.verbose = None;

        let result = transcribe_file(&mut self.model, path, &options).map_err(|error| {
            asr_error(format!(
                "Whisper transcription failed for {}: {error}",
                path.display()
            ))
        })?;

        let segments = result
            .segments
            .into_iter()
            .map(|segment| AsrSegment {
                start_seconds: segment.start,
                text: segment.text,
            })
            .collect::<Vec<_>>();

        candidate_from_segments(&self.config.model, Some(result.language), segments)
    }
}

fn candidate_from_segments(
    model: &str,
    language: Option<String>,
    segments: Vec<AsrSegment>,
) -> Result<TranscriptCandidate> {
    let mut normalized = Vec::new();
    for segment in segments {
        let text = normalize_text(&segment.text);
        if text.is_empty() {
            continue;
        }
        if !segment.start_seconds.is_finite() || segment.start_seconds < 0.0 {
            return Err(asr_error(format!(
                "Whisper emitted invalid segment timestamp {}",
                segment.start_seconds
            )));
        }
        normalized.push(TranscriptSegment {
            start_seconds: Some(segment.start_seconds.floor() as u64),
            text,
        });
    }

    let candidate = TranscriptCandidate {
        derivation: TranscriptDerivation::LocalAsr,
        language: language.filter(|language| !language.trim().is_empty()),
        timestamps: true,
        engine: Some(ENGINE_NAME.into()),
        model: Some(model.to_owned()),
        segments: normalized,
    };
    candidate.validate()?;
    Ok(candidate)
}

fn normalize_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn asr_error(message: impl Into<String>) -> VesselError {
    VesselError::Extractor(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loaded_backend_is_safe_to_move_to_blocking_worker() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<WhisperCandleBackend>();
    }

    #[test]
    fn defaults_are_cpu_first_and_multilingual() {
        let config = AsrConfig::default();
        assert_eq!(config.device, "cpu");
        assert_eq!(config.model, "small");
        assert_eq!(config.language, None);
        assert!(!config.word_timestamps);
    }

    #[test]
    fn whisper_segments_become_sourcearium_ready_candidate() {
        let candidate = candidate_from_segments(
            "small",
            Some("en".into()),
            vec![
                AsrSegment {
                    start_seconds: 3.9,
                    text: "  hello   world  ".into(),
                },
                AsrSegment {
                    start_seconds: 8.1,
                    text: "second segment".into(),
                },
            ],
        )
        .expect("candidate");

        assert_eq!(candidate.derivation, TranscriptDerivation::LocalAsr);
        assert_eq!(candidate.engine.as_deref(), Some(ENGINE_NAME));
        assert_eq!(candidate.model.as_deref(), Some("small"));
        assert_eq!(candidate.language.as_deref(), Some("en"));
        assert_eq!(candidate.segments[0].start_seconds, Some(3));
        assert_eq!(candidate.segments[0].text, "hello world");
    }

    #[test]
    fn invalid_timestamps_are_rejected() {
        let error = candidate_from_segments(
            "small",
            None,
            vec![AsrSegment {
                start_seconds: -1.0,
                text: "bad".into(),
            }],
        )
        .expect_err("negative timestamp must fail");
        assert!(error.to_string().contains("invalid segment timestamp"));
    }
}
