use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;

use crate::application::ports::Operations;
use crate::domain::operation::{OperationRecord, OperationRequest, record_file_name};
use crate::domain::source::PluginSource;

/// Results under `<state dir>/operations`, one JSON file per source, and
/// `operation.lock` next to them.
pub struct FsOperations {
    dir: PathBuf,
}

impl FsOperations {
    pub fn new(state_dir: PathBuf) -> Self {
        Self {
            dir: state_dir.join("operations"),
        }
    }
}

impl Operations for FsOperations {
    type Guard = File;

    fn try_begin(&self) -> Result<Option<File>, String> {
        fs::create_dir_all(&self.dir).map_err(|error| error.to_string())?;
        let path = self.dir.join("operation.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| format!("{} : {error}", path.display()))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(file)),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => Err(format!("{} : {error}", path.display())),
        }
    }

    fn load(&self, source: &PluginSource) -> Option<OperationRecord> {
        let text = fs::read_to_string(self.dir.join(record_file_name(source))).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Written to a temporary file then renamed, so a reader never sees half
    /// a result.
    fn save(&self, record: &OperationRecord) -> Result<(), String> {
        fs::create_dir_all(&self.dir).map_err(|error| error.to_string())?;
        let name = record_file_name(&record.request.source);
        let temporary = self.dir.join(format!(".{name}.{}", std::process::id()));
        let json = serde_json::to_vec_pretty(record).map_err(|error| error.to_string())?;
        fs::write(&temporary, json).map_err(|error| error.to_string())?;
        fs::rename(&temporary, self.dir.join(name)).map_err(|error| error.to_string())
    }

    fn latest_finish(&self) -> u64 {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return 0;
        };
        entries
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".json"))
            .filter_map(|entry| fs::read_to_string(entry.path()).ok())
            .filter_map(|text| serde_json::from_str::<OperationRecord>(&text).ok())
            .filter_map(|record| record.finished_unix_ms)
            .max()
            .unwrap_or(0)
    }
}

/// Starts this binary with `--run-operation` in a session of its own, with no
/// terminal: closing the fiche or the sidebar does not stop it.
pub fn spawn_operation(request: &OperationRequest) -> io::Result<()> {
    let json = serde_json::to_string(request).map_err(io::Error::other)?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("--run-operation")
        .arg(json)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: setsid only changes the session of the child between fork and
    // exec; it allocates nothing and touches no shared state.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
