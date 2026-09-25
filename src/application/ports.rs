use std::fmt;

/// Downloads. `file://` URLs are served from disk, which the tests use.
pub trait Fetcher {
    /// Body of `url`, refused beyond `limit` bytes.
    fn fetch(&self, url: &str, limit: u64) -> Result<Vec<u8>, FetchError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// HTTP 404 or missing file.
    NotFound,
    Failed(String),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "introuvable"),
            Self::Failed(detail) => write!(f, "{detail}"),
        }
    }
}

/// The `herdr` command line.
pub trait HerdrCli {
    /// Standard output of `herdr --version`.
    fn version(&self) -> Result<String, String>;
}
