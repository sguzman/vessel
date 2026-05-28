use async_trait::async_trait;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Executor, Pool, Sqlite};
use time::OffsetDateTime;
use tracing::info;
use uuid::Uuid;

use vessel_core::models::{ChannelMetadata, VideoMetadata, canonical_json_hash};
use vessel_core::{Result, VesselError};
use vessel_ledger::{FetchAttempt, Ledger, RefreshDecision, SyncOptions};

use crate::migrations::MIGRATIONS;
use crate::traits::{CurrentStateStore, SnapshotStore};

#[derive(Debug, Clone)]
pub struct DatabasePaths {
    pub requested: String,
    pub sqlite_url: String,
}

#[derive(Clone)]
pub struct SqliteStore {
    pool: Pool<Sqlite>,
}

impl SqliteStore {
    pub fn new(pool: Pool<Sqlite>) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &Pool<Sqlite> {
        &self.pool
    }
}

pub async fn init_sqlite_database(target: &str) -> Result<(SqliteStore, DatabasePaths)> {
    let sqlite_url = normalize_sqlite_target(target);
    let options = sqlite_url
        .parse::<SqliteConnectOptions>()
        .map_err(|err| VesselError::Database(format!("invalid sqlite url: {err}")))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(|err| VesselError::Database(err.to_string()))?;
    apply_migrations(&pool).await?;
    info!(database = %sqlite_url, "sqlite initialized");
    Ok((
        SqliteStore::new(pool),
        DatabasePaths {
            requested: target.to_owned(),
            sqlite_url,
        },
    ))
}

async fn apply_migrations(pool: &Pool<Sqlite>) -> Result<()> {
    for (_, sql) in MIGRATIONS {
        pool.execute(*sql)
            .await
            .map_err(|err| VesselError::Database(err.to_string()))?;
    }
    Ok(())
}

fn normalize_sqlite_target(target: &str) -> String {
    if target.starts_with("sqlite:") {
        target.to_owned()
    } else {
        format!("sqlite://{target}")
    }
}

#[async_trait]
impl CurrentStateStore for SqliteStore {
    async fn put_channel_latest(
        &self,
        channel: &ChannelMetadata,
        content_hash: &str,
    ) -> Result<()> {
        let now = channel
            .fetched_at
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|err| VesselError::Database(err.to_string()))?;
        sqlx::query(
            r#"
INSERT INTO channels (
    id, platform, channel_id, handle, canonical_url, title, description, subscriber_count,
    video_count, view_count, avatar_url, banner_url, latest_snapshot_id, first_seen_at, last_seen_at, updated_at
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
ON CONFLICT(channel_id) DO UPDATE SET
    handle = excluded.handle,
    canonical_url = excluded.canonical_url,
    title = excluded.title,
    description = excluded.description,
    subscriber_count = excluded.subscriber_count,
    video_count = excluded.video_count,
    view_count = excluded.view_count,
    avatar_url = excluded.avatar_url,
    banner_url = excluded.banner_url,
    latest_snapshot_id = excluded.latest_snapshot_id,
    last_seen_at = excluded.last_seen_at,
    updated_at = excluded.updated_at
"#,
        )
        .bind(channel.channel_id.clone())
        .bind(format!("{:?}", channel.platform))
        .bind(channel.channel_id.clone())
        .bind(channel.handle.clone())
        .bind(channel.url.clone())
        .bind(channel.title.clone())
        .bind(channel.description.clone())
        .bind(channel.subscriber_count.map(|v| v as i64))
        .bind(channel.video_count.map(|v| v as i64))
        .bind(channel.view_count.map(|v| v as i64))
        .bind(channel.avatar_url.clone())
        .bind(channel.banner_url.clone())
        .bind(content_hash.to_owned())
        .bind(now.clone())
        .bind(now.clone())
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|err| VesselError::Database(err.to_string()))?;
        Ok(())
    }

    async fn put_video_latest(&self, video: &VideoMetadata, content_hash: &str) -> Result<()> {
        let now = video
            .fetched_at
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|err| VesselError::Database(err.to_string()))?;
        sqlx::query(
            r#"
INSERT INTO videos (
    id, platform, video_id, channel_id, canonical_url, title, description, upload_date, duration_seconds,
    view_count, like_count, comment_count, availability, latest_snapshot_id, first_seen_at, last_seen_at, updated_at
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
ON CONFLICT(video_id) DO UPDATE SET
    channel_id = excluded.channel_id,
    canonical_url = excluded.canonical_url,
    title = excluded.title,
    description = excluded.description,
    upload_date = excluded.upload_date,
    duration_seconds = excluded.duration_seconds,
    view_count = excluded.view_count,
    like_count = excluded.like_count,
    comment_count = excluded.comment_count,
    availability = excluded.availability,
    latest_snapshot_id = excluded.latest_snapshot_id,
    last_seen_at = excluded.last_seen_at,
    updated_at = excluded.updated_at
"#,
        )
        .bind(video.video_id.clone())
        .bind(format!("{:?}", video.platform))
        .bind(video.video_id.clone())
        .bind(video.channel_id.clone())
        .bind(video.url.clone())
        .bind(video.title.clone())
        .bind(video.description.clone())
        .bind(video.upload_date.clone())
        .bind(video.duration_seconds.map(|v| v as i64))
        .bind(video.view_count.map(|v| v as i64))
        .bind(video.like_count.map(|v| v as i64))
        .bind(video.comment_count.map(|v| v as i64))
        .bind(format!("{:?}", video.availability))
        .bind(content_hash.to_owned())
        .bind(now.clone())
        .bind(now.clone())
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|err| VesselError::Database(err.to_string()))?;
        Ok(())
    }
}

