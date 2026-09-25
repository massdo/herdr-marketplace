use crate::application::load_catalog::{LoadError, load_catalog};
use crate::application::ports::{Fetcher, HerdrCli};
use crate::domain::compat::Platform;
use crate::domain::listing::{Listing, build_listing};
use crate::domain::registry::{InstalledPlugin, parse_registry};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedListing {
    pub listing: Listing,
    /// Set when the registry could not be read: nothing is marked installed.
    pub registry_error: Option<String>,
}

/// What the sidebar loads when it opens: the index, then a fresh registry.
pub fn load_listing<F: Fetcher, H: HerdrCli>(
    fetcher: &F,
    herdr: &H,
    url: &str,
    host: Platform,
) -> Result<LoadedListing, LoadError> {
    let loaded = load_catalog(fetcher, herdr, url)?;
    let (installed, registry_error) = match read_registry(herdr) {
        Ok(installed) => (installed, None),
        Err(error) => (Vec::new(), Some(error)),
    };
    Ok(LoadedListing {
        listing: build_listing(&loaded.catalog, &installed, host, loaded.herdr),
        registry_error,
    })
}

pub fn read_registry<H: HerdrCli>(herdr: &H) -> Result<Vec<InstalledPlugin>, String> {
    parse_registry(&herdr.plugin_list()?)
}
