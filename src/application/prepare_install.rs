use crate::application::load_listing::read_registry;
use crate::application::load_readme::raw_url;
use crate::application::ports::{FetchError, Fetcher, HerdrCli};
use crate::domain::compat::Platform;
use crate::domain::details::DetailsTarget;
use crate::domain::install::{Plan, check_target, install_args, installed_from, plan};
use crate::domain::manifest::{Manifest, parse_manifest};
use crate::domain::registry::InstalledPlugin;
use crate::domain::source::PluginSource;
use crate::domain::version::Version;

/// Upper bound for a manifest body.
pub const MANIFEST_LIMIT: u64 = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prepared {
    /// Same source, same commit: already installed, nothing to do.
    UpToDate,
    Preview(Box<InstallPreview>),
    Refused(String),
}

/// Everything shown before the confirmation, and the request it confirms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPreview {
    pub manifest: Manifest,
    pub source: PluginSource,
    pub commit: String,
    pub plan: Plan,
    /// Registry record of the plugin a switch replaces.
    pub replaces: Option<InstalledPlugin>,
    pub args: Vec<String>,
}

/// Builds the preview from the manifest read at the commit shown and a
/// freshly read registry. Runs nothing.
pub fn prepare_install<F: Fetcher, H: HerdrCli>(
    fetcher: &F,
    herdr: &H,
    target: &DetailsTarget,
    host: Platform,
) -> Prepared {
    match preview(fetcher, herdr, target, host) {
        Ok(Some(preview)) => Prepared::Preview(Box::new(preview)),
        Ok(None) => Prepared::UpToDate,
        Err(reason) => Prepared::Refused(reason),
    }
}

fn preview<F: Fetcher, H: HerdrCli>(
    fetcher: &F,
    herdr: &H,
    target: &DetailsTarget,
    host: Platform,
) -> Result<Option<InstallPreview>, String> {
    check_target(target)?;
    let herdr_version = herdr
        .version()
        .ok()
        .and_then(|output| Version::from_herdr_output(&output))
        .ok_or("cannot determine the Herdr version")?;
    let registry =
        read_registry(herdr).map_err(|error| format!("unreadable Herdr registry: {error}"))?;
    let installed = installed_from(&registry, &target.source)?.cloned();
    if installed
        .as_ref()
        .and_then(InstalledPlugin::resolved_commit)
        == Some(target.commit.as_str())
    {
        return Ok(None);
    }
    let manifest = read_manifest(fetcher, target)?;
    let plan = plan(target, &manifest, &registry, host, herdr_version)?;
    Ok(Some(InstallPreview {
        replaces: installed.filter(|_| matches!(plan, Plan::Switch { .. })),
        args: install_args(&target.source, &target.commit),
        source: target.source.clone(),
        commit: target.commit.clone(),
        manifest,
        plan,
    }))
}

fn read_manifest<F: Fetcher>(fetcher: &F, target: &DetailsTarget) -> Result<Manifest, String> {
    let url = raw_url(
        &target.source,
        &target.commit,
        &target.source.subdir,
        "herdr-plugin.toml",
    );
    let body = fetcher
        .fetch(&url, MANIFEST_LIMIT)
        .map_err(|error| match error {
            FetchError::NotFound => "herdr-plugin.toml not found at this commit".to_string(),
            FetchError::Failed(detail) => format!("network error: {detail}"),
        })?;
    parse_manifest(&String::from_utf8_lossy(&body))
        .map_err(|error| format!("invalid herdr-plugin.toml: {error}"))
}