#[async_trait]
impl SnapshotStore for SqliteStore {
    async fn insert_channel_snapshot_if_changed(
        &self,
        channel: &ChannelMetadata,
        content_hash: &str,
    ) -> Result<bool> {
        let normalized_json =
            serde_json::to_string(channel).map_err(|err| VesselError::Database(err.to_string()))?;
        let raw_json = serde_json::to_string(&channel.raw)
            .map_err(|err| VesselError::Database(err.to_string()))?;
        let fetched_at = channel
            .fetched_at
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|err| VesselError::Database(err.to_string()))?;
        let result = sqlx::query(
            r#"
INSERT OR IGNORE INTO channel_snapshots (
    id, channel_id, fetched_at, content_hash, normalized_json, raw_json, changed_fields_json
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
"#,
        )
        .bind(Uuid::now_v7().to_string())
        .bind(channel.channel_id.clone())
        .bind(fetched_at)
        .bind(content_hash.to_owned())
        .bind(normalized_json)
        .bind(raw_json)
        .bind("{}")
        .execute(&self.pool)
        .await
        .map_err(|err| VesselError::Database(err.to_string()))?;
        Ok(result.rows_affected() > 0)
    }

    async fn insert_video_snapshot_if_changed(
        &self,
        video: &VideoMetadata,
        content_hash: &str,
    ) -> Result<bool> {
        let normalized_json =
            serde_json::to_string(video).map_err(|err| VesselError::Database(err.to_string()))?;
        let raw_json = serde_json::to_string(&video.raw)
            .map_err(|err| VesselError::Database(err.to_string()))?;
        let fetched_at = video
            .fetched_at
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|err| VesselError::Database(err.to_string()))?;
        let result = sqlx::query(
            r#"
INSERT OR IGNORE INTO video_snapshots (
    id, video_id, fetched_at, content_hash, normalized_json, raw_json, changed_fields_json
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
"#,
        )
        .bind(Uuid::now_v7().to_string())
        .bind(video.video_id.clone())
        .bind(fetched_at)
        .bind(content_hash.to_owned())
        .bind(normalized_json)
        .bind(raw_json)
        .bind("{}")
        .execute(&self.pool)
        .await
        .map_err(|err| VesselError::Database(err.to_string()))?;
        Ok(result.rows_affected() > 0)
    }

