use std::path::{Path, PathBuf};
use std::process::Command;

use clap::{Args, Parser, Subcommand};
use reqwest::Client;
use time::OffsetDateTime;
use vessel_core::models::{InputKind, InputRef, VideoMetadata};
use vessel_core::{Config, Result, VesselError, load_config};
use vessel_download::{BasicDownloadPlanner, DownloadPlanner, execute_download};
use vessel_extractors::youtube::{
    ChannelVideoRef, YoutubeExtractor, extract_channel, extract_comments, extract_video,
    list_channel_videos,
};
use vessel_extractors::{ExtractContext, ExtractRequest, ExtractedItem, ExtractorRegistry};
use vessel_formats::{FormatSelector, parse_selector};
use vessel_ledger::{AttemptStatus, FetchAttempt, Ledger};
use vessel_postprocess::{PostprocessRequest, build_plan, execute_plan};
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
    #[arg(long = "download-thumbnails")]
    download_thumbnails: bool,
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
    Subtitles(VideoSubtitlesCommand),
    Comments(VideoCommentsCommand),
}

#[derive(Debug, Args)]
struct VideoRefArg {
    video: String,
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
            VideoSubcommand::Subtitles(cmd) => match cmd.command {
                VideoSubtitlesSubcommand::Sync(args) => video_subtitles_sync(args, &config).await,
            },
            VideoSubcommand::Comments(cmd) => match cmd.command {
                VideoCommentsSubcommand::Sync(args) => video_comments_sync(args, &config).await,
            },
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
                "download_thumbnails": args.download_thumbnails,
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
        let subtitle_snapshots_inserted =
            store.sync_subtitle_tracks(&video, &video.subtitles).await?;
        let thumbnail_artifacts = sync_video_thumbnails(&store, &video).await?;
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
            "subtitle_snapshots_inserted": subtitle_snapshots_inserted,
            "thumbnail_artifacts": thumbnail_artifacts,
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

async fn video_subtitles_sync(args: VideoRefArg, config: &Config) -> Result<()> {
    let (store, _) = init_sqlite_database(&config.database.url).await?;
    let video = extract_video(&parse_video_input(&args.video)).await?;
    store.upsert_video_snapshot(&video).await?;
    let subtitle_snapshots_inserted = store.sync_subtitle_tracks(&video, &video.subtitles).await?;
    let artifact_paths = sync_subtitle_artifacts(&store, &video).await?;
    let history = store.load_subtitle_history(&video.video_id).await?;

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "status": "synced",
            "video_id": video.video_id,
            "title": video.title,
            "tracks": video.subtitles.len(),
            "subtitle_snapshots_inserted": subtitle_snapshots_inserted,
            "artifacts_written": artifact_paths.len(),
            "artifact_paths": artifact_paths,
            "history_counts": {
                "tracks": history.tracks.len(),
                "snapshots": history.snapshots.len(),
            },
        }))
        .map_err(|err| VesselError::Config(err.to_string()))?
    );
    Ok(())
}

