//! Sidebar rows: the catalogue merged with the Herdr registry.

use super::compat::Platform;
use super::index::{Catalog, Entry};
use super::registry::InstalledPlugin;
use super::search::catalog_order;
use super::source::PluginSource;
use super::version::{Version, is_newer};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub entry: Entry,
    /// Registry record of the plugin installed from this source.
    pub installed: Option<InstalledPlugin>,
    pub compatible: bool,
    pub in_catalog: bool,
}

impl Row {
    /// The catalogue's version when it updates the installed plugin: a
    /// compatible entry at another commit, with a newer version. A new
    /// commit alone is no update.
    pub fn update(&self) -> Option<&str> {
        let installed = self.installed.as_ref()?;
        let version = self.entry.version.as_deref()?;
        (self.in_catalog
            && self.compatible
            && installed.resolved_commit() != Some(self.entry.commit.as_str())
            && is_newer(version, &installed.version))
        .then_some(version)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Listing {
    /// In catalogue order.
    pub rows: Vec<Row>,
    /// Incompatible catalogue entries that are not installed, in catalogue
    /// order: hidden, but counted by the search.
    pub hidden: Vec<Entry>,
}

/// Compatible entries and every plugin installed from GitHub, even
/// incompatible or absent from the catalogue. Locally linked plugins are
/// left out.
pub fn build_listing(
    catalog: &Catalog,
    installed: &[InstalledPlugin],
    host: Platform,
    herdr: Version,
) -> Listing {
    let github: Vec<(&InstalledPlugin, &PluginSource)> = installed
        .iter()
        .filter_map(|plugin| plugin.github_source().map(|source| (plugin, source)))
        .collect();
    let mut in_catalog = vec![false; github.len()];
    let mut listing = Listing::default();

    for entry in &catalog.entries {
        let mut installed = None;
        for (index, (plugin, source)) in github.iter().enumerate() {
            if source.same_source(&entry.source) {
                in_catalog[index] = true;
                installed.get_or_insert((*plugin).clone());
            }
        }
        let compatible = entry.is_compatible(host, herdr);
        if installed.is_none() && !compatible {
            listing.hidden.push(entry.clone());
            continue;
        }
        listing.rows.push(Row {
            entry: entry.clone(),
            installed,
            compatible,
            in_catalog: true,
        });
    }

    for ((plugin, source), _) in github
        .into_iter()
        .zip(in_catalog)
        .filter(|(_, in_catalog)| !in_catalog)
    {
        let entry = off_catalog_entry(plugin, source);
        listing.rows.push(Row {
            compatible: entry.is_compatible(host, herdr),
            entry,
            installed: Some(plugin.clone()),
            in_catalog: false,
        });
    }
    listing
        .rows
        .sort_by(|a, b| catalog_order(&a.entry, &b.entry));
    listing
}

/// Identity, id and SHA from the registry; ranked as 0 stars.
fn off_catalog_entry(plugin: &InstalledPlugin, source: &PluginSource) -> Entry {
    Entry {
        source: source.clone(),
        commit: plugin.resolved_commit().unwrap_or_default().to_string(),
        id: plugin.plugin_id.clone(),
        name: plugin.name.clone(),
        version: Some(plugin.version.clone()),
        description: plugin.description.clone(),
        topics: Vec::new(),
        stars: 0,
        platforms: plugin.platforms.clone(),
        min_herdr_version: Some(plugin.min_herdr_version.clone()),
    }
}