    async fn record_fetch_attempt(&self, attempt: FetchAttempt) -> Result<()> {
        let started_at = attempt
            .started_at
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|err| VesselError::Database(err.to_string()))?;
        let finished_at = attempt
            .finished_at
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|err| VesselError::Database(err.to_string()))?;
        sqlx::query(
            r#"
INSERT INTO fetch_attempts (
    id, run_id, target_kind, target_external_id, started_at, finished_at, status, error_message
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
"#,
        )
        .bind(Uuid::now_v7().to_string())
        .bind(attempt.run_id.to_string())
        .bind(attempt.target_kind)
        .bind(attempt.target_external_id)
        .bind(started_at)
        .bind(finished_at)
        .bind(format!("{:?}", attempt.status))
        .bind(attempt.error_message)
        .execute(&self.pool)
        .await
        .map_err(|err| VesselError::Database(err.to_string()))?;
        Ok(())
    }

    async fn should_refresh_video(
        &self,
        _video_id: &str,
        opts: &SyncOptions,
    ) -> Result<RefreshDecision> {
        Ok(RefreshDecision {
            should_fetch: opts.force,
            reason: if opts.force {
                "force requested".to_owned()
            } else {
                "bootstrap stub defaults to skip".to_owned()
            },
        })
    }
}

#[async_trait]
impl Ledger for SqliteStore {
    async fn start_run(&self, command: &str) -> Result<Uuid> {
        let run_id = Uuid::now_v7();
        let started_at = OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|err| VesselError::Database(err.to_string()))?;
        sqlx::query(
            "INSERT INTO fetch_runs (id, command, started_at, status) VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(run_id.to_string())
        .bind(command)
        .bind(started_at)
        .bind("running")
        .execute(&self.pool)
        .await
        .map_err(|err| VesselError::Database(err.to_string()))?;
        Ok(run_id)
    }

    async fn record_attempt(&self, attempt: FetchAttempt) -> Result<()> {
        <Self as SnapshotStore>::record_fetch_attempt(self, attempt).await
    }

    async fn upsert_channel_snapshot(&self, channel: &ChannelMetadata) -> Result<bool> {
        let hash =
            canonical_json_hash(channel).map_err(|err| VesselError::Database(err.to_string()))?;
        let inserted = self
            .insert_channel_snapshot_if_changed(channel, &hash)
            .await?;
        self.put_channel_latest(channel, &hash).await?;
        Ok(inserted)
    }

    async fn upsert_video_snapshot(&self, video: &VideoMetadata) -> Result<bool> {
        let hash =
            canonical_json_hash(video).map_err(|err| VesselError::Database(err.to_string()))?;
        let inserted = self.insert_video_snapshot_if_changed(video, &hash).await?;
        self.put_video_latest(video, &hash).await?;
        Ok(inserted)
    }

    async fn should_refresh_video(
        &self,
        video_id: &str,
        opts: &SyncOptions,
    ) -> Result<RefreshDecision> {
        <Self as SnapshotStore>::should_refresh_video(self, video_id, opts).await
    }

    async fn finish_run(&self, run_id: Uuid, ok: bool) -> Result<()> {
        let finished_at = OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|err| VesselError::Database(err.to_string()))?;
        sqlx::query("UPDATE fetch_runs SET finished_at = ?2, status = ?3 WHERE id = ?1")
            .bind(run_id.to_string())
            .bind(finished_at)
            .bind(if ok { "ok" } else { "failed" })
            .execute(&self.pool)
            .await
            .map_err(|err| VesselError::Database(err.to_string()))?;
        Ok(())
    }
}
