use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PostprocessStep {
    Merge,
    Remux,
    ExtractAudio,
    EmbedMetadata,
    EmbedThumbnail,
    ConvertSubtitles,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostprocessPlan {
    pub steps: Vec<PostprocessStep>,
}
