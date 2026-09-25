//! Install operations run outside the fiche, and their kept results.

use serde::{Deserialize, Serialize};

use super::install::installed_from;
use super::registry::InstalledPlugin;
use super::source::PluginSource;

/// A confirmed request, handed to the process that runs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationRequest {
    /// Tells this operation's result from an older one of the same source.
    pub id: String,
    pub source: PluginSource,
    pub commit: String,
    /// Arguments of `herdr`.
    pub args: Vec<String>,
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
        }
    }
}

/// Success is exit code 0 and the registry showing the source at the
/// commit; code 0 without that is never reported as a success.
pub fn install_status(
    exit_code: Option<i32>,
    registry: &Result<Vec<InstalledPlugin>, String>,
    request: &OperationRequest,
) -> Status {
    if exit_code != Some(0) {
        return Status::Failed;
    }
    let confirmed = registry.as_ref().is_ok_and(|registry| {
        installed_from(registry, &request.source).is_ok_and(|installed| {
            installed.and_then(InstalledPlugin::resolved_commit) == Some(request.commit.as_str())
        })
    });
    if confirmed {
        Status::Succeeded
    } else {
        Status::Unconfirmed
    }
}

/// The source as the registry shows it, in words.
pub fn registry_state(
    registry: &Result<Vec<InstalledPlugin>, String>,
    source: &PluginSource,
) -> String {
    match registry {
        Err(error) => format!("état inconnu, registre illisible : {error}"),
        Ok(registry) => match installed_from(registry, source) {
            Ok(Some(plugin)) => format!(
                "installé à {}",
                plugin.resolved_commit().unwrap_or("un commit inconnu")
            ),
            Ok(None) => "non installé".to_string(),
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
