//! Public plugin index, schemaVersion 1. The index lists repositories; the
//! catalogue keeps one entry per manifest, because a repository can carry
//! several plugins.

use std::fmt;

use serde::Deserialize;
use serde_json::Value;

use super::compat::{Platform, is_compatible};
use super::search::catalog_order;
use super::source::{PluginSource, is_github_segment, is_subdir_segment};
use super::version::Version;

pub const SCHEMA_VERSION: u64 = 1;
const MANIFEST_FILE: &str = "herdr-plugin.toml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub source: PluginSource,
    /// Full `headCommit` SHA of the repository: the commit read, then installed.
    pub commit: String,
    pub id: String,
    /// Manifest name, or the id when the name is missing.
    pub name: String,
    pub version: Option<String>,
    /// Manifest description, or the repository description.
    pub description: Option<String>,
    pub topics: Vec<String>,
    pub stars: u64,
    pub platforms: Option<Vec<String>>,
    pub min_herdr_version: Option<String>,
}

impl Entry {
    pub fn is_compatible(&self, host: Platform, herdr: Version) -> bool {
        is_compatible(
            self.platforms.as_deref(),
            self.min_herdr_version.as_deref(),
            host,
            herdr,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    /// Kept entries, in catalogue order.
    pub entries: Vec<Entry>,
    /// Manifests read: `entries.len() + rejected`.
    pub read: usize,
    pub rejected: usize,
    /// `pluginCount` announced by the index.
    pub plugin_count: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexError {
    InvalidJson(String),
    UnknownSchema(String),
    MissingPlugins,
}

impl fmt::Display for IndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson(detail) => write!(f, "index illisible : JSON invalide ({detail})"),
            Self::UnknownSchema(version) => {
                write!(f, "index illisible : schemaVersion inconnu ({version})")
            }
            Self::MissingPlugins => write!(f, "index illisible : liste plugins absente"),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawRepository {
    owner: Option<String>,
    name: Option<String>,
    head_commit: Option<String>,
    stars: Option<u64>,
    topics: Option<Vec<String>>,
    description: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawManifest {
    path: Option<String>,
    id: Option<String>,
    name: Option<String>,
    version: Option<String>,
    description: Option<String>,
    platforms: Option<Vec<String>>,
    min_herdr_version: Option<String>,
}

/// Whole index or an error: never a partial list. A manifest whose
/// repository or own fields are invalid is rejected and counted; the others
/// stay usable.
pub fn parse_index(bytes: &[u8]) -> Result<Catalog, IndexError> {
    let root: Value = serde_json::from_slice(bytes)
        .map_err(|error| IndexError::InvalidJson(error.to_string()))?;
    match root.get("schemaVersion") {
        Some(version) if version.as_u64() == Some(SCHEMA_VERSION) => {}
        Some(version) => return Err(IndexError::UnknownSchema(version.to_string())),
        None => return Err(IndexError::UnknownSchema("absent".into())),
    }
    let repositories = root
        .get("plugins")
        .and_then(Value::as_array)
        .ok_or(IndexError::MissingPlugins)?;

    let mut entries = Vec::new();
    let mut read = 0;
    for repository in repositories {
        let Some(manifests) = repository.get("manifests").and_then(Value::as_array) else {
            continue;
        };
        let raw_repository = RawRepository::deserialize(repository).ok();
        for manifest in manifests {
            read += 1;
            let raw_manifest = RawManifest::deserialize(manifest).ok();
            if let Some(entry) = raw_repository
                .as_ref()
                .zip(raw_manifest)
                .and_then(|(repository, manifest)| entry(repository, manifest))
            {
                entries.push(entry);
            }
        }
    }
    entries.sort_by(catalog_order);
    Ok(Catalog {
        rejected: read - entries.len(),
        entries,
        read,
        plugin_count: root.get("pluginCount").and_then(Value::as_u64),
    })
}

fn entry(repository: &RawRepository, manifest: RawManifest) -> Option<Entry> {
    // Owner and repo end up in URLs and in `herdr plugin install`: they must
    // pass Herdr's own rules.
    let owner = repository
        .owner
        .clone()
        .filter(|owner| is_github_segment(owner))?;
    let repo = repository
        .name
        .clone()
        .filter(|repo| is_github_segment(repo))?;
    let commit = repository
        .head_commit
        .clone()
        .filter(|sha| is_full_sha(sha))?;
    let subdir = manifest_subdir(manifest.path.as_deref()?)?;
    let id = non_empty(manifest.id)?;
    Some(Entry {
        source: PluginSource {
            owner,
            repo,
            subdir,
        },
        commit,
        name: non_empty(manifest.name).unwrap_or_else(|| id.clone()),
        id,
        version: non_empty(manifest.version),
        description: non_empty(manifest.description)
            .or_else(|| non_empty(repository.description.clone())),
        topics: repository.topics.clone().unwrap_or_default(),
        stars: repository.stars.unwrap_or(0),
        platforms: manifest.platforms,
        min_herdr_version: manifest.min_herdr_version,
    })
}

/// 40 lowercase hexadecimal characters.
pub fn is_full_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Folder of the manifest in the repository, empty at the root.
fn manifest_subdir(path: &str) -> Option<String> {
    if path == MANIFEST_FILE {
        return Some(String::new());
    }
    let subdir = path.strip_suffix(MANIFEST_FILE)?.strip_suffix('/')?;
    subdir
        .split('/')
        .all(is_subdir_segment)
        .then(|| subdir.to_string())
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}
