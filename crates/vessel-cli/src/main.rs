use std::collections::HashSet;
use std::collections::HashMap;
use std::env;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use clap::{ArgAction, Args, Parser, Subcommand};
use reqwest::{Client, StatusCode};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use tokio::io::AsyncWriteExt;
use tracing::{debug, info, warn};
use vessel_asr::{AsrConfig, LoadedAsrBackend};
use vessel_core::models::{ChannelMetadata, InputKind, InputRef, VideoMetadata};
use vessel_core::{
    ChannelCategoryConfig, Config, MaterializeStatus, Result,
    RuntimeLayout, SourceariumYoutubeSource, TranscriptCandidate, TranscriptDerivation, VesselError,
    VideoSelection, apply_sourcearium_prune, apply_youtube_transcript_diarization, discover_youtube_sources,
    inventory_sourcearium_repository, load_config, load_speaker_evidence,
    load_youtube_transcript_artifact,
    materialize_youtube_transcript, plan_sourcearium_prune,
    refresh_youtube_transcript_diarization_provenance, resolve_runtime_layout, validate_sourcearium_repository, };
use vessel_diarization::{
    DEFAULT_CLUSTERING_THRESHOLD, DEFAULT_WINDOW_SHIFT_RATIO, DiarizationConfig, DiarizationResult,
    DiarizationSegment, SHERPA_ONNX_BACKEND_NAME, SHERPA_ONNX_RUNTIME_VERSION, SherpaOnnxDiarizer,
    find_sherpa_runtime_library, persist_speaker_evidence, probe_sherpa_runtime,
};
use vessel_download::{BasicDownloadPlanner, DownloadPlanner, execute_download};
use vessel_extractors::youtube::{
    ChannelTabCursor, ChannelVideoCrawlReport, ChannelVideoRef, YT_DLP_BACKEND_NAME, YtDlpConfig,
    YoutubeExtractor,
    acquire_best_caption_candidate, crawl_channel_videos, extract_channel, extract_comments,
    extract_video,
};
use vessel_extractors::{
    ExtractContext, ExtractRequest, ExtractedItem, ExtractorRegistry, PluginCatalog, load_plugins,
};
use vessel_formats::{FormatSelector, parse_selector};
use vessel_ledger::{AttemptStatus, FetchAttempt, Ledger};
use vessel_postprocess::{PostprocessRequest, build_plan, execute_plan};
use vessel_store::{init_sqlite_database, init_sqlite_database_path};

#[derive(Debug, Parser)]
#[command(
    name = "vessel",
    version,
    about = "Media acquisition orchestration and Sourcearium text materialization"
)]
struct Cli {
    #[arg(long = "project", global = true)]
    project: Option<String>,
    #[arg(short = 'q', long = "quiet", global = true)]
    quiet: bool,
    #[arg(short = 'v', long = "verbose", global = true, action = ArgAction::Count)]
    verbose: u8,
    #[arg(long = "no-progress", global = true)]
    no_progress: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Doctor,
    Update(UpdateArgs),
    Validate(ValidateArgs),
    Inventory(InventoryArgs),
    Prune(PruneArgs),
    Config(ConfigCommand),
    Dataset(DatasetCommand),
    Channel(ChannelCommand),
    Video(VideoCommand),
    Project(ProjectCommand),
    Asr(AsrCommand),
    Diarization(DiarizationCommand),
    Info(UrlArg),
    Formats(UrlArg),
    Download(DownloadArgs),
    Plugin(PluginCommand),
}

#[derive(Debug, Args)]
struct PruneArgs {
    #[arg(long = "sourcearium", default_value = ".")]
    sourcearium: PathBuf,
    #[arg(long)]
    apply: bool,
}

#[derive(Debug, Args)]
struct InventoryArgs {
    #[arg(long = "sourcearium", default_value = ".")]
    sourcearium: PathBuf,
}

#[derive(Debug, Args)]
struct ValidateArgs {
    #[arg(long = "sourcearium", default_value = ".")]
    sourcearium: PathBuf,
}

#[derive(Debug, Args)]
struct UpdateArgs {
    #[arg(long = "sourcearium", default_value = ".")]
    sourcearium: PathBuf,
    #[arg(long = "max-videos")]
    max_videos: Option<usize>,
    #[arg(long = "video-id")]
    video_ids: Vec<String>,
    #[arg(long = "youtube-backend", default_value = "yt-dlp")]
    youtube_backend: String,
    #[arg(long = "yt-dlp-executable", default_value = "yt-dlp")]
    yt_dlp_executable: PathBuf,
    #[arg(long = "force-local-asr")]
    force_local_asr: bool,
    #[arg(long = "asr-backend")]
    asr_backend: Option<String>,
    #[arg(long = "asr-model")]
    asr_model: Option<String>,
    #[arg(long = "asr-model-dir")]
    asr_model_dir: Option<PathBuf>,
    #[arg(long = "asr-executable")]
    asr_executable: Option<PathBuf>,
    #[arg(long = "asr-device")]
    asr_device: Option<String>,
    #[arg(long = "asr-language")]
    asr_language: Option<String>,
    #[arg(long)]
    diarize: bool,
    #[arg(long = "diarization-backend", default_value = "whisperx")]
    diarization_backend: String,
    #[arg(long = "diarization-segmentation-model")]
    diarization_segmentation_model: Option<PathBuf>,
    #[arg(long = "diarization-embedding-model")]
    diarization_embedding_model: Option<PathBuf>,
    #[arg(long = "diarization-runtime-dir")]
    diarization_runtime_dir: Option<PathBuf>,
    #[arg(long = "diarization-provider", default_value = "cpu")]
    diarization_provider: String,
    #[arg(long = "diarization-num-threads", default_value_t = 4)]
    diarization_num_threads: i32,
    #[arg(
        long = "diarization-clustering-threshold",
        default_value_t = DEFAULT_CLUSTERING_THRESHOLD
    )]
    diarization_clustering_threshold: f32,
    #[arg(
        long = "diarization-window-shift-ratio",
        default_value_t = DEFAULT_WINDOW_SHIFT_RATIO
    )]
    diarization_window_shift_ratio: f32,
    #[arg(long = "diarization-model")]
    diarization_model: Option<String>,
    #[arg(long = "min-speakers")]
    min_speakers: Option<usize>,
    #[arg(long = "max-speakers")]
    max_speakers: Option<usize>,
    #[arg(long = "speaker-embeddings")]
    speaker_embeddings: bool,
    #[arg(long = "attribute-speakers")]
    attribute_speakers: bool,
    #[arg(long = "speaker-min-similarity", default_value_t = 0.80)]
    speaker_min_similarity: f64,
    #[arg(long = "speaker-min-margin", default_value_t = 0.05)]
    speaker_min_margin: f64,
    #[arg(long = "speaker-min-anchor-dominance", default_value_t = 0.80)]
    speaker_min_anchor_dominance: f64,
    #[arg(long = "hf-token-env", default_value = "HF_TOKEN")]
    hf_token_env: String,
    #[arg(long = "upgrade-check-days", default_value_t = 30)]
    upgrade_check_days: u64,
    #[arg(long = "report-items")]
    report_items: bool,
    #[arg(long)]
    preview: bool,
}

#[derive(Debug, Args)]
struct UrlArg {
    url: String,
}

#[derive(Debug, Args)]
struct ProjectCommand {
    #[command(subcommand)]
    command: ProjectSubcommand,
}

#[derive(Debug, Subcommand)]
enum ProjectSubcommand {
    List,
}

#[derive(Debug, Args)]
struct AsrCommand {
    #[command(subcommand)]
    command: AsrSubcommand,
}

#[derive(Debug, Subcommand)]
enum AsrSubcommand {
    Models,
    Fetch(AsrFetchArgs),
    Doctor(AsrDoctorArgs),
}

#[derive(Debug, Args)]
struct AsrFetchArgs {
    #[arg(long, default_value = "whisper-candle")]
    backend: String,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    executable: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct AsrDoctorArgs {
    #[arg(long, default_value = "whisper-candle")]
    backend: String,
    #[arg(long)]
    executable: Option<PathBuf>,
    #[arg(long = "model-dir")]
    model_dir: Option<PathBuf>,
    #[arg(long = "hf-token-env", default_value = "HF_TOKEN")]
    hf_token_env: String,
}

#[derive(Debug, Args)]
struct DiarizationCommand {
    #[command(subcommand)]
    command: DiarizationSubcommand,
}

#[derive(Debug, Subcommand)]
enum DiarizationSubcommand {
    Models,
    Fetch(DiarizationFetchArgs),
    Doctor(DiarizationDoctorArgs),
    Seed(DiarizationSeedArgs),
    Run(DiarizationRunArgs),
    Apply(DiarizationApplyArgs),
    Reembed(DiarizationReembedArgs),
}

#[derive(Debug, Args)]
struct DiarizationFetchArgs {
    #[arg(long, default_value = "sherpa-onnx")]
    backend: String,
    #[arg(long = "dir")]
    dir: Option<PathBuf>,
    #[arg(long = "runtime-dir")]
    runtime_dir: Option<PathBuf>,
    #[arg(long = "embedding-profile", default_value = "zh-3dspeaker")]
    embedding_profile: String,
    #[arg(long)]
    plan: bool,
}

#[derive(Debug, Args)]
struct DiarizationDoctorArgs {
    #[arg(long, default_value = "sherpa-onnx")]
    backend: String,
    #[arg(long = "runtime-dir")]
    runtime_dir: Option<PathBuf>,
    #[arg(long = "segmentation-model")]
    segmentation_model: Option<PathBuf>,
    #[arg(long = "embedding-model")]
    embedding_model: Option<PathBuf>,
    #[arg(long = "provider", default_value = "cpu")]
    provider: String,
}

#[derive(Debug, Args)]
struct DiarizationSeedArgs {
    #[arg(long = "video-id")]
    video_id: String,
    #[arg(long)]
    force: bool,
}

#[derive(Debug, Args)]
struct DiarizationRunArgs {
    input: Option<PathBuf>,
    #[arg(long = "fixture-video-id")]
    fixture_video_id: Option<String>,
    #[arg(long, default_value = "sherpa-onnx")]
    backend: String,
    #[arg(long = "runtime-dir")]
    runtime_dir: Option<PathBuf>,
    #[arg(long = "segmentation-model")]
    segmentation_model: Option<PathBuf>,
    #[arg(long = "embedding-model")]
    embedding_model: Option<PathBuf>,
    #[arg(long = "provider", default_value = "cpu")]
    provider: String,
    #[arg(long = "num-threads", default_value_t = 4)]
    num_threads: i32,
    #[arg(long = "num-speakers")]
    num_speakers: Option<usize>,
    #[arg(
        long = "clustering-threshold",
        default_value_t = DEFAULT_CLUSTERING_THRESHOLD
    )]
    clustering_threshold: f32,
    #[arg(
        long = "window-shift-ratio",
        default_value_t = DEFAULT_WINDOW_SHIFT_RATIO
    )]
    window_shift_ratio: f32,
    #[arg(long = "speaker-embeddings")]
    speaker_embeddings: bool,
}

#[derive(Debug, Args)]
struct DiarizationApplyArgs {
    #[arg(long = "sourcearium", default_value = ".")]
    sourcearium: PathBuf,
    #[arg(long = "source-key")]
    source_key: String,
    #[arg(long = "video-id")]
    video_id: String,
    #[arg(long = "runtime-dir")]
    runtime_dir: Option<PathBuf>,
    #[arg(long = "segmentation-model")]
    segmentation_model: Option<PathBuf>,
    #[arg(long = "embedding-model")]
    embedding_model: Option<PathBuf>,
    #[arg(long = "provider", default_value = "cpu")]
    provider: String,
    #[arg(long = "num-threads", default_value_t = 4)]
    num_threads: i32,
    #[arg(long = "num-speakers")]
    num_speakers: Option<usize>,
    #[arg(
        long = "clustering-threshold",
        default_value_t = DEFAULT_CLUSTERING_THRESHOLD
    )]
    clustering_threshold: f32,
    #[arg(
        long = "window-shift-ratio",
        default_value_t = DEFAULT_WINDOW_SHIFT_RATIO
    )]
    window_shift_ratio: f32,
}

#[derive(Debug, Args)]
struct DiarizationReembedArgs {
    #[arg(long = "sourcearium", default_value = ".")]
    sourcearium: PathBuf,
    #[arg(long = "source-key")]
    source_key: String,
    #[arg(long = "video-id")]
    video_id: String,
    #[arg(long = "runtime-dir")]
    runtime_dir: Option<PathBuf>,
    #[arg(long = "segmentation-model")]
    segmentation_model: Option<PathBuf>,
    #[arg(long = "embedding-model")]
    embedding_model: Option<PathBuf>,
    #[arg(long = "provider", default_value = "cpu")]
    provider: String,
    #[arg(long = "num-threads", default_value_t = 4)]
    num_threads: i32,
}

#[derive(Debug, Args)]
struct DownloadArgs {
    url: String,
    #[arg(short = 'f', long = "format")]
    format: Option<String>,
    #[arg(long = "remux-video")]
    remux_video: Option<String>,
    #[arg(short = 'x', long = "extract-audio")]
    extract_audio: bool,
    #[arg(long = "audio-format", default_value = "mp3")]
    audio_format: String,
    #[arg(long = "embed-metadata")]
    embed_metadata: bool,
    #[arg(long = "embed-thumbnail")]
    embed_thumbnail: bool,
    #[arg(long = "subtitles")]
    subtitles: bool,
    #[arg(long = "convert-subs")]
    convert_subs: Option<String>,
}

#[derive(Debug, Args)]
struct PluginCommand {
    #[command(subcommand)]
    command: PluginSubcommand,
}

#[derive(Debug, Subcommand)]
enum PluginSubcommand {
    List,
    Install(PluginInstallArgs),
}

#[derive(Debug, Args)]
struct PluginInstallArgs {
    name: String,
    #[arg(long = "dir")]
    dir: Option<String>,
    #[arg(long = "kind", default_value = "fixture-extractor")]
    kind: String,
}

#[derive(Debug, Args)]
struct ConfigCommand {
    #[command(subcommand)]
    command: ConfigSubcommand,
}

#[derive(Debug, Subcommand)]
enum ConfigSubcommand {
    Show,
}

#[derive(Debug, Args)]
struct DatasetCommand {
    #[command(subcommand)]
    command: DatasetSubcommand,
}

#[derive(Debug, Subcommand)]
enum DatasetSubcommand {
    Init(DatasetInitArgs),
    Destroy(DatasetDestroyArgs),
}

#[derive(Debug, Args)]
struct DatasetInitArgs {
    #[arg(long = "db")]
    db: Option<String>,
    #[arg(long = "recreate")]
    recreate: bool,
}

#[derive(Debug, Args)]
struct DatasetDestroyArgs {
    #[arg(long = "db")]
    db: Option<String>,
    #[arg(long = "yes")]
    yes: bool,
}

#[derive(Debug, Args)]
struct ChannelCommand {
    #[command(subcommand)]
    command: ChannelSubcommand,
}

#[derive(Debug, Subcommand)]
enum ChannelSubcommand {
    Add(ChannelAddArgs),
    Sync(ChannelSyncArgs),
    Config(ChannelConfigCommand),
}

#[derive(Debug, Args)]
struct ChannelAddArgs {
    channel: String,
    #[arg(long)]
    category: Option<String>,
}

#[derive(Debug, Args)]
struct ChannelSyncArgs {
    #[arg(long)]
    full: bool,
    #[arg(long = "metrics-only")]
    metrics_only: bool,
    #[arg(long)]
    comments: bool,
    #[arg(long)]
    subtitles: bool,
    #[arg(long = "download-thumbnails")]
    download_thumbnails: bool,
    #[arg(long)]
    since: Option<String>,
    #[arg(long = "max-videos")]
    max_videos: Option<usize>,
    #[arg(long = "category")]
    categories: Vec<String>,
}

#[derive(Debug, Args)]
struct ChannelConfigCommand {
    #[command(subcommand)]
    command: ChannelConfigSubcommand,
}

#[derive(Debug, Subcommand)]
enum ChannelConfigSubcommand {
    Init,
    Show,
}

#[derive(Debug, Args)]
struct VideoCommand {
    #[command(subcommand)]
    command: VideoSubcommand,
}

#[derive(Debug, Subcommand)]
enum VideoSubcommand {
    Refresh(VideoRefreshArgs),
    History(VideoRefArg),
    Subtitles(VideoSubtitlesCommand),
    Comments(VideoCommentsCommand),
}

#[derive(Debug, Args)]
struct VideoRefArg {
    video: String,
}

#[derive(Debug, Args)]
struct VideoRefreshArgs {
    video: String,
    #[arg(long = "metrics-only")]
    metrics_only: bool,
}

#[derive(Debug, Args)]
struct VideoSubtitlesCommand {
    #[command(subcommand)]
    command: VideoSubtitlesSubcommand,
}

#[derive(Debug, Subcommand)]
enum VideoSubtitlesSubcommand {
    Sync(VideoRefArg),
}

#[derive(Debug, Args)]
struct VideoCommentsCommand {
    #[command(subcommand)]
    command: VideoCommentsSubcommand,
}

#[derive(Debug, Subcommand)]
enum VideoCommentsSubcommand {
    Sync(VideoRefArg),
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let (config, paths, loaded_from) = load_config()?;
    let layout = resolve_runtime_layout(&config, &paths, cli.project.as_deref())?;
    init_logging(&config, &cli)?;
    debug!(target: "cli", project = %layout.project_name, "cli parsed");

    match cli.command {
        Commands::Doctor => doctor(&config, &paths, &loaded_from, &layout).await,
        Commands::Update(args) => sourcearium_update(args, &config).await,
        Commands::Validate(args) => sourcearium_validate(args),
        Commands::Inventory(args) => sourcearium_inventory(args),
        Commands::Prune(args) => sourcearium_prune(args),
        Commands::Config(cmd) => match cmd.command {
            ConfigSubcommand::Show => show_config(&config, &paths, &loaded_from, &layout),
        },
        Commands::Dataset(cmd) => match cmd.command {
            DatasetSubcommand::Init(args) => dataset_init(args, &layout).await,
            DatasetSubcommand::Destroy(args) => dataset_destroy(args, &layout).await,
        },
        Commands::Channel(cmd) => match cmd.command {
            ChannelSubcommand::Add(args) => channel_add(args, &layout).await,
            ChannelSubcommand::Sync(args) => channel_sync(args, &layout).await,
            ChannelSubcommand::Config(cmd) => match cmd.command {
                ChannelConfigSubcommand::Init => channel_config_init(&layout).await,
                ChannelConfigSubcommand::Show => channel_config_show(&layout),
            },
        },
        Commands::Video(cmd) => match cmd.command {
            VideoSubcommand::Refresh(args) => video_refresh(args, &layout, &paths).await,
            VideoSubcommand::History(args) => video_history(args, &layout).await,
            VideoSubcommand::Subtitles(cmd) => match cmd.command {
                VideoSubtitlesSubcommand::Sync(args) => video_subtitles_sync(args, &layout).await,
            },
            VideoSubcommand::Comments(cmd) => match cmd.command {
                VideoCommentsSubcommand::Sync(args) => video_comments_sync(args, &layout).await,
            },
        },
        Commands::Project(cmd) => match cmd.command {
            ProjectSubcommand::List => project_list(&layout).await,
        },
        Commands::Asr(cmd) => match cmd.command {
            AsrSubcommand::Models => asr_models(),
            AsrSubcommand::Fetch(args) => asr_fetch(args),
            AsrSubcommand::Doctor(args) => asr_doctor(args),
        },
        Commands::Diarization(cmd) => match cmd.command {
            DiarizationSubcommand::Models => diarization_models(),
            DiarizationSubcommand::Fetch(args) => diarization_fetch(args).await,
            DiarizationSubcommand::Doctor(args) => diarization_doctor(args),
            DiarizationSubcommand::Seed(args) => diarization_seed(args).await,
            DiarizationSubcommand::Run(args) => diarization_run(args).await,
            DiarizationSubcommand::Apply(args) => diarization_apply(args).await,
            DiarizationSubcommand::Reembed(args) => diarization_reembed(args),
        },
        Commands::Info(arg) => extract_preview(arg.url, InputKind::Url, &paths, &layout).await,
        Commands::Formats(arg) => formats(arg.url).await,
        Commands::Download(args) => download(args, &layout).await,
        Commands::Plugin(cmd) => match cmd.command {
            PluginSubcommand::List => plugin_list(&paths, &layout).await,
            PluginSubcommand::Install(args) => plugin_install(args, &paths, &layout).await,
        },
    }
}

const SHERPA_SEGMENTATION_ARCHIVE_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-segmentation-models/sherpa-onnx-pyannote-segmentation-3-0.tar.bz2";
const SHERPA_EMBEDDING_MODEL_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx";
const SHERPA_SEGMENTATION_DIR: &str = "sherpa-onnx-pyannote-segmentation-3-0";
const SHERPA_SEGMENTATION_ARCHIVE_BYTES: u64 = 6_958_444;
const SHERPA_SEGMENTATION_ARCHIVE_SHA256: &str =
    "24615ee884c897d9d2ba09bb4d30da6bb1b15e685065962db5b02e76e4996488";
const SHERPA_EMBEDDING_FILENAME: &str =
    "3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx";
const SHERPA_EMBEDDING_BYTES: u64 = 39_593_761;
const SHERPA_EMBEDDING_SHA256: &str =
    "1a331345f04805badbb495c775a6ddffcdd1a732567d5ec8b3d5749e3c7a5e4b";
const SHERPA_EN_EMBEDDING_MODEL_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx";
const SHERPA_EN_EMBEDDING_FILENAME: &str = "3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx";
const SHERPA_EN_EMBEDDING_BYTES: u64 = 29_596_978;
const SHERPA_EN_EMBEDDING_SHA256: &str =
    "357a834f702b80161e5b981182c038e18553c1f2ca752ed6cec2052365d4129b";
const SHERPA_RUNTIME_RECEIPT_FILENAME: &str = "vessel-runtime-integrity.json";
const SHERPA_MODELS_RECEIPT_FILENAME: &str = "vessel-models-integrity.json";

fn default_vessel_data_root() -> PathBuf {
    env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .map(|home| home.join(".local").join("share"))
        })
        .unwrap_or_else(|| PathBuf::from(".local/share"))
        .join("vessel")
}

fn default_diarization_model_root() -> PathBuf {
    default_vessel_data_root()
        .join("models")
        .join("diarization")
        .join(SHERPA_ONNX_BACKEND_NAME)
}

fn default_diarization_runtime_root() -> PathBuf {
    default_vessel_data_root()
        .join("runtime")
        .join("diarization")
        .join(SHERPA_ONNX_BACKEND_NAME)
        .join(format!("v{SHERPA_ONNX_RUNTIME_VERSION}"))
}

fn validate_fixture_video_id(video_id: &str) -> Result<&str> {
    let video_id = video_id.trim();
    if video_id.is_empty()
        || !video_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(VesselError::Config(format!(
            "invalid YouTube video id for diarization fixture: {video_id:?}"
        )));
    }
    Ok(video_id)
}

fn diarization_fixture_dir(video_id: &str) -> Result<PathBuf> {
    let video_id = validate_fixture_video_id(video_id)?;
    Ok(default_vessel_data_root()
        .join("fixtures")
        .join("diarization")
        .join("youtube")
        .join(video_id))
}

fn diarization_fixture_wav(video_id: &str) -> Result<PathBuf> {
    Ok(diarization_fixture_dir(video_id)?.join("input.wav"))
}

