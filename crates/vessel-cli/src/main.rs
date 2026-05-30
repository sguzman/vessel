use std::path::{Path, PathBuf};
use std::process::Command;

use clap::{ArgAction, Args, Parser, Subcommand};
use reqwest::Client;
use time::OffsetDateTime;
use tracing::{debug, info, warn};
use vessel_core::models::{InputKind, InputRef, VideoMetadata};
use vessel_core::{Config, Result, RuntimeLayout, VesselError, load_config, resolve_runtime_layout};
use vessel_download::{BasicDownloadPlanner, DownloadPlanner, execute_download};
use vessel_extractors::youtube::{
    ChannelTabCursor, ChannelVideoRef, YoutubeExtractor, crawl_channel_videos, extract_channel,
    extract_comments, extract_video,
};
use vessel_extractors::{
    ExtractContext, ExtractRequest, ExtractedItem, ExtractorRegistry, PluginCatalog,
    load_plugins,
};
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
    Config(ConfigCommand),
    Dataset(DatasetCommand),
    Channel(ChannelCommand),
    Video(VideoCommand),
    Project(ProjectCommand),
    Info(UrlArg),
    Formats(UrlArg),
    Download(DownloadArgs),
    Plugin(PluginCommand),
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
}

#[derive(Debug, Args)]
struct ChannelAddArgs {
    channel: String,
}

#[derive(Debug, Args)]
struct ChannelSyncArgs {
    #[arg(long)]
    full: bool,
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
    let layout = resolve_runtime_layout(&config, &paths, cli.project.as_deref())?;
    init_logging(&config, &cli)?;
    debug!(target: "cli", project = %layout.project_name, "cli parsed");

