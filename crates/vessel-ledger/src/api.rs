use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use vessel_core::Result;
use vessel_core::models::{ChannelMetadata, VideoMetadata};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AttemptStatus {
    Success,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchAttempt {
    pub run_id: Uuid,
    pub target_kind: String,
    pub target_external_id: String,
    pub status: AttemptStatus,
    pub started_at: OffsetDateTime,
    pub finished_at: OffsetDateTime,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshDecision {
    pub should_fetch: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChannelSyncReport {
    pub discovered: usize,
    pub inserted: usize,
    pub skipped: usize,
    pub errors: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncOptions {
    pub force: bool,
    pub include_comments: bool,
    pub include_subtitles: bool,
    pub since: Option<String>,
    pub max_videos: Option<usize>,
}

#[async_trait]
pub trait Ledger: Send + Sync {
    async fn start_run(&self, command: &str) -> Result<Uuid>;
    async fn record_attempt(&self, attempt: FetchAttempt) -> Result<()>;
    async fn upsert_channel_snapshot(&self, channel: &ChannelMetadata) -> Result<bool>;
    async fn upsert_video_snapshot(&self, video: &VideoMetadata) -> Result<bool>;
    async fn should_refresh_video(
        &self,
        video_id: &str,
        opts: &SyncOptions,
    ) -> Result<RefreshDecision>;
    async fn finish_run(&self, run_id: Uuid, ok: bool) -> Result<()>;
}
