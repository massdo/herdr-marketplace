use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::application::ports::Operations;
use crate::domain::operation::{OperationRecord, OperationRequest, Status, record_file_name};
use crate::domain::source::PluginSource;

/// Results under `<state dir>/operations`, one JSON file per source, and
/// `operation.lock` next to them.
pub struct FsOperations {
    dir: PathBuf,
    cache: RefCell<HashMap<PathBuf, CachedRecord>>,
}

struct CachedRecord {
    stamp: (SystemTime, u64, u64),
    record: Option<OperationRecord>,
}

impl FsOperations {
    pub fn new(state_dir: PathBuf) -> Self {
        Self {
            dir: state_dir.join("operations"),
            cache: RefCell::new(HashMap::new()),
        }
    }

    fn load_path(&self, path: &Path) -> Option<OperationRecord> {
        // Stat the opened file, so an atomic replacement cannot cache old
        // content under the new file's timestamp.
        let metadata = fs::metadata(path).ok()?;
        let stamp = (metadata.modified().ok()?, metadata.len(), metadata.ino());
        if let Some(cached) = self
            .cache
            .borrow()
            .get(path)
            .filter(|cached| cached.stamp == stamp)
        {
            return cached.record.clone();
        }
        let file = File::open(path).ok()?;
        let metadata = file.metadata().ok()?;
        let stamp = (metadata.modified().ok()?, metadata.len(), metadata.ino());
        let record = serde_json::from_reader(file).ok();
        self.cache
            .borrow_mut()
            .insert(path.to_path_buf(), CachedRecord { stamp, record });
        self.cache.borrow().get(path)?.record.clone()
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
        self.load_path(&self.dir.join(record_file_name(source)))
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
                let record = self.load_path(&entry.path())?;
                record.finished_unix_ms.or_else(|| {
                    // A failed final write or dead worker must also refresh
                    // the sidebar, even if it never observed Running.
                    if record.status == Status::Running
                        && !record
                            .worker_pid
                            .is_some_and(|pid| self.worker_running(pid, record.worker_started))
                    {
                        Some(
                            SystemTime::now()
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

    fn worker_started(&self, pid: u32) -> Option<u64> {
        let Ok(pid) = i32::try_from(pid) else {
            return None;
        };
        if pid <= 0 {
            return None;
        }
        process_start(pid)
    }
}

#[cfg(target_os = "macos")]
fn process_start(pid: i32) -> Option<u64> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    // SAFETY: proc_pidinfo writes at most size bytes into this valid buffer;
    // it is read only when the entire structure was returned.
    unsafe {
        if libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        ) != size
        {
            return None;
        }
        let info = info.assume_init();
        (info.pbi_status != libc::SZOMB)
            .then_some(info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec)
    }
}

#[cfg(target_os = "linux")]
fn process_start(pid: i32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command field can contain spaces and parentheses. Fields after
    // its final ')' start with state (3); starttime is field 22.
    let mut fields = stat.rsplit_once(')')?.1.split_whitespace();
    if matches!(fields.next()?, "Z" | "X") {
        return None;
    }
    fields.nth(18)?.parse().ok()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn process_start(_: i32) -> Option<u64> {
    None
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

/// Own the inherited reservation and restore close-on-exec, keeping it out
/// of Herdr and its builds. A bad descriptor is an ordinary startup error.
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
