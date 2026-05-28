use serde::{Deserialize, Serialize};

use vessel_core::models::VideoMetadata;
use vessel_formats::FormatSelector;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadPlan {
    pub video_id: String,
    pub selector: FormatSelector,
    pub output_template: String,
}

pub trait DownloadPlanner: Send + Sync {
    fn plan(&self, item: &VideoMetadata, selector: FormatSelector) -> DownloadPlan;
}

#[derive(Debug, Default)]
pub struct StubDownloadPlanner;

impl DownloadPlanner for StubDownloadPlanner {
    fn plan(&self, item: &VideoMetadata, selector: FormatSelector) -> DownloadPlan {
        DownloadPlan {
            video_id: item.video_id.clone(),
            selector,
            output_template: "%(title)s.%(ext)s".to_owned(),
        }
    }
}
