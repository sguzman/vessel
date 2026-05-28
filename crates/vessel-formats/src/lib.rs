use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FormatSelector {
    Best,
    Worst,
    BestAudio,
    BestVideo,
    ExactFormatId(String),
    Merge(Box<FormatSelector>, Box<FormatSelector>),
    Fallback(Vec<FormatSelector>),
    Filtered {
        base: Box<FormatSelector>,
        predicate: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputTemplate {
    pub raw: String,
}

impl Default for OutputTemplate {
    fn default() -> Self {
        Self {
            raw: "%(channel)s/%(upload_date)s - %(title)s [%(id)s].%(ext)s".to_owned(),
        }
    }
}