    match cli.command {
        Commands::Doctor => doctor(&config, &paths, &loaded_from, &layout).await,
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
        Commands::Info(arg) => extract_preview(arg.url, InputKind::Url, &paths, &layout).await,
        Commands::Formats(arg) => formats(arg.url).await,
        Commands::Download(args) => download(args, &layout).await,
        Commands::Plugin(cmd) => match cmd.command {
            PluginSubcommand::List => plugin_list(&paths, &layout).await,
            PluginSubcommand::Install(args) => plugin_install(args, &paths, &layout).await,
        },
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

async fn dataset_init(args: DatasetInitArgs, layout: &RuntimeLayout) -> Result<()> {
    info!(target: "info", project = %layout.project_name, "dataset init started");
    ensure_project_layout(layout).await?;
    let target = args.db.unwrap_or_else(|| layout.database_url.clone());
    if args.recreate {
        vessel_logging::progress(
            "dataset",
            format!("recreating dataset db at {}", sqlite_target_path(&target)?.display()),
        );
        remove_sqlite_files(&target)?;
    }
    let (_store, paths) = init_sqlite_database(&target).await?;
    vessel_logging::progress(
        "dataset",
        format!("dataset initialized at {}", sqlite_target_path(&target)?.display()),
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
    let (store, _) = init_sqlite_database(&layout.database_url).await?;
    let input = parse_channel_input(&args.channel);
    debug!(target: "extractor", input = %input.raw, "resolving channel metadata");
    let channel = extract_channel(&input).await?;
    vessel_logging::progress(
        "channel",
        format!(
            "resolved channel {} ({})",
            channel.title.clone().unwrap_or_else(|| channel.channel_id.clone()),
            channel.channel_id
        ),
    );
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

async fn channel_sync(args: ChannelSyncArgs, layout: &RuntimeLayout) -> Result<()> {
    info!(target: "info", project = %layout.project_name, full = args.full, "channel sync started");
    ensure_project_layout(layout).await?;
    let (store, _) = init_sqlite_database(&layout.database_url).await?;
    let tracked_channels = store.list_tracked_channels().await?;
    vessel_logging::progress(
        "info",
        format!("syncing {} tracked channel{}", tracked_channels.len(), if tracked_channels.len() == 1 { "" } else { "s" }),
    );
    if tracked_channels.is_empty() {
        warn!(target: "warn", "channel sync has no tracked channels");
    }
    let run_id = store.start_run("channel sync").await?;
    let started_at = OffsetDateTime::now_utc();

    let sync_comments = args.full || args.comments;
    let sync_subtitles = args.full || args.subtitles;
    let sync_thumbnails = args.full || args.download_thumbnails;

    let result = async {
        let mut summary = serde_json::json!({
            "status": "synced",
            "run_id": run_id,
            "tracked_channels": tracked_channels.len(),
            "channels_processed": 0usize,
            "channel_history_inserted": 0usize,
            "videos_discovered": 0usize,
            "unique_videos_discovered": 0usize,
            "video_history_inserted": 0usize,
            "videos_refreshed": 0usize,
            "video_refreshes_skipped_by_since": 0usize,
            "tabs_visited": Vec::<String>::new(),
            "tabs_completed": Vec::<String>::new(),
            "tabs_resumed_from_checkpoint": Vec::<String>::new(),
            "videos_discovered_per_tab": serde_json::Map::<String, serde_json::Value>::new(),
            "errors": 0usize,
            "options": {
                "full": args.full,
                "comments": sync_comments,
                "subtitles": sync_subtitles,
                "download_thumbnails": sync_thumbnails,
                "since": args.since,
                "max_videos": args.max_videos,
            }
        });

        for tracked in tracked_channels {
            match sync_one_channel(&store, &tracked, &args, run_id, started_at, layout).await {
                Ok(channel_report) => {
                    increment_summary(&mut summary, "channels_processed", 1);
                    increment_summary(
                        &mut summary,
                        "channel_history_inserted",
                        usize::from(channel_report.channel_snapshot_inserted),
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
                        channel_report.video_snapshots_inserted,
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
    args: VideoRefArg,
    layout: &RuntimeLayout,
    paths: &vessel_core::ConfigPaths,
) -> Result<()> {
    info!(target: "info", video = %args.video, "video refresh started");
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
        let snapshot_inserted = store.upsert_video_snapshot(&video).await?;
        if snapshot_inserted {
            vessel_logging::progress("db", format!("video_history inserted for {}", video.video_id));
        } else {
            vessel_logging::progress("db", format!("video_history unchanged for {}", video.video_id));
        }
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
            "revision_inserted": snapshot_inserted,
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
        format!("artifact recorded for {} at {}", video.video_id, artifact.path),
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
    vessel_logging::progress("download", format!("download complete for {}", video.video_id));

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
    channel_snapshot_inserted: bool,
    videos_discovered: usize,
    unique_videos_discovered: usize,
    video_snapshots_inserted: usize,
    videos_refreshed: usize,
    skipped_by_since: usize,
    tabs_visited: Vec<String>,
    tabs_completed: Vec<String>,
    tabs_resumed_from_checkpoint: Vec<String>,
    videos_discovered_per_tab: std::collections::BTreeMap<String, usize>,
}

async fn sync_one_channel(
    store: &vessel_store::SqliteStore,
    tracked: &StoredTrackedChannel,
    args: &ChannelSyncArgs,
    run_id: uuid::Uuid,
    started_at: OffsetDateTime,
    layout: &RuntimeLayout,
) -> Result<ChannelSyncReport> {
    let input = parse_channel_input(&tracked.canonical_url);
    vessel_logging::progress(
        "channel",
        format!(
            "{}: refresh started",
            tracked
                .title
                .clone()
                .unwrap_or_else(|| tracked.channel_id.clone())
        ),
    );
    let mut channel = extract_channel(&input).await?;
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
            .record_channel_video_membership(&channel.channel_id, &video_ref.video_id, &video_ref.tab_name)
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
            report.video_snapshots_inserted += 1;
        }
        report.videos_refreshed += 1;
    }

    if channel.video_count.is_none() {
        channel.video_count = Some(discovered_count as u64);
    }
    if channel.view_count.is_none() {
        channel.view_count = store.aggregate_channel_video_view_count(&channel.channel_id).await?;
    }
    report.channel_snapshot_inserted = store.upsert_channel_snapshot(&channel).await?;
    if report.channel_snapshot_inserted {
        vessel_logging::progress("db", format!("channel_history inserted for {}", channel.channel_id));
    } else {
        vessel_logging::progress("db", format!("channel_history unchanged for {}", channel.channel_id));
    }
    vessel_logging::progress("db", format!("channel_metrics inserted for {}", channel.channel_id));
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
    let sync_comments = args.full || args.comments;
    let sync_subtitles = args.full || args.subtitles;
    let sync_thumbnails = args.full || args.download_thumbnails;
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
    let snapshot_inserted = store.upsert_video_snapshot(&video).await?;
    if snapshot_inserted {
        vessel_logging::progress("db", format!("video_history inserted for {}", video.video_id));
    } else {
        vessel_logging::progress("db", format!("video_history unchanged for {}", video.video_id));
    }
    vessel_logging::progress("db", format!("video_metrics inserted for {}", video.video_id));
    if sync_subtitles {
        let inserted = store.sync_subtitle_tracks(&video, &video.subtitles).await?;
        vessel_logging::progress(
            "db",
            format!("subtitle_history inserted {} track(s) for {}", inserted, video.video_id),
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
                Ok((store, _)) => store.list_tracked_channels().await.unwrap_or_default().len(),
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

fn default_project_plugin_dir(_paths: &vessel_core::ConfigPaths, layout: &RuntimeLayout) -> PathBuf {
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
        if !array.iter().any(|item| item.as_str() == Some(value.as_str())) {
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
        let current = object.get(tab).and_then(serde_json::Value::as_u64).unwrap_or(0);
        object.insert(tab.clone(), serde_json::Value::from(current + *count as u64));
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
