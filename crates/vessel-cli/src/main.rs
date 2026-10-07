use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use clap::{ArgAction, Args, Parser, Subcommand};
use time::OffsetDateTime;
use tracing::{debug, info, warn};
use vessel_asr::{AsrConfig, LoadedAsrBackend};
use vessel_core::models::{ChannelMetadata, InputKind, InputRef, VideoMetadata};
use vessel_core::{
    Config, MaterializeStatus, Result, TranscriptCandidate, VesselError, VideoSelection,
    apply_sourcearium_prune, discover_youtube_sources, inventory_sourcearium_repository,
    load_config, load_youtube_transcript_artifact, materialize_youtube_transcript,
    plan_sourcearium_prune, validate_sourcearium_repository,
};
use vessel_extractors::youtube::{
    ChannelVideoCrawlReport, ChannelVideoRef, YT_DLP_BACKEND_NAME, YtDlpConfig,
    acquire_best_caption_candidate,
};
use vessel_store::init_sqlite_database_path;

#[derive(Debug, Parser)]
#[command(
    name = "vessel",
    version,
    about = "Sourcearium media-text acquisition orchestrator"
)]
struct Cli {
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
    #[arg(long = "source-key")]
    source_keys: Vec<String>,
    #[arg(long = "video-id")]
    video_ids: Vec<String>,
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
    #[arg(long = "diarization-model")]
    diarization_model: Option<String>,
    #[arg(long = "min-speakers")]
    min_speakers: Option<usize>,
    #[arg(long = "max-speakers")]
    max_speakers: Option<usize>,
    #[arg(long = "hf-token-env", default_value = "HF_TOKEN")]
    hf_token_env: String,
    #[arg(long = "upgrade-check-days", default_value_t = 30)]
    upgrade_check_days: u64,
    #[arg(long = "report-items")]
    report_items: bool,
    #[arg(long)]
    preview: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let (config, _, _) = load_config()?;
    init_logging(&config, &cli)?;
    debug!(target: "cli", "cli parsed");
    match cli.command {
        Commands::Doctor => doctor().await,
        Commands::Update(args) => sourcearium_update(args, &config).await,
        Commands::Validate(args) => sourcearium_validate(args),
        Commands::Inventory(args) => sourcearium_inventory(args),
        Commands::Prune(args) => sourcearium_prune(args),
    }
}