async fn video_comments_sync(args: VideoRefArg, config: &Config) -> Result<()> {
    let (store, _) = init_sqlite_database(&config.database.url).await?;
    let input = parse_video_input(&args.video);
    let video = extract_video(&input).await?;
    store.upsert_video_snapshot(&video).await?;
    let comments = extract_comments(&input, 100).await?;
    let comment_snapshots_inserted = store.sync_comments(&comments).await?;
    let history = store.load_comment_history(&video.video_id).await?;

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "status": "synced",
            "video_id": video.video_id,
            "title": video.title,
            "comments_fetched": comments.len(),
            "comment_snapshots_inserted": comment_snapshots_inserted,
            "history_counts": {
                "comments": history.comments.len(),
                "snapshots": history.snapshots.len(),
            },
            "native_only": true,
        }))
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

    let selector = parse_format_selector(args.format.as_deref())?;
    let planner = BasicDownloadPlanner;
    let plan = match planner.plan(&video, selector.clone(), &config.download.output) {
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
        sync_subtitle_artifacts(&store, &video).await?
    } else {
        Vec::new()
    };

    let thumbnail_path = if args.embed_thumbnail {
        sync_primary_thumbnail_artifact(&store, &video).await?
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
            &final_output_path.to_string_lossy(),
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
    let mut generated_artifacts = Vec::new();
    for generated in postprocess_result.generated_artifacts {
        let generated_hash = hash_file(&generated.path).await?;
        let generated_size = tokio::fs::metadata(&generated.path).await?.len();
        let stored = store
            .insert_artifact(
                &video.video_id,
                &generated.kind,
                &generated.path.to_string_lossy(),
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

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "status": "downloaded",
            "video_id": video.video_id,
            "title": video.title,
            "format_ids": result.files.iter().map(|file| file.format_id.clone()).collect::<Vec<_>>(),
            "output_path": final_output_path,
            "bytes_written": byte_size,
            "resumed": result.files.iter().any(|file| file.resumed),
            "artifact_id": artifact.artifact_id,
            "content_hash": artifact.content_hash,
            "downloaded_files": result.files,
            "postprocess_plan": postprocess_plan,
            "generated_artifacts": generated_artifacts,
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
        if sync_channel_video(store, video_ref, args).await? {
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
    args: &ChannelSyncArgs,
) -> Result<bool> {
    let input = InputRef {
        raw: video_ref.video_id,
        kind: InputKind::VideoId,
    };
    let video = extract_video(&input).await?;
    let snapshot_inserted = store.upsert_video_snapshot(&video).await?;
    if args.subtitles {
        store.sync_subtitle_tracks(&video, &video.subtitles).await?;
        sync_subtitle_artifacts(store, &video).await?;
    }
    if args.download_thumbnails {
        sync_video_thumbnails(store, &video).await?;
    }
    if args.comments {
        let comments = extract_comments(&input, 40).await?;
        store.sync_comments(&comments).await?;
    }
    Ok(snapshot_inserted)
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
    Ok(hash_bytes(&bytes))
}

fn hash_bytes(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
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
        let path = PathBuf::from("thumbnails")
            .join(&video.video_id)
            .join(format!("{label}.{ext}"));
        write_bytes(&path, &bytes).await?;
        store
            .insert_artifact(
                &video.video_id,
                "thumbnail",
                &path.to_string_lossy(),
                &hash_bytes(&bytes),
                bytes.len() as u64,
                None,
            )
            .await?;
        paths.push(path.to_string_lossy().into_owned());
    }
    Ok(paths)
}

async fn sync_primary_thumbnail_artifact(
    store: &vessel_store::SqliteStore,
    video: &VideoMetadata,
) -> Result<Option<PathBuf>> {
    let paths = sync_video_thumbnails(store, video).await?;
    Ok(paths.last().map(PathBuf::from))
}

async fn sync_subtitle_artifacts(
    store: &vessel_store::SqliteStore,
    video: &VideoMetadata,
) -> Result<Vec<String>> {
    let client = http_client()?;
    let mut paths = Vec::new();
    for track in &video.subtitles {
        let Some(url) = track.url.as_deref() else {
            continue;
        };
        let bytes = fetch_binary(&client, url).await?;
        let suffix = if track.is_auto_generated { ".auto" } else { "" };
        let path = PathBuf::from("subtitles")
            .join(&video.video_id)
            .join(format!(
                "{}{}.vtt",
                sanitize_component(&track.language),
                suffix
            ));
        write_bytes(&path, &bytes).await?;
        store
            .insert_artifact(
                &video.video_id,
                "subtitle",
                &path.to_string_lossy(),
                &hash_bytes(&bytes),
                bytes.len() as u64,
                None,
            )
            .await?;
        paths.push(path.to_string_lossy().into_owned());
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
