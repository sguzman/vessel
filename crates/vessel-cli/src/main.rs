use std::path::Path;
use std::process::Command;

use clap::{Args, Parser, Subcommand};
use time::OffsetDateTime;
use vessel_core::models::{InputKind, InputRef};
use vessel_core::{Config, Result, VesselError, load_config};
use vessel_download::{BasicDownloadPlanner, DownloadPlanner, execute_download};
use vessel_extractors::youtube::{
    ChannelVideoRef, YoutubeExtractor, extract_channel, extract_video, list_channel_videos,
};
use vessel_extractors::{ExtractContext, ExtractRequest, ExtractedItem, ExtractorRegistry};
use vessel_formats::FormatSelector;
use vessel_ledger::{AttemptStatus, FetchAttempt, Ledger};
use vessel_store::{StoredTrackedChannel, init_sqlite_database};

#[derive(Debug, Parser)]
#[command(
    name = "vessel",
    version,
    about = "Rust-native media extraction and metadata ledger"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Doctor,
    Config(ConfigCommand),
    Dataset(DatasetCommand),
    Channel(ChannelCommand),
    Video(VideoCommand),
    Info(UrlArg),
    Formats(UrlArg),
    Download(DownloadArgs),
}

#[derive(Debug, Args)]
struct UrlArg {
    url: String,
}

#[derive(Debug, Args)]
struct DownloadArgs {
    url: String,
    #[arg(short = 'f', long = "format")]
    format: Option<String>,
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
}

#[derive(Debug, Args)]
struct DatasetInitArgs {
    #[arg(long = "db")]
    db: Option<String>,
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
}

#[derive(Debug, Args)]
struct ChannelAddArgs {
    channel: String,
}

#[derive(Debug, Args)]
struct ChannelSyncArgs {
    #[arg(long)]
    comments: bool,
    #[arg(long)]
    subtitles: bool,
    #[arg(long)]
    since: Option<String>,
    #[arg(long = "max-videos")]
    max_videos: Option<usize>,
}

#[derive(Debug, Args)]
struct VideoCommand {
    #[command(subcommand)]
    command: VideoSubcommand,
}

#[derive(Debug, Subcommand)]
enum VideoSubcommand {
    Refresh(VideoRefArg),
    History(VideoRefArg),
}

#[derive(Debug, Args)]
struct VideoRefArg {
    video: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let (config, paths, loaded_from) = load_config()?;
    init_logging(&config)?;

    match cli.command {
        Commands::Doctor => doctor(&config, &paths, &loaded_from).await,
        Commands::Config(cmd) => match cmd.command {
            ConfigSubcommand::Show => show_config(&config, &paths, &loaded_from),
        },
        Commands::Dataset(cmd) => match cmd.command {
            DatasetSubcommand::Init(args) => dataset_init(args, &config).await,
        },
        Commands::Channel(cmd) => match cmd.command {
            ChannelSubcommand::Add(args) => channel_add(args, &config).await,
            ChannelSubcommand::Sync(args) => channel_sync(args, &config).await,
        },
        Commands::Video(cmd) => match cmd.command {
            VideoSubcommand::Refresh(args) => video_refresh(args, &config).await,
            VideoSubcommand::History(args) => video_history(args, &config).await,
        },
        Commands::Info(arg) => extract_preview(arg.url, InputKind::Url).await,
        Commands::Formats(arg) => formats(arg.url).await,
        Commands::Download(args) => download(args, &config).await,
    }
}

fn init_logging(config: &Config) -> Result<()> {
    vessel_logging::init(&config.logging.level, config.logging.format)
}

async fn doctor(
    config: &Config,
    paths: &vessel_core::ConfigPaths,
    loaded_from: &[std::path::PathBuf],
) -> Result<()> {
    let requested = config.database.url.clone();
    let db_target = requested.strip_prefix("sqlite://").unwrap_or(&requested);
    let db_exists = Path::new(db_target).exists();
    let report = serde_json::json!({
        "config_paths": {
            "system": paths.system,
            "user": paths.user,
            "project": paths.project,
            "loaded_from": loaded_from,
        },
        "database": {
            "configured_url": config.database.url,
            "exists": db_exists,
        },
        "binaries": {
            "ffmpeg": binary_available("ffmpeg"),
            "yt-dlp": binary_available("yt-dlp"),
        }
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|err| VesselError::Config(err.to_string()))?
    );
    Ok(())
}

