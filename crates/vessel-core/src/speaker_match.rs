use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Result, SpeakerRegistryV1, VesselError};

pub const SPEAKER_EVIDENCE_SCHEMA_V1: u8 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeakerEvidenceV1 {
    pub schema: u8,
    pub video_id: String,
    pub asr: SpeakerEvidenceProvenance,
    pub diarization: SpeakerEvidenceDiarization,
    #[serde(default)]
    pub segments: Vec<SpeakerEvidenceSegment>,
    #[serde(default)]
    pub speaker_embeddings: Option<BTreeMap<String, Vec<f64>>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeakerEvidenceProvenance {
    pub engine: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeakerEvidenceDiarization {
    pub engine: String,
    pub model: String,
    pub label_scope: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeakerEvidenceSegment {
    pub start: f64,
    pub end: f64,
    pub speaker: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SpeakerMatchConfig {
    pub min_similarity: f64,
    pub min_margin: f64,
    pub min_anchor_dominance: f64,
}

impl Default for SpeakerMatchConfig {
    fn default() -> Self {
        Self {
            min_similarity: 0.80,
            min_margin: 0.05,
            min_anchor_dominance: 0.80,
        }
    }
}

impl SpeakerMatchConfig {
    pub fn validate(self) -> Result<Self> {
        if !(-1.0..=1.0).contains(&self.min_similarity) {
            return Err(match_error("min_similarity must be between -1 and 1"));
        }
        if !(0.0..=2.0).contains(&self.min_margin) {
            return Err(match_error("min_margin must be between 0 and 2"));
        }
        if !(0.0..=1.0).contains(&self.min_anchor_dominance) {
            return Err(match_error(
                "min_anchor_dominance must be between 0 and 1",
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorEvidenceStatus {
    Used,
    MissingEvidence,
    InvalidEvidence,
    IncompatibleProvenance,
    NoSpeechOverlap,
    LowDominance,
    MissingEmbedding,
    IncompatibleDimension,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AnchorEvidenceDiagnostic {
    pub speaker_key: String,
    pub video_id: String,
    pub start_seconds: u64,
    pub end_seconds: u64,
    pub status: AnchorEvidenceStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diarization_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dominance: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeakerMatchStatus {
    Matched,
    BelowSimilarity,
    AmbiguousMargin,
    MissingEmbedding,
    NoCompatibleAnchors,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpeakerMatch {
    pub diarization_label: String,
    pub status: SpeakerMatchStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_identity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub similarity: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub second_identity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub second_similarity: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub margin: Option<f64>,
    pub anchor_samples: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpeakerMatchReport {
    pub target_video_id: String,
    pub registry_revision: u64,
    pub evidence_fingerprint: String,
    pub diarization_engine: String,
    pub diarization_model: String,
    pub config: SpeakerMatchConfig,
    pub anchor_samples_used: usize,
    pub identities_with_compatible_anchors: usize,
    pub anchor_diagnostics: Vec<AnchorEvidenceDiagnostic>,
    pub matches: Vec<SpeakerMatch>,
}

impl SpeakerEvidenceV1 {
    pub fn validate(&self) -> Result<()> {
        if self.schema != SPEAKER_EVIDENCE_SCHEMA_V1 {
            return Err(match_error(format!(
                "unsupported speaker evidence schema {}; expected {}",
                self.schema, SPEAKER_EVIDENCE_SCHEMA_V1
            )));
        }
        require_nonempty("speaker evidence video_id", &self.video_id)?;
        require_nonempty("speaker evidence ASR engine", &self.asr.engine)?;
        require_nonempty("speaker evidence ASR model", &self.asr.model)?;
        require_nonempty(
            "speaker evidence diarization engine",
            &self.diarization.engine,
        )?;
        require_nonempty(
            "speaker evidence diarization model",
            &self.diarization.model,
        )?;
        if self.diarization.label_scope != "file_local" {
            return Err(match_error(format!(
                "speaker evidence label_scope must be \"file_local\", got {:?}",
                self.diarization.label_scope
            )));
        }

        for segment in &self.segments {
            if !segment.start.is_finite()
                || !segment.end.is_finite()
                || segment.start < 0.0
                || segment.end <= segment.start
            {
                return Err(match_error(format!(
                    "invalid speaker evidence segment {}-{}",
                    segment.start, segment.end
                )));
            }
            require_nonempty("speaker evidence segment label", &segment.speaker)?;
        }

        if let Some(embeddings) = self.speaker_embeddings.as_ref() {
            let mut dimension = None;
            for (label, embedding) in embeddings {
                require_nonempty("speaker embedding label", label)?;
                if embedding.is_empty() {
                    return Err(match_error(format!(
                        "speaker embedding {label:?} must not be empty"
                    )));
                }
                if embedding.iter().any(|value| !value.is_finite()) {
                    return Err(match_error(format!(
                        "speaker embedding {label:?} contains non-finite values"
                    )));
                }
                let norm = embedding.iter().map(|value| value * value).sum::<f64>().sqrt();
                if norm <= f64::EPSILON {
                    return Err(match_error(format!(
                        "speaker embedding {label:?} has zero norm"
                    )));
                }
                if let Some(expected) = dimension {
                    if embedding.len() != expected {
                        return Err(match_error(format!(
                            "speaker embedding dimension mismatch: {label:?} has {}, expected {expected}",
                            embedding.len()
                        )));
                    }
                } else {
                    dimension = Some(embedding.len());
                }
            }
        }

        Ok(())
    }

    pub fn embedding_dimension(&self) -> Option<usize> {
        self.speaker_embeddings
            .as_ref()
            .and_then(|embeddings| embeddings.values().next())
            .map(Vec::len)
    }

    fn embedding(&self, label: &str) -> Option<&[f64]> {
        self.speaker_embeddings
            .as_ref()?
            .get(label)
            .map(Vec::as_slice)
    }
}

pub fn load_speaker_evidence(path: &Path) -> Result<SpeakerEvidenceV1> {
    let raw = fs::read_to_string(path)?;
    let evidence: SpeakerEvidenceV1 = serde_json::from_str(&raw).map_err(|error| {
        match_error(format!(
            "{}: invalid speaker evidence JSON: {error}",
            path.display()
        ))
    })?;
    evidence.validate()?;
    Ok(evidence)
}

pub fn speaker_evidence_fingerprint(
    registry: &SpeakerRegistryV1,
    evidence_dir: &Path,
    target_video_id: &str,
) -> Result<String> {
    registry.validate()?;
    require_nonempty("target video id", target_video_id)?;

    let mut video_ids = BTreeSet::new();
    video_ids.insert(target_video_id.to_owned());
    for speaker in &registry.speakers {
        for anchor in &speaker.anchors {
            video_ids.insert(anchor.video_id.clone());
        }
    }

    let mut hasher = blake3::Hasher::new();
    hasher.update(b"vessel-speaker-evidence-fingerprint-v1\0");
    for video_id in video_ids {
        hasher.update(video_id.as_bytes());
        hasher.update(&[0]);
        let path = evidence_path(evidence_dir, &video_id);
        match fs::read(&path) {
            Ok(bytes) => {
                hasher.update(b"present\0");
                hasher.update(&(bytes.len() as u64).to_le_bytes());
                hasher.update(&bytes);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                hasher.update(b"missing\0");
            }
            Err(error) => return Err(error.into()),
        }
    }

    Ok(hasher.finalize().to_hex().to_string())
}

pub fn match_speakers_from_evidence(
    registry: &SpeakerRegistryV1,
    evidence_dir: &Path,
    target_video_id: &str,
    config: SpeakerMatchConfig,
) -> Result<SpeakerMatchReport> {
    registry.validate()?;
    let config = config.validate()?;
    require_nonempty("target video id", target_video_id)?;
    let evidence_fingerprint =
        speaker_evidence_fingerprint(registry, evidence_dir, target_video_id)?;

    let target_path = evidence_path(evidence_dir, target_video_id);
    if !target_path.is_file() {
        return Err(match_error(format!(
            "target speaker evidence is missing: {}; run vessel diarization apply for this video first",
            target_path.display()
        )));
    }
    let target = load_speaker_evidence(&target_path)?;
    if target.video_id != target_video_id {
        return Err(match_error(format!(
            "target speaker evidence video_id {:?} does not match requested {target_video_id:?}",
            target.video_id
        )));
    }
    let target_dimension = target.embedding_dimension().ok_or_else(|| {
        match_error(format!(
            "target speaker evidence {} has no speaker embeddings; rerun vessel diarization apply for this video",
            target_path.display()
        ))
    })?;

    let mut anchor_diagnostics = Vec::new();
    let mut identity_samples = BTreeMap::<String, Vec<Vec<f64>>>::new();

    for speaker in &registry.speakers {
        for anchor in &speaker.anchors {
            let path = evidence_path(evidence_dir, &anchor.video_id);
            if !path.is_file() {
                anchor_diagnostics.push(anchor_diagnostic(
                    &speaker.key,
                    anchor,
                    AnchorEvidenceStatus::MissingEvidence,
                    None,
                    None,
                    Some(format!("missing {}", path.display())),
                ));
                continue;
            }

            let evidence = match load_speaker_evidence(&path) {
                Ok(evidence) => evidence,
                Err(error) => {
                    anchor_diagnostics.push(anchor_diagnostic(
                        &speaker.key,
                        anchor,
                        AnchorEvidenceStatus::InvalidEvidence,
                        None,
                        None,
                        Some(error.to_string()),
                    ));
                    continue;
                }
            };

            if evidence.diarization.engine != target.diarization.engine
                || !compatible_embedding_provenance(
                    &evidence.diarization.model,
                    &target.diarization.model,
                )
            {
                anchor_diagnostics.push(anchor_diagnostic(
                    &speaker.key,
                    anchor,
                    AnchorEvidenceStatus::IncompatibleProvenance,
                    None,
                    None,
                    Some(format!(
                        "anchor uses {}/{} but target uses {}/{}",
                        evidence.diarization.engine,
                        evidence.diarization.model,
                        target.diarization.engine,
                        target.diarization.model
                    )),
                ));
                continue;
            }

            let Some((label, dominance)) = dominant_anchor_label(&evidence, anchor.start_seconds, anchor.end_seconds) else {
                anchor_diagnostics.push(anchor_diagnostic(
                    &speaker.key,
                    anchor,
                    AnchorEvidenceStatus::NoSpeechOverlap,
                    None,
                    None,
                    None,
                ));
                continue;
            };

            if dominance < config.min_anchor_dominance {
                anchor_diagnostics.push(anchor_diagnostic(
                    &speaker.key,
                    anchor,
                    AnchorEvidenceStatus::LowDominance,
                    Some(label),
                    Some(dominance),
                    Some(format!(
                        "dominance {:.4} is below minimum {:.4}",
                        dominance, config.min_anchor_dominance
                    )),
                ));
                continue;
            }

            let Some(embedding) = evidence.embedding(&label) else {
                anchor_diagnostics.push(anchor_diagnostic(
                    &speaker.key,
                    anchor,
                    AnchorEvidenceStatus::MissingEmbedding,
                    Some(label),
                    Some(dominance),
                    None,
                ));
                continue;
            };

            if embedding.len() != target_dimension {
                anchor_diagnostics.push(anchor_diagnostic(
                    &speaker.key,
                    anchor,
                    AnchorEvidenceStatus::IncompatibleDimension,
                    Some(label),
                    Some(dominance),
                    Some(format!(
                        "anchor embedding dimension {} differs from target dimension {target_dimension}",
                        embedding.len()
                    )),
                ));
                continue;
            }

            let normalized = normalize_embedding(embedding)?;
            identity_samples
                .entry(speaker.key.clone())
                .or_default()
                .push(normalized);
            anchor_diagnostics.push(anchor_diagnostic(
                &speaker.key,
                anchor,
                AnchorEvidenceStatus::Used,
                Some(label),
                Some(dominance),
                None,
            ));
        }
    }

    let centroids = identity_samples
        .iter()
        .map(|(identity, samples)| {
            centroid(samples).map(|centroid| (identity.clone(), centroid, samples.len()))
        })
        .collect::<Result<Vec<_>>>()?;

    let mut labels = BTreeSet::new();
    for segment in &target.segments {
        labels.insert(segment.speaker.clone());
    }
    if let Some(embeddings) = target.speaker_embeddings.as_ref() {
        labels.extend(embeddings.keys().cloned());
    }

    let mut matches = Vec::new();
    for label in labels {
        let Some(embedding) = target.embedding(&label) else {
            matches.push(SpeakerMatch {
                diarization_label: label,
                status: SpeakerMatchStatus::MissingEmbedding,
                best_identity: None,
                similarity: None,
                second_identity: None,
                second_similarity: None,
                margin: None,
                anchor_samples: 0,
            });
            continue;
        };

        if centroids.is_empty() {
            matches.push(SpeakerMatch {
                diarization_label: label,
                status: SpeakerMatchStatus::NoCompatibleAnchors,
                best_identity: None,
                similarity: None,
                second_identity: None,
                second_similarity: None,
                margin: None,
                anchor_samples: 0,
            });
            continue;
        }

        let normalized = normalize_embedding(embedding)?;
        let mut scored = centroids
            .iter()
            .map(|(identity, centroid, sample_count)| {
                (
                    identity.clone(),
                    dot(&normalized, centroid),
                    *sample_count,
                )
            })
            .collect::<Vec<_>>();
        scored.sort_by(|left, right| {
            right
                .1
                .partial_cmp(&left.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.0.cmp(&right.0))
        });

        let (best_identity, best_similarity, best_anchor_samples) = scored[0].clone();
        let second = scored.get(1).cloned();
        let margin = second
            .as_ref()
            .map(|(_, similarity, _)| best_similarity - similarity);

        let status = if best_similarity < config.min_similarity {
            SpeakerMatchStatus::BelowSimilarity
        } else if margin.is_some_and(|margin| margin < config.min_margin) {
            SpeakerMatchStatus::AmbiguousMargin
        } else {
            SpeakerMatchStatus::Matched
        };

        matches.push(SpeakerMatch {
            diarization_label: label,
            status,
            best_identity: Some(best_identity),
            similarity: Some(best_similarity),
            second_identity: second.as_ref().map(|(identity, _, _)| identity.clone()),
            second_similarity: second.as_ref().map(|(_, similarity, _)| *similarity),
            margin,
            anchor_samples: best_anchor_samples,
        });
    }

    Ok(SpeakerMatchReport {
        target_video_id: target_video_id.to_owned(),
        registry_revision: registry.revision,
        evidence_fingerprint,
        diarization_engine: target.diarization.engine,
        diarization_model: target.diarization.model,
        config,
        anchor_samples_used: identity_samples.values().map(Vec::len).sum(),
        identities_with_compatible_anchors: centroids.len(),
        anchor_diagnostics,
        matches,
    })
}

fn model_component<'a>(model: &'a str, key: &str) -> Option<&'a str> {
    model.split(';').find_map(|component| {
        let (component_key, value) = component.split_once('=')?;
        (component_key == key).then_some(value)
    })
}

fn compatible_embedding_provenance(left: &str, right: &str) -> bool {
    match (
        model_component(left, "segmentation"),
        model_component(left, "embedding"),
        model_component(right, "segmentation"),
        model_component(right, "embedding"),
    ) {
        (Some(left_seg), Some(left_emb), Some(right_seg), Some(right_emb)) => {
            if left_seg != right_seg || left_emb != right_emb {
                return false;
            }
            match (
                model_component(left, "embedding_aggregation"),
                model_component(right, "embedding_aggregation"),
            ) {
                (Some(left_aggregation), Some(right_aggregation)) => {
                    left_aggregation == right_aggregation
                }
                (None, None) => true,
                _ => false,
            }
        }
        _ => left == right,
    }
}

fn evidence_path(evidence_dir: &Path, video_id: &str) -> PathBuf {
    evidence_dir.join(format!("{video_id}.json"))
}

fn anchor_diagnostic(
    speaker_key: &str,
    anchor: &crate::SpeakerAnchorV1,
    status: AnchorEvidenceStatus,
    diarization_label: Option<String>,
    dominance: Option<f64>,
    detail: Option<String>,
) -> AnchorEvidenceDiagnostic {
    AnchorEvidenceDiagnostic {
        speaker_key: speaker_key.to_owned(),
        video_id: anchor.video_id.clone(),
        start_seconds: anchor.start_seconds,
        end_seconds: anchor.end_seconds,
        status,
        diarization_label,
        dominance,
        detail,
    }
}

fn dominant_anchor_label(
    evidence: &SpeakerEvidenceV1,
    start_seconds: u64,
    end_seconds: u64,
) -> Option<(String, f64)> {
    let start = start_seconds as f64;
    let end = end_seconds as f64;
    let mut overlap_by_label = BTreeMap::<String, f64>::new();

    for segment in &evidence.segments {
        let overlap_start = segment.start.max(start);
        let overlap_end = segment.end.min(end);
        let overlap = (overlap_end - overlap_start).max(0.0);
        if overlap > 0.0 {
            *overlap_by_label.entry(segment.speaker.clone()).or_default() += overlap;
        }
    }

    let total_overlap = overlap_by_label.values().sum::<f64>();
    if total_overlap <= f64::EPSILON {
        return None;
    }

    let (label, overlap) = overlap_by_label.into_iter().max_by(|left, right| {
        left.1
            .partial_cmp(&right.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| right.0.cmp(&left.0))
    })?;
    Some((label, overlap / total_overlap))
}

fn normalize_embedding(embedding: &[f64]) -> Result<Vec<f64>> {
    let norm = embedding.iter().map(|value| value * value).sum::<f64>().sqrt();
    if !norm.is_finite() || norm <= f64::EPSILON {
        return Err(match_error("speaker embedding has invalid or zero norm"));
    }
    Ok(embedding.iter().map(|value| value / norm).collect())
}

fn centroid(samples: &[Vec<f64>]) -> Result<Vec<f64>> {
    let first = samples
        .first()
        .ok_or_else(|| match_error("cannot build speaker centroid without samples"))?;
    let dimension = first.len();
    let mut mean = vec![0.0; dimension];

    for sample in samples {
        if sample.len() != dimension {
            return Err(match_error(
                "cannot build speaker centroid from mixed embedding dimensions",
            ));
        }
        for (index, value) in sample.iter().enumerate() {
            mean[index] += value;
        }
    }
    for value in &mut mean {
        *value /= samples.len() as f64;
    }
    normalize_embedding(&mean)
}

fn dot(left: &[f64], right: &[f64]) -> f64 {
    left.iter()
        .zip(right)
        .map(|(left, right)| left * right)
        .sum()
}

fn require_nonempty(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(match_error(format!("{field} is required")));
    }
    Ok(())
}

fn match_error(message: impl Into<String>) -> VesselError {
    VesselError::Corpus(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        SpeakerAnchorV1, SpeakerAttribution, SpeakerIdentityV1, SpeakerRegistryV1,
    };

    fn registry() -> SpeakerRegistryV1 {
        SpeakerRegistryV1 {
            schema: 1,
            source_family: "youtube".into(),
            source_id: "UCexample".into(),
            revision: 3,
            speakers: vec![
                SpeakerIdentityV1 {
                    key: "creator".into(),
                    display_name: Some("Creator".into()),
                    relation: Some("creator".into()),
                    anchors: vec![SpeakerAnchorV1 {
                        video_id: "anchor-creator".into(),
                        start_seconds: 0,
                        end_seconds: 8,
                        basis: SpeakerAttribution::HumanConfirmed,
                    }],
                },
                SpeakerIdentityV1 {
                    key: "guest:one".into(),
                    display_name: Some("Guest One".into()),
                    relation: Some("guest".into()),
                    anchors: vec![SpeakerAnchorV1 {
                        video_id: "anchor-guest".into(),
                        start_seconds: 0,
                        end_seconds: 8,
                        basis: SpeakerAttribution::HumanConfirmed,
                    }],
                },
            ],
        }
    }

    fn write_evidence(
        directory: &Path,
        video_id: &str,
        segments: &[(f64, f64, &str)],
        embeddings: &[(&str, &[f64])],
    ) {
        fs::create_dir_all(directory).unwrap();
        let evidence = SpeakerEvidenceV1 {
            schema: 1,
            video_id: video_id.into(),
            asr: SpeakerEvidenceProvenance {
                engine: "whisperx-faster-whisper".into(),
                model: "large-v3".into(),
            },
            diarization: SpeakerEvidenceDiarization {
                engine: "pyannote-audio".into(),
                model: "pyannote/speaker-diarization-community-1".into(),
                label_scope: "file_local".into(),
            },
            segments: segments
                .iter()
                .map(|(start, end, speaker)| SpeakerEvidenceSegment {
                    start: *start,
                    end: *end,
                    speaker: (*speaker).into(),
                })
                .collect(),
            speaker_embeddings: Some(
                embeddings
                    .iter()
                    .map(|(label, embedding)| {
                        ((*label).to_owned(), embedding.to_vec())
                    })
                    .collect(),
            ),
        };
        fs::write(
            evidence_path(directory, video_id),
            serde_json::to_vec_pretty(&evidence).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn embedding_provenance_ignores_clustering_configuration() {
        let old = "segmentation=seg/model.onnx;embedding=emb/model.onnx";
        let tuned =
            "segmentation=seg/model.onnx;embedding=emb/model.onnx;num_speakers=auto;clustering_threshold=0.900000;window_shift_ratio=0.100000;min_duration_on=0.300000;min_duration_off=0.500000";
        let other_embedding =
            "segmentation=seg/model.onnx;embedding=other/model.onnx;num_speakers=auto;clustering_threshold=0.900000";

        assert!(compatible_embedding_provenance(old, tuned));
        assert!(!compatible_embedding_provenance(tuned, other_embedding));
        let chunked =
            "segmentation=seg/model.onnx;embedding=emb/model.onnx;embedding_aggregation=chunk_centroid_v1_3s_16max;clustering_threshold=0.900000";
        assert!(!compatible_embedding_provenance(old, chunked));
        assert!(compatible_embedding_provenance(chunked, chunked));
        assert!(compatible_embedding_provenance("legacy-model", "legacy-model"));
        assert!(!compatible_embedding_provenance("legacy-a", "legacy-b"));
    }

    #[test]
    fn evidence_fingerprint_changes_when_anchor_evidence_changes() {
        let root = std::env::temp_dir().join(format!(
            "vessel-speaker-fingerprint-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let evidence_dir = root.join("speaker-evidence");

        write_evidence(
            &evidence_dir,
            "anchor-creator",
            &[(0.0, 8.0, "SPEAKER_00")],
            &[("SPEAKER_00", &[1.0, 0.0])],
        );
        write_evidence(
            &evidence_dir,
            "anchor-guest",
            &[(0.0, 8.0, "SPEAKER_01")],
            &[("SPEAKER_01", &[0.0, 1.0])],
        );
        write_evidence(
            &evidence_dir,
            "target",
            &[(0.0, 4.0, "SPEAKER_00")],
            &[("SPEAKER_00", &[1.0, 0.0])],
        );

        let before =
            speaker_evidence_fingerprint(&registry(), &evidence_dir, "target").expect("fingerprint");

        write_evidence(
            &evidence_dir,
            "anchor-guest",
            &[(0.0, 8.0, "SPEAKER_01")],
            &[("SPEAKER_01", &[0.1, 0.99])],
        );
        let after =
            speaker_evidence_fingerprint(&registry(), &evidence_dir, "target").expect("fingerprint");

        assert_ne!(before, after);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn evidence_fingerprint_tracks_missing_anchor_state() {
        let root = std::env::temp_dir().join(format!(
            "vessel-speaker-fingerprint-missing-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let evidence_dir = root.join("speaker-evidence");

        write_evidence(
            &evidence_dir,
            "anchor-creator",
            &[(0.0, 8.0, "SPEAKER_00")],
            &[("SPEAKER_00", &[1.0, 0.0])],
        );
        write_evidence(
            &evidence_dir,
            "target",
            &[(0.0, 4.0, "SPEAKER_00")],
            &[("SPEAKER_00", &[1.0, 0.0])],
        );

        let missing =
            speaker_evidence_fingerprint(&registry(), &evidence_dir, "target").expect("fingerprint");
        write_evidence(
            &evidence_dir,
            "anchor-guest",
            &[(0.0, 8.0, "SPEAKER_01")],
            &[("SPEAKER_01", &[0.0, 1.0])],
        );
        let present =
            speaker_evidence_fingerprint(&registry(), &evidence_dir, "target").expect("fingerprint");

        assert_ne!(missing, present);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn matches_target_clusters_against_human_confirmed_anchor_embeddings() {
        let root = std::env::temp_dir().join(format!(
            "vessel-speaker-match-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let evidence_dir = root.join("speaker-evidence");

        write_evidence(
            &evidence_dir,
            "anchor-creator",
            &[(0.0, 8.0, "SPEAKER_00")],
            &[("SPEAKER_00", &[1.0, 0.0])],
        );
        write_evidence(
            &evidence_dir,
            "anchor-guest",
            &[(0.0, 8.0, "SPEAKER_01")],
            &[("SPEAKER_01", &[0.0, 1.0])],
        );
        write_evidence(
            &evidence_dir,
            "target",
            &[
                (0.0, 4.0, "SPEAKER_00"),
                (4.0, 8.0, "SPEAKER_01"),
            ],
            &[
                ("SPEAKER_00", &[0.99, 0.08]),
                ("SPEAKER_01", &[0.05, 0.99]),
            ],
        );

        let report = match_speakers_from_evidence(
            &registry(),
            &evidence_dir,
            "target",
            SpeakerMatchConfig::default(),
        )
        .expect("speaker matching");

        assert_eq!(report.anchor_samples_used, 2);
        assert_eq!(report.identities_with_compatible_anchors, 2);
        assert_eq!(report.matches.len(), 2);

        let first = report
            .matches
            .iter()
            .find(|item| item.diarization_label == "SPEAKER_00")
            .unwrap();
        assert_eq!(first.status, SpeakerMatchStatus::Matched);
        assert_eq!(first.best_identity.as_deref(), Some("creator"));
        assert!(first.similarity.unwrap() > 0.99);

        let second = report
            .matches
            .iter()
            .find(|item| item.diarization_label == "SPEAKER_01")
            .unwrap();
        assert_eq!(second.status, SpeakerMatchStatus::Matched);
        assert_eq!(second.best_identity.as_deref(), Some("guest:one"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ambiguous_similarity_stays_unresolved() {
        let root = std::env::temp_dir().join(format!(
            "vessel-speaker-ambiguous-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let evidence_dir = root.join("speaker-evidence");

        write_evidence(
            &evidence_dir,
            "anchor-creator",
            &[(0.0, 8.0, "SPEAKER_00")],
            &[("SPEAKER_00", &[1.0, 0.0])],
        );
        write_evidence(
            &evidence_dir,
            "anchor-guest",
            &[(0.0, 8.0, "SPEAKER_01")],
            &[("SPEAKER_01", &[0.98, 0.2])],
        );
        write_evidence(
            &evidence_dir,
            "target",
            &[(0.0, 4.0, "SPEAKER_00")],
            &[("SPEAKER_00", &[0.99, 0.1])],
        );

        let report = match_speakers_from_evidence(
            &registry(),
            &evidence_dir,
            "target",
            SpeakerMatchConfig {
                min_similarity: 0.8,
                min_margin: 0.05,
                min_anchor_dominance: 0.8,
            },
        )
        .expect("speaker matching");

        assert_eq!(report.matches[0].status, SpeakerMatchStatus::AmbiguousMargin);
        assert!(report.matches[0].margin.unwrap() < 0.05);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn anchor_must_be_dominated_by_one_diarized_cluster() {
        let root = std::env::temp_dir().join(format!(
            "vessel-speaker-dominance-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let evidence_dir = root.join("speaker-evidence");

        write_evidence(
            &evidence_dir,
            "anchor-creator",
            &[
                (0.0, 4.0, "SPEAKER_00"),
                (4.0, 8.0, "SPEAKER_01"),
            ],
            &[
                ("SPEAKER_00", &[1.0, 0.0]),
                ("SPEAKER_01", &[0.0, 1.0]),
            ],
        );
        write_evidence(
            &evidence_dir,
            "anchor-guest",
            &[(0.0, 8.0, "SPEAKER_01")],
            &[("SPEAKER_01", &[0.0, 1.0])],
        );
        write_evidence(
            &evidence_dir,
            "target",
            &[(0.0, 4.0, "SPEAKER_00")],
            &[("SPEAKER_00", &[1.0, 0.0])],
        );

        let report = match_speakers_from_evidence(
            &registry(),
            &evidence_dir,
            "target",
            SpeakerMatchConfig::default(),
        )
        .expect("speaker matching");

        let creator_anchor = report
            .anchor_diagnostics
            .iter()
            .find(|item| item.speaker_key == "creator")
            .unwrap();
        assert_eq!(creator_anchor.status, AnchorEvidenceStatus::LowDominance);
        assert_eq!(creator_anchor.dominance, Some(0.5));

        let _ = fs::remove_dir_all(root);
    }
}