fn sherpa_runtime_archive() -> Result<(String, u64, &'static str)> {
    let version = SHERPA_ONNX_RUNTIME_VERSION;
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Ok((
            format!("sherpa-onnx-v{version}-linux-x64-shared-lib.tar.bz2"),
            9_547_977,
            "bbeb203da0f69e37235b50e168d61d1f64ad2de256490cc64ed5535957415a97",
        ))
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Ok((
            format!("sherpa-onnx-v{version}-linux-aarch64-shared-cpu-lib.tar.bz2"),
            12_331_011,
            "3575bde0543da12fc626c814c14287455f70a22b72caa483c7398d5f20f4cb12",
        ))
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Ok((
            format!("sherpa-onnx-v{version}-win-x64-shared-MT-Release-lib.tar.bz2"),
            7_890_278,
            "e539a6859e67faf6450473c4a3e0b1a6674b9f45df63c1483dbe8dca3a04f222",
        ))
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Ok((
            format!("sherpa-onnx-v{version}-osx-x64-shared-lib.tar.bz2"),
            9_770_199,
            "dbe7e7aa269f742efec7366d5c4d8020cc32fc833023b9b87e5d6282d70a62b8",
        ))
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Ok((
            format!("sherpa-onnx-v{version}-osx-arm64-shared-lib.tar.bz2"),
            8_512_718,
            "d628e43aed6b719be163549876f41c909b75df26b8f439a5af69de03896bc6f5",
        ))
    } else {
        Err(VesselError::Config(format!(
            "no sherpa runtime bundle is defined for {}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )))
    }
}

fn sherpa_model_paths(root: &Path) -> (PathBuf, PathBuf) {
    (
        root.join(SHERPA_SEGMENTATION_DIR).join("model.onnx"),
        root.join(SHERPA_EMBEDDING_FILENAME),
    )
}

fn sherpa_embedding_spec(profile: &str) -> Result<(&'static str, &'static str, u64, &'static str)> {
    match profile {
        "zh-3dspeaker" => Ok((
            SHERPA_EMBEDDING_FILENAME,
            SHERPA_EMBEDDING_MODEL_URL,
            SHERPA_EMBEDDING_BYTES,
            SHERPA_EMBEDDING_SHA256,
        )),
        "en-voxceleb" => Ok((
            SHERPA_EN_EMBEDDING_FILENAME,
            SHERPA_EN_EMBEDDING_MODEL_URL,
            SHERPA_EN_EMBEDDING_BYTES,
            SHERPA_EN_EMBEDDING_SHA256,
        )),
        other => Err(VesselError::Config(format!(
            "unknown sherpa speaker embedding profile {other:?}; expected zh-3dspeaker or en-voxceleb"
        ))),
    }
}

#[derive(Debug)]
struct IntegrityStatus {
    receipt: PathBuf,
    present: bool,
    verified: bool,
    errors: Vec<String>,
}

fn blake3_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(&mut file).map_err(|error| {
        VesselError::Extractor(format!("failed to hash {}: {error}", path.display()))
    })?;
    Ok(hasher.finalize().to_hex().to_string())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn verify_sha256(path: &Path, label: &str, expected_sha256: &str) -> Result<()> {
    let expected = expected_sha256
        .strip_prefix("sha256:")
        .unwrap_or(expected_sha256)
        .to_ascii_lowercase();
    let actual = sha256_file(path)?;
    if actual != expected {
        return Err(VesselError::Extractor(format!(
            "{label} SHA-256 mismatch: expected {expected}, got {actual} for {}",
            path.display()
        )));
    }
    Ok(())
}

fn write_integrity_receipt(
    receipt: &Path,
    root: &Path,
    kind: &str,
    files: &[(&str, &Path)],
) -> Result<()> {
    let mut entries = serde_json::Map::new();
    for (name, path) in files {
        let relative = path.strip_prefix(root).map_err(|_| {
            VesselError::Config(format!(
                "integrity file {} is outside receipt root {}",
                path.display(),
                root.display()
            ))
        })?;
        entries.insert(
            (*name).to_owned(),
            serde_json::json!({
                "relative_path": relative,
                "bytes": std::fs::metadata(path)?.len(),
                "blake3": blake3_file(path)?,
            }),
        );
    }

    let value = serde_json::json!({
        "schema": 1,
        "backend": SHERPA_ONNX_BACKEND_NAME,
        "kind": kind,
        "runtime_version": SHERPA_ONNX_RUNTIME_VERSION,
        "files": entries,
    });
    let rendered = serde_json::to_vec_pretty(&value)
        .map_err(|error| VesselError::Config(error.to_string()))?;
    let temp = receipt.with_extension("json.tmp");
    std::fs::write(&temp, rendered)?;
    std::fs::rename(&temp, receipt)?;
    Ok(())
}

fn verify_integrity_receipt(receipt: &Path, root: &Path) -> IntegrityStatus {
    if !receipt.is_file() {
        return IntegrityStatus {
            receipt: receipt.to_path_buf(),
            present: false,
            verified: false,
            errors: Vec::new(),
        };
    }

    let mut errors = Vec::new();
    let value = match std::fs::read_to_string(receipt)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
    {
        Some(value) => value,
        None => {
            return IntegrityStatus {
                receipt: receipt.to_path_buf(),
                present: true,
                verified: false,
                errors: vec!["receipt is not valid JSON".into()],
            };
        }
    };

    let Some(files) = value.get("files").and_then(serde_json::Value::as_object) else {
        return IntegrityStatus {
            receipt: receipt.to_path_buf(),
            present: true,
            verified: false,
            errors: vec!["receipt has no files object".into()],
        };
    };

    for (name, entry) in files {
        let Some(relative) = entry
            .get("relative_path")
            .and_then(serde_json::Value::as_str)
        else {
            errors.push(format!("{name}: missing relative_path"));
            continue;
        };
        let path = root.join(relative);
        if !path.is_file() {
            errors.push(format!("{name}: missing {}", path.display()));
            continue;
        }

        let expected_bytes = entry.get("bytes").and_then(serde_json::Value::as_u64);
        let actual_bytes = std::fs::metadata(&path).map(|metadata| metadata.len());
        match (expected_bytes, actual_bytes) {
            (Some(expected), Ok(actual)) if expected == actual => {}
            (Some(expected), Ok(actual)) => errors.push(format!(
                "{name}: size mismatch expected={expected} actual={actual}"
            )),
            (_, Err(error)) => errors.push(format!("{name}: metadata failed: {error}")),
            (None, _) => errors.push(format!("{name}: receipt missing byte count")),
        }

        let expected_hash = entry.get("blake3").and_then(serde_json::Value::as_str);
        match (expected_hash, blake3_file(&path)) {
            (Some(expected), Ok(actual)) if expected == actual => {}
            (Some(expected), Ok(actual)) => errors.push(format!(
                "{name}: BLAKE3 mismatch expected={expected} actual={actual}"
            )),
            (_, Err(error)) => errors.push(format!("{name}: hash failed: {error}")),
            (None, _) => errors.push(format!("{name}: receipt missing BLAKE3 hash")),
        }
    }

    IntegrityStatus {
        receipt: receipt.to_path_buf(),
        present: true,
        verified: errors.is_empty(),
        errors,
    }
}

fn integrity_status_json(status: &IntegrityStatus) -> serde_json::Value {
    serde_json::json!({
        "receipt": status.receipt,
        "present": status.present,
        "verified": status.verified,
        "errors": status.errors,
    })
}

fn install_staged_directory(staging: &Path, target: &Path) -> Result<()> {
    let parent = target.parent().ok_or_else(|| {
        VesselError::Config(format!(
            "staged install target has no parent: {}",
            target.display()
        ))
    })?;
    std::fs::create_dir_all(parent)?;

    let target_name = target
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            VesselError::Config(format!(
                "staged install target has no UTF-8 file name: {}",
                target.display()
            ))
        })?;
    let backup = parent.join(format!(".{target_name}.previous"));
    if backup.exists() {
        std::fs::remove_dir_all(&backup)?;
    }

    let had_target = target.exists();
    if had_target {
        if !target.is_dir() {
            return Err(VesselError::Config(format!(
                "staged install target exists but is not a directory: {}",
                target.display()
            )));
        }
        std::fs::rename(target, &backup)?;
    }

    match std::fs::rename(staging, target) {
        Ok(()) => {
            if backup.exists() {
                std::fs::remove_dir_all(backup)?;
            }
            Ok(())
        }
        Err(error) => {
            if had_target && backup.exists() {
                let _ = std::fs::rename(&backup, target);
            }
            Err(VesselError::Extractor(format!(
                "failed to install staged directory {} -> {}: {error}",
                staging.display(),
                target.display()
            )))
        }
    }
}

fn reset_staging_directory(path: &Path) -> Result<()> {
    if path.exists() {
        std::fs::remove_dir_all(path)?;
    }
    std::fs::create_dir_all(path)?;
    Ok(())
}

fn planned_remaining_bytes(
    destination: &Path,
    expected_bytes: u64,
    already_available: bool,
) -> (u64, u64) {
    if already_available {
        return (0, 0);
    }
    if destination.is_file()
        && std::fs::metadata(destination)
            .map(|metadata| metadata.len() == expected_bytes)
            .unwrap_or(false)
    {
        return (expected_bytes, 0);
    }
    let partial = destination.with_extension("download");
    let resume_bytes = std::fs::metadata(partial)
        .map(|metadata| metadata.len().min(expected_bytes))
        .unwrap_or(0);
    (resume_bytes, expected_bytes.saturating_sub(resume_bytes))
}

fn validate_sherpa_bundle(
    runtime_library: &Path,
    segmentation_model: &Path,
    embedding_model: &Path,
) -> Result<()> {
    eprintln!(
        "[diarization] validating fetched runtime and models runtime={} segmentation={} embedding={}",
        runtime_library.display(),
        segmentation_model.display(),
        embedding_model.display(),
    );
    let config = DiarizationConfig {
        backend: SHERPA_ONNX_BACKEND_NAME.into(),
        runtime_library: runtime_library.to_path_buf(),
        segmentation_model: segmentation_model.to_path_buf(),
        embedding_model: embedding_model.to_path_buf(),
        provider: "cpu".into(),
        num_threads: 1,
        num_speakers: None,
        clustering_threshold: DEFAULT_CLUSTERING_THRESHOLD,
        window_shift_ratio: DEFAULT_WINDOW_SHIFT_RATIO,
        min_duration_on: 0.3,
        min_duration_off: 0.5,
        speaker_embeddings: true,
    };
    let _backend = SherpaOnnxDiarizer::load(config)?;
    eprintln!("[diarization] fetched runtime and models validated");
    Ok(())
}

async fn diarization_fetch(args: DiarizationFetchArgs) -> Result<()> {
    if args.backend != SHERPA_ONNX_BACKEND_NAME {
        return Err(VesselError::Config(format!(
            "model/runtime fetching is not implemented for diarization backend {:?}",
            args.backend
        )));
    }

    let model_root = args.dir.unwrap_or_else(default_diarization_model_root);
    let runtime_root = args
        .runtime_dir
        .unwrap_or_else(default_diarization_runtime_root);
    let (embedding_filename, embedding_url, embedding_bytes, embedding_sha256) =
        sherpa_embedding_spec(&args.embedding_profile)?;
    let segmentation_model = model_root.join(SHERPA_SEGMENTATION_DIR).join("model.onnx");
    let embedding_model = model_root.join(embedding_filename);
    let (runtime_archive_name, runtime_archive_bytes, runtime_archive_sha256) =
        sherpa_runtime_archive()?;
    let runtime_url = format!(
        "https://github.com/k2-fsa/sherpa-onnx/releases/download/v{}/{}",
        SHERPA_ONNX_RUNTIME_VERSION, runtime_archive_name
    );
    let runtime_parent = runtime_root.parent().ok_or_else(|| {
        VesselError::Config(format!(
            "runtime directory has no parent: {}",
            runtime_root.display()
        ))
    })?;
    let runtime_archive_path = runtime_parent.join(&runtime_archive_name);
    let segmentation_archive_path =
        model_root.join("sherpa-onnx-pyannote-segmentation-3-0.tar.bz2");

    if args.plan {
        let runtime_available = find_sherpa_runtime_library(&runtime_root)
            .ok()
            .is_some_and(|path| probe_sherpa_runtime(&path).is_ok());
        let segmentation_available = segmentation_model.is_file();
        let embedding_available = embedding_model.is_file();
        let (runtime_resume_bytes, runtime_remaining_bytes) = planned_remaining_bytes(
            &runtime_archive_path,
            runtime_archive_bytes,
            runtime_available,
        );
        let (segmentation_resume_bytes, segmentation_remaining_bytes) = planned_remaining_bytes(
            &segmentation_archive_path,
            SHERPA_SEGMENTATION_ARCHIVE_BYTES,
            segmentation_available,
        );
        let (embedding_resume_bytes, embedding_remaining_bytes) =
            planned_remaining_bytes(&embedding_model, embedding_bytes, embedding_available);
        let plan = serde_json::json!({
            "status": "plan",
            "network_io": false,
            "backend": SHERPA_ONNX_BACKEND_NAME,
            "build_time_fetch": false,
            "transactional_install": true,
            "integrity": {
                "download_algorithm": "sha256",
                "installed_receipt_algorithm": "blake3",
                "runtime_receipt": runtime_root.join(SHERPA_RUNTIME_RECEIPT_FILENAME),
                "model_receipt": model_root.join(SHERPA_MODELS_RECEIPT_FILENAME),
                "network_required_for_verification": false,
            },
            "downloads": [
                {
                    "kind": "native_runtime",
                    "url": runtime_url,
                    "expected_bytes": runtime_archive_bytes,
                    "expected_sha256": runtime_archive_sha256,
                    "destination_root": runtime_root,
                    "already_available": runtime_available,
                    "resume_bytes": runtime_resume_bytes,
                    "remaining_bytes": runtime_remaining_bytes,
                },
                {
                    "kind": "segmentation_model",
                    "url": SHERPA_SEGMENTATION_ARCHIVE_URL,
                    "expected_bytes": SHERPA_SEGMENTATION_ARCHIVE_BYTES,
                    "expected_sha256": SHERPA_SEGMENTATION_ARCHIVE_SHA256,
                    "destination": segmentation_model,
                    "already_available": segmentation_available,
                    "resume_bytes": segmentation_resume_bytes,
                    "remaining_bytes": segmentation_remaining_bytes,
                },
                {
                    "kind": "speaker_embedding_model",
                    "url": embedding_url,
                    "expected_bytes": embedding_bytes,
                    "expected_sha256": embedding_sha256,
                    "profile": args.embedding_profile,
                    "destination": embedding_model,
                    "already_available": embedding_available,
                    "resume_bytes": embedding_resume_bytes,
                    "remaining_bytes": embedding_remaining_bytes,
                }
            ],
            "total_expected_bytes": runtime_archive_bytes
                + SHERPA_SEGMENTATION_ARCHIVE_BYTES
                + embedding_bytes,
            "total_remaining_bytes": runtime_remaining_bytes
                + segmentation_remaining_bytes
                + embedding_remaining_bytes,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&plan)
                .map_err(|error| VesselError::Config(error.to_string()))?
        );
        return Ok(());
    }

    tokio::fs::create_dir_all(&model_root).await?;
    tokio::fs::create_dir_all(runtime_parent).await?;
    let client = Client::new();

    let runtime_library = match find_sherpa_runtime_library(&runtime_root) {
        Ok(path) if probe_sherpa_runtime(&path).is_ok() => {
            eprintln!(
                "[diarization] sherpa runtime already available and loadable path={}",
                path.display()
            );
            path
        }
        existing => {
            if let Ok(path) = existing {
                eprintln!(
                    "[diarization] existing sherpa runtime is not loadable; staging a replacement path={}",
                    path.display()
                );
            }

            let archive_path = runtime_archive_path.clone();
            download_with_progress(
                &client,
                &runtime_url,
                &archive_path,
                "sherpa native runtime",
                Some(runtime_archive_bytes),
                Some(runtime_archive_sha256),
            )
            .await?;

            let runtime_name = runtime_root
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| {
                    VesselError::Config(format!(
                        "runtime directory has no UTF-8 file name: {}",
                        runtime_root.display()
                    ))
                })?;
            let staging_root = runtime_parent.join(format!(".{runtime_name}.installing"));
            reset_staging_directory(&staging_root)?;

            let unpack_root = staging_root.clone();
            let archive_for_worker = archive_path.clone();
            let extraction = tokio::task::spawn_blocking(move || -> Result<()> {
                let file = std::fs::File::open(&archive_for_worker)?;
                let decoder = bzip2::read::BzDecoder::new(file);
                let mut archive = tar::Archive::new(decoder);
                archive.unpack(&unpack_root)?;
                Ok(())
            })
            .await
            .map_err(|error| {
                VesselError::Extractor(format!(
                    "sherpa runtime archive worker failed to join: {error}"
                ))
            })?;
            if let Err(error) = extraction {
                let _ = std::fs::remove_dir_all(&staging_root);
                let _ = std::fs::remove_file(&archive_path);
                return Err(error);
            }

            let staged_runtime = match find_sherpa_runtime_library(&staging_root) {
                Ok(path) => path,
                Err(error) => {
                    let _ = std::fs::remove_dir_all(&staging_root);
                    let _ = std::fs::remove_file(&archive_path);
                    return Err(error);
                }
            };
            if let Err(error) = probe_sherpa_runtime(&staged_runtime) {
                let _ = std::fs::remove_dir_all(&staging_root);
                let _ = std::fs::remove_file(&archive_path);
                return Err(VesselError::Extractor(format!(
                    "staged sherpa runtime failed validation: {error}"
                )));
            }

            install_staged_directory(&staging_root, &runtime_root)?;
            let _ = tokio::fs::remove_file(&archive_path).await;
            let installed = find_sherpa_runtime_library(&runtime_root)?;
            probe_sherpa_runtime(&installed)?;
            installed
        }
    };

    if !segmentation_model.is_file() {
        let archive_path = segmentation_archive_path.clone();
        download_with_progress(
            &client,
            SHERPA_SEGMENTATION_ARCHIVE_URL,
            &archive_path,
            "sherpa segmentation model",
            Some(SHERPA_SEGMENTATION_ARCHIVE_BYTES),
            Some(SHERPA_SEGMENTATION_ARCHIVE_SHA256),
        )
        .await?;

        let staging_root = model_root.join(".segmentation.installing");
        reset_staging_directory(&staging_root)?;
        let unpack_root = staging_root.clone();
        let archive_for_worker = archive_path.clone();
        let extraction = tokio::task::spawn_blocking(move || -> Result<()> {
            let file = std::fs::File::open(&archive_for_worker)?;
            let decoder = bzip2::read::BzDecoder::new(file);
            let mut archive = tar::Archive::new(decoder);
            archive.unpack(&unpack_root)?;
            Ok(())
        })
        .await
        .map_err(|error| {
            VesselError::Extractor(format!(
                "sherpa segmentation archive worker failed to join: {error}"
            ))
        })?;
        if let Err(error) = extraction {
            let _ = std::fs::remove_dir_all(&staging_root);
            let _ = std::fs::remove_file(&archive_path);
            return Err(error);
        }

        let staged_model_dir = staging_root.join(SHERPA_SEGMENTATION_DIR);
        let staged_model = staged_model_dir.join("model.onnx");
        if !staged_model.is_file() {
            let _ = std::fs::remove_dir_all(&staging_root);
            let _ = std::fs::remove_file(&archive_path);
            return Err(VesselError::Extractor(format!(
                "segmentation archive did not contain expected model {}",
                staged_model.display()
            )));
        }

        let target_model_dir = model_root.join(SHERPA_SEGMENTATION_DIR);
        install_staged_directory(&staged_model_dir, &target_model_dir)?;
        let _ = std::fs::remove_dir_all(&staging_root);
        let _ = tokio::fs::remove_file(&archive_path).await;
    } else {
        eprintln!(
            "[diarization] segmentation model already available path={}",
            segmentation_model.display()
        );
    }

    if !embedding_model.is_file() {
        download_with_progress(
            &client,
            embedding_url,
            &embedding_model,
            "sherpa speaker embedding model",
            Some(embedding_bytes),
            Some(embedding_sha256),
        )
        .await?;
    } else {
        eprintln!(
            "[diarization] embedding model already available path={}",
            embedding_model.display()
        );
    }

    if !segmentation_model.is_file() || !embedding_model.is_file() {
        return Err(VesselError::Extractor(format!(
            "sherpa model fetch completed without expected files under {}",
            model_root.display()
        )));
    }

    probe_sherpa_runtime(&runtime_library)?;
    validate_sherpa_bundle(&runtime_library, &segmentation_model, &embedding_model)?;

    let runtime_receipt = runtime_root.join(SHERPA_RUNTIME_RECEIPT_FILENAME);
    write_integrity_receipt(
        &runtime_receipt,
        &runtime_root,
        "native_runtime",
        &[("runtime_library", &runtime_library)],
    )?;
    let models_receipt = if args.embedding_profile == "zh-3dspeaker" {
        model_root.join(SHERPA_MODELS_RECEIPT_FILENAME)
    } else {
        model_root.join(format!(
            "vessel-models-integrity.{}.json",
            args.embedding_profile
        ))
    };
    write_integrity_receipt(
        &models_receipt,
        &model_root,
        "inference_models",
        &[
            ("segmentation_model", &segmentation_model),
            ("speaker_embedding_model", &embedding_model),
        ],
    )?;

    let report = serde_json::json!({
        "status": "ok",
        "backend": SHERPA_ONNX_BACKEND_NAME,
        "network_phase": "explicit_runtime_command",
        "build_time_fetch": false,
        "transactional_install": true,
        "bundle_load_validated": true,
        "runtime_version": SHERPA_ONNX_RUNTIME_VERSION,
        "runtime_directory": runtime_root,
        "runtime_library": runtime_library,
        "model_directory": model_root,
        "segmentation_model": segmentation_model,
        "embedding_model": embedding_model,
        "embedding_profile": args.embedding_profile,
        "runtime_integrity_receipt": runtime_receipt,
        "model_integrity_receipt": models_receipt,
        "python_required": false,
        "reuse_with": {
            "diarization_backend": SHERPA_ONNX_BACKEND_NAME,
            "diarization_runtime_dir": runtime_root,
            "diarization_segmentation_model": segmentation_model,
            "diarization_embedding_model": embedding_model,
        }
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|error| VesselError::Config(error.to_string()))?
    );
    Ok(())
}

async fn download_with_progress(
    client: &Client,
    url: &str,
    destination: &Path,
    label: &str,
    expected_bytes: Option<u64>,
    expected_sha256: Option<&str>,
) -> Result<()> {
    let parent = destination.parent().ok_or_else(|| {
        VesselError::Config(format!(
            "download destination has no parent: {}",
            destination.display()
        ))
    })?;
    tokio::fs::create_dir_all(parent).await?;

    if destination.is_file() {
        let actual = tokio::fs::metadata(destination).await?.len();
        let size_matches = expected_bytes.is_none_or(|expected| expected == actual);
        let digest_matches = if size_matches {
            expected_sha256
                .map(|expected| verify_sha256(destination, label, expected).is_ok())
                .unwrap_or(true)
        } else {
            false
        };
        if size_matches && digest_matches {
            eprintln!(
                "[diarization] completed download already available label={} bytes={} sha256_verified={} path={}",
                label,
                actual,
                expected_sha256.is_some(),
                destination.display()
            );
            return Ok(());
        }
        eprintln!(
            "[diarization] existing artifact failed pinned integrity checks; fetching replacement label={} path={}",
            label,
            destination.display()
        );
    }

    let temp = destination.with_extension("download");
    let mut resume_from = tokio::fs::metadata(&temp)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0);

    let mut response = {
        let mut request = client.get(url);
        if resume_from > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={resume_from}-"));
            eprintln!(
                "[diarization] resuming download label={} from_byte={} temp={}",
                label,
                resume_from,
                temp.display()
            );
        }
        request
            .send()
            .await
            .map_err(|error| VesselError::Extractor(format!("{label} download failed: {error}")))?
    };

    if response.status() == StatusCode::RANGE_NOT_SATISFIABLE && resume_from > 0 {
        eprintln!(
            "[diarization] remote rejected resume range; restarting label={} from byte 0",
            label
        );
        let _ = tokio::fs::remove_file(&temp).await;
        resume_from = 0;
        response =
            client.get(url).send().await.map_err(|error| {
                VesselError::Extractor(format!("{label} download failed: {error}"))
            })?;
    }

    let append = resume_from > 0 && response.status() == StatusCode::PARTIAL_CONTENT;
    if !response.status().is_success() {
        return Err(VesselError::Extractor(format!(
            "{label} download returned HTTP {}",
            response.status()
        )));
    }

    if resume_from > 0 && !append {
        eprintln!(
            "[diarization] remote ignored resume range; restarting label={} from byte 0",
            label
        );
        resume_from = 0;
    }

    let response_bytes = response.content_length();
    let total = response_bytes.map(|bytes| resume_from.saturating_add(bytes));
    let mut file = if append {
        tokio::fs::OpenOptions::new()
            .append(true)
            .open(&temp)
            .await?
    } else {
        tokio::fs::File::create(&temp).await?
    };
    let mut downloaded = resume_from;
    let mut next_report = downloaded;

    eprintln!(
        "[diarization] download started label={} destination={} resumed_bytes={} total_bytes={}",
        label,
        destination.display(),
        resume_from,
        total
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".into()),
    );

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| VesselError::Extractor(format!("{label} download failed: {error}")))?
    {
        file.write_all(&chunk).await?;
        downloaded = downloaded.saturating_add(chunk.len() as u64);
        if downloaded >= next_report {
            if let Some(total) = total {
                let percent = if total == 0 {
                    100.0
                } else {
                    downloaded as f64 * 100.0 / total as f64
                };
                eprintln!(
                    "[diarization] downloading label={} {:.1}% ({}/{})",
                    label, percent, downloaded, total
                );
                next_report = downloaded.saturating_add((total / 20).max(1));
            } else {
                eprintln!(
                    "[diarization] downloading label={} bytes={}",
                    label, downloaded
                );
                next_report = downloaded.saturating_add(16 * 1024 * 1024);
            }
        }
    }
    file.flush().await?;
    drop(file);

    let actual_bytes = tokio::fs::metadata(&temp).await?.len();
    if let Some(expected_bytes) = expected_bytes
        && actual_bytes != expected_bytes
    {
        return Err(VesselError::Extractor(format!(
            "{label} download size mismatch: expected {expected_bytes} bytes, got {actual_bytes}; partial data was kept at {} for retry/resume",
            temp.display()
        )));
    }

    if let Some(expected_sha256) = expected_sha256 {
        verify_sha256(&temp, label, expected_sha256)?;
        eprintln!(
            "[diarization] SHA-256 verified label={} sha256={}",
            label, expected_sha256
        );
    }

    if destination.exists() {
        tokio::fs::remove_file(destination).await?;
    }
    tokio::fs::rename(&temp, destination).await?;
    eprintln!(
        "[diarization] download completed label={} bytes={} sha256_verified={} path={}",
        label,
        downloaded,
        expected_sha256.is_some(),
        destination.display()
    );
    Ok(())
}