fn show_config(
    config: &Config,
    paths: &vessel_core::ConfigPaths,
    loaded_from: &[std::path::PathBuf],
) -> Result<()> {
    let report = serde_json::json!({
        "config": config,
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
    Ok(())
}

async fn dataset_init(args: DatasetInitArgs, config: &Config) -> Result<()> {
    let target = args.db.unwrap_or_else(|| config.database.url.clone());
    let (_store, paths) = init_sqlite_database(&target).await?;
    let report = serde_json::json!({
        "status": "initialized",
        "database": {
            "requested": paths.requested,
            "sqlite_url": paths.sqlite_url,
        }
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|err| VesselError::Config(err.to_string()))?
    );
    Ok(())
}

async fn extract_preview(url: String, kind: InputKind) -> Result<()> {
    let registry = build_registry();
    let input = InputRef { raw: url, kind };
    let extractor = registry
        .best_for(&input)
        .ok_or_else(|| VesselError::Unsupported("no extractor matched input".to_owned()))?;
    let item = extractor
        .extract(ExtractRequest { input }, ExtractContext)
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&item).map_err(|err| VesselError::Config(err.to_string()))?
    );
    Ok(())
}

async fn channel_add(args: ChannelAddArgs, config: &Config) -> Result<()> {
    let (store, _) = init_sqlite_database(&config.database.url).await?;
    let input = parse_channel_input(&args.channel);
    let channel = extract_channel(&input).await?;
    store.upsert_channel_snapshot(&channel).await?;
    store.add_tracked_channel(&channel).await?;

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "status": "tracked",
            "channel_id": channel.channel_id,
            "handle": channel.handle,
            "title": channel.title,
            "url": channel.url,
        }))
        .map_err(|err| VesselError::Config(err.to_string()))?
    );
    Ok(())
}

