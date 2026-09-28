use std::io::Read;
use std::os::unix::ffi::OsStringExt;
use std::path::Path;
use std::time::Duration;

use crate::application::ports::{CatalogFetcher, FetchError, Fetched, Fetcher};

const TIMEOUT: Duration = Duration::from_secs(30);

/// http(s) through ureq, `file://` from disk.
pub struct HttpFetcher {
    agent: ureq::Agent,
}

impl HttpFetcher {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .user_agent(concat!("herdr-marketplace/", env!("CARGO_PKG_VERSION")))
            .build();
        Self {
            agent: config.into(),
        }
    }
}

impl Default for HttpFetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Fetcher for HttpFetcher {
    fn fetch(&self, url: &str, limit: u64) -> Result<Vec<u8>, FetchError> {
        if let Some(path) = url.strip_prefix("file://") {
            let path = path
                .strip_prefix("localhost/")
                .map_or_else(|| path.to_string(), |path| format!("/{path}"));
            if !path.starts_with('/') {
                return Err(FetchError::Failed(
                    "file URL must have a local absolute path".into(),
                ));
            }
            let path = path.split(['?', '#']).next().unwrap_or_default();
            let path =
                std::ffi::OsString::from_vec(percent_encoding::percent_decode_str(path).collect());
            return read_file(Path::new(&path), limit);
        }
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(FetchError::Failed(format!("unsupported URL: {url}")));
        }
        match self.agent.get(url).call() {
            Ok(mut response) => response
                .body_mut()
                .with_config()
                .limit(limit)
                .read_to_vec()
                .map_err(|error| FetchError::Failed(error.to_string())),
            Err(ureq::Error::StatusCode(404)) => Err(FetchError::NotFound),
            Err(error) => Err(FetchError::Failed(error.to_string())),
        }
    }
}

impl CatalogFetcher for HttpFetcher {
    fn fetch_index(
        &self,
        url: &str,
        etag: Option<&str>,
        limit: u64,
    ) -> Result<Fetched, FetchError> {
        // `file://` has no ETag, so it is never cached.
        if !url.starts_with("http://") && !url.starts_with("https://") {
            let body = self.fetch(url, limit)?;
            return Ok(Fetched::Body { body, etag: None });
        }
        let mut request = self.agent.get(url);
        if let Some(etag) = etag {
            request = request.header("If-None-Match", etag);
        }
        let mut response = match request.call() {
            Ok(response) => response,
            Err(ureq::Error::StatusCode(404)) => return Err(FetchError::NotFound),
            Err(error) => return Err(FetchError::Failed(error.to_string())),
        };
        if response.status() == 304 {
            return Ok(Fetched::Unchanged);
        }
        let etag = response
            .headers()
            .get("etag")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body = response
            .body_mut()
            .with_config()
            .limit(limit)
            .read_to_vec()
            .map_err(|error| FetchError::Failed(error.to_string()))?;
        Ok(Fetched::Body { body, etag })
    }
}

fn read_file(path: &Path, limit: u64) -> Result<Vec<u8>, FetchError> {
    let file = std::fs::File::open(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => FetchError::NotFound,
        _ => FetchError::Failed(format!("{}: {error}", path.display())),
    })?;
    let mut body = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut body)
        .map_err(|error| FetchError::Failed(format!("{}: {error}", path.display())))?;
    if body.len() as u64 > limit {
        return Err(FetchError::Failed(format!(
            "{}: file too large",
            path.display()
        )));
    }
    Ok(body)
}
