use std::collections::BTreeMap;
use std::fmt;

use crate::domain::error::AppError;
use crate::domain::ids::PaneId;
use crate::domain::operation::OperationRecord;
use crate::domain::pane::{LayoutSnapshot, OpenedPane, PaneInfo};
use crate::domain::source::PluginSource;

/// Downloads. `file://` URLs are served from disk, which the tests use.
pub trait Fetcher {
    /// Body of `url`, refused beyond `limit` bytes.
    fn fetch(&self, url: &str, limit: u64) -> Result<Vec<u8>, FetchError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// HTTP 404 or missing file.
    NotFound,
    Failed(String),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "not found"),
            Self::Failed(detail) => write!(f, "{detail}"),
        }
    }
}

/// The `herdr` command line.
pub trait HerdrCli {
    /// Standard output of `herdr --version`.
    fn version(&self) -> Result<String, String>;
    /// Standard output of `herdr plugin list --json`.
    fn plugin_list(&self) -> Result<String, String>;
    /// Runs `herdr` with these arguments, each passed as is.
    fn run(&self, args: &[String]) -> Result<CommandOutput, String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    /// `None` when a signal ended the command.
    pub code: Option<i32>,
    /// Standard output then standard error.
    pub output: String,
}

/// Where operation results are kept, and the lock that allows one
/// operation at a time.
pub trait Operations {
    /// Held while an operation runs; dropping it releases the lock.
    type Guard;
    /// The lock, or `None` while another operation holds it.
    fn try_begin(&self) -> Result<Option<Self::Guard>, String>;
    fn load(&self, source: &PluginSource) -> Option<OperationRecord>;
    fn save(&self, record: &OperationRecord) -> Result<(), String>;
    /// Latest end time among the kept results, in Unix milliseconds.
    fn latest_finish(&self) -> u64;
}

/// Herdr socket operations on panes. The OS lock is an adapter detail.
pub trait HerdrPort {
    fn list_panes(&self, workspace_id: Option<&str>) -> Result<Vec<PaneInfo>, AppError>;
    fn pane_layout(&self, pane_id: &PaneId) -> Result<LayoutSnapshot, AppError>;
    fn open_plugin_pane(&self, request: OpenPluginPane) -> Result<OpenedPane, AppError>;
    fn swap_panes(&self, source: &PaneId, target: &PaneId) -> Result<(), AppError>;
    fn focus_pane(&self, pane_id: &PaneId) -> Result<(), AppError>;
    fn resize_pane(&self, pane_id: &PaneId, direction: &str, amount: f64) -> Result<(), AppError>;
    /// Session token that recognises a marketplace pane: `token_key = "v1"`.
    fn report_identity(&self, pane_id: &PaneId, token_key: &str) -> Result<(), AppError>;
    fn close_plugin_pane(&self, pane_id: &PaneId) -> Result<(), AppError>;
}

#[derive(Debug, Clone)]
pub struct OpenPluginPane {
    pub plugin_id: String,
    pub entrypoint: String,
    pub target_pane_id: PaneId,
    pub focus: bool,
    pub env: BTreeMap<String, String>,
}
