//! Local updates change the plugin inside an existing Details pane.
use std::collections::hash_map::DefaultHasher;
use std::fs::{self, DirBuilder};
use std::hash::{Hash, Hasher};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::domain::details::DetailsTarget;
use crate::domain::error::AppError;
use crate::domain::ids::PaneId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetailsUpdate {
    pub target: DetailsTarget,
    pub video_cache: Option<PathBuf>,
}

fn folder(state: &Path, socket: &Path, pane: &PaneId) -> PathBuf {
    // Short and confined to one directory, even with long or unusual IDs.
    // The Herdr socket distinguishes panes of different sessions.
    let mut hash = DefaultHasher::new();
    (socket, &pane.0).hash(&mut hash);
    state
        .join("details")
        .join(format!("{:016x}", hash.finish()))
}

pub struct DetailsControl(PathBuf);

fn alive(folder: &Path) -> bool {
    fs::read_to_string(folder.join("owner"))
        .ok()
        .and_then(|owner| owner.parse::<libc::pid_t>().ok())
        // SAFETY: signal 0 checks liveness only.
        .is_some_and(|owner| owner > 0 && unsafe { libc::kill(owner, 0) } == 0)
}

impl DetailsControl {
    pub fn open(state: &Path, socket: &Path, pane: &PaneId) -> Result<Self, AppError> {
        if let Ok(entries) = fs::read_dir(state.join("details")) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.len() == 16
                    && name.bytes().all(|byte| byte.is_ascii_hexdigit())
                    && entry.path().join("owner").exists()
                    && !alive(&entry.path())
                {
                    let _ = fs::remove_dir_all(entry.path());
                }
            }
        }
        let folder = folder(state, socket, pane);
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&folder)?;
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o700))?;
        for name in ["closing", "target.json", "pending.json"] {
            let _ = fs::remove_file(folder.join(name));
        }
        fs::write(folder.join("owner"), std::process::id().to_string())?;
        Ok(Self(folder))
    }

    pub fn send(
        state: &Path,
        socket: &Path,
        pane: &PaneId,
        update: DetailsUpdate,
    ) -> Result<(), AppError> {
        let folder = folder(state, socket, pane);
        if folder.join("closing").exists() {
            return Err(AppError::Io {
                message: "Details is closing".into(),
            });
        }
        if !alive(&folder) {
            return Err(AppError::Io {
                message: "Details is not ready for updates; close and reopen it".into(),
            });
        }
        let mut file = tempfile::NamedTempFile::new_in(&folder)?;
        serde_json::to_writer(&mut file, &update).map_err(|error| AppError::Io {
            message: error.to_string(),
        })?;
        file.persist(folder.join("target.json"))
            .map_err(|error| AppError::Io {
                message: error.to_string(),
            })?;
        Ok(())
    }

    /// Let the plugin exit before Herdr tears its PTY down. Herdr removes
    /// plugin panes automatically when their command exits.
    pub fn request_close(state: &Path, socket: &Path, pane: &PaneId) -> Result<bool, AppError> {
        let folder = folder(state, socket, pane);
        if !alive(&folder) {
            return Ok(false);
        }
        fs::write(folder.join("closing"), [])?;
        Ok(true)
    }

    pub fn closing(state: &Path, socket: &Path, pane: &PaneId) -> bool {
        folder(state, socket, pane).join("closing").exists()
    }

    pub fn take(
        state: &Path,
        socket: &Path,
        pane: &PaneId,
    ) -> Result<Option<DetailsUpdate>, AppError> {
        let folder = folder(state, socket, pane);
        let pending = folder.join("pending.json");
        // Claim the entire record atomically. A newer click may already have
        // replaced target.json when we finish reading this one.
        match fs::rename(folder.join("target.json"), &pending) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
            Ok(()) => {}
        }
        let bytes = fs::read(&pending)?;
        let _ = fs::remove_file(pending);
        if bytes.len() > 64 * 1024 {
            return Err(AppError::Io {
                message: "Details update is too large".into(),
            });
        }
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| AppError::Io {
                message: error.to_string(),
            })
    }
}

impl Drop for DetailsControl {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
