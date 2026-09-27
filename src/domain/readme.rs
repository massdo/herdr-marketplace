//! Where the links and images of a README lead, resolved as GitHub resolves
//! them: a relative path starts from the README's folder, at the commit
//! shown.

use super::source::PluginSource;

const GITHUB: &str = "https://github.com";
const RAW: &str = "https://raw.githubusercontent.com";

/// The README shown: its repository, commit and folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadmePlace {
    pub source: PluginSource,
    pub commit: String,
    /// Folder of the README in the repository, empty at the root.
    pub folder: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    /// Opened in the browser.
    Web(String),
    /// A heading of the same README, by its GitHub anchor.
    Anchor(String),
}

/// Where a link goes without knowing the README's place: only anchors and
/// absolute addresses.
pub fn absolute_link(href: &str) -> Option<LinkTarget> {
    let href = href.trim();
    if let Some(fragment) = href.strip_prefix('#') {
        return Some(LinkTarget::Anchor(fragment_anchor(fragment)));
    }
    web(href).map(LinkTarget::Web)
}

/// An image address that needs no README place.
pub fn absolute_image(src: &str) -> Option<String> {
    let src = src.trim();
    let lower = src.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") {
        Some(raw_github(src))
    } else if src.starts_with("//") {
        Some(raw_github(&format!("https:{src}")))
    } else {
        None
    }
}

impl ReadmePlace {
    /// Where a link goes; `None` for a scheme that is not the web's.
    pub fn link(&self, href: &str) -> Option<LinkTarget> {
        if let Some(target) = absolute_link(href) {
            return Some(target);
        }
        let href = href.trim();
        if href.is_empty() || has_scheme(href) {
            return None;
        }
        let (path, fragment) = match href.split_once('#') {
            Some((path, fragment)) => (path, Some(fragment)),
            None => (href, None),
        };
        let path = self.join(path);
        let mut url = if path.is_empty() {
            format!("{}/tree/{}", self.repository(), self.commit)
        } else {
            format!("{}/blob/{}/{path}", self.repository(), self.commit)
        };
        if let Some(fragment) = fragment {
            url.push('#');
            url.push_str(fragment);
        }
        Some(LinkTarget::Web(url))
    }

    /// Address to download an image from; `None` when it cannot be fetched.
    pub fn image(&self, src: &str) -> Option<String> {
        if let Some(url) = absolute_image(src) {
            return Some(url);
        }
        let src = src.trim();
        if src.is_empty() || src.starts_with('#') || has_scheme(src) {
            return None;
        }
        let path = self.join(src.split(['?', '#']).next().unwrap_or_default());
        (!path.is_empty()).then(|| {
            format!(
                "{RAW}/{}/{}/{}/{path}",
                self.source.owner, self.source.repo, self.commit
            )
        })
    }

    fn repository(&self) -> String {
        format!("{GITHUB}/{}/{}", self.source.owner, self.source.repo)
    }

    /// Repository path of `path`, from the README's folder, or from the root
    /// when it starts with `/`; `..` never climbs above the root.
    fn join(&self, path: &str) -> String {
        let (base, path) = match path.strip_prefix('/') {
            Some(path) => ("", path),
            None => (self.folder.as_str(), path),
        };
        let mut segments: Vec<&str> = base.split('/').filter(|part| !part.is_empty()).collect();
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    segments.pop();
                }
                part => segments.push(part),
            }
        }
        segments
            .into_iter()
            .map(encode_segment)
            .collect::<Vec<_>>()
            .join("/")
    }
}

/// Encode path bytes once, preserving already escaped octets.
fn encode_segment(segment: &str) -> String {
    let mut out = String::new();
    let bytes = segment.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'%'
            && bytes.get(index + 1).is_some_and(u8::is_ascii_hexdigit)
            && bytes.get(index + 2).is_some_and(u8::is_ascii_hexdigit)
        {
            out.push_str(&segment[index..index + 3]);
            index += 3;
            continue;
        }
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
        index += 1;
    }
    out
}

/// The plugin's folder on GitHub at the commit, where GitHub shows its
/// README.
pub fn github_page(source: &PluginSource, commit: &str) -> String {
    let mut url = format!("{GITHUB}/{}/{}/tree/{commit}", source.owner, source.repo);
    if !source.subdir.is_empty() {
        url.push('/');
        url.push_str(&source.subdir);
    }
    url
}

/// GitHub's anchor of a heading: lowercase, spaces as hyphens, punctuation
/// and symbols dropped.
pub fn anchor(heading: &str) -> String {
    heading
        .trim()
        .to_lowercase()
        .chars()
        .filter_map(|ch| match ch {
            ' ' => Some('-'),
            ch if ch.is_alphanumeric() || ch == '-' || ch == '_' => Some(ch),
            _ => None,
        })
        .collect()
}

fn fragment_anchor(fragment: &str) -> String {
    percent_encoding::percent_decode_str(fragment)
        .decode_utf8_lossy()
        .to_lowercase()
}

/// An address the browser opens as it is.
fn web(href: &str) -> Option<String> {
    let lower = href.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:")
    {
        Some(href.to_string())
    } else if href.starts_with("//") {
        Some(format!("https:{href}"))
    } else {
        None
    }
}

/// `javascript:`, `data:`, `file:`… : a scheme before any path character.
fn has_scheme(href: &str) -> bool {
    href.split_once(':').is_some_and(|(scheme, _)| {
        !scheme.is_empty()
            && scheme
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))
    })
}

/// `github.com/{owner}/{repo}/blob/{ref}/{path}` (or `raw`) served as the file
/// itself, from raw.githubusercontent.com.
fn raw_github(url: &str) -> String {
    let rest = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"));
    if let Some(rest) = rest {
        let parts: Vec<&str> = rest.splitn(5, '/').collect();
        if let [owner, repo, "blob" | "raw", reference, path] = parts.as_slice() {
            let path = path.split(['?', '#']).next().unwrap_or_default();
            return format!("{RAW}/{owner}/{repo}/{reference}/{path}");
        }
    }
    url.to_string()
}
