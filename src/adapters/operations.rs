use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::UNIX_EPOCH;

use crate::application::ports::Operations;
use crate::domain::operation::{OperationRecord, OperationRequest, Status, record_file_name};
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
            .map_err(|error| format!("{}: {error}", path.display()))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(file)),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => Err(format!("{}: {error}", path.display())),
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
            .filter_map(|entry| {
                let text = fs::read_to_string(entry.path()).ok()?;
                let record: OperationRecord = serde_json::from_str(&text).ok()?;
                record.finished_unix_ms.or_else(|| {
                    // A failed final write or dead worker must also refresh
                    // the sidebar, even if it never observed Running.
                    if record.status == Status::Running
                        && !record
                            .worker_pid
                            .is_some_and(|pid| self.worker_running(pid))
                    {
                        Some(
                            entry
                                .metadata()
                                .ok()?
                                .modified()
                                .ok()?
                                .duration_since(UNIX_EPOCH)
                                .ok()?
                                .as_millis() as u64,
                        )
                    } else {
                        None
                    }
                })
            })
            .max()
            .unwrap_or(0)
    }

    fn worker_running(&self, pid: u32) -> bool {
        let Ok(pid) = i32::try_from(pid) else {
            return false;
        };
        if pid <= 0 {
            return false;
        }
        // SAFETY: signal 0 only checks whether the process exists.
        unsafe {
            libc::kill(pid, 0) == 0
                || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
        }
    }
}

/// Starts this binary with `--run-operation` in a session of its own, with no
/// terminal: closing the details pane or the sidebar does not stop it.
/// `guard` is already locked by the confirming pane. Its open file
/// description is inherited across exec, leaving no unlocked launch gap.
pub fn spawn_operation(
    request: &OperationRequest,
    guard: File,
    binary: &Path,
) -> io::Result<Child> {
    let json = serde_json::to_vec(request).map_err(io::Error::other)?;
    let fd = guard.as_raw_fd();
    let mut command = Command::new(binary);
    command
        .arg("--run-operation")
        .arg("-")
        .arg(fd.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // SAFETY: these libc calls allocate nothing and change only the child.
    // The captured descriptor stays open until spawn returns.
    unsafe {
        command.pre_exec(move || {
            if libc::fcntl(fd, libc::F_SETFD, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    // The confirmed manifest can exceed the OS argument size limit. Closing
    // stdin after writing also lets the worker proceed if the pane closes.
    child.stdin.take().expect("piped stdin").write_all(&json)?;
    Ok(child)
}

/// Own the inherited reservation. Further inheritance is explicit in the
/// Herdr command adapter. A bad descriptor is an ordinary startup error.
pub fn inherited_lock(fd: i32) -> io::Result<File> {
    if fd < 3 {
        return Err(io::Error::other("invalid operation lock descriptor"));
    }
    // SAFETY: fcntl validates fd; only a successful duplicate becomes owned.
    unsafe {
        let copy = libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3);
        if copy == -1 {
            return Err(io::Error::last_os_error());
        }
        libc::close(fd);
        Ok(File::from_raw_fd(copy))
    }
}
