use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::{ArgAction, Args, Parser, Subcommand};
use reqwest::Client;
use time::OffsetDateTime;
use tracing::{debug, info, warn};
use vessel_asr::{AsrConfig, WhisperCandleBackend};
use vessel_core::models::{InputKind, InputRef, VideoMetadata};
use vessel_core::{
    ChannelCategoryConfig, Config, MaterializeStatus, Result, RuntimeLayout, TranscriptCandidate,
    VesselError, VideoSelection, discover_youtube_sources, load_config,
    load_youtube_transcript_artifact, materialize_youtube_transcript, resolve_runtime_layout,
    validate_sourcearium_repository,
};
use vessel_download::{BasicDownloadPlanner, DownloadPlanner, execute_download};
use vessel_extractors::youtube::{
    ChannelTabCursor, ChannelVideoRef, YoutubeExtractor, acquire_best_caption_candidate,
    crawl_channel_videos, extract_channel, extract_comments, extract_video,
};
use vessel_extractors::{
    ExtractContext, ExtractRequest, ExtractedItem, ExtractorRegistry, PluginCatalog,
    load_plugins,
};
use vessel_formats::{FormatSelector, parse_selector};
use vessel_ledger::{AttemptStatus, FetchAttempt, Ledger};
use vessel_postprocess::{PostprocessRequest, build_plan, execute_plan};
use vessel_store::{init_sqlite_database, init_sqlite_database_path};

#[derive(Debug, Parser)]
#[command(
    name = "vessel",
    version,
    about = "Rust-native media acquisition and Sourcearium text materialization"
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
    #[arg(long = "asr-model")]
    asr_model: Option<String>,
    #[arg(long = "asr-device")]
    asr_device: Option<String>,
    #[arg(long = "asr-language")]
    asr_language: Option<String>,
    #[arg(long = "upgrade-check-days", default_value_t = 30)]
    upgrade_check_days: u64,
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
        Commands::Update(args) => sourcearium_update(args).await,
        Commands::Validate(args) => sourcearium_validate(args),
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
        Commands::Info(arg) => extract_preview(arg.url, InputKind::Url, &paths, &layout).await,
        Commands::Formats(arg) => formats(arg.url).await,
        Commands::Download(args) => download(args, &layout).await,
        Commands::Plugin(cmd) => match cmd.command {
            PluginSubcommand::List => plugin_list(&paths, &layout).await,
            PluginSubcommand::Install(args) => plugin_install(args, &paths, &layout).await,
        },
    }
}

