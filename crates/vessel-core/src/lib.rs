pub mod config;
pub mod error;
pub mod events;
pub mod models;
pub mod sourcearium;
pub mod sourcearium_repo;
pub mod speaker_match;
pub mod speaker_registry;
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
    SourceariumYoutubeSource, SpeakerAttributionApplyResult, TranscriptDiarizationApplyResult,
    apply_sourcearium_prune, apply_speaker_match_report, apply_youtube_transcript_diarization,
    discover_youtube_sources, inventory_sourcearium_repository, load_youtube_transcript_artifact,
    materialize_youtube_transcript, plan_sourcearium_prune, render_speaker_attributed_transcript,
    validate_sourcearium_repository,
};
pub use speaker_match::{
    AnchorEvidenceDiagnostic, AnchorEvidenceStatus, SPEAKER_EVIDENCE_SCHEMA_V1,
    SpeakerEvidenceDiarization, SpeakerEvidenceProvenance, SpeakerEvidenceSegment,
    SpeakerEvidenceV1, SpeakerMatch, SpeakerMatchConfig, SpeakerMatchReport, SpeakerMatchStatus,
    load_speaker_evidence, match_speakers_from_evidence, speaker_evidence_fingerprint,
};
pub use speaker_registry::{
    SPEAKER_REGISTRY_SCHEMA_V1, SpeakerAnchorV1, SpeakerIdentityV1, SpeakerRegistryV1,
    load_speaker_registry, write_speaker_registry,
};
pub use transcript::{
    DiarizationProvenance, SpeakerAttribution, TranscriptCandidate, TranscriptDerivation,
    TranscriptProvider, TranscriptRequest, TranscriptSegment, TranscriptSpeaker,
};
