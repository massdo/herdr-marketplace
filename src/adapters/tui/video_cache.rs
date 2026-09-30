//! Videos belong to an open Marketplace, rather than to a replaceable
//! details pane. The sidebar owns downloads; details read their growing files.

use std::collections::HashMap;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::graphics::VIDEO_LIMIT;
use super::video::Download;
use crate::adapters::download_cancel::Cancellation;

const SOCKET: &str = "cache.sock";
const TICK: Duration = Duration::from_millis(20);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Progress {
    pub received: u64,
    pub total: Option<u64>,
    pub complete: bool,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct CachedVideo {
    pub path: PathBuf,
}

impl CachedVideo {
    fn state_path(&self) -> PathBuf {
        self.path.with_extension("json")
    }

    pub fn progress(&self) -> Result<Progress, String> {
        let bytes = fs::read(self.state_path())
            .map_err(|_| "Marketplace's video cache is closed".to_string())?;
        let progress: Progress =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        if let Some(error) = &progress.error {
            return Err(error.clone());
        }
        Ok(progress)
    }

    fn save(&self, progress: &Progress) -> Result<(), String> {
        let folder = self.path.parent().ok_or("video has no folder")?;
        let mut file =
            tempfile::NamedTempFile::new_in(folder).map_err(|error| error.to_string())?;
        serde_json::to_writer(&mut file, progress).map_err(|error| error.to_string())?;
        file.persist(self.state_path())
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    fn remove(&self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(self.state_path());
        let _ = fs::remove_file(self.path.with_extension("stream"));
    }

    pub fn header(&self) -> Option<StreamHeader> {
        serde_json::from_slice(&fs::read(self.path.with_extension("stream")).ok()?).ok()
    }

    fn save_header(&self, header: &StreamHeader) -> Result<(), String> {
        let mut file = tempfile::NamedTempFile::new_in(self.path.parent().unwrap())
            .map_err(|e| e.to_string())?;
        serde_json::to_writer(&mut file, header).map_err(|e| e.to_string())?;
        file.persist(self.path.with_extension("stream"))
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

struct Job {
    file: CachedVideo,
    stop: Cancellation,
    thread: JoinHandle<()>,
}

struct Local {
    folder: PathBuf,
    files: Mutex<HashMap<String, CachedVideo>>,
    jobs: Mutex<Vec<Job>>,
}

enum Backend {
    Local(Local),
    Shared(PathBuf),
}

pub struct Cache(Backend);

impl Cache {
    pub fn new(folder: PathBuf) -> Self {
        Self(Backend::Local(Local {
            folder,
            files: Mutex::new(HashMap::new()),
            jobs: Mutex::new(Vec::new()),
        }))
    }

    pub fn shared(folder: PathBuf) -> Self {
        Self(Backend::Shared(folder))
    }

    /// Starts at most one transfer for a URL; it keeps running when its
    /// details pane stops playback or is replaced by another plugin.
    pub fn get(&self, url: &str, download: Arc<dyn Download>) -> Result<CachedVideo, String> {
        match &self.0 {
            Backend::Shared(folder) => {
                let mut socket = UnixStream::connect(folder.join(SOCKET))
                    .map_err(|_| "Open Marketplace to play this video".to_string())?;
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .map_err(|e| e.to_string())?;
                socket
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .map_err(|e| e.to_string())?;
                serde_json::to_writer(&mut socket, url).map_err(|error| error.to_string())?;
                socket
                    .shutdown(std::net::Shutdown::Write)
                    .map_err(|error| error.to_string())?;
                let answer: Result<String, String> = serde_json::from_reader(socket.take(8192))
                    .map_err(|error| error.to_string())?;
                let name = answer?;
                if Path::new(&name).components().count() != 1 || !name.ends_with(".mp4") {
                    return Err("invalid video cache answer".into());
                }
                Ok(CachedVideo {
                    path: folder.join(name),
                })
            }
            Backend::Local(local) => {
                let mut files = local.files.lock().unwrap();
                if let Some(file) = files.get(url)
                    && file.path.exists()
                    && file.progress().is_ok()
                {
                    return Ok(file.clone());
                }
                DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(&local.folder)
                    .map_err(|error| error.to_string())?;
                static NEXT: AtomicU64 = AtomicU64::new(1);
                let (file, mut output) = loop {
                    let number = NEXT.fetch_add(1, Ordering::Relaxed);
                    let path = local
                        .folder
                        .join(format!("{}-{number}.mp4", std::process::id()));
                    match OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(&path)
                    {
                        Ok(output) => break (CachedVideo { path }, output),
                        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                        Err(error) => return Err(error.to_string()),
                    }
                };
                file.save(&Progress::default())?;
                let stop = Cancellation::default();
                let (saved, cancelled, address) = (file.clone(), stop.clone(), url.to_string());
                let thread = thread::spawn(move || {
                    let (head_file, head_download, head_url, head_stop) = (
                        saved.clone(),
                        download.clone(),
                        address.clone(),
                        cancelled.clone(),
                    );
                    let header = thread::spawn(move || {
                        if let Ok(Some(header)) =
                            head_download.streaming_header(&head_url, &head_stop)
                        {
                            let _ = head_file.save_header(&header);
                        }
                    });
                    let mut state = Progress::default();
                    let mut last = None::<Instant>;
                    let result = download.download(
                        &address,
                        VIDEO_LIMIT,
                        &mut output,
                        &cancelled,
                        &mut |received, total| {
                            state.received = received;
                            state.total = total;
                            if last.is_none_or(|at| at.elapsed() >= Duration::from_millis(100)) {
                                let _ = saved.save(&state);
                                last = Some(Instant::now());
                            }
                        },
                    );
                    // Close the writer before marking a complete file ready.
                    drop(output);
                    match result {
                        Ok(()) => state.complete = true,
                        Err(error) => state.error = Some(error),
                    }
                    let _ = saved.save(&state);
                    // A completed original no longer needs range requests.
                    cancelled.cancel();
                    let _ = header.join();
                    if state.error.is_some() {
                        let _ = fs::remove_file(&saved.path);
                        let _ = fs::remove_file(saved.path.with_extension("stream"));
                    }
                });
                local.jobs.lock().unwrap().push(Job {
                    file: file.clone(),
                    stop,
                    thread,
                });
                files.insert(url.to_string(), file.clone());
                Ok(file)
            }
        }
    }
}

impl Drop for Cache {
    fn drop(&mut self) {
        if let Backend::Local(local) = &mut self.0 {
            let jobs = std::mem::take(local.jobs.get_mut().unwrap());
            for job in &jobs {
                job.stop.cancel();
            }
            for job in jobs {
                let _ = job.thread.join();
                job.file.remove();
            }
        }
    }
}

/// A private Unix socket lets replaceable details processes ask the sidebar
/// for the same files. No remote URL is handed to FFmpeg.
pub struct CacheServer {
    folder: PathBuf,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// Herdr can kill a pane before Rust destructors run. A separate process
/// owns the cache and transfers, then cleans them when the sidebar is gone.
pub struct CacheProcess {
    folder: PathBuf,
    child: Child,
}

impl CacheProcess {
    pub fn start(state: &Path) -> Result<Self, String> {
        let owner = std::process::id();
        let folder = state.join("videos").join(owner.to_string());
        let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
        command
            .arg("--video-cache")
            .arg(owner.to_string())
            .env("HERDR_PLUGIN_STATE_DIR", state)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: setsid is async-signal-safe. The helper must survive the
        // termination of the pane's process group to finish its cleanup.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().map_err(|e| e.to_string())?;
        let mut cache = Self { folder, child };
        let deadline = Instant::now() + Duration::from_secs(2);
        while !cache.folder.join(SOCKET).exists() {
            if cache.child.try_wait().map_err(|e| e.to_string())?.is_some() {
                return Err("video cache could not start".into());
            }
            if Instant::now() >= deadline {
                return Err("video cache did not start in time".into());
            }
            thread::sleep(Duration::from_millis(5));
        }
        Ok(cache)
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }
}

impl Drop for CacheProcess {
    fn drop(&mut self) {
        // SAFETY: our child registers SIGTERM to unwind and join transfers.
        if self.child.try_wait().ok().flatten().is_none() {
            unsafe {
                libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM);
            }
        }
        let _ = self.child.wait();
    }
}

/// Only invoked by CacheProcess; the folder is derived from the plugin's
/// state directory, not from a path supplied over the socket or CLI.
pub fn run_cache_process(
    state: &Path,
    owner: libc::pid_t,
    download: Arc<dyn Download>,
) -> Result<(), String> {
    if owner <= 0 {
        return Err("invalid video cache owner".into());
    }
    let stopped = Arc::new(AtomicBool::new(false));
    let signal = signal_hook::flag::register(signal_hook::consts::SIGTERM, stopped.clone())
        .map_err(|e| e.to_string())?;
    let result = (|| {
        let server = CacheServer::start(state.join("videos").join(owner.to_string()), download)?;
        loop {
            // SAFETY: signal 0 only checks the owner; errors other than ESRCH
            // do not destroy the cache of a live sidebar.
            let gone = unsafe { libc::kill(owner, 0) } == -1
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
            if gone || stopped.load(Ordering::Acquire) {
                break;
            }
            thread::sleep(TICK);
        }
        drop(server);
        Ok(())
    })();
    signal_hook::low_level::unregister(signal);
    result
}

impl CacheServer {
    pub fn start(folder: PathBuf, download: Arc<dyn Download>) -> Result<Self, String> {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&folder)
            .map_err(|e| e.to_string())?;
        let listener = UnixListener::bind(folder.join(SOCKET)).map_err(|e| e.to_string())?;
        fs::set_permissions(folder.join(SOCKET), fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let directory = folder.clone();
        let thread = thread::spawn(move || {
            let cache = Cache::new(directory);
            while !stopped.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut socket, _)) => {
                        let _ = socket.set_read_timeout(Some(Duration::from_millis(500)));
                        let _ = socket.set_write_timeout(Some(Duration::from_millis(500)));
                        let answer = serde_json::from_reader::<_, String>((&mut socket).take(8192))
                            .map_err(|e| e.to_string())
                            .and_then(|url| cache.get(&url, download.clone()))
                            .map(|file| {
                                file.path
                                    .file_name()
                                    .unwrap()
                                    .to_string_lossy()
                                    .into_owned()
                            });
                        let _ = serde_json::to_writer(socket, &answer);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(TICK)
                    }
                    Err(_) => break,
                }
            }
            // The cache cancels and joins every transfer before removing files.
        });
        Ok(Self {
            folder,
            stop,
            thread: Some(thread),
        })
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }
}