fn diarization_models() -> Result<()> {
    let report = serde_json::json!({
        "backends": [
            {
                "name": SHERPA_ONNX_BACKEND_NAME,
                "status": "implemented",
                "primary": true,
                "python_required": false,
                "implementation_language": "rust",
                "runtime_abi": "sherpa-c",
                "runtime_loading": "dynamic_at_execution",
                "build_time_fetch": false,
                "runtime_version": SHERPA_ONNX_RUNTIME_VERSION,
                "runtime_archive_bytes": sherpa_runtime_archive().ok().map(|(_, bytes, _)| bytes),
                "segmentation_archive_bytes": SHERPA_SEGMENTATION_ARCHIVE_BYTES,
                "embedding_model_bytes": SHERPA_EMBEDDING_BYTES,
                "segmentation_model": "sherpa-onnx-pyannote-segmentation-3-0/model.onnx",
                "embedding_model": "3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx",
                "embedding_profiles": [
                    {
                        "name": "zh-3dspeaker",
                        "default": true,
                        "language": "zh-cn",
                        "training_corpus": "3D-Speaker",
                        "filename": SHERPA_EMBEDDING_FILENAME,
                        "bytes": SHERPA_EMBEDDING_BYTES,
                    },
                    {
                        "name": "en-voxceleb",
                        "default": false,
                        "language": "en",
                        "training_corpus": "VoxCeleb",
                        "filename": SHERPA_EN_EMBEDDING_FILENAME,
                        "bytes": SHERPA_EN_EMBEDDING_BYTES,
                    }
                ],
                "provider": "cpu",
                "speaker_embeddings": true,
            },
            {
                "name": "whisperx",
                "status": "compatibility",
                "primary": false,
                "python_required": true,
                "implementation_language": "python",
                "runtime_abi": "python-cli",
                "speaker_embeddings": true,
            }
        ]
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|error| VesselError::Config(error.to_string()))?
    );
    Ok(())
}

fn diarization_doctor(args: DiarizationDoctorArgs) -> Result<()> {
    match args.backend.as_str() {
        SHERPA_ONNX_BACKEND_NAME => {
            let default_model_root = default_diarization_model_root();
            let (default_segmentation, default_embedding) = sherpa_model_paths(&default_model_root);
            let using_default_models =
                args.segmentation_model.is_none() && args.embedding_model.is_none();
            let segmentation_path = args.segmentation_model.unwrap_or(default_segmentation);
            let embedding_path = args.embedding_model.unwrap_or(default_embedding);
            let runtime_root = args
                .runtime_dir
                .unwrap_or_else(default_diarization_runtime_root);
            let runtime_library = find_sherpa_runtime_library(&runtime_root).ok();
            let runtime_integrity = verify_integrity_receipt(
                &runtime_root.join(SHERPA_RUNTIME_RECEIPT_FILENAME),
                &runtime_root,
            );
            let model_integrity = if using_default_models {
                verify_integrity_receipt(
                    &default_model_root.join(SHERPA_MODELS_RECEIPT_FILENAME),
                    &default_model_root,
                )
            } else {
                IntegrityStatus {
                    receipt: default_model_root.join(SHERPA_MODELS_RECEIPT_FILENAME),
                    present: false,
                    verified: false,
                    errors: Vec::new(),
                }
            };
            let runtime_probe = runtime_library
                .as_deref()
                .map(probe_sherpa_runtime)
                .transpose();
            let runtime_loadable = runtime_probe
                .as_ref()
                .is_ok_and(|_| runtime_library.is_some());
            let runtime_error = runtime_probe.err().map(|error| error.to_string());
            let bundle_probe = if let Some(runtime_library) = runtime_library.as_deref()
                && segmentation_path.is_file()
                && embedding_path.is_file()
            {
                Some(validate_sherpa_bundle(
                    runtime_library,
                    &segmentation_path,
                    &embedding_path,
                ))
            } else {
                None
            };
            let bundle_loadable = bundle_probe.as_ref().is_some_and(|result| result.is_ok());
            let bundle_error = bundle_probe
                .and_then(|result| result.err())
                .map(|error| error.to_string());
            let receipt_integrity_ok = (!runtime_integrity.present || runtime_integrity.verified)
                && (!model_integrity.present || model_integrity.verified);
            let ready = runtime_loadable
                && bundle_loadable
                && segmentation_path.is_file()
                && embedding_path.is_file()
                && receipt_integrity_ok
                && !args.provider.trim().is_empty();
            let report = serde_json::json!({
                "backend": SHERPA_ONNX_BACKEND_NAME,
                "implemented": true,
                "python_required": false,
                "implementation_language": "rust",
                "runtime_abi": "sherpa-c",
                "runtime_loading": "dynamic_at_execution",
                "build_time_fetch": false,
                "runtime_version": SHERPA_ONNX_RUNTIME_VERSION,
                "provider": args.provider,
                "runtime_directory": runtime_root,
                "runtime_library": runtime_library,
                "runtime_loadable": runtime_loadable,
                "runtime_error": runtime_error,
                "bundle_loadable": bundle_loadable,
                "bundle_error": bundle_error,
                "runtime_integrity": integrity_status_json(&runtime_integrity),
                "model_integrity": integrity_status_json(&model_integrity),
                "segmentation_model": {
                    "path": segmentation_path,
                    "exists": segmentation_path.is_file(),
                },
                "embedding_model": {
                    "path": embedding_path,
                    "exists": embedding_path.is_file(),
                },
                "ready_for_diarization": ready,
                "fetch_command": "vessel diarization fetch",
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&report)
                    .map_err(|error| VesselError::Config(error.to_string()))?
            );
            Ok(())
        }
        "whisperx" => {
            let report = serde_json::json!({
                "backend": "whisperx",
                "implemented": true,
                "compatibility_only": true,
                "python_required": true,
                "ready_for_diarization": false,
                "message": "Use vessel asr doctor --backend whisperx for the optional Python compatibility backend.",
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&report)
                    .map_err(|error| VesselError::Config(error.to_string()))?
            );
            Ok(())
        }
        other => Err(VesselError::Config(format!(
            "unsupported diarization backend {other:?}"
        ))),
    }
}

async fn diarization_seed(args: DiarizationSeedArgs) -> Result<()> {
    let video_id = validate_fixture_video_id(&args.video_id)?.to_owned();
    let target_dir = diarization_fixture_dir(&video_id)?;
    let target_wav = target_dir.join("input.wav");
    let provenance_path = target_dir.join("provenance.json");

    if target_wav.is_file() && provenance_path.is_file() && !args.force {
        let report = serde_json::json!({
            "status": "already_seeded",
            "video_id": video_id,
            "fixture": target_wav,
            "provenance": provenance_path,
            "network_io": false,
            "durable": true,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|error| VesselError::Config(error.to_string()))?
        );
        return Ok(());
    }

    let parent = target_dir.parent().ok_or_else(|| {
        VesselError::Config(format!(
            "diarization fixture directory has no parent: {}",
            target_dir.display()
        ))
    })?;
    tokio::fs::create_dir_all(parent).await?;
    let staging_dir = parent.join(format!(".{video_id}.seeding"));
    if args.force && staging_dir.exists() {
        tokio::fs::remove_dir_all(&staging_dir).await?;
    }
    tokio::fs::create_dir_all(&staging_dir).await?;

    eprintln!(
        "[diarization] seeding durable fixture video_id={} staging={}",
        video_id,
        staging_dir.display()
    );
    let video = extract_video(&InputRef {
        raw: video_id.clone(),
        kind: InputKind::VideoId,
    })
    .await?;

    let output_template = staging_dir
        .join("source.%(ext)s")
        .to_string_lossy()
        .into_owned();
    let planner = BasicDownloadPlanner;
    let plan = planner.plan(&video, FormatSelector::BestAudio, &output_template)?;
    let source_audio = plan
        .downloads
        .first()
        .map(|download| download.output_path.clone())
        .ok_or_else(|| {
            VesselError::Extractor(format!(
                "diarization seed audio planner produced no download for {video_id}"
            ))
        })?;
    if !source_audio.is_file() {
        execute_download(&plan).await?;
    }

    let staged_wav = staging_dir.join("input.wav");
    if !staged_wav.is_file() {
        eprintln!(
            "[diarization] normalizing fixture audio input={} output={}",
            source_audio.display(),
            staged_wav.display()
        );
        transcode_asr_audio(&source_audio, &staged_wav).await?;
    }

    if !staged_wav.is_file() || std::fs::metadata(&staged_wav)?.len() <= 44 {
        return Err(VesselError::Extractor(format!(
            "diarization seed did not produce a valid WAV at {}",
            staged_wav.display()
        )));
    }

    let provenance = serde_json::json!({
        "schema": 1,
        "kind": "youtube_diarization_fixture",
        "video_id": video_id,
        "source_url": format!("https://www.youtube.com/watch?v={}", video.video_id),
        "channel_id": video.channel_id,
        "upload_date": video.upload_date,
        "sample_rate_hz": 16000,
        "channels": 1,
        "codec": "pcm_s16le",
        "network_phase": "explicit_seed_command",
        "durable": true,
    });
    std::fs::write(
        staging_dir.join("provenance.json"),
        serde_json::to_vec_pretty(&provenance)
            .map_err(|error| VesselError::Config(error.to_string()))?,
    )?;

    if source_audio.is_file() {
        let _ = tokio::fs::remove_file(&source_audio).await;
    }

    install_staged_directory(&staging_dir, &target_dir)?;

    let report = serde_json::json!({
        "status": "seeded",
        "video_id": video.video_id,
        "fixture": target_wav,
        "provenance": provenance_path,
        "network_io": true,
        "durable": true,
        "reuse_with": {
            "command": format!(
                "vessel diarization run --fixture-video-id {} --speaker-embeddings",
                video.video_id
            ),
        },
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|error| VesselError::Config(error.to_string()))?
    );
    Ok(())
}

fn diarization_reembed(args: DiarizationReembedArgs) -> Result<()> {
    let sourcearium_root = if args.sourcearium.is_absolute() {
        args.sourcearium.clone()
    } else {
        std::env::current_dir()?.join(&args.sourcearium)
    };
    let source = discover_youtube_sources(&sourcearium_root)?
        .into_iter()
        .find(|source| source.policy.source_key == args.source_key)
        .ok_or_else(|| {
            VesselError::Config(format!(
                "Sourcearium source key {:?} was not found under {}",
                args.source_key,
                sourcearium_root.display()
            ))
        })?;

    let input = diarization_fixture_wav(&args.video_id)?;
    if !input.is_file() {
        return Err(VesselError::Config(format!(
            "durable diarization fixture is missing for {}; seed it first with: vessel diarization seed --video-id {}",
            args.video_id, args.video_id
        )));
    }
    let evidence_dir = sourcearium_root
        .join(".cache")
        .join("vessel")
        .join("speaker-evidence");
    let evidence_path = evidence_dir.join(format!("{}.json", args.video_id));
    let mut evidence = load_speaker_evidence(&evidence_path)?;
    if evidence.video_id != args.video_id {
        return Err(VesselError::Config(format!(
            "speaker evidence video_id {:?} does not match requested {:?}",
            evidence.video_id, args.video_id
        )));
    }

    let default_model_root = default_diarization_model_root();
    let (default_segmentation, default_embedding) = sherpa_model_paths(&default_model_root);
    let segmentation_model = args.segmentation_model.unwrap_or(default_segmentation);
    let embedding_model = args.embedding_model.unwrap_or(default_embedding);
    let runtime_root = args
        .runtime_dir
        .unwrap_or_else(default_diarization_runtime_root);
    let runtime_library = find_sherpa_runtime_library(&runtime_root).map_err(|_| {
        VesselError::Config(format!(
            "sherpa-onnx runtime is missing; run vessel diarization fetch or pass --runtime-dir explicitly (expected default under {})",
            runtime_root.display()
        ))
    })?;

    let config = DiarizationConfig {
        backend: SHERPA_ONNX_BACKEND_NAME.into(),
        runtime_library,
        segmentation_model,
        embedding_model,
        provider: args.provider,
        num_threads: args.num_threads,
        num_speakers: None,
        clustering_threshold: DEFAULT_CLUSTERING_THRESHOLD,
        window_shift_ratio: DEFAULT_WINDOW_SHIFT_RATIO,
        min_duration_on: 0.3,
        min_duration_off: 0.5,
        speaker_embeddings: true,
    };
    config.validate()?;
    let refreshed_model = config.reembedded_model_provenance(&evidence.diarization.model)?;
    let segments = evidence
        .segments
        .iter()
        .map(|segment| DiarizationSegment {
            start_seconds: segment.start,
            end_seconds: segment.end,
            speaker: segment.speaker.clone(),
        })
        .collect::<Vec<_>>();

    let started = Instant::now();
    let backend = SherpaOnnxDiarizer::load(config)?;
    let embeddings = backend.reembed_segments_path(&input, &segments)?;
    if embeddings.is_empty() {
        return Err(VesselError::Config(
            "re-embedding produced no usable speaker embeddings".into(),
        ));
    }
    evidence.diarization.model = refreshed_model.clone();
    evidence.speaker_embeddings = Some(embeddings.clone());
    let persisted = persist_speaker_evidence(&evidence_dir, &evidence)?;
    let refreshed = refresh_youtube_transcript_diarization_provenance(
        &source,
        &args.video_id,
        &evidence.diarization.engine,
        &refreshed_model,
    )?;

    let report = serde_json::json!({
        "status": "ok",
        "sourcearium_root": sourcearium_root,
        "source_key": args.source_key,
        "video_id": args.video_id,
        "transcript": refreshed.path,
        "transcript_metadata_updated": refreshed.updated,
        "speaker_attribution_cleared": refreshed.speaker_attribution_cleared,
        "evidence": persisted,
        "model": refreshed_model,
        "segment_count": segments.len(),
        "speaker_count": segments
            .iter()
            .map(|segment| segment.speaker.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        "speaker_embedding_count": embeddings.len(),
        "elapsed_seconds": started.elapsed().as_secs_f64(),
        "network_io": false,
        "asr_invoked": false,
        "segmentation_invoked": false,
        "clustering_invoked": false,
        "sourcearium_mutation": true,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|error| VesselError::Config(error.to_string()))?
    );
    Ok(())
}

async fn diarization_apply(args: DiarizationApplyArgs) -> Result<()> {
    let sourcearium_root = if args.sourcearium.is_absolute() {
        args.sourcearium
    } else {
        std::env::current_dir()?.join(args.sourcearium)
    };
    let source = discover_youtube_sources(&sourcearium_root)?
        .into_iter()
        .find(|source| source.policy.source_key == args.source_key)
        .ok_or_else(|| {
            VesselError::Config(format!(
                "Sourcearium source key {:?} was not found under {}",
                args.source_key,
                sourcearium_root.display()
            ))
        })?;

    let existing = load_youtube_transcript_artifact(&source, &args.video_id)?.ok_or_else(|| {
        VesselError::Config(format!(
            "Sourcearium transcript does not exist for video {:?}",
            args.video_id
        ))
    })?;
    let mut candidate =
        TranscriptCandidate::from_sourcearium_artifact(&existing.artifact, &existing.body)?;
    if candidate.derivation != TranscriptDerivation::LocalAsr {
        return Err(VesselError::Config(format!(
            "standalone diarization enrichment currently requires a local_asr transcript; {} uses {}",
            existing.path.display(),
            candidate.derivation.as_str()
        )));
    }
    if !candidate.timestamps {
        return Err(VesselError::Config(format!(
            "standalone diarization enrichment requires timestamped transcript segments: {}",
            existing.path.display()
        )));
    }

    let input = diarization_fixture_wav(&args.video_id)?;
    if !input.is_file() {
        return Err(VesselError::Config(format!(
            "durable diarization fixture is missing for {}; seed it first with: vessel diarization seed --video-id {}",
            args.video_id, args.video_id
        )));
    }

    let default_model_root = default_diarization_model_root();
    let (default_segmentation, default_embedding) = sherpa_model_paths(&default_model_root);
    let segmentation_model = args.segmentation_model.unwrap_or(default_segmentation);
    let embedding_model = args.embedding_model.unwrap_or(default_embedding);
    let runtime_root = args
        .runtime_dir
        .unwrap_or_else(default_diarization_runtime_root);
    let runtime_library = find_sherpa_runtime_library(&runtime_root).map_err(|_| {
        VesselError::Config(format!(
            "sherpa-onnx runtime is missing; run vessel diarization fetch or pass --runtime-dir explicitly (expected default under {})",
            runtime_root.display()
        ))
    })?;
    if !segmentation_model.is_file() || !embedding_model.is_file() {
        return Err(VesselError::Config(format!(
            "sherpa-onnx diarization models are missing; run vessel diarization fetch or pass explicit model paths (expected defaults under {})",
            default_model_root.display()
        )));
    }

    let config = DiarizationConfig {
        backend: SHERPA_ONNX_BACKEND_NAME.into(),
        runtime_library,
        segmentation_model,
        embedding_model,
        provider: args.provider,
        num_threads: args.num_threads,
        num_speakers: args.num_speakers,
        clustering_threshold: args.clustering_threshold,
        window_shift_ratio: args.window_shift_ratio,
        min_duration_on: 0.3,
        min_duration_off: 0.5,
        speaker_embeddings: true,
    };
    config.validate()?;

    let started = Instant::now();
    let (result, _backend) = acquire_local_diarization(&input, None, &config).await?;
    candidate.diarization = None;
    result.apply_to_candidate(&mut candidate)?;
    let evidence = result.to_speaker_evidence(&args.video_id, &candidate)?;
    let evidence_dir = sourcearium_root
        .join(".cache")
        .join("vessel")
        .join("speaker-evidence");
    let evidence_path = persist_speaker_evidence(&evidence_dir, &evidence)?;
    let applied = apply_youtube_transcript_diarization(&source, &args.video_id, &candidate)?;
    let transcript_segments = candidate.segments.len();
    let speaker_assigned_segments = candidate
        .segments
        .iter()
        .filter(|segment| segment.speaker.is_some())
        .count();
    let speaker_unresolved_segments = transcript_segments.saturating_sub(speaker_assigned_segments);

    let speaker_count = result
        .segments
        .iter()
        .map(|segment| segment.speaker.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let report = serde_json::json!({
        "status": "ok",
        "sourcearium_root": sourcearium_root,
        "source_key": args.source_key,
        "video_id": args.video_id,
        "transcript": applied.path,
        "transcript_updated": applied.updated,
        "speaker_attribution_cleared": applied.speaker_attribution_cleared,
        "evidence": evidence_path,
        "engine": result.engine,
        "model": result.model,
        "elapsed_seconds": started.elapsed().as_secs_f64(),
        "speaker_count": speaker_count,
        "segment_count": result.segments.len(),
        "speaker_embedding_count": result.speaker_embeddings.len(),
        "transcript_segments": transcript_segments,
        "speaker_assigned_segments": speaker_assigned_segments,
        "speaker_unresolved_segments": speaker_unresolved_segments,
        "network_io": false,
        "asr_invoked": false,
        "sourcearium_mutation": true,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|error| VesselError::Config(error.to_string()))?
    );
    Ok(())
}

async fn diarization_run(args: DiarizationRunArgs) -> Result<()> {
    let input = match (args.input.as_ref(), args.fixture_video_id.as_deref()) {
        (Some(path), None) => path.clone(),
        (None, Some(video_id)) => diarization_fixture_wav(video_id)?,
        (Some(_), Some(_)) => {
            return Err(VesselError::Config(
                "diarization run accepts either a WAV path or --fixture-video-id, not both".into(),
            ));
        }
        (None, None) => {
            return Err(VesselError::Config(
                "diarization run requires a WAV path or --fixture-video-id".into(),
            ));
        }
    };

    if args.backend != SHERPA_ONNX_BACKEND_NAME {
        return Err(VesselError::Config(format!(
            "standalone diarization run is not implemented for backend {:?}",
            args.backend
        )));
    }
    if !input.is_file() {
        let hint = args
            .fixture_video_id
            .as_deref()
            .map(|video_id| {
                format!("; seed it first with: vessel diarization seed --video-id {video_id}")
            })
            .unwrap_or_default();
        return Err(VesselError::Config(format!(
            "diarization input does not exist: {}{}",
            input.display(),
            hint
        )));
    }

    let default_model_root = default_diarization_model_root();
    let (default_segmentation, default_embedding) = sherpa_model_paths(&default_model_root);
    let using_default_models = args.segmentation_model.is_none() && args.embedding_model.is_none();
    let segmentation_model = args.segmentation_model.unwrap_or(default_segmentation);
    let embedding_model = args.embedding_model.unwrap_or(default_embedding);

    let using_default_runtime = args.runtime_dir.is_none();
    let runtime_root = args
        .runtime_dir
        .unwrap_or_else(default_diarization_runtime_root);
    let runtime_library = find_sherpa_runtime_library(&runtime_root).map_err(|_| {
        VesselError::Config(format!(
            "sherpa-onnx runtime is missing; run vessel diarization fetch or pass --runtime-dir explicitly (expected default under {})",
            runtime_root.display()
        ))
    })?;

    if !segmentation_model.is_file() || !embedding_model.is_file() {
        return Err(VesselError::Config(format!(
            "sherpa-onnx diarization models are missing; run vessel diarization fetch or pass explicit model paths (expected defaults under {})",
            default_model_root.display()
        )));
    }

    if using_default_runtime {
        let integrity = verify_integrity_receipt(
            &runtime_root.join(SHERPA_RUNTIME_RECEIPT_FILENAME),
            &runtime_root,
        );
        if integrity.present && !integrity.verified {
            return Err(VesselError::Config(format!(
                "default sherpa runtime failed offline integrity verification: {}",
                integrity.errors.join("; ")
            )));
        }
    }
    if using_default_models {
        let integrity = verify_integrity_receipt(
            &default_model_root.join(SHERPA_MODELS_RECEIPT_FILENAME),
            &default_model_root,
        );
        if integrity.present && !integrity.verified {
            return Err(VesselError::Config(format!(
                "default sherpa models failed offline integrity verification: {}",
                integrity.errors.join("; ")
            )));
        }
    }

    let config = DiarizationConfig {
        backend: SHERPA_ONNX_BACKEND_NAME.into(),
        runtime_library,
        segmentation_model,
        embedding_model,
        provider: args.provider,
        num_threads: args.num_threads,
        num_speakers: args.num_speakers,
        clustering_threshold: args.clustering_threshold,
        window_shift_ratio: args.window_shift_ratio,
        min_duration_on: 0.3,
        min_duration_off: 0.5,
        speaker_embeddings: args.speaker_embeddings,
    };
    config.validate()?;

    let started = Instant::now();
    let (result, _backend) = acquire_local_diarization(&input, None, &config).await?;

    let mut speaker_labels = result
        .segments
        .iter()
        .map(|segment| segment.speaker.clone())
        .collect::<Vec<_>>();
    speaker_labels.sort();
    speaker_labels.dedup();

    let segments = result
        .segments
        .iter()
        .map(|segment| {
            serde_json::json!({
                "start_seconds": segment.start_seconds,
                "end_seconds": segment.end_seconds,
                "speaker": segment.speaker,
            })
        })
        .collect::<Vec<_>>();

    let embedding_summaries = result
        .speaker_embeddings
        .iter()
        .map(|(speaker, values)| {
            let norm = values.iter().map(|value| value * value).sum::<f64>().sqrt();
            (
                speaker.clone(),
                serde_json::json!({
                    "dimension": values.len(),
                    "l2_norm": norm,
                }),
            )
        })
        .collect::<serde_json::Map<String, serde_json::Value>>();

    let report = serde_json::json!({
        "status": "ok",
        "backend": SHERPA_ONNX_BACKEND_NAME,
        "input": input,
        "fixture_video_id": args.fixture_video_id,
        "engine": result.engine,
        "model": result.model,
        "elapsed_seconds": started.elapsed().as_secs_f64(),
        "speaker_count": speaker_labels.len(),
        "speaker_labels": speaker_labels,
        "segment_count": segments.len(),
        "segments": segments,
        "speaker_embeddings": {
            "enabled": config.speaker_embeddings,
            "count": embedding_summaries.len(),
            "speakers": embedding_summaries,
        },
        "network_io": false,
        "sourcearium_mutation": false,
        "asr_invoked": false,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|error| VesselError::Config(error.to_string()))?
    );
    Ok(())
}

fn asr_models() -> Result<()> {
    let report = serde_json::json!({
        "backends": [
            {
                "name": vessel_asr::WHISPER_CANDLE_ENGINE_NAME,
                "status": "implemented",
                "default_model": "small",
                "models": vessel_asr::WHISPER_MODEL_NAMES,
                "languages": "multilingual",
                "offline_model_dir": true,
                "diarization": false,
            },
            {
                "name": vessel_asr::PHONON2_BACKEND_NAME,
                "status": "implemented",
                "default_model": "phonon-2",
                "models": vessel_asr::PHONON_MODEL_NAMES,
                "languages": ["en"],
                "offline_model_dir": true,
                "diarization": false,
            },
            {
                "name": vessel_asr::WHISPERX_BACKEND_NAME,
                "status": "implemented",
                "default_model": "large-v3",
                "languages": "multilingual",
                "offline_model_dir": true,
                "diarization": true,
            }
        ]
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|error| VesselError::Config(error.to_string()))?
    );
    Ok(())
}

fn asr_doctor(args: AsrDoctorArgs) -> Result<()> {
    let model_dir = args.model_dir.as_ref().map(|path| {
        serde_json::json!({
            "path": path,
            "exists": path.exists(),
            "is_directory": path.is_dir(),
        })
    });

    let report = match args.backend.as_str() {
        vessel_asr::WHISPER_CANDLE_ENGINE_NAME => serde_json::json!({
            "backend": args.backend,
            "implemented": true,
            "built_in": true,
            "executable_required": false,
            "executable_available": true,
            "model_dir": model_dir,
            "ready_for_transcription": args
                .model_dir
                .as_ref()
                .is_none_or(|path| path.is_dir()),
            "ready_for_diarization": false,
        }),
        vessel_asr::PHONON2_BACKEND_NAME => {
            let executable = args.executable.unwrap_or_else(|| PathBuf::from("fermion"));
            let executable_available = Command::new(&executable)
                .arg("--help")
                .output()
                .is_ok_and(|output| output.status.success());
            serde_json::json!({
                "backend": args.backend,
                "implemented": true,
                "built_in": false,
                "executable": executable,
                "executable_available": executable_available,
                "model_dir": model_dir,
                "ready_for_transcription": executable_available
                    && args.model_dir.as_ref().is_none_or(|path| path.is_dir()),
                "ready_for_diarization": false,
            })
        }
        vessel_asr::WHISPERX_BACKEND_NAME => {
            let executable = args.executable.unwrap_or_else(|| PathBuf::from("whisperx"));
            let executable_available = Command::new(&executable)
                .arg("--help")
                .output()
                .is_ok_and(|output| output.status.success());
            let hf_token_present =
                std::env::var(&args.hf_token_env).is_ok_and(|value| !value.trim().is_empty());
            serde_json::json!({
                "backend": args.backend,
                "implemented": true,
                "built_in": false,
                "executable": executable,
                "executable_available": executable_available,
                "model_dir": model_dir,
                "hf_token_env": args.hf_token_env,
                "hf_token_present": hf_token_present,
                "ready_for_transcription": executable_available
                    && args.model_dir.as_ref().is_none_or(|path| path.is_dir()),
                "ready_for_diarization": executable_available
                    && hf_token_present
                    && args.model_dir.as_ref().is_none_or(|path| path.is_dir()),
            })
        }
        other => {
            return Err(VesselError::Config(format!(
                "unsupported ASR backend {other:?}"
            )));
        }
    };

    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|error| VesselError::Config(error.to_string()))?
    );
    Ok(())
}

