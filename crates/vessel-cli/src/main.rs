use std::path::Path;
use std::process::Command;

use clap::{Args, Parser, Subcommand};
use time::OffsetDateTime;
use vessel_core::models::{InputKind, InputRef};
use vessel_core::{Config, Result, VesselError, load_config};
use vessel_extractors::youtube::YoutubeExtractor;
use vessel_extractors::{ExtractContext, ExtractRequest, ExtractedItem, ExtractorRegistry};
use vessel_formats::FormatSelector;
use vessel_ledger::{AttemptStatus, FetchAttempt, Ledger};
use vessel_store::init_sqlite_database;

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
            ChannelSubcommand::Add(args) => stub_channel_add(args),
            ChannelSubcommand::Sync(args) => stub_channel_sync(args),
        },
        Commands::Video(cmd) => match cmd.command {
            VideoSubcommand::Refresh(args) => video_refresh(args, &config).await,
            VideoSubcommand::History(args) => video_history(args, &config).await,
        },
        Commands::Info(arg) => extract_preview(arg.url, InputKind::Url).await,
        Commands::Formats(arg) => stub_formats(arg.url),
        Commands::Download(args) => stub_download(args),
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

fn stub_channel_add(args: ChannelAddArgs) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "status": "stub",
            "command": "channel add",
            "channel": args.channel,
            "next": "Persist tracked channel identities in the ledger."
        }))
        .map_err(|err| VesselError::Config(err.to_string()))?
    );
    Ok(())
}

fn stub_channel_sync(args: ChannelSyncArgs) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "status": "stub",
            "command": "channel sync",
            "options": {
                "comments": args.comments,
                "subtitles": args.subtitles,
                "since": args.since,
                "max_videos": args.max_videos,
            },
            "next": "Implement channel discovery, staleness policy, and snapshot upserts."
        }))
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

fn stub_formats(url: String) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "status": "stub",
            "command": "formats",
            "url": url,
            "baseline_selector_ast": FormatSelector::Fallback(vec![
                FormatSelector::Merge(
                    Box::new(FormatSelector::BestVideo),
                    Box::new(FormatSelector::BestAudio),
                ),
                FormatSelector::Best,
            ])
        }))
        .map_err(|err| VesselError::Config(err.to_string()))?
    );
    Ok(())
}

fn stub_download(args: DownloadArgs) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "status": "stub",
            "command": "download",
            "url": args.url,
            "format": args.format.unwrap_or_else(|| "best".to_owned()),
            "next": "Wire extractor metadata into download planning and artifact/archive persistence."
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

fn binary_available(name: &str) -> bool {
    Command::new(name).arg("-version").output().is_ok()
}
