use async_trait::async_trait;
use time::OffsetDateTime;

use vessel_core::Result;
use vessel_core::models::{
    Availability, ChannelMetadata, InputKind, InputRef, Platform, VideoMetadata,
};

use crate::traits::{ExtractContext, ExtractRequest, ExtractedItem, Extractor, SupportLevel};

#[derive(Debug, Default)]
pub struct YoutubeExtractor;

#[async_trait]
impl Extractor for YoutubeExtractor {
    fn name(&self) -> &'static str {
        "youtube"
    }

    fn supports(&self, input: &InputRef) -> SupportLevel {
        match input.kind {
            InputKind::Url
                if input.raw.contains("youtube.com") || input.raw.contains("youtu.be") =>
            {
                SupportLevel::Native
            }
            InputKind::VideoId | InputKind::ChannelId | InputKind::PlaylistId => {
                SupportLevel::Native
            }
            InputKind::Url => SupportLevel::Unsupported,
        }
    }

    async fn extract(
        &self,
        request: ExtractRequest,
        _ctx: ExtractContext,
    ) -> Result<ExtractedItem> {
        let now = OffsetDateTime::now_utc();
        if request.input.raw.contains("/channel/") || request.input.raw.contains("/@") {
            return Ok(ExtractedItem::Channel(ChannelMetadata {
                platform: Platform::YouTube,
                channel_id: "stub-channel".to_owned(),
                handle: Some("@stub".to_owned()),
                url: request.input.raw,
                title: Some("Stub Channel".to_owned()),
                description: Some("Bootstrap placeholder extractor.".to_owned()),
                subscriber_count: None,
                video_count: None,
                view_count: None,
                avatar_url: None,
                banner_url: None,
                fetched_at: now,
                raw: serde_json::json!({ "stub": true }),
            }));
        }

        Ok(ExtractedItem::Video(VideoMetadata {
            platform: Platform::YouTube,
            video_id: "stub-video".to_owned(),
            channel_id: Some("stub-channel".to_owned()),
            url: request.input.raw,
            title: Some("Stub Video".to_owned()),
            description: Some("Bootstrap placeholder extractor.".to_owned()),
            duration_seconds: None,
            upload_date: None,
            release_timestamp: None,
            view_count: None,
            like_count: None,
            comment_count: None,
            availability: Availability::Unknown,
            formats: Vec::new(),
            subtitles: Vec::new(),
            thumbnails: Vec::new(),
            fetched_at: now,
            raw: serde_json::json!({ "stub": true }),
        }))
    }
}
