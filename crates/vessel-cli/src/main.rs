use std::path::Path;
use std::process::Command;

use clap::{Args, Parser, Subcommand};
use vessel_core::models::{InputKind, InputRef};
use vessel_core::{Config, Result, VesselError, load_config};
use vessel_extractors::youtube::YoutubeExtractor;
use vessel_extractors::{ExtractContext, ExtractRequest, ExtractorRegistry};
use vessel_formats::FormatSelector;
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
            VideoSubcommand::Refresh(args) => stub_video_refresh(args),
            VideoSubcommand::History(args) => stub_video_history(args),
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

fn stub_video_refresh(args: VideoRefArg) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "status": "stub",
            "command": "video refresh",
            "video": args.video
        }))
        .map_err(|err| VesselError::Config(err.to_string()))?
    );
    Ok(())
}

fn stub_video_history(args: VideoRefArg) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "status": "stub",
            "command": "video history",
            "video": args.video,
            "next": "Query video_snapshots and render diffs."
        }))
        .map_err(|err| VesselError::Config(err.to_string()))?
    );
    Ok(())
}

async fn extract_preview(url: String, kind: InputKind) -> Result<()> {
    let mut registry = ExtractorRegistry::default();
    registry.register(YoutubeExtractor);
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

fn binary_available(name: &str) -> bool {
    Command::new(name).arg("-version").output().is_ok()
}
