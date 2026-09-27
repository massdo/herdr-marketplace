//! Automatic README downloads may reach public HTTPS endpoints only.
use std::net::IpAddr;
use std::time::Duration;

use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};

use crate::application::ports::{FetchError, Fetcher};

pub struct ImageFetcher(ureq::Agent);

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
        let mut response = self
            .0
            .get(url)
            .call()
            .map_err(|error| FetchError::Failed(error.to_string()))?;
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
}
