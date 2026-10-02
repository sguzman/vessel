use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use sherpa_onnx::{
    FastClusteringConfig, OfflineSpeakerDiarization, OfflineSpeakerDiarizationConfig,
    OfflineSpeakerSegmentationModelConfig, OfflineSpeakerSegmentationPyannoteModelConfig,
    SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig, Wave,
};
use vessel_core::{
    DiarizationProvenance, Result, SpeakerAttribution, SpeakerEvidenceDiarization,
    SpeakerEvidenceProvenance, SpeakerEvidenceSegment, SpeakerEvidenceV1, TranscriptCandidate,
    TranscriptSpeaker, VesselError,
};

pub const SHERPA_ONNX_BACKEND_NAME: &str = "sherpa-onnx";
pub const SHERPA_ONNX_ENGINE_NAME: &str = "sherpa-onnx";
pub const DEFAULT_CLUSTERING_THRESHOLD: f32 = 0.5;
pub const DEFAULT_WINDOW_SHIFT_RATIO: f32 = 0.1;

#[derive(Debug, Clone, PartialEq)]
pub struct DiarizationConfig {
    pub backend: String,
    pub segmentation_model: PathBuf,
    pub embedding_model: PathBuf,
    pub provider: String,
    pub num_threads: i32,
    pub num_speakers: Option<usize>,
    pub clustering_threshold: f32,
    pub window_shift_ratio: f32,
    pub min_duration_on: f32,
    pub min_duration_off: f32,
    pub speaker_embeddings: bool,
}

impl DiarizationConfig {
    pub fn validate(&self) -> Result<()> {
        if self.backend != SHERPA_ONNX_BACKEND_NAME {
            return Err(diarization_error(format!(
                "unsupported diarization backend {:?}",
                self.backend
            )));
        }
        require_file("segmentation model", &self.segmentation_model)?;
        require_file("speaker embedding model", &self.embedding_model)?;
        if self.provider.trim().is_empty() {
            return Err(diarization_error("diarization provider must not be empty"));
        }
        if self.num_threads < 1 {
            return Err(diarization_error(
                "diarization num_threads must be at least 1",
            ));
        }
        if self.num_speakers == Some(0) {
            return Err(diarization_error(
                "diarization num_speakers must be at least 1 when specified",
            ));
        }
        if !(0.0..=1.0).contains(&self.clustering_threshold) {
            return Err(diarization_error(
                "diarization clustering threshold must be between 0 and 1",
            ));
        }
        if !(0.0..=1.0).contains(&self.window_shift_ratio)
            || self.window_shift_ratio <= f32::EPSILON
        {
            return Err(diarization_error(
                "diarization window_shift_ratio must be greater than 0 and at most 1",
            ));
        }
        if self.min_duration_on < 0.0 || self.min_duration_off < 0.0 {
            return Err(diarization_error(
                "diarization minimum durations must be non-negative",
            ));
        }
        Ok(())
    }