fn asr_fetch(args: AsrFetchArgs) -> Result<()> {
    let location = vessel_asr::fetch_asr_model(
        &args.backend,
        args.model.as_deref(),
        args.executable.as_deref(),
    )?;
    let directory = location.directory.display().to_string();
    let report = serde_json::json!({
        "status": "ok",
        "backend": location.backend,
        "model": location.model,
        "directory": directory,
        "reuse_with": {
            "asr_backend": args.backend,
            "asr_model_dir": directory,
        }
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|error| VesselError::Config(error.to_string()))?
    );
    Ok(())
}

fn absolute_sourcearium_root(path: PathBuf) -> Result<PathBuf> {
    let root = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()?.join(path)
    };
    if !root.join("sourcearium.toml").is_file() {
        return Err(VesselError::Corpus(format!(
            "{} does not look like a Sourcearium root; sourcearium.toml is missing",
            root.display()
        )));
    }
    Ok(root)
}

async fn sourcearium_update(args: UpdateArgs, config: &Config) -> Result<()> {
    validate_update_youtube_backend(&args)?;
    reject_retired_speaker_controls(&args)?;
    let asr_config = resolve_asr_config(&args);
    let diarization_config = resolve_diarization_config(&args)?;
    let report_items = args.report_items || args.preview;
    let sourcearium_root = if args.sourcearium.is_absolute() {
        args.sourcearium.clone()
    } else {
        std::env::current_dir()?.join(&args.sourcearium)
    };

    if !sourcearium_root.join("sourcearium.toml").is_file() {
        return Err(VesselError::Corpus(format!(
            "{} does not look like a Sourcearium root; sourcearium.toml is missing",
            sourcearium_root.display()
        )));
    }

    let sources = discover_youtube_sources(&sourcearium_root)?;
    let operational_db_path = sourcearium_root
        .join(".cache")
        .join("vessel")
        .join("vessel.sqlite");
    let (operational_store, _) = init_sqlite_database_path(&operational_db_path).await?;
    let mut asr_backend: Option<LoadedAsrBackend> = None;
    let mut diarization_backend: Option<SherpaOnnxDiarizer> = None;
    let mut source_reports = Vec::new();
    let mut remote_videos_processed = 0usize;
    let mut limit_reached = false;

    for source in sources {
        let policy = &source.policy;
        let mut summary = serde_json::json!({
            "source_key": policy.source_key,
            "channel": policy.channel.input,
            "discovered": 0,
            "explicitly_excluded": 0,
            "outside_date_policy": 0,
            "already_strongest": 0,
            "existing_unmanaged": 0,
            "upgrade_check_deferred": 0,
            "upgrade_checks_completed": 0,
            "created": 0,
            "updated": 0,
            "unchanged": 0,
            "preserved_stronger": 0,
            "preserved_unknown_derivation": 0,
            "preserved_without_better_caption": 0,
            "unresolved_date": 0,
            "requires_local_asr": 0,
            "local_asr_attempted": 0,
            "local_asr_fixture_reused": 0,
            "local_asr_materialized": 0,
            "diarization_attempted": 0,
            "diarization_completed": 0,
            "diarization_skipped": 0,
            "caption_access_degraded": 0,
            "caption_empty_response_tracks": 0,
            "unresolved_no_provider": 0,
            "errors": [],
        });
        if report_items {
            summary["items"] = serde_json::json!([]);
        }
        summary["preview"] = serde_json::Value::Bool(args.preview);
        summary["youtube_backend"] = serde_json::Value::String(args.youtube_backend.clone());

        if !policy.transcripts.enabled {
            summary["status"] = serde_json::Value::String("transcripts_disabled".into());
            source_reports.push(summary);
            continue;
        }
        if args.force_local_asr && !policy.transcripts.allow_local_asr {
            summary["status"] = serde_json::Value::String("force_local_asr_disallowed".into());
            summary["errors"]
                .as_array_mut()
                .expect("errors array")
                .push(serde_json::json!({
                    "message": "--force-local-asr was requested but this Sourcearium policy has allow_local_asr = false",
                }));
            source_reports.push(summary);
            continue;
        }

        let channel_input = parse_sourcearium_channel_input(&policy.channel.input)?;
        let (channel, external_crawl) =
            match discover_update_channel(&args, config, &channel_input).await {
                Ok(discovery) => discovery,
                Err(error) => {
                    summary["status"] =
                        serde_json::Value::String("channel_resolution_failed".into());
                    summary["errors"]
                        .as_array_mut()
                        .expect("errors array")
                        .push(serde_json::json!({"message": error.to_string()}));
                    source_reports.push(summary);
                    continue;
                }
            };
        summary["channel_discovery_backend"] =
            serde_json::Value::String(args.youtube_backend.clone());

        if let Some(expected_channel_id) = policy.channel.id.as_deref()
            && channel.channel_id != expected_channel_id
        {
            summary["status"] = serde_json::Value::String("channel_identity_mismatch".into());
            summary["errors"]
                .as_array_mut()
                .expect("errors array")
                .push(serde_json::json!({
                    "message": "resolved channel id does not match Sourcearium policy",
                    "expected": expected_channel_id,
                    "actual": channel.channel_id,
                }));
            source_reports.push(summary);
            continue;
        }

        if let Err(error) = operational_store
            .add_tracked_channel(&channel, &policy.source_key)
            .await
        {
            summary["status"] = serde_json::Value::String("operational_state_failed".into());
            summary["errors"]
                .as_array_mut()
                .expect("errors array")
                .push(serde_json::json!({"message": error.to_string()}));
            source_reports.push(summary);
            continue;
        }

        let existing_cursors = if external_crawl.is_some() {
            Vec::new()
        } else {
            match operational_store
                .load_channel_tab_cursors(&channel.channel_id)
                .await
            {
                Ok(cursors) => cursors
                    .into_iter()
                    .map(|cursor| ChannelTabCursor {
                        tab_name: cursor.tab_name,
                        continuation_token: cursor.continuation_token,
                        visitor_data: cursor.visitor_data,
                        delegated_session_id: cursor.delegated_session_id,
                        last_seen_published_at: cursor.last_seen_published_at,
                        backfill_complete: cursor.backfill_complete,
                    })
                    .collect::<Vec<_>>(),
                Err(error) => {
                    summary["status"] =
                        serde_json::Value::String("operational_state_failed".into());
                    summary["errors"]
                        .as_array_mut()
                        .expect("errors array")
                        .push(serde_json::json!({"message": error.to_string()}));
                    source_reports.push(summary);
                    continue;
                }
            }
        };
        summary["existing_tab_cursors"] = serde_json::Value::from(existing_cursors.len() as u64);

        let crawl = if let Some(crawl) = external_crawl {
            crawl
        } else {
            match crawl_channel_videos(&channel_input, &existing_cursors).await {
                Ok(crawl) => crawl,
                Err(error) => {
                    summary["status"] =
                        serde_json::Value::String("channel_crawl_failed".into());
                    summary["errors"]
                        .as_array_mut()
                        .expect("errors array")
                        .push(serde_json::json!({"message": error.to_string()}));
                    source_reports.push(summary);
                    continue;
                }
            }
        };
        summary["discovered"] = serde_json::Value::from(crawl.videos.len() as u64);
        summary["tabs_visited"] = serde_json::json!(crawl.tabs_visited);
        summary["tabs_completed"] = serde_json::json!(crawl.tabs_completed);
        summary["tabs_resumed_from_checkpoint"] =
            serde_json::json!(crawl.tabs_resumed_from_checkpoint);

        let mut membership_persisted = true;
        for video_ref in &crawl.videos {
            if let Err(error) = operational_store
                .record_channel_video_membership(
                    &channel.channel_id,
                    &video_ref.video_id,
                    &video_ref.tab_name,
                )
                .await
            {
                membership_persisted = false;
                push_update_error(&mut summary, &video_ref.video_id, error);
            }
        }
        summary["operational_membership_persisted"] = serde_json::Value::Bool(membership_persisted);

        let mut cursor_state_advanced = false;
        if membership_persisted {
            let mut cursor_writes_succeeded = true;
            for cursor in &crawl.cursors {
                if let Err(error) = operational_store
                    .save_channel_tab_cursor(
                        &channel.channel_id,
                        &cursor.tab_name,
                        cursor.continuation_token.as_deref(),
                        cursor.visitor_data.as_deref(),
                        cursor.delegated_session_id.as_deref(),
                        cursor.last_seen_published_at.as_deref(),
                        cursor.backfill_complete,
                    )
                    .await
                {
                    cursor_writes_succeeded = false;
                    summary["errors"]
                        .as_array_mut()
                        .expect("errors array")
                        .push(serde_json::json!({"message": error.to_string()}));
                }
            }

            if cursor_writes_succeeded {
                cursor_state_advanced = true;
                if let Err(error) = operational_store
                    .mark_tracked_channel_synced(&channel.channel_id)
                    .await
                {
                    summary["errors"]
                        .as_array_mut()
                        .expect("errors array")
                        .push(serde_json::json!({"message": error.to_string()}));
                }
            }
        }
        summary["cursor_state_advanced"] = serde_json::Value::Bool(cursor_state_advanced);

        let backlog_ids = match operational_store
            .list_channel_video_ids(&channel.channel_id)
            .await
        {
            Ok(ids) => ids,
            Err(error) => {
                summary["errors"]
                    .as_array_mut()
                    .expect("errors array")
                    .push(serde_json::json!({"message": error.to_string()}));
                Vec::new()
            }
        };
        summary["known_backlog"] = serde_json::Value::from(backlog_ids.len() as u64);

        let mut candidates = crawl.videos;
        let mut seen_video_ids = candidates
            .iter()
            .map(|video| video.video_id.clone())
            .collect::<HashSet<_>>();
        for video_id in backlog_ids {
            if seen_video_ids.insert(video_id.clone()) {
                candidates.push(ChannelVideoRef {
                    video_id,
                    tab_name: "operational_backlog".into(),
                    title: None,
                    published_at: None,
                });
            }
        }

        if !args.video_ids.is_empty() {
            candidates.retain(|video| args.video_ids.contains(&video.video_id));
            summary["video_filter"] = serde_json::json!(&args.video_ids);
            summary["video_filter_matches"] = serde_json::Value::from(candidates.len() as u64);
        }

        for video_ref in candidates {
            let existing = match load_youtube_transcript_artifact(&source, &video_ref.video_id) {
                Ok(existing) => existing,
                Err(error) => {
                    push_update_error(&mut summary, &video_ref.video_id, error);
                    continue;
                }
            };

            let cached_published_on = match operational_store
                .load_source_video_published_on(&video_ref.video_id)
                .await
            {
                Ok(value) => value,
                Err(error) => {
                    push_update_error(&mut summary, &video_ref.video_id, error);
                    None
                }
            };
            let known_date = existing
                .as_ref()
                .and_then(|existing| existing.artifact.source.published.as_deref())
                .or(cached_published_on.as_deref());

            let initial_selection = match policy.select_video(&video_ref.video_id, known_date) {
                Ok(selection) => selection,
                Err(error) => {
                    push_update_error(&mut summary, &video_ref.video_id, error);
                    continue;
                }
            };

            match initial_selection {
                VideoSelection::ExplicitlyExcluded => {
                    increment_summary(&mut summary, "explicitly_excluded", 1);
                    push_update_item(
                        &mut summary,
                        report_items,
                        &video_ref.video_id,
                        "excluded_explicit",
                        serde_json::json!({}),
                    );
                    continue;
                }
                VideoSelection::BeforeCutoff => {
                    increment_summary(&mut summary, "outside_date_policy", 1);
                    push_update_item(
                        &mut summary,
                        report_items,
                        &video_ref.video_id,
                        "excluded_before_cutoff",
                        serde_json::json!({"published_on": known_date}),
                    );
                    continue;
                }
                VideoSelection::Included
                | VideoSelection::ExplicitlyIncluded
                | VideoSelection::PublicationDateUnresolved => {}
            }

            if let Some(existing_artifact) = existing.as_ref() {
                match existing_artifact
                    .artifact
                    .representation
                    .derivation
                    .as_str()
                {
                    "creator_subtitles" => {
                        increment_summary(&mut summary, "already_strongest", 1);
                        push_update_item(
                            &mut summary,
                            report_items,
                            &video_ref.video_id,
                            "already_strongest",
                            serde_json::json!({"derivation": "creator_subtitles"}),
                        );
                        continue;
                    }
                    "platform_auto_caption" | "local_asr" => {
                        let last_probe = match operational_store
                            .load_transcript_probe_at(&video_ref.video_id)
                            .await
                        {
                            Ok(last_probe) => last_probe,
                            Err(error) => {
                                push_update_error(&mut summary, &video_ref.video_id, error);
                                None
                            }
                        };
                        if !args.force_local_asr
                            && !transcript_upgrade_probe_due(
                                last_probe.as_deref(),
                                args.upgrade_check_days,
                                OffsetDateTime::now_utc(),
                            )
                        {
                            increment_summary(&mut summary, "upgrade_check_deferred", 1);
                            push_update_item(
                                &mut summary,
                                report_items,
                                &video_ref.video_id,
                                "upgrade_check_deferred",
                                serde_json::json!({"derivation": existing_artifact.artifact.representation.derivation}),
                            );
                            continue;
                        }
                    }
                    _ => {
                        increment_summary(&mut summary, "existing_unmanaged", 1);
                        push_update_item(
                            &mut summary,
                            report_items,
                            &video_ref.video_id,
                            "existing_unmanaged",
                            serde_json::json!({"derivation": existing_artifact.artifact.representation.derivation}),
                        );
                        continue;
                    }
                }
            }

            if args
                .max_videos
                .is_some_and(|limit| remote_videos_processed >= limit)
            {
                limit_reached = true;
                break;
            }
            remote_videos_processed += 1;

            let video_input = InputRef {
                raw: video_ref.video_id.clone(),
                kind: InputKind::VideoId,
            };
            let video = match extract_update_video(&args, config, &video_input).await
            {
                Ok(video) => video,
                Err(error) => {
                    push_update_error(&mut summary, &video_ref.video_id, error);
                    continue;
                }
            };

            if let Some(expected_channel_id) = policy.channel.id.as_deref()
                && video.channel_id.as_deref() != Some(expected_channel_id)
            {
                push_update_error(
                    &mut summary,
                    &video_ref.video_id,
                    VesselError::Corpus(format!(
                        "video belongs to channel {:?}, expected {expected_channel_id}",
                        video.channel_id
                    )),
                );
                continue;
            }

            let publication_date = normalize_update_publication_date(video.upload_date.as_deref());
            if let Some(published_on) = publication_date.as_deref()
                && let Err(error) = operational_store
                    .save_source_video_published_on(
                        &video.video_id,
                        video.channel_id.as_deref(),
                        published_on,
                    )
                    .await
            {
                push_update_error(&mut summary, &video.video_id, error);
            }
            let selection = match policy.select_video(&video.video_id, publication_date.as_deref())
            {
                Ok(selection) => selection,
                Err(error) => {
                    push_update_error(&mut summary, &video.video_id, error);
                    continue;
                }
            };

            match selection {
                VideoSelection::ExplicitlyExcluded => {
                    increment_summary(&mut summary, "explicitly_excluded", 1);
                    push_update_item(
                        &mut summary,
                        report_items,
                        &video.video_id,
                        "excluded_explicit",
                        serde_json::json!({
                            "stage": "resolved_video",
                        }),
                    );
                    continue;
                }
                VideoSelection::BeforeCutoff => {
                    increment_summary(&mut summary, "outside_date_policy", 1);
                    push_update_item(
                        &mut summary,
                        report_items,
                        &video.video_id,
                        "excluded_before_cutoff",
                        serde_json::json!({
                            "stage": "resolved_video",
                            "published_on": publication_date,
                        }),
                    );
                    continue;
                }
                VideoSelection::PublicationDateUnresolved => {
                    increment_summary(&mut summary, "unresolved_date", 1);
                    push_update_item(
                        &mut summary,
                        report_items,
                        &video.video_id,
                        "unresolved_publication_date",
                        serde_json::json!({}),
                    );
                    continue;
                }
                VideoSelection::Included | VideoSelection::ExplicitlyIncluded => {}
            }

            let (caption_candidate, empty_response_derivations) = if args.force_local_asr {
                (None, Vec::new())
            } else {
                let caption_acquisition =
                    match acquire_best_caption_candidate(&video, &policy.transcripts).await {
                        Ok(acquisition) => acquisition,
                        Err(error) => {
                            push_update_error(&mut summary, &video.video_id, error);
                            continue;
                        }
                    };
                (
                    caption_acquisition.candidate,
                    caption_acquisition
                        .empty_response_derivations
                        .iter()
                        .map(|derivation| derivation.as_str())
                        .collect::<Vec<_>>(),
                )
            };
            if !empty_response_derivations.is_empty() {
                increment_summary(&mut summary, "caption_access_degraded", 1);
                increment_summary(
                    &mut summary,
                    "caption_empty_response_tracks",
                    empty_response_derivations.len(),
                );
            }
            let caption_probe = serde_json::json!({
                "advertised_tracks": video.subtitles.len(),
                "empty_response_derivations": empty_response_derivations,
                "force_local_asr": args.force_local_asr,
                "caption_fetch_skipped": args.force_local_asr,
            });

            let (mut candidate, asr_cache_dir) = if let Some(candidate) = caption_candidate {
                (candidate, None)
            } else if existing.is_some() && !args.force_local_asr {
                increment_summary(&mut summary, "preserved_without_better_caption", 1);
                push_update_item(
                    &mut summary,
                    report_items,
                    &video.video_id,
                    if args.preview {
                        "would_preserve_without_better_caption"
                    } else {
                        "preserved_without_better_caption"
                    },
                    serde_json::json!({
                        "caption_probe": caption_probe,
                    }),
                );
                if !args.preview {
                    match operational_store
                        .mark_transcript_probed(&video.video_id)
                        .await
                    {
                        Ok(()) => increment_summary(&mut summary, "upgrade_checks_completed", 1),
                        Err(error) => push_update_error(&mut summary, &video.video_id, error),
                    }
                }
                continue;
            } else if policy.transcripts.allow_local_asr {
                increment_summary(&mut summary, "requires_local_asr", 1);
                if args.preview {
                    push_update_item(
                        &mut summary,
                        report_items,
                        &video.video_id,
                        "would_require_local_asr",
                        serde_json::json!({
                            "backend": &asr_config.backend,
                            "force_local_asr": args.force_local_asr,
                            "model": &asr_config.model,
                            "model_dir": &asr_config.model_dir,
                            "executable": &asr_config.executable,
                            "device": &asr_config.device,
                            "diarize": args.diarize,
                            "diarization_backend": &args.diarization_backend,
                            "diarization_segmentation_model": &args.diarization_segmentation_model,
                            "diarization_embedding_model": &args.diarization_embedding_model,
                            "caption_probe": caption_probe,
                        }),
                    );
                    continue;
                }
                increment_summary(&mut summary, "local_asr_attempted", 1);
                let yt_dlp = (args.youtube_backend == YT_DLP_BACKEND_NAME)
                    .then(|| update_yt_dlp_config(&args, config));
                match acquire_local_asr_candidate(
                    &sourcearium_root,
                    &video,
                    asr_backend.take(),
                    &asr_config,
                    yt_dlp.as_ref(),
                )
                .await
                {
                    Ok((candidate, cache_dir, backend, fixture_reused)) => {
                        asr_backend = Some(backend);
                        if fixture_reused {
                            increment_summary(&mut summary, "local_asr_fixture_reused", 1);
                            push_update_item(
                                &mut summary,
                                report_items,
                                &video.video_id,
                                "local_asr_fixture_reused",
                                serde_json::json!({
                                    "fixture": diarization_fixture_wav(&video.video_id)?,
                                    "network_media_download": false,
                                }),
                            );
                        }
                        (candidate, Some(cache_dir))
                    }
                    Err(error) => {
                        push_update_error(&mut summary, &video.video_id, error);
                        continue;
                    }
                }
            } else {
                increment_summary(&mut summary, "unresolved_no_provider", 1);
                push_update_item(
                    &mut summary,
                    report_items,
                    &video.video_id,
                    "unresolved_no_provider",
                    serde_json::json!({
                        "caption_probe": caption_probe,
                    }),
                );
                continue;
            };

            if let Some(config) = diarization_config.as_ref() {
                if let Some(cache_dir) = asr_cache_dir.as_ref() {
                    increment_summary(&mut summary, "diarization_attempted", 1);
                    let wav = cache_dir.join("whisper-input.wav");
                    match acquire_local_diarization(&wav, diarization_backend.take(), config).await
                    {
                        Ok((result, backend)) => {
                            diarization_backend = Some(backend);
                            if let Err(error) = result.apply_to_candidate(&mut candidate) {
                                push_update_error(&mut summary, &video.video_id, error);
                                continue;
                            }
                            let evidence =
                                match result.to_speaker_evidence(&video.video_id, &candidate) {
                                    Ok(evidence) => evidence,
                                    Err(error) => {
                                        push_update_error(&mut summary, &video.video_id, error);
                                        continue;
                                    }
                                };
                            let evidence_dir = sourcearium_root
                                .join(".cache")
                                .join("vessel")
                                .join("speaker-evidence");
                            match persist_speaker_evidence(&evidence_dir, &evidence) {
                                Ok(path) => {
                                    increment_summary(&mut summary, "diarization_completed", 1);
                                    push_update_item(
                                        &mut summary,
                                        report_items,
                                        &video.video_id,
                                        "diarized",
                                        serde_json::json!({
                                            "backend": result.engine,
                                            "model": result.model,
                                            "segments": result.segments.len(),
                                            "speaker_embeddings": result.speaker_embeddings.len(),
                                            "evidence": path,
                                        }),
                                    );
                                }
                                Err(error) => {
                                    push_update_error(&mut summary, &video.video_id, error);
                                    continue;
                                }
                            }
                        }
                        Err(error) => {
                            push_update_error(&mut summary, &video.video_id, error);
                            continue;
                        }
                    }
                } else {
                    increment_summary(&mut summary, "diarization_skipped", 1);
                    push_update_item(
                        &mut summary,
                        report_items,
                        &video.video_id,
                        "diarization_skipped",
                        serde_json::json!({
                            "reason": "Rust-native diarization currently runs on the normalized local-ASR audio path; caption-backed audio preparation is not wired yet",
                        }),
                    );
                }
            }

            if args.preview {
                let existing_derivation = existing
                    .as_ref()
                    .map(|artifact| artifact.artifact.representation.derivation.as_str());
                let action =
                    preview_materialization_action(existing_derivation, candidate.derivation);
                push_update_item(
                    &mut summary,
                    report_items,
                    &video.video_id,
                    action,
                    serde_json::json!({
                        "candidate_derivation": candidate.derivation.as_str(),
                        "language": candidate.language,
                    }),
                );
                continue;
            }

            match materialize_youtube_transcript(
                &sourcearium_root,
                &source,
                &video,
                Some(&channel),
                &candidate,
            ) {
                Ok(result) => {
                    let materialize_action = match result.status {
                        MaterializeStatus::Created => {
                            increment_summary(&mut summary, "created", 1);
                            "created"
                        }
                        MaterializeStatus::Updated => {
                            increment_summary(&mut summary, "updated", 1);
                            "updated"
                        }
                        MaterializeStatus::Unchanged => {
                            increment_summary(&mut summary, "unchanged", 1);
                            "unchanged"
                        }
                        MaterializeStatus::PreservedStronger => {
                            increment_summary(&mut summary, "preserved_stronger", 1);
                            "preserved_stronger"
                        }
                        MaterializeStatus::PreservedUnknownDerivation => {
                            increment_summary(&mut summary, "preserved_unknown_derivation", 1);
                            "preserved_unknown_derivation"
                        }
                    };
                    push_update_item(
                        &mut summary,
                        report_items,
                        &video.video_id,
                        materialize_action,
                        serde_json::json!({
                            "derivation": candidate.derivation.as_str(),
                            "path": result.path,
                        }),
                    );

                    match operational_store
                        .mark_transcript_probed(&video.video_id)
                        .await
                    {
                        Ok(()) => increment_summary(&mut summary, "upgrade_checks_completed", 1),
                        Err(error) => push_update_error(&mut summary, &video.video_id, error),
                    }

                    if let Some(cache_dir) = asr_cache_dir {
                        increment_summary(&mut summary, "local_asr_materialized", 1);
                        if let Err(error) = tokio::fs::remove_dir_all(&cache_dir).await {
                            warn!(
                                target: "asr",
                                cache = %cache_dir.display(),
                                error = %error,
                                "ASR transcript materialized but temporary cache cleanup failed"
                            );
                        }
                    }
                }
                Err(error) => push_update_error(&mut summary, &video.video_id, error),
            }
        }

        let source_has_errors = summary["errors"]
            .as_array()
            .is_some_and(|errors| !errors.is_empty());
        summary["status"] =
            serde_json::Value::String(if source_has_errors { "partial" } else { "ok" }.into());
        source_reports.push(summary);

        if limit_reached {
            break;
        }
    }

    let total_errors = source_reports
        .iter()
        .filter_map(|source| source["errors"].as_array())
        .map(Vec::len)
        .sum::<usize>();
    let report_status = if total_errors > 0 {
        "partial"
    } else if limit_reached {
        "limited"
    } else {
        "ok"
    };

    operational_store.checkpoint_and_close().await?;

    let report = serde_json::json!({
        "status": report_status,
        "error_count": total_errors,
        "sourcearium_root": sourcearium_root,
        "operational_state": {
            "sqlite": operational_db_path,
            "checkpointed": true,
        },
        "sources": source_reports,
        "remote_videos_processed": remote_videos_processed,
        "max_videos": args.max_videos,
        "upgrade_check_days": args.upgrade_check_days,
        "preview": args.preview,
        "limit_reached": limit_reached,
        "local_asr_implemented": true,
        "diarization": {
            "enabled": args.diarize,
            "backend": args.diarization_backend,
            "segmentation_model": args.diarization_segmentation_model,
            "embedding_model": args.diarization_embedding_model,
            "runtime_dir": args.diarization_runtime_dir,
            "runtime_loading": "dynamic_at_execution",
            "build_time_fetch": false,
            "provider": args.diarization_provider,
            "num_threads": args.diarization_num_threads,
            "speaker_embeddings": diarization_config
                .as_ref()
                .map(|config| config.speaker_embeddings)
                .unwrap_or(asr_config.speaker_embeddings),
            "backend_loaded": diarization_backend.is_some(),
        },
        "asr": {
            "engine": asr_config.backend,
            "force_local_asr": args.force_local_asr,
            "model": asr_config.model,
            "model_dir": asr_config.model_dir,
            "executable": asr_config.executable,
            "device": asr_config.device,
            "language": asr_config.language,
            "diarize": asr_config.diarize,
            "diarization_model": asr_config.diarization_model,
            "min_speakers": asr_config.min_speakers,
            "max_speakers": asr_config.max_speakers,
            "speaker_embeddings": asr_config.speaker_embeddings,
            "model_loaded": asr_backend.is_some(),
        },
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|error| VesselError::Config(error.to_string()))?
    );
    Ok(())
}

