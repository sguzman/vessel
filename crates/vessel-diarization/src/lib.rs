use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{CStr, CString, c_char, c_float, c_void};
use std::fs;
use std::path::{Path, PathBuf};
use std::ptr;
use std::slice;

use vessel_core::{
    DiarizationProvenance, Result, SpeakerAttribution, SpeakerEvidenceDiarization,
    SpeakerEvidenceProvenance, SpeakerEvidenceSegment, SpeakerEvidenceV1, TranscriptCandidate,
    TranscriptSpeaker, VesselError,
};

pub const SHERPA_ONNX_BACKEND_NAME: &str = "sherpa-onnx";
pub const SHERPA_ONNX_ENGINE_NAME: &str = "sherpa-onnx";
pub const SHERPA_ONNX_RUNTIME_VERSION: &str = "1.13.6";
pub const DEFAULT_CLUSTERING_THRESHOLD: f32 = 0.5;
pub const DEFAULT_WINDOW_SHIFT_RATIO: f32 = 0.1;
pub const SPEAKER_EMBEDDING_AGGREGATION: &str = "chunk_centroid_v1_3s_16max";
const SPEAKER_EMBEDDING_CHUNK_SECONDS: f64 = 3.0;
const SPEAKER_EMBEDDING_MAX_CHUNKS: usize = 16;
const TRANSCRIPT_ALIGNMENT_MIN_OVERLAP_SECONDS: f64 = 0.25;
const TRANSCRIPT_ALIGNMENT_MIN_DOMINANCE: f64 = 0.60;
const TRANSCRIPT_ALIGNMENT_MIN_MARGIN: f64 = 0.15;

