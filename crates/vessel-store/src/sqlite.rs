use async_trait::async_trait;
use serde::Serialize;
use sqlx::Row;
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

#[derive(Debug, Clone, Serialize)]
pub struct StoredVideoLatest {
    pub video_id: String,
    pub channel_id: Option<String>,
    pub canonical_url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub upload_date: Option<String>,
    pub duration_seconds: Option<i64>,
    pub view_count: Option<i64>,
    pub like_count: Option<i64>,
    pub comment_count: Option<i64>,
    pub availability: String,
    pub latest_snapshot_id: Option<String>,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoredVideoSnapshot {
    pub snapshot_id: String,
    pub video_id: String,
    pub fetched_at: String,
    pub content_hash: String,
    pub normalized_json: serde_json::Value,
    pub raw_json: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct VideoHistory {
    pub current: Option<StoredVideoLatest>,
    pub snapshots: Vec<StoredVideoSnapshot>,
    pub fetch_attempt_count: i64,
}

impl SqliteStore {
    pub fn new(pool: Pool<Sqlite>) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &Pool<Sqlite> {
        &self.pool
    }

    pub async fn load_video_history(&self, video_ref: &str) -> Result<VideoHistory> {
        let current = sqlx::query(
            r#"
SELECT
    video_id,
    channel_id,
    canonical_url,
    title,
    description,
    upload_date,
    duration_seconds,
    view_count,
    like_count,
    comment_count,
    availability,
    latest_snapshot_id,
    first_seen_at,
    last_seen_at,
    updated_at
FROM videos
WHERE video_id = ?1 OR canonical_url = ?1
LIMIT 1
"#,
        )
        .bind(video_ref)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| VesselError::Database(err.to_string()))?
        .map(|row| StoredVideoLatest {
            video_id: row.get("video_id"),
            channel_id: row.get("channel_id"),
            canonical_url: row.get("canonical_url"),
            title: row.get("title"),
            description: row.get("description"),
            upload_date: row.get("upload_date"),
            duration_seconds: row.get("duration_seconds"),
            view_count: row.get("view_count"),
            like_count: row.get("like_count"),
            comment_count: row.get("comment_count"),
            availability: row.get("availability"),
            latest_snapshot_id: row.get("latest_snapshot_id"),
            first_seen_at: row.get("first_seen_at"),
            last_seen_at: row.get("last_seen_at"),
            updated_at: row.get("updated_at"),
        });

        let lookup_id = current
            .as_ref()
            .map(|video| video.video_id.clone())
            .unwrap_or_else(|| video_ref.to_owned());

        let snapshot_rows = sqlx::query(
            r#"
SELECT
    id,
    video_id,
    fetched_at,
    content_hash,
    normalized_json,
    raw_json
FROM video_snapshots
WHERE video_id = ?1
ORDER BY fetched_at DESC
"#,
        )
        .bind(&lookup_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|err| VesselError::Database(err.to_string()))?;

        let snapshots = snapshot_rows
            .into_iter()
            .map(|row| {
                Ok(StoredVideoSnapshot {
                    snapshot_id: row.get("id"),
                    video_id: row.get("video_id"),
                    fetched_at: row.get("fetched_at"),
                    content_hash: row.get("content_hash"),
                    normalized_json: serde_json::from_str::<serde_json::Value>(
                        &row.get::<String, _>("normalized_json"),
                    )
                    .map_err(|err| VesselError::Database(err.to_string()))?,
                    raw_json: serde_json::from_str::<serde_json::Value>(
                        &row.get::<String, _>("raw_json"),
                    )
                    .map_err(|err| VesselError::Database(err.to_string()))?,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let fetch_attempt_count: i64 = sqlx::query_scalar(
            r#"
SELECT COUNT(*)
FROM fetch_attempts
WHERE target_external_id = ?1
"#,
        )
        .bind(&lookup_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|err| VesselError::Database(err.to_string()))?;

        Ok(VideoHistory {
            current,
            snapshots,
            fetch_attempt_count,
        })
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
        let hash = channel_snapshot_hash(channel)?;
        let inserted = self
            .insert_channel_snapshot_if_changed(channel, &hash)
            .await?;
        self.put_channel_latest(channel, &hash).await?;
        Ok(inserted)
    }

    async fn upsert_video_snapshot(&self, video: &VideoMetadata) -> Result<bool> {
        let hash = video_snapshot_hash(video)?;
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

fn channel_snapshot_hash(channel: &ChannelMetadata) -> Result<String> {
    canonical_json_hash(&serde_json::json!({
        "platform": &channel.platform,
        "channel_id": &channel.channel_id,
        "handle": &channel.handle,
        "url": &channel.url,
        "title": &channel.title,
        "description": &channel.description,
        "subscriber_count": channel.subscriber_count,
        "video_count": channel.video_count,
        "view_count": channel.view_count,
        "avatar_url": &channel.avatar_url,
        "banner_url": &channel.banner_url,
    }))
    .map_err(|err| VesselError::Database(err.to_string()))
}

fn video_snapshot_hash(video: &VideoMetadata) -> Result<String> {
    canonical_json_hash(&serde_json::json!({
        "platform": &video.platform,
        "video_id": &video.video_id,
        "channel_id": &video.channel_id,
        "url": &video.url,
        "title": &video.title,
        "description": &video.description,
        "duration_seconds": video.duration_seconds,
        "upload_date": &video.upload_date,
        "release_timestamp": video.release_timestamp.map(|ts| ts.unix_timestamp_nanos()),
        "view_count": video.view_count,
        "like_count": video.like_count,
        "comment_count": video.comment_count,
        "availability": &video.availability,
        "formats": &video.formats,
        "subtitles": &video.subtitles,
        "thumbnails": &video.thumbnails,
    }))
    .map_err(|err| VesselError::Database(err.to_string()))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use time::OffsetDateTime;
    use uuid::Uuid;
    use vessel_core::models::{Availability, Platform, VideoMetadata};
    use vessel_ledger::Ledger;

    use super::init_sqlite_database;

    fn temp_db_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("vessel-{name}-{}.sqlite", Uuid::now_v7()))
    }

    fn sample_video(fetched_at: OffsetDateTime) -> VideoMetadata {
        VideoMetadata {
            platform: Platform::YouTube,
            video_id: "video-123".to_owned(),
            channel_id: Some("channel-456".to_owned()),
            url: "https://www.youtube.com/watch?v=video-123".to_owned(),
            title: Some("Sample Title".to_owned()),
            description: Some("Sample Description".to_owned()),
            duration_seconds: Some(42),
            upload_date: Some("2024-01-01".to_owned()),
            release_timestamp: None,
            view_count: Some(100),
            like_count: None,
            comment_count: None,
            availability: Availability::Public,
            formats: Vec::new(),
            subtitles: Vec::new(),
            thumbnails: Vec::new(),
            fetched_at,
            raw: serde_json::json!({ "sample": true }),
        }
    }

    #[tokio::test]
    async fn idempotent_video_snapshot_upsert_and_history() {
        let path = temp_db_path("history");
        let path_str = path.to_string_lossy().into_owned();
        let (store, _) = init_sqlite_database(&path_str).await.expect("db init");
        let fetched_at = OffsetDateTime::now_utc();

        let first = store
            .upsert_video_snapshot(&sample_video(fetched_at))
            .await
            .expect("first upsert");
        let second = store
            .upsert_video_snapshot(&sample_video(fetched_at + time::Duration::minutes(1)))
            .await
            .expect("second upsert");
        let history = store
            .load_video_history("video-123")
            .await
            .expect("history");

        assert!(first);
        assert!(!second);
        assert_eq!(history.snapshots.len(), 1);
        assert!(history.current.is_some());

        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = fs::remove_file(path.with_extension("sqlite-shm"));
    }
}