async fn channel_sync(args: ChannelSyncArgs, config: &Config) -> Result<()> {
    let (store, _) = init_sqlite_database(&config.database.url).await?;
    let tracked_channels = store.list_tracked_channels().await?;
    let run_id = store.start_run("channel sync").await?;
    let started_at = OffsetDateTime::now_utc();

    let result = async {
        let mut summary = serde_json::json!({
            "status": "synced",
            "run_id": run_id,
            "tracked_channels": tracked_channels.len(),
            "channels_processed": 0usize,
            "channel_snapshots_inserted": 0usize,
            "videos_discovered": 0usize,
            "video_snapshots_inserted": 0usize,
            "video_refreshes_skipped_by_since": 0usize,
            "errors": 0usize,
            "options": {
                "comments": args.comments,
                "subtitles": args.subtitles,
                "since": args.since,
                "max_videos": args.max_videos,
            }
        });

        for tracked in tracked_channels {
            match sync_one_channel(&store, &tracked, &args, run_id, started_at).await {
                Ok(channel_report) => {
                    increment_summary(&mut summary, "channels_processed", 1);
                    increment_summary(
                        &mut summary,
                        "channel_snapshots_inserted",
                        usize::from(channel_report.channel_snapshot_inserted),
                    );
                    increment_summary(
                        &mut summary,
                        "videos_discovered",
                        channel_report.videos_discovered,
                    );
                    increment_summary(
                        &mut summary,
                        "video_snapshots_inserted",
                        channel_report.video_snapshots_inserted,
                    );
                    increment_summary(
                        &mut summary,
                        "video_refreshes_skipped_by_since",
                        channel_report.skipped_by_since,
                    );
                }
                Err(err) => {
                    increment_summary(&mut summary, "errors", 1);
                    store
                        .record_attempt(FetchAttempt {
                            run_id,
                            target_kind: "channel".to_owned(),
                            target_external_id: tracked.channel_id.clone(),
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

async fn video_refresh(args: VideoRefArg, config: &Config) -> Result<()> {
    let (store, _) = init_sqlite_database(&config.database.url).await?;
    let registry = build_registry();
    let input = parse_video_input(&args.video);
    let target_id = args.video.clone();
    let run_id = store.start_run("video refresh").await?;
    let started_at = OffsetDateTime::now_utc();

    let result = async {
        let extractor = registry
            .best_for(&input)
            .ok_or_else(|| VesselError::Unsupported("no extractor matched input".to_owned()))?;
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
        let snapshot_inserted = store.upsert_video_snapshot(&video).await?;
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
            "snapshot_inserted": snapshot_inserted,
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

async fn video_history(args: VideoRefArg, config: &Config) -> Result<()> {
    let (store, _) = init_sqlite_database(&config.database.url).await?;
    let lookup = if args.video.contains("://") {
        extract_video_id_from_input(&args.video).await?
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

async fn formats(url: String) -> Result<()> {
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

async fn download(args: DownloadArgs, config: &Config) -> Result<()> {
    let (store, _) = init_sqlite_database(&config.database.url).await?;
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

    let selector = parse_format_selector(args.format.as_deref());
    let planner = BasicDownloadPlanner;
    let fallback_shape = select_format_for_output(&video, &selector);
    let native_plan = planner.plan(&video, selector.clone(), &config.download.output);
    let fallback_output_path =
        render_fallback_output_path(&video, fallback_shape, &config.download.output);
    let result = match native_plan {
        Ok(plan) => match execute_download(&plan).await {
            Ok(result) => result,
            Err(_err) if binary_available("yt-dlp") => {
                fallback_download_with_ytdlp(&args.url, args.format.as_deref(), &plan.output_path)
                    .await?;
                let bytes_written = tokio::fs::metadata(&plan.output_path).await?.len();
                vessel_download::DownloadResult {
                    format_id: fallback_shape
                        .as_ref()
                        .map(|format| format.format_id.clone())
                        .unwrap_or_else(|| plan.format_id.clone()),
                    output_path: plan.output_path.clone(),
                    temp_path: plan.temp_path.clone(),
                    bytes_written,
                    resumed: false,
                }
            }
            Err(err) => return Err(err),
        },
        Err(_err) if binary_available("yt-dlp") => {
            fallback_download_with_ytdlp(&args.url, args.format.as_deref(), &fallback_output_path)
                .await?;
            let bytes_written = tokio::fs::metadata(&fallback_output_path).await?.len();
            vessel_download::DownloadResult {
                format_id: fallback_shape
                    .as_ref()
                    .map(|format| format.format_id.clone())
                    .unwrap_or_else(|| "yt-dlp".to_owned()),
                output_path: fallback_output_path.clone(),
                temp_path: fallback_output_path.with_extension("part"),
                bytes_written,
                resumed: false,
            }
        }
        Err(err) => return Err(err),
    };
    let file_hash = hash_file(&result.output_path).await?;
    let artifact = store
        .insert_artifact(
            &video.video_id,
            "video",
            &result.output_path.to_string_lossy(),
            &file_hash,
            result.bytes_written,
            Some(&result.format_id),
        )
        .await?;
    store
        .insert_archive_entry("youtube", &video.video_id, &artifact.artifact_id)
        .await?;

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "status": "downloaded",
            "video_id": video.video_id,
            "title": video.title,
            "format_id": result.format_id,
            "output_path": result.output_path,
            "bytes_written": result.bytes_written,
            "resumed": result.resumed,
            "artifact_id": artifact.artifact_id,
            "content_hash": artifact.content_hash,
        }))
        .map_err(|err| VesselError::Config(err.to_string()))?
    );
    Ok(())
}

fn build_registry() -> ExtractorRegistry {
    let mut registry = ExtractorRegistry::default();
    registry.register(YoutubeExtractor);
    registry
}

#[derive(Debug, Default)]
struct ChannelSyncReport {
    channel_snapshot_inserted: bool,
    videos_discovered: usize,
    video_snapshots_inserted: usize,
    skipped_by_since: usize,
}

async fn sync_one_channel(
    store: &vessel_store::SqliteStore,
    tracked: &StoredTrackedChannel,
    args: &ChannelSyncArgs,
    run_id: uuid::Uuid,
    started_at: OffsetDateTime,
) -> Result<ChannelSyncReport> {
    let input = parse_channel_input(&tracked.canonical_url);
    let channel = extract_channel(&input).await?;
    let channel_snapshot_inserted = store.upsert_channel_snapshot(&channel).await?;
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

    let mut videos = list_channel_videos(&input).await?;
    let discovered_count = videos.len();
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
        channel_snapshot_inserted,
        videos_discovered: discovered_count,
        ..ChannelSyncReport::default()
    };

    for video_ref in videos {
        if sync_channel_video(store, video_ref).await? {
            report.video_snapshots_inserted += 1;
        }
    }

    report.skipped_by_since = skipped_by_since;
    store
        .mark_tracked_channel_synced(&channel.channel_id)
        .await?;
    Ok(report)
}

async fn sync_channel_video(
    store: &vessel_store::SqliteStore,
    video_ref: ChannelVideoRef,
) -> Result<bool> {
    let input = InputRef {
        raw: video_ref.video_id,
        kind: InputKind::VideoId,
    };
    let video = extract_video(&input).await?;
    store.upsert_video_snapshot(&video).await
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

fn parse_format_selector(raw: Option<&str>) -> FormatSelector {
    match raw {
        None | Some("best") => FormatSelector::Best,
        Some("worst") => FormatSelector::Worst,
        Some("ba") | Some("bestaudio") => FormatSelector::BestAudio,
        Some("bv") | Some("bestvideo") => FormatSelector::BestVideo,
        Some(value) => FormatSelector::ExactFormatId(value.to_owned()),
    }
}

fn select_format_for_output<'a>(
    video: &'a vessel_core::models::VideoMetadata,
    selector: &FormatSelector,
) -> Option<&'a vessel_core::models::MediaFormat> {
    match selector {
        FormatSelector::ExactFormatId(format_id) => video
            .formats
            .iter()
            .find(|format| format.format_id == *format_id),
        FormatSelector::Worst => video
            .formats
            .iter()
            .filter(|format| format.has_video || format.has_audio)
            .min_by_key(|format| {
                (
                    u8::from(format.has_video && format.has_audio),
                    format.height.unwrap_or(0),
                    format.bitrate.unwrap_or(0),
                )
            }),
        _ => video
            .formats
            .iter()
            .filter(|format| format.has_video || format.has_audio)
            .max_by_key(|format| {
                (
                    u8::from(format.has_video && format.has_audio),
                    format.height.unwrap_or(0),
                    format.bitrate.unwrap_or(0),
                )
            }),
    }
}

fn render_fallback_output_path(
    video: &vessel_core::models::VideoMetadata,
    format: Option<&vessel_core::models::MediaFormat>,
    template: &str,
) -> std::path::PathBuf {
    let ext = format.map(|format| format.ext.as_str()).unwrap_or("mp4");
    let mut output = template.to_owned();
    output = output.replace("%(id)s", &sanitize_component(&video.video_id));
    output = output.replace(
        "%(title)s",
        &sanitize_component(video.title.as_deref().unwrap_or(&video.video_id)),
    );
    output = output.replace(
        "%(channel)s",
        &sanitize_component(video.channel_id.as_deref().unwrap_or("unknown-channel")),
    );
    output = output.replace(
        "%(upload_date)s",
        &sanitize_component(video.upload_date.as_deref().unwrap_or("unknown-date")),
    );
    output = output.replace("%(ext)s", ext);
    std::path::PathBuf::from(output)
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

async fn extract_video_id_from_input(raw: &str) -> Result<String> {
    let registry = build_registry();
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

async fn hash_file(path: &std::path::Path) -> Result<String> {
    let bytes = tokio::fs::read(path).await?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

async fn fallback_download_with_ytdlp(
    url: &str,
    requested_format: Option<&str>,
    output_path: &std::path::Path,
) -> Result<()> {
    let mut command = tokio::process::Command::new("yt-dlp");
    command.arg("--no-progress");
    command.arg("--output").arg(output_path);
    if let Some(format) = requested_format {
        command.arg("-f").arg(format);
    }
    command.arg(url);

    let status = command.status().await?;
    if status.success() {
        Ok(())
    } else {
        Err(VesselError::Extractor(format!(
            "yt-dlp fallback failed with exit status {status}"
        )))
    }
}

fn binary_available(name: &str) -> bool {
    Command::new(name).arg("-version").output().is_ok()
}
