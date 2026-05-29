use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Platform {
    YouTube,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InputKind {
    Url,
    VideoId,
    ChannelId,
    PlaylistId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputRef {
    pub raw: String,
    pub kind: InputKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Availability {
    Public,
    Unlisted,
    Private,
    MembersOnly,
    Deleted,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelMetadata {
    pub platform: Platform,
    pub channel_id: String,
    pub handle: Option<String>,
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub subscriber_count: Option<u64>,
    pub video_count: Option<u64>,
    pub view_count: Option<u64>,
    pub avatar_url: Option<String>,
    pub banner_url: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub fetched_at: OffsetDateTime,
    pub raw: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thumbnail {
    pub url: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleTrack {
    pub language: String,
    pub url: Option<String>,
    pub is_auto_generated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommentMetadata {
    pub platform: Platform,
    pub comment_id: String,
    pub video_id: String,
    pub author_channel_id: Option<String>,
    pub author_name: Option<String>,
    pub text: String,
    pub like_count: Option<u64>,
    pub reply_count: Option<u64>,
    pub published_at: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub fetched_at: OffsetDateTime,
    pub raw: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaFormat {
    pub format_id: String,
    pub ext: String,
    pub note: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub download_url: Option<String>,
    pub protocol: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bitrate: Option<u64>,
    pub has_video: bool,
    pub has_audio: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoMetadata {
    pub platform: Platform,
    pub video_id: String,
    pub channel_id: Option<String>,
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub duration_seconds: Option<u64>,
    pub upload_date: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub release_timestamp: Option<OffsetDateTime>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    pub primary_category: Option<String>,
    pub view_count: Option<u64>,
    pub like_count: Option<u64>,
    pub comment_count: Option<u64>,
    pub availability: Availability,
    pub formats: Vec<MediaFormat>,
    pub subtitles: Vec<SubtitleTrack>,
    pub thumbnails: Vec<Thumbnail>,
    #[serde(with = "time::serde::rfc3339")]
    pub fetched_at: OffsetDateTime,
    pub raw: Value,
}

pub fn canonical_json_hash<T>(value: &T) -> anyhow::Result<String>
where
    T: Serialize,
{
    let json = serde_json::to_vec(value)?;
    Ok(blake3::hash(&json).to_hex().to_string())
}