#[derive(Debug, Clone, PartialEq)]
pub struct DiarizationConfig {
    pub backend: String,
    pub runtime_library: PathBuf,
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
        require_file("sherpa runtime library", &self.runtime_library)?;
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
        let num_speakers = self
            .num_speakers
            .map(|value| value.to_string())
            .unwrap_or_else(|| "auto".into());
        format!(
            "segmentation={};embedding={};embedding_aggregation={};num_speakers={};clustering_threshold={:.6};window_shift_ratio={:.6};min_duration_on={:.6};min_duration_off={:.6}",
            portable_model_id(&self.segmentation_model),
            portable_model_id(&self.embedding_model),
            SPEAKER_EMBEDDING_AGGREGATION,
            num_speakers,
            self.clustering_threshold,
            self.window_shift_ratio,
            self.min_duration_on,
            self.min_duration_off,
        )
    }

    pub fn reembedded_model_provenance(&self, existing: &str) -> Result<String> {
        let expected_segmentation = portable_model_id(&self.segmentation_model);
        if provenance_component(existing, "segmentation") != Some(expected_segmentation.as_str()) {
            return Err(diarization_error(format!(
                "existing diarization provenance is incompatible with configured segmentation model: {existing}"
            )));
        }

        let replacement_embedding = portable_model_id(&self.embedding_model);
        let mut components = existing
            .split(';')
            .filter(|component| {
                !component.starts_with("embedding=")
                    && !component.starts_with("embedding_aggregation=")
            })
            .map(str::to_owned)
            .collect::<Vec<_>>();

        let insert_at = components
            .iter()
            .position(|component| component.starts_with("segmentation="))
            .map(|index| index + 1)
            .unwrap_or(0);
        components.insert(
            insert_at,
            format!("embedding={replacement_embedding}"),
        );
        components.insert(
            insert_at + 1,
            format!("embedding_aggregation={SPEAKER_EMBEDDING_AGGREGATION}"),
        );
        Ok(components.join(";"))
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
            let norm = embedding
                .iter()
                .map(|value| value * value)
                .sum::<f64>()
                .sqrt();
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

        let diarization_end = self
            .segments
            .iter()
            .map(|segment| segment.end_seconds)
            .fold(0.0_f64, f64::max);
        let starts = candidate
            .segments
            .iter()
            .map(|segment| segment.start_seconds)
            .collect::<Vec<_>>();

        for (index, segment) in candidate.segments.iter_mut().enumerate() {
            let Some(start_seconds) = segment.start_seconds else {
                segment.speaker = None;
                continue;
            };
            let start = start_seconds as f64;
            let end = starts
                .iter()
                .skip(index + 1)
                .flatten()
                .map(|value| *value as f64)
                .find(|next| *next > start)
                .unwrap_or(diarization_end);

            segment.speaker = align_transcript_interval(&self.segments, start, end).map(|label| {
                TranscriptSpeaker {
                    diarization_label: label,
                    identity: None,
                    attribution: SpeakerAttribution::Unresolved,
                }
            });
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

#[repr(C)]
struct OfflineSpeakerSegmentationPyannoteModelConfig {
    model: *const c_char,
    window_shift_ratio: c_float,
}

#[repr(C)]
struct OfflineSpeakerSegmentationModelConfig {
    pyannote: OfflineSpeakerSegmentationPyannoteModelConfig,
    num_threads: i32,
    debug: i32,
    provider: *const c_char,
}

#[repr(C)]
struct SpeakerEmbeddingExtractorConfig {
    model: *const c_char,
    num_threads: i32,
    debug: i32,
    provider: *const c_char,
}

#[repr(C)]
struct FastClusteringConfig {
    num_clusters: i32,
    threshold: c_float,
}

#[repr(C)]
struct OfflineSpeakerDiarizationConfig {
    segmentation: OfflineSpeakerSegmentationModelConfig,
    embedding: SpeakerEmbeddingExtractorConfig,
    clustering: FastClusteringConfig,
    min_duration_on: c_float,
    min_duration_off: c_float,
}

#[repr(C)]
struct OfflineSpeakerDiarization {
    _private: [u8; 0],
}

#[repr(C)]
struct OfflineSpeakerDiarizationResult {
    _private: [u8; 0],
}

#[repr(C)]
struct SpeakerEmbeddingExtractor {
    _private: [u8; 0],
}

#[repr(C)]
struct OnlineStream {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct RawDiarizationSegment {
    start: c_float,
    end: c_float,
    speaker: i32,
}

#[repr(C)]
struct SherpaOnnxWave {
    samples: *const f32,
    sample_rate: i32,
    num_samples: i32,
}

type CreateDiarizer = unsafe extern "C" fn(
    *const OfflineSpeakerDiarizationConfig,
) -> *const OfflineSpeakerDiarization;
type DestroyDiarizer = unsafe extern "C" fn(*const OfflineSpeakerDiarization);
type DiarizerSampleRate = unsafe extern "C" fn(*const OfflineSpeakerDiarization) -> i32;
type DiarizationProgressCallback = unsafe extern "C" fn(i32, i32, *mut c_void) -> i32;
type ProcessDiarization = unsafe extern "C" fn(
    *const OfflineSpeakerDiarization,
    *const f32,
    i32,
    DiarizationProgressCallback,
    *mut c_void,
) -> *const OfflineSpeakerDiarizationResult;
type ResultNumSpeakers = unsafe extern "C" fn(*const OfflineSpeakerDiarizationResult) -> i32;
type ResultNumSegments = unsafe extern "C" fn(*const OfflineSpeakerDiarizationResult) -> i32;
type SortSegments =
    unsafe extern "C" fn(*const OfflineSpeakerDiarizationResult) -> *const RawDiarizationSegment;
type DestroySegments = unsafe extern "C" fn(*const RawDiarizationSegment);
type DestroyResult = unsafe extern "C" fn(*const OfflineSpeakerDiarizationResult);

type ReadWave = unsafe extern "C" fn(*const c_char) -> *const SherpaOnnxWave;
type FreeWave = unsafe extern "C" fn(*const SherpaOnnxWave);

type CreateEmbeddingExtractor = unsafe extern "C" fn(
    *const SpeakerEmbeddingExtractorConfig,
) -> *const SpeakerEmbeddingExtractor;
type DestroyEmbeddingExtractor = unsafe extern "C" fn(*const SpeakerEmbeddingExtractor);
type EmbeddingDim = unsafe extern "C" fn(*const SpeakerEmbeddingExtractor) -> i32;
type CreateEmbeddingStream =
    unsafe extern "C" fn(*const SpeakerEmbeddingExtractor) -> *const OnlineStream;
type EmbeddingReady =
    unsafe extern "C" fn(*const SpeakerEmbeddingExtractor, *const OnlineStream) -> i32;
type ComputeEmbedding =
    unsafe extern "C" fn(*const SpeakerEmbeddingExtractor, *const OnlineStream) -> *const f32;
type DestroyEmbedding = unsafe extern "C" fn(*const f32);
type DestroyOnlineStream = unsafe extern "C" fn(*const OnlineStream);
type AcceptWaveform = unsafe extern "C" fn(*const OnlineStream, i32, *const f32, i32);
type InputFinished = unsafe extern "C" fn(*const OnlineStream);

struct SherpaApi {
    _main: DynamicLibrary,
    _dependencies: Vec<DynamicLibrary>,
    create_diarizer: CreateDiarizer,
    destroy_diarizer: DestroyDiarizer,
    diarizer_sample_rate: DiarizerSampleRate,
    process_diarization: ProcessDiarization,
    result_num_speakers: ResultNumSpeakers,
    result_num_segments: ResultNumSegments,
    sort_segments: SortSegments,
    destroy_segments: DestroySegments,
    destroy_result: DestroyResult,
    read_wave: ReadWave,
    free_wave: FreeWave,
    create_embedding_extractor: CreateEmbeddingExtractor,
    destroy_embedding_extractor: DestroyEmbeddingExtractor,
    embedding_dim: EmbeddingDim,
    create_embedding_stream: CreateEmbeddingStream,
    embedding_ready: EmbeddingReady,
    compute_embedding: ComputeEmbedding,
    destroy_embedding: DestroyEmbedding,
    destroy_online_stream: DestroyOnlineStream,
    accept_waveform: AcceptWaveform,
    input_finished: InputFinished,
}

unsafe impl Send for SherpaApi {}
unsafe impl Sync for SherpaApi {}

impl SherpaApi {
    fn load(main_path: &Path) -> Result<Self> {
        let dependencies = preload_runtime_dependencies(main_path);
        let main = DynamicLibrary::open(main_path).map_err(|error| {
            diarization_error(format!(
                "failed to load sherpa runtime {}: {error}",
                main_path.display()
            ))
        })?;

        unsafe {
            Ok(Self {
                create_diarizer: main.symbol("SherpaOnnxCreateOfflineSpeakerDiarization")?,
                destroy_diarizer: main.symbol("SherpaOnnxDestroyOfflineSpeakerDiarization")?,
                diarizer_sample_rate: main
                    .symbol("SherpaOnnxOfflineSpeakerDiarizationGetSampleRate")?,
                process_diarization: main
                    .symbol("SherpaOnnxOfflineSpeakerDiarizationProcessWithCallback")?,
                result_num_speakers: main
                    .symbol("SherpaOnnxOfflineSpeakerDiarizationResultGetNumSpeakers")?,
                result_num_segments: main
                    .symbol("SherpaOnnxOfflineSpeakerDiarizationResultGetNumSegments")?,
                sort_segments: main
                    .symbol("SherpaOnnxOfflineSpeakerDiarizationResultSortByStartTime")?,
                destroy_segments: main
                    .symbol("SherpaOnnxOfflineSpeakerDiarizationDestroySegment")?,
                destroy_result: main.symbol("SherpaOnnxOfflineSpeakerDiarizationDestroyResult")?,
                read_wave: main.symbol("SherpaOnnxReadWave")?,
                free_wave: main.symbol("SherpaOnnxFreeWave")?,
                create_embedding_extractor: main
                    .symbol("SherpaOnnxCreateSpeakerEmbeddingExtractor")?,
                destroy_embedding_extractor: main
                    .symbol("SherpaOnnxDestroySpeakerEmbeddingExtractor")?,
                embedding_dim: main.symbol("SherpaOnnxSpeakerEmbeddingExtractorDim")?,
                create_embedding_stream: main
                    .symbol("SherpaOnnxSpeakerEmbeddingExtractorCreateStream")?,
                embedding_ready: main.symbol("SherpaOnnxSpeakerEmbeddingExtractorIsReady")?,
                compute_embedding: main
                    .symbol("SherpaOnnxSpeakerEmbeddingExtractorComputeEmbedding")?,
                destroy_embedding: main
                    .symbol("SherpaOnnxSpeakerEmbeddingExtractorDestroyEmbedding")?,
                destroy_online_stream: main.symbol("SherpaOnnxDestroyOnlineStream")?,
                accept_waveform: main.symbol("SherpaOnnxOnlineStreamAcceptWaveform")?,
                input_finished: main.symbol("SherpaOnnxOnlineStreamInputFinished")?,
                _main: main,
                _dependencies: dependencies,
            })
        }
    }
}

pub struct SherpaOnnxDiarizer {
    config: DiarizationConfig,
    api: SherpaApi,
    diarizer: *const OfflineSpeakerDiarization,
    embedding_extractor: *const SpeakerEmbeddingExtractor,
}

unsafe impl Send for SherpaOnnxDiarizer {}

impl SherpaOnnxDiarizer {
    pub fn load(config: DiarizationConfig) -> Result<Self> {
        config.validate()?;
        let api = SherpaApi::load(&config.runtime_library)?;

        let segmentation_model = path_cstring(&config.segmentation_model)?;
        let embedding_model = path_cstring(&config.embedding_model)?;
        let provider = CString::new(config.provider.as_str())
            .map_err(|_| diarization_error("diarization provider contains an embedded NUL byte"))?;

        let embedding_config = SpeakerEmbeddingExtractorConfig {
            model: embedding_model.as_ptr(),
            num_threads: config.num_threads,
            debug: 0,
            provider: provider.as_ptr(),
        };
        let diarizer_config = OfflineSpeakerDiarizationConfig {
            segmentation: OfflineSpeakerSegmentationModelConfig {
                pyannote: OfflineSpeakerSegmentationPyannoteModelConfig {
                    model: segmentation_model.as_ptr(),
                    window_shift_ratio: config.window_shift_ratio,
                },
                num_threads: config.num_threads,
                debug: 0,
                provider: provider.as_ptr(),
            },
            embedding: SpeakerEmbeddingExtractorConfig {
                model: embedding_model.as_ptr(),
                num_threads: config.num_threads,
                debug: 0,
                provider: provider.as_ptr(),
            },
            clustering: FastClusteringConfig {
                num_clusters: config.num_speakers.map(|value| value as i32).unwrap_or(-1),
                threshold: config.clustering_threshold,
            },
            min_duration_on: config.min_duration_on,
            min_duration_off: config.min_duration_off,
        };

        eprintln!(
            "[diarization] runtime load completed engine={} runtime={} segmentation={} embedding={}",
            SHERPA_ONNX_ENGINE_NAME,
            config.runtime_library.display(),
            config.segmentation_model.display(),
            config.embedding_model.display(),
        );

        let diarizer = unsafe { (api.create_diarizer)(&diarizer_config) };
        if diarizer.is_null() {
            return Err(diarization_error(
                "failed to initialize sherpa-onnx offline speaker diarization",
            ));
        }

        let embedding_extractor = if config.speaker_embeddings {
            let extractor = unsafe { (api.create_embedding_extractor)(&embedding_config) };
            if extractor.is_null() {
                unsafe { (api.destroy_diarizer)(diarizer) };
                return Err(diarization_error(
                    "failed to initialize sherpa-onnx speaker embedding extractor",
                ));
            }
            extractor
        } else {
            ptr::null()
        };

        let sample_rate = unsafe { (api.diarizer_sample_rate)(diarizer) };
        eprintln!(
            "[diarization] model load completed engine={} sample_rate={}",
            SHERPA_ONNX_ENGINE_NAME, sample_rate,
        );

        Ok(Self {
            config,
            api,
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
        let filename = path_cstring(path)?;
        let wave = unsafe { (self.api.read_wave)(filename.as_ptr()) };
        if wave.is_null() {
            return Err(diarization_error(format!(
                "failed to read diarization WAV {}",
                path.display()
            )));
        }
        let wave_ref = unsafe { &*wave };
        let expected_sample_rate = unsafe { (self.api.diarizer_sample_rate)(self.diarizer) };
        if wave_ref.sample_rate != expected_sample_rate {
            let actual_sample_rate = wave_ref.sample_rate;
            unsafe { (self.api.free_wave)(wave) };
            return Err(diarization_error(format!(
                "diarization WAV sample rate {} does not match sherpa-onnx expected {}",
                actual_sample_rate, expected_sample_rate
            )));
        }
        if wave_ref.num_samples <= 0 || wave_ref.samples.is_null() {
            unsafe { (self.api.free_wave)(wave) };
            return Err(diarization_error("diarization WAV contains no samples"));
        }

        eprintln!(
            "[diarization] inference started engine={} input={} samples={}",
            SHERPA_ONNX_ENGINE_NAME,
            path.display(),
            wave_ref.num_samples,
        );
        let mut progress = DiarizationProgressState::default();
        let raw = unsafe {
            (self.api.process_diarization)(
                self.diarizer,
                wave_ref.samples,
                wave_ref.num_samples,
                diarization_progress,
                (&mut progress as *mut DiarizationProgressState).cast::<c_void>(),
            )
        };
        if raw.is_null() {
            unsafe { (self.api.free_wave)(wave) };
            return Err(diarization_error(
                "sherpa-onnx diarization returned no result",
            ));
        }

        let num_speakers = unsafe { (self.api.result_num_speakers)(raw) };
        let num_segments = unsafe { (self.api.result_num_segments)(raw) };
        let raw_segments_ptr = unsafe { (self.api.sort_segments)(raw) };
        let raw_segments = if num_segments > 0 && !raw_segments_ptr.is_null() {
            unsafe { slice::from_raw_parts(raw_segments_ptr, num_segments as usize) }
                .iter()
                .copied()
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

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

        let samples =
            unsafe { slice::from_raw_parts(wave_ref.samples, wave_ref.num_samples as usize) };
        let mut speaker_embeddings = BTreeMap::new();
        if !self.embedding_extractor.is_null() {
            let dim = unsafe { (self.api.embedding_dim)(self.embedding_extractor) };
            if dim <= 0 {
                self.destroy_process_values(raw, raw_segments_ptr, wave);
                return Err(diarization_error(
                    "sherpa-onnx speaker embedding dimension is invalid",
                ));
            }

            let speaker_ids = raw_segments
                .iter()
                .filter_map(|segment| (segment.speaker >= 0).then_some(segment.speaker))
                .collect::<BTreeSet<_>>();
            for speaker in speaker_ids {
                let label = speaker_label(speaker);
                let speaker_samples = concatenate_speaker_audio(
                    samples,
                    wave_ref.sample_rate,
                    &raw_segments,
                    speaker,
                );
                if let Some(embedding) =
                    self.embedding_centroid(&speaker_samples, wave_ref.sample_rate, dim)
                {
                    speaker_embeddings.insert(label, embedding);
                }
            }
        }

        self.destroy_process_values(raw, raw_segments_ptr, wave);

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
            num_speakers,
            result.segments.len(),
            result.speaker_embeddings.len(),
        );
        Ok(result)
    }

    pub fn reembed_segments_path(
        &self,
        path: &Path,
        segments: &[DiarizationSegment],
    ) -> Result<BTreeMap<String, Vec<f64>>> {
        if self.embedding_extractor.is_null() {
            return Err(diarization_error(
                "speaker embedding extractor is not enabled in this diarization config",
            ));
        }
        if !path.is_file() {
            return Err(diarization_error(format!(
                "diarization input does not exist: {}",
                path.display()
            )));
        }
        let filename = path_cstring(path)?;
        let wave = unsafe { (self.api.read_wave)(filename.as_ptr()) };
        if wave.is_null() {
            return Err(diarization_error(format!(
                "failed to read diarization WAV {}",
                path.display()
            )));
        }
        let wave_ref = unsafe { &*wave };
        let expected_sample_rate = unsafe { (self.api.diarizer_sample_rate)(self.diarizer) };
        if wave_ref.sample_rate != expected_sample_rate
            || wave_ref.num_samples <= 0
            || wave_ref.samples.is_null()
        {
            unsafe { (self.api.free_wave)(wave) };
            return Err(diarization_error(
                "re-embedding WAV is empty or has an incompatible sample rate",
            ));
        }

        let dimension = unsafe { (self.api.embedding_dim)(self.embedding_extractor) };
        if dimension <= 0 {
            unsafe { (self.api.free_wave)(wave) };
            return Err(diarization_error(
                "sherpa-onnx speaker embedding dimension is invalid",
            ));
        }
        let samples =
            unsafe { slice::from_raw_parts(wave_ref.samples, wave_ref.num_samples as usize) };
        let labels = segments
            .iter()
            .map(|segment| segment.speaker.clone())
            .collect::<BTreeSet<_>>();
        let mut embeddings = BTreeMap::new();
        for label in labels {
            let speaker_samples =
                concatenate_labeled_speaker_audio(samples, wave_ref.sample_rate, segments, &label);
            if let Some(embedding) =
                self.embedding_centroid(&speaker_samples, wave_ref.sample_rate, dimension)
            {
                embeddings.insert(label, embedding);
            }
        }
        unsafe { (self.api.free_wave)(wave) };
        Ok(embeddings)
    }

    fn embedding_centroid(
        &self,
        samples: &[f32],
        sample_rate: i32,
        dimension: i32,
    ) -> Option<Vec<f64>> {
        let windows = representative_embedding_windows(samples.len(), sample_rate);
        if windows.is_empty() {
            return None;
        }

        let mut embeddings = Vec::new();
        for window in windows {
            let chunk = &samples[window];
            let stream = unsafe { (self.api.create_embedding_stream)(self.embedding_extractor) };
            if stream.is_null() {
                continue;
            }
            unsafe {
                (self.api.accept_waveform)(stream, sample_rate, chunk.as_ptr(), chunk.len() as i32);
                (self.api.input_finished)(stream);
            }

            let ready = unsafe { (self.api.embedding_ready)(self.embedding_extractor, stream) };
            if ready != 0 {
                let embedding =
                    unsafe { (self.api.compute_embedding)(self.embedding_extractor, stream) };
                if !embedding.is_null() {
                    let mut values =
                        unsafe { slice::from_raw_parts(embedding, dimension as usize) }
                            .iter()
                            .map(|value| f64::from(*value))
                            .collect::<Vec<_>>();
                    if normalize_embedding_in_place(&mut values) {
                        embeddings.push(values);
                    }
                    unsafe { (self.api.destroy_embedding)(embedding) };
                }
            }
            unsafe { (self.api.destroy_online_stream)(stream) };
        }

        if embeddings.is_empty() {
            return None;
        }
        let dimension = embeddings[0].len();
        let mut centroid = vec![0.0; dimension];
        for embedding in &embeddings {
            if embedding.len() != dimension {
                return None;
            }
            for (index, value) in embedding.iter().enumerate() {
                centroid[index] += value;
            }
        }
        for value in &mut centroid {
            *value /= embeddings.len() as f64;
        }
        normalize_embedding_in_place(&mut centroid).then_some(centroid)
    }

    fn destroy_process_values(
        &self,
        raw: *const OfflineSpeakerDiarizationResult,
        raw_segments: *const RawDiarizationSegment,
        wave: *const SherpaOnnxWave,
    ) {
        unsafe {
            if !raw_segments.is_null() {
                (self.api.destroy_segments)(raw_segments);
            }
            if !raw.is_null() {
                (self.api.destroy_result)(raw);
            }
            if !wave.is_null() {
                (self.api.free_wave)(wave);
            }
        }
    }
}

impl Drop for SherpaOnnxDiarizer {
    fn drop(&mut self) {
        unsafe {
            if !self.embedding_extractor.is_null() {
                (self.api.destroy_embedding_extractor)(self.embedding_extractor);
                self.embedding_extractor = ptr::null();
            }
            if !self.diarizer.is_null() {
                (self.api.destroy_diarizer)(self.diarizer);
                self.diarizer = ptr::null();
            }
        }
    }
}

const DIARIZATION_PROGRESS_STEP_PERCENT: i32 = 10;

#[derive(Debug)]
struct DiarizationProgressState {
    next_percent: i32,
}

impl Default for DiarizationProgressState {
    fn default() -> Self {
        Self {
            next_percent: DIARIZATION_PROGRESS_STEP_PERCENT,
        }
    }
}

impl DiarizationProgressState {
    fn observe(&mut self, processed_chunks: i32, total_chunks: i32) -> bool {
        if total_chunks <= 0 || processed_chunks <= 0 {
            return false;
        }
        let percent = ((processed_chunks as i64 * 100) / total_chunks as i64).clamp(0, 100) as i32;
        let should_report = processed_chunks >= total_chunks || percent >= self.next_percent;
        if should_report {
            while self.next_percent <= percent {
                self.next_percent += DIARIZATION_PROGRESS_STEP_PERCENT;
            }
        }
        should_report
    }
}

unsafe extern "C" fn diarization_progress(
    processed_chunks: i32,
    total_chunks: i32,
    arg: *mut c_void,
) -> i32 {
    if arg.is_null() {
        return 0;
    }
    let state = unsafe { &mut *arg.cast::<DiarizationProgressState>() };
    if state.observe(processed_chunks, total_chunks) {
        let percent = 100.0 * processed_chunks as f64 / total_chunks as f64;
        eprintln!(
            "[diarization] progress {:.1}% ({}/{})",
            percent, processed_chunks, total_chunks
        );
    }
    0
}

pub fn probe_sherpa_runtime(runtime_library: &Path) -> Result<()> {
    let _api = SherpaApi::load(runtime_library)?;
    Ok(())
}

pub fn find_sherpa_runtime_library(root: &Path) -> Result<PathBuf> {
    if root.is_file() && is_sherpa_runtime_library(root) {
        return Ok(root.to_path_buf());
    }
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > 5 || !dir.is_dir() {
            continue;
        }
        let entries = fs::read_dir(&dir)?;
        for entry in entries {
            let path = entry?.path();
            if path.is_file() && is_sherpa_runtime_library(&path) {
                return Ok(path);
            }
            if path.is_dir() {
                stack.push((path, depth + 1));
            }
        }
    }
    Err(diarization_error(format!(
        "sherpa runtime library was not found under {}",
        root.display()
    )))
}

fn is_sherpa_runtime_library(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if cfg!(target_os = "windows") {
        name.eq_ignore_ascii_case("sherpa-onnx-c-api.dll")
    } else if cfg!(target_os = "macos") {
        name == "libsherpa-onnx-c-api.dylib"
    } else {
        name == "libsherpa-onnx-c-api.so" || name.starts_with("libsherpa-onnx-c-api.so.")
    }
}

fn preload_runtime_dependencies(main_path: &Path) -> Vec<DynamicLibrary> {
    let Some(dir) = main_path.parent() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut candidates = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.is_file() && runtime_dependency_candidate(path) && !is_sherpa_runtime_library(path)
        })
        .filter(|path| path != main_path)
        .collect::<Vec<_>>();
    candidates.sort();

    let mut loaded = Vec::new();
    let mut remaining = candidates;
    for _ in 0..4 {
        let mut next = Vec::new();
        let before = remaining.len();
        for path in remaining {
            match DynamicLibrary::open(&path) {
                Ok(lib) => loaded.push(lib),
                Err(_) => next.push(path),
            }
        }
        if next.is_empty() || next.len() == before {
            break;
        }
        remaining = next;
    }
    loaded
}

fn runtime_dependency_candidate(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if cfg!(target_os = "windows") {
        name.to_ascii_lowercase().ends_with(".dll")
    } else if cfg!(target_os = "macos") {
        name.ends_with(".dylib")
    } else {
        name.contains(".so")
    }
}

struct DynamicLibrary {
    handle: *mut c_void,
}

unsafe impl Send for DynamicLibrary {}
unsafe impl Sync for DynamicLibrary {}

impl DynamicLibrary {
    fn open(path: &Path) -> std::result::Result<Self, String> {
        platform_dynlib::open(path).map(|handle| Self { handle })
    }

    unsafe fn symbol<T: Copy>(&self, name: &str) -> Result<T> {
        let symbol = platform_dynlib::symbol(self.handle, name)
            .map_err(|error| diarization_error(format!("missing sherpa symbol {name}: {error}")))?;
        if std::mem::size_of::<T>() != std::mem::size_of::<*mut c_void>() {
            return Err(diarization_error(format!(
                "invalid function pointer size for sherpa symbol {name}"
            )));
        }
        Ok(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&symbol) })
    }
}

