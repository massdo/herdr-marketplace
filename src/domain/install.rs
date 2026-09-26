//! Install rules: which requests the marketplace accepts, and the exact
//! Herdr command a confirmed request becomes.

use serde::{Deserialize, Serialize};

use super::compat::{Platform, is_compatible};
use super::details::DetailsTarget;
use super::index::is_full_sha;
use super::manifest::Manifest;
use super::registry::{InstalledPlugin, InstalledSource};
use super::source::{PluginSource, is_github_segment, is_subdir_segment};
use super::version::Version;

/// What confirming the preview does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Plan {
    Install,
    /// The same source is installed at `from`: move it to the commit shown.
    Switch {
        from: String,
    },
}

/// `herdr plugin install <owner>/<repo>[/<subdir>] --ref <SHA> --yes`, one
/// argument per element, never through a shell.
pub fn install_args(source: &PluginSource, commit: &str) -> Vec<String> {
    vec![
        "plugin".into(),
        "install".into(),
        source.to_string(),
        "--ref".into(),
        commit.into(),
        "--yes".into(),
    ]
}

/// Refusals known before reading the registry or the manifest.
pub fn check_target(target: &DetailsTarget) -> Result<(), String> {
    let source = &target.source;
    let subdir_ok = source.subdir.is_empty() || source.subdir.split('/').all(is_subdir_segment);
    if !is_github_segment(&source.owner) || !is_github_segment(&source.repo) || !subdir_ok {
        return Err(format!("source rejected by Herdr's rules: {source}"));
    }
    if !is_full_sha(&target.commit) {
        return Err(format!("invalid SHA: {}", target.commit));
    }
    if !target.in_catalog {
        return Err("plugin not in the catalog: it can only be removed".into());
    }
    if !target.compatible {
        return Err("incompatible plugin: it can only be removed".into());
    }
    Ok(())
}

/// The plugin installed from exactly this source, if any. Owner and repo
/// ignore case; the subdir does not.
pub fn installed_from<'a>(
    registry: &'a [InstalledPlugin],
    source: &PluginSource,
) -> Result<Option<&'a InstalledPlugin>, String> {
    let found: Vec<&InstalledPlugin> = registry
        .iter()
        .filter(|plugin| {
            plugin
                .github_source()
                .is_some_and(|installed| installed.same_source(source))
        })
        .collect();
    match found.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(one)),
        _ => Err(format!("several installed plugins match {source}")),
    }
}

/// Decision once the manifest has been read at the commit shown. Herdr
/// would silently replace a plugin of the same id from another source, or
/// refuse a locally linked one: the marketplace refuses both.
pub fn plan(
    target: &DetailsTarget,
    manifest: &Manifest,
    registry: &[InstalledPlugin],
    host: Platform,
    herdr: Version,
) -> Result<Plan, String> {
    if manifest.id != target.id.trim() {
        return Err(format!(
            "the manifest at this commit declares id {}, the index announces {}",
            manifest.id, target.id
        ));
    }
    if !is_compatible(
        manifest.platforms.as_deref(),
        Some(&manifest.min_herdr_version),
        host,
        herdr,
    ) {
        return Err(format!(
            "incompatible plugin: platforms {:?}, Herdr {} or newer",
            manifest.platforms.as_deref().unwrap_or_default(),
            manifest.min_herdr_version
        ));
    }
    if let Some(installed) = installed_from(registry, &target.source)? {
        if installed.plugin_id != manifest.id {
            return Err(format!(
                "{} is installed as {}, the manifest at this commit declares {}",
                target.source, installed.plugin_id, manifest.id
            ));
        }
        return Ok(Plan::Switch {
            from: installed.resolved_commit().unwrap_or_default().to_string(),
        });
    }
    match registry
        .iter()
        .find(|plugin| plugin.plugin_id == manifest.id)
        .map(|plugin| &plugin.source)
    {
        Some(InstalledSource::Local) => Err(format!(
            "id {} is already linked locally: Herdr would refuse the install",
            manifest.id
        )),
        Some(InstalledSource::Github { source, .. }) => Err(format!(
            "id {} is already installed from {source}: Herdr would replace it",
            manifest.id
        )),
        None => Ok(Plan::Install),
    }
}
