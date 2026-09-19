pub mod config;
pub mod error;
pub mod events;
pub mod models;
pub mod sourcearium;
pub mod sourcearium_repo;
pub mod transcript;

pub use config::{
    ChannelCategoryConfig, ChannelsConfig, Config, ConfigPaths, LoggingFormat, RuntimeLayout,
    load_config, resolve_runtime_layout,
};
pub use error::{Result, VesselError};
pub use events::VesselEvent;
pub use sourcearium::{
    AcquisitionV1, SourceIdentityV1, SourceariumArtifactV1, TextRepresentationV1, VideoSelection,
    YoutubeChannelPolicyV1, YoutubeSelectionPolicyV1, YoutubeSourcePolicyV1,
    YoutubeTranscriptPolicyV1,
};
pub use sourcearium_repo::{
    ExistingSourceariumArtifact, MaterializeResult, MaterializeStatus, SourceariumInventoryReport,
    SourceariumValidationReport, SourceariumYoutubeSource, discover_youtube_sources,
    inventory_sourcearium_repository, load_youtube_transcript_artifact,
    materialize_youtube_transcript, validate_sourcearium_repository,
};
pub use transcript::{
    TranscriptCandidate, TranscriptDerivation, TranscriptProvider, TranscriptRequest,
    TranscriptSegment,
};
