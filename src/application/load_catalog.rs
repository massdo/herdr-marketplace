use std::fmt;

use crate::application::ports::{FetchError, Fetcher, HerdrCli};
use crate::domain::index::{Catalog, IndexError, parse_index};
use crate::domain::version::Version;

/// Upper bound for the index body.
pub const INDEX_LIMIT: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedCatalog {
    pub catalog: Catalog,
    /// Installed Herdr version, against which compatibility is judged.
    pub herdr: Version,
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
            Self::HerdrVersion(detail) => write!(f, "version de Herdr indéterminable : {detail}"),
            Self::Fetch(error) => write!(f, "téléchargement de l'index impossible : {error}"),
            Self::Index(error) => write!(f, "{error}"),
        }
    }
}

/// One download, then everything happens in memory.
pub fn load_catalog<F: Fetcher, H: HerdrCli>(
    fetcher: &F,
    herdr: &H,
    url: &str,
) -> Result<LoadedCatalog, LoadError> {
    let output = herdr.version().map_err(LoadError::HerdrVersion)?;
    let herdr = Version::from_herdr_output(&output)
        .ok_or_else(|| LoadError::HerdrVersion(format!("réponse illisible : {}", output.trim())))?;
    let body = fetcher.fetch(url, INDEX_LIMIT).map_err(LoadError::Fetch)?;
    let catalog = parse_index(&body).map_err(LoadError::Index)?;
    Ok(LoadedCatalog { catalog, herdr })
}
