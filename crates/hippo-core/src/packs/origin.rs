//! `origin.json`: where an installed pack came from.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use super::review::PackReview;
use crate::error::{Error, Result};
use crate::records::{decode, encode, Verdict};
use crate::util::write_atomic;

/// The file name inside a pack directory.
const FILE: &str = "origin.json";

/// Where an installed pack came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    pub name: String,
    pub version: String,
    pub source: OriginSource,
    pub commit: Option<String>,
    pub pack_hash: String,
    /// Every file as installed, except `origin.json`, with its SHA-256.
    pub files: BTreeMap<String, String>,
    pub review: ReviewRef,
    pub registry_review: Option<RegistryReviewRef>,
    #[serde(with = "time::serde::rfc3339")]
    pub added: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

/// The source of a pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum OriginSource {
    Git {
        url: String,
        /// `""` for the repository root.
        path: String,
        /// `None` for the default branch.
        #[serde(rename = "ref")]
        git_ref: Option<String>,
    },
    Path {
        path: PathBuf,
    },
    Registry {
        index: PathBuf,
        name: String,
        repo: String,
        repo_id: u64,
        owner_id: u64,
        path: String,
    },
}

/// The local pack review of the content last installed, with the full
/// record. The record has no `protocol` key of its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRef {
    pub id: String,
    pub verdict: Verdict,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub record: PackReview,
}

/// The registry's review of the same content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryReviewRef {
    pub id: String,
    pub verdict: Verdict,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
}

impl Origin {
    /// Writes `<pack_dir>/origin.json` atomically, `protocol` first.
    pub fn save(&self, pack_dir: &Path) -> Result<()> {
        write_atomic(&pack_dir.join(FILE), &encode(self)?)
    }

    /// Reads `<pack_dir>/origin.json`; `None` when there is none. A
    /// missing or different `protocol` is [`Error::Protocol`].
    pub fn load(pack_dir: &Path) -> Result<Option<Origin>> {
        let path = pack_dir.join(FILE);
        match fs::read(&path) {
            Ok(bytes) => decode(&bytes).map(Some),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(Error::io_at(&path, e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packs::review::persona_hash;

    fn at(seconds: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(seconds).unwrap()
    }

    fn origin(source: OriginSource) -> Origin {
        let record = PackReview {
            id: "01JCA5ZB3K9V7Q2M4N6P8R0T1W".into(),
            pack: "glossary-terms".into(),
            version: "1.0.0".into(),
            source: "src".into(),
            commit: None,
            repo_id: None,
            pack_hash: "b".repeat(64),
            reviewer: "pack-reviewer".into(),
            persona_hash: persona_hash(),
            invocation: "claude -p".into(),
            warnings: vec![],
            verdict: Verdict::Confirm,
            notes: "Fine.".into(),
            findings: vec![],
            at: at(1_794_000_000),
        };
        Origin {
            name: "glossary-terms".into(),
            version: "1.0.0".into(),
            source,
            commit: Some("3f2c1a9e0b4d7c6e5f8a9b0c1d2e3f4a5b6c7d8e".into()),
            pack_hash: "b".repeat(64),
            files: BTreeMap::from([("README.md".to_owned(), "c".repeat(64))]),
            review: ReviewRef {
                id: record.id.clone(),
                verdict: Verdict::Confirm,
                at: record.at,
                record,
            },
            registry_review: None,
            added: at(1_794_000_000),
            updated: at(1_794_000_100),
        }
    }

    #[test]
    fn each_source_kind_round_trips() {
        let sources = [
            OriginSource::Git {
                url: "https://github.com/alice/hippo-packs".into(),
                path: String::new(),
                git_ref: None,
            },
            OriginSource::Git {
                url: "https://github.com/alice/hippo-packs".into(),
                path: "glossary-terms".into(),
                git_ref: Some("v1.0.0".into()),
            },
            OriginSource::Path {
                path: "/tmp/pack".into(),
            },
            OriginSource::Registry {
                index: "/tmp/registry".into(),
                name: "glossary-terms".into(),
                repo: "alice/hippo-packs".into(),
                repo_id: 123,
                owner_id: 456,
                path: "glossary-terms".into(),
            },
        ];
        for source in sources {
            let dir = tempfile::tempdir().unwrap();
            let mut origin = origin(source);
            if matches!(origin.source, OriginSource::Registry { .. }) {
                origin.registry_review = Some(RegistryReviewRef {
                    id: "01JCA5ZB3K9V7Q2M4N6P8R0T1X".into(),
                    verdict: Verdict::Confirm,
                    at: at(1_793_000_000),
                });
            }
            origin.save(dir.path()).unwrap();
            assert_eq!(Origin::load(dir.path()).unwrap(), Some(origin));
        }
    }

    #[test]
    fn the_file_has_the_shape_of_the_design() {
        let dir = tempfile::tempdir().unwrap();
        let git = OriginSource::Git {
            url: "u".into(),
            path: String::new(),
            git_ref: None,
        };
        origin(git).save(dir.path()).unwrap();
        let text = fs::read_to_string(dir.path().join("origin.json")).unwrap();
        assert!(
            text.starts_with("{\n  \"protocol\": \"HIPPO_V1\",\n  \"name\""),
            "{text}"
        );
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            value["source"],
            serde_json::json!({"kind": "git", "url": "u", "path": "", "ref": null})
        );
        assert_eq!(value["registry_review"], serde_json::Value::Null);
        assert_eq!(value["added"], "2026-11-06T21:20:00Z");
        let path = serde_json::to_value(OriginSource::Path { path: "/p".into() }).unwrap();
        assert_eq!(path, serde_json::json!({"kind": "path", "path": "/p"}));
    }

    #[test]
    fn the_nested_record_has_no_protocol_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = OriginSource::Path { path: "/p".into() };
        origin(path).save(dir.path()).unwrap();
        let text = fs::read_to_string(dir.path().join("origin.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(value.get("protocol").is_some());
        let record = value["review"]["record"].as_object().unwrap();
        assert!(!record.contains_key("protocol"));
        assert_eq!(record["pack"], "glossary-terms");
        assert_eq!(text.matches("\"protocol\"").count(), 1);
    }

    #[test]
    fn another_protocol_is_refused_and_no_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Origin::load(dir.path()).unwrap(), None);
        let path = OriginSource::Path { path: "/p".into() };
        origin(path).save(dir.path()).unwrap();
        let file = dir.path().join("origin.json");
        let text = fs::read_to_string(&file)
            .unwrap()
            .replace("HIPPO_V1", "HIPPO_V0");
        fs::write(&file, text).unwrap();
        assert!(matches!(
            Origin::load(dir.path()),
            Err(Error::Protocol { found: Some(found) }) if found == "HIPPO_V0"
        ));
    }
}
