use crate::application::ports::{FetchError, Fetcher};
use crate::domain::source::PluginSource;

const RAW_BASE: &str = "https://raw.githubusercontent.com";
/// Upper bound for a README body.
pub const README_LIMIT: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Readme {
    /// `fallback`: the plugin folder has no README.md of its own, this is the
    /// one at the repository root.
    Found {
        text: String,
        fallback: bool,
    },
    NotFound,
}

/// README.md at the exact commit, never the default branch. A plugin in a
/// subfolder without its own README.md falls back to the root one. A network
/// error is returned as such so it can be retried.
pub fn load_readme<F: Fetcher>(
    fetcher: &F,
    source: &PluginSource,
    commit: &str,
) -> Result<Readme, String> {
    let mut places = vec![(source.subdir.as_str(), false)];
    if !source.subdir.is_empty() {
        places.push(("", true));
    }
    for (folder, fallback) in places {
        match fetcher.fetch(&raw_url(source, commit, folder, "README.md"), README_LIMIT) {
            Ok(body) => {
                return Ok(Readme::Found {
                    text: String::from_utf8_lossy(&body).into_owned(),
                    fallback,
                });
            }
            Err(FetchError::NotFound) => continue,
            Err(FetchError::Failed(error)) => return Err(error),
        }
    }
    Ok(Readme::NotFound)
}

/// `https://raw.githubusercontent.com/{owner}/{repo}/{commit}/{folder}/{file}`.
pub fn raw_url(source: &PluginSource, commit: &str, folder: &str, file: &str) -> String {
    let mut url = format!(
        "{RAW_BASE}/{}/{}/{}",
        encode(&source.owner),
        encode(&source.repo),
        encode(commit)
    );
    for segment in folder.split('/').filter(|segment| !segment.is_empty()) {
        url.push('/');
        url.push_str(&encode(segment));
    }
    url.push('/');
    url.push_str(file);
    url
}

/// Percent-encodes everything but RFC 3986 unreserved characters.
fn encode(segment: &str) -> String {
    let mut out = String::new();
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}
