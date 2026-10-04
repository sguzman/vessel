pub mod config;
pub mod error;
pub mod events;
pub mod models;
pub mod sourcearium;
pub mod sourcearium_repo;
pub mod speaker_evidence;
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
    SourceariumPruneCandidate, SourceariumPrunePlan, SourceariumValidationReport,
    SourceariumYoutubeSource, TranscriptDiarizationApplyResult,
    TranscriptDiarizationProvenanceRefreshResult, apply_sourcearium_prune,
    apply_youtube_transcript_diarization, discover_youtube_sources,
    inventory_sourcearium_repository, load_youtube_transcript_artifact,
    materialize_youtube_transcript, plan_sourcearium_prune,
    refresh_youtube_transcript_diarization_provenance, validate_sourcearium_repository,
};
pub use speaker_evidence::{
    SPEAKER_EVIDENCE_SCHEMA_V1, SpeakerEvidenceDiarization, SpeakerEvidenceProvenance,
    SpeakerEvidenceSegment, SpeakerEvidenceV1, load_speaker_evidence,
};
pub use transcript::{
    DiarizationProvenance, SpeakerAttribution, TranscriptCandidate, TranscriptDerivation,
    TranscriptProvider, TranscriptRequest, TranscriptSegment, TranscriptSpeaker,
};