impl Drop for DynamicLibrary {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            platform_dynlib::close(self.handle);
            self.handle = ptr::null_mut();
        }
    }
}

#[cfg(unix)]
mod platform_dynlib {
    use super::*;

    const RTLD_NOW: i32 = 2;
    #[cfg(target_os = "macos")]
    const RTLD_GLOBAL: i32 = 0x8;
    #[cfg(not(target_os = "macos"))]
    const RTLD_GLOBAL: i32 = 0x100;

    unsafe extern "C" {
        fn dlopen(filename: *const c_char, flags: i32) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        fn dlclose(handle: *mut c_void) -> i32;
        fn dlerror() -> *const c_char;
    }

    pub fn open(path: &Path) -> std::result::Result<*mut c_void, String> {
        use std::os::unix::ffi::OsStrExt;
        let path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| "dynamic-library path contains an embedded NUL byte".to_owned())?;
        let handle = unsafe { dlopen(path.as_ptr(), RTLD_NOW | RTLD_GLOBAL) };
        if handle.is_null() {
            Err(last_error())
        } else {
            Ok(handle)
        }
    }

    pub fn symbol(handle: *mut c_void, name: &str) -> std::result::Result<*mut c_void, String> {
        let name = CString::new(name)
            .map_err(|_| "dynamic-library symbol contains an embedded NUL byte".to_owned())?;
        unsafe {
            let _ = dlerror();
            let value = dlsym(handle, name.as_ptr());
            let error = dlerror();
            if !error.is_null() {
                Err(CStr::from_ptr(error).to_string_lossy().into_owned())
            } else if value.is_null() {
                Err("symbol resolved to NULL".into())
            } else {
                Ok(value)
            }
        }
    }

    pub fn close(handle: *mut c_void) {
        unsafe {
            let _ = dlclose(handle);
        }
    }

    fn last_error() -> String {
        unsafe {
            let error = dlerror();
            if error.is_null() {
                "unknown dynamic-loader error".into()
            } else {
                CStr::from_ptr(error).to_string_lossy().into_owned()
            }
        }
    }
}