fn preview_materialization_action(
    existing_derivation: Option<&str>,
    candidate: vessel_core::TranscriptDerivation,
) -> &'static str {
    match existing_derivation {
        None => "would_create",
        Some("local_asr")
            if candidate.quality_rank()
                > vessel_core::TranscriptDerivation::LocalAsr.quality_rank() =>
        {
            "would_upgrade"
        }
        Some("platform_auto_caption")
            if candidate == vessel_core::TranscriptDerivation::CreatorSubtitles =>
        {
            "would_upgrade"
        }
        Some(_) => "would_compare_content",
    }
}

fn transcript_upgrade_probe_due(
    last_probed_at: Option<&str>,
    interval_days: u64,
    now: OffsetDateTime,
) -> bool {
    if interval_days == 0 {
        return true;
    }

    let Some(last_probed_at) = last_probed_at else {
        return true;
    };
    let Ok(last_probed_at) = OffsetDateTime::parse(
        last_probed_at,
        &time::format_description::well_known::Rfc3339,
    ) else {
        return true;
    };

    let elapsed_seconds = now
        .unix_timestamp()
        .saturating_sub(last_probed_at.unix_timestamp());
    elapsed_seconds >= 0 && (elapsed_seconds as u64) >= interval_days.saturating_mul(86_400)
}

fn reject_retired_speaker_controls(args: &UpdateArgs) -> Result<()> {
    if args.attribute_speakers || args.speaker_embeddings {
        return Err(VesselError::Unsupported(
            "named-speaker identity/embedding controls are retired from active Vessel; use --diarize with the WhisperX backend for anonymous speaker separation".into(),
        ));
    }
    Ok(())
}

fn resolve_diarization_config(args: &UpdateArgs) -> Result<Option<DiarizationConfig>> {
    if !args.diarize || args.diarization_backend == "whisperx" {
        return Ok(None);
    }
    if args.diarization_backend != SHERPA_ONNX_BACKEND_NAME {
        return Err(VesselError::Config(format!(
            "unsupported diarization backend {:?}",
            args.diarization_backend
        )));
    }

    let default_root = default_diarization_model_root();
    let (default_segmentation, default_embedding) = sherpa_model_paths(&default_root);
    let runtime_root = args
        .diarization_runtime_dir
        .clone()
        .unwrap_or_else(default_diarization_runtime_root);
    let runtime_library = find_sherpa_runtime_library(&runtime_root).map_err(|_| {
        VesselError::Config(format!(
            "sherpa-onnx runtime is missing; run vessel diarization fetch or pass --diarization-runtime-dir explicitly (expected default under {})",
            runtime_root.display()
        ))
    })?;
    let segmentation_model = args
        .diarization_segmentation_model
        .clone()
        .unwrap_or(default_segmentation);
    let embedding_model = args
        .diarization_embedding_model
        .clone()
        .unwrap_or(default_embedding);
    if !segmentation_model.is_file() || !embedding_model.is_file() {
        return Err(VesselError::Config(format!(
            "sherpa-onnx diarization models are missing; run vessel diarization fetch or pass --diarization-segmentation-model and --diarization-embedding-model explicitly (expected defaults under {})",
            default_root.display()
        )));
    }

    let num_speakers = match (args.min_speakers, args.max_speakers) {
        (None, None) => None,
        (Some(min), Some(max)) if min == max => Some(min),
        (Some(_), Some(_)) => {
            return Err(VesselError::Config(
                "sherpa-onnx currently accepts an exact speaker count; set --min-speakers and --max-speakers to the same value, or omit both".into(),
            ));
        }
        _ => {
            return Err(VesselError::Config(
                "sherpa-onnx speaker-count hints require both --min-speakers and --max-speakers with the same value".into(),
            ));
        }
    };

    let config = DiarizationConfig {
        backend: SHERPA_ONNX_BACKEND_NAME.into(),
        runtime_library,
        segmentation_model,
        embedding_model,
        provider: args.diarization_provider.clone(),
        num_threads: args.diarization_num_threads,
        num_speakers,
        clustering_threshold: args.diarization_clustering_threshold,
        window_shift_ratio: args.diarization_window_shift_ratio,
        min_duration_on: 0.3,
        min_duration_off: 0.5,
        speaker_embeddings: false,
    };
    config.validate()?;
    Ok(Some(config))
}

fn validate_update_youtube_backend(args: &UpdateArgs) -> Result<()> {
    match args.youtube_backend.as_str() {
        YT_DLP_BACKEND_NAME | "native" => Ok(()),
        other => Err(VesselError::Config(format!(
            "unsupported YouTube backend {other:?}; expected yt-dlp or native"
        ))),
    }
}

fn update_yt_dlp_config(args: &UpdateArgs, config: &Config) -> YtDlpConfig {
    YtDlpConfig::new(
        args.yt_dlp_executable.clone(),
        config.youtube.cookies_from_browser.clone(),
    )
}

async fn discover_update_channel(
    args: &UpdateArgs,
    config: &Config,
    input: &InputRef,
) -> Result<(ChannelMetadata, Option<ChannelVideoCrawlReport>)> {
    match args.youtube_backend.as_str() {
        YT_DLP_BACKEND_NAME => {
            let (channel, crawl) = update_yt_dlp_config(args, config).discover_channel(input).await?;
            Ok((channel, Some(crawl)))
        }
        "native" => Ok((extract_channel(input).await?, None)),
        other => Err(VesselError::Config(format!(
            "unsupported YouTube backend {other:?}; expected yt-dlp or native"
        ))),
    }
}

async fn extract_update_video(
    args: &UpdateArgs,
    config: &Config,
    input: &InputRef,
) -> Result<VideoMetadata> {
    match args.youtube_backend.as_str() {
        YT_DLP_BACKEND_NAME => update_yt_dlp_config(args, config).extract_video(input).await,
        "native" => extract_video(input).await,
        other => Err(VesselError::Config(format!(
            "unsupported YouTube backend {other:?}; expected yt-dlp or native"
        ))),
    }
}

fn resolve_asr_config(args: &UpdateArgs) -> AsrConfig {
    let mut config = AsrConfig::default();
    if args.asr_backend.is_none() {
        config.backend = vessel_asr::WHISPERX_BACKEND_NAME.to_owned();
        config.model = "large-v3".to_owned();
    }
    if let Some(backend) = args.asr_backend.as_deref() {
        config.backend = backend.to_owned();
        if args.asr_model.is_none() {
            config.model = match backend {
                vessel_asr::PHONON2_BACKEND_NAME => vessel_asr::PHONON2_BACKEND_NAME.to_owned(),
                vessel_asr::WHISPERX_BACKEND_NAME => "large-v3".to_owned(),
                _ => config.model,
            };
        }
    }
    if let Some(model) = args.asr_model.as_deref() {
        config.model = model.to_owned();
    }
    if let Some(model_dir) = args.asr_model_dir.as_ref() {
        config.model_dir = Some(model_dir.clone());
    }
    if let Some(executable) = args.asr_executable.as_ref() {
        config.executable = Some(executable.clone());
    }
    if let Some(device) = args.asr_device.as_deref() {
        config.device = device.to_owned();
    }
    if let Some(language) = args.asr_language.as_deref() {
        config.language = Some(language.to_owned());
    }
    config.diarize = args.diarize && args.diarization_backend == "whisperx";
    if let Some(model) = args.diarization_model.as_deref() {
        config.diarization_model = model.to_owned();
    }
    config.min_speakers = config.diarize.then_some(args.min_speakers).flatten();
    config.max_speakers = config.diarize.then_some(args.max_speakers).flatten();
    config.speaker_embeddings = false;
    config.hf_token_env = args.hf_token_env.clone();
    config
}

async fn acquire_local_diarization(
    wav: &Path,
    backend: Option<SherpaOnnxDiarizer>,
    config: &DiarizationConfig,
) -> Result<(DiarizationResult, SherpaOnnxDiarizer)> {
    if cfg!(debug_assertions) {
        warn!(
            target: "diarization",
            "speaker diarization is running from an unoptimized debug build; use cargo build --release for real inference work"
        );
    }

    let config = config.clone();
    let heartbeat_input = wav.to_path_buf();
    let wav_for_worker = wav.to_path_buf();
    let (heartbeat_stop_tx, heartbeat_stop_rx) = std::sync::mpsc::channel::<()>();
    let heartbeat = std::thread::spawn(move || {
        let started = Instant::now();
        loop {
            match heartbeat_stop_rx.recv_timeout(Duration::from_secs(15)) {
                Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    eprintln!(
                        "[diarization] still running backend={} input={} elapsed={:.0}s",
                        SHERPA_ONNX_BACKEND_NAME,
                        heartbeat_input.display(),
                        started.elapsed().as_secs_f64(),
                    );
                }
            }
        }
    });

    let worker_result = tokio::task::spawn_blocking(move || {
        let backend = match backend {
            Some(backend) => backend,
            None => SherpaOnnxDiarizer::load(config)?,
        };
        let result = backend.process_path(&wav_for_worker)?;
        Ok::<_, VesselError>((result, backend))
    })
    .await
    .map_err(|error| VesselError::Extractor(format!("diarization worker failed to join: {error}")));

    let _ = heartbeat_stop_tx.send(());
    let _ = heartbeat.join();

    worker_result?
}

fn reuse_normalized_audio_fixture(source: &Path, destination: &Path) -> Result<()> {
    if std::fs::metadata(source)?.len() <= 44 {
        return Err(VesselError::Extractor(format!(
            "durable diarization fixture is too small to be valid audio: {}",
            source.display()
        )));
    }
    match std::fs::hard_link(source, destination) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(source, destination)?;
            Ok(())
        }
    }
}

async fn acquire_local_asr_candidate(
    sourcearium_root: &Path,
    video: &VideoMetadata,
    backend: Option<LoadedAsrBackend>,
    config: &AsrConfig,
    yt_dlp: Option<&YtDlpConfig>,
) -> Result<(TranscriptCandidate, PathBuf, LoadedAsrBackend, bool)> {
    if cfg!(debug_assertions) {
        warn!(
            target: "asr",
            "local ASR is running from an unoptimized debug build; use cargo build --release for real transcription work"
        );
    }
    let cache_dir = sourcearium_root
        .join(".cache")
        .join("vessel")
        .join("asr")
        .join(&video.video_id);
    tokio::fs::create_dir_all(&cache_dir).await?;

    let whisper_wav = cache_dir.join("whisper-input.wav");
    let durable_fixture = diarization_fixture_wav(&video.video_id)?;
    let mut fixture_reused = false;

    if whisper_wav.is_file() {
        info!(
            target: "asr",
            input = %whisper_wav.display(),
            "reusing cached normalized ASR audio"
        );
    } else if durable_fixture.is_file() {
        reuse_normalized_audio_fixture(&durable_fixture, &whisper_wav)?;
        info!(
            target: "asr",
            fixture = %durable_fixture.display(),
            input = %whisper_wav.display(),
            "reusing durable diarization fixture for ASR; no media download required"
        );
        fixture_reused = true;
    } else {
        let source_audio = if let Some(yt_dlp) = yt_dlp {
            let output_template = cache_dir.join("source.%(ext)s");
            info!(
                target: "asr",
                video_id = %video.video_id,
                "acquiring ASR source audio through yt-dlp"
            );
            yt_dlp
                .download_best_audio(&video.video_id, &output_template)
                .await?
        } else {
            let output_template = cache_dir
                .join("source.%(ext)s")
                .to_string_lossy()
                .into_owned();
            let planner = BasicDownloadPlanner;
            let plan = planner.plan(video, FormatSelector::BestAudio, &output_template)?;
            let source_audio = plan
                .downloads
                .first()
                .map(|download| download.output_path.clone())
                .ok_or_else(|| {
                    VesselError::Extractor(format!(
                        "ASR audio planner produced no download for {}",
                        video.video_id
                    ))
                })?;

            if !source_audio.is_file() {
                execute_download(&plan).await?;
            }
            source_audio
        };

        info!(
            target: "asr",
            input = %source_audio.display(),
            output = %whisper_wav.display(),
            "ASR audio normalization started"
        );
        transcode_asr_audio(&source_audio, &whisper_wav).await?;
        info!(
            target: "asr",
            output = %whisper_wav.display(),
            "ASR audio normalization completed"
        );
    }

    let config = config.clone();
    let heartbeat_backend = config.backend.clone();
    let heartbeat_model = config.model.clone();
    let heartbeat_input = whisper_wav.clone();
    let wav_for_worker = whisper_wav.clone();

    let (heartbeat_stop_tx, heartbeat_stop_rx) = std::sync::mpsc::channel::<()>();
    let heartbeat = std::thread::spawn(move || {
        let started = Instant::now();
        loop {
            match heartbeat_stop_rx.recv_timeout(Duration::from_secs(15)) {
                Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    eprintln!(
                        "[asr] transcription still running backend={} model={} input={} elapsed={:.0}s",
                        heartbeat_backend,
                        heartbeat_model,
                        heartbeat_input.display(),
                        started.elapsed().as_secs_f64(),
                    );
                }
            }
        }
    });

    let worker_result = tokio::task::spawn_blocking(move || {
        let mut backend = match backend {
            Some(backend) => backend,
            None => LoadedAsrBackend::load(config)?,
        };
        let candidate = backend.transcribe_path(&wav_for_worker)?;
        Ok::<_, VesselError>((candidate, backend))
    })
    .await
    .map_err(|error| VesselError::Extractor(format!("local ASR worker failed to join: {error}")));

    let _ = heartbeat_stop_tx.send(());
    let _ = heartbeat.join();

    let (candidate, backend) = worker_result??;
    Ok((candidate, cache_dir, backend, fixture_reused))
}

async fn transcode_asr_audio(source: &Path, destination: &Path) -> Result<()> {
    let status = tokio::process::Command::new("ffmpeg")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-y")
        .arg("-i")
        .arg(source)
        .arg("-vn")
        .arg("-ac")
        .arg("1")
        .arg("-ar")
        .arg("16000")
        .arg("-c:a")
        .arg("pcm_s16le")
        .arg(destination)
        .status()
        .await
        .map_err(|error| {
            VesselError::Extractor(format!("failed to start ffmpeg for ASR input: {error}"))
        })?;

    if !status.success() {
        return Err(VesselError::Extractor(format!(
            "ffmpeg failed to normalize ASR input {} -> {}",
            source.display(),
            destination.display()
        )));
    }
    Ok(())
}

fn parse_sourcearium_channel_input(raw: &str) -> Result<InputRef> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(VesselError::Corpus(
            "Sourcearium channel input must not be empty".into(),
        ));
    }

    if raw.starts_with("https://") || raw.starts_with("http://") {
        return Ok(InputRef {
            raw: raw.to_owned(),
            kind: InputKind::Url,
        });
    }
    if raw.starts_with('@') {
        return Ok(InputRef {
            raw: format!("https://www.youtube.com/{raw}"),
            kind: InputKind::Url,
        });
    }
    if raw.starts_with("UC") {
        return Ok(InputRef {
            raw: raw.to_owned(),
            kind: InputKind::ChannelId,
        });
    }

    Err(VesselError::Corpus(format!(
        "unsupported Sourcearium YouTube channel input {raw:?}; use a channel URL, @handle, or channel id"
    )))
}

fn normalize_update_publication_date(value: Option<&str>) -> Option<String> {
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
    None
}

fn push_update_item(
    summary: &mut serde_json::Value,
    enabled: bool,
    video_id: &str,
    action: &str,
    details: serde_json::Value,
) {
    if !enabled {
        return;
    }
    let Some(items) = summary
        .get_mut("items")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };
    items.push(serde_json::json!({
        "video_id": video_id,
        "action": action,
        "details": details,
    }));
}

fn push_update_error(summary: &mut serde_json::Value, video_id: &str, error: VesselError) {
    let message = error.to_string();
    summary["errors"]
        .as_array_mut()
        .expect("errors array")
        .push(serde_json::json!({
            "video_id": video_id,
            "message": message.clone(),
        }));

    if let Some(items) = summary
        .get_mut("items")
        .and_then(serde_json::Value::as_array_mut)
    {
        items.push(serde_json::json!({
            "video_id": video_id,
            "action": "failed",
            "details": {
                "message": message,
            },
        }));
    }
}

fn sourcearium_prune(args: PruneArgs) -> Result<()> {
    let sourcearium_root = if args.sourcearium.is_absolute() {
        args.sourcearium
    } else {
        std::env::current_dir()?.join(args.sourcearium)
    };

    let plan = plan_sourcearium_prune(&sourcearium_root)?;
    if args.apply {
        let removed = apply_sourcearium_prune(&sourcearium_root, &plan)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "mode": "apply",
                "removed": removed,
                "plan": plan,
            }))
            .map_err(|error| VesselError::Config(error.to_string()))?
        );
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "mode": "plan",
                "removed": 0,
                "plan": plan,
            }))
            .map_err(|error| VesselError::Config(error.to_string()))?
        );
    }
    Ok(())
}

fn sourcearium_inventory(args: InventoryArgs) -> Result<()> {
    let sourcearium_root = if args.sourcearium.is_absolute() {
        args.sourcearium
    } else {
        std::env::current_dir()?.join(args.sourcearium)
    };
    let report = inventory_sourcearium_repository(&sourcearium_root)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|error| VesselError::Config(error.to_string()))?
    );
    Ok(())
}

fn sourcearium_validate(args: ValidateArgs) -> Result<()> {
    let sourcearium_root = if args.sourcearium.is_absolute() {
        args.sourcearium
    } else {
        std::env::current_dir()?.join(args.sourcearium)
    };

    let report = validate_sourcearium_repository(&sourcearium_root)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|error| VesselError::Config(error.to_string()))?
    );
    if report.valid {
        Ok(())
    } else {
        Err(VesselError::Corpus(format!(
            "Sourcearium validation failed with {} error(s)",
            report.errors.len()
        )))
    }
}

fn init_logging(config: &Config, cli: &Cli) -> Result<()> {
    vessel_logging::init(
        &config.logging.level,
        config.logging.format,
        vessel_logging::LoggingOptions {
            quiet: cli.quiet,
            verbose: cli.verbose,
            progress: config.logging.progress && !cli.no_progress,
        },
    )
}

