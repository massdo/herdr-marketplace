use super::version::Version;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Linux,
    Macos,
    Windows,
}

impl Platform {
    /// Same choice as Herdr's `current_platform`.
    pub fn current() -> Self {
        if cfg!(target_os = "linux") {
            Self::Linux
        } else if cfg!(target_os = "macos") {
            Self::Macos
        } else {
            Self::Windows
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Macos => "macos",
            Self::Windows => "windows",
        }
    }
}

/// Herdr 0.9.1 rules: undeclared platforms install everywhere; only
/// `min_herdr_version` constrains the version, and a missing or unreadable
/// one makes Herdr refuse the manifest.
pub fn is_compatible(
    platforms: Option<&[String]>,
    min_herdr_version: Option<&str>,
    host: Platform,
    herdr: Version,
) -> bool {
    let platform_ok = platforms.is_none_or(|list| list.iter().any(|name| name == host.name()));
    let version_ok = min_herdr_version
        .and_then(|value| Version::parse(value.trim()))
        .is_some_and(|required| required <= herdr);
    platform_ok && version_ok
}