impl Drop for CacheServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_dir_all(&self.folder);
    }
}

/// Clean session folders left by a forcibly killed sidebar; live sessions
/// retain their files even while no details pane is open.
pub fn clean_stale(folder: &Path) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let pid = name.parse::<libc::pid_t>().ok().or_else(|| {
            let (pid, number) = name.strip_suffix(".mp4")?.split_once('-')?;
            number.parse::<u64>().ok()?;
            pid.parse().ok()
        });
        let Some(pid) = pid.filter(|pid| *pid > 0) else {
            continue;
        };
        // SAFETY: signal 0 only checks that the owner process exists.
        let gone = unsafe { libc::kill(pid, 0) } == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        if gone {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                let _ = fs::remove_dir_all(entry.path());
            } else {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

/// A front-loaded `moov` lets a non-seekable pipe play while bytes arrive.
/// Tail-loaded metadata is handled by `StreamHeader` when ranges are available.
pub fn stream_metadata(path: &Path) -> Option<Option<u64>> {
    file_metadata(path, false)
}

pub fn file_metadata(path: &Path, complete: bool) -> Option<Option<u64>> {
    use std::io::{Seek, SeekFrom};
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let mut offset = 0u64;
    while offset.checked_add(8)? <= length {
        file.seek(SeekFrom::Start(offset)).ok()?;
        let mut header = [0; 8];
        file.read_exact(&mut header).ok()?;
        let mut size = u64::from(u32::from_be_bytes(header[..4].try_into().ok()?));
        let mut head = 8;
        if size == 1 {
            let mut extended = [0; 8];
            file.read_exact(&mut extended).ok()?;
            size = u64::from_be_bytes(extended);
            head = 16;
        }
        if size < head || size > VIDEO_LIMIT {
            return None;
        }
        let end = offset.checked_add(size)?;
        if &header[4..] == b"mdat" && !complete {
            return None;
        }
        if end > length {
            return None;
        }
        if &header[4..] == b"moov" {
            let mut moov = vec![0; (size - head) as usize];
            file.read_exact(&mut moov).ok()?;
            return Some(movie_duration(&moov));
        }
        offset = end;
    }
    None
}

/// A tail-loaded moov relocated before mdat for the streaming decoder.
/// The original file stays intact for subsequent direct seeks.
#[derive(Clone, Serialize, Deserialize)]
pub struct StreamHeader {
    pub prefix: Vec<u8>,
    pub offset: u64,
    pub end: u64,
    pub duration: Option<u64>,
}

/// Locate a single mdat following small leading boxes. A front-loaded moov
/// already streams; unsupported layouts safely use the original file.
pub fn media_range(bytes: &[u8]) -> Option<(usize, u64)> {
    let mut offset = 0usize;
    while offset.checked_add(8)? <= bytes.len() {
        let size = u64::from(u32::from_be_bytes(
            bytes[offset..offset + 4].try_into().ok()?,
        ));
        let kind = &bytes[offset + 4..offset + 8];
        if kind == b"moov" {
            return None;
        }
        let size = if size == 1 {
            u64::from_be_bytes(bytes.get(offset + 8..offset + 16)?.try_into().ok()?)
        } else {
            size
        };
        if size < 8 {
            return None;
        }
        let end = (offset as u64).checked_add(size)?;
        if kind == b"mdat" {
            return (end <= VIDEO_LIMIT).then_some((offset, end));
        }
        offset = usize::try_from(end).ok()?;
    }
    None
}

pub fn relocate_header(front: &[u8], tail: &[u8], offset: usize, end: u64) -> Option<StreamHeader> {
    let mut cursor = 0usize;
    while cursor.checked_add(8)? <= tail.len() {
        let size = u32::from_be_bytes(tail[cursor..cursor + 4].try_into().ok()?) as usize;
        if size < 8 || cursor.checked_add(size)? > tail.len() {
            return None;
        }
        if &tail[cursor + 4..cursor + 8] == b"moov" {
            let mut moov = tail[cursor..cursor + size].to_vec();
            let duration = movie_duration(&moov[8..]);
            shift_offsets(&mut moov[8..], size as u64, 0)?;
            let mut prefix = front.get(..offset)?.to_vec();
            prefix.extend(moov);
            return Some(StreamHeader {
                prefix,
                offset: offset as u64,
                end,
                duration,
            });
        }
        if &tail[cursor + 4..cursor + 8] == b"mdat" {
            return None;
        }
        cursor += size;
    }
    None
}

fn shift_offsets(mut boxes: &mut [u8], delta: u64, depth: usize) -> Option<()> {
    if depth > 12 {
        return None;
    }
    while !boxes.is_empty() {
        let size = u32::from_be_bytes(boxes.get(..4)?.try_into().ok()?) as usize;
        if size < 8 || size > boxes.len() {
            return None;
        }
        let (box_bytes, rest) = boxes.split_at_mut(size);
        let kind: [u8; 4] = box_bytes[4..8].try_into().ok()?;
        let data = &mut box_bytes[8..];
        match &kind {
            b"trak" | b"mdia" | b"minf" | b"stbl" => shift_offsets(data, delta, depth + 1)?,
            b"stco" | b"co64" => {
                let count = u32::from_be_bytes(data.get(4..8)?.try_into().ok()?) as usize;
                let width = if &kind == b"stco" { 4 } else { 8 };
                if count.checked_mul(width)?.checked_add(8)? != data.len() {
                    return None;
                }
                for entry in data[8..].chunks_exact_mut(width) {
                    if width == 4 {
                        let value = u32::from_be_bytes(entry.try_into().ok()?);
                        entry.copy_from_slice(
                            &value.checked_add(u32::try_from(delta).ok()?)?.to_be_bytes(),
                        );
                    } else {
                        let value = u64::from_be_bytes(entry.try_into().ok()?);
                        entry.copy_from_slice(&value.checked_add(delta)?.to_be_bytes());
                    }
                }
            }
            b"cmov" => return None,
            _ => {}
        }
        boxes = rest;
    }
    Some(())
}

fn movie_duration(mut boxes: &[u8]) -> Option<u64> {
    while boxes.len() >= 8 {
        let size = u32::from_be_bytes(boxes[..4].try_into().ok()?) as usize;
        if size < 8 || size > boxes.len() {
            return None;
        }
        if &boxes[4..8] == b"mvhd" {
            let data = &boxes[8..size];
            let (scale, duration) = match *data.first()? {
                0 => (
                    u32::from_be_bytes(data.get(12..16)?.try_into().ok()?),
                    u64::from(u32::from_be_bytes(data.get(16..20)?.try_into().ok()?)),
                ),
                1 => (
                    u32::from_be_bytes(data.get(20..24)?.try_into().ok()?),
                    u64::from_be_bytes(data.get(24..32)?.try_into().ok()?),
                ),
                _ => return None,
            };
            return duration
                .checked_mul(1000)?
                .checked_div(u64::from(scale))
                .map(|ms| ms.min(600_000));
        }
        boxes = &boxes[size..];
    }
    None
}
