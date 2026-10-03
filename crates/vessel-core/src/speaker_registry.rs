use std::collections::HashSet;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{Result, SpeakerAttribution, VesselError};

pub const SPEAKER_REGISTRY_SCHEMA_V1: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeakerRegistryV1 {
    pub schema: u8,
    pub source_family: String,
    pub source_id: String,
    pub revision: u64,
    #[serde(default)]
    pub speakers: Vec<SpeakerIdentityV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeakerIdentityV1 {
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relation: Option<String>,
    #[serde(default)]
    pub anchors: Vec<SpeakerAnchorV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeakerAnchorV1 {
    pub video_id: String,
    pub start_seconds: u64,
    pub end_seconds: u64,
    pub basis: SpeakerAttribution,
}

impl SpeakerRegistryV1 {
    pub fn validate(&self) -> Result<()> {
        if self.schema != SPEAKER_REGISTRY_SCHEMA_V1 {
            return Err(registry_error(format!(
                "unsupported speaker registry schema {}; expected {}",
                self.schema, SPEAKER_REGISTRY_SCHEMA_V1
            )));
        }
        require_nonempty("source_family", &self.source_family)?;
        require_nonempty("source_id", &self.source_id)?;
        if self.revision == 0 {
            return Err(registry_error("speaker registry revision must be at least 1"));
        }

        let mut speaker_keys = HashSet::new();
        let mut anchors = HashSet::new();
        for speaker in &self.speakers {
            require_key("speaker key", &speaker.key)?;
            if !speaker_keys.insert(speaker.key.clone()) {
                return Err(registry_error(format!(
                    "duplicate speaker key {:?}",
                    speaker.key
                )));
            }
            validate_optional_nonempty("speaker display_name", speaker.display_name.as_deref())?;
            validate_optional_nonempty("speaker relation", speaker.relation.as_deref())?;

            for anchor in &speaker.anchors {
                require_nonempty("speaker anchor video_id", &anchor.video_id)?;
                if anchor.start_seconds >= anchor.end_seconds {
                    return Err(registry_error(format!(
                        "speaker anchor {}:{}-{} must have start < end",
                        anchor.video_id, anchor.start_seconds, anchor.end_seconds
                    )));
                }
                if anchor.basis == SpeakerAttribution::Unresolved {
                    return Err(registry_error(
                        "speaker registry anchors cannot have unresolved attribution",
                    ));
                }
                let identity = (
                    speaker.key.clone(),
                    anchor.video_id.clone(),
                    anchor.start_seconds,
                    anchor.end_seconds,
                );
                if !anchors.insert(identity) {
                    return Err(registry_error(format!(
                        "duplicate speaker anchor for {:?} in {}:{}-{}",
                        speaker.key, anchor.video_id, anchor.start_seconds, anchor.end_seconds
                    )));
                }
            }
        }

        Ok(())
    }

    pub fn add_speaker(
        &mut self,
        key: &str,
        display_name: Option<&str>,
        relation: Option<&str>,
    ) -> Result<()> {
        require_key("speaker key", key)?;
        validate_optional_nonempty("speaker display_name", display_name)?;
        validate_optional_nonempty("speaker relation", relation)?;
        if self.speakers.iter().any(|speaker| speaker.key == key) {
            return Err(registry_error(format!("duplicate speaker key {key:?}")));
        }

        self.speakers.push(SpeakerIdentityV1 {
            key: key.to_owned(),
            display_name: display_name.map(str::to_owned),
            relation: relation.map(str::to_owned),
            anchors: Vec::new(),
        });
        self.revision = self.revision.saturating_add(1);
        self.validate()
    }

    pub fn add_human_anchor(
        &mut self,
        speaker_key: &str,
        video_id: &str,
        start_seconds: u64,
        end_seconds: u64,
    ) -> Result<()> {
        if start_seconds >= end_seconds {
            return Err(registry_error("speaker anchor start must be before end"));
        }
        let speaker = self
            .speakers
            .iter_mut()
            .find(|speaker| speaker.key == speaker_key)
            .ok_or_else(|| registry_error(format!("unknown speaker key {speaker_key:?}")))?;

        let anchor = SpeakerAnchorV1 {
            video_id: video_id.to_owned(),
            start_seconds,
            end_seconds,
            basis: SpeakerAttribution::HumanConfirmed,
        };
        if !speaker.anchors.contains(&anchor) {
            speaker.anchors.push(anchor);
            speaker.anchors.sort_by(|left, right| {
                (&left.video_id, left.start_seconds, left.end_seconds).cmp(&(
                    &right.video_id,
                    right.start_seconds,
                    right.end_seconds,
                ))
            });
            self.revision = self.revision.saturating_add(1);
        }
        self.validate()
    }
}

pub fn load_speaker_registry(path: &Path) -> Result<Option<SpeakerRegistryV1>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path)?;
    let registry: SpeakerRegistryV1 = toml::from_str(&raw).map_err(|error| {
        registry_error(format!("{}: invalid speaker registry TOML: {error}", path.display()))
    })?;
    registry.validate()?;
    Ok(Some(registry))
}