async fn sourcearium_update(args: UpdateArgs) -> Result<()> {
    let asr_config = resolve_asr_config(&args);
    let sourcearium_root = if args.sourcearium.is_absolute() {
        args.sourcearium
    } else {
        std::env::current_dir()?.join(args.sourcearium)
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
    let mut asr_backend: Option<WhisperCandleBackend> = None;
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
            "local_asr_materialized": 0,
            "unresolved_no_provider": 0,
            "errors": [],
        });

        if !policy.transcripts.enabled {
            summary["status"] = serde_json::Value::String("transcripts_disabled".into());
            source_reports.push(summary);
            continue;
        }

        let channel_input = parse_sourcearium_channel_input(&policy.channel.input)?;
        let channel = match extract_channel(&channel_input).await {
            Ok(channel) => channel,
            Err(error) => {
                summary["status"] = serde_json::Value::String("channel_resolution_failed".into());
                summary["errors"]
                    .as_array_mut()
                    .expect("errors array")
                    .push(serde_json::json!({"message": error.to_string()}));
                source_reports.push(summary);
                continue;
            }
        };

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

        let existing_cursors = match operational_store
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
                summary["status"] = serde_json::Value::String("operational_state_failed".into());
                summary["errors"]
                    .as_array_mut()
                    .expect("errors array")
                    .push(serde_json::json!({"message": error.to_string()}));
                source_reports.push(summary);
                continue;
            }
        };
        summary["existing_tab_cursors"] =
            serde_json::Value::from(existing_cursors.len() as u64);

        let crawl = match crawl_channel_videos(&channel_input, &existing_cursors).await {
            Ok(crawl) => crawl,
            Err(error) => {
                summary["status"] = serde_json::Value::String("channel_crawl_failed".into());
                summary["errors"]
                    .as_array_mut()
                    .expect("errors array")
                    .push(serde_json::json!({"message": error.to_string()}));
                source_reports.push(summary);
                continue;
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
        summary["operational_membership_persisted"] =
            serde_json::Value::Bool(membership_persisted);

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
                    continue;
                }
                VideoSelection::BeforeCutoff => {
                    increment_summary(&mut summary, "outside_date_policy", 1);
                    continue;
                }
                VideoSelection::Included
                | VideoSelection::ExplicitlyIncluded
                | VideoSelection::PublicationDateUnresolved => {}
            }

            if let Some(existing_artifact) = existing.as_ref() {
                match existing_artifact.artifact.representation.derivation.as_str() {
                    "creator_subtitles" => {
                        increment_summary(&mut summary, "already_strongest", 1);
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
                        if !transcript_upgrade_probe_due(
                            last_probe.as_deref(),
                            args.upgrade_check_days,
                            OffsetDateTime::now_utc(),
                        ) {
                            increment_summary(&mut summary, "upgrade_check_deferred", 1);
                            continue;
                        }
                    }
                    _ => {
                        increment_summary(&mut summary, "existing_unmanaged", 1);
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

            let video = match extract_video(&InputRef {
                raw: video_ref.video_id.clone(),
                kind: InputKind::VideoId,
            })
            .await
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
            let selection = match policy.select_video(&video.video_id, publication_date.as_deref()) {
                Ok(selection) => selection,
                Err(error) => {
                    push_update_error(&mut summary, &video.video_id, error);
                    continue;
                }
            };

            match selection {
                VideoSelection::ExplicitlyExcluded => {
                    increment_summary(&mut summary, "explicitly_excluded", 1);
                    continue;
                }
                VideoSelection::BeforeCutoff => {
                    increment_summary(&mut summary, "outside_date_policy", 1);
                    continue;
                }
                VideoSelection::PublicationDateUnresolved => {
                    increment_summary(&mut summary, "unresolved_date", 1);
                    continue;
                }
                VideoSelection::Included | VideoSelection::ExplicitlyIncluded => {}
            }

            let candidate = match acquire_best_caption_candidate(&video, &policy.transcripts).await {
                Ok(candidate) => candidate,
                Err(error) => {
                    push_update_error(&mut summary, &video.video_id, error);
                    continue;
                }
            };

            let (candidate, asr_cache_dir) = if let Some(candidate) = candidate {
                (candidate, None)
            } else if existing.is_some() {
                increment_summary(&mut summary, "preserved_without_better_caption", 1);
                match operational_store
                    .mark_transcript_probed(&video.video_id)
                    .await
                {
                    Ok(()) => increment_summary(&mut summary, "upgrade_checks_completed", 1),
                    Err(error) => push_update_error(&mut summary, &video.video_id, error),
                }
                continue;
            } else if policy.transcripts.allow_local_asr {
                increment_summary(&mut summary, "requires_local_asr", 1);
                increment_summary(&mut summary, "local_asr_attempted", 1);
                match acquire_local_asr_candidate(
                    &sourcearium_root,
                    &video,
                    asr_backend.take(),
                    &asr_config,
                )
                .await
                {
                    Ok((candidate, cache_dir, backend)) => {
                        asr_backend = Some(backend);
                        (candidate, Some(cache_dir))
                    }
                    Err(error) => {
                        push_update_error(&mut summary, &video.video_id, error);
                        continue;
                    }
                }
            } else {
                increment_summary(&mut summary, "unresolved_no_provider", 1);
                continue;
            };

            match materialize_youtube_transcript(
                &sourcearium_root,
                &source,
                &video,
                Some(&channel),
                &candidate,
            ) {
                Ok(result) => {
                    match result.status {
                        MaterializeStatus::Created => increment_summary(&mut summary, "created", 1),
                        MaterializeStatus::Updated => increment_summary(&mut summary, "updated", 1),
                        MaterializeStatus::Unchanged => {
                            increment_summary(&mut summary, "unchanged", 1)
                        }
                        MaterializeStatus::PreservedStronger => {
                            increment_summary(&mut summary, "preserved_stronger", 1)
                        }
                        MaterializeStatus::PreservedUnknownDerivation => {
                            increment_summary(&mut summary, "preserved_unknown_derivation", 1)
                        }
                    }

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
        summary["status"] = serde_json::Value::String(
            if source_has_errors { "partial" } else { "ok" }.into(),
        );
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

    let report = serde_json::json!({
        "status": report_status,
        "error_count": total_errors,
        "sourcearium_root": sourcearium_root,
        "operational_state": {
            "sqlite": operational_db_path,
        },
        "sources": source_reports,
        "remote_videos_processed": remote_videos_processed,
        "max_videos": args.max_videos,
        "upgrade_check_days": args.upgrade_check_days,
        "limit_reached": limit_reached,
        "local_asr_implemented": true,
        "asr": {
            "engine": vessel_asr::ENGINE_NAME,
            "model": asr_config.model,
            "device": asr_config.device,
            "language": asr_config.language,
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
    elapsed_seconds >= 0
        && (elapsed_seconds as u64) >= interval_days.saturating_mul(86_400)
}

fn resolve_asr_config(args: &UpdateArgs) -> AsrConfig {
    let mut config = AsrConfig::default();
    if let Some(model) = args.asr_model.as_deref() {
        config.model = model.to_owned();
    }
    if let Some(device) = args.asr_device.as_deref() {
        config.device = device.to_owned();
    }
    if let Some(language) = args.asr_language.as_deref() {
        config.language = Some(language.to_owned());
    }
    config
}

async fn acquire_local_asr_candidate(
    sourcearium_root: &Path,
    video: &VideoMetadata,
    backend: Option<WhisperCandleBackend>,
    config: &AsrConfig,
) -> Result<(TranscriptCandidate, PathBuf, WhisperCandleBackend)> {
    let cache_dir = sourcearium_root
        .join(".cache")
        .join("vessel")
        .join("asr")
        .join(&video.video_id);
    tokio::fs::create_dir_all(&cache_dir).await?;

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

    let whisper_wav = cache_dir.join("whisper-input.wav");
    if !whisper_wav.is_file() {
        transcode_asr_audio(&source_audio, &whisper_wav).await?;
    }

    let config = config.clone();
    let wav_for_worker = whisper_wav.clone();
    let (candidate, backend) = tokio::task::spawn_blocking(move || {
        let mut backend = match backend {
            Some(backend) => backend,
            None => WhisperCandleBackend::load(config)?,
        };
        let candidate = backend.transcribe_path(&wav_for_worker)?;
        Ok::<_, VesselError>((candidate, backend))
    })
    .await
    .map_err(|error| {
        VesselError::Extractor(format!("local ASR worker failed to join: {error}"))
    })??;

    Ok((candidate, cache_dir, backend))
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

fn push_update_error(summary: &mut serde_json::Value, video_id: &str, error: VesselError) {
    summary["errors"]
        .as_array_mut()
        .expect("errors array")
        .push(serde_json::json!({
            "video_id": video_id,
            "message": error.to_string(),
        }));
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

async fn channel_config_init(layout: &RuntimeLayout) -> Result<()> {
    info!(target: "info", project = %layout.project_name, "channel config init started");
    ensure_project_layout(layout).await?;
    let path = project_channel_registry_path(layout);
    let existed = path.exists();
    if !existed {
        let mut config = Config::default();
        config.dataset.project = Some(layout.project_name.clone());
        config.channels.categories.insert(
            "default".to_owned(),
            ChannelCategoryConfig::default(),
        );
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
    let category = args.category.ok_or_else(|| {
        VesselError::Config("channel add requires --category <name>".to_owned())
    })?;
    let category = category.trim().to_owned();
    if category.is_empty() {
        return Err(VesselError::Config(
            "channel add requires a non-empty category".to_owned(),
        ));
    }
    let normalized_channel = normalize_channel_entry(&args.channel)?;
    let mut registry = load_project_channel_registry(layout)?;
    for (existing_category, spec) in &registry.channels.categories {
        if spec.channels.iter().any(|channel| channel.trim() == normalized_channel) {
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
            channel.title.clone().unwrap_or_else(|| channel.channel_id.clone()),
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
        registry.channels.categories.keys().cloned().collect::<Vec<_>>()
    } else {
        args.categories.clone()
    };
    vessel_logging::progress(
        "info",
        format!(
            "syncing {} configured channel{} across {} categor{}",
            configured_targets.len(),
            if configured_targets.len() == 1 { "" } else { "s" },
            selected_categories.len(),
            if selected_categories.len() == 1 { "y" } else { "ies" }
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
        format!(
            "[{}] {}: refresh started",
            tracked.category,
            tracked.input
        ),
    );
    let mut channel = extract_channel(&input).await?;
    store.add_tracked_channel(&channel, &tracked.category).await?;
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
            report.video_history_inserted += 1;
        }
        report.video_metrics_inserted += 1;
        report.videos_refreshed += 1;
    }

    if channel.video_count.is_none() {
        channel.video_count = Some(discovered_count as u64);
    }
    if channel.view_count.is_none() {
        channel.view_count = store.aggregate_channel_video_view_count(&channel.channel_id).await?;
    }
    report.channel_metrics_inserted += 1;
    report.channel_history_inserted = if args.metrics_only {
        store.record_channel_metrics_only(&channel).await?;
        vessel_logging::progress(
            "db",
            format!("channel_history skipped for {} (metrics-only)", channel.channel_id),
        );
        false
    } else {
        let inserted = store.upsert_channel_snapshot(&channel).await?;
        if inserted {
            vessel_logging::progress("db", format!("channel_history inserted for {}", channel.channel_id));
        } else {
            vessel_logging::progress("db", format!("channel_history unchanged for {}", channel.channel_id));
        }
        inserted
    };
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use time::OffsetDateTime;

    use super::{
        UpdateArgs, normalize_update_publication_date, parse_sourcearium_channel_input,
        resolve_asr_config, resolve_configured_channels, transcript_upgrade_probe_due,
    };
    use vessel_core::models::InputKind;
    use vessel_core::{ChannelCategoryConfig, Config};

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

        let gaming =
            resolve_configured_channels(&config, &["gaming".to_owned()]).expect("gaming");
        assert_eq!(gaming.len(), 1);
        assert_eq!(gaming[0].category, "gaming");
    }

    #[test]
    fn asr_cli_overrides_are_operational_only() {
        let args = UpdateArgs {
            sourcearium: PathBuf::from("."),
            max_videos: None,
            asr_model: Some("base".into()),
            asr_device: Some("cpu".into()),
            asr_language: Some("es".into()),
            upgrade_check_days: 30,
        };
        let config = resolve_asr_config(&args);
        assert_eq!(config.model, "base");
        assert_eq!(config.device, "cpu");
        assert_eq!(config.language.as_deref(), Some("es"));
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
