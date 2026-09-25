//! Removal rules: only the plugin installed from the fiche's exact source.

use super::install::installed_from;
use super::registry::InstalledPlugin;
use super::source::PluginSource;

/// What confirming the removal runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovalPlan {
    pub installed: InstalledPlugin,
    pub args: Vec<String>,
}

/// The plugin installed from `source` (owner and repo ignore case, the
/// subdir does not), removed by its source spelled as the registry records
/// it, since `herdr plugin uninstall` compares it with its case. Never by id:
/// a namesake from another source or a locally linked plugin is not touched.
pub fn plan_removal(
    registry: &[InstalledPlugin],
    source: &PluginSource,
) -> Result<RemovalPlan, String> {
    let installed = installed_from(registry, source)?
        .ok_or_else(|| format!("aucun plugin installé depuis {source}"))?;
    let recorded = installed
        .github_source()
        .map(ToString::to_string)
        .unwrap_or_default();
    Ok(RemovalPlan {
        args: vec!["plugin".into(), "uninstall".into(), recorded],
        installed: installed.clone(),
    })
}