pub fn write_speaker_registry(path: &Path, registry: &SpeakerRegistryV1) -> Result<()> {
    registry.validate()?;
    let parent = path.parent().ok_or_else(|| {
        registry_error(format!("speaker registry path has no parent: {}", path.display()))
    })?;
    fs::create_dir_all(parent)?;
    let mut rendered = toml::to_string_pretty(registry)
        .map_err(|error| registry_error(format!("speaker registry serialization failed: {error}")))?;
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    let temp = path.with_extension("toml.tmp");
    fs::write(&temp, rendered.as_bytes())?;
    fs::rename(&temp, path)?;
    Ok(())
}

fn require_nonempty(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(registry_error(format!("{field} is required")));
    }
    Ok(())
}

fn validate_optional_nonempty(field: &str, value: Option<&str>) -> Result<()> {
    if value.is_some_and(|value| value.trim().is_empty()) {
        return Err(registry_error(format!("{field} must not be empty when present")));
    }
    Ok(())
}

fn require_key(field: &str, value: &str) -> Result<()> {
    require_nonempty(field, value)?;
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.'))
    {
        return Err(registry_error(format!(
            "{field} must use only ASCII letters, digits, '-', '_', ':', or '.'"
        )));
    }
    Ok(())
}

fn registry_error(message: impl Into<String>) -> VesselError {
    VesselError::Corpus(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> SpeakerRegistryV1 {
        SpeakerRegistryV1 {
            schema: 1,
            source_family: "youtube".into(),
            source_id: "UCexample".into(),
            revision: 1,
            speakers: vec![SpeakerIdentityV1 {
                key: "creator".into(),
                display_name: Some("Example Creator".into()),
                relation: Some("creator".into()),
                anchors: Vec::new(),
            }],
        }
    }

    #[test]
    fn adding_speaker_advances_revision_and_rejects_duplicates() {
        let mut registry = registry();
        registry
            .add_speaker("guest:abigail_thorn", Some("Abigail Thorn"), Some("guest"))
            .expect("add speaker");
        assert_eq!(registry.revision, 2);
        assert_eq!(registry.speakers.len(), 2);
        assert_eq!(registry.speakers[1].key, "guest:abigail_thorn");
        assert_eq!(
            registry.speakers[1].display_name.as_deref(),
            Some("Abigail Thorn")
        );
        assert_eq!(registry.speakers[1].relation.as_deref(), Some("guest"));
        assert!(
            registry
                .add_speaker("guest:abigail_thorn", Some("Abigail Thorn"), Some("guest"))
                .is_err()
        );
    }

    #[test]
    fn human_anchor_advances_revision_and_round_trips() {
        let mut registry = registry();
        registry
            .add_human_anchor("creator", "abc123", 12, 48)
            .expect("anchor");
        assert_eq!(registry.revision, 2);
        assert_eq!(
            registry.speakers[0].anchors[0].basis,
            SpeakerAttribution::HumanConfirmed
        );

        let rendered = toml::to_string_pretty(&registry).expect("serialize");
        let reparsed: SpeakerRegistryV1 = toml::from_str(&rendered).expect("parse");
        assert_eq!(reparsed, registry);
    }

    #[test]
    fn unresolved_anchor_is_rejected() {
        let mut registry = registry();
        registry.speakers[0].anchors.push(SpeakerAnchorV1 {
            video_id: "abc123".into(),
            start_seconds: 1,
            end_seconds: 2,
            basis: SpeakerAttribution::Unresolved,
        });
        assert!(registry.validate().is_err());
    }

    #[test]
    fn duplicate_speaker_keys_are_rejected() {
        let mut registry = registry();
        registry.speakers.push(registry.speakers[0].clone());
        assert!(registry.validate().is_err());
    }
}
