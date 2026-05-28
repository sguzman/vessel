use async_trait::async_trait;

use vessel_core::Result;
use vessel_core::models::{ChannelMetadata, VideoMetadata};
use vessel_ledger::{FetchAttempt, RefreshDecision, SyncOptions};

#[async_trait]
pub trait CurrentStateStore: Send + Sync {
    async fn put_channel_latest(&self, channel: &ChannelMetadata, content_hash: &str)
    -> Result<()>;
    async fn put_video_latest(&self, video: &VideoMetadata, content_hash: &str) -> Result<()>;
}

#[async_trait]
pub trait SnapshotStore: Send + Sync {
    async fn insert_channel_snapshot_if_changed(
        &self,
        channel: &ChannelMetadata,
        content_hash: &str,
    ) -> Result<bool>;
    async fn insert_video_snapshot_if_changed(
        &self,
        video: &VideoMetadata,
        content_hash: &str,
    ) -> Result<bool>;
    async fn record_fetch_attempt(&self, attempt: FetchAttempt) -> Result<()>;
    async fn should_refresh_video(
        &self,
        video_id: &str,
        opts: &SyncOptions,
    ) -> Result<RefreshDecision>;
}
