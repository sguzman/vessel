use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use serde_json::Value;

use vessel_core::{
    DiarizationProvenance, Result, SpeakerAttribution, TranscriptCandidate, TranscriptDerivation,
    TranscriptSegment, TranscriptSpeaker, VesselError,
};
use whisper_core::{
    TranscribeOptions, WhisperModel, WhichModel, device, fetch_model as fetch_whisper_model,
    load_model, transcribe_file,
};

pub const WHISPER_CANDLE_ENGINE_NAME: &str = "whisper-candle";
pub const PHONON2_BACKEND_NAME: &str = "phonon-2";
pub const PHONON_ENGINE_NAME: &str = "fermion-phonon";
pub const WHISPERX_BACKEND_NAME: &str = "whisperx";
pub const WHISPERX_ENGINE_NAME: &str = "whisperx-faster-whisper";
pub const DEFAULT_DIARIZATION_MODEL: &str = "pyannote/speaker-diarization-community-1";
pub const ENGINE_NAME: &str = WHISPER_CANDLE_ENGINE_NAME;
pub const WHISPER_MODEL_NAMES: &[&str] = &[
    "tiny",
    "tiny.en",
    "base",
    "base.en",
    "small",
    "small.en",
    "medium",
    "medium.en",
    "large-v1",
    "large-v2",
    "large-v3",
    "large-v3-turbo",
];
pub const PHONON_MODEL_NAMES: &[&str] =
    &["phonon-2", "phonon-1", "phonon-1-big", "phonon-1-micro"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsrModelLocation {
    pub backend: String,
    pub model: String,
    pub directory: PathBuf,
}

pub fn default_model_for_backend(backend: &str) -> Result<&'static str> {
    match backend {
        WHISPER_CANDLE_ENGINE_NAME => Ok("small"),
        PHONON2_BACKEND_NAME => Ok("phonon-2"),
        WHISPERX_BACKEND_NAME => Ok("large-v3"),
        other => Err(asr_error(format!("unsupported ASR backend {other:?}"))),
    }
}

pub fn fetch_asr_model(
    backend: &str,
    model: Option<&str>,
    executable: Option<&Path>,
) -> Result<AsrModelLocation> {
    let model = model.unwrap_or(default_model_for_backend(backend)?);

    match backend {
        WHISPER_CANDLE_ENGINE_NAME => {
            let which: WhichModel = model.parse().map_err(|error| {
                asr_error(format!("invalid Whisper model {model:?}: {error}"))
            })?;
            eprintln!("[asr] model download/cache check started backend={backend} model={model}");
            let files = fetch_whisper_model(which).map_err(|error| {
                asr_error(format!("failed to fetch Whisper model {model:?}: {error}"))
            })?;
            let config_dir = files.config.parent().ok_or_else(|| {
                asr_error("Whisper config path has no parent directory")
            })?;
            let weights_dir = files.weights.parent().ok_or_else(|| {
                asr_error("Whisper weights path has no parent directory")
            })?;
            if config_dir != weights_dir {
                return Err(asr_error(format!(
                    "Whisper model files landed in different directories: {} and {}",
                    config_dir.display(),
                    weights_dir.display()
                )));
            }
            eprintln!(
                "[asr] model available backend={} model={} directory={}",
                backend,
                model,
                config_dir.display()
            );
            Ok(AsrModelLocation {
                backend: backend.to_owned(),
                model: model.to_owned(),
                directory: config_dir.to_path_buf(),
            })
        }
        PHONON2_BACKEND_NAME => {
            let executable = executable.unwrap_or_else(|| Path::new("fermion"));
            eprintln!(
                "[asr] model download/cache check started backend={} model={} executable={}",
                backend,
                model,
                executable.display()
            );
            let output = Command::new(executable)
                .arg("transcribe")
                .arg(model)
                .arg("--download-only")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .output()
                .map_err(|error| {
                    asr_error(format!(
                        "failed to start Phonon CLI {:?}: {error}; install fermion-research or pass an explicit executable",
                        executable
                    ))
                })?;
            if !output.status.success() {
                return Err(asr_error(format!(
                    "Phonon model download failed with status {}",
                    output.status
                )));
            }
            let stdout = String::from_utf8(output.stdout).map_err(|error| {
                asr_error(format!("Phonon download output was not UTF-8: {error}"))
            })?;
            let directory = PathBuf::from(stdout.trim());
            if stdout.trim().is_empty() || !directory.is_dir() {
                return Err(asr_error(format!(
                    "Phonon download did not return a valid model directory: {:?}",
                    stdout.trim()
                )));
            }
            eprintln!(
                "[asr] model available backend={} model={} directory={}",
                backend,
                model,
                directory.display()
            );
            Ok(AsrModelLocation {
                backend: backend.to_owned(),
                model: model.to_owned(),
                directory,
            })
        }
        WHISPERX_BACKEND_NAME => {
            let python = whisperx_python_executable(executable);
            let script = r#"
import sys
from faster_whisper.utils import _MODELS
from huggingface_hub import snapshot_download

model = sys.argv[1]
repo = model if "/" in model else _MODELS.get(model)
if repo is None:
    raise SystemExit(f"unknown faster-whisper model: {model}")
path = snapshot_download(
    repo_id=repo,
    allow_patterns=[
        "config.json",
        "preprocessor_config.json",
        "model.bin",
        "tokenizer.json",
        "vocabulary.*",
    ],
)
print(path)
"#;
            eprintln!(
                "[asr] model download/cache check started backend={} model={} python={}",
                backend,
                model,
                python.display()
            );
            let output = Command::new(&python)
                .arg("-c")
                .arg(script)
                .arg(model)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .output()
                .map_err(|error| {
                    asr_error(format!(
                        "failed to start WhisperX Python environment {:?}: {error}; install whisperx/faster-whisper or use --executable with the WhisperX venv binary",
                        python
                    ))
                })?;
            if !output.status.success() {
                return Err(asr_error(format!(
                    "WhisperX model download failed with status {}",
                    output.status
                )));
            }
            let stdout = String::from_utf8(output.stdout).map_err(|error| {
                asr_error(format!("WhisperX model download output was not UTF-8: {error}"))
            })?;
            let directory = PathBuf::from(stdout.trim());
            if stdout.trim().is_empty() || !directory.is_dir() {
                return Err(asr_error(format!(
                    "WhisperX model download did not return a valid model directory: {:?}",
                    stdout.trim()
                )));
            }
            eprintln!(
                "[asr] model available backend={} model={} directory={}",
                backend,
                model,
                directory.display()
            );
            Ok(AsrModelLocation {
                backend: backend.to_owned(),
                model: model.to_owned(),
                directory,
            })
        }
        other => Err(asr_error(format!("unsupported ASR backend {other:?}"))),
    }
}