async fn doctor(
    config: &Config,
    paths: &vessel_core::ConfigPaths,
    loaded_from: &[std::path::PathBuf],
    layout: &RuntimeLayout,
) -> Result<()> {
    info!(target: "info", "doctor started");
    let db_exists = layout.database_path.exists();
    let plugin_dirs = plugin_directories(paths, layout);
    let plugins = load_plugins(&plugin_dirs);
    let report = serde_json::json!({
        "config_paths": {
            "system": paths.system,
            "user": paths.user,
            "project": paths.project,
            "loaded_from": loaded_from,
        },
        "project": {
            "name": layout.project_name,
            "cache_root": layout.cache_root,
            "root": layout.project_root,
        },
        "database": {
            "configured_url": config.database.url,
            "resolved_url": layout.database_url,
            "resolved_path": layout.database_path,
            "exists": db_exists,
        },
        "storage": {
            "download_output": layout.download_output,
            "downloads_root": layout.downloads_root,
            "thumbnails_root": layout.thumbnails_root,
            "subtitles_root": layout.subtitles_root,
            "plugins_root": layout.plugins_root,
        },
        "binaries": {
            "ffmpeg": binary_available("ffmpeg"),
            "ffprobe": binary_available("ffprobe"),
            "yt_dlp": binary_available("yt-dlp"),
            "whisperx": binary_available("whisperx"),
            "uv": binary_available("uv"),
        },
        "plugins": {
            "directories": plugin_dirs,
            "loaded": plugins.plugins(),
            "errors": plugins.errors(),
        },
        "providers": {
            "youtube_po_token": provider_status(
                &plugins,
                config.youtube.po_token_provider.as_deref(),
                "youtube.po_token",
            ),
            "youtube_cookies": provider_status(
                &plugins,
                config.youtube.cookies_from_browser.as_deref(),
                "youtube.cookies",
            ),
        }
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|err| VesselError::Config(err.to_string()))?
    );
    debug!(target: "doctor", database_exists = db_exists, "doctor completed");
    Ok(())
}

fn show_config(
    config: &Config,
    paths: &vessel_core::ConfigPaths,
    loaded_from: &[std::path::PathBuf],
    layout: &RuntimeLayout,
) -> Result<()> {
    info!(target: "info", "config show started");
    let registry_path = project_channel_registry_path(layout);
    let registry_config = load_project_channel_registry(layout)?;
    let registry_summary = summarize_channel_registry(&registry_config)?;
    let report = serde_json::json!({
        "config": config,
        "resolved": {
            "project_name": layout.project_name,
            "cache_root": layout.cache_root,
            "project_root": layout.project_root,
            "database_url": layout.database_url,
            "database_path": layout.database_path,
            "download_output": layout.download_output,
            "downloads_root": layout.downloads_root,
            "thumbnails_root": layout.thumbnails_root,
            "subtitles_root": layout.subtitles_root,
            "plugins_root": layout.plugins_root,
            "litecli_example": format!("litecli {}", layout.database_path.display()),
            "channel_registry_path": registry_path,
            "channel_registry": registry_summary,
        },
        "paths": {
            "system": paths.system,
            "user": paths.user,
            "project": paths.project,
            "loaded_from": loaded_from,
            "precedence": ["defaults", "system", "user", "project", "environment", "cli"]
        }
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|err| VesselError::Config(err.to_string()))?
    );
    debug!(target: "config", "config show completed");
    Ok(())
}

#[derive(Debug, Clone)]
struct ConfiguredChannelTarget {
    category: String,
    input: String,
}

fn project_channel_registry_path(layout: &RuntimeLayout) -> PathBuf {
    layout.project_root.join("vessel.toml")
}

fn load_project_channel_registry(layout: &RuntimeLayout) -> Result<Config> {
    let path = project_channel_registry_path(layout);
    if !path.exists() {
        return Ok(Config::default());
    }
    let raw = std::fs::read_to_string(&path)?;
    toml::from_str::<Config>(&raw)
        .map_err(|err| VesselError::Config(format!("{}: {err}", path.display())))
}

fn save_project_channel_registry(layout: &RuntimeLayout, config: &Config) -> Result<()> {
    let path = project_channel_registry_path(layout);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        &path,
        toml::to_string_pretty(config).map_err(|err| VesselError::Config(err.to_string()))?,
    )?;
    Ok(())
}

fn summarize_channel_registry(config: &Config) -> Result<serde_json::Value> {
    let resolved = resolve_configured_channels(config, &[])?;
    let mut categories = serde_json::Map::new();
    for (name, category) in &config.channels.categories {
        categories.insert(
            name.clone(),
            serde_json::json!({
                "count": category.channels.len(),
                "channels": category.channels,
            }),
        );
    }
    Ok(serde_json::json!({
        "categories": categories,
        "category_count": config.channels.categories.len(),
        "total_channels": resolved.len(),
    }))
}

fn resolve_configured_channels(
    config: &Config,
    selected_categories: &[String],
) -> Result<Vec<ConfiguredChannelTarget>> {
    let mut normalized_seen = HashMap::<String, String>::new();
    for (category, spec) in &config.channels.categories {
        if category.trim().is_empty() {
            return Err(VesselError::Config(
                "channel category names must be non-empty".to_owned(),
            ));
        }
        for channel in &spec.channels {
            let normalized = normalize_channel_entry(channel)?;
            if let Some(previous) = normalized_seen.insert(normalized, category.clone()) {
                if previous != *category {
                    return Err(VesselError::Config(format!(
                        "channel is configured in more than one category: {channel} ({previous}, {category})"
                    )));
                }
            }
        }
    }

    let requested = if selected_categories.is_empty() {
        config
            .channels
            .categories
            .keys()
            .cloned()
            .collect::<Vec<_>>()
    } else {
        selected_categories.to_vec()
    };

    let available = config
        .channels
        .categories
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    for category in &requested {
        if !config.channels.categories.contains_key(category) {
            return Err(VesselError::Config(format!(
                "unknown category '{category}'. available categories: {}",
                available.join(", ")
            )));
        }
    }

    let mut resolved = Vec::new();
    for category in requested {
        if let Some(spec) = config.channels.categories.get(&category) {
            for channel in &spec.channels {
                resolved.push(ConfiguredChannelTarget {
                    category: category.clone(),
                    input: channel.trim().to_owned(),
                });
            }
        }
    }
    Ok(resolved)
}

fn normalize_channel_entry(input: &str) -> Result<String> {
    let normalized = input.trim().to_owned();
    if normalized.is_empty() {
        return Err(VesselError::Config(
            "channel entries in vessel.toml must be non-empty".to_owned(),
        ));
    }
    Ok(normalized)
}

async fn dataset_init(args: DatasetInitArgs, layout: &RuntimeLayout) -> Result<()> {
    info!(target: "info", project = %layout.project_name, "dataset init started");
    ensure_project_layout(layout).await?;
    let target = args.db.unwrap_or_else(|| layout.database_url.clone());
    if args.recreate {
        vessel_logging::progress(
            "dataset",
            format!(
                "recreating dataset db at {}",
                sqlite_target_path(&target)?.display()
            ),
        );
        remove_sqlite_files(&target)?;
    }
    let (_store, paths) = init_sqlite_database(&target).await?;
    vessel_logging::progress(
        "dataset",
        format!(
            "dataset initialized at {}",
            sqlite_target_path(&target)?.display()
        ),
    );
    let report = serde_json::json!({
        "status": "initialized",
        "project": {
            "name": layout.project_name,
            "root": layout.project_root,
        },
        "database": {
            "requested": paths.requested,
            "sqlite_url": paths.sqlite_url,
            "sqlite_path": sqlite_target_path(&target)?,
            "recreated": args.recreate,
        },
        "storage": {
            "download_output": layout.download_output,
            "downloads_root": layout.downloads_root,
            "thumbnails_root": layout.thumbnails_root,
            "subtitles_root": layout.subtitles_root,
            "plugins_root": layout.plugins_root,
        }
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|err| VesselError::Config(err.to_string()))?
    );
    Ok(())
}

async fn dataset_destroy(args: DatasetDestroyArgs, layout: &RuntimeLayout) -> Result<()> {
    if !args.yes {
        return Err(VesselError::Unsupported(
            "dataset destroy is destructive; rerun with --yes".to_owned(),
        ));
    }

    warn!(target: "warn", project = %layout.project_name, "dataset destroy requested");
    ensure_project_layout(layout).await?;
    let target = args.db.unwrap_or_else(|| layout.database_url.clone());
    let database_path = sqlite_target_path(&target)?;
    let removed_files = remove_sqlite_files(&target)?;
    vessel_logging::progress(
        "dataset",
        format!("removed {} database files", removed_files.len()),
    );
    let report = serde_json::json!({
        "status": "destroyed",
        "project": {
            "name": layout.project_name,
            "root": layout.project_root,
        },
        "database": {
            "requested": target,
            "sqlite_path": database_path,
            "removed_files": removed_files,
        }
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|err| VesselError::Config(err.to_string()))?
    );
    Ok(())
}

async fn channel_config_init(layout: &RuntimeLayout) -> Result<()> {
    info!(target: "info", project = %layout.project_name, "channel config init started");
    ensure_project_layout(layout).await?;
    let path = project_channel_registry_path(layout);
    let existed = path.exists();
    if !existed {
        let mut config = Config::default();
        config.dataset.project = Some(layout.project_name.clone());
        config
            .channels
            .categories
            .insert("default".to_owned(), ChannelCategoryConfig::default());
        save_project_channel_registry(layout, &config)?;
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "status": if existed { "exists" } else { "initialized" },
            "path": path,
            "project": layout.project_name,
        }))
        .map_err(|err| VesselError::Config(err.to_string()))?
    );
    Ok(())
}

fn channel_config_show(layout: &RuntimeLayout) -> Result<()> {
    info!(target: "info", project = %layout.project_name, "channel config show started");
    let path = project_channel_registry_path(layout);
    let config = load_project_channel_registry(layout)?;
    let summary = summarize_channel_registry(&config)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "status": "ok",
            "path": path,
            "project": layout.project_name,
            "registry": summary,
        }))
        .map_err(|err| VesselError::Config(err.to_string()))?
    );
    Ok(())
}

async fn extract_preview(
    url: String,
    kind: InputKind,
    paths: &vessel_core::ConfigPaths,
    layout: &RuntimeLayout,
) -> Result<()> {
    info!(target: "info", url = %url, "info extraction started");
    let registry = build_registry(&load_runtime_plugins(paths, layout));
    let input = InputRef { raw: url, kind };
    let extractor = registry
        .best_for(&input)
        .ok_or_else(|| VesselError::Unsupported("no extractor matched input".to_owned()))?;
    debug!(target: "extractor", extractor = extractor.name(), input = %input.raw, "extractor selected");
    let item = extractor
        .extract(ExtractRequest { input }, ExtractContext)
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&item).map_err(|err| VesselError::Config(err.to_string()))?
    );
    Ok(())
}

async fn channel_add(args: ChannelAddArgs, layout: &RuntimeLayout) -> Result<()> {
    info!(target: "info", channel = %args.channel, "channel add started");
    ensure_project_layout(layout).await?;
    let category = args
        .category
        .ok_or_else(|| VesselError::Config("channel add requires --category <name>".to_owned()))?;
    let category = category.trim().to_owned();
    if category.is_empty() {
        return Err(VesselError::Config(
            "channel add requires a non-empty category".to_owned(),
        ));
    }
    let normalized_channel = normalize_channel_entry(&args.channel)?;
    let mut registry = load_project_channel_registry(layout)?;
    for (existing_category, spec) in &registry.channels.categories {
        if spec
            .channels
            .iter()
            .any(|channel| channel.trim() == normalized_channel)
        {
            if existing_category != &category {
                return Err(VesselError::Config(format!(
                    "channel is already assigned to category '{existing_category}'"
                )));
            }
        }
    }
    let (store, _) = init_sqlite_database(&layout.database_url).await?;
    let input = parse_channel_input(&normalized_channel);
    debug!(target: "extractor", input = %input.raw, "resolving channel metadata");
    let channel = extract_channel(&input).await?;
    vessel_logging::progress(
        "channel",
        format!(
            "resolved channel {} ({})",
            channel
                .title
                .clone()
                .unwrap_or_else(|| channel.channel_id.clone()),
            channel.channel_id
        ),
    );
    store.upsert_channel_snapshot(&channel).await?;
    store.add_tracked_channel(&channel, &category).await?;
    let category_entry = registry
        .channels
        .categories
        .entry(category.clone())
        .or_insert_with(ChannelCategoryConfig::default);
    if !category_entry
        .channels
        .iter()
        .any(|configured| configured.trim() == normalized_channel)
    {
        category_entry.channels.push(normalized_channel);
    }
    registry.dataset.project = Some(layout.project_name.clone());
    save_project_channel_registry(layout, &registry)?;

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "status": "tracked",
            "category": category,
            "channel_id": channel.channel_id,
            "handle": channel.handle,
            "title": channel.title,
            "url": channel.url,
            "registry_path": project_channel_registry_path(layout),
        }))
        .map_err(|err| VesselError::Config(err.to_string()))?
    );
    Ok(())
}

async fn channel_sync(args: ChannelSyncArgs, layout: &RuntimeLayout) -> Result<()> {
    info!(target: "info", project = %layout.project_name, full = args.full, metrics_only = args.metrics_only, "channel sync started");
    ensure_project_layout(layout).await?;
    let registry = load_project_channel_registry(layout)?;
    let configured_targets = resolve_configured_channels(&registry, &args.categories)?;
    let (store, _) = init_sqlite_database(&layout.database_url).await?;
    let selected_categories = if args.categories.is_empty() {
        registry
            .channels
            .categories
            .keys()
            .cloned()
            .collect::<Vec<_>>()
    } else {
        args.categories.clone()
    };
    vessel_logging::progress(
        "info",
        format!(
            "syncing {} configured channel{} across {} categor{}",
            configured_targets.len(),
            if configured_targets.len() == 1 {
                ""
            } else {
                "s"
            },
            selected_categories.len(),
            if selected_categories.len() == 1 {
                "y"
            } else {
                "ies"
            }
        ),
    );
    if configured_targets.is_empty() {
        warn!(target: "warn", "channel sync has no configured channels");
    }
    let run_id = store.start_run("channel sync").await?;
    let started_at = OffsetDateTime::now_utc();

    let sync_comments = !args.metrics_only && (args.full || args.comments);
    let sync_subtitles = !args.metrics_only && (args.full || args.subtitles);
    let sync_thumbnails = !args.metrics_only && (args.full || args.download_thumbnails);

    let result = async {
        let mut summary = serde_json::json!({
            "status": "synced",
            "run_id": run_id,
            "tracked_channels": configured_targets.len(),
            "matched_channels": configured_targets.len(),
            "selected_categories": selected_categories,
            "channels_processed": 0usize,
            "channel_history_inserted": 0usize,
            "channel_metrics_inserted": 0usize,
            "videos_discovered": 0usize,
            "unique_videos_discovered": 0usize,
            "video_history_inserted": 0usize,
            "video_metrics_inserted": 0usize,
            "videos_refreshed": 0usize,
            "video_refreshes_skipped_by_since": 0usize,
            "tabs_visited": Vec::<String>::new(),
            "tabs_completed": Vec::<String>::new(),
            "tabs_resumed_from_checkpoint": Vec::<String>::new(),
            "videos_discovered_per_tab": serde_json::Map::<String, serde_json::Value>::new(),
            "errors": 0usize,
            "options": {
                "full": args.full,
                "metrics_only": args.metrics_only,
                "comments": sync_comments,
                "subtitles": sync_subtitles,
                "download_thumbnails": sync_thumbnails,
                "since": args.since,
                "max_videos": args.max_videos,
            }
        });

        for target in configured_targets {
            match sync_one_channel(&store, &target, &args, run_id, started_at, layout).await {
                Ok(channel_report) => {
                    increment_summary(&mut summary, "channels_processed", 1);
                    increment_summary(
                        &mut summary,
                        "channel_history_inserted",
                        usize::from(channel_report.channel_history_inserted),
                    );
                    increment_summary(
                        &mut summary,
                        "channel_metrics_inserted",
                        channel_report.channel_metrics_inserted,
                    );
                    increment_summary(
                        &mut summary,
                        "videos_discovered",
                        channel_report.videos_discovered,
                    );
                    increment_summary(
                        &mut summary,
                        "unique_videos_discovered",
                        channel_report.unique_videos_discovered,
                    );
                    increment_summary(
                        &mut summary,
                        "video_history_inserted",
                        channel_report.video_history_inserted,
                    );
                    increment_summary(
                        &mut summary,
                        "video_metrics_inserted",
                        channel_report.video_metrics_inserted,
                    );
                    increment_summary(
                        &mut summary,
                        "videos_refreshed",
                        channel_report.videos_refreshed,
                    );
                    increment_summary(
                        &mut summary,
                        "video_refreshes_skipped_by_since",
                        channel_report.skipped_by_since,
                    );
                    extend_summary_array(
                        &mut summary,
                        "tabs_visited",
                        &channel_report.tabs_visited,
                    );
                    extend_summary_array(
                        &mut summary,
                        "tabs_completed",
                        &channel_report.tabs_completed,
                    );
                    extend_summary_array(
                        &mut summary,
                        "tabs_resumed_from_checkpoint",
                        &channel_report.tabs_resumed_from_checkpoint,
                    );
                    merge_summary_tab_counts(
                        &mut summary,
                        "videos_discovered_per_tab",
                        &channel_report.videos_discovered_per_tab,
                    );
                }
                Err(err) => {
                    increment_summary(&mut summary, "errors", 1);
                    store
                        .record_attempt(FetchAttempt {
                            run_id,
                            target_kind: "channel".to_owned(),
                            target_external_id: target.input.clone(),
                            status: AttemptStatus::Failed,
                            started_at,
                            finished_at: OffsetDateTime::now_utc(),
                            error_message: Some(err.to_string()),
                        })
                        .await?;
                }
            }
        }

        store.finish_run(run_id, true).await?;
        Ok::<_, VesselError>(summary)
    }
    .await;

    match result {
        Ok(summary) => {
            info!(target: "info", "channel sync completed");
            println!(
                "{}",
                serde_json::to_string_pretty(&summary)
                    .map_err(|err| VesselError::Config(err.to_string()))?
            );
            Ok(())
        }
        Err(err) => {
            store.finish_run(run_id, false).await?;
            Err(err)
        }
    }
}

async fn video_refresh(
    args: VideoRefreshArgs,
    layout: &RuntimeLayout,
    paths: &vessel_core::ConfigPaths,
) -> Result<()> {
    info!(target: "info", video = %args.video, metrics_only = args.metrics_only, "video refresh started");
    ensure_project_layout(layout).await?;
    let (store, _) = init_sqlite_database(&layout.database_url).await?;
    let registry = build_registry(&load_runtime_plugins(paths, layout));
    let input = parse_video_input(&args.video);
    let target_id = args.video.clone();
    let run_id = store.start_run("video refresh").await?;
    let started_at = OffsetDateTime::now_utc();

    let result = async {
        let extractor = registry
            .best_for(&input)
            .ok_or_else(|| VesselError::Unsupported("no extractor matched input".to_owned()))?;
        debug!(target: "extractor", extractor = extractor.name(), input = %target_id, "extractor selected");
        let item = extractor
            .extract(ExtractRequest { input }, ExtractContext)
            .await?;
        let video = match item {
            ExtractedItem::Video(video) => video,
            _ => {
                return Err(VesselError::Unsupported(
                    "video refresh requires a video target".to_owned(),
                ));
            }
        };
        vessel_logging::progress(
            "video",
            format!(
                "refreshing {} {}",
                video.video_id,
                video.title.clone().unwrap_or_default()
            ),
        );
        let history_inserted = if args.metrics_only {
            store.record_video_metrics_only(&video).await?;
            vessel_logging::progress(
                "db",
                format!("video_history skipped for {} (metrics-only)", video.video_id),
            );
            false
        } else {
            let inserted = store.upsert_video_snapshot(&video).await?;
            if inserted {
                vessel_logging::progress("db", format!("video_history inserted for {}", video.video_id));
            } else {
                vessel_logging::progress("db", format!("video_history unchanged for {}", video.video_id));
            }
            inserted
        };
        vessel_logging::progress("db", format!("video_metrics inserted for {}", video.video_id));
        let finished_at = OffsetDateTime::now_utc();
        store
            .record_attempt(FetchAttempt {
                run_id,
                target_kind: "video".to_owned(),
                target_external_id: video.video_id.clone(),
                status: AttemptStatus::Success,
                started_at,
                finished_at,
                error_message: None,
            })
            .await?;
        store.finish_run(run_id, true).await?;

        Ok::<_, VesselError>(serde_json::json!({
            "status": "refreshed",
            "run_id": run_id,
            "video_id": video.video_id,
            "title": video.title,
            "metrics_only": args.metrics_only,
            "history_inserted": history_inserted,
            "fetched_at": video.fetched_at.format(&time::format_description::well_known::Rfc3339)
                .map_err(|err| VesselError::Config(err.to_string()))?,
        }))
    }
    .await;

    match result {
        Ok(report) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&report)
                    .map_err(|err| VesselError::Config(err.to_string()))?
            );
            Ok(())
        }
        Err(err) => {
            let finished_at = OffsetDateTime::now_utc();
            store
                .record_attempt(FetchAttempt {
                    run_id,
                    target_kind: "video".to_owned(),
                    target_external_id: target_id,
                    status: AttemptStatus::Failed,
                    started_at,
                    finished_at,
                    error_message: Some(err.to_string()),
                })
                .await?;
            store.finish_run(run_id, false).await?;
            Err(err)
        }
    }
}

async fn video_history(args: VideoRefArg, layout: &RuntimeLayout) -> Result<()> {
    debug!(target: "video", video = %args.video, "video history lookup started");
    ensure_project_layout(layout).await?;
    let (store, _) = init_sqlite_database(&layout.database_url).await?;
    let lookup = if args.video.contains("://") {
        extract_video_id_from_input(&args.video, layout).await?
    } else {
        args.video
    };
    let history = store.load_video_history(&lookup).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&history)
            .map_err(|err| VesselError::Config(err.to_string()))?
    );
    Ok(())
}

async fn video_subtitles_sync(args: VideoRefArg, layout: &RuntimeLayout) -> Result<()> {
    info!(target: "info", video = %args.video, "video subtitles sync started");
    ensure_project_layout(layout).await?;
    let (store, _) = init_sqlite_database(&layout.database_url).await?;
    let video = extract_video(&parse_video_input(&args.video)).await?;
    store.upsert_video_snapshot(&video).await?;
    let subtitle_history_inserted = store.sync_subtitle_tracks(&video, &video.subtitles).await?;
    let artifact_paths = sync_subtitle_artifacts(&store, &video, layout).await?;
    let history = store.load_subtitle_history(&video.video_id).await?;

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "status": "synced",
            "video_id": video.video_id,
            "title": video.title,
            "tracks": video.subtitles.len(),
            "subtitle_history_inserted": subtitle_history_inserted,
            "artifacts_written": artifact_paths.len(),
            "artifact_paths": artifact_paths,
            "history_counts": {
                "tracks": history.tracks.len(),
                "history": history.revisions.len(),
            },
        }))
        .map_err(|err| VesselError::Config(err.to_string()))?
    );
    Ok(())
}

async fn video_comments_sync(args: VideoRefArg, layout: &RuntimeLayout) -> Result<()> {
    info!(target: "info", video = %args.video, "video comments sync started");
    ensure_project_layout(layout).await?;
    let (store, _) = init_sqlite_database(&layout.database_url).await?;
    let input = parse_video_input(&args.video);
    let video = extract_video(&input).await?;
    store.upsert_video_snapshot(&video).await?;
    let comments = extract_comments(&input, 100).await?;
    let comment_history_inserted = store.sync_comments(&comments).await?;
    let history = store.load_comment_history(&video.video_id).await?;

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "status": "synced",
            "video_id": video.video_id,
            "title": video.title,
            "comments_fetched": comments.len(),
            "comment_history_inserted": comment_history_inserted,
            "history_counts": {
                "comments": history.comments.len(),
                "history": history.revisions.len(),
            },
            "native_only": true,
        }))
        .map_err(|err| VesselError::Config(err.to_string()))?
    );
    Ok(())
}

async fn formats(url: String) -> Result<()> {
    info!(target: "info", url = %url, "formats requested");
    let video = extract_video(&parse_video_input(&url)).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "video_id": video.video_id,
            "title": video.title,
            "formats": video.formats,
            "default_selector": "best",
        }))
        .map_err(|err| VesselError::Config(err.to_string()))?
    );
    Ok(())
}

