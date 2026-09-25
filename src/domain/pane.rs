use std::collections::BTreeMap;

use serde::Deserialize;

use super::ids::{PaneId, TabId, WorkspaceId};
use super::{EXPLORER_TOKEN_KEY, SIDEBAR_TOKEN_KEY, TOKEN_VALUE};

/// Context captured from the action before any socket I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginContext {
    pub workspace_id: WorkspaceId,
    pub tab_id: TabId,
    pub pane_id: PaneId,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PaneInfo {
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    #[serde(default)]
    pub focused: bool,
    pub label: Option<String>,
    pub title: Option<String>,
    #[serde(default)]
    pub tokens: BTreeMap<String, String>,
}

impl PaneInfo {
    pub fn id(&self) -> PaneId {
        PaneId(self.pane_id.clone())
    }

    pub fn is_marketplace_sidebar(&self) -> bool {
        self.tokens.get(SIDEBAR_TOKEN_KEY).map(String::as_str) == Some(TOKEN_VALUE)
    }

    pub fn is_explorer(&self) -> bool {
        self.tokens.contains_key(EXPLORER_TOKEN_KEY)
            || matches!(self.label.as_deref(), Some("Sidebar" | "Explorer"))
            || matches!(self.title.as_deref(), Some("Sidebar" | "Explorer"))
    }

    pub fn is_excluded_working_target(&self) -> bool {
        self.is_marketplace_sidebar() || self.is_explorer()
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LayoutSnapshot {
    pub workspace_id: String,
    pub tab_id: String,
    pub area: LayoutRect,
    pub focused_pane_id: String,
    pub panes: Vec<LayoutPane>,
    pub splits: Vec<LayoutSplit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct LayoutRect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LayoutPane {
    pub pane_id: String,
    pub focused: bool,
    pub rect: LayoutRect,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LayoutSplit {
    pub id: String,
    pub direction: String,
    pub ratio: f64,
    pub rect: LayoutRect,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResizeStep {
    pub direction: &'static str,
    pub amount: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedPane {
    pub pane_id: PaneId,
}