fn whisperx_python_executable(executable: Option<&Path>) -> PathBuf {
    if let Some(executable) = executable
        && let Some(parent) = executable.parent()
    {
        let python = parent.join("python");
        if python.is_file() {
            return python;
        }
        let python3 = parent.join("python3");
        if python3.is_file() {
            return python3;
        }
    }
    PathBuf::from("python3")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsrConfig {
    pub backend: String,
    pub model: String,
    pub model_dir: Option<PathBuf>,
    pub executable: Option<PathBuf>,
    pub device: String,
    pub language: Option<String>,
    pub word_timestamps: bool,
    pub diarize: bool,
    pub diarization_model: String,
    pub min_speakers: Option<usize>,
    pub max_speakers: Option<usize>,
    pub speaker_embeddings: bool,
    pub hf_token_env: String,
}

impl Default for AsrConfig {
    fn default() -> Self {
        Self {
            backend: WHISPER_CANDLE_ENGINE_NAME.into(),
            model: "small".into(),
            model_dir: None,
            executable: None,
            device: "cpu".into(),
            language: None,
            word_timestamps: false,
            diarize: false,
            diarization_model: DEFAULT_DIARIZATION_MODEL.into(),
            min_speakers: None,
            max_speakers: None,
            speaker_embeddings: false,
            hf_token_env: "HF_TOKEN".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct AsrSegment {
    start_seconds: f64,
    text: String,
}

pub trait LocalAsrBackend: Send + Sync {
    fn engine_name(&self) -> &'static str;
    fn model_name(&self) -> &str;
    fn transcribe_path(&mut self, path: &Path) -> Result<TranscriptCandidate>;
}

pub enum LoadedAsrBackend {
    WhisperCandle(WhisperCandleBackend),
    Phonon2(Phonon2Backend),
    WhisperX(WhisperXBackend),
}

impl LoadedAsrBackend {
    pub fn load(config: AsrConfig) -> Result<Self> {
        match config.backend.as_str() {
            WHISPER_CANDLE_ENGINE_NAME => {
                Ok(Self::WhisperCandle(WhisperCandleBackend::load(config)?))
            }
            WHISPERX_BACKEND_NAME => Ok(Self::WhisperX(WhisperXBackend::load(config)?)),
            PHONON2_BACKEND_NAME => Ok(Self::Phonon2(Phonon2Backend::load(config)?)),
            other => Err(asr_error(format!("unsupported ASR backend {other:?}"))),
        }
    }

    pub fn engine_name(&self) -> &'static str {
        match self {
            Self::WhisperCandle(backend) => backend.engine_name(),
            Self::Phonon2(backend) => backend.engine_name(),
            Self::WhisperX(backend) => backend.engine_name(),
        }
    }

    pub fn model_name(&self) -> &str {
        match self {
            Self::WhisperCandle(backend) => backend.model_name(),
            Self::Phonon2(backend) => backend.model_name(),
            Self::WhisperX(backend) => backend.model_name(),
        }
    }

    pub fn transcribe_path(&mut self, path: &Path) -> Result<TranscriptCandidate> {
        match self {
            Self::WhisperCandle(backend) => backend.transcribe_path(path),
            Self::Phonon2(backend) => backend.transcribe_path(path),
            Self::WhisperX(backend) => backend.transcribe_path(path),
        }
    }
}

pub struct WhisperCandleBackend {
    config: AsrConfig,
    model: WhisperModel,
}

impl WhisperCandleBackend {
    pub fn load(config: AsrConfig) -> Result<Self> {
        if config.diarize {
            return Err(asr_error(
                "whisper-candle does not support speaker diarization; use --asr-backend whisperx",
            ));
        }
        let started = Instant::now();
        eprintln!(
            "[asr] model load started engine={} model={} model_dir={} device={}",
            WHISPER_CANDLE_ENGINE_NAME,
            config.model,
            config
                .model_dir
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<automatic-cache>".into()),
            config.device,
        );

        let device = device(&config.device).map_err(|error| {
            asr_error(format!(
                "failed to initialize ASR device {:?}: {error}",
                config.device
            ))
        })?;

        let mut model = if let Some(model_dir) = config.model_dir.as_deref() {
            if !model_dir.is_dir() {
                return Err(asr_error(format!(
                    "ASR model directory does not exist: {}",
                    model_dir.display()
                )));
            }
            let config_path = model_dir.join("config.json");
            let weights_path = model_dir.join("model.safetensors");
            if !config_path.is_file() {
                return Err(asr_error(format!(
                    "ASR model directory is missing config.json: {}",
                    model_dir.display()
                )));
            }
            if !weights_path.is_file() {
                return Err(asr_error(format!(
                    "ASR model directory is missing model.safetensors: {}",
                    model_dir.display()
                )));
            }
            WhisperModel::load(&config_path, &weights_path, &device).map_err(|error| {
                asr_error(format!(
                    "failed to load local Whisper model from {}: {error}",
                    model_dir.display()
                ))
            })?
        } else {
            load_model(&config.model, &device).map_err(|error| {
                asr_error(format!(
                    "failed to load Whisper model {:?}: {error}",
                    config.model
                ))
            })?
        };

        if let Some(model_dir) = config.model_dir.as_deref() {
            let generation_config = model_dir.join("generation_config.json");
            if generation_config.is_file() {
                model
                    .set_alignment_heads_from_file(&generation_config)
                    .map_err(|error| {
                        asr_error(format!(
                            "failed to load Whisper generation config {}: {error}",
                            generation_config.display()
                        ))
                    })?;
            }
        }

        eprintln!(
            "[asr] model load completed engine={} model={} elapsed={:.1}s",
            WHISPER_CANDLE_ENGINE_NAME,
            config.model,
            started.elapsed().as_secs_f64(),
        );
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
        // whisper-candle-core uses Some(false) for progress-only output.
        // Never leave a long-running transcription completely silent.
        options.verbose = Some(false);

        let started = Instant::now();
        eprintln!(
            "[asr] transcription started engine={} model={} input={}",
            WHISPER_CANDLE_ENGINE_NAME,
            self.config.model,
            path.display(),
        );

        let result = transcribe_file(&mut self.model, path, &options).map_err(|error| {
            asr_error(format!(
                "Whisper transcription failed for {}: {error}",
                path.display()
            ))
        })?;

        eprintln!(
            "[asr] transcription completed engine={} model={} elapsed={:.1}s segments={}",
            WHISPER_CANDLE_ENGINE_NAME,
            self.config.model,
            started.elapsed().as_secs_f64(),
            result.segments.len(),
        );

        let segments = result
            .segments
            .into_iter()
            .map(|segment| AsrSegment {
                start_seconds: segment.start,
                text: segment.text,
            })
            .collect::<Vec<_>>();

        candidate_from_segments(
            WHISPER_CANDLE_ENGINE_NAME,
            &self.config.model,
            Some(result.language),
            segments,
        )
    }
}

impl LocalAsrBackend for WhisperCandleBackend {
    fn engine_name(&self) -> &'static str {
        WHISPER_CANDLE_ENGINE_NAME
    }

    fn model_name(&self) -> &str {
        &self.config.model
    }

    fn transcribe_path(&mut self, path: &Path) -> Result<TranscriptCandidate> {
        WhisperCandleBackend::transcribe_path(self, path)
    }
}

pub struct WhisperXBackend {
    config: AsrConfig,
}

impl WhisperXBackend {
    pub fn load(config: AsrConfig) -> Result<Self> {
        if !config.diarize
            && (config.min_speakers.is_some()
                || config.max_speakers.is_some()
                || config.speaker_embeddings)
        {
            return Err(asr_error(
                "WhisperX speaker-count and speaker-embedding options require --diarize",
            ));
        }
        if config.diarize
            && config
                .min_speakers
                .zip(config.max_speakers)
                .is_some_and(|(min, max)| min > max)
        {
            return Err(asr_error(
                "WhisperX minimum speaker count cannot exceed maximum speaker count",
            ));
        }
        Ok(Self { config })
    }

    fn executable(&self) -> &Path {
        self.config
            .executable
            .as_deref()
            .unwrap_or_else(|| Path::new("whisperx"))
    }

    pub fn transcribe_path(&mut self, path: &Path) -> Result<TranscriptCandidate> {
        if !path.is_file() {
            return Err(asr_error(format!(
                "ASR input does not exist or is not a file: {}",
                path.display()
            )));
        }

        let parent = path.parent().ok_or_else(|| {
            asr_error(format!("WhisperX input has no parent directory: {}", path.display()))
        })?;
        let output_dir = parent.join("whisperx-output");
        fs::create_dir_all(&output_dir).map_err(|error| {
            asr_error(format!(
                "failed to create WhisperX output directory {}: {error}",
                output_dir.display()
            ))
        })?;

        let stem = path.file_stem().and_then(|value| value.to_str()).ok_or_else(|| {
            asr_error(format!("WhisperX input has no UTF-8 file stem: {}", path.display()))
        })?;
        let output_json = output_dir.join(format!("{stem}.json"));
        if output_json.exists() {
            fs::remove_file(&output_json).map_err(|error| {
                asr_error(format!(
                    "failed to remove stale WhisperX output {}: {error}",
                    output_json.display()
                ))
            })?;
        }

        let mut command = Command::new(self.executable());
        command
            .arg(path)
            .arg("--model")
            .arg(&self.config.model)
            .arg("--device")
            .arg(&self.config.device)
            .arg("--output_dir")
            .arg(&output_dir)
            .arg("--output_format")
            .arg("json")
            .arg("--verbose")
            .arg("True")
            .arg("--print_progress")
            .arg("True")
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        if self.config.device == "cpu" {
            command.arg("--compute_type").arg("int8");
        }
        if let Some(language) = self.config.language.as_deref() {
            command.arg("--language").arg(language);
        }
        if let Some(model_dir) = self.config.model_dir.as_deref() {
            command
                .arg("--model_dir")
                .arg(model_dir)
                .arg("--model_cache_only")
                .arg("True");
        }

        if self.config.diarize {
            command
                .arg("--diarize")
                .arg("--diarize_model")
                .arg(&self.config.diarization_model);
            if let Some(min) = self.config.min_speakers {
                command.arg("--min_speakers").arg(min.to_string());
            }
            if let Some(max) = self.config.max_speakers {
                command.arg("--max_speakers").arg(max.to_string());
            }
            if self.config.speaker_embeddings {
                command.arg("--speaker_embeddings");
            }
            if let Ok(token) = std::env::var(&self.config.hf_token_env)
                && !token.trim().is_empty()
            {
                command.arg("--hf_token").arg(token);
            }
        }

        let started = Instant::now();
        eprintln!(
            "[asr] transcription started engine={} model={} diarize={} input={}",
            WHISPERX_ENGINE_NAME,
            self.config.model,
            self.config.diarize,
            path.display(),
        );
        let status = command.status().map_err(|error| {
            asr_error(format!(
                "failed to start WhisperX executable {:?}: {error}; install whisperx or use --asr-executable",
                self.executable()
            ))
        })?;
        if !status.success() {
            return Err(asr_error(format!(
                "WhisperX exited unsuccessfully with status {status}"
            )));
        }

        let raw = fs::read_to_string(&output_json).map_err(|error| {
            asr_error(format!(
                "failed to read WhisperX JSON output {}: {error}",
                output_json.display()
            ))
        })?;
        let candidate = parse_whisperx_json(&raw, &self.config)?;
        if self.config.diarize {
            let evidence_path = persist_whisperx_speaker_evidence(path, &raw, &self.config)?;
            eprintln!(
                "[asr] speaker evidence persisted path={}",
                evidence_path.display()
            );
        }

        eprintln!(
            "[asr] transcription completed engine={} model={} diarize={} elapsed={:.1}s segments={}",
            WHISPERX_ENGINE_NAME,
            self.config.model,
            self.config.diarize,
            started.elapsed().as_secs_f64(),
            candidate.segments.len(),
        );
        Ok(candidate)
    }
}

impl LocalAsrBackend for WhisperXBackend {
    fn engine_name(&self) -> &'static str {
        WHISPERX_ENGINE_NAME
    }

    fn model_name(&self) -> &str {
        &self.config.model
    }

    fn transcribe_path(&mut self, path: &Path) -> Result<TranscriptCandidate> {
        WhisperXBackend::transcribe_path(self, path)
    }
}

fn parse_whisperx_json(raw: &str, config: &AsrConfig) -> Result<TranscriptCandidate> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|error| asr_error(format!("invalid WhisperX JSON output: {error}")))?;
    let language = value
        .get("language")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| config.language.clone());

    let raw_segments = value
        .get("segments")
        .and_then(Value::as_array)
        .ok_or_else(|| asr_error("WhisperX JSON output is missing segments"))?;
    let mut segments = Vec::new();
    for segment in raw_segments {
        let start_seconds = segment
            .get("start")
            .and_then(Value::as_f64)
            .ok_or_else(|| asr_error("WhisperX segment is missing start timestamp"))?;
        if !start_seconds.is_finite() || start_seconds < 0.0 {
            return Err(asr_error(format!(
                "WhisperX emitted invalid segment timestamp {start_seconds}"
            )));
        }
        let text = segment
            .get("text")
            .and_then(Value::as_str)
            .map(normalize_text)
            .unwrap_or_default();
        if text.is_empty() {
            continue;
        }
        let speaker = segment
            .get("speaker")
            .and_then(Value::as_str)
            .filter(|label| !label.trim().is_empty())
            .map(|label| TranscriptSpeaker {
                diarization_label: label.to_owned(),
                identity: None,
                attribution: SpeakerAttribution::Unresolved,
            });

        segments.push(TranscriptSegment {
            start_seconds: Some(start_seconds.floor() as u64),
            text,
            speaker,
        });
    }

    let candidate = TranscriptCandidate {
        derivation: TranscriptDerivation::LocalAsr,
        language,
        timestamps: true,
        engine: Some(WHISPERX_ENGINE_NAME.into()),
        model: Some(config.model.clone()),
        diarization: config.diarize.then(|| DiarizationProvenance {
            engine: "pyannote-audio".into(),
            model: config.diarization_model.clone(),
            registry_revision: None,
        }),
        segments,
    };
    candidate.validate()?;
    Ok(candidate)
}

