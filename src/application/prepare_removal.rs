use crate::application::load_listing::read_registry;
use crate::application::ports::HerdrCli;
use crate::domain::source::PluginSource;
use crate::domain::uninstall::{RemovalPlan, plan_removal};

/// The removal a confirmation would run, from a freshly read registry.
/// Incompatible and off-catalogue plugins are removed like the others.
pub fn prepare_removal<H: HerdrCli>(
    herdr: &H,
    source: &PluginSource,
) -> Result<RemovalPlan, String> {
    let registry =
        read_registry(herdr).map_err(|error| format!("registre Herdr illisible : {error}"))?;
    plan_removal(&registry, source)
}
