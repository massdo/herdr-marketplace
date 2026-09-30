use std::env;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::domain::details::DetailsTarget;
use crate::domain::error::AppError;
use crate::domain::ids::{PaneId, TabId, WorkspaceId};
use crate::domain::pane::OriginContext;
use crate::domain::{DEFAULT_INDEX_URL, DETAILS_ENV, INDEX_URL_ENV, PLUGIN_ID};

/// Values read once at process start, then passed as ordinary data.
#[derive(Debug, Clone)]
pub struct ProcessEnv {
    pub socket_path: PathBuf,
    pub own_pane_id: Option<PaneId>,
    pub state_dir: PathBuf,
}

#[derive(Debug, Deserialize)]
struct PluginContextJson {
    workspace_id: Option<String>,
    tab_id: Option<String>,
    focused_pane_id: Option<String>,
}

pub fn load() -> Result<ProcessEnv, AppError> {
    Ok(ProcessEnv {
        socket_path: socket_path()?,
        own_pane_id: env_string("HERDR_PANE_ID").map(PaneId),
        state_dir: state_dir(),
    })
}

/// Origin of an action, adapted from herdr-npm v0.1.0.
pub fn origin_from_env() -> Result<OriginContext, AppError> {
    let context = plugin_context();
    let workspace = first_non_empty(&[
        env_string("HERDR_WORKSPACE_ID"),
        env_string("HERDR_ACTIVE_WORKSPACE_ID"),
        context.as_ref().and_then(|item| item.workspace_id.clone()),
    ]);
    let tab = first_non_empty(&[
        env_string("HERDR_TAB_ID"),
        env_string("HERDR_ACTIVE_TAB_ID"),
        context.as_ref().and_then(|item| item.tab_id.clone()),
    ]);
    let pane = first_non_empty(&[
        env_string("HERDR_ACTIVE_PANE_ID"),
        context
            .as_ref()
            .and_then(|item| item.focused_pane_id.clone()),
        env_string("HERDR_PANE_ID"),
    ]);
    match (workspace, tab, pane) {
        (Some(workspace_id), Some(tab_id), Some(pane_id)) => Ok(OriginContext {
            workspace_id: WorkspaceId(workspace_id),
            tab_id: TabId(tab_id),
            pane_id: PaneId(pane_id),
        }),
        _ => Err(AppError::OriginMissing),
    }
}

pub fn socket_path() -> Result<PathBuf, AppError> {
    if let Some(path) = env::var_os("HERDR_SOCKET_PATH") {
        return Ok(PathBuf::from(path));
    }
    let home = env::var_os("HOME").ok_or(AppError::Io {
        message: "HOME is unset and HERDR_SOCKET_PATH is missing".into(),
    })?;
    Ok(PathBuf::from(home).join(".config/herdr/herdr.sock"))
}

fn state_dir() -> PathBuf {
    if let Some(path) = env::var_os("HERDR_PLUGIN_STATE_DIR") {
        return PathBuf::from(path);
    }
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".local/state/herdr/plugins")
        .join(PLUGIN_ID)
}

/// Plugin and commit handed to a details pane by the sidebar.
pub fn details_target() -> Result<DetailsTarget, AppError> {
    let raw = env_string(DETAILS_ENV).ok_or_else(|| AppError::Io {
        message: format!("{DETAILS_ENV} is missing: the details pane opens from the sidebar"),
    })?;
    serde_json::from_str(&raw).map_err(|error| AppError::Io {
        message: format!("{DETAILS_ENV} is unreadable: {error}"),
    })
}

/// Index source: `HERDR_MARKETPLACE_INDEX_URL` (http(s) or file://) or the
/// public index.
pub fn index_url() -> String {
    env_string(INDEX_URL_ENV).unwrap_or_else(|| DEFAULT_INDEX_URL.to_string())
}

/// Where the saved index lives: in the plugin's own directory, which
/// `herdr plugin uninstall` removes. `None` when Herdr gives no directory.
pub fn catalog_dir() -> Option<PathBuf> {
    env::var_os("HERDR_PLUGIN_ROOT")
        .filter(|root| !root.is_empty())
        .map(|root| PathBuf::from(root).join("target/catalog"))
}

/// The Herdr binary that launched the plugin, else `herdr` from `PATH`.
pub fn herdr_bin() -> PathBuf {
    env_string("HERDR_BIN_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("herdr"))
}

/// The private FFmpeg that plays README videos; `None` without one.
pub fn ffmpeg_bin() -> Option<PathBuf> {
    let exe = env::current_exe().ok();
    ffmpeg_from(
        env::var("HERDR_MARKETPLACE_FFMPEG").ok().as_deref(),
        exe.as_deref(),
    )
}

/// `HERDR_MARKETPLACE_FFMPEG` when it names a file, for tests and
/// development; else `ffmpeg` next to the executable `exe`, where a verified
/// install puts it.
pub fn ffmpeg_from(variable: Option<&str>, exe: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = variable.filter(|path| !path.is_empty()).map(PathBuf::from)
        && path.is_file()
    {
        return Some(path);
    }
    exe?.parent()
        .map(|folder| folder.join("ffmpeg"))
        .filter(|path| path.is_file())
}

fn plugin_context() -> Option<PluginContextJson> {
    let raw = env::var("HERDR_PLUGIN_CONTEXT_JSON").ok()?;
    serde_json::from_str(&raw).ok()
}

fn env_string(key: &str) -> Option<String> {
    env::var(key).ok().filter(|value| !value.is_empty())
}

fn first_non_empty(values: &[Option<String>]) -> Option<String> {
    values.iter().flatten().next().cloned()
}
