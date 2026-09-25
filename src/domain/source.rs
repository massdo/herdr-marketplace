use std::fmt;

/// Identity of a plugin: owner/repo/subdir. `subdir` is empty at the root.
/// The manifest id is not unique and never identifies a plugin on its own.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PluginSource {
    pub owner: String,
    pub repo: String,
    pub subdir: String,
}

impl fmt::Display for PluginSource {
    /// `owner/repo[/subdir]`, the shorthand Herdr takes.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.repo)?;
        if !self.subdir.is_empty() {
            write!(f, "/{}", self.subdir)?;
        }
        Ok(())
    }
}

/// Herdr 0.9.1 `validate_subdir_segment`.
pub fn is_subdir_segment(segment: &str) -> bool {
    !segment.is_empty() && segment != "." && segment != ".." && !segment.contains(['\\', '\0'])
}
