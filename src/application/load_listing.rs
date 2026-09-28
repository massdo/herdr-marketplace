use crate::application::load_catalog::{LoadError, LoadedCatalog, load_catalog};
use crate::application::ports::{CatalogCache, CatalogFetcher, HerdrCli};
use crate::domain::compat::Platform;
use crate::domain::listing::{Listing, build_listing};
use crate::domain::registry::{InstalledPlugin, parse_registry};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedListing {
    pub loaded: LoadedCatalog,
    pub listing: Listing,
    /// Set when the registry could not be read: nothing is marked installed.
    pub registry_error: Option<String>,
}

impl LoadedListing {
    pub fn new(
        loaded: LoadedCatalog,
        registry: Result<Vec<InstalledPlugin>, String>,
        host: Platform,
    ) -> Self {
        let mut listing = Self {
            loaded,
            listing: Listing::default(),
            registry_error: None,
        };
        listing.use_registry(registry, host);
        listing
    }

    /// Rows rebuilt from the same catalogue and a registry read again.
    pub fn use_registry(&mut self, registry: Result<Vec<InstalledPlugin>, String>, host: Platform) {
        let (installed, error) = match registry {
            Ok(installed) => (installed, None),
            Err(error) => (Vec::new(), Some(error)),
        };
        self.listing = build_listing(&self.loaded.catalog, &installed, host, self.loaded.herdr);
        self.registry_error = error;
    }
}

/// What the sidebar loads when it opens: the index, then a fresh registry.
pub fn load_listing<F: CatalogFetcher, C: CatalogCache, H: HerdrCli>(
    fetcher: &F,
    cache: &C,
    herdr: &H,
    url: &str,
    host: Platform,
) -> Result<LoadedListing, LoadError> {
    let loaded = load_catalog(fetcher, cache, herdr, url)?;
    Ok(LoadedListing::new(loaded, read_registry(herdr), host))
}

pub fn read_registry<H: HerdrCli>(herdr: &H) -> Result<Vec<InstalledPlugin>, String> {
    parse_registry(&herdr.plugin_list()?)
}