async fn download(args: DownloadArgs, layout: &RuntimeLayout) -> Result<()> {
    info!(target: "info", url = %args.url, "download started");
    ensure_project_layout(layout).await?;
    let (store, _) = init_sqlite_database(&layout.database_url).await?;
    let video = extract_video(&parse_video_input(&args.url)).await?;
    let archived = store.is_video_archived("youtube", &video.video_id).await?;
    if archived {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "status": "skipped",
                "reason": "already archived",
                "video_id": video.video_id,
            }))
            .map_err(|err| VesselError::Config(err.to_string()))?
        );
        return Ok(());
    }

    let selector = parse_format_selector(args.format.as_deref())?;
    debug!(target: "download", selector = ?selector, "format selector chosen");
    let planner = BasicDownloadPlanner;
    let plan = match planner.plan(&video, selector.clone(), &layout.download_output) {
        Ok(plan) => plan,
        Err(err) => {
            return print_unsupported_report_with_details(
                "download.plan",
                &format!("native download planning failed: {err}"),
                serde_json::json!({
                    "video_id": video.video_id,
                    "requested_format": args.format,
                    "requested_postprocess": {
                        "remux_video": args.remux_video,
                        "extract_audio": args.extract_audio,
                        "audio_format": args.audio_format,
                        "embed_metadata": args.embed_metadata,
                        "embed_thumbnail": args.embed_thumbnail,
                        "subtitles": args.subtitles,
                        "convert_subs": args.convert_subs,
                    },
                    "native_only": true,
                }),
            );
        }
    };
    vessel_logging::progress(
        "download",
        format!(
            "selected formats {}",
            plan.downloads
                .iter()
                .map(|item| item.format_id.clone())
                .collect::<Vec<_>>()
                .join("+")
        ),
    );
    let result = match execute_download(&plan).await {
        Ok(result) => result,
        Err(err) => {
            return print_unsupported_report_with_details(
                "download.execute",
                &format!("native download failed without external fallback: {err}"),
                serde_json::json!({
                    "video_id": video.video_id,
                    "format_ids": plan.downloads.iter().map(|item| item.format_id.clone()).collect::<Vec<_>>(),
                    "output_path": plan.output_path,
                    "native_only": true,
                }),
            );
        }
    };

    let subtitle_paths = if args.subtitles || args.convert_subs.is_some() {
        sync_subtitle_artifacts(&store, &video, layout).await?
    } else {
        Vec::new()
    };

    let thumbnail_path = if args.embed_thumbnail {
        sync_primary_thumbnail_artifact(&store, &video, layout).await?
    } else {
        None
    };

    let extract_audio_format = args.extract_audio.then(|| args.audio_format.clone());
    let postprocess_request = PostprocessRequest {
        video_title: video.title.clone(),
        channel_id: video.channel_id.clone(),
        description: video.description.clone(),
        final_output_path: postprocess_output_path(&plan.output_path, &args),
        media_inputs: result
            .files
            .iter()
            .map(|file| file.output_path.clone())
            .collect(),
        thumbnail_path: thumbnail_path.clone(),
        subtitle_paths: subtitle_paths.iter().map(PathBuf::from).collect(),
        remux_video: args.remux_video.clone(),
        extract_audio: extract_audio_format,
        embed_metadata: args.embed_metadata,
        embed_thumbnail: args.embed_thumbnail,
        convert_subtitles: args.convert_subs.clone(),
    };
    let postprocess_plan = build_plan(&postprocess_request);
    debug!(target: "download", plan = ?postprocess_plan, "postprocess plan built");
    vessel_logging::progress("download", "running postprocess plan");
    let postprocess_result = execute_plan(&postprocess_request, &postprocess_plan).await?;

    let final_output_path = postprocess_result
        .final_media_path
        .clone()
        .unwrap_or_else(|| plan.output_path.clone());
    let file_hash = hash_file(&final_output_path).await?;
    let byte_size = tokio::fs::metadata(&final_output_path).await?.len();
    let artifact = store
        .insert_artifact(
            &video.video_id,
            "video",
            &project_relative_path(layout, &final_output_path),
            &file_hash,
            byte_size,
            Some(
                &result
                    .files
                    .iter()
                    .map(|file| file.format_id.clone())
                    .collect::<Vec<_>>()
                    .join("+"),
            ),
        )
        .await?;
    vessel_logging::progress(
        "db",
        format!(
            "artifact recorded for {} at {}",
            video.video_id, artifact.path
        ),
    );
    let mut generated_artifacts = Vec::new();
    for generated in postprocess_result.generated_artifacts {
        let generated_hash = hash_file(&generated.path).await?;
        let generated_size = tokio::fs::metadata(&generated.path).await?.len();
        let stored = store
            .insert_artifact(
                &video.video_id,
                &generated.kind,
                &project_relative_path(layout, &generated.path),
                &generated_hash,
                generated_size,
                None,
            )
            .await?;
        generated_artifacts.push(serde_json::json!({
            "artifact_id": stored.artifact_id,
            "kind": stored.artifact_kind,
            "path": stored.path,
            "content_hash": stored.content_hash,
        }));
    }
    store
        .insert_archive_entry("youtube", &video.video_id, &artifact.artifact_id)
        .await?;
    vessel_logging::progress(
        "download",
        format!("download complete for {}", video.video_id),
    );

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "status": "downloaded",
            "video_id": video.video_id,
            "title": video.title,
            "format_ids": result.files.iter().map(|file| file.format_id.clone()).collect::<Vec<_>>(),
            "output_path": project_relative_path(layout, &final_output_path),
            "bytes_written": byte_size,
            "resumed": result.files.iter().any(|file| file.resumed),
            "artifact_id": artifact.artifact_id,
            "content_hash": artifact.content_hash,
            "downloaded_files": result.files.iter().map(|file| serde_json::json!({
                "format_id": file.format_id,
                "output_path": project_relative_path(layout, &file.output_path),
                "temp_path": project_relative_path(layout, &file.temp_path),
                "bytes_written": file.bytes_written,
                "resumed": file.resumed,
                "role": file.role,
            })).collect::<Vec<_>>(),
            "postprocess_plan": postprocess_plan,
            "generated_artifacts": generated_artifacts,
        }))
        .map_err(|err| VesselError::Config(err.to_string()))?
    );
    Ok(())
}

fn build_registry(plugins: &PluginCatalog) -> ExtractorRegistry {
    let mut registry = ExtractorRegistry::default();
    registry.register(YoutubeExtractor);
    for extractor in plugins.extractors() {
        registry.register_arc(extractor);
    }
    registry
}

#[derive(Debug, Default)]
struct ChannelSyncReport {
    channel_history_inserted: bool,
    videos_discovered: usize,
    unique_videos_discovered: usize,
    video_history_inserted: usize,
    video_metrics_inserted: usize,
    channel_metrics_inserted: usize,
    videos_refreshed: usize,
    skipped_by_since: usize,
    tabs_visited: Vec<String>,
    tabs_completed: Vec<String>,
    tabs_resumed_from_checkpoint: Vec<String>,
    videos_discovered_per_tab: std::collections::BTreeMap<String, usize>,
}

async fn sync_one_channel(
    store: &vessel_store::SqliteStore,
    tracked: &ConfiguredChannelTarget,
    args: &ChannelSyncArgs,
    run_id: uuid::Uuid,
    started_at: OffsetDateTime,
    layout: &RuntimeLayout,
) -> Result<ChannelSyncReport> {
    let input = parse_channel_input(&tracked.input);
    vessel_logging::progress(
        "channel",
        format!("[{}] {}: refresh started", tracked.category, tracked.input),
    );
    let mut channel = extract_channel(&input).await?;
    store
        .add_tracked_channel(&channel, &tracked.category)
        .await?;
    debug!(
        target: "channel",
        channel_id = %channel.channel_id,
        title = ?channel.title,
        "channel metadata resolved"
    );

    let existing_cursors = store
        .load_channel_tab_cursors(&channel.channel_id)
        .await?
        .into_iter()
        .map(|cursor| ChannelTabCursor {
            tab_name: cursor.tab_name,
            continuation_token: cursor.continuation_token,
            visitor_data: cursor.visitor_data,
            delegated_session_id: cursor.delegated_session_id,
            last_seen_published_at: cursor.last_seen_published_at,
            backfill_complete: cursor.backfill_complete,
        })
        .collect::<Vec<_>>();
    debug!(
        target: "crawl",
        channel_id = %channel.channel_id,
        existing_cursors = existing_cursors.len(),
        "starting channel crawl"
    );
    let crawl = crawl_channel_videos(&input, &existing_cursors).await?;
    vessel_logging::progress(
        "crawl",
        format!(
            "{}: discovered {} unique videos across {} tabs",
            channel
                .title
                .clone()
                .unwrap_or_else(|| channel.channel_id.clone()),
            crawl.videos.len(),
            crawl.tabs_visited.len()
        ),
    );
    let discovered_count = crawl.videos.len();
    for video_ref in &crawl.videos {
        store
            .record_channel_video_membership(
                &channel.channel_id,
                &video_ref.video_id,
                &video_ref.tab_name,
            )
            .await?;
    }
    let mut videos = crawl.videos;
    if let Some(since) = &args.since {
        videos.retain(|video| {
            video
                .published_at
                .as_deref()
                .map(|published| published >= since.as_str())
                .unwrap_or(true)
        });
    }
    let skipped_by_since = discovered_count.saturating_sub(videos.len());
    if let Some(max_videos) = args.max_videos {
        videos.truncate(max_videos);
    }

    let mut report = ChannelSyncReport {
        videos_discovered: discovered_count,
        unique_videos_discovered: discovered_count,
        tabs_visited: crawl.tabs_visited.clone(),
        tabs_completed: crawl.tabs_completed.clone(),
        tabs_resumed_from_checkpoint: crawl.tabs_resumed_from_checkpoint.clone(),
        videos_discovered_per_tab: crawl.videos_per_tab.clone(),
        ..ChannelSyncReport::default()
    };

    let total = videos.len();
    for (index, video_ref) in videos.into_iter().enumerate() {
        if sync_channel_video(store, video_ref, args, layout, index + 1, total).await? {
            report.video_history_inserted += 1;
        }
        report.video_metrics_inserted += 1;
        report.videos_refreshed += 1;
    }

    if channel.video_count.is_none() {
        channel.video_count = Some(discovered_count as u64);
    }
    if channel.view_count.is_none() {
        channel.view_count = store
            .aggregate_channel_video_view_count(&channel.channel_id)
            .await?;
    }
    report.channel_metrics_inserted += 1;
    report.channel_history_inserted = if args.metrics_only {
        store.record_channel_metrics_only(&channel).await?;
        vessel_logging::progress(
            "db",
            format!(
                "channel_history skipped for {} (metrics-only)",
                channel.channel_id
            ),
        );
        false
    } else {
        let inserted = store.upsert_channel_snapshot(&channel).await?;
        if inserted {
            vessel_logging::progress(
                "db",
                format!("channel_history inserted for {}", channel.channel_id),
            );
        } else {
            vessel_logging::progress(
                "db",
                format!("channel_history unchanged for {}", channel.channel_id),
            );
        }
        inserted
    };
    vessel_logging::progress(
        "db",
        format!("channel_metrics inserted for {}", channel.channel_id),
    );
    store
        .record_attempt(FetchAttempt {
            run_id,
            target_kind: "channel".to_owned(),
            target_external_id: channel.channel_id.clone(),
            status: AttemptStatus::Success,
            started_at,
            finished_at: OffsetDateTime::now_utc(),
            error_message: None,
        })
        .await?;

    report.skipped_by_since = skipped_by_since;
    for cursor in crawl.cursors {
        store
            .save_channel_tab_cursor(
                &channel.channel_id,
                &cursor.tab_name,
                cursor.continuation_token.as_deref(),
                cursor.visitor_data.as_deref(),
                cursor.delegated_session_id.as_deref(),
                cursor.last_seen_published_at.as_deref(),
                cursor.backfill_complete,
            )
            .await?;
    }
    store
        .mark_tracked_channel_synced(&channel.channel_id)
        .await?;
    vessel_logging::progress(
        "channel",
        format!(
            "{}: sync complete ({} refreshed)",
            channel
                .title
                .clone()
                .unwrap_or_else(|| channel.channel_id.clone()),
            report.videos_refreshed
        ),
    );
    Ok(report)
}

async fn sync_channel_video(
    store: &vessel_store::SqliteStore,
    video_ref: ChannelVideoRef,
    args: &ChannelSyncArgs,
    layout: &RuntimeLayout,
    index: usize,
    total: usize,
) -> Result<bool> {
    let sync_comments = !args.metrics_only && (args.full || args.comments);
    let sync_subtitles = !args.metrics_only && (args.full || args.subtitles);
    let sync_thumbnails = !args.metrics_only && (args.full || args.download_thumbnails);
    let input = InputRef {
        raw: video_ref.video_id,
        kind: InputKind::VideoId,
    };
    vessel_logging::progress(
        "video",
        format!(
            "[{index}/{total}] refreshing {} {}",
            input.raw,
            video_ref.title.unwrap_or_default()
        ),
    );
    let video = extract_video(&input).await?;
    let history_inserted = if args.metrics_only {
        store.record_video_metrics_only(&video).await?;
        vessel_logging::progress(
            "db",
            format!(
                "video_history skipped for {} (metrics-only)",
                video.video_id
            ),
        );
        false
    } else {
        let inserted = store.upsert_video_snapshot(&video).await?;
        if inserted {
            vessel_logging::progress(
                "db",
                format!("video_history inserted for {}", video.video_id),
            );
        } else {
            vessel_logging::progress(
                "db",
                format!("video_history unchanged for {}", video.video_id),
            );
        }
        inserted
    };
    vessel_logging::progress(
        "db",
        format!("video_metrics inserted for {}", video.video_id),
    );
    if sync_subtitles {
        let inserted = store.sync_subtitle_tracks(&video, &video.subtitles).await?;
        vessel_logging::progress(
            "db",
            format!(
                "subtitle_history inserted {} track(s) for {}",
                inserted, video.video_id
            ),
        );
        sync_subtitle_artifacts(store, &video, layout).await?;
    }
    if sync_thumbnails {
        debug!(target: "download", video_id = %video.video_id, "syncing thumbnails");
        sync_video_thumbnails(store, &video, layout).await?;
    }
    if sync_comments {
        let comments = extract_comments(&input, 40).await?;
        let inserted = store.sync_comments(&comments).await?;
        vessel_logging::progress(
            "db",
            format!(
                "comment_history inserted {} row(s) for {}",
                inserted, video.video_id
            ),
        );
    }
    Ok(history_inserted)
}

fn parse_video_input(raw: &str) -> InputRef {
    let kind = if raw.contains("://") {
        InputKind::Url
    } else {
        InputKind::VideoId
    };
    InputRef {
        raw: raw.to_owned(),
        kind,
    }
}

fn parse_channel_input(raw: &str) -> InputRef {
    let kind = if raw.contains("://") || raw.contains('@') || raw.contains("/channel/") {
        InputKind::Url
    } else {
        InputKind::ChannelId
    };
    InputRef {
        raw: raw.to_owned(),
        kind,
    }
}

fn parse_format_selector(raw: Option<&str>) -> Result<FormatSelector> {
    match raw {
        None => Ok(FormatSelector::Best),
        Some(value) => parse_selector(value)
            .map_err(|err| VesselError::Unsupported(format!("invalid format selector: {err}"))),
    }
}

fn sqlite_target_path(target: &str) -> Result<PathBuf> {
    let normalized = if target.starts_with("sqlite:") {
        target.to_owned()
    } else {
        format!("sqlite://{target}")
    };
    let path = normalized
        .strip_prefix("sqlite://")
        .or_else(|| normalized.strip_prefix("sqlite:"))
        .ok_or_else(|| VesselError::Config("invalid sqlite target".to_owned()))?;
    Ok(PathBuf::from(path))
}

fn remove_sqlite_files(target: &str) -> Result<Vec<String>> {
    let db_path = sqlite_target_path(target)?;
    let mut removed = Vec::new();
    for path in [
        db_path.clone(),
        PathBuf::from(format!("{}-wal", db_path.display())),
        PathBuf::from(format!("{}-shm", db_path.display())),
    ] {
        if path.exists() {
            std::fs::remove_file(&path)?;
            removed.push(path.display().to_string());
        }
    }
    Ok(removed)
}

fn sanitize_component(input: &str) -> String {
    input
        .chars()
        .map(|ch| match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            _ if ch.is_control() => '_',
            _ => ch,
        })
        .collect()
}

async fn plugin_list(paths: &vessel_core::ConfigPaths, layout: &RuntimeLayout) -> Result<()> {
    let directories = plugin_directories(paths, layout);
    let plugins = load_runtime_plugins(paths, layout);
    let report = serde_json::json!({
        "status": "ok",
        "directories": directories,
        "plugins": plugins.plugins(),
        "errors": plugins.errors(),
        "providers": plugins.providers(),
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|err| VesselError::Config(err.to_string()))?
    );
    Ok(())
}

async fn project_list(layout: &RuntimeLayout) -> Result<()> {
    tokio::fs::create_dir_all(&layout.cache_root).await?;
    let mut entries = tokio::fs::read_dir(&layout.cache_root).await?;
    let mut projects = Vec::new();

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let project_name = entry.file_name().to_string_lossy().into_owned();
        let database_path = path.join("vessel.sqlite");
        let plugins_root = path.join("plugins");
        let tracked = if database_path.exists() {
            match init_sqlite_database(&format!("sqlite://{}", database_path.display())).await {
                Ok((store, _)) => store
                    .list_tracked_channels()
                    .await
                    .unwrap_or_default()
                    .len(),
                Err(_) => 0,
            }
        } else {
            0
        };
        projects.push(serde_json::json!({
            "name": project_name,
            "root": path,
            "database_path": database_path,
            "database_exists": database_path.exists(),
            "plugins_root": plugins_root,
            "tracked_channels": tracked,
        }));
    }

    projects.sort_by(|left, right| {
        left.get("name")
            .and_then(serde_json::Value::as_str)
            .cmp(&right.get("name").and_then(serde_json::Value::as_str))
    });

    let report = serde_json::json!({
        "status": "ok",
        "cache_root": layout.cache_root,
        "selected_project": layout.project_name,
        "projects": projects,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|err| VesselError::Config(err.to_string()))?
    );
    Ok(())
}

async fn plugin_install(
    args: PluginInstallArgs,
    paths: &vessel_core::ConfigPaths,
    layout: &RuntimeLayout,
) -> Result<()> {
    ensure_project_layout(layout).await?;
    let target_root = args
        .dir
        .map(PathBuf::from)
        .unwrap_or_else(|| default_project_plugin_dir(paths, layout));
    let plugin_id = sanitize_plugin_id(&args.name);
    let plugin_dir = target_root.join(&plugin_id);
    let fixtures_dir = plugin_dir.join("fixtures");
    tokio::fs::create_dir_all(&fixtures_dir).await?;

    match args.kind.as_str() {
        "fixture-extractor" => {
            let manifest = format!(
                "id = \"{plugin_id}\"\nversion = \"0.1.0\"\n\n[[extractors]]\nid = \"fixture\"\nname = \"{plugin_id} fixture\"\nkind = \"fixture\"\nfixtures = \"fixtures/videos.json\"\nmatch_contains = [\"fixture://{plugin_id}\", \"example.test/{plugin_id}\"]\nsupport_level = \"generic\"\n\n[[providers]]\nid = \"po_token\"\npurpose = \"youtube.po_token\"\nkind = \"env\"\nenv = \"VESSEL_{}_PO_TOKEN\"\n",
                plugin_id.to_ascii_uppercase().replace('-', "_")
            );
            tokio::fs::write(plugin_dir.join("plugin.toml"), manifest).await?;
            tokio::fs::write(
                fixtures_dir.join("videos.json"),
                serde_json::to_string_pretty(&serde_json::json!({
                    "videos": {},
                    "channels": {},
                }))
                .map_err(|err| VesselError::Config(err.to_string()))?,
            )
            .await?;
        }
        "provider-env" => {
            let manifest = format!(
                "id = \"{plugin_id}\"\nversion = \"0.1.0\"\n\n[[providers]]\nid = \"po_token\"\npurpose = \"youtube.po_token\"\nkind = \"env\"\nenv = \"VESSEL_{}_PO_TOKEN\"\n",
                plugin_id.to_ascii_uppercase().replace('-', "_")
            );
            tokio::fs::write(plugin_dir.join("plugin.toml"), manifest).await?;
        }
        other => {
            return Err(VesselError::Unsupported(format!(
                "unsupported plugin scaffold kind `{other}`; supported kinds are fixture-extractor and provider-env"
            )));
        }
    }

    let report = serde_json::json!({
        "status": "installed",
        "plugin_id": plugin_id,
        "kind": args.kind,
        "path": plugin_dir,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|err| VesselError::Config(err.to_string()))?
    );
    Ok(())
}

fn load_runtime_plugins(paths: &vessel_core::ConfigPaths, layout: &RuntimeLayout) -> PluginCatalog {
    load_plugins(&plugin_directories(paths, layout))
}

fn plugin_directories(paths: &vessel_core::ConfigPaths, layout: &RuntimeLayout) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(parent) = paths.system.parent() {
        directories.push(parent.join("plugins"));
    }
    if let Some(parent) = paths.user.parent() {
        directories.push(parent.join("plugins"));
    }
    directories.push(default_project_plugin_dir(paths, layout));
    directories
}

fn default_project_plugin_dir(
    _paths: &vessel_core::ConfigPaths,
    layout: &RuntimeLayout,
) -> PathBuf {
    layout.plugins_root.clone()
}

fn sanitize_plugin_id(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_owned()
}

fn provider_status(
    plugins: &PluginCatalog,
    configured: Option<&str>,
    purpose: &str,
) -> serde_json::Value {
    match configured {
        None => serde_json::json!({
            "configured": false,
        }),
        Some(reference) => match plugins.resolve_provider(reference, purpose) {
            Ok(Some(provider)) => serde_json::json!({
                "configured": true,
                "reference": format!("{}.{}", provider.plugin_id, provider.provider_id),
                "purpose": provider.purpose,
                "resolved": true,
                "value_present": !provider.value.is_empty(),
            }),
            Ok(None) => serde_json::json!({
                "configured": true,
                "reference": reference,
                "resolved": false,
            }),
            Err(err) => serde_json::json!({
                "configured": true,
                "reference": reference,
                "resolved": false,
                "error": err.to_string(),
            }),
        },
    }
}

async fn extract_video_id_from_input(raw: &str, layout: &RuntimeLayout) -> Result<String> {
    let paths = vessel_core::ConfigPaths::discover();
    let registry = build_registry(&load_runtime_plugins(&paths, layout));
    let input = parse_video_input(raw);
    let extractor = registry
        .best_for(&input)
        .ok_or_else(|| VesselError::Unsupported("no extractor matched input".to_owned()))?;
    let item = extractor
        .extract(ExtractRequest { input }, ExtractContext)
        .await?;
    match item {
        ExtractedItem::Video(video) => Ok(video.video_id),
        _ => Err(VesselError::Unsupported(
            "history lookup requires a video target".to_owned(),
        )),
    }
}

fn increment_summary(summary: &mut serde_json::Value, key: &str, delta: usize) {
    let current = summary
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    summary[key] = serde_json::Value::from(current + delta as u64);
}

fn extend_summary_array(summary: &mut serde_json::Value, key: &str, values: &[String]) {
    let array = summary[key]
        .as_array_mut()
        .expect("summary key should be an array");
    for value in values {
        if !array
            .iter()
            .any(|item| item.as_str() == Some(value.as_str()))
        {
            array.push(serde_json::Value::String(value.clone()));
        }
    }
}

fn merge_summary_tab_counts(
    summary: &mut serde_json::Value,
    key: &str,
    values: &std::collections::BTreeMap<String, usize>,
) {
    let object = summary[key]
        .as_object_mut()
        .expect("summary key should be an object");
    for (tab, count) in values {
        let current = object
            .get(tab)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        object.insert(
            tab.clone(),
            serde_json::Value::from(current + *count as u64),
        );
    }
}

async fn hash_file(path: &std::path::Path) -> Result<String> {
    let bytes = tokio::fs::read(path).await?;
    Ok(hash_bytes(&bytes))
}

