//! Automatic README downloads may reach public HTTPS endpoints only.
use std::net::IpAddr;
use std::time::Duration;

use ureq::Body;
use ureq::http::header::{CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, LOCATION, RANGE};
use ureq::http::{Response, StatusCode};
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};

use crate::application::ports::{FetchError, Fetcher};

pub struct ImageFetcher(ureq::Agent);

/// What an address serves, told without downloading it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub content_type: String,
    /// Bytes of the whole file, when the server tells.
    pub size: Option<u64>,
}

impl Default for ImageFetcher {
    fn default() -> Self {
        Self::with_resolver(DefaultResolver::default())
    }
}

impl ImageFetcher {
    fn with_resolver(resolver: impl Resolver) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .https_only(true)
            .max_redirects(0)
            // A proxy would resolve the destination itself, bypassing our check.
            .proxy(None)
            .user_agent(concat!("herdr-marketplace/", env!("CARGO_PKG_VERSION")))
            .build();
        Self(ureq::Agent::with_parts(
            config,
            DefaultConnector::default(),
            PublicResolver(resolver),
        ))
    }
}

impl Fetcher for ImageFetcher {
    fn fetch(&self, url: &str, limit: u64) -> Result<Vec<u8>, FetchError> {
        let mut response = self.get(url, None)?;
        if !response.status().is_success() {
            return Err(FetchError::Failed(format!(
                "image HTTP status {} (redirects disabled)",
                response.status()
            )));
        }
        response
            .body_mut()
            .with_config()
            .limit(limit)
            .read_to_vec()
            .map_err(|error| FetchError::Failed(error.to_string()))
    }
}

impl ImageFetcher {
    /// Type and size of the file at `url`, under the rules of `fetch`. Only
    /// its first byte is asked for, and none is read.
    pub fn probe(&self, url: &str) -> Result<Probe, FetchError> {
        let response = self.get(url, Some("bytes=0-0"))?;
        let header = |name| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
        };
        let size = match response.status() {
            StatusCode::PARTIAL_CONTENT => header(CONTENT_RANGE).and_then(total_size),
            StatusCode::OK => header(CONTENT_LENGTH).and_then(|length| length.trim().parse().ok()),
            status => {
                return Err(FetchError::Failed(format!(
                    "video HTTP status {status} (redirects disabled)"
                )));
            }
        };
        Ok(Probe {
            content_type: header(CONTENT_TYPE).unwrap_or_default().to_string(),
            size,
        })
    }

    /// `GET url`, for `range` of it if given. GitHub serves the files
    /// uploaded to a README through one redirect to its storage. It is the
    /// only redirect followed.
    fn get(&self, url: &str, range: Option<&str>) -> Result<Response<Body>, FetchError> {
        let call = |url: &str| {
            let mut request = self.0.get(url);
            if let Some(range) = range {
                request = request.header(RANGE, range);
            }
            request
                .call()
                .map_err(|error| FetchError::Failed(error.to_string()))
        };
        let response = call(url)?;
        if !(response.status().is_redirection() && github_attachment(url)) {
            return Ok(response);
        }
        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|location| location.to_str().ok())
            .unwrap_or_default();
        if !github_storage(location) {
            return Err(FetchError::Failed(
                "GitHub attachment redirected outside GitHub's storage".into(),
            ));
        }
        call(location)
    }
}

/// Size of the whole file in a `Content-Range`, after its `/`:
/// `bytes 0-0/9841526`. `None` when the server does not know it.
pub fn total_size(content_range: &str) -> Option<u64> {
    content_range.rsplit_once('/')?.1.trim().parse().ok()
}

/// A file uploaded to a README: `https://github.com/user-attachments/assets/…`.
pub(crate) fn github_attachment(url: &str) -> bool {
    url.strip_prefix("https://github.com/user-attachments/assets/")
        .is_some_and(|rest| !rest.is_empty())
}

/// Where GitHub keeps those files: its S3 buckets and githubusercontent.com,
/// over HTTPS.
fn github_storage(url: &str) -> bool {
    let Ok(uri) = url.parse::<ureq::http::Uri>() else {
        return false;
    };
    let host = uri.host().unwrap_or_default().to_ascii_lowercase();
    uri.scheme_str() == Some("https")
        && (host.ends_with(".githubusercontent.com")
            || (host.starts_with("github-production-user-asset-")
                && host.ends_with(".s3.amazonaws.com")))
}

#[derive(Debug)]
struct PublicResolver<R>(R);

