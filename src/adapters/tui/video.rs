//! README videos, played on a layer of the details pane as animations are
//! (see `animation`). FFmpeg reads a growing cached MP4 through a bounded
//! pipe, then a seekable file once complete. Marketplace owns the transfers.

use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{ChildStderr, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::animation::Spot;
pub use super::video_cache::{Cache, clean_stale};
use super::video_cache::{
    CachedVideo, StreamHeader, file_metadata, media_range, relocate_header, stream_metadata,
};
use crate::adapters::download_cancel::Cancellation;
use crate::adapters::image_fetch::ImageFetcher;
use crate::adapters::images::rgb_png;
use crate::adapters::pane_graphics::{Cells, Layer};

/// Frames a video shows per second, at most.
pub const VIDEO_FPS: u32 = 10;
/// Longest side of a frame, in pixels: the terminal stretches it to the
/// cells of its block.
pub const VIDEO_SIDE: u32 = 800;
/// First-frame decoding time allowed, excluding waits for missing stream data.
pub const FIRST_FRAME: Duration = Duration::from_secs(10);
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
    Paused(bool),
    /// The requested frame needs bytes still arriving in the session cache.
    Buffering {
        milliseconds: u64,
        paused: bool,
    },
    Position {
        milliseconds: u64,
        duration: Option<u64>,
    },
    /// Its natural end; a non-looped video stays stopped.
    Ended,
    /// The pane or its video block left the view; it may autoplay on return.
    Hidden,
    Failed(String),
}

