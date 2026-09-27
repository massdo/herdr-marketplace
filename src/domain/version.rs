use std::fmt;

/// Version read the way Herdr 0.9.1 reads it (`update::Version::parse`):
/// exactly three numbers, an optional `v` prefix, nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.strip_prefix('v').unwrap_or(value);
        let parts: Vec<&str> = value.split('.').collect();
        if parts.len() != 3 {
            return None;
        }
        Some(Self {
            major: parts[0].parse().ok()?,
            minor: parts[1].parse().ok()?,
            patch: parts[2].parse().ok()?,
        })
    }

    /// Reads `herdr --version` (`herdr 0.9.1`, or `herdr 0.9.1-preview.N`).
    /// Herdr compares plugins against its base version, before the channel.
    pub fn from_herdr_output(output: &str) -> Option<Self> {
        let mut words = output.split_whitespace();
        if words.next() != Some("herdr") {
            return None;
        }
        let version = words.next()?;
        Self::parse(version.split('-').next()?)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}
