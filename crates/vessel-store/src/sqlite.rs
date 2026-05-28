use async_trait::async_trait;
use serde::Serialize;
use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Executor, Pool, Sqlite};
use std::path::Path;
use time::OffsetDateTime;
use tracing::info;
use uuid::Uuid;

use vessel_core::models::{
    ChannelMetadata, CommentMetadata, SubtitleTrack, VideoMetadata, canonical_json_hash,
};
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
pub struct StoredTrackedChannel {
    pub channel_id: String,
    pub canonical_url: String,
    pub handle: Option<String>,
    pub title: Option<String>,
    pub added_at: String,
    pub last_sync_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArtifactRecord {
    pub artifact_id: String,
    pub video_id: String,
    pub artifact_kind: String,
    pub path: String,
    pub content_hash: String,
    pub byte_size: i64,
    pub format_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoredVideoSnapshot {
    pub snapshot_id: String,
    pub video_id: String,
    pub fetched_at: String,
    pub content_hash: String,
    pub changed_fields: Vec<String>,
    pub normalized_json: serde_json::Value,
    pub raw_json: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct VideoHistory {
    pub current: Option<StoredVideoLatest>,
    pub snapshots: Vec<StoredVideoSnapshot>,
    pub fetch_attempt_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoredSubtitleTrack {
    pub language: String,
    pub url: Option<String>,
    pub is_auto_generated: bool,
    pub latest_snapshot_id: Option<String>,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoredSubtitleSnapshot {
    pub snapshot_id: String,
    pub language: String,
    pub is_auto_generated: bool,
    pub fetched_at: String,
    pub content_hash: String,
    pub changed_fields: Vec<String>,
    pub normalized_json: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct SubtitleHistory {
    pub tracks: Vec<StoredSubtitleTrack>,
    pub snapshots: Vec<StoredSubtitleSnapshot>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoredComment {
    pub comment_id: String,
    pub video_id: String,
    pub author_channel_id: Option<String>,
    pub author_name: Option<String>,
    pub text: String,
    pub like_count: Option<i64>,
    pub reply_count: Option<i64>,
    pub published_at: Option<String>,
    pub updated_at: String,
    pub latest_snapshot_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoredCommentSnapshot {
    pub snapshot_id: String,
    pub comment_id: String,
    pub fetched_at: String,
    pub content_hash: String,
    pub changed_fields: Vec<String>,
    pub normalized_json: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommentHistory {
    pub comments: Vec<StoredComment>,
    pub snapshots: Vec<StoredCommentSnapshot>,
}

impl SqliteStore {
    pub fn new(pool: Pool<Sqlite>) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &Pool<Sqlite> {
        &self.pool
    }

    pub async fn add_tracked_channel(&self, channel: &ChannelMetadata) -> Result<()> {
        let added_at = channel
            .fetched_at
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|err| VesselError::Database(err.to_string()))?;
        sqlx::query(
            r#"
INSERT INTO tracked_channels (
    channel_id, canonical_url, handle, title, added_at, last_sync_at
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6)
ON CONFLICT(channel_id) DO UPDATE SET
    canonical_url = excluded.canonical_url,
    handle = excluded.handle,
    title = excluded.title
"#,
        )
        .bind(channel.channel_id.clone())
        .bind(channel.url.clone())
        .bind(channel.handle.clone())
        .bind(channel.title.clone())
        .bind(added_at)
        .bind(None::<String>)
        .execute(&self.pool)
        .await
        .map_err(|err| VesselError::Database(err.to_string()))?;
        Ok(())
    }

    pub async fn list_tracked_channels(&self) -> Result<Vec<StoredTrackedChannel>> {
        let rows = sqlx::query(
            r#"
SELECT channel_id, canonical_url, handle, title, added_at, last_sync_at
FROM tracked_channels
ORDER BY added_at ASC
"#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|err| VesselError::Database(err.to_string()))?;

        Ok(rows
            .into_iter()
            .map(|row| StoredTrackedChannel {
                channel_id: row.get("channel_id"),
                canonical_url: row.get("canonical_url"),
                handle: row.get("handle"),
                title: row.get("title"),
                added_at: row.get("added_at"),
                last_sync_at: row.get("last_sync_at"),
            })
            .collect())
    }

    pub async fn mark_tracked_channel_synced(&self, channel_id: &str) -> Result<()> {
        let synced_at = OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|err| VesselError::Database(err.to_string()))?;
        sqlx::query("UPDATE tracked_channels SET last_sync_at = ?2 WHERE channel_id = ?1")
            .bind(channel_id)
            .bind(synced_at)
            .execute(&self.pool)
            .await
            .map_err(|err| VesselError::Database(err.to_string()))?;
        Ok(())
    }

    pub async fn is_video_archived(&self, platform: &str, external_id: &str) -> Result<bool> {
        let archive_key = format!("{platform}:{external_id}");
        let exists = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM download_archive WHERE archive_key = ?1",
        )
        .bind(archive_key)
        .fetch_one(&self.pool)
        .await
        .map_err(|err| VesselError::Database(err.to_string()))?;
        Ok(exists > 0)
    }

    pub async fn insert_artifact(
        &self,
        video_id: &str,
        artifact_kind: &str,
        path: &str,
        content_hash: &str,
        byte_size: u64,
        format_id: Option<&str>,
    ) -> Result<ArtifactRecord> {
        let artifact_id = Uuid::now_v7().to_string();
        let created_at = OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|err| VesselError::Database(err.to_string()))?;
        sqlx::query(
            r#"
INSERT INTO artifacts (
    id, video_id, artifact_kind, path, content_hash, byte_size, format_id, created_at
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
"#,
        )
        .bind(&artifact_id)
        .bind(video_id)
        .bind(artifact_kind)
        .bind(path)
        .bind(content_hash)
        .bind(byte_size as i64)
        .bind(format_id)
        .bind(&created_at)
        .execute(&self.pool)
        .await
        .map_err(|err| VesselError::Database(err.to_string()))?;

        Ok(ArtifactRecord {
            artifact_id,
            video_id: video_id.to_owned(),
            artifact_kind: artifact_kind.to_owned(),
            path: path.to_owned(),
            content_hash: content_hash.to_owned(),
            byte_size: byte_size as i64,
            format_id: format_id.map(ToOwned::to_owned),
            created_at,
        })
    }

    pub async fn insert_archive_entry(
        &self,
        platform: &str,
        external_id: &str,
        artifact_id: &str,
    ) -> Result<()> {
        let archive_id = Uuid::now_v7().to_string();
        let downloaded_at = OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|err| VesselError::Database(err.to_string()))?;
        let archive_key = format!("{platform}:{external_id}");
        sqlx::query(
            r#"
INSERT OR REPLACE INTO download_archive (
    id, platform, external_id, archive_key, downloaded_at, artifact_id
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6)
"#,
        )
        .bind(archive_id)
        .bind(platform)
        .bind(external_id)
        .bind(archive_key)
        .bind(downloaded_at)
        .bind(artifact_id)
        .execute(&self.pool)
        .await
        .map_err(|err| VesselError::Database(err.to_string()))?;
        Ok(())
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
    changed_fields_json,
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
                    changed_fields: serde_json::from_str::<Vec<String>>(
                        &row.get::<String, _>("changed_fields_json"),
                    )
                    .map_err(|err| VesselError::Database(err.to_string()))?,
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

    pub async fn sync_subtitle_tracks(
        &self,
        video: &VideoMetadata,
        tracks: &[SubtitleTrack],
    ) -> Result<usize> {
        let mut inserted = 0usize;
        for track in tracks {
            if self.upsert_subtitle_track(video, track).await? {
                inserted += 1;
            }
        }
        Ok(inserted)
    }

    pub async fn load_subtitle_history(&self, video_id: &str) -> Result<SubtitleHistory> {
        let tracks = sqlx::query(
            r#"
SELECT language, url, is_auto_generated, latest_snapshot_id, first_seen_at, last_seen_at, updated_at
FROM subtitle_tracks
WHERE video_id = ?1
ORDER BY language ASC
"#,
        )
        .bind(video_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|err| VesselError::Database(err.to_string()))?
        .into_iter()
        .map(|row| StoredSubtitleTrack {
            language: row.get("language"),
            url: row.get("url"),
            is_auto_generated: row.get::<i64, _>("is_auto_generated") != 0,
            latest_snapshot_id: row.get("latest_snapshot_id"),
            first_seen_at: row.get("first_seen_at"),
            last_seen_at: row.get("last_seen_at"),
            updated_at: row.get("updated_at"),
        })
        .collect();

        let snapshots = sqlx::query(
            r#"
SELECT id, language, is_auto_generated, fetched_at, content_hash, changed_fields_json, normalized_json
FROM subtitle_snapshots
WHERE video_id = ?1
ORDER BY fetched_at DESC
"#,
        )
        .bind(video_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|err| VesselError::Database(err.to_string()))?
        .into_iter()
        .map(|row| -> Result<StoredSubtitleSnapshot> {
            Ok(StoredSubtitleSnapshot {
                snapshot_id: row.get("id"),
                language: row.get("language"),
                is_auto_generated: row.get::<i64, _>("is_auto_generated") != 0,
                fetched_at: row.get("fetched_at"),
                content_hash: row.get("content_hash"),
                changed_fields: serde_json::from_str(&row.get::<String, _>("changed_fields_json"))
                    .map_err(|err| VesselError::Database(err.to_string()))?,
                normalized_json: serde_json::from_str(&row.get::<String, _>("normalized_json"))
                    .map_err(|err| VesselError::Database(err.to_string()))?,
            })
        })
        .collect::<Result<Vec<_>>>()?;

        Ok(SubtitleHistory { tracks, snapshots })
    }

    pub async fn sync_comments(&self, comments: &[CommentMetadata]) -> Result<usize> {
        let mut inserted = 0usize;
        for comment in comments {
            if self.upsert_comment(comment).await? {
                inserted += 1;
            }
        }
        Ok(inserted)
    }

    pub async fn load_comment_history(&self, video_id: &str) -> Result<CommentHistory> {
        let comments = sqlx::query(
            r#"
SELECT comment_id, video_id, author_channel_id, author_name, text, like_count, reply_count, published_at, updated_at, latest_snapshot_id
FROM comments
WHERE video_id = ?1
ORDER BY updated_at DESC
"#,
        )
        .bind(video_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|err| VesselError::Database(err.to_string()))?
        .into_iter()
        .map(|row| StoredComment {
            comment_id: row.get("comment_id"),
            video_id: row.get("video_id"),
            author_channel_id: row.get("author_channel_id"),
            author_name: row.get("author_name"),
            text: row.get("text"),
            like_count: row.get("like_count"),
            reply_count: row.get("reply_count"),
            published_at: row.get("published_at"),
            updated_at: row.get("updated_at"),
            latest_snapshot_id: row.get("latest_snapshot_id"),
        })
        .collect();

        let snapshots = sqlx::query(
            r#"
SELECT id, comment_id, fetched_at, content_hash, changed_fields_json, normalized_json
FROM comment_snapshots
WHERE video_id = ?1
ORDER BY fetched_at DESC
"#,
        )
        .bind(video_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|err| VesselError::Database(err.to_string()))?
        .into_iter()
        .map(|row| -> Result<StoredCommentSnapshot> {
            Ok(StoredCommentSnapshot {
                snapshot_id: row.get("id"),
                comment_id: row.get("comment_id"),
                fetched_at: row.get("fetched_at"),
                content_hash: row.get("content_hash"),
                changed_fields: serde_json::from_str(&row.get::<String, _>("changed_fields_json"))
                    .map_err(|err| VesselError::Database(err.to_string()))?,
                normalized_json: serde_json::from_str(&row.get::<String, _>("normalized_json"))
                    .map_err(|err| VesselError::Database(err.to_string()))?,
            })
        })
        .collect::<Result<Vec<_>>>()?;

        Ok(CommentHistory {
            comments,
            snapshots,
        })
    }
}

pub async fn init_sqlite_database(target: &str) -> Result<(SqliteStore, DatabasePaths)> {
    let sqlite_url = normalize_sqlite_target(target);
    ensure_sqlite_parent_dir(&sqlite_url)?;
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

fn ensure_sqlite_parent_dir(sqlite_url: &str) -> Result<()> {
    let path = sqlite_url
        .strip_prefix("sqlite://")
        .or_else(|| sqlite_url.strip_prefix("sqlite:"))
        .unwrap_or(sqlite_url);
    let path = Path::new(path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
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
        let next_projection = video_snapshot_projection(video);
        let previous_projection = self
            .latest_video_snapshot_projection(&video.video_id)
            .await?;
        let changed_fields = previous_projection
            .as_ref()
            .map(|previous| diff_paths(previous, &next_projection))
            .unwrap_or_default();
        let normalized_json =
            serde_json::to_string(video).map_err(|err| VesselError::Database(err.to_string()))?;
        let raw_json = serde_json::to_string(&video.raw)
            .map_err(|err| VesselError::Database(err.to_string()))?;
        let changed_fields_json = serde_json::to_string(&changed_fields)
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
        .bind(changed_fields_json)
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

impl SqliteStore {
    async fn upsert_subtitle_track(
        &self,
        video: &VideoMetadata,
        track: &SubtitleTrack,
    ) -> Result<bool> {
        let projection = subtitle_track_projection(track);
        let content_hash = canonical_json_hash(&projection)
            .map_err(|err| VesselError::Database(err.to_string()))?;
        let previous = self
            .latest_subtitle_projection(&video.video_id, &track.language, track.is_auto_generated)
            .await?;
        let changed_fields = previous
            .as_ref()
            .map(|previous| diff_paths(previous, &projection))
            .unwrap_or_default();
        let normalized_json =
            serde_json::to_string(track).map_err(|err| VesselError::Database(err.to_string()))?;
        let raw_json = normalized_json.clone();
        let changed_fields_json = serde_json::to_string(&changed_fields)
            .map_err(|err| VesselError::Database(err.to_string()))?;
        let fetched_at = video
            .fetched_at
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|err| VesselError::Database(err.to_string()))?;
        let snapshot_id = Uuid::now_v7().to_string();
        let result = sqlx::query(
            r#"
INSERT OR IGNORE INTO subtitle_snapshots (
    id, video_id, language, is_auto_generated, fetched_at, content_hash, normalized_json, raw_json, changed_fields_json
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
"#,
        )
        .bind(&snapshot_id)
        .bind(&video.video_id)
        .bind(&track.language)
        .bind(i64::from(track.is_auto_generated))
        .bind(&fetched_at)
        .bind(&content_hash)
        .bind(&normalized_json)
        .bind(&raw_json)
        .bind(&changed_fields_json)
        .execute(&self.pool)
        .await
        .map_err(|err| VesselError::Database(err.to_string()))?;
        let latest_snapshot_id = if result.rows_affected() > 0 {
            Some(snapshot_id)
        } else {
            sqlx::query_scalar::<_, String>(
                "SELECT id FROM subtitle_snapshots WHERE video_id = ?1 AND language = ?2 AND is_auto_generated = ?3 AND content_hash = ?4 LIMIT 1",
            )
            .bind(&video.video_id)
            .bind(&track.language)
            .bind(i64::from(track.is_auto_generated))
            .bind(&content_hash)
            .fetch_one(&self.pool)
            .await
            .ok()
        };

        sqlx::query(
            r#"
INSERT INTO subtitle_tracks (
    id, video_id, language, url, is_auto_generated, latest_snapshot_id, first_seen_at, last_seen_at, updated_at
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
ON CONFLICT(video_id, language, is_auto_generated) DO UPDATE SET
    url = excluded.url,
    latest_snapshot_id = excluded.latest_snapshot_id,
    last_seen_at = excluded.last_seen_at,
    updated_at = excluded.updated_at
"#,
        )
        .bind(format!("{}:{}:{}", video.video_id, track.language, track.is_auto_generated))
        .bind(&video.video_id)
        .bind(&track.language)
        .bind(&track.url)
        .bind(i64::from(track.is_auto_generated))
        .bind(latest_snapshot_id)
        .bind(&fetched_at)
        .bind(&fetched_at)
        .bind(&fetched_at)
        .execute(&self.pool)
        .await
        .map_err(|err| VesselError::Database(err.to_string()))?;
        Ok(result.rows_affected() > 0)
    }

    async fn latest_subtitle_projection(
        &self,
        video_id: &str,
        language: &str,
        is_auto_generated: bool,
    ) -> Result<Option<serde_json::Value>> {
        let row = sqlx::query(
            r#"
SELECT normalized_json
FROM subtitle_snapshots
WHERE video_id = ?1 AND language = ?2 AND is_auto_generated = ?3
ORDER BY fetched_at DESC
LIMIT 1
"#,
        )
        .bind(video_id)
        .bind(language)
        .bind(i64::from(is_auto_generated))
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| VesselError::Database(err.to_string()))?;
        row.map(|row| {
            let json =
                serde_json::from_str::<serde_json::Value>(&row.get::<String, _>("normalized_json"))
                    .map_err(|err| VesselError::Database(err.to_string()))?;
            Ok(subtitle_track_projection_from_value(&json))
        })
        .transpose()
    }

    async fn upsert_comment(&self, comment: &CommentMetadata) -> Result<bool> {
        let projection = comment_projection(comment);
        let content_hash = canonical_json_hash(&projection)
            .map_err(|err| VesselError::Database(err.to_string()))?;
        let previous = self.latest_comment_projection(&comment.comment_id).await?;
        let changed_fields = previous
            .as_ref()
            .map(|previous| diff_paths(previous, &projection))
            .unwrap_or_default();
        let normalized_json =
            serde_json::to_string(comment).map_err(|err| VesselError::Database(err.to_string()))?;
        let raw_json = serde_json::to_string(&comment.raw)
            .map_err(|err| VesselError::Database(err.to_string()))?;
        let changed_fields_json = serde_json::to_string(&changed_fields)
            .map_err(|err| VesselError::Database(err.to_string()))?;
        let fetched_at = comment
            .fetched_at
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|err| VesselError::Database(err.to_string()))?;
        let snapshot_id = Uuid::now_v7().to_string();
        let result = sqlx::query(
            r#"
INSERT OR IGNORE INTO comment_snapshots (
    id, comment_id, video_id, fetched_at, content_hash, normalized_json, raw_json, changed_fields_json
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
"#,
        )
        .bind(&snapshot_id)
        .bind(&comment.comment_id)
        .bind(&comment.video_id)
        .bind(&fetched_at)
        .bind(&content_hash)
        .bind(&normalized_json)
        .bind(&raw_json)
        .bind(&changed_fields_json)
        .execute(&self.pool)
        .await
        .map_err(|err| VesselError::Database(err.to_string()))?;
        let latest_snapshot_id = if result.rows_affected() > 0 {
            Some(snapshot_id)
        } else {
            sqlx::query_scalar::<_, String>(
                "SELECT id FROM comment_snapshots WHERE comment_id = ?1 AND content_hash = ?2 LIMIT 1",
            )
            .bind(&comment.comment_id)
            .bind(&content_hash)
            .fetch_one(&self.pool)
            .await
            .ok()
        };
        sqlx::query(
            r#"
INSERT INTO comments (
    id, platform, comment_id, video_id, author_channel_id, author_name, text, like_count, reply_count, published_at, updated_at, latest_snapshot_id
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
ON CONFLICT(comment_id) DO UPDATE SET
    author_channel_id = excluded.author_channel_id,
    author_name = excluded.author_name,
    text = excluded.text,
    like_count = excluded.like_count,
    reply_count = excluded.reply_count,
    published_at = excluded.published_at,
    updated_at = excluded.updated_at,
    latest_snapshot_id = excluded.latest_snapshot_id
"#,
        )
        .bind(&comment.comment_id)
        .bind(format!("{:?}", comment.platform))
        .bind(&comment.comment_id)
        .bind(&comment.video_id)
        .bind(&comment.author_channel_id)
        .bind(&comment.author_name)
        .bind(&comment.text)
        .bind(comment.like_count.map(|v| v as i64))
        .bind(comment.reply_count.map(|v| v as i64))
        .bind(&comment.published_at)
        .bind(&fetched_at)
        .bind(latest_snapshot_id)
        .execute(&self.pool)
        .await
        .map_err(|err| VesselError::Database(err.to_string()))?;
        Ok(result.rows_affected() > 0)
    }

    async fn latest_comment_projection(
        &self,
        comment_id: &str,
    ) -> Result<Option<serde_json::Value>> {
        let row = sqlx::query(
            r#"
SELECT normalized_json
FROM comment_snapshots
WHERE comment_id = ?1
ORDER BY fetched_at DESC
LIMIT 1
"#,
        )
        .bind(comment_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| VesselError::Database(err.to_string()))?;

        row.map(|row| {
            let json =
                serde_json::from_str::<serde_json::Value>(&row.get::<String, _>("normalized_json"))
                    .map_err(|err| VesselError::Database(err.to_string()))?;
            Ok(comment_projection_from_value(&json))
        })
        .transpose()
    }

    async fn latest_video_snapshot_projection(
        &self,
        video_id: &str,
    ) -> Result<Option<serde_json::Value>> {
        let row = sqlx::query(
            r#"
SELECT normalized_json
FROM video_snapshots
WHERE video_id = ?1
ORDER BY fetched_at DESC
LIMIT 1
"#,
        )
        .bind(video_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| VesselError::Database(err.to_string()))?;

        row.map(|row| {
            let json =
                serde_json::from_str::<serde_json::Value>(&row.get::<String, _>("normalized_json"))
                    .map_err(|err| VesselError::Database(err.to_string()))?;
            Ok(video_snapshot_projection_from_value(&json))
        })
        .transpose()
    }
}

fn channel_snapshot_hash(channel: &ChannelMetadata) -> Result<String> {
    canonical_json_hash(&channel_snapshot_projection(channel))
        .map_err(|err| VesselError::Database(err.to_string()))
}

fn channel_snapshot_projection(channel: &ChannelMetadata) -> serde_json::Value {
    serde_json::json!({
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
    })
}

fn video_snapshot_hash(video: &VideoMetadata) -> Result<String> {
    canonical_json_hash(&video_snapshot_projection(video))
        .map_err(|err| VesselError::Database(err.to_string()))
}

fn video_snapshot_projection(video: &VideoMetadata) -> serde_json::Value {
    serde_json::json!({
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
    })
}

fn video_snapshot_projection_from_value(value: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "platform": value.get("platform"),
        "video_id": value.get("video_id"),
        "channel_id": value.get("channel_id"),
        "url": value.get("url"),
        "title": value.get("title"),
        "description": value.get("description"),
        "duration_seconds": value.get("duration_seconds"),
        "upload_date": value.get("upload_date"),
        "release_timestamp": value.get("release_timestamp"),
        "view_count": value.get("view_count"),
        "like_count": value.get("like_count"),
        "comment_count": value.get("comment_count"),
        "availability": value.get("availability"),
        "formats": value.get("formats"),
        "subtitles": value.get("subtitles"),
        "thumbnails": value.get("thumbnails"),
    })
}

fn subtitle_track_projection(track: &SubtitleTrack) -> serde_json::Value {
    serde_json::json!({
        "language": &track.language,
        "url": &track.url,
        "is_auto_generated": track.is_auto_generated,
    })
}

fn subtitle_track_projection_from_value(value: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "language": value.get("language"),
        "url": value.get("url"),
        "is_auto_generated": value.get("is_auto_generated"),
    })
}

fn comment_projection(comment: &CommentMetadata) -> serde_json::Value {
    serde_json::json!({
        "platform": &comment.platform,
        "comment_id": &comment.comment_id,
        "video_id": &comment.video_id,
        "author_channel_id": &comment.author_channel_id,
        "author_name": &comment.author_name,
        "text": &comment.text,
        "like_count": comment.like_count,
        "reply_count": comment.reply_count,
        "published_at": &comment.published_at,
    })
}

fn comment_projection_from_value(value: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "platform": value.get("platform"),
        "comment_id": value.get("comment_id"),
        "video_id": value.get("video_id"),
        "author_channel_id": value.get("author_channel_id"),
        "author_name": value.get("author_name"),
        "text": value.get("text"),
        "like_count": value.get("like_count"),
        "reply_count": value.get("reply_count"),
        "published_at": value.get("published_at"),
    })
}

fn diff_paths(previous: &serde_json::Value, next: &serde_json::Value) -> Vec<String> {
    let mut paths = Vec::new();
    collect_diff_paths(previous, next, "", &mut paths);
    paths
}

fn collect_diff_paths(
    previous: &serde_json::Value,
    next: &serde_json::Value,
    prefix: &str,
    output: &mut Vec<String>,
) {
    match (previous, next) {
        (serde_json::Value::Object(previous_map), serde_json::Value::Object(next_map)) => {
            let mut keys = previous_map
                .keys()
                .chain(next_map.keys())
                .cloned()
                .collect::<Vec<_>>();
            keys.sort();
            keys.dedup();
            for key in keys {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                match (previous_map.get(&key), next_map.get(&key)) {
                    (Some(previous_value), Some(next_value)) => {
                        collect_diff_paths(previous_value, next_value, &path, output);
                    }
                    _ => output.push(path),
                }
            }
        }
        _ => {
            if previous != next {
                output.push(prefix.to_owned());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use time::OffsetDateTime;
    use uuid::Uuid;
    use vessel_core::models::{
        Availability, ChannelMetadata, CommentMetadata, Platform, SubtitleTrack, VideoMetadata,
    };
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

    fn sample_subtitle(language: &str, url: &str, is_auto_generated: bool) -> SubtitleTrack {
        SubtitleTrack {
            language: language.to_owned(),
            url: Some(url.to_owned()),
            is_auto_generated,
        }
    }

    fn sample_comment(fetched_at: OffsetDateTime, text: &str) -> CommentMetadata {
        CommentMetadata {
            platform: Platform::YouTube,
            comment_id: "comment-123".to_owned(),
            video_id: "video-123".to_owned(),
            author_channel_id: Some("author-456".to_owned()),
            author_name: Some("Author".to_owned()),
            text: text.to_owned(),
            like_count: Some(5),
            reply_count: Some(1),
            published_at: Some("2024-01-01T00:00:00Z".to_owned()),
            fetched_at,
            raw: serde_json::json!({ "text": text }),
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

    #[tokio::test]
    async fn changed_video_metadata_creates_one_new_snapshot_with_diff() {
        let path = temp_db_path("diff");
        let path_str = path.to_string_lossy().into_owned();
        let (store, _) = init_sqlite_database(&path_str).await.expect("db init");
        let fetched_at = OffsetDateTime::now_utc();

        let mut changed_video = sample_video(fetched_at + time::Duration::minutes(1));
        changed_video.title = Some("Updated Title".to_owned());
        changed_video.view_count = Some(200);

        let first = store
            .upsert_video_snapshot(&sample_video(fetched_at))
            .await
            .expect("first upsert");
        let second = store
            .upsert_video_snapshot(&changed_video)
            .await
            .expect("changed upsert");
        let third = store
            .upsert_video_snapshot(&changed_video)
            .await
            .expect("repeat changed upsert");
        let history = store
            .load_video_history("video-123")
            .await
            .expect("history");

        assert!(first);
        assert!(second);
        assert!(!third);
        assert_eq!(history.snapshots.len(), 2);
        assert_eq!(
            history.snapshots[0].changed_fields,
            vec!["title".to_owned(), "view_count".to_owned()]
        );

        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = fs::remove_file(path.with_extension("sqlite-shm"));
    }

    #[tokio::test]
    async fn tracked_channels_can_be_added_and_listed() {
        let path = temp_db_path("tracked");
        let path_str = path.to_string_lossy().into_owned();
        let (store, _) = init_sqlite_database(&path_str).await.expect("db init");
        let now = OffsetDateTime::now_utc();

        let channel = ChannelMetadata {
            platform: Platform::YouTube,
            channel_id: "UC-tracked".to_owned(),
            handle: Some("@tracked".to_owned()),
            url: "https://www.youtube.com/@tracked".to_owned(),
            title: Some("Tracked".to_owned()),
            description: Some("Tracked channel".to_owned()),
            subscriber_count: None,
            video_count: None,
            view_count: None,
            avatar_url: None,
            banner_url: None,
            fetched_at: now,
            raw: serde_json::json!({ "tracked": true }),
        };

        store
            .add_tracked_channel(&channel)
            .await
            .expect("add channel");
        let tracked = store.list_tracked_channels().await.expect("list tracked");

        assert_eq!(tracked.len(), 1);
        assert_eq!(tracked[0].channel_id, "UC-tracked");
        assert_eq!(tracked[0].handle.as_deref(), Some("@tracked"));

        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = fs::remove_file(path.with_extension("sqlite-shm"));
    }

    #[tokio::test]
    async fn subtitle_tracks_are_snapshotted_without_colliding_manual_and_auto_tracks() {
        let path = temp_db_path("subtitles");
        let path_str = path.to_string_lossy().into_owned();
        let (store, _) = init_sqlite_database(&path_str).await.expect("db init");
        let mut video = sample_video(OffsetDateTime::now_utc());
        video.subtitles = vec![
            sample_subtitle("en", "https://example.invalid/manual-en.vtt", false),
            sample_subtitle("en", "https://example.invalid/auto-en.vtt", true),
        ];

        let first = store
            .sync_subtitle_tracks(&video, &video.subtitles)
            .await
            .expect("first subtitle sync");
        let second = store
            .sync_subtitle_tracks(&video, &video.subtitles)
            .await
            .expect("second subtitle sync");
        let history = store
            .load_subtitle_history(&video.video_id)
            .await
            .expect("subtitle history");

        assert_eq!(first, 2);
        assert_eq!(second, 0);
        assert_eq!(history.tracks.len(), 2);
        assert_eq!(history.snapshots.len(), 2);

        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = fs::remove_file(path.with_extension("sqlite-shm"));
    }

    #[tokio::test]
    async fn comments_are_snapshotted_idempotently_and_record_diffs() {
        let path = temp_db_path("comments");
        let path_str = path.to_string_lossy().into_owned();
        let (store, _) = init_sqlite_database(&path_str).await.expect("db init");
        let now = OffsetDateTime::now_utc();

        let first = store
            .sync_comments(&[sample_comment(now, "First body")])
            .await
            .expect("first comment sync");
        let second = store
            .sync_comments(&[sample_comment(
                now + time::Duration::minutes(1),
                "First body",
            )])
            .await
            .expect("second comment sync");
        let third = store
            .sync_comments(&[sample_comment(
                now + time::Duration::minutes(2),
                "Edited body",
            )])
            .await
            .expect("third comment sync");
        let history = store
            .load_comment_history("video-123")
            .await
            .expect("comment history");

        assert_eq!(first, 1);
        assert_eq!(second, 0);
        assert_eq!(third, 1);
        assert_eq!(history.comments.len(), 1);
        assert_eq!(history.snapshots.len(), 2);
        assert_eq!(history.snapshots[0].changed_fields, vec!["text".to_owned()]);

        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = fs::remove_file(path.with_extension("sqlite-shm"));
    }
}
