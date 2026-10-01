use crate::application::ports::{Fetcher, HerdrCli};
use crate::application::prepare_install::{InstallPreview, Prepared, prepare_install};
use crate::domain::compat::Platform;
use crate::domain::details::DetailsTarget;
use crate::domain::install::Plan;
use crate::domain::version::is_newer;

/// The checks of the install preview, then two more: an update only replaces
/// the plugin installed from this source, with a newer version. The preview
/// is never shown; it is the request an update confirms. Runs nothing.
pub fn prepare_update<F: Fetcher, H: HerdrCli>(
    fetcher: &F,
    herdr: &H,
    target: &DetailsTarget,
    host: Platform,
) -> Result<InstallPreview, String> {
    let preview = match prepare_install(fetcher, herdr, target, host) {
        Prepared::Refused(reason) => return Err(reason),
        Prepared::UpToDate => return Err("already installed at this commit".into()),
        Prepared::Preview(preview) => preview,
    };
    if preview.plan == Plan::Install {
        return Err("not installed from this source".into());
    }
    let installed = preview
        .replaces
        .as_ref()
        .map_or("", |plugin| plugin.version.as_str());
    if !is_newer(&preview.manifest.version, installed) {
        return Err(format!(
            "the manifest at this commit declares {}, not newer than the installed {installed}",
            preview.manifest.version
        ));
    }
    Ok(*preview)
}
