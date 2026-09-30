//! README videos, played on a layer of the details pane as animations are
//! (see `animation`). A click downloads the video into the plugin's state
//! folder, then the private FFmpeg turns it into raw frames the size of its
//! block, read at their pace through a pipe: FFmpeg waits while the pipe is
//! full, so one frame at a time is in memory. Every stop kills FFmpeg and
//! deletes the file.

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{ErrorKind, Read};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{ChildStderr, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::animation::Spot;
use super::graphics::VIDEO_LIMIT;
use crate::adapters::image_fetch::ImageFetcher;
use crate::adapters::images::rgb_png;
use crate::adapters::pane_graphics::{Cells, Layer};

/// Frames a video shows per second, at most.
pub const VIDEO_FPS: u32 = 10;
/// Longest side of a frame, in pixels: the terminal stretches it to the
/// cells of its block.
pub const VIDEO_SIDE: u32 = 800;
/// How long FFmpeg may take to give the first frame.
pub const FIRST_FRAME: Duration = Duration::from_secs(10);
/// A video file older than this was left by a pane that is gone.
pub const STALE: Duration = Duration::from_secs(60 * 60);
/// How long a frame shows.
const FRAME: Duration = Duration::from_millis(1000 / VIDEO_FPS as u64);
/// Bytes of FFmpeg's errors kept.
const ERRORS: usize = 64 * 1024;
/// How often a playback looks whether it must stop.
const TICK: Duration = Duration::from_millis(20);

/// What a playback tells the pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoEvent {
    /// Bytes downloaded, of the whole size when known.
    Downloading(u64, Option<u64>),
    /// The first frame shows.
    Playing,
    /// Its end, or the pane no longer shows its block.
    Ended,
    Failed(String),
}