async fn sourcearium_update(args: UpdateArgs, config: &Config) -> Result<()> {
    let asr_config = resolve_asr_config(&args)?;
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

    let mut sources = discover_youtube_sources(&sourcearium_root)?;
    if !args.source_keys.is_empty() {
        let available = sources
            .iter()
            .map(|source| source.policy.source_key.clone())
            .collect::<HashSet<_>>();
        let missing = args
            .source_keys
            .iter()
            .filter(|source_key| !available.contains(*source_key))
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(VesselError::Corpus(format!(
                "unknown Sourcearium YouTube source key(s): {}",
                missing.join(", ")
            )));
        }
        sources.retain(|source| args.source_keys.contains(&source.policy.source_key));
    }
    let operational_db_path = sourcearium_root
        .join(".cache")
        .join("vessel")
        .join("vessel.sqlite");
    let (operational_store, _) = init_sqlite_database_path(&operational_db_path).await?;
    let mut asr_backend: Option<LoadedAsrBackend> = None;
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
            "caption_access_degraded": 0,
            "caption_empty_response_tracks": 0,
            "unresolved_no_provider": 0,
            "errors": [],
        });
        if report_items {
            summary["items"] = serde_json::json!([]);
        }
        summary["preview"] = serde_json::Value::Bool(args.preview);
        summary["youtube_backend"] = serde_json::Value::String(YT_DLP_BACKEND_NAME.to_owned());

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
        let (channel, crawl) = match discover_update_channel(&args, config, &channel_input).await {
            Ok(discovery) => discovery,
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
        summary["channel_discovery_backend"] =
            serde_json::Value::String(YT_DLP_BACKEND_NAME.to_owned());

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

        if membership_persisted {
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
            let video = match extract_update_video(&args, config, &video_input).await {
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

            let (candidate, asr_cache_dir) = if let Some(candidate) = caption_candidate {
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
                            "caption_probe": caption_probe,
                        }),
                    );
                    continue;
                }
                increment_summary(&mut summary, "local_asr_attempted", 1);
                let yt_dlp = update_yt_dlp_config(&args, config);
                match acquire_local_asr_candidate(
                    &sourcearium_root,
                    &video,
                    asr_backend.take(),
                    &asr_config,
                    &yt_dlp,
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
        "youtube_backend": YT_DLP_BACKEND_NAME,
        "diarization": {
            "enabled": args.diarize,
            "backend": "whisperx/pyannote",
            "model": asr_config.diarization_model,
            "scope": "local_asr",
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
) -> Result<(ChannelMetadata, ChannelVideoCrawlReport)> {
    let (channel, crawl) = update_yt_dlp_config(args, config)
        .discover_channel(input)
        .await?;
    Ok((channel, crawl))
}

async fn extract_update_video(
    args: &UpdateArgs,
    config: &Config,
    input: &InputRef,
) -> Result<VideoMetadata> {
    update_yt_dlp_config(args, config)
        .extract_video(input)
        .await
}

fn resolve_asr_config(args: &UpdateArgs) -> Result<AsrConfig> {
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
    config.diarize = args.diarize;
    if let Some(model) = args.diarization_model.as_deref() {
        config.diarization_model = model.to_owned();
    }
    config.min_speakers = config.diarize.then_some(args.min_speakers).flatten();
    config.max_speakers = config.diarize.then_some(args.max_speakers).flatten();
    config.hf_token_env = args.hf_token_env.clone();
    if config.backend != vessel_asr::WHISPERX_BACKEND_NAME
        && config.backend != vessel_asr::PHONON2_BACKEND_NAME
    {
        return Err(VesselError::Config(format!(
            "unsupported ASR backend {:?}; expected whisperx or phonon-2",
            config.backend
        )));
    }
    if config.diarize && config.backend != vessel_asr::WHISPERX_BACKEND_NAME {
        return Err(VesselError::Config(
            "--diarize requires the whisperx ASR backend".into(),
        ));
    }
    Ok(config)
}

async fn acquire_local_asr_candidate(
    sourcearium_root: &Path,
    video: &VideoMetadata,
    backend: Option<LoadedAsrBackend>,
    config: &AsrConfig,
    yt_dlp: &YtDlpConfig,
) -> Result<(TranscriptCandidate, PathBuf, LoadedAsrBackend)> {
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
    let cached_wav_valid = whisper_wav.is_file() && asr_audio_is_valid(&whisper_wav).await;
    if cached_wav_valid {
        info!(
            target: "asr",
            input = %whisper_wav.display(),
            "reusing verified cached normalized ASR audio"
        );
    } else {
        if whisper_wav.exists() {
            warn!(
                target: "asr",
                input = %whisper_wav.display(),
                "discarding invalid cached normalized ASR audio"
            );
            tokio::fs::remove_file(&whisper_wav).await?;
        }

        let output_template = cache_dir.join("source.%(ext)s");
        info!(
            target: "asr",
            video_id = %video.video_id,
            "acquiring ASR source audio through yt-dlp"
        );
        let source_audio = yt_dlp
            .download_best_audio(&video.video_id, &output_template)
            .await?;
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
    Ok((candidate, cache_dir, backend))
}

async fn asr_audio_is_valid(path: &Path) -> bool {
    let output = match tokio::process::Command::new("ffprobe")
        .arg("-v")
        .arg("error")
        .arg("-select_streams")
        .arg("a:0")
        .arg("-show_entries")
        .arg("stream=codec_name,sample_rate,channels")
        .arg("-of")
        .arg("default=noprint_wrappers=1")
        .arg(path)
        .output()
        .await
    {
        Ok(output) => output,
        Err(_) => return false,
    };

    if !output.status.success() {
        return false;
    }

    ffprobe_matches_asr_contract(&String::from_utf8_lossy(&output.stdout))
}

fn ffprobe_matches_asr_contract(output: &str) -> bool {
    let mut codec = None;
    let mut sample_rate = None;
    let mut channels = None;

    for line in output.lines() {
        if let Some(value) = line.strip_prefix("codec_name=") {
            codec = Some(value.trim());
        } else if let Some(value) = line.strip_prefix("sample_rate=") {
            sample_rate = Some(value.trim());
        } else if let Some(value) = line.strip_prefix("channels=") {
            channels = Some(value.trim());
        }
    }

    codec == Some("pcm_s16le") && sample_rate == Some("16000") && channels == Some("1")
}

async fn transcode_asr_audio(source: &Path, destination: &Path) -> Result<()> {
    let partial = destination.with_extension("wav.partial");
    if partial.exists() {
        tokio::fs::remove_file(&partial).await?;
    }

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
        .arg(&partial)
        .status()
        .await
        .map_err(|error| {
            VesselError::Extractor(format!("failed to start ffmpeg for ASR input: {error}"))
        })?;

    if !status.success() {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(VesselError::Extractor(format!(
            "ffmpeg failed to normalize ASR input {} -> {}",
            source.display(),
            destination.display()
        )));
    }

    if !asr_audio_is_valid(&partial).await {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(VesselError::Extractor(format!(
            "ffmpeg produced invalid normalized ASR audio {}",
            partial.display()
        )));
    }

    tokio::fs::rename(&partial, destination).await?;
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

fn increment_summary(summary: &mut serde_json::Value, key: &str, delta: usize) {
    let current = summary
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    summary[key] = serde_json::Value::from(current + delta as u64);
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

async fn doctor() -> Result<()> {
    let report = serde_json::json!({
        "architecture": "external-toolchain",
        "python_policy": {
            "manager": "uv",
            "runtime_installs": false,
            "source_builds": false,
            "offline_preparation_preferred": true,
        },
        "diarization_preflight": {
            "hf_auth_available": huggingface_auth_source().is_some(),
            "hf_auth_source": huggingface_auth_source(),
            "model": vessel_asr::DEFAULT_DIARIZATION_MODEL,
            "named_speaker_identity": false,
        },
        "binaries": {
            "yt_dlp": binary_available("yt-dlp", &["--version"]),
            "ffmpeg": binary_available("ffmpeg", &["-version"]),
            "ffprobe": binary_available("ffprobe", &["-version"]),
            "whisperx": binary_available("whisperx", &["--help"]),
            "uv": binary_available("uv", &["--version"]),
            "phonon_qa": binary_available("fermion", &["--version"]),
        },
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|error| VesselError::Config(error.to_string()))?
    );
    Ok(())
}

fn binary_available(binary: &str, args: &[&str]) -> bool {
    std::process::Command::new(binary)
        .args(args)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn huggingface_auth_source() -> Option<&'static str> {
    if std::env::var("HF_TOKEN")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
    {
        return Some("env");
    }

    if std::env::var_os("HF_TOKEN_PATH")
        .map(PathBuf::from)
        .is_some_and(|path| path.is_file())
    {
        return Some("token_file");
    }

    if std::env::var_os("HF_HOME")
        .map(PathBuf::from)
        .map(|path| path.join("token"))
        .is_some_and(|path| path.is_file())
    {
        return Some("token_file");
    }

    if std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|path| path.join(".cache").join("huggingface").join("token"))
        .is_some_and(|path| path.is_file())
    {
        return Some("token_file");
    }

    None
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffprobe_contract_requires_pcm_s16le_16khz_mono() {
        assert!(ffprobe_matches_asr_contract(
            "codec_name=pcm_s16le\nsample_rate=16000\nchannels=1\n"
        ));
        assert!(!ffprobe_matches_asr_contract(
            "codec_name=pcm_s16le\nsample_rate=48000\nchannels=1\n"
        ));
        assert!(!ffprobe_matches_asr_contract(
            "codec_name=aac\nsample_rate=16000\nchannels=1\n"
        ));
        assert!(!ffprobe_matches_asr_contract(""));
    }

    #[test]
    fn cli_is_reduced_to_corpus_maintenance_surface() {
        for removed in [
            "dataset",
            "channel",
            "video",
            "download",
            "formats",
            "plugin",
            "diarization",
            "info",
            "asr",
        ] {
            assert!(
                Cli::try_parse_from(["vessel", removed]).is_err(),
                "{removed} must stay retired"
            );
        }
        assert!(Cli::try_parse_from(["vessel", "update", "--youtube-backend", "native"]).is_err());
    }

    #[test]
    fn update_defaults_to_external_toolchain() {
        let cli = Cli::try_parse_from(["vessel", "update"]).expect("parse update");
        let Commands::Update(args) = cli.command else {
            panic!("expected update command")
        };
        assert_eq!(args.yt_dlp_executable, PathBuf::from("yt-dlp"));
        let asr = resolve_asr_config(&args).expect("default ASR config");
        assert_eq!(asr.backend, vessel_asr::WHISPERX_BACKEND_NAME);
        assert_eq!(asr.model, "large-v3");
    }

    #[test]
    fn diarization_requires_whisperx() {
        let cli =
            Cli::try_parse_from(["vessel", "update", "--asr-backend", "phonon-2", "--diarize"])
                .expect("parse");
        let Commands::Update(args) = cli.command else {
            panic!("expected update")
        };
        assert!(resolve_asr_config(&args).is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn materialize_then_repeat_is_idempotent_through_fake_toolchain() {
        use std::fs;
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::os::unix::fs::PermissionsExt;
        use std::time::{SystemTime, UNIX_EPOCH};

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "vessel-toolchain-acceptance-{}-{nonce}",
            std::process::id()
        ));
        let source_dir = root.join("sources/youtube/example");
        fs::create_dir_all(&source_dir).expect("source dir");
        fs::write(root.join("sourcearium.toml"), "").expect("sourcearium marker");
        fs::write(
            source_dir.join("source.toml"),
            r#"schema = 1
family = "youtube"
source_key = "example"

[channel]
input = "https://www.youtube.com/@example"
"#,
        )
        .expect("source policy");

        let listener = TcpListener::bind("127.0.0.1:0").expect("caption listener");
        let caption_addr = listener.local_addr().expect("caption address");
        let caption_server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("caption request");
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request).expect("read caption request");
            let body = r#"{"events":[{"tStartMs":0,"segs":[{"utf8":"Fixture transcript."}]}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("write caption response");
        });
        let caption_url = format!("http://{caption_addr}/timedtext?lang=en");

        let fake = root.join("fake-yt-dlp");
        let fake_script = r#"#!/bin/sh
last=""
for arg in "$@"; do last="$arg"; done
case "$last" in
  *watch?v=video123)
    cat <<'JSON'
{"id":"video123","channel_id":"UC_TEST","webpage_url":"https://www.youtube.com/watch?v=video123","title":"Fixture video","upload_date":"20261003","formats":[],"subtitles":{"en":[{"url":"__CAPTION_URL__","ext":"json3"}]},"automatic_captions":{},"thumbnails":[]}
JSON
    ;;
  *)
    cat <<'JSON'
{"id":"UC_TEST","channel_id":"UC_TEST","channel":"Example","uploader_id":"@example","channel_url":"https://www.youtube.com/channel/UC_TEST","entries":[{"id":"video123","ie_key":"Youtube","url":"https://www.youtube.com/watch?v=video123","title":"Fixture video","upload_date":"20261003"}]}
JSON
    ;;
