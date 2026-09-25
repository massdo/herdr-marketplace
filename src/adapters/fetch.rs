use std::io::Read;
use std::time::Duration;

use crate::application::ports::{FetchError, Fetcher};

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
            return read_file(path, limit);
        }
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(FetchError::Failed(format!(
                "URL non prise en charge : {url}"
            )));
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

fn read_file(path: &str, limit: u64) -> Result<Vec<u8>, FetchError> {
    let file = std::fs::File::open(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => FetchError::NotFound,
        _ => FetchError::Failed(format!("{path} : {error}")),
    })?;
    let mut body = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut body)
        .map_err(|error| FetchError::Failed(format!("{path} : {error}")))?;
    if body.len() as u64 > limit {
        return Err(FetchError::Failed(format!(
            "{path} : fichier trop volumineux"
        )));
    }
    Ok(body)
}