/// Where a video comes from.
pub trait Download: Send + Sync {
    fn download(
        &self,
        url: &str,
        limit: u64,
        file: &mut File,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<(), String>;
}

impl Download for ImageFetcher {
    fn download(
        &self,
        url: &str,
        limit: u64,
        file: &mut File,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<(), String> {
        ImageFetcher::download(self, url, limit, file, progress).map_err(|error| error.to_string())
    }
}

/// Where the frames of a video show: a layer of the pane.
pub trait Frames: Send {
    fn show(&mut self, png: &[u8], width: u32, height: u32, cells: Cells) -> Result<(), String>;
}

impl Frames for Layer {
    fn show(&mut self, png: &[u8], width: u32, height: u32, cells: Cells) -> Result<(), String> {
        Layer::show(self, png, width, height, cells)
    }
}

/// Opens the layer the frames of a video show on.
pub type OpenFrames = Box<dyn FnOnce() -> Result<Box<dyn Frames>, String> + Send>;

/// A video to play, and what it plays with.
pub struct Video {
    pub url: String,
    /// It starts over at its end.
    pub looped: bool,
    /// Pixels of its block, which its frames fit in.
    pub fit: (u32, u32),
    /// Folder of its downloaded file.
    pub folder: PathBuf,
    pub ffmpeg: PathBuf,
    pub download: Arc<dyn Download>,
    /// Opens the layer its frames show on, at the first one.
    pub open: OpenFrames,
    /// Whether Herdr shows the pane.
    pub visible: Arc<AtomicBool>,
    /// How long FFmpeg may take to give the first frame.
    pub first_frame: Duration,
    pub events: Arc<dyn Fn(VideoEvent) + Send + Sync>,
}

/// A video playing in a thread of its own, until it ends or goes.
pub struct Playback {
    spots: Sender<Option<Spot>>,
    /// The last spot sent to the thread.
    spot: Option<Option<Spot>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Playback {
    pub fn start(video: Video) -> Self {
        let (spots, received) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = thread::spawn(move || play(video, received, stopped));
        Self {
            spots,
            spot: None,
            stop,
            thread: Some(thread),
        }
    }

    /// Where the pane shows the block of the video; `None`, out of view,
    /// stops it.
    pub fn show(&mut self, spot: Option<Spot>) {
        if self.spot != Some(spot) {
            self.spot = Some(spot);
            let _ = self.spots.send(spot);
        }
    }
}

/// Once dropped, FFmpeg no longer runs, the file is deleted and the layer
/// closed.
impl Drop for Playback {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Pixels of the frames for a block of `fit` pixels: within it and
/// `VIDEO_SIDE`, in its proportions, 2 × 2 at least.
pub fn frame_size((width, height): (u32, u32)) -> (u32, u32) {
    let scale = (f64::from(VIDEO_SIDE) / f64::from(width.max(height).max(1))).min(1.0);
    let side = |pixels: u32| ((f64::from(pixels) * scale).round() as u32).max(2);
    (side(width), side(height))
}

/// FFmpeg reads `input`, as MP4 or MOV and only as a file, 600 s of it at
/// most, and writes raw RGB frames of `width` × `height` pixels at
/// `VIDEO_FPS`, without sound: the video fits in them, black bands around.
pub fn ffmpeg_args(input: &Path, width: u32, height: u32) -> Vec<String> {
    let filters = format!(
        "fps={VIDEO_FPS},scale={width}:{height}:force_original_aspect_ratio=decrease,pad={width}:{height}:(ow-iw)/2:(oh-ih)/2"
    );
    let mut args: Vec<String> = [
        "-nostdin",
        "-v",
        "error",
        "-threads",
        "2",
        "-protocol_whitelist",
        "file",
        "-f",
        "mov",
        "-t",
        "600",
        "-i",
    ]
    .map(String::from)
    .to_vec();
    args.push(input.to_string_lossy().into_owned());
    args.extend(
        [
            "-an", "-vf", &filters, "-f", "rawvideo", "-pix_fmt", "rgb24", "pipe:1",
        ]
        .map(String::from),
    );
    args
}

/// Whether frame `index`, due `index` frames after the first, is more than a
/// frame late `elapsed` after the first: it is then skipped.
pub fn late(index: u32, elapsed: Duration) -> bool {
    elapsed > FRAME * index + FRAME
}

/// Deletes the video files a pane left in `folder`: those of a process that
/// is gone, or older than `STALE`, never a newer one of a living process.
pub fn clean_stale(folder: &Path) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(owner) else {
            continue;
        };
        // SAFETY: signal 0 only checks that the process exists.
        let gone = unsafe { libc::kill(pid, 0) } == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        let old = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > STALE);
        if gone || old {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// The process a video file belongs to: `<pid>-<n>.mp4`.
fn owner(name: &str) -> Option<libc::pid_t> {
    let (pid, number) = name.strip_suffix(".mp4")?.split_once('-')?;
    number.parse::<u64>().ok()?;
    pid.parse().ok().filter(|pid| *pid > 0)
}

/// A new file in `folder`, readable by its owner only: `<pid>-<n>.mp4`.
fn new_file(folder: &Path) -> std::io::Result<(PathBuf, File)> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(folder)?;
    loop {
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = folder.join(format!("{}-{number}.mp4", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
}

/// Why a playback stops.
enum Halt {
    /// Its owner dropped it: nobody to tell.
    Dropped,
    /// Its end, or the pane no longer shows its block.
    Ended,
    Failed(String),
}

/// The thread of a playback: downloads the video, plays it, then cleans up
/// and tells why it stopped.
fn play(video: Video, spots: Receiver<Option<Spot>>, stop: Arc<AtomicBool>) {
    let Video {
        url,
        looped,
        fit,
        folder,
        ffmpeg,
        download,
        open,
        visible,
        first_frame,
        events,
    } = video;
    let mut session = Session {
        spots,
        stop,
        visible,
        spot: None,
        seen: false,
        open: Some(open),
        layer: None,
        first_frame,
        events: events.clone(),
        playing: false,
    };
    let halt = match new_file(&folder) {
        Err(error) => Halt::Failed(format!("cannot create the video file: {error}")),
        Ok((path, file)) => {
            let played = session
                .download(&download, &url, file)
                .and_then(|()| session.run(&ffmpeg, &path, frame_size(fit), looped));
            session.layer = None;
            let _ = fs::remove_file(&path);
            played.err().unwrap_or(Halt::Ended)
        }
    };
    match halt {
        Halt::Dropped => {}
        Halt::Ended => events(VideoEvent::Ended),
        Halt::Failed(reason) => events(VideoEvent::Failed(reason)),
    }
}

struct Session {
    spots: Receiver<Option<Spot>>,
    stop: Arc<AtomicBool>,
    visible: Arc<AtomicBool>,
    /// Where the pane shows the block; `None` until it tells.
    spot: Option<Spot>,
    /// Herdr showed the pane since the start.
    seen: bool,
    open: Option<OpenFrames>,
    layer: Option<Box<dyn Frames>>,
    first_frame: Duration,
    events: Arc<dyn Fn(VideoEvent) + Send + Sync>,
    /// The first frame showed.
    playing: bool,
}

impl Session {
    /// Why the playback must stop now, if it must: its owner dropped it,
    /// the pane no longer shows its block, or Herdr hid the pane after
    /// showing it.
    fn halted(&mut self) -> Option<Halt> {
        if self.stop.load(Ordering::Acquire) {
            return Some(Halt::Dropped);
        }
        while let Ok(spot) = self.spots.try_recv() {
            match spot {
                Some(spot) => self.spot = Some(spot),
                None => return Some(Halt::Ended),
            }
        }
        if self.visible.load(Ordering::Acquire) {
            self.seen = true;
        } else if self.seen {
            return Some(Halt::Ended);
        }
        None
    }

    /// Downloads the video into `file`, telling its progress, from a thread
    /// of its own: a stop does not wait for the network, and the file,
    /// deleted, goes with that thread.
    fn download(
        &mut self,
        download: &Arc<dyn Download>,
        url: &str,
        mut file: File,
    ) -> Result<(), Halt> {
        let (done, result) = mpsc::channel();
        let (download, url) = (download.clone(), url.to_string());
        let (events, stop) = (self.events.clone(), self.stop.clone());
        thread::spawn(move || {
            let mut progress = |received, total| {
                if !stop.load(Ordering::Acquire) {
                    events(VideoEvent::Downloading(received, total));
                }
            };
            let _ = done.send(download.download(&url, VIDEO_LIMIT, &mut file, &mut progress));
        });
        loop {
            if let Some(halt) = self.halted() {
                return Err(halt);
            }
            match result.recv_timeout(TICK) {
                Ok(downloaded) => return downloaded.map_err(Halt::Failed),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(Halt::Failed("the download stopped".into()));
                }
            }
        }
    }

    /// Plays the file until its end, over and over when `looped`.
    fn run(
        &mut self,
        ffmpeg: &Path,
        path: &Path,
        size: (u32, u32),
        looped: bool,
    ) -> Result<(), Halt> {
        loop {
            let shown = self.pass(ffmpeg, path, size)?;
            if !looped || shown == 0 {
                return Ok(());
            }
        }
    }

    /// One pass of FFmpeg over the file, at a low priority: the frames
    /// shown. FFmpeg is killed if it does not end by itself, and waited for
    /// in every case.
    fn pass(
        &mut self,
        ffmpeg: &Path,
        path: &Path,
        (width, height): (u32, u32),
    ) -> Result<usize, Halt> {
        let mut command = Command::new(ffmpeg);
        command
            .args(ffmpeg_args(path, width, height))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // SAFETY: setpriority is async-signal-safe and touches nothing else.
        unsafe {
            command.pre_exec(|| {
                libc::setpriority(libc::PRIO_PROCESS, 0, 10);
                Ok(())
            });
        }
        let mut child = command
            .spawn()
            .map_err(|error| Halt::Failed(format!("cannot run FFmpeg: {error}")))?;
        let errors = child
            .stderr
            .take()
            .map(|stderr| thread::spawn(move || first_errors(stderr)));
        let shown = match child.stdout.take() {
            Some(stdout) => self.frames(stdout, width, height),
            None => Err(Halt::Failed("FFmpeg has no output".into())),
        };
        if shown.is_err() {
            let _ = child.kill();
        }
        let status = child.wait();
        let errors = errors
            .and_then(|errors| errors.join().ok())
            .unwrap_or_default();
        let shown = shown?;
        match status {
            Ok(status) if status.success() => Ok(shown),
            Ok(status) => Err(Halt::Failed(
                errors
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                    .map_or_else(|| format!("FFmpeg stopped: {status}"), str::to_string),
            )),
            Err(error) => Err(Halt::Failed(error.to_string())),
        }
    }

    /// Shows the frames FFmpeg writes, each at its time, until their end:
    /// how many showed. A late frame is skipped.
    fn frames(&mut self, mut stdout: ChildStdout, width: u32, height: u32) -> Result<usize, Halt> {
        let mut frame = vec![0; width as usize * height as usize * 3];
        let deadline = Instant::now() + self.first_frame;
        let mut start = None::<Instant>;
        let mut shown = 0;
        let mut index = 0;
        loop {
            let first = start.is_none().then_some(deadline);
            if !self.read(&mut stdout, &mut frame, first)? {
                return Ok(shown);
            }
            match start {
                Some(start) if late(index, start.elapsed()) => {
                    index += 1;
                    continue;
                }
                Some(start) => self.wait_until(start + FRAME * index)?,
                None => {}
            }
            self.show(&frame, width, height)?;
            start.get_or_insert_with(Instant::now);
            shown += 1;
            index += 1;
            if !self.playing {
                self.playing = true;
                (self.events)(VideoEvent::Playing);
            }
        }
    }

    /// Fills `frame` from `stdout`, before `deadline` if any; false at the
    /// end of the frames, where a part of one is dropped.
    fn read(
        &mut self,
        stdout: &mut ChildStdout,
        frame: &mut [u8],
        deadline: Option<Instant>,
    ) -> Result<bool, Halt> {
        let mut filled = 0;
        while filled < frame.len() {
            if let Some(halt) = self.halted() {
                return Err(halt);
            }
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return Err(Halt::Failed(format!(
                    "no frame after {} s",
                    self.first_frame.as_secs_f32()
                )));
            }
            if !readable(stdout, TICK) {
                continue;
            }
            match stdout.read(&mut frame[filled..]) {
                Ok(0) => return Ok(false),
                Ok(read) => filled += read,
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => return Err(Halt::Failed(error.to_string())),
            }
        }
        Ok(true)
    }

    fn wait_until(&mut self, due: Instant) -> Result<(), Halt> {
        loop {
            if let Some(halt) = self.halted() {
                return Err(halt);
            }
            let left = due.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(());
            }
            thread::sleep(left.min(TICK));
        }
    }

    /// Shows `frame` over the rows of the block the pane shows, cut to them,
    /// once it shows them. The layer opens with the first frame.
    fn show(&mut self, frame: &[u8], width: u32, height: u32) -> Result<(), Halt> {
        let spot = loop {
            if let Some(halt) = self.halted() {
                return Err(halt);
            }
            match self.spot {
                Some(spot) if self.seen => break spot,
                _ => thread::sleep(TICK),
            }
        };
        let band =
            |edge: u16| (u64::from(edge) * u64::from(height) / u64::from(spot.rows.max(1))) as u32;
        let top = band(spot.first).min(height - 1);
        let bottom = band(spot.first + spot.cells.rows).clamp(top + 1, height);
        let row = width as usize * 3;
        let part = &frame[top as usize * row..bottom as usize * row];
        let png = rgb_png(width, bottom - top, part).map_err(Halt::Failed)?;
        let shown = match &mut self.layer {
            Some(layer) => layer.show(&png, width, bottom - top, spot.cells),
            None => match self.open.take() {
                Some(open) => open().and_then(|opened| {
                    self.layer
                        .insert(opened)
                        .show(&png, width, bottom - top, spot.cells)
                }),
                None => Err("the layer of the video is closed".into()),
            },
        };
        shown.map_err(Halt::Failed)
    }
}

/// Whether `stdout` has bytes, or its end, to read within `wait`.
fn readable(stdout: &ChildStdout, wait: Duration) -> bool {
    let mut poll = libc::pollfd {
        fd: stdout.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid pollfd.
    unsafe { libc::poll(&mut poll, 1, wait.as_millis() as libc::c_int) > 0 }
}

/// What FFmpeg writes on its error output, its first `ERRORS` bytes kept;
/// the rest is read too, so that it never waits on a full pipe.
fn first_errors(mut stderr: ChildStderr) -> String {
    let mut kept = Vec::new();
    let mut buffer = [0; 4096];
    while let Ok(read) = stderr.read(&mut buffer) {
        if read == 0 {
            break;
        }
        let room = ERRORS.saturating_sub(kept.len());
        kept.extend_from_slice(&buffer[..read.min(room)]);
    }
    String::from_utf8_lossy(&kept).into_owned()
}