fn persist_whisperx_speaker_evidence(
    input: &Path,
    raw: &str,
    config: &AsrConfig,
) -> Result<PathBuf> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|error| asr_error(format!("invalid WhisperX JSON output: {error}")))?;

    let video_dir = input.parent().ok_or_else(|| {
        asr_error(format!(
            "WhisperX input has no parent directory: {}",
            input.display()
        ))
    })?;
    let video_id = video_dir
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            asr_error(format!(
                "WhisperX input parent has no UTF-8 video id: {}",
                video_dir.display()
            ))
        })?;
    let vessel_cache = video_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| {
            asr_error(format!(
                "cannot resolve Vessel cache root from {}",
                input.display()
            ))
        })?;
    let evidence_dir = vessel_cache.join("speaker-evidence");
    fs::create_dir_all(&evidence_dir).map_err(|error| {
        asr_error(format!(
            "failed to create speaker evidence directory {}: {error}",
            evidence_dir.display()
        ))
    })?;

    let segments = value
        .get("segments")
        .and_then(Value::as_array)
        .map(|segments| {
            segments
                .iter()
                .filter_map(|segment| {
                    let speaker = segment.get("speaker")?.as_str()?;
                    let start = segment.get("start")?.as_f64()?;
                    let end = segment.get("end")?.as_f64()?;
                    Some(serde_json::json!({
                        "start": start,
                        "end": end,
                        "speaker": speaker,
                    }))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let embeddings = if config.speaker_embeddings {
        value
            .get("speaker_embeddings")
            .cloned()
            .unwrap_or(Value::Null)
    } else {
        Value::Null
    };

    let evidence = serde_json::json!({
        "schema": 1,
        "video_id": video_id,
        "asr": {
            "engine": WHISPERX_ENGINE_NAME,
            "model": config.model,
        },
        "diarization": {
            "engine": "pyannote-audio",
            "model": config.diarization_model,
            "label_scope": "file_local",
        },
        "segments": segments,
        "speaker_embeddings": embeddings,
    });
    let rendered = serde_json::to_vec_pretty(&evidence)
        .map_err(|error| asr_error(format!("speaker evidence serialization failed: {error}")))?;
    let path = evidence_dir.join(format!("{video_id}.json"));
    let temp = evidence_dir.join(format!("{video_id}.json.tmp"));
    fs::write(&temp, rendered).map_err(|error| {
        asr_error(format!(
            "failed to write speaker evidence temp file {}: {error}",
            temp.display()
        ))
    })?;
    fs::rename(&temp, &path).map_err(|error| {
        asr_error(format!(
            "failed to finalize speaker evidence {}: {error}",
            path.display()
        ))
    })?;
    Ok(path)
}

pub struct Phonon2Backend {
    config: AsrConfig,
}

impl Phonon2Backend {
    pub fn load(config: AsrConfig) -> Result<Self> {
        if config.diarize {
            return Err(asr_error(
                "phonon-2 does not support speaker diarization; use --asr-backend whisperx",
            ));
        }
        if let Some(language) = config.language.as_deref()
            && !language.eq_ignore_ascii_case("en")
            && !language.to_ascii_lowercase().starts_with("en-")
        {
            return Err(asr_error(format!(
                "Phonon-2 is English-only; unsupported requested language {language:?}"
            )));
        }
        if config.device != "cpu" {
            return Err(asr_error(format!(
                "Phonon-2 CLI backend currently supports Vessel device=\"cpu\" only; got {:?}",
                config.device
            )));
        }
        if let Some(model_dir) = config.model_dir.as_deref()
            && !model_dir.is_dir()
        {
            return Err(asr_error(format!(
                "Phonon model directory does not exist: {}",
                model_dir.display()
            )));
        }
        Ok(Self { config })
    }

    fn executable(&self) -> &Path {
        self.config
            .executable
            .as_deref()
            .unwrap_or_else(|| Path::new("fermion"))
    }

    fn model_argument(&self) -> String {
        self.config
            .model_dir
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| self.config.model.clone())
    }

    pub fn transcribe_path(&mut self, path: &Path) -> Result<TranscriptCandidate> {
        if !path.is_file() {
            return Err(asr_error(format!(
                "ASR input does not exist or is not a file: {}",
                path.display()
            )));
        }

        let model_argument = self.model_argument();
        let started = Instant::now();
        eprintln!(
            "[asr] transcription started engine={} model={} input={}",
            PHONON_ENGINE_NAME,
            model_argument,
            path.display(),
        );

        let child = Command::new(self.executable())
            .arg("transcribe")
            .arg(&model_argument)
            .arg(path)
            .arg("--json")
            .arg("--verbose")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| {
                asr_error(format!(
                    "failed to start Phonon CLI {:?}: {error}; install fermion-research or use --asr-executable",
                    self.executable()
                ))
            })?;

        let output = child
            .wait_with_output()
            .map_err(|error| asr_error(format!("failed while waiting for Phonon CLI: {error}")))?;
        if !output.status.success() {
            return Err(asr_error(format!(
                "Phonon CLI exited unsuccessfully with status {}",
                output.status
            )));
        }

        let raw = String::from_utf8(output.stdout)
            .map_err(|error| asr_error(format!("Phonon JSON output was not UTF-8: {error}")))?;
        let candidate = parse_phonon_json(&raw, &self.config.model)?;

        eprintln!(
            "[asr] transcription completed engine={} model={} elapsed={:.1}s segments={}",
            candidate.engine.as_deref().unwrap_or(PHONON_ENGINE_NAME),
            candidate.model.as_deref().unwrap_or(&self.config.model),
            started.elapsed().as_secs_f64(),
            candidate.segments.len(),
        );

        Ok(candidate)
    }
}

impl LocalAsrBackend for Phonon2Backend {
    fn engine_name(&self) -> &'static str {
        PHONON_ENGINE_NAME
    }

    fn model_name(&self) -> &str {
        &self.config.model
    }

    fn transcribe_path(&mut self, path: &Path) -> Result<TranscriptCandidate> {
        Phonon2Backend::transcribe_path(self, path)
    }
}