/// Where a video comes from.
pub trait Download: Send + Sync {
    fn streaming_header(
        &self,
        _url: &str,
        _cancellation: &Cancellation,
    ) -> Result<Option<StreamHeader>, String> {
        Ok(None)
    }
    fn download(
        &self,
        url: &str,
        limit: u64,
        file: &mut File,
        cancellation: &Cancellation,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<(), String>;
}

impl Download for ImageFetcher {
    fn streaming_header(
        &self,
        url: &str,
        cancellation: &Cancellation,
    ) -> Result<Option<StreamHeader>, String> {
        let front = self
            .video_range(url, "bytes=0-65535", 65536, cancellation)
            .map_err(|e| e.to_string())?;
        let Some((offset, end)) = media_range(&front) else {
            return Ok(None);
        };
        let tail = self
            .video_range(url, &format!("bytes={end}-"), 1024 * 1024, cancellation)
            .map_err(|e| e.to_string())?;
        Ok(relocate_header(&front, &tail, offset, end))
    }
    fn download(
        &self,
        url: &str,
        limit: u64,
        file: &mut File,
        cancellation: &Cancellation,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<(), String> {
        ImageFetcher::download(self, url, limit, file, cancellation, progress)
            .map_err(|error| error.to_string())
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
    pub start: u64,
    /// It starts over at its end.
    pub looped: bool,
    /// Pixels of its block, which its frames fit in.
    pub fit: (u32, u32),
    pub cache: Arc<Cache>,
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
    controls: Sender<Control>,
    /// The last spot sent to the thread.
    spot: Option<Option<Spot>>,
    stop: Cancellation,
    thread: Option<JoinHandle<()>>,
}

enum Control {
    Pause(bool),
    Seek(u64),
}

impl Playback {
    pub fn start(video: Video) -> Self {
        let (spots, received) = mpsc::channel();
        let (controls, commands) = mpsc::channel();
        let stop = Cancellation::default();
        let stopped = stop.clone();
        let thread = thread::spawn(move || play(video, received, commands, stopped));
        Self {
            spots,
            controls,
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

    pub fn pause(&self, paused: bool) {
        let _ = self.controls.send(Control::Pause(paused));
    }

    pub fn seek(&self, milliseconds: u64) {
        let _ = self.controls.send(Control::Seek(milliseconds));
    }
}

/// Once dropped, FFmpeg stops and the layer closes. The cached file and its
/// transfer remain owned by Marketplace for replay.
impl Drop for Playback {
    fn drop(&mut self) {
        self.stop.cancel();
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

/// Why a playback stops.
enum Halt {
    /// Its owner dropped it: nobody to tell.
    Dropped,
    /// Its natural end.
    Ended,
    Hidden,
    Seek(u64),
    Failed(String),
}

/// The thread of a playback: downloads the video, plays it, then cleans up
/// and tells why it stopped.
fn play(
    video: Video,
    spots: Receiver<Option<Spot>>,
    controls: Receiver<Control>,
    stop: Cancellation,
) {
    let Video {
        url,
        start,
        looped,
        fit,
        cache,
        ffmpeg,
        download,
        open,
        visible,
        first_frame,
        events,
    } = video;
    let mut session = Session {
        spots,
        controls,
        stop,
        visible,
        spot: None,
        seen: false,
        open: Some(open),
        layer: None,
        first_frame,
        events: events.clone(),
        playing: false,
        buffering: false,
        position: start,
        paused: false,
        paused_for: Duration::ZERO,
        duration: None,
        file: None,
        downloaded: None,
    };
    let prepared = cache.get(&url, download).map_err(Halt::Failed);
    let halt = prepared.and_then(|file| {
        session.file = Some(file.clone());
        session.prepare(&file)?;
        session.run(&ffmpeg, &file, frame_size(fit), looped, start)
    });
    session.layer = None;
    let halt = halt.err().unwrap_or(Halt::Ended);
    match halt {
        Halt::Dropped | Halt::Seek(_) => {}
        Halt::Ended => events(VideoEvent::Ended),
        Halt::Hidden => events(VideoEvent::Hidden),
        Halt::Failed(reason) => events(VideoEvent::Failed(reason)),
    }
}

struct Session {
    spots: Receiver<Option<Spot>>,
    controls: Receiver<Control>,
    paused: bool,
    paused_for: Duration,
    duration: Option<u64>,
    file: Option<CachedVideo>,
    downloaded: Option<(u64, Option<u64>)>,
    stop: Cancellation,
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
    buffering: bool,
    position: u64,
}

impl Session {
    /// Why the playback must stop now, if it must: its owner dropped it,
    /// the pane no longer shows its block, or Herdr hid the pane after
    /// showing it.
    fn halted(&mut self) -> Option<Halt> {
        if self.stop.cancelled() {
            return Some(Halt::Dropped);
        }
        while let Ok(command) = self.controls.try_recv() {
            match command {
                Control::Pause(paused) => {
                    self.paused = paused;
                    (self.events)(if self.buffering {
                        VideoEvent::Buffering {
                            milliseconds: self.position,
                            paused,
                        }
                    } else {
                        VideoEvent::Paused(paused)
                    });
                }
                Control::Seek(milliseconds) => return Some(Halt::Seek(milliseconds)),
            }
        }
        if let Some(file) = &self.file {
            match file.progress() {
                Ok(progress) => {
                    let downloaded = (progress.received, progress.total);
                    if !progress.complete
                        && self.downloaded != Some(downloaded)
                        && progress.received > 0
                    {
                        (self.events)(VideoEvent::Downloading(progress.received, progress.total));
                        self.downloaded = Some(downloaded);
                    }
                }
                Err(reason) => return Some(Halt::Failed(reason)),
            }
        }
        while let Ok(spot) = self.spots.try_recv() {
            match spot {
                Some(spot) => self.spot = Some(spot),
                None => return Some(Halt::Hidden),
            }
        }
        if self.visible.load(Ordering::Acquire) {
            self.seen = true;
        } else if self.seen {
            return Some(Halt::Hidden);
        }
        None
    }

    /// Wait only for front-loaded MP4 metadata, rather than for the whole
    /// file. A non-streamable MP4 safely falls back to the complete file.
    fn prepare(&mut self, file: &CachedVideo) -> Result<(), Halt> {
        loop {
            if let Some(halt) = self.halted() {
                // A seek before the metadata arrived can be applied once ready.
                if !matches!(halt, Halt::Seek(_)) {
                    return Err(halt);
                }
            }
            let progress = file.progress().map_err(Halt::Failed)?;
            if let Some(duration) = stream_metadata(&file.path) {
                self.duration = duration;
                return Ok(());
            }
            if let Some(header) = file.header() {
                self.duration = header.duration;
                return Ok(());
            }
            if progress.complete {
                self.duration = file_metadata(&file.path, true).flatten();
                return Ok(());
            }
            thread::sleep(TICK);
        }
    }

    fn active(&mut self) -> Result<(), Halt> {
        let start = Instant::now();
        loop {
            if let Some(halt) = self.halted() {
                return Err(halt);
            }
            if !self.paused {
                self.paused_for += start.elapsed();
                return Ok(());
            }
            thread::sleep(TICK);
        }
    }

    /// A seek replaces only the decoder; the shared transfer keeps running.
    fn run(
        &mut self,
        ffmpeg: &Path,
        file: &CachedVideo,
        size: (u32, u32),
        looped: bool,
        start: u64,
    ) -> Result<(), Halt> {
        let mut offset = start;
        loop {
            match self.pass(ffmpeg, file, size, offset) {
                Err(Halt::Seek(to)) => {
                    offset = to.min(self.duration.map_or(600_000, |ms| ms.saturating_sub(100)));
                }
                Err(halt) => return Err(halt),
                Ok(shown) if looped && shown > 0 => offset = 0,
                Ok(_) => return Ok(()),
            }
        }
    }

    /// One pass of FFmpeg over the file, at a low priority: the frames
    /// shown. FFmpeg is killed if it does not end by itself, and waited for
    /// in every case.
    fn pass(
        &mut self,
        ffmpeg: &Path,
        file: &CachedVideo,
        (width, height): (u32, u32),
        offset: u64,
    ) -> Result<usize, Halt> {
        self.position = offset;
        if self.playing || offset > 0 {
            self.buffering();
        }
        let mut command = Command::new(ffmpeg);
        let streaming = !file.progress().map_err(Halt::Failed)?.complete;
        let mut args = ffmpeg_args(&file.path, width, height);
        if streaming {
            let whitelist = args
                .iter()
                .position(|arg| arg == "-protocol_whitelist")
                .unwrap()
                + 1;
            args[whitelist] = "pipe".into();
            let input = args.iter().position(|arg| arg == "-i").unwrap() + 1;
            args[input] = "pipe:0".into();
            args.splice(
                input - 1..input - 1,
                ["-probesize", "32768", "-analyzeduration", "100000"].map(String::from),
            );
        }
        if offset > 0 {
            // A pipe seeks by decoding from the cached beginning; a complete
            // file can seek directly. Both avoid a new HTTP transfer.
            let at =
                args.iter().position(|arg| arg == "-i").unwrap() + if streaming { 2 } else { 0 };
            args.splice(
                at..at,
                ["-ss".into(), format!("{:.3}", offset as f64 / 1000.0)],
            );
        }
        command
            .args(args)
            .stdin(if streaming {
                Stdio::piped()
            } else {
                Stdio::null()
            })
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
        let feeding_stop = Arc::new(AtomicBool::new(false));
        let waiting_for_data = Arc::new(AtomicBool::new(false));
        let feeding = child.stdin.take().map(|stdin| {
            let file = file.clone();
            let stopped = feeding_stop.clone();
            let waiting = waiting_for_data.clone();
            thread::spawn(move || feed(file, stdin, stopped, waiting))
        });
        let errors = child
            .stderr
            .take()
            .map(|stderr| thread::spawn(move || first_errors(stderr)));
        let shown = match child.stdout.take() {
            Some(stdout) => self.frames(stdout, width, height, offset, &waiting_for_data),
            None => Err(Halt::Failed("FFmpeg has no output".into())),
        };
        if shown.is_err() {
            let _ = child.kill();
        }
        let status = child.wait();
        feeding_stop.store(true, Ordering::Release);
        if let Some(feeding) = feeding {
            let _ = feeding.join();
        }
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
    fn frames(
        &mut self,
        mut stdout: ChildStdout,
        width: u32,
        height: u32,
        offset: u64,
        waiting_for_data: &AtomicBool,
    ) -> Result<usize, Halt> {
        let mut frame = vec![0; width as usize * height as usize * 3];
        let mut start = None::<Instant>;
        let mut shown = 0;
        let mut index = 0;
        self.paused_for = Duration::ZERO;
        loop {
            if index > 0 {
                self.active()?;
            }
            if !self.read(&mut stdout, &mut frame, start.is_none(), waiting_for_data)? {
                return Ok(shown);
            }
            match start {
                Some(start) if late(index, start.elapsed().saturating_sub(self.paused_for)) => {
                    index += 1;
                    continue;
                }
                Some(start) => self.wait_until(start + FRAME * index)?,
                None => {}
            }
            self.show(&frame, width, height)?;
            start.get_or_insert_with(Instant::now);
            shown += 1;
            self.position = offset + u64::from(index) * 100;
            (self.events)(VideoEvent::Position {
                milliseconds: self.position,
                duration: self.duration,
            });
            index += 1;
            if !self.playing || self.buffering {
                self.playing = true;
                self.buffering = false;
                (self.events)(if self.paused {
                    VideoEvent::Paused(true)
                } else {
                    VideoEvent::Playing
                });
            }
        }
    }

    fn buffering(&mut self) {
        self.buffering = true;
        (self.events)(VideoEvent::Buffering {
            milliseconds: self.position,
            paused: self.paused,
        });
    }

    /// The decoder's first-frame budget excludes waiting for network data.
    /// Later stalls also suspend presentation so arriving frames stay visible.
    fn read(
        &mut self,
        stdout: &mut ChildStdout,
        frame: &mut [u8],
        first: bool,
        waiting_for_data: &AtomicBool,
    ) -> Result<bool, Halt> {
        let mut filled = 0;
        let mut budget = self.first_frame;
        let started = Instant::now();
        let mut checked = started;
        while filled < frame.len() {
            if let Some(halt) = self.halted() {
                return Err(halt);
            }
            let now = Instant::now();
            let elapsed = now.duration_since(checked);
            checked = now;
            let waiting = waiting_for_data.load(Ordering::Acquire);
            if waiting {
                if !first {
                    self.paused_for += elapsed;
                }
                if !self.buffering && started.elapsed() >= Duration::from_millis(250) {
                    self.buffering();
                }
            } else {
                budget = budget.saturating_sub(elapsed);
                if !first && self.paused {
                    self.paused_for += elapsed;
                }
            }
            if first && budget.is_zero() {
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
            self.active()?;
            let left = (due + self.paused_for).saturating_duration_since(Instant::now());
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

/// Feeds the growing cached file through a bounded OS pipe. Killing the
/// decoder unblocks a writer even while the user has paused playback.
fn feed(
    file: CachedVideo,
    mut stdin: std::process::ChildStdin,
    stopped: Arc<AtomicBool>,
    waiting: Arc<AtomicBool>,
) {
    use std::io::{Seek, SeekFrom};
    let Ok(mut input) = File::open(&file.path) else {
        return;
    };
    let mut left = None;
    if let Some(header) = file.header() {
        if stdin.write_all(&header.prefix).is_err()
            || input.seek(SeekFrom::Start(header.offset)).is_err()
        {
            return;
        }
        left = Some(header.end.saturating_sub(header.offset));
    }
    let mut buffer = [0; 64 * 1024];
    while !stopped.load(Ordering::Acquire) {
        let room = left.map_or(buffer.len(), |left| left.min(buffer.len() as u64) as usize);
        if room == 0 {
            return;
        }
        match input.read(&mut buffer[..room]) {
            Ok(0) => match file.progress() {
                Ok(progress) if !progress.complete => {
                    waiting.store(true, Ordering::Release);
                    thread::sleep(TICK);
                }
                _ => {
                    waiting.store(false, Ordering::Release);
                    return;
                }
            },
            Ok(read) => {
                waiting.store(false, Ordering::Release);
                if let Some(left) = &mut left {
                    *left -= read as u64;
                }
                if stdin.write_all(&buffer[..read]).is_err() {
                    return;
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}
