use std::fs;
use std::path::{Path, PathBuf};

use crate::{Result, VesselError, YoutubeSourcePolicyV1};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceariumYoutubeSource {
    pub source_dir: PathBuf,
    pub policy_path: PathBuf,
    pub policy: YoutubeSourcePolicyV1,
}

pub fn discover_youtube_sources(sourcearium_root: &Path) -> Result<Vec<SourceariumYoutubeSource>> {
    let youtube_root = sourcearium_root.join("sources").join("youtube");
    if !youtube_root.exists() {
        return Ok(Vec::new());
    }
    if !youtube_root.is_dir() {
        return Err(corpus_error(format!(
            "Sourcearium YouTube root is not a directory: {}",
            youtube_root.display()
        )));
    }

    let mut source_dirs = fs::read_dir(&youtube_root)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    source_dirs.retain(|path| path.is_dir());
    source_dirs.sort();

    let mut sources = Vec::new();
    for source_dir in source_dirs {
        let policy_path = source_dir.join("source.toml");
        if !policy_path.exists() {
            continue;
        }

        let dir_key = source_dir
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| {
                corpus_error(format!(
                    "Sourcearium source directory is not valid UTF-8: {}",
                    source_dir.display()
                ))
            })?;

        let raw = fs::read_to_string(&policy_path)?;
        let policy = YoutubeSourcePolicyV1::parse_toml(&raw).map_err(|error| {
            corpus_error(format!("{}: {error}", policy_path.display()))
        })?;

        if policy.source_key != dir_key {
            return Err(corpus_error(format!(
                "{} declares source_key {:?}, but directory key is {:?}",
                policy_path.display(),
                policy.source_key,
                dir_key
            )));
        }

        sources.push(SourceariumYoutubeSource {
            source_dir,
            policy_path,
            policy,
        });
    }

    Ok(sources)
}

fn corpus_error(message: impl Into<String>) -> VesselError {
    VesselError::Corpus(message.into())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use uuid::Uuid;

    use super::*;

    fn temp_sourcearium() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "vessel-sourcearium-test-{}",
            Uuid::new_v4()
        ));
        fs::create_dir_all(root.join("sources").join("youtube")).expect("create source root");
        root
    }

    fn write_policy(root: &Path, key: &str, declared_key: &str) {
        let dir = root.join("sources").join("youtube").join(key);
        fs::create_dir_all(&dir).expect("create source dir");
        fs::write(
            dir.join("source.toml"),
            format!(
                r#"schema = 1
family = "youtube"
source_key = "{declared_key}"

[channel]
input = "https://www.youtube.com/@{key}"
"#
            ),
        )
        .expect("write policy");
    }

    #[test]
    fn discovers_sources_in_deterministic_order() {
        let root = temp_sourcearium();
        write_policy(&root, "zeta", "zeta");
        write_policy(&root, "alpha", "alpha");

        let sources = discover_youtube_sources(&root).expect("discover");
        let keys = sources
            .iter()
            .map(|source| source.policy.source_key.as_str())
            .collect::<Vec<_>>();
        assert_eq!(keys, vec!["alpha", "zeta"]);

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn ignores_directories_without_source_policy() {
        let root = temp_sourcearium();
        fs::create_dir_all(root.join("sources").join("youtube").join("notes"))
            .expect("create notes dir");
        write_policy(&root, "alpha", "alpha");

        let sources = discover_youtube_sources(&root).expect("discover");
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].policy.source_key, "alpha");

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn rejects_source_key_directory_mismatch() {
        let root = temp_sourcearium();
        write_policy(&root, "folder-key", "different-key");

        let error = discover_youtube_sources(&root).expect_err("mismatch must fail");
        assert!(error.to_string().contains("directory key"));

        fs::remove_dir_all(root).expect("cleanup");
    }
}