#[cfg(windows)]
mod platform_dynlib {
    use super::*;
    use std::os::windows::ffi::OsStrExt;

    unsafe extern "system" {
        fn LoadLibraryW(filename: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
        fn FreeLibrary(module: *mut c_void) -> i32;
        fn GetLastError() -> u32;
    }

    pub fn open(path: &Path) -> std::result::Result<*mut c_void, String> {
        let mut wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
        wide.push(0);
        let handle = unsafe { LoadLibraryW(wide.as_ptr()) };
        if handle.is_null() {
            Err(format!("LoadLibraryW failed with error {}", unsafe {
                GetLastError()
            }))
        } else {
            Ok(handle)
        }
    }

    pub fn symbol(handle: *mut c_void, name: &str) -> std::result::Result<*mut c_void, String> {
        let name = CString::new(name)
            .map_err(|_| "dynamic-library symbol contains an embedded NUL byte".to_owned())?;
        let value = unsafe { GetProcAddress(handle, name.as_ptr().cast()) };
        if value.is_null() {
            Err(format!("GetProcAddress failed with error {}", unsafe {
                GetLastError()
            }))
        } else {
            Ok(value)
        }
    }

    pub fn close(handle: *mut c_void) {
        unsafe {
            let _ = FreeLibrary(handle);
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod platform_dynlib {
    use super::*;

    pub fn open(_path: &Path) -> std::result::Result<*mut c_void, String> {
        Err("runtime sherpa loading is not implemented on this platform".into())
    }

    pub fn symbol(_handle: *mut c_void, _name: &str) -> std::result::Result<*mut c_void, String> {
        Err("runtime sherpa loading is not implemented on this platform".into())
    }

    pub fn close(_handle: *mut c_void) {}
}

pub fn persist_speaker_evidence(
    evidence_dir: &Path,
    evidence: &SpeakerEvidenceV1,
) -> Result<PathBuf> {
    evidence.validate()?;
    fs::create_dir_all(evidence_dir)?;
    let path = evidence_dir.join(format!("{}.json", evidence.video_id));
    let temp = evidence_dir.join(format!("{}.json.tmp", evidence.video_id));
    let rendered = serde_json::to_vec_pretty(evidence).map_err(|error| {
        diarization_error(format!("speaker evidence serialization failed: {error}"))
    })?;
    fs::write(&temp, rendered)?;
    fs::rename(&temp, &path)?;
    Ok(path)
}

fn align_transcript_interval(
    diarization: &[DiarizationSegment],
    start: f64,
    end: f64,
) -> Option<String> {
    if !start.is_finite() || !end.is_finite() || end <= start {
        return None;
    }

    let mut overlap_by_speaker = BTreeMap::<&str, f64>::new();
    for segment in diarization {
        let overlap_start = start.max(segment.start_seconds);
        let overlap_end = end.min(segment.end_seconds);
        let overlap = overlap_end - overlap_start;
        if overlap > 0.0 {
            *overlap_by_speaker
                .entry(segment.speaker.as_str())
                .or_default() += overlap;
        }
    }

    let total_overlap = overlap_by_speaker.values().copied().sum::<f64>();
    if total_overlap < TRANSCRIPT_ALIGNMENT_MIN_OVERLAP_SECONDS {
        return None;
    }

    let mut ranked = overlap_by_speaker.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|left, right| right.1.total_cmp(&left.1).then_with(|| left.0.cmp(right.0)));
    let (best_speaker, best_overlap) = *ranked.first()?;
    let second_overlap = ranked.get(1).map(|(_, overlap)| *overlap).unwrap_or(0.0);
    let dominance = best_overlap / total_overlap;
    let margin = (best_overlap - second_overlap) / total_overlap;

    if dominance < TRANSCRIPT_ALIGNMENT_MIN_DOMINANCE
        || (ranked.len() > 1 && margin < TRANSCRIPT_ALIGNMENT_MIN_MARGIN)
    {
        return None;
    }

    Some(best_speaker.to_owned())
}

fn provenance_component<'a>(model: &'a str, key: &str) -> Option<&'a str> {
    model.split(';').find_map(|component| {
        let (component_key, value) = component.split_once('=')?;
        (component_key == key).then_some(value)
    })
}

fn representative_embedding_windows(
    sample_count: usize,
    sample_rate: i32,
) -> Vec<std::ops::Range<usize>> {
    if sample_count == 0 || sample_rate <= 0 {
        return Vec::new();
    }
    let chunk_len = (SPEAKER_EMBEDDING_CHUNK_SECONDS * f64::from(sample_rate)).round() as usize;
    if chunk_len == 0 || sample_count <= chunk_len {
        return vec![0..sample_count];
    }

    let available_full_chunks = (sample_count / chunk_len).max(1);
    let count = available_full_chunks.min(SPEAKER_EMBEDDING_MAX_CHUNKS);
    if count == 1 {
        let start = (sample_count - chunk_len) / 2;
        return vec![start..start + chunk_len];
    }

    let max_start = sample_count - chunk_len;
    (0..count)
        .map(|index| {
            let start = index * max_start / (count - 1);
            start..start + chunk_len
        })
        .collect()
}

fn normalize_embedding_in_place(values: &mut [f64]) -> bool {
    let norm = values.iter().map(|value| value * value).sum::<f64>().sqrt();
    if !norm.is_finite() || norm <= f64::EPSILON {
        return false;
    }
    for value in values {
        *value /= norm;
    }
    true
}

fn concatenate_speaker_audio(
    samples: &[f32],
    sample_rate: i32,
    segments: &[RawDiarizationSegment],
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

fn concatenate_labeled_speaker_audio(
    samples: &[f32],
    sample_rate: i32,
    segments: &[DiarizationSegment],
    speaker: &str,
) -> Vec<f32> {
    if sample_rate <= 0 {
        return Vec::new();
    }
    let sample_rate = f64::from(sample_rate);
    let mut output = Vec::new();
    for segment in segments.iter().filter(|segment| segment.speaker == speaker) {
        let start = ((segment.start_seconds * sample_rate).floor() as usize).min(samples.len());
        let end = ((segment.end_seconds * sample_rate).ceil() as usize).min(samples.len());
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

fn path_cstring(path: &Path) -> Result<CString> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        return CString::new(path.as_os_str().as_bytes())
            .map_err(|_| diarization_error(format!("path contains NUL: {}", path.display())));
    }
    #[cfg(not(unix))]
    {
        CString::new(path.to_string_lossy().as_bytes())
            .map_err(|_| diarization_error(format!("path contains NUL: {}", path.display())))
    }
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
    use vessel_core::{TranscriptDerivation, TranscriptSegment};

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
    fn model_provenance_includes_clustering_configuration() {
        let config = DiarizationConfig {
            backend: SHERPA_ONNX_BACKEND_NAME.into(),
            runtime_library: PathBuf::from("/runtime/libsherpa-onnx-c-api.so"),
            segmentation_model: PathBuf::from("/models/segmentation/model.onnx"),
            embedding_model: PathBuf::from("/models/embedding/model.onnx"),
            provider: "cpu".into(),
            num_threads: 4,
            num_speakers: None,
            clustering_threshold: 0.9,
            window_shift_ratio: 0.1,
            min_duration_on: 0.3,
            min_duration_off: 0.5,
            speaker_embeddings: true,
        };

        let provenance = config.model_provenance();
        assert!(provenance.contains("segmentation=segmentation/model.onnx"));
        assert!(provenance.contains("embedding=embedding/model.onnx"));
        assert!(provenance.contains(&format!(
            "embedding_aggregation={SPEAKER_EMBEDDING_AGGREGATION}"
        )));
        assert!(provenance.contains("num_speakers=auto"));
        assert!(provenance.contains("clustering_threshold=0.900000"));
        assert!(provenance.contains("window_shift_ratio=0.100000"));
        assert!(provenance.contains("min_duration_on=0.300000"));
        assert!(provenance.contains("min_duration_off=0.500000"));
    }

    #[test]
    fn reembedded_provenance_can_replace_embedding_model_without_reclustering() {
        let config = DiarizationConfig {
            backend: SHERPA_ONNX_BACKEND_NAME.into(),
            runtime_library: PathBuf::from("/runtime/libsherpa-onnx-c-api.so"),
            segmentation_model: PathBuf::from("/models/segmentation/model.onnx"),
            embedding_model: PathBuf::from("/models/embedding/english.onnx"),
            provider: "cpu".into(),
            num_threads: 4,
            num_speakers: None,
            clustering_threshold: 0.5,
            window_shift_ratio: 0.1,
            min_duration_on: 0.3,
            min_duration_off: 0.5,
            speaker_embeddings: true,
        };
        let existing = "segmentation=segmentation/model.onnx;embedding=embedding/chinese.onnx;embedding_aggregation=legacy;num_speakers=auto;clustering_threshold=0.900000";
        let updated = config.reembedded_model_provenance(existing).unwrap();
        assert!(updated.contains("clustering_threshold=0.900000"));
        assert!(updated.contains("embedding=embedding/english.onnx"));
        assert!(!updated.contains("embedding=embedding/chinese.onnx"));
        assert!(updated.contains(&format!(
            "embedding_aggregation={SPEAKER_EMBEDDING_AGGREGATION}"
        )));
    }

    #[test]
    fn reembedded_provenance_rejects_different_segmentation_model() {
        let config = DiarizationConfig {
            backend: SHERPA_ONNX_BACKEND_NAME.into(),
            runtime_library: PathBuf::from("/runtime/libsherpa-onnx-c-api.so"),
            segmentation_model: PathBuf::from("/models/segmentation/new.onnx"),
            embedding_model: PathBuf::from("/models/embedding/english.onnx"),
            provider: "cpu".into(),
            num_threads: 4,
            num_speakers: None,
            clustering_threshold: 0.5,
            window_shift_ratio: 0.1,
            min_duration_on: 0.3,
            min_duration_off: 0.5,
            speaker_embeddings: true,
        };
        let existing = "segmentation=segmentation/model.onnx;embedding=embedding/chinese.onnx;num_speakers=auto;clustering_threshold=0.900000";
        assert!(config.reembedded_model_provenance(existing).is_err());
    }

    #[test]
    fn sparse_speaker_ids_are_not_assumed_contiguous() {
        let segments = vec![
            RawDiarizationSegment {
                start: 0.0,
                end: 1.0,
                speaker: 0,
            },
            RawDiarizationSegment {
                start: 1.0,
                end: 2.0,
                speaker: 34,
            },
            RawDiarizationSegment {
                start: 2.0,
                end: 3.0,
                speaker: 71,
            },
        ];
        let ids = segments
            .iter()
            .filter_map(|segment| (segment.speaker >= 0).then_some(segment.speaker))
            .collect::<BTreeSet<_>>();
        assert_eq!(ids, BTreeSet::from([0, 34, 71]));
    }

    #[test]
    fn representative_embedding_windows_are_bounded_and_distributed() {
        let sample_rate = 16_000;
        let sample_count = sample_rate as usize * 1_000;
        let windows = representative_embedding_windows(sample_count, sample_rate);

        assert_eq!(windows.len(), SPEAKER_EMBEDDING_MAX_CHUNKS);
        assert_eq!(windows[0].start, 0);
        assert_eq!(windows.last().unwrap().end, sample_count);
        assert!(
            windows
                .iter()
                .all(|window| window.len() == sample_rate as usize * 3)
        );
    }

    #[test]
    fn short_speaker_audio_uses_one_embedding_window() {
        let windows = representative_embedding_windows(16_000 * 2, 16_000);
        assert_eq!(windows, vec![0..32_000]);
    }

    #[test]
    fn diarization_progress_reports_only_coarse_milestones() {
        let mut state = DiarizationProgressState::default();
        let mut reported = Vec::new();
        for processed in 1..=1_546 {
            if state.observe(processed, 1_546) {
                reported.push(processed);
            }
        }
        assert_eq!(reported.len(), 10);
        assert!(reported[0] >= 154 && reported[0] <= 155);
        assert_eq!(reported.last().copied(), Some(1_546));
    }

    #[test]
    fn diarization_progress_handles_small_chunk_counts_without_spam() {
        let mut state = DiarizationProgressState::default();
        let reported = (1..=3)
            .filter(|processed| state.observe(*processed, 3))
            .collect::<Vec<_>>();
        assert_eq!(reported, vec![1, 2, 3]);
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
    fn interval_overlap_labels_segment_that_starts_before_first_speech_frame() {
        let result = DiarizationResult {
            engine: SHERPA_ONNX_ENGINE_NAME.into(),
            model: "segmentation=seg/model.onnx;embedding=emb/model.onnx".into(),
            segments: vec![DiarizationSegment {
                start_seconds: 0.031,
                end_seconds: 15.978,
                speaker: "SPEAKER_00".into(),
            }],
            speaker_embeddings: BTreeMap::new(),
        };
        let mut candidate = TranscriptCandidate {
            derivation: TranscriptDerivation::LocalAsr,
            language: Some("en".into()),
            timestamps: true,
            engine: Some("whisper-candle".into()),
            model: Some("small".into()),
            diarization: None,
            segments: vec![
                TranscriptSegment {
                    start_seconds: Some(0),
                    text: "Opening.".into(),
                    speaker: None,
                },
                TranscriptSegment {
                    start_seconds: Some(4),
                    text: "Continuation.".into(),
                    speaker: None,
                },
            ],
        };

        result.apply_to_candidate(&mut candidate).expect("apply");
        assert_eq!(
            candidate.segments[0]
                .speaker
                .as_ref()
                .map(|speaker| speaker.diarization_label.as_str()),
            Some("SPEAKER_00")
        );
    }

    #[test]
    fn near_even_speaker_change_remains_unresolved() {
        let diarization = vec![
            DiarizationSegment {
                start_seconds: 15.978,
                end_seconds: 24.044,
                speaker: "SPEAKER_01".into(),
            },
            DiarizationSegment {
                start_seconds: 24.044,
                end_seconds: 26.761,
                speaker: "SPEAKER_02".into(),
            },
            DiarizationSegment {
                start_seconds: 26.761,
                end_seconds: 47.197,
                speaker: "SPEAKER_01".into(),
            },
        ];

        assert_eq!(align_transcript_interval(&diarization, 21.0, 27.0), None);
        assert_eq!(
            align_transcript_interval(&diarization, 27.0, 31.0).as_deref(),
            Some("SPEAKER_01")
        );
    }

    #[test]
    fn silence_gap_does_not_defeat_single_speaker_overlap() {
        let diarization = vec![
            DiarizationSegment {
                start_seconds: 26.761,
                end_seconds: 47.197,
                speaker: "SPEAKER_01".into(),
            },
            DiarizationSegment {
                start_seconds: 52.630,
                end_seconds: 61.068,
                speaker: "SPEAKER_01".into(),
            },
        ];

        assert_eq!(
            align_transcript_interval(&diarization, 47.0, 57.0).as_deref(),
            Some("SPEAKER_01")
        );
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

    #[test]
    fn runtime_finder_recurses_into_extracted_bundle() {
        let root = std::env::temp_dir().join(format!("vessel-runtime-find-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let lib = root
            .join("sherpa-bundle")
            .join("lib")
            .join(if cfg!(target_os = "windows") {
                "sherpa-onnx-c-api.dll"
            } else if cfg!(target_os = "macos") {
                "libsherpa-onnx-c-api.dylib"
            } else {
                "libsherpa-onnx-c-api.so"
            });
        fs::create_dir_all(lib.parent().unwrap()).expect("runtime dir");
        fs::write(&lib, b"not-a-real-library").expect("runtime placeholder");

        assert_eq!(find_sherpa_runtime_library(&root).unwrap(), lib);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_library_name_detection_is_platform_specific() {
        #[cfg(target_os = "linux")]
        assert!(is_sherpa_runtime_library(Path::new(
            "libsherpa-onnx-c-api.so"
        )));
        #[cfg(target_os = "windows")]
        assert!(is_sherpa_runtime_library(Path::new(
            "sherpa-onnx-c-api.dll"
        )));
    }
}