esac
"#
        .replace("__CAPTION_URL__", &caption_url);
        fs::write(&fake, fake_script).expect("fake yt-dlp");
        let mut permissions = fs::metadata(&fake).expect("fake metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&fake, permissions).expect("make fake executable");

        let make_args = || UpdateArgs {
            sourcearium: root.clone(),
            max_videos: Some(1),
            source_keys: Vec::new(),
            video_ids: Vec::new(),
            yt_dlp_executable: fake.clone(),
            force_local_asr: false,
            asr_backend: None,
            asr_model: None,
            asr_model_dir: None,
            asr_executable: None,
            asr_device: None,
            asr_language: None,
            diarize: false,
            diarization_model: None,
            min_speakers: None,
            max_speakers: None,
            hf_token_env: "HF_TOKEN".into(),
            upgrade_check_days: 0,
            report_items: true,
            preview: false,
        };

        sourcearium_update(make_args(), &Config::default())
            .await
            .expect("materializing update");
        caption_server.join().expect("caption server");

        let transcript_path = source_dir
            .join("transcripts")
            .join("2026-10-03__video123__fixture-video.md");
        let first_bytes = fs::read(&transcript_path).expect("materialized transcript");
        let first_text = String::from_utf8(first_bytes.clone()).expect("transcript utf-8");
        assert!(first_text.contains("derivation = \"creator_subtitles\""));
        assert!(first_text.contains("[00:00:00] Fixture transcript."));

        let validation = validate_sourcearium_repository(&root).expect("validate materialized corpus");
        assert!(validation.valid, "{:?}", validation.errors);
        assert_eq!(validation.artifacts_validated, 1);

        let inventory = inventory_sourcearium_repository(&root).expect("inventory corpus");
        assert_eq!(inventory.artifacts, 1);
        assert_eq!(
            inventory.by_derivation.get("creator_subtitles"),
            Some(&1)
        );

        sourcearium_update(make_args(), &Config::default())
            .await
            .expect("repeat update");
        let second_bytes = fs::read(&transcript_path).expect("repeat transcript");
        assert_eq!(second_bytes, first_bytes, "repeat update must be a durable no-op");

        let inventory_after =
            inventory_sourcearium_repository(&root).expect("inventory repeated corpus");
        assert_eq!(inventory_after.artifacts, 1);
        assert_eq!(
            inventory_after.by_derivation.get("creator_subtitles"),
            Some(&1)
        );
        assert!(root.join(".cache/vessel/vessel.sqlite").is_file());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn publication_dates_normalize() {
        assert_eq!(
            normalize_update_publication_date(Some("20261003")),
            Some("2026-10-03".into())
        );
        assert_eq!(
            normalize_update_publication_date(Some("2026-10-03T12:00:00Z")),
            Some("2026-10-03".into())
        );
    }
}
