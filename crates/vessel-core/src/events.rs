use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VesselEvent {
    RunStarted {
        run_id: Uuid,
        at: OffsetDateTime,
        command: String,
    },
    ConfigLoaded {
        source: String,
    },
    ExtractorSelected {
        extractor: String,
        input: String,
    },
    RequestStarted {
        target: String,
    },
    RequestFinished {
        target: String,
        ok: bool,
    },
    RetryScheduled {
        target: String,
        attempt: u32,
    },
    ChannelFetched {
        channel_id: String,
    },
    VideoDiscovered {
        video_id: String,
    },
    SnapshotUnchanged {
        entity: String,
        entity_id: String,
    },
    SnapshotInserted {
        entity: String,
        entity_id: String,
        content_hash: String,
    },
    DownloadStarted {
        video_id: String,
    },
    DownloadCompleted {
        video_id: String,
        artifact_path: String,
    },
    PostprocessStarted {
        job: String,
    },
    PostprocessCompleted {
        job: String,
    },
    RunFinished {
        run_id: Uuid,
        at: OffsetDateTime,
        ok: bool,
    },
}
