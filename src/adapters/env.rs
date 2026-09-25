use std::env;
use std::path::PathBuf;

use crate::domain::{DEFAULT_INDEX_URL, INDEX_URL_ENV};

/// Index source: `HERDR_MARKETPLACE_INDEX_URL` (http(s) or file://) or the
/// public index.
pub fn index_url() -> String {
    env_string(INDEX_URL_ENV).unwrap_or_else(|| DEFAULT_INDEX_URL.to_string())
}

/// The Herdr binary that launched the plugin, else `herdr` from `PATH`.
pub fn herdr_bin() -> PathBuf {
    env_string("HERDR_BIN_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("herdr"))
}

fn env_string(key: &str) -> Option<String> {
    env::var(key).ok().filter(|value| !value.is_empty())
}
