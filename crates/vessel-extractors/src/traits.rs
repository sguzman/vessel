use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use vessel_core::models::{ChannelMetadata, InputRef, VideoMetadata};
use vessel_core::{Result, VesselError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractRequest {
    pub input: InputRef,
}

#[derive(Debug, Clone, Default)]
pub struct ExtractContext;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SupportLevel {
    Unsupported,
    Generic,
    Native,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExtractedItem {
    Video(VideoMetadata),
    Playlist { id: String, title: Option<String> },
    Channel(ChannelMetadata),
    Collection(Vec<ExtractedItem>),
    Failure { message: String },
}

#[async_trait]
pub trait Extractor: Send + Sync {
    fn name(&self) -> &'static str;

    fn supports(&self, input: &InputRef) -> SupportLevel;

    async fn extract(&self, request: ExtractRequest, ctx: ExtractContext) -> Result<ExtractedItem>;
}

pub fn unsupported(name: &str) -> Result<ExtractedItem> {
    Err(VesselError::Unsupported(name.to_owned()))
}
