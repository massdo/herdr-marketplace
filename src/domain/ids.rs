use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkspaceId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TabId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PaneId(pub String);

impl WorkspaceId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TabId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl PaneId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WorkspaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl fmt::Display for TabId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl fmt::Display for PaneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<String> for WorkspaceId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<String> for TabId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<String> for PaneId {
    fn from(value: String) -> Self {
        Self(value)
    }
}
