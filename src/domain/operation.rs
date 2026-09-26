//! Install and removal operations run outside the details pane, and their kept
//! results.

use serde::{Deserialize, Serialize};

use super::details::DetailsTarget;
use super::install::{Plan, installed_from};
use super::manifest::Manifest;
use super::registry::InstalledPlugin;
use super::source::PluginSource;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Install,
    Uninstall,
}

/// The state the user reviewed, checked again under the operation lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Confirmation {
    Install {
        target: DetailsTarget,
        manifest: Box<Manifest>,
        plan: Plan,
    },
    Uninstall {
        installed: InstalledPlugin,
    },
}

/// A confirmed request, handed to the process that runs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationRequest {
    /// Tells this operation's result from an older one of the same source.
    pub id: String,
    pub kind: OperationKind,
    /// The source shown in the details pane; the arguments may spell it as the
    /// registry does.
    pub source: PluginSource,
    pub commit: String,
    /// Arguments of `herdr`.
    pub args: Vec<String>,
    /// Old result files remain readable; execution requires a confirmation.
    #[serde(default)]
    pub confirmation: Option<Confirmation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Running,
    Succeeded,
    Failed,
    /// Herdr answered 0 but the registry does not show the expected state.
    Unconfirmed,
    /// Another marketplace operation was running.
    Refused,
}

/// Last operation of a source, kept in the plugin state directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationRecord {
    pub request: OperationRequest,
    pub status: Status,
    pub exit_code: Option<i32>,
    /// What Herdr printed, capped.
    pub output: String,
    /// State of the source in the registry read after the command.
    pub registry_after: Option<String>,
    pub finished_unix_ms: Option<u64>,
    #[serde(default)]
    pub worker_pid: Option<u32>,
    /// Returned to an open pane even when saving the result failed.
    #[serde(default)]
    pub persistence_error: Option<String>,
}

impl OperationRecord {
    pub fn running(request: &OperationRequest) -> Self {
        Self {
            request: request.clone(),
            status: Status::Running,
            exit_code: None,
            output: String::new(),
            registry_after: None,
            finished_unix_ms: None,
            worker_pid: None,
            persistence_error: None,
        }
    }
}

/// An install succeeds when Herdr answers 0 and the registry shows the
/// source at the commit; code 0 without that is an unconfirmed result, never
/// a success. A removal succeeds when Herdr answers 0 and the source is
/// gone from the registry; anything else is a failure.
pub fn operation_status(
    exit_code: Option<i32>,
    registry: &Result<Vec<InstalledPlugin>, String>,
    request: &OperationRequest,
) -> Status {
    if exit_code != Some(0) {
        return Status::Failed;
    }
    let installed = registry
        .as_ref()
        .ok()
        .and_then(|registry| installed_from(registry, &request.source).ok());
    match request.kind {
        OperationKind::Install => {
            let commit = installed
                .flatten()
                .and_then(InstalledPlugin::resolved_commit);
            if commit == Some(request.commit.as_str()) {
                Status::Succeeded
            } else {
                Status::Unconfirmed
            }
        }
        OperationKind::Uninstall => match installed {
            Some(None) => Status::Succeeded,
            _ => Status::Failed,
        },
    }
}

/// The source as the registry shows it, in words.
pub fn registry_state(
    registry: &Result<Vec<InstalledPlugin>, String>,
    source: &PluginSource,
) -> String {
    match registry {
        Err(error) => format!("unknown state, unreadable registry: {error}"),
        Ok(registry) => match installed_from(registry, source) {
            Ok(Some(plugin)) => format!(
                "installed at {}",
                plugin.resolved_commit().unwrap_or("an unknown commit")
            ),
            Ok(None) => "not installed".to_string(),
            Err(error) => error,
        },
    }
}

/// File name of a source's result. Owner and repo ignore case; uppercase
/// letters of the subdir are escaped so that case-insensitive file systems
/// keep `alt` and `ALT` apart.
pub fn record_file_name(source: &PluginSource) -> String {
    let mut key = format!(
        "{}/{}",
        source.owner.to_ascii_lowercase(),
        source.repo.to_ascii_lowercase()
    );
    if !source.subdir.is_empty() {
        key.push('/');
        key.push_str(&source.subdir);
    }
    let mut name = String::new();
    for byte in key.bytes() {
        if byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        {
            name.push(byte as char);
        } else {
            name.push_str(&format!("%{byte:02X}"));
        }
    }
    name.push_str(".json");
    name
}