fn parse_phonon_json(raw: &str, requested_model: &str) -> Result<TranscriptCandidate> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|error| asr_error(format!("invalid Phonon JSON output: {error}")))?;

    if value
        .get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(asr_error(
            "Phonon reported truncated output; refusing to materialize a partial transcript",
        ));
    }

    let runtime_backend = value
        .get("backend")
        .and_then(Value::as_str)
        .unwrap_or("phonon");
    let runtime_engine = value
        .get("engine")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let provenance_engine = format!("fermion-{runtime_backend}-{runtime_engine}");

    let model = value
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or(requested_model);

    let segments = value
        .get("segments")
        .and_then(Value::as_array)
        .ok_or_else(|| asr_error("Phonon JSON output is missing segments"))?
        .iter()
        .filter_map(|segment| {
            let start_seconds = segment.get("start")?.as_f64()?;
            let text = segment.get("text")?.as_str()?.to_owned();
            Some(AsrSegment {
                start_seconds,
                text,
            })
        })
        .collect::<Vec<_>>();

    candidate_from_segments(&provenance_engine, model, Some("en".into()), segments)
}

fn candidate_from_segments(
    engine: &str,
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
                "{engine} emitted invalid segment timestamp {}",
                segment.start_seconds
            )));
        }
        normalized.push(TranscriptSegment {
            start_seconds: Some(segment.start_seconds.floor() as u64),
            text,
            speaker: None,
        });
    }

    let candidate = TranscriptCandidate {
        derivation: TranscriptDerivation::LocalAsr,
        language: language.filter(|language| !language.trim().is_empty()),
        timestamps: true,
        engine: Some(engine.to_owned()),
        model: Some(model.to_owned()),
        diarization: None,
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
    fn whisperx_python_prefers_sibling_virtualenv_interpreter() {
        let root = std::env::temp_dir().join(format!(
            "vessel-whisperx-python-{}",
            std::process::id()
        ));
        let bin = root.join("bin");
        fs::create_dir_all(&bin).expect("bin");
        fs::write(bin.join("python"), b"").expect("python marker");

        assert_eq!(
            whisperx_python_executable(Some(&bin.join("whisperx"))),
            bin.join("python")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn backend_default_models_are_explicit() {
        assert_eq!(
            default_model_for_backend(WHISPER_CANDLE_ENGINE_NAME).unwrap(),
            "small"
        );
        assert_eq!(
            default_model_for_backend(PHONON2_BACKEND_NAME).unwrap(),
            "phonon-2"
        );
        assert_eq!(default_model_for_backend("whisperx").unwrap(), "large-v3");
        assert!(default_model_for_backend("unknown").is_err());
    }

    #[test]
    fn loaded_backend_is_safe_to_move_to_blocking_worker() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<WhisperCandleBackend>();
    }

    #[test]
    fn defaults_are_cpu_first_and_multilingual() {
        let config = AsrConfig::default();
        assert_eq!(config.backend, WHISPER_CANDLE_ENGINE_NAME);
        assert_eq!(config.device, "cpu");
        assert_eq!(config.model, "small");
        assert_eq!(config.model_dir, None);
        assert_eq!(config.executable, None);
        assert_eq!(config.language, None);
        assert!(!config.word_timestamps);
        assert!(!config.diarize);
        assert_eq!(config.diarization_model, DEFAULT_DIARIZATION_MODEL);
        assert_eq!(config.min_speakers, None);
        assert_eq!(config.max_speakers, None);
        assert!(!config.speaker_embeddings);
        assert_eq!(config.hf_token_env, "HF_TOKEN");
    }

    #[test]
    fn loaded_backend_wrapper_is_safe_to_move_to_blocking_worker() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<LoadedAsrBackend>();
    }

    #[test]
    fn non_diarizing_backends_refuse_diarization_requests() {
        let mut whisper = AsrConfig::default();
        whisper.diarize = true;
        let error = WhisperCandleBackend::load(whisper)
            .err()
            .expect("whisper-candle diarization must fail explicitly");
        assert!(error.to_string().contains("does not support speaker diarization"));

        let mut phonon = AsrConfig::default();
        phonon.backend = PHONON2_BACKEND_NAME.into();
        phonon.model = PHONON2_BACKEND_NAME.into();
        phonon.diarize = true;
        let error = Phonon2Backend::load(phonon)
            .err()
            .expect("phonon diarization must fail explicitly");
        assert!(error.to_string().contains("does not support speaker diarization"));
    }

    #[test]
    fn whisperx_speaker_controls_require_diarization() {
        let mut config = AsrConfig::default();
        config.backend = WHISPERX_BACKEND_NAME.into();
        config.speaker_embeddings = true;
        let error = WhisperXBackend::load(config)
            .err()
            .expect("speaker embedding request without diarization must fail");
        assert!(error.to_string().contains("require --diarize"));
    }

    #[test]
    fn speaker_evidence_path_is_outside_disposable_video_asr_directory() {
        let root = std::env::temp_dir().join(format!(
            "vessel-speaker-evidence-{}",
            std::process::id()
        ));
        let video_dir = root.join(".cache/vessel/asr/video123");
        fs::create_dir_all(&video_dir).expect("video dir");
        let input = video_dir.join("whisper-input.wav");
        fs::write(&input, b"").expect("input");

        let mut config = AsrConfig::default();
        config.backend = WHISPERX_BACKEND_NAME.into();
        config.model = "large-v3".into();
        config.diarize = true;
        config.speaker_embeddings = true;

        let raw = r#"{
          "segments": [
            {"start": 1.0, "end": 2.0, "text": "hello", "speaker": "SPEAKER_00"}
          ],
          "speaker_embeddings": {
            "SPEAKER_00": [0.1, 0.2, 0.3]
          }
        }"#;
        let path =
            persist_whisperx_speaker_evidence(&input, raw, &config).expect("speaker evidence");
        assert_eq!(
            path,
            root.join(".cache/vessel/speaker-evidence/video123.json")
        );
        assert!(path.is_file());
        let persisted: Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(persisted["video_id"], "video123");
        assert_eq!(persisted["segments"][0]["speaker"], "SPEAKER_00");
        assert_eq!(
            persisted["speaker_embeddings"]["SPEAKER_00"][1]
                .as_f64()
                .unwrap(),
            0.2
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn parses_whisperx_diarized_segments_without_inventing_identity() {
        let mut config = AsrConfig::default();
        config.backend = WHISPERX_BACKEND_NAME.into();
        config.model = "large-v3".into();
        config.diarize = true;

        let raw = r#"{
          "language": "en",
          "segments": [
            {"start": 1.25, "end": 4.0, "text": " First voice. ", "speaker": "SPEAKER_00"},
            {"start": 4.1, "end": 8.0, "text": "Second voice.", "speaker": "SPEAKER_01"}
          ]
        }"#;

        let candidate = parse_whisperx_json(raw, &config).expect("parse whisperx");
        assert_eq!(candidate.engine.as_deref(), Some(WHISPERX_ENGINE_NAME));
        assert_eq!(candidate.model.as_deref(), Some("large-v3"));
        assert_eq!(candidate.language.as_deref(), Some("en"));
        assert_eq!(
            candidate.diarization.as_ref().map(|value| value.engine.as_str()),
            Some("pyannote-audio")
        );
        let first = candidate.segments[0].speaker.as_ref().expect("speaker");
        assert_eq!(first.diarization_label, "SPEAKER_00");
        assert_eq!(first.identity, None);
        assert_eq!(first.attribution, SpeakerAttribution::Unresolved);
    }

    #[test]
    fn parses_phonon_json_with_runtime_provenance() {
        let raw = r#"{
          "model": "FermionResearch/Phonon-2",
          "backend": "phonon2-five-value",
          "engine": "cpu",
          "segments": [
            {"id": 0, "start": 0.0, "end": 4.2, "text": "Hello world."},
            {"id": 1, "start": 5.1, "end": 8.0, "text": "Second segment."}
          ],
          "truncated": false
        }"#;

        let candidate = parse_phonon_json(raw, "phonon-2").expect("parse phonon");
        assert_eq!(
            candidate.engine.as_deref(),
            Some("fermion-phonon2-five-value-cpu")
        );
        assert_eq!(candidate.model.as_deref(), Some("FermionResearch/Phonon-2"));
        assert_eq!(candidate.language.as_deref(), Some("en"));
        assert_eq!(candidate.segments.len(), 2);
        assert_eq!(candidate.segments[1].start_seconds, Some(5));
    }

    #[test]
    fn refuses_truncated_phonon_output() {
        let raw = r#"{"segments":[],"truncated":true}"#;
        let error = parse_phonon_json(raw, "phonon-2").expect_err("truncated output must fail");
        assert!(error.to_string().contains("truncated"));
    }

    #[test]
    fn phonon_rejects_non_english_requests_before_execution() {
        let mut config = AsrConfig::default();
        config.backend = PHONON2_BACKEND_NAME.into();
        config.model = PHONON2_BACKEND_NAME.into();
        config.language = Some("es".into());
        let error = Phonon2Backend::load(config)
            .err()
            .expect("non-English Phonon configuration must fail");
        assert!(error.to_string().contains("English-only"));
    }

    #[test]
    fn whisper_segments_become_sourcearium_ready_candidate() {
        let candidate = candidate_from_segments(
            WHISPER_CANDLE_ENGINE_NAME,
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
        assert_eq!(
            candidate.engine.as_deref(),
            Some(WHISPER_CANDLE_ENGINE_NAME)
        );
        assert_eq!(candidate.model.as_deref(), Some("small"));
        assert_eq!(candidate.language.as_deref(), Some("en"));
        assert_eq!(candidate.segments[0].start_seconds, Some(3));
        assert_eq!(candidate.segments[0].text, "hello world");
    }

    #[test]
    fn invalid_timestamps_are_rejected() {
        let error = candidate_from_segments(
            WHISPER_CANDLE_ENGINE_NAME,
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