impl<R: Resolver> Resolver for PublicResolver<R> {
    fn resolve(
        &self,
        uri: &ureq::http::Uri,
        config: &ureq::config::Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        if uri.host().is_some_and(|host| {
            let host = host.trim_end_matches('.').to_ascii_lowercase();
            host == "localhost" || host.ends_with(".localhost")
        }) {
            return Err(ureq::Error::HostNotFound);
        }
        let addresses = self.0.resolve(uri, config, timeout)?;
        // These exact addresses go to the connector: there is no second DNS
        // lookup between validation and connection (including mixed answers).
        if addresses.is_empty() || addresses.iter().any(|address| !public_ip(address.ip())) {
            return Err(ureq::Error::HostNotFound);
        }
        Ok(addresses)
    }
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_documentation()
                || a == 0
                || a >= 224
                || (a == 100 && (64..=127).contains(&b))
                || (a == 198 && (b == 18 || b == 19))
                || (a == 192 && b == 0 && c == 0))
        }
        IpAddr::V6(ip) => {
            if let Some(ip) = ip.to_ipv4_mapped() {
                return public_ip(IpAddr::V4(ip));
            }
            let segments = ip.segments();
            // Global unicast only; exclude transition/special-purpose and
            // documentation prefixes as well as all local IPv6 ranges.
            segments[0] & 0xe000 == 0x2000
                && !(segments[0] == 0x2001 && (segments[1] < 0x200 || segments[1] == 0xdb8))
                && segments[0] != 0x2002
                && !(segments[0] == 0x3fff && segments[1] < 0x1000)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Addresses(Vec<std::net::SocketAddr>);
    impl Resolver for Addresses {
        fn resolve(
            &self,
            _: &ureq::http::Uri,
            _: &ureq::config::Config,
            _: NextTimeout,
        ) -> Result<ResolvedSocketAddrs, ureq::Error> {
            let mut addresses = self.empty();
            for address in &self.0 {
                addresses.push(*address);
            }
            Ok(addresses)
        }
    }

    #[test]
    fn image_downloads_refuse_local_addresses_even_behind_a_public_dns_name() {
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "224.0.0.1",
            "198.18.0.1",
            "::1",
            "fe80::1",
            "fc00::1",
            "::ffff:127.0.0.1",
            "64:ff9b::7f00:1",
            "2002:7f00:1::",
        ] {
            let address = std::net::SocketAddr::new(ip.parse().unwrap(), 443);
            let fetcher = ImageFetcher::with_resolver(Addresses(vec![
                "8.8.8.8:443".parse().unwrap(),
                address,
            ]));
            let error = fetcher
                .fetch("https://public.example/image.png", 100)
                .unwrap_err();
            assert!(
                error.to_string().contains("host not found"),
                "{ip}: {error}"
            );
        }
        for ip in ["8.8.8.8", "185.199.108.133", "2606:4700::1111"] {
            assert!(public_ip(ip.parse().unwrap()));
        }
    }

    #[test]
    fn images_require_https_without_redirects_or_a_proxy() {
        let fetcher = ImageFetcher::default();
        for url in [
            "http://127.0.0.1:8080/reset",
            "file:///dev/zero",
            "https://localhost/image.png",
        ] {
            assert!(fetcher.fetch(url, 100).is_err(), "{url}");
        }
        assert_eq!(fetcher.0.config().max_redirects(), 0);
        assert!(fetcher.0.config().https_only());
        assert!(fetcher.0.config().proxy().is_none());
    }

    #[test]
    fn a_file_size_is_read_after_the_slash_of_its_content_range() {
        assert_eq!(total_size("bytes 0-0/9841526"), Some(9_841_526));
        assert_eq!(total_size("bytes 0-0/*"), None);
        assert_eq!(total_size("not a range"), None);
    }

    #[test]
    fn only_a_github_attachment_follows_its_redirect_to_github_storage() {
        let upload =
            "https://github.com/user-attachments/assets/23ee0639-4a6e-42c5-a003-6e71ab619c43";
        assert!(github_attachment(upload));
        for other in [
            "https://github.com/user-attachments/assets/",
            "https://github.com/owner/repo/blob/main/demo.gif",
            "http://github.com/user-attachments/assets/23ee0639",
            "https://example.com/user-attachments/assets/23ee0639",
        ] {
            assert!(!github_attachment(other), "{other}");
        }
        for storage in [
            "https://github-production-user-asset-6210df.s3.amazonaws.com/9/1-a.gif?X-Amz-Signature=0",
            "https://private-user-images.githubusercontent.com/9/1-a.gif?jwt=0",
        ] {
            assert!(github_storage(storage), "{storage}");
        }
        for elsewhere in [
            "http://private-user-images.githubusercontent.com/9/1-a.gif",
            "https://githubusercontent.com.example.com/1-a.gif",
            "https://example.com/?host=.githubusercontent.com",
            "https://s3.amazonaws.com/github-production-user-asset-6210df/1-a.gif",
            "https://github-production-user-asset-6210df.s3.amazonaws.com.example.com/1-a.gif",
            "https://github.com/login",
            "/relative/1-a.gif",
            "",
        ] {
            assert!(!github_storage(elsewhere), "{elsewhere}");
        }
    }
}
