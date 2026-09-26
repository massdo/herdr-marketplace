use serde::{Deserialize, Serialize};

use super::listing::Row;
use super::source::PluginSource;

/// What a details pane shows, handed over when it opens. The pane keeps it
/// even if the sidebar closes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetailsTarget {
    pub source: PluginSource,
    /// Commit whose README is shown.
    pub commit: String,
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub in_catalog: bool,
    pub compatible: bool,
}

impl DetailsTarget {
    /// The indexed commit, except for a plugin installed but incompatible or
    /// off the catalogue: that one is shown at its installed commit.
    pub fn from_row(row: &Row) -> Self {
        let entry = &row.entry;
        let installed = row
            .installed
            .as_ref()
            .filter(|_| !row.compatible || !row.in_catalog);
        match installed {
            Some(plugin) => Self {
                source: entry.source.clone(),
                commit: plugin
                    .resolved_commit()
                    .unwrap_or(&entry.commit)
                    .to_string(),
                id: plugin.plugin_id.clone(),
                name: plugin.name.clone(),
                version: Some(plugin.version.clone()),
                in_catalog: row.in_catalog,
                compatible: row.compatible,
            },
            None => Self {
                source: entry.source.clone(),
                commit: entry.commit.clone(),
                id: entry.id.clone(),
                name: entry.name.clone(),
                version: entry.version.clone(),
                in_catalog: row.in_catalog,
                compatible: row.compatible,
            },
        }
    }
}
