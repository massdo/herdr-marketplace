//! Herdr plugin registry, as printed by `herdr plugin list --json`.

use serde::Deserialize;

use super::source::PluginSource;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPlugin {
    pub plugin_id: String,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub platforms: Option<Vec<String>>,
    pub min_herdr_version: String,
    pub source: InstalledSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstalledSource {
    Local,
    /// Owner, repo and subdir exactly as Herdr recorded them.
    Github {
        source: PluginSource,
        resolved_commit: Option<String>,
    },
}

impl InstalledPlugin {
    pub fn github_source(&self) -> Option<&PluginSource> {
        match &self.source {
            InstalledSource::Github { source, .. } => Some(source),
            InstalledSource::Local => None,
        }
    }

    pub fn resolved_commit(&self) -> Option<&str> {
        match &self.source {
            InstalledSource::Github {
                resolved_commit, ..
            } => resolved_commit.as_deref(),
            InstalledSource::Local => None,
        }
    }
}

#[derive(Deserialize)]
struct Response {
    result: Option<ListResult>,
    error: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct ListResult {
    plugins: Vec<RawPlugin>,
}

#[derive(Deserialize)]
struct RawPlugin {
    plugin_id: String,
    name: String,
    version: String,
    description: Option<String>,
    platforms: Option<Vec<String>>,
    #[serde(default)]
    min_herdr_version: String,
    #[serde(default)]
    source: RawSource,
}

#[derive(Deserialize, Default)]
struct RawSource {
    kind: Option<String>,
    owner: Option<String>,
    repo: Option<String>,
    subdir: Option<String>,
    resolved_commit: Option<String>,
}

/// Every installed plugin, or why the registry cannot be read.
pub fn parse_registry(json: &str) -> Result<Vec<InstalledPlugin>, String> {
    let response: Response = serde_json::from_str(json)
        .map_err(|error| format!("unreadable answer from herdr plugin list: {error}"))?;
    if let Some(error) = response.error {
        return Err(format!("herdr plugin list answered an error: {error}"));
    }
    let result = response
        .result
        .ok_or("herdr plugin list answered no result")?;
    result.plugins.into_iter().map(installed).collect()
}

fn installed(raw: RawPlugin) -> Result<InstalledPlugin, String> {
    let source = match raw.source.kind.as_deref() {
        None | Some("local") => InstalledSource::Local,
        Some("github") => match (raw.source.owner, raw.source.repo) {
            (Some(owner), Some(repo)) => InstalledSource::Github {
                source: PluginSource {
                    owner,
                    repo,
                    subdir: raw.source.subdir.unwrap_or_default(),
                },
                resolved_commit: raw.source.resolved_commit,
            },
            _ => {
                return Err(format!(
                    "plugin {} installed from GitHub without owner/repo",
                    raw.plugin_id
                ));
            }
        },
        Some(other) => return Err(format!("unknown plugin source: {other}")),
    };
    Ok(InstalledPlugin {
        plugin_id: raw.plugin_id,
        name: raw.name,
        version: raw.version,
        description: raw.description,
        platforms: raw.platforms,
        min_herdr_version: raw.min_herdr_version,
        source,
    })
}
