use std::fmt;

use crate::application::ports::{
    CachedIndex, CatalogCache, CatalogFetcher, FetchError, Fetched, HerdrCli,
};
use crate::domain::index::{Catalog, IndexError, parse_index};
use crate::domain::version::Version;

/// Upper bound for the index body.
pub const INDEX_LIMIT: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedCatalog {
    pub catalog: Catalog,
    /// Installed Herdr version, against which compatibility is judged.
    pub herdr: Version,
    /// The index could not be refreshed: this is the saved copy.
    pub not_refreshed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    HerdrVersion(String),
    Fetch(FetchError),
    Index(IndexError),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HerdrVersion(detail) => {
                write!(f, "cannot determine the Herdr version: {detail}")
            }
            Self::Fetch(error) => write!(f, "cannot download the index: {error}"),
            Self::Index(error) => write!(f, "{error}"),
        }
    }
}

/// The saved copy when the index has not changed, else one download; then
/// everything happens in memory. The saved copy also stands in when the
/// index cannot be refreshed.
pub fn load_catalog<F: CatalogFetcher, C: CatalogCache, H: HerdrCli>(
    fetcher: &F,
    cache: &C,
    herdr: &H,
    url: &str,
) -> Result<LoadedCatalog, LoadError> {
    let output = herdr.version().map_err(LoadError::HerdrVersion)?;
    let herdr = Version::from_herdr_output(&output)
        .ok_or_else(|| LoadError::HerdrVersion(format!("unreadable answer: {}", output.trim())))?;
    let loaded = |catalog, not_refreshed| LoadedCatalog {
        catalog,
        herdr,
        not_refreshed,
    };
    // Only a copy of this URL that still reads sends its ETag.
    let saved = cache
        .read()
        .filter(|saved| saved.url == url)
        .and_then(|saved| Some((saved.etag, parse_index(&saved.body).ok()?)));
    let etag = saved.as_ref().map(|(etag, _)| etag.as_str());
    let error = match fetcher.fetch_index(url, etag, INDEX_LIMIT) {
        Ok(Fetched::Unchanged) => match saved {
            Some((_, catalog)) => return Ok(loaded(catalog, false)),
            None => LoadError::Fetch(FetchError::Failed("unchanged, but no saved copy".into())),
        },
        Ok(Fetched::Body { body, etag }) => match parse_index(&body) {
            Ok(catalog) => {
                // Without an ETag, a saved copy would send a stale one.
                match etag {
                    Some(etag) => cache.replace(&CachedIndex {
                        url: url.to_string(),
                        etag,
                        body,
                    }),
                    None => cache.remove(),
                }
                return Ok(loaded(catalog, false));
            }
            Err(error) => LoadError::Index(error),
        },
        Err(error) => LoadError::Fetch(error),
    };
    match saved {
        Some((_, catalog)) => Ok(loaded(catalog, true)),
        None => Err(error),
    }
}