fn hash_bytes(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

async fn ensure_project_layout(layout: &RuntimeLayout) -> Result<()> {
    for path in [
        &layout.project_root,
        &layout.downloads_root,
        &layout.thumbnails_root,
        &layout.subtitles_root,
        &layout.plugins_root,
    ] {
        tokio::fs::create_dir_all(path).await?;
    }
    Ok(())
}

fn project_relative_path(layout: &RuntimeLayout, path: &Path) -> String {
    path.strip_prefix(&layout.project_root)
        .map(|relative| relative.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string_lossy().into_owned())
}

fn postprocess_output_path(base_output_path: &Path, args: &DownloadArgs) -> PathBuf {
    if let Some(audio_format) = args.extract_audio.then_some(args.audio_format.as_str()) {
        return base_output_path.with_extension(audio_format);
    }
    if let Some(remux_video) = args.remux_video.as_deref() {
        return base_output_path.with_extension(remux_video);
    }
    base_output_path.to_path_buf()
}

async fn sync_video_thumbnails(
    store: &vessel_store::SqliteStore,
    video: &VideoMetadata,
    layout: &RuntimeLayout,
) -> Result<Vec<String>> {
    let client = http_client()?;
    let mut paths = Vec::new();
    for thumbnail in &video.thumbnails {
        let bytes = fetch_binary(&client, &thumbnail.url).await?;
        let ext = thumbnail_extension(&thumbnail.url);
        let label = match (thumbnail.width, thumbnail.height) {
            (Some(width), Some(height)) => format!("{width}x{height}"),
            _ => "original".to_owned(),
        };
        let relative_path = PathBuf::from("thumbnails")
            .join(&video.video_id)
            .join(format!("{label}.{ext}"));
        let path = layout.project_root.join(&relative_path);
        write_bytes(&path, &bytes).await?;
        store
            .insert_artifact(
                &video.video_id,
                "thumbnail",
                &relative_path.to_string_lossy(),
                &hash_bytes(&bytes),
                bytes.len() as u64,
                None,
            )
            .await?;
        paths.push(relative_path.to_string_lossy().into_owned());
    }
    Ok(paths)
}

async fn sync_primary_thumbnail_artifact(
    store: &vessel_store::SqliteStore,
    video: &VideoMetadata,
    layout: &RuntimeLayout,
) -> Result<Option<PathBuf>> {
    let paths = sync_video_thumbnails(store, video, layout).await?;
    Ok(paths.last().map(PathBuf::from))
}

async fn sync_subtitle_artifacts(
    store: &vessel_store::SqliteStore,
    video: &VideoMetadata,
    layout: &RuntimeLayout,
) -> Result<Vec<String>> {
    let client = http_client()?;
    let mut paths = Vec::new();
    for track in &video.subtitles {
        let Some(url) = track.url.as_deref() else {
            continue;
        };
        let bytes = fetch_binary(&client, url).await?;
        let suffix = if track.is_auto_generated { ".auto" } else { "" };
        let relative_path = PathBuf::from("subtitles")
            .join(&video.video_id)
            .join(format!(
                "{}{}.vtt",
                sanitize_component(&track.language),
                suffix
            ));
        let path = layout.project_root.join(&relative_path);
        write_bytes(&path, &bytes).await?;
        store
            .insert_artifact(
                &video.video_id,
                "subtitle",
                &relative_path.to_string_lossy(),
                &hash_bytes(&bytes),
                bytes.len() as u64,
                None,
            )
            .await?;
        paths.push(relative_path.to_string_lossy().into_owned());
    }
    Ok(paths)
}

async fn fetch_binary(client: &Client, url: &str) -> Result<Vec<u8>> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|err| VesselError::Extractor(format!("artifact request failed: {err}")))?;
    let status = response.status();
    if !status.is_success() {
        return Err(VesselError::Extractor(format!(
            "artifact request returned http status {status}"
        )));
    }
    response
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(|err| VesselError::Extractor(format!("artifact response decode failed: {err}")))
}

async fn write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(path, bytes).await?;
    Ok(())
}

fn http_client() -> Result<Client> {
    Client::builder()
        .user_agent(
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36",
        )
        .build()
        .map_err(|err| VesselError::Extractor(format!("http client build failed: {err}")))
}

fn thumbnail_extension(url: &str) -> &'static str {
    let lower = url.to_ascii_lowercase();
    if lower.contains(".webp") {
        "webp"
    } else if lower.contains(".png") {
        "png"
    } else {
        "jpg"
    }
}

fn print_unsupported_report_with_details(
    feature: &str,
    message: &str,
    details: serde_json::Value,
) -> Result<()> {
    let report = serde_json::json!({
        "status": "unsupported",
        "feature": feature,
        "message": message,
        "native_only": true,
        "details": details,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|err| VesselError::Config(err.to_string()))?
    );
    Ok(())
}

fn binary_available(name: &str) -> bool {
    Command::new(name).arg("-version").output().is_ok()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use clap::Parser;
    use time::OffsetDateTime;

    use super::{
        Cli, Commands, DiarizationSubcommand, SHERPA_EMBEDDING_BYTES, SHERPA_EMBEDDING_FILENAME,
        SHERPA_EMBEDDING_SHA256, SHERPA_EN_EMBEDDING_BYTES, SHERPA_EN_EMBEDDING_FILENAME,
        SHERPA_EN_EMBEDDING_SHA256, UpdateArgs, diarization_fixture_dir,
        install_staged_directory, normalize_update_publication_date,
        parse_sourcearium_channel_input, planned_remaining_bytes, preview_materialization_action,
        push_update_error, push_update_item, resolve_asr_config, resolve_configured_channels,
        resolve_diarization_config, reuse_normalized_audio_fixture, sha256_file,
        sherpa_embedding_spec, transcript_upgrade_probe_due, validate_fixture_video_id,
        verify_integrity_receipt, write_integrity_receipt,
    };    use vessel_core::models::InputKind;
    use vessel_core::{ChannelCategoryConfig, Config, VesselError};
    use vessel_diarization::{DEFAULT_CLUSTERING_THRESHOLD, DEFAULT_WINDOW_SHIFT_RATIO};

    #[test]
    fn diarization_fixture_ids_are_path_safe() {
        assert_eq!(
            validate_fixture_video_id("g5o-OpVUHF0").unwrap(),
            "g5o-OpVUHF0"
        );
        assert!(validate_fixture_video_id("../escape").is_err());
        assert!(validate_fixture_video_id("bad/id").is_err());
        let path = diarization_fixture_dir("g5o-OpVUHF0").expect("fixture path");
        assert!(path.ends_with("fixtures/diarization/youtube/g5o-OpVUHF0"));
    }

    #[test]
    fn sha256_file_matches_known_vector() {
        let root = std::env::temp_dir().join(format!("vessel-sha256-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("hash temp root");
        let path = root.join("abc.txt");
        fs::write(&path, b"abc").expect("hash fixture");
        assert_eq!(
            sha256_file(&path).expect("sha256"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn diarization_plan_accounts_for_resumable_partial_bytes() {
        let root =
            std::env::temp_dir().join(format!("vessel-diarization-plan-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("plan temp root");

        let destination = root.join("runtime.tar.bz2");
        let partial = destination.with_extension("download");
        fs::write(&partial, vec![0u8; 40]).expect("partial download");

        let (resume, remaining) = planned_remaining_bytes(&destination, 100, false);
        assert_eq!(resume, 40);
        assert_eq!(remaining, 60);

        fs::write(&destination, vec![0u8; 100]).expect("complete archive");
        let (resume, remaining) = planned_remaining_bytes(&destination, 100, false);
        assert_eq!(resume, 100);
        assert_eq!(remaining, 0);

        let (resume, remaining) = planned_remaining_bytes(&destination, 100, true);
        assert_eq!(resume, 0);
        assert_eq!(remaining, 0);

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn staged_directory_install_replaces_live_tree_only_after_stage_exists() {
        let root =
            std::env::temp_dir().join(format!("vessel-staged-install-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("staged install temp root");

        let live = root.join("live");
        let staging = root.join("staging");
        fs::create_dir_all(&live).expect("live dir");
        fs::create_dir_all(&staging).expect("staging dir");
        fs::write(live.join("version.txt"), b"old").expect("old live content");
        fs::write(staging.join("version.txt"), b"new").expect("new staged content");

        install_staged_directory(&staging, &live).expect("install staged directory");

        assert!(!staging.exists());
        assert_eq!(
            fs::read_to_string(live.join("version.txt")).expect("installed content"),
            "new"
        );
        assert!(!root.join(".live.previous").exists());

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn sherpa_embedding_profiles_are_explicit_and_pinned() {
        let (default_name, _, default_bytes, default_sha) =
            sherpa_embedding_spec("zh-3dspeaker").expect("default embedding profile");
        assert_eq!(default_name, SHERPA_EMBEDDING_FILENAME);
        assert_eq!(default_bytes, SHERPA_EMBEDDING_BYTES);
        assert_eq!(default_sha, SHERPA_EMBEDDING_SHA256);

        let (english_name, _, english_bytes, english_sha) =
            sherpa_embedding_spec("en-voxceleb").expect("English embedding profile");
        assert_eq!(english_name, SHERPA_EN_EMBEDDING_FILENAME);
        assert_eq!(english_bytes, SHERPA_EN_EMBEDDING_BYTES);
        assert_eq!(english_sha, SHERPA_EN_EMBEDDING_SHA256);
        assert!(sherpa_embedding_spec("unknown").is_err());
    }

    #[test]
    fn diarization_integrity_receipt_detects_post_fetch_corruption() {
        let root = std::env::temp_dir().join(format!(
            "vessel-diarization-integrity-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("integrity temp root");

        let artifact = root.join("model.onnx");
        let receipt = root.join("receipt.json");
        fs::write(&artifact, b"abc123").expect("artifact");

        write_integrity_receipt(
            &receipt,
            &root,
            "test_artifact",
            &[("model", artifact.as_path())],
        )
        .expect("write receipt");

        let clean = verify_integrity_receipt(&receipt, &root);
        assert!(clean.present);
        assert!(clean.verified);
        assert!(clean.errors.is_empty());

        fs::write(&artifact, b"abc124").expect("corrupt artifact with same byte count");
        let corrupted = verify_integrity_receipt(&receipt, &root);
        assert!(corrupted.present);
        assert!(!corrupted.verified);
        assert!(
            corrupted
                .errors
                .iter()
                .any(|error| error.contains("BLAKE3 mismatch"))
        );

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn configured_channels_select_all_or_requested_categories() {
        let mut config = Config::default();
        config.channels.categories.insert(
            "gaming".to_owned(),
            ChannelCategoryConfig {
                channels: vec!["https://www.youtube.com/@one".to_owned()],
            },
        );
        config.channels.categories.insert(
            "news".to_owned(),
            ChannelCategoryConfig {
                channels: vec!["https://www.youtube.com/@two".to_owned()],
            },
        );

        let all = resolve_configured_channels(&config, &[]).expect("all categories");
        assert_eq!(all.len(), 2);

        let gaming = resolve_configured_channels(&config, &["gaming".to_owned()]).expect("gaming");
        assert_eq!(gaming.len(), 1);
        assert_eq!(gaming[0].category, "gaming");
    }

    #[test]
    fn update_defaults_to_external_toolchain() {
        let cli = Cli::try_parse_from(["vessel", "update"]).expect("parse update");
        let Commands::Update(args) = cli.command else {
            panic!("expected update command");
        };
        assert_eq!(args.youtube_backend, "yt-dlp");
        assert_eq!(args.yt_dlp_executable, PathBuf::from("yt-dlp"));
        assert_eq!(args.diarization_backend, "whisperx");
        let asr = resolve_asr_config(&args);
        assert_eq!(asr.backend, vessel_asr::WHISPERX_BACKEND_NAME);
        assert_eq!(asr.model, "large-v3");
    }

    #[test]
    fn asr_cli_overrides_are_operational_only() {
        let args = UpdateArgs {
            sourcearium: PathBuf::from("."),
            max_videos: None,
            video_ids: Vec::new(),
            youtube_backend: "yt-dlp".into(),
            yt_dlp_executable: PathBuf::from("yt-dlp"),
            force_local_asr: false,
            asr_backend: Some("whisper-candle".into()),
            asr_model: Some("base".into()),
            asr_model_dir: Some(PathBuf::from("/models/whisper-base")),
            asr_executable: Some(PathBuf::from("/bin/asr")),
            asr_device: Some("cpu".into()),
            asr_language: Some("es".into()),
            diarize: true,
            diarization_backend: "sherpa-onnx".into(),
            diarization_segmentation_model: None,
            diarization_embedding_model: None,
            diarization_runtime_dir: None,
            diarization_provider: "cpu".into(),
            diarization_num_threads: 4,
            diarization_clustering_threshold: DEFAULT_CLUSTERING_THRESHOLD,
            diarization_window_shift_ratio: DEFAULT_WINDOW_SHIFT_RATIO,
            diarization_model: Some("example/diarizer".into()),
            min_speakers: Some(1),
            max_speakers: Some(3),
            speaker_embeddings: true,
            attribute_speakers: false,
            speaker_min_similarity: 0.80,
            speaker_min_margin: 0.05,
            speaker_min_anchor_dominance: 0.80,
            hf_token_env: "TEST_HF_TOKEN".into(),
            upgrade_check_days: 30,
            report_items: false,
            preview: false,
        };
        let config = resolve_asr_config(&args);
        assert_eq!(config.backend, "whisper-candle");
        assert_eq!(config.model, "base");
        assert_eq!(
            config.model_dir.as_deref(),
            Some(std::path::Path::new("/models/whisper-base"))
        );
        assert_eq!(
            config.executable.as_deref(),
            Some(std::path::Path::new("/bin/asr"))
        );
        assert_eq!(config.device, "cpu");
        assert_eq!(config.language.as_deref(), Some("es"));
        assert!(!config.diarize);
        assert_eq!(config.diarization_model, "example/diarizer");
        assert_eq!(config.min_speakers, None);
        assert_eq!(config.max_speakers, None);
        assert!(!config.speaker_embeddings);
        assert_eq!(config.hf_token_env, "TEST_HF_TOKEN");
    }

    #[test]
    fn phonon_backend_gets_phonon_default_model() {
        let args = UpdateArgs {
            sourcearium: PathBuf::from("."),
            max_videos: None,
            video_ids: Vec::new(),
            youtube_backend: "yt-dlp".into(),
            yt_dlp_executable: PathBuf::from("yt-dlp"),
            force_local_asr: false,
            asr_backend: Some("phonon-2".into()),
            asr_model: None,
            asr_model_dir: None,
            asr_executable: None,
            asr_device: None,
            asr_language: None,
            diarize: false,
            diarization_backend: "sherpa-onnx".into(),
            diarization_segmentation_model: None,
            diarization_embedding_model: None,
            diarization_runtime_dir: None,
            diarization_provider: "cpu".into(),
            diarization_num_threads: 4,
            diarization_clustering_threshold: DEFAULT_CLUSTERING_THRESHOLD,
            diarization_window_shift_ratio: DEFAULT_WINDOW_SHIFT_RATIO,
            diarization_model: None,
            min_speakers: None,
            max_speakers: None,
            speaker_embeddings: false,
            attribute_speakers: false,
            speaker_min_similarity: 0.80,
            speaker_min_margin: 0.05,
            speaker_min_anchor_dominance: 0.80,
            hf_token_env: "HF_TOKEN".into(),
            upgrade_check_days: 30,
            report_items: false,
            preview: false,
        };
        let config = resolve_asr_config(&args);
        assert_eq!(config.backend, "phonon-2");
        assert_eq!(config.model, "phonon-2");
    }

    #[test]
    fn whisperx_backend_gets_large_v3_default_model() {
        let args = UpdateArgs {
            sourcearium: PathBuf::from("."),
            max_videos: None,
            video_ids: Vec::new(),
            youtube_backend: "yt-dlp".into(),
            yt_dlp_executable: PathBuf::from("yt-dlp"),
            force_local_asr: false,
            asr_backend: Some("whisperx".into()),
            asr_model: None,
            asr_model_dir: None,
            asr_executable: None,
            asr_device: None,
            asr_language: None,
            diarize: true,
            diarization_backend: "whisperx".into(),
            diarization_segmentation_model: None,
            diarization_embedding_model: None,
            diarization_runtime_dir: None,
            diarization_provider: "cpu".into(),
            diarization_num_threads: 4,
            diarization_clustering_threshold: DEFAULT_CLUSTERING_THRESHOLD,
            diarization_window_shift_ratio: DEFAULT_WINDOW_SHIFT_RATIO,
            diarization_model: None,
            min_speakers: None,
            max_speakers: None,
            speaker_embeddings: false,
            attribute_speakers: false,
            speaker_min_similarity: 0.80,
            speaker_min_margin: 0.05,
            speaker_min_anchor_dominance: 0.80,
            hf_token_env: "HF_TOKEN".into(),
            upgrade_check_days: 30,
            report_items: false,
            preview: false,
        };
        let config = resolve_asr_config(&args);
        assert_eq!(config.backend, "whisperx");
        assert_eq!(config.model, "large-v3");
        assert!(config.diarize);
    }

    #[test]
    fn retired_speaker_controls_are_rejected() {
        let cli = Cli::try_parse_from([
            "vessel",
            "update",
            "--speaker-embeddings",
        ])
        .expect("legacy flag still parses during migration");
        let Commands::Update(args) = cli.command else {
            panic!("expected update command");
        };
        let error = super::reject_retired_speaker_controls(&args)
            .expect_err("speaker embedding controls must be retired");
        assert!(error.to_string().contains("retired from active Vessel"));
    }

    #[test]
    fn force_local_asr_flag_is_explicit() {
        let cli = Cli::try_parse_from([
            "vessel",
            "update",
            "--sourcearium",
            "/tmp/sourcearium",
            "--video-id",
            "video-1",
            "--force-local-asr",
            "--max-videos",
            "1",
        ])
        .expect("parse forced local ASR");
        let Commands::Update(args) = cli.command else {
            panic!("expected update command");
        };
        assert!(args.force_local_asr);
        assert_eq!(args.video_ids, vec!["video-1"]);
        assert_eq!(args.max_videos, Some(1));
    }

    #[test]
    fn rust_diarization_config_is_independent_from_asr_backend() {
        let root =
            std::env::temp_dir().join(format!("vessel-diarization-config-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("temp model dir");
        let segmentation = root.join("segmentation.onnx");
        let embedding = root.join("embedding.onnx");
        let runtime = root.join(if cfg!(target_os = "windows") {
            "sherpa-onnx-c-api.dll"
        } else if cfg!(target_os = "macos") {
            "libsherpa-onnx-c-api.dylib"
        } else {
            "libsherpa-onnx-c-api.so"
        });
        fs::write(&segmentation, b"model").expect("segmentation model");
        fs::write(&embedding, b"model").expect("embedding model");
        fs::write(&runtime, b"runtime").expect("runtime library");

        let args = UpdateArgs {
            sourcearium: PathBuf::from("."),
            max_videos: None,
            video_ids: Vec::new(),
            youtube_backend: "yt-dlp".into(),
            yt_dlp_executable: PathBuf::from("yt-dlp"),
            force_local_asr: false,
            asr_backend: Some("phonon-2".into()),
            asr_model: None,
            asr_model_dir: None,
            asr_executable: None,
            asr_device: None,
            asr_language: Some("en".into()),
            diarize: true,
            diarization_backend: "sherpa-onnx".into(),
            diarization_segmentation_model: Some(segmentation.clone()),
            diarization_embedding_model: Some(embedding.clone()),
            diarization_runtime_dir: Some(root.clone()),
            diarization_provider: "cpu".into(),
            diarization_num_threads: 6,
            diarization_clustering_threshold: 0.57,
            diarization_window_shift_ratio: 0.12,
            diarization_model: None,
            min_speakers: Some(2),
            max_speakers: Some(2),
            speaker_embeddings: false,
            attribute_speakers: false,
            speaker_min_similarity: 0.80,
            speaker_min_margin: 0.05,
            speaker_min_anchor_dominance: 0.80,
            hf_token_env: "HF_TOKEN".into(),
            upgrade_check_days: 30,
            report_items: false,
            preview: false,
        };

        let asr = resolve_asr_config(&args);
        assert_eq!(asr.backend, "phonon-2");
        assert!(!asr.diarize);
        let diarization = resolve_diarization_config(&args)
            .expect("resolve diarization")
            .expect("configured");
        assert_eq!(diarization.backend, "sherpa-onnx");
        assert_eq!(diarization.runtime_library, runtime);
        assert_eq!(diarization.segmentation_model, segmentation);
        assert_eq!(diarization.embedding_model, embedding);
        assert_eq!(diarization.provider, "cpu");
        assert_eq!(diarization.num_threads, 6);
        assert_eq!(diarization.num_speakers, Some(2));
        assert_eq!(diarization.clustering_threshold, 0.57);
        assert_eq!(diarization.window_shift_ratio, 0.12);
        assert!(!diarization.speaker_embeddings);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn durable_fixture_reuse_preserves_source_and_creates_asr_input() {
        let root =
            std::env::temp_dir().join(format!("vessel-asr-fixture-reuse-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("temp dir");
        let fixture = root.join("fixture.wav");
        let input = root.join("whisper-input.wav");
        let bytes = vec![7_u8; 128];
        fs::write(&fixture, &bytes).expect("write fixture");

        reuse_normalized_audio_fixture(&fixture, &input).expect("reuse fixture");

        assert_eq!(fs::read(&fixture).unwrap(), bytes);
        assert_eq!(fs::read(&input).unwrap(), bytes);
        fs::remove_file(&input).expect("remove disposable input");
        assert!(fixture.is_file());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn diarization_reembed_parses_as_offline_embedding_refresh() {
        let cli = Cli::try_parse_from([
            "vessel",
            "diarization",
            "reembed",
            "--sourcearium",
            "/tmp/sourcearium",
            "--source-key",
            "example",
            "--video-id",
            "video-1",
        ])
        .expect("parse diarization reembed");
        let Commands::Diarization(command) = cli.command else {
            panic!("expected diarization command");
        };
        let DiarizationSubcommand::Reembed(args) = command.command else {
            panic!("expected diarization reembed");
        };
        assert_eq!(args.source_key, "example");
        assert_eq!(args.video_id, "video-1");
        assert_eq!(args.num_threads, 4);
    }

    #[test]
    fn transcript_upgrade_probe_cadence_is_operational() {
        let now = OffsetDateTime::UNIX_EPOCH + time::Duration::days(40);
        let recent = (now - time::Duration::days(5))
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        let old = (now - time::Duration::days(35))
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();

        assert!(!transcript_upgrade_probe_due(Some(&recent), 30, now));
        assert!(transcript_upgrade_probe_due(Some(&old), 30, now));
        assert!(transcript_upgrade_probe_due(None, 30, now));
        assert!(transcript_upgrade_probe_due(Some(&recent), 0, now));
        assert!(transcript_upgrade_probe_due(Some("invalid"), 30, now));
    }

    #[test]
    fn preview_actions_distinguish_create_upgrade_and_compare() {
        use vessel_core::TranscriptDerivation;

        assert_eq!(
            preview_materialization_action(None, TranscriptDerivation::CreatorSubtitles),
            "would_create"
        );
        assert_eq!(
            preview_materialization_action(
                Some("local_asr"),
                TranscriptDerivation::PlatformAutoCaption,
            ),
            "would_upgrade"
        );
        assert_eq!(
            preview_materialization_action(
                Some("platform_auto_caption"),
                TranscriptDerivation::CreatorSubtitles,
            ),
            "would_upgrade"
        );
        assert_eq!(
            preview_materialization_action(
                Some("platform_auto_caption"),
                TranscriptDerivation::PlatformAutoCaption,
            ),
            "would_compare_content"
        );
    }

    #[test]
    fn update_errors_also_enter_item_trace_when_enabled() {
        let mut summary = serde_json::json!({
            "errors": [],
            "items": [],
        });
        push_update_error(
            &mut summary,
            "abc123",
            VesselError::Extractor("youtube anti-bot gate".into()),
        );

        assert_eq!(summary["errors"][0]["video_id"], "abc123");
        assert_eq!(summary["items"][0]["video_id"], "abc123");
        assert_eq!(summary["items"][0]["action"], "failed");
        assert!(
            summary["items"][0]["details"]["message"]
                .as_str()
                .unwrap()
                .contains("anti-bot gate")
        );
    }

    #[test]
    fn report_items_are_opt_in_and_structured() {
        let mut summary = serde_json::json!({"items": []});
        push_update_item(
            &mut summary,
            true,
            "abc123",
            "created",
            serde_json::json!({"derivation": "creator_subtitles"}),
        );
        assert_eq!(summary["items"][0]["video_id"], "abc123");
        assert_eq!(summary["items"][0]["action"], "created");

        let mut disabled = serde_json::json!({"items": []});
        push_update_item(
            &mut disabled,
            false,
            "abc123",
            "created",
            serde_json::json!({}),
        );
        assert_eq!(disabled["items"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn sourcearium_channel_handles_are_normalized_to_urls() {
        let input = parse_sourcearium_channel_input("@example").expect("handle");
        assert!(matches!(input.kind, InputKind::Url));
        assert_eq!(input.raw, "https://www.youtube.com/@example");

        let input = parse_sourcearium_channel_input("UC123").expect("id");
        assert!(matches!(input.kind, InputKind::ChannelId));
        assert_eq!(input.raw, "UC123");
    }

    #[test]
    fn update_publication_dates_accept_youtube_compact_dates() {
        assert_eq!(
            normalize_update_publication_date(Some("20260918")),
            Some("2026-09-18".into())
        );
        assert_eq!(
            normalize_update_publication_date(Some("2026-09-18")),
            Some("2026-09-18".into())
        );
        assert_eq!(normalize_update_publication_date(Some("2 years ago")), None);
    }

    #[test]
    fn configured_channels_reject_duplicates_across_categories() {
        let mut config = Config::default();
        config.channels.categories.insert(
            "gaming".to_owned(),
            ChannelCategoryConfig {
                channels: vec!["https://www.youtube.com/@same".to_owned()],
            },
        );
        config.channels.categories.insert(
            "news".to_owned(),
            ChannelCategoryConfig {
                channels: vec!["https://www.youtube.com/@same".to_owned()],
            },
        );

        let err = resolve_configured_channels(&config, &[]).expect_err("duplicate should fail");
        assert!(err.to_string().contains("more than one category"));
    }
}