    pub fn model_provenance(&self) -> String {
        format!(
            "segmentation={};embedding={}",
            portable_model_id(&self.segmentation_model),
            portable_model_id(&self.embedding_model)
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiarizationSegment {
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub speaker: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiarizationResult {
    pub engine: String,
    pub model: String,
    pub segments: Vec<DiarizationSegment>,
    pub speaker_embeddings: BTreeMap<String, Vec<f64>>,
}

impl DiarizationResult {
    pub fn validate(&self) -> Result<()> {
        if self.engine.trim().is_empty() {
            return Err(diarization_error("diarization engine must not be empty"));
        }
        if self.model.trim().is_empty() {
            return Err(diarization_error("diarization model must not be empty"));
        }
        let mut dimension = None;
        for segment in &self.segments {
            if !segment.start_seconds.is_finite()
                || !segment.end_seconds.is_finite()
                || segment.start_seconds < 0.0
                || segment.end_seconds <= segment.start_seconds
                || segment.speaker.trim().is_empty()
            {
                return Err(diarization_error("invalid diarization segment"));
            }
        }
        for (speaker, embedding) in &self.speaker_embeddings {
            if speaker.trim().is_empty() || embedding.is_empty() {
                return Err(diarization_error("invalid speaker embedding"));
            }
            if embedding.iter().any(|value| !value.is_finite()) {
                return Err(diarization_error(format!(
                    "speaker embedding {speaker:?} contains non-finite values"
                )));
            }
            let norm = embedding.iter().map(|value| value * value).sum::<f64>().sqrt();
            if norm <= f64::EPSILON {
                return Err(diarization_error(format!(
                    "speaker embedding {speaker:?} has zero norm"
                )));
            }
            if let Some(expected) = dimension {
                if embedding.len() != expected {
                    return Err(diarization_error(format!(
                        "speaker embedding {speaker:?} has dimension {}, expected {expected}",
                        embedding.len()
                    )));
                }
            } else {
                dimension = Some(embedding.len());
            }
        }
        Ok(())
    }

    pub fn apply_to_candidate(&self, candidate: &mut TranscriptCandidate) -> Result<()> {
        self.validate()?;
        if let Some(existing) = candidate.diarization.as_ref()
            && (existing.engine != self.engine || existing.model != self.model)
        {
            return Err(diarization_error(format!(
                "transcript candidate already has incompatible diarization {}/{}",
                existing.engine, existing.model
            )));
        }

        candidate.diarization = Some(DiarizationProvenance {
            engine: self.engine.clone(),
            model: self.model.clone(),
            registry_revision: None,
        });

        for segment in &mut candidate.segments {
            let Some(start_seconds) = segment.start_seconds else {
                segment.speaker = None;
                continue;
            };
            let time = start_seconds as f64;
            let speakers = self
                .segments
                .iter()
                .filter(|item| item.start_seconds <= time && time < item.end_seconds)
                .map(|item| item.speaker.as_str())
                .collect::<BTreeSet<_>>();
            segment.speaker = if speakers.len() == 1 {
                Some(TranscriptSpeaker {
                    diarization_label: (*speakers.iter().next().expect("one speaker")).to_owned(),
                    identity: None,
                    attribution: SpeakerAttribution::Unresolved,
                })
            } else {
                None
            };
        }

        candidate.validate()
    }

    pub fn to_speaker_evidence(
        &self,
        video_id: &str,
        candidate: &TranscriptCandidate,
    ) -> Result<SpeakerEvidenceV1> {
        self.validate()?;
        let asr_engine = candidate.engine.as_deref().ok_or_else(|| {
            diarization_error("speaker evidence requires transcript ASR engine provenance")
        })?;
        let asr_model = candidate.model.as_deref().ok_or_else(|| {
            diarization_error("speaker evidence requires transcript ASR model provenance")
        })?;
        let evidence = SpeakerEvidenceV1 {
            schema: 1,
            video_id: video_id.to_owned(),
            asr: SpeakerEvidenceProvenance {
                engine: asr_engine.to_owned(),
                model: asr_model.to_owned(),
            },
            diarization: SpeakerEvidenceDiarization {
                engine: self.engine.clone(),
                model: self.model.clone(),
                label_scope: "file_local".into(),
            },
            segments: self
                .segments
                .iter()
                .map(|segment| SpeakerEvidenceSegment {
                    start: segment.start_seconds,
                    end: segment.end_seconds,
                    speaker: segment.speaker.clone(),
                })
                .collect(),
            speaker_embeddings: (!self.speaker_embeddings.is_empty())
                .then(|| self.speaker_embeddings.clone()),
        };
        evidence.validate()?;
        Ok(evidence)
    }
}

pub struct SherpaOnnxDiarizer {
    config: DiarizationConfig,
    diarizer: OfflineSpeakerDiarization,
    embedding_extractor: Option<SpeakerEmbeddingExtractor>,
}

impl SherpaOnnxDiarizer {
    pub fn load(config: DiarizationConfig) -> Result<Self> {
        config.validate()?;
        let embedding_config = SpeakerEmbeddingExtractorConfig {
            model: Some(path_string(&config.embedding_model)?),
            num_threads: config.num_threads,
            debug: false,
            provider: Some(config.provider.clone()),
        };
        let diarizer_config = OfflineSpeakerDiarizationConfig {
            segmentation: OfflineSpeakerSegmentationModelConfig {
                pyannote: OfflineSpeakerSegmentationPyannoteModelConfig {
                    model: Some(path_string(&config.segmentation_model)?),
                    window_shift_ratio: config.window_shift_ratio,
                },
                num_threads: config.num_threads,
                debug: false,
                provider: Some(config.provider.clone()),
            },
            embedding: embedding_config.clone(),
            clustering: FastClusteringConfig {
                num_clusters: config
                    .num_speakers
                    .map(|value| value as i32)
                    .unwrap_or(-1),
                threshold: config.clustering_threshold,
            },
            min_duration_on: config.min_duration_on,
            min_duration_off: config.min_duration_off,
        };

        eprintln!(
            "[diarization] model load started engine={} segmentation={} embedding={} provider={}",
            SHERPA_ONNX_ENGINE_NAME,
            config.segmentation_model.display(),
            config.embedding_model.display(),
            config.provider,
        );
        let diarizer = OfflineSpeakerDiarization::create(&diarizer_config).ok_or_else(|| {
            diarization_error("failed to initialize sherpa-onnx offline speaker diarization")
        })?;
        let embedding_extractor = if config.speaker_embeddings {
            Some(SpeakerEmbeddingExtractor::create(&embedding_config).ok_or_else(|| {
                diarization_error("failed to initialize sherpa-onnx speaker embedding extractor")
            })?)
        } else {
            None
        };
        eprintln!(
            "[diarization] model load completed engine={} sample_rate={}",
            SHERPA_ONNX_ENGINE_NAME,
            diarizer.sample_rate(),
        );

        Ok(Self {
            config,
            diarizer,
            embedding_extractor,
        })
    }

    pub fn process_path(&self, path: &Path) -> Result<DiarizationResult> {
        if !path.is_file() {
            return Err(diarization_error(format!(
                "diarization input does not exist: {}",
                path.display()
            )));
        }
        let filename = path_string(path)?;
        let wave = Wave::read(&filename).ok_or_else(|| {
            diarization_error(format!("failed to read diarization WAV {}", path.display()))
        })?;
        if wave.sample_rate() != self.diarizer.sample_rate() {
            return Err(diarization_error(format!(
                "diarization WAV sample rate {} does not match sherpa-onnx expected {}",
                wave.sample_rate(),
                self.diarizer.sample_rate()
            )));
        }

        eprintln!(
            "[diarization] inference started engine={} input={} samples={}",
            SHERPA_ONNX_ENGINE_NAME,
            path.display(),
            wave.num_samples(),
        );
        let raw = self
            .diarizer
            .process(wave.samples())
            .ok_or_else(|| diarization_error("sherpa-onnx diarization returned no result"))?;
        let raw_segments = raw.sort_by_start_time();
        let segments = raw_segments
            .iter()
            .filter(|segment| {
                segment.speaker >= 0
                    && segment.start.is_finite()
                    && segment.end.is_finite()
                    && segment.start >= 0.0
                    && segment.end > segment.start
            })
            .map(|segment| DiarizationSegment {
                start_seconds: segment.start as f64,
                end_seconds: segment.end as f64,
                speaker: speaker_label(segment.speaker),
            })
            .collect::<Vec<_>>();

        let mut speaker_embeddings = BTreeMap::new();
        if let Some(extractor) = self.embedding_extractor.as_ref() {
            for speaker in 0..raw.num_speakers() {
                let label = speaker_label(speaker);
                let samples = concatenate_speaker_audio(
                    wave.samples(),
                    wave.sample_rate(),
                    &raw_segments,
                    speaker,
                );
                if samples.is_empty() {
                    continue;
                }
                let Some(stream) = extractor.create_stream() else {
                    continue;
                };
                stream.accept_waveform(wave.sample_rate(), &samples);
                stream.input_finished();
                if !extractor.is_ready(&stream) {
                    continue;
                }
                if let Some(embedding) = extractor.compute(&stream) {
                    speaker_embeddings.insert(
                        label,
                        embedding.into_iter().map(f64::from).collect(),
                    );
                }
            }
        }

        let result = DiarizationResult {
            engine: SHERPA_ONNX_ENGINE_NAME.into(),
            model: self.config.model_provenance(),
            segments,
            speaker_embeddings,
        };
        result.validate()?;
        eprintln!(
            "[diarization] inference completed engine={} speakers={} segments={} embeddings={}",
            SHERPA_ONNX_ENGINE_NAME,
            raw.num_speakers(),
            result.segments.len(),
            result.speaker_embeddings.len(),
        );
        Ok(result)
    }
}

pub fn persist_speaker_evidence(
    evidence_dir: &Path,
    evidence: &SpeakerEvidenceV1,
) -> Result<PathBuf> {
    evidence.validate()?;
    fs::create_dir_all(evidence_dir)?;
    let path = evidence_dir.join(format!("{}.json", evidence.video_id));
    let temp = evidence_dir.join(format!("{}.json.tmp", evidence.video_id));
    let rendered = serde_json::to_vec_pretty(evidence)
        .map_err(|error| diarization_error(format!("speaker evidence serialization failed: {error}")))?;
    fs::write(&temp, rendered)?;
    fs::rename(&temp, &path)?;
    Ok(path)
}

fn concatenate_speaker_audio(
    samples: &[f32],
    sample_rate: i32,
    segments: &[sherpa_onnx::OfflineSpeakerDiarizationSegment],
    speaker: i32,
) -> Vec<f32> {
    if sample_rate <= 0 {
        return Vec::new();
    }
    let sample_rate = sample_rate as f64;
    let mut output = Vec::new();
    for segment in segments.iter().filter(|segment| segment.speaker == speaker) {
        let start = ((segment.start as f64 * sample_rate).floor() as usize).min(samples.len());
        let end = ((segment.end as f64 * sample_rate).ceil() as usize).min(samples.len());
        if start < end {
            output.extend_from_slice(&samples[start..end]);
        }
    }
    output
}

fn speaker_label(speaker: i32) -> String {
    format!("SPEAKER_{speaker:02}")
}

fn portable_model_id(path: &Path) -> String {
    let file = path
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let parent = path
        .parent()
        .and_then(Path::file_name)
        .map(|value| value.to_string_lossy().into_owned());
    parent
        .map(|parent| format!("{parent}/{file}"))
        .unwrap_or(file)
}

fn path_string(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| diarization_error(format!("path is not valid UTF-8: {}", path.display())))
}

fn require_file(label: &str, path: &Path) -> Result<()> {
    if !path.is_file() {
        return Err(diarization_error(format!(
            "{label} does not exist or is not a file: {}",
            path.display()
        )));
    }
    Ok(())
}

fn diarization_error(message: impl Into<String>) -> VesselError {
    VesselError::Extractor(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vessel_core::{
        TranscriptDerivation, TranscriptSegment,
    };

    fn synthetic_result() -> DiarizationResult {
        DiarizationResult {
            engine: SHERPA_ONNX_ENGINE_NAME.into(),
            model: "segmentation=seg/model.onnx;embedding=emb/model.onnx".into(),
            segments: vec![
                DiarizationSegment {
                    start_seconds: 0.0,
                    end_seconds: 5.0,
                    speaker: "SPEAKER_00".into(),
                },
                DiarizationSegment {
                    start_seconds: 5.0,
                    end_seconds: 10.0,
                    speaker: "SPEAKER_01".into(),
                },
            ],
            speaker_embeddings: BTreeMap::from([
                ("SPEAKER_00".into(), vec![1.0, 0.0]),
                ("SPEAKER_01".into(), vec![0.0, 1.0]),
            ]),
        }
    }

    fn candidate() -> TranscriptCandidate {
        TranscriptCandidate {
            derivation: TranscriptDerivation::LocalAsr,
            language: Some("en".into()),
            timestamps: true,
            engine: Some("phonon-2".into()),
            model: Some("phonon-2".into()),
            diarization: None,
            segments: vec![
                TranscriptSegment {
                    start_seconds: Some(1),
                    text: "First.".into(),
                    speaker: None,
                },
                TranscriptSegment {
                    start_seconds: Some(7),
                    text: "Second.".into(),
                    speaker: None,
                },
            ],
        }
    }

    #[test]
    fn applies_file_local_speakers_without_inventing_identity() {
        let result = synthetic_result();
        let mut candidate = candidate();
        result.apply_to_candidate(&mut candidate).expect("apply");
        assert_eq!(
            candidate.segments[0]
                .speaker
                .as_ref()
                .map(|speaker| speaker.diarization_label.as_str()),
            Some("SPEAKER_00")
        );
        assert_eq!(
            candidate.segments[1]
                .speaker
                .as_ref()
                .map(|speaker| speaker.diarization_label.as_str()),
            Some("SPEAKER_01")
        );
        assert_eq!(
            candidate.segments[0]
                .speaker
                .as_ref()
                .and_then(|speaker| speaker.identity.as_deref()),
            None
        );
        assert_eq!(
            candidate.segments[0]
                .speaker
                .as_ref()
                .map(|speaker| speaker.attribution),
            Some(SpeakerAttribution::Unresolved)
        );
    }

    #[test]
    fn overlapping_speakers_remain_unlabeled_in_text_projection() {
        let mut result = synthetic_result();
        result.segments.push(DiarizationSegment {
            start_seconds: 0.5,
            end_seconds: 2.0,
            speaker: "SPEAKER_01".into(),
        });
        let mut candidate = candidate();
        result.apply_to_candidate(&mut candidate).expect("apply");
        assert!(candidate.segments[0].speaker.is_none());
    }

    #[test]
    fn evidence_preserves_asr_and_diarization_provenance() {
        let result = synthetic_result();
        let candidate = candidate();
        let evidence = result
            .to_speaker_evidence("video123", &candidate)
            .expect("evidence");
        assert_eq!(evidence.video_id, "video123");
        assert_eq!(evidence.asr.engine, "phonon-2");
        assert_eq!(evidence.diarization.engine, SHERPA_ONNX_ENGINE_NAME);
        assert_eq!(evidence.segments.len(), 2);
        assert_eq!(evidence.speaker_embeddings.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn portable_model_provenance_avoids_machine_absolute_paths() {
        let path = Path::new("/home/user/models/sherpa-pyannote/model.onnx");
        assert_eq!(portable_model_id(path), "sherpa-pyannote/model.onnx");
    }
}
