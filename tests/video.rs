//! README videos play with a private FFmpeg: its frames, at their pace, on
//! a layer of the pane. Marketplace retains videos across details replacement.
//! Offline: FFmpeg, the download and the layer are simulated.

use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Barrier, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use herdr_marketplace::adapters::download_cancel::Cancellation;
use herdr_marketplace::adapters::env::{ffmpeg_bin, ffmpeg_from};
use herdr_marketplace::adapters::pane_graphics::Cells;
use herdr_marketplace::adapters::tui::animation::Spot;
use herdr_marketplace::adapters::tui::video::{
    Cache, Download, FIRST_FRAME, Frames, Playback, Video, VideoEvent, clean_stale, ffmpeg_args,
    frame_size, late,
};
use herdr_marketplace::adapters::tui::video_cache::{
    CacheServer, StreamHeader, media_range, relocate_header,
};
use image::GenericImageView;
use tempfile::TempDir;

#[test]
fn ffmpeg_is_the_variable_s_file_else_the_one_next_to_the_executable() {
    let folder = tempfile::tempdir().unwrap();
    let exe = folder.path().join("herdr-marketplace");
    let chosen = folder.path().join("chosen-ffmpeg");
    let missing = folder.path().join("missing-ffmpeg");
    fs::write(&chosen, "").unwrap();
    let chosen_text = chosen.to_str().unwrap();
    let missing_text = missing.to_str().unwrap();

    assert_eq!(
        ffmpeg_from(Some(chosen_text), Some(&exe)),
        Some(chosen.clone())
    );
    assert_eq!(ffmpeg_from(Some(""), Some(&exe)), None);
    assert_eq!(ffmpeg_from(Some(missing_text), Some(&exe)), None);
    assert_eq!(ffmpeg_from(None, Some(&exe)), None);
    assert_eq!(ffmpeg_from(None, None), None);

    let next_to_exe = folder.path().join("ffmpeg");
    fs::write(&next_to_exe, "").unwrap();
    assert_eq!(ffmpeg_from(Some(chosen_text), Some(&exe)), Some(chosen));
    assert_eq!(ffmpeg_from(Some(""), Some(&exe)), Some(next_to_exe.clone()));
    assert_eq!(
        ffmpeg_from(Some(missing_text), Some(&exe)),
        Some(next_to_exe.clone())
    );
    assert_eq!(ffmpeg_from(None, Some(&exe)), Some(next_to_exe));
    assert_eq!(ffmpeg_from(None, None), None);
}

#[test]
fn ffmpeg_reads_the_file_only_and_writes_raw_frames_that_fit() {
    assert_eq!(
        ffmpeg_args(Path::new("/state/videos/12-1.mp4"), 800, 450),
        [
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
            "/state/videos/12-1.mp4",
            "-an",
            "-vf",
            "fps=10,scale=800:450:force_original_aspect_ratio=decrease,pad=800:450:(ow-iw)/2:(oh-ih)/2",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "pipe:1",
        ]
    );
}

#[test]
fn frames_fit_their_block_and_800_pixels() {
    assert_eq!(frame_size((1600, 900)), (800, 450));
    assert_eq!(frame_size((400, 300)), (400, 300));
    assert_eq!(frame_size((1, 1)), (2, 2));
}

#[test]
fn a_frame_more_than_100_ms_late_is_skipped() {
    assert!(!late(5, Duration::from_millis(580)));
    assert!(late(5, Duration::from_millis(620)));
}

/// Frames of 160 × 90 pixels, as `frame_size` makes them for this block.
const FIT: (u32, u32) = (160, 90);

/// Simulated FFmpegs, written once for all the tests: a program still open
/// for writing cannot run. Each finds its test's folder from its input,
/// `<test>/videos/<file>`, to note its runs and its pid there.
fn fake(name: &str) -> PathBuf {
    static FAKES: OnceLock<PathBuf> = OnceLock::new();
    const FIND_TEST: &str =
        "while [ \"$1\" != -i ]; do shift; done\ntest=${2%/*}\ntest=${test%/*}\n";
    let folder = FAKES.get_or_init(|| {
        let folder = Path::new(env!("CARGO_TARGET_TMPDIR")).join("video-fakes");
        fs::create_dir_all(&folder).unwrap();
        for (name, body) in [
            // Ten frames, then their end.
            ("frames", "echo run >> \"$test/runs\"\nhead -c 432000 /dev/zero\n"),
            // Two frames, then nothing more.
            ("hangs", "echo $$ > \"$test/pid\"\nhead -c 86400 /dev/zero\nexec sleep 30\n"),
            ("silent", "echo $$ > \"$test/pid\"\nexec sleep 30\n"),
            (
                "fails",
                "echo 'Invalid data found when processing input' >&2\necho 'second line' >&2\nexit 1\n",
            ),
        ] {
            let path = folder.join(name);
            fs::write(&path, format!("#!/bin/sh\n{FIND_TEST}{body}")).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        folder
    });
    folder.join(name)
}

/// Copies `bytes` into the video file, or fails.
struct Copy(Result<Vec<u8>, String>);

impl Download for Copy {
    fn download(
        &self,
        _url: &str,
        _limit: u64,
        file: &mut File,
        cancellation: &Cancellation,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<(), String> {
        if cancellation.cancelled() {
            return Err("cancelled".into());
        }
        let bytes = self.0.clone()?;
        file.write_all(&bytes).map_err(|error| error.to_string())?;
        progress(bytes.len() as u64, Some(bytes.len() as u64));
        Ok(())
    }
}

/// A layer that keeps the size and cells of what it shows.
struct FakeLayer {
    shown: Arc<Mutex<Vec<(u32, u32, Cells)>>>,
    closed: Arc<AtomicBool>,
}

impl Frames for FakeLayer {
    fn show(&mut self, png: &[u8], width: u32, height: u32, cells: Cells) -> Result<(), String> {
        let decoded = image::load_from_memory(png).map_err(|error| error.to_string())?;
        assert_eq!(decoded.dimensions(), (width, height));
        self.shown.lock().unwrap().push((width, height, cells));
        Ok(())
    }
}

impl Drop for FakeLayer {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

/// A playback of the tests and what it left.
struct Played {
    test: TempDir,
    cache: Arc<Cache>,
    events: Receiver<VideoEvent>,
    shown: Arc<Mutex<Vec<(u32, u32, Cells)>>>,
    closed: Arc<AtomicBool>,
}

impl Played {
    fn close_cache(&mut self) {
        self.cache = Arc::new(Cache::new(self.folder()));
    }
    fn folder(&self) -> PathBuf {
        self.test.path().join("videos")
    }

    /// The events up to the first `wanted` one.
    fn until(&self, wanted: impl Fn(&VideoEvent) -> bool) -> Vec<VideoEvent> {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut seen = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let event = self
                .events
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("no such event, only {seen:?}"));
            let done = wanted(&event);
            seen.push(event);
            if done {
                return seen;
            }
        }
    }

    fn files(&self) -> Vec<PathBuf> {
        fs::read_dir(self.folder())
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.extension().is_some_and(|ext| ext == "mp4"))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn pid(&self) -> i32 {
        fs::read_to_string(self.test.path().join("pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }

    fn runs(&self) -> usize {
        fs::read_to_string(self.test.path().join("runs"))
            .map(|runs| runs.lines().count())
            .unwrap_or(0)
    }
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// A block of 4 rows, `shown` of them from the top on screen.
fn spot(shown: u16) -> Spot {
    Spot {
        cells: Cells {
            column: 2,
            row: 5,
            columns: 20,
            rows: shown,
        },
        first: 0,
        rows: 4,
    }
}

fn play(
    ffmpeg: PathBuf,
    download: Copy,
    looped: bool,
    first_frame: Duration,
) -> (Playback, Played) {
    let test = tempfile::tempdir().unwrap();
    let cache = Arc::new(Cache::new(test.path().join("videos")));
    let (sender, events) = mpsc::channel();
    let shown = Arc::new(Mutex::new(Vec::new()));
    let closed = Arc::new(AtomicBool::new(false));
    let layer = FakeLayer {
        shown: shown.clone(),
        closed: closed.clone(),
    };
    let playback = Playback::start(Video {
        start: 0,
        url: "https://github.com/user-attachments/assets/demo".into(),
        looped,
        fit: FIT,
        cache: cache.clone(),
        ffmpeg,
        download: Arc::new(download),
        open: Box::new(move || Ok(Box::new(layer) as Box<dyn Frames>)),
        visible: Arc::new(AtomicBool::new(true)),
        first_frame,
        events: Arc::new(move |event| {
            let _ = sender.send(event);
        }),
    });
    let played = Played {
        test,
        cache,
        events,
        shown,
        closed,
    };
    (playback, played)
}

fn some_bytes() -> Copy {
    Copy(Ok(b"not really a video".to_vec()))
}

struct Counted(Arc<AtomicUsize>);

impl Download for Counted {
    fn download(
        &self,
        _: &str,
        _: u64,
        file: &mut File,
        cancellation: &Cancellation,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<(), String> {
        assert!(!cancellation.cancelled());
        self.0.fetch_add(1, Ordering::SeqCst);
        file.write_all(b"video").unwrap();
        progress(5, Some(5));
        Ok(())
    }
}

fn cached_play(
    cache: Arc<Cache>,
    url: &str,
    download: Arc<dyn Download>,
) -> (Playback, Receiver<VideoEvent>) {
    let (sender, events) = mpsc::channel();
    let mut playback = Playback::start(Video {
        start: 0,
        url: url.into(),
        looped: false,
        fit: FIT,
        cache,
        ffmpeg: fake("frames"),
        download,
        open: Box::new(|| {
            Ok(Box::new(FakeLayer {
                shown: Arc::default(),
                closed: Arc::default(),
            }))
        }),
        visible: Arc::new(AtomicBool::new(true)),
        first_frame: FIRST_FRAME,
        events: Arc::new(move |event| {
            let _ = sender.send(event);
        }),
    });
    playback.show(Some(spot(4)));
    (playback, events)
}

#[test]
fn replay_uses_one_download_and_closing_marketplace_deletes_the_cache() {
    let test = tempfile::tempdir().unwrap();
    let folder = test.path().join("videos");
    let cache = Arc::new(Cache::new(folder.clone()));
    let downloads = Arc::new(AtomicUsize::new(0));
    for _ in 0..2 {
        let (playback, events) =
            cached_play(cache.clone(), "same", Arc::new(Counted(downloads.clone())));
        loop {
            match events.recv_timeout(Duration::from_secs(20)).unwrap() {
                VideoEvent::Ended => break,
                VideoEvent::Failed(reason) => panic!("{reason}"),
                _ => {}
            }
        }
        drop(playback);
        assert_eq!(
            fs::read_dir(&folder)
                .unwrap()
                .flatten()
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "mp4"))
                .count(),
            1
        );
    }
    assert_eq!(downloads.load(Ordering::SeqCst), 1);
    drop(cache);
    assert_eq!(fs::read_dir(&folder).unwrap().count(), 0);
}

#[test]
fn changing_plugins_and_returning_reuses_both_cached_videos() {
    let test = tempfile::tempdir().unwrap();
    let folder = test.path().join("videos");
    let cache = Arc::new(Cache::new(folder.clone()));
    let downloads = Arc::new(AtomicUsize::new(0));
    for url in ["first", "second", "first"] {
        let (playback, events) =
            cached_play(cache.clone(), url, Arc::new(Counted(downloads.clone())));
        loop {
            match events.recv_timeout(Duration::from_secs(20)).unwrap() {
                VideoEvent::Ended => break,
                VideoEvent::Failed(reason) => panic!("{reason}"),
                _ => {}
            }
        }
        drop(playback);
    }
    assert_eq!(downloads.load(Ordering::SeqCst), 2);
    assert_eq!(
        fs::read_dir(&folder)
            .unwrap()
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "mp4"))
            .count(),
        2
    );
    drop(cache);
    assert_eq!(fs::read_dir(&folder).unwrap().count(), 0);
}

struct UntilCancelled(Arc<AtomicBool>);

impl Download for UntilCancelled {
    fn download(
        &self,
        _: &str,
        _: u64,
        file: &mut File,
        cancellation: &Cancellation,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<(), String> {
        file.write_all(b"partial").unwrap();
        progress(7, None);
        while !cancellation.cancelled() {
            std::thread::sleep(Duration::from_millis(5));
        }
        self.0.store(true, Ordering::SeqCst);
        Err("cancelled".into())
    }
}

#[test]
fn stopping_or_hiding_keeps_the_transfer_until_marketplace_closes() {
    for hidden in [false, true] {
        let test = tempfile::tempdir().unwrap();
        let folder = test.path().join("videos");
        let cache = Arc::new(Cache::new(folder.clone()));
        let finished = Arc::new(AtomicBool::new(false));
        let (mut playback, events) = cached_play(
            cache.clone(),
            "partial",
            Arc::new(UntilCancelled(finished.clone())),
        );
        assert_eq!(
            events.recv_timeout(Duration::from_secs(5)).unwrap(),
            VideoEvent::Downloading(7, None)
        );
        let started = Instant::now();
        if hidden {
            playback.show(None);
            assert_eq!(
                events.recv_timeout(Duration::from_secs(5)).unwrap(),
                VideoEvent::Hidden
            );
        }
        drop(playback);
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(
            !finished.load(Ordering::SeqCst),
            "details stopped a shared transfer"
        );
        drop(cache);
        assert!(
            finished.load(Ordering::SeqCst),
            "closing Marketplace left a transfer"
        );
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 0);
    }
}

#[test]
fn a_video_keeps_its_download_for_replay_until_marketplace_closes() {
    let (mut playback, mut played) = play(fake("frames"), some_bytes(), false, FIRST_FRAME);
    playback.show(Some(spot(4)));
    let events = played.until(|event| *event == VideoEvent::Ended);
    assert!(events.contains(&VideoEvent::Playing), "{events:?}");
    let shown = played.shown.lock().unwrap().clone();
    assert!(!shown.is_empty() && shown.len() <= 10, "{shown:?}");
    assert!(
        shown
            .iter()
            .all(|(width, height, cells)| (*width, *height) == FIT && *cells == spot(4).cells)
    );
    assert_eq!(played.runs(), 1);
    assert_eq!(played.files().len(), 1);
    assert!(played.closed.load(Ordering::SeqCst));
    drop(playback);
    played.close_cache();
    assert!(played.files().is_empty());
}

#[test]
fn a_looped_video_starts_over_at_its_end() {
    let (mut playback, played) = play(fake("frames"), some_bytes(), true, FIRST_FRAME);
    playback.show(Some(spot(4)));
    let deadline = Instant::now() + Duration::from_secs(20);
    while played.runs() < 2 {
        assert!(Instant::now() < deadline, "no second pass");
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(playback);
    assert_eq!(played.files().len(), 1);
    assert!(played.closed.load(Ordering::SeqCst));
    assert!(
        !played
            .events
            .try_iter()
            .any(|event| event == VideoEvent::Ended),
        "a looped video does not end"
    );
}

#[test]
fn half_of_a_block_on_screen_gets_half_of_each_frame() {
    let (mut playback, played) = play(fake("frames"), some_bytes(), false, FIRST_FRAME);
    playback.show(Some(spot(2)));
    played.until(|event| *event == VideoEvent::Ended);
    let shown = played.shown.lock().unwrap().clone();
    assert!(!shown.is_empty());
    assert!(
        shown
            .iter()
            .all(|(width, height, cells)| (*width, *height) == (160, 45) && cells.rows == 2),
        "{shown:?}"
    );
}

#[test]
fn a_stop_kills_ffmpeg_closes_the_layer_and_preserves_the_cache() {
    // The block leaves the view: the video ends.
    let (mut playback, played) = play(fake("hangs"), some_bytes(), false, FIRST_FRAME);
    playback.show(Some(spot(4)));
    played.until(|event| *event == VideoEvent::Playing);
    let pid = played.pid();
    assert!(alive(pid));
    assert_eq!(played.files().len(), 1);
    playback.show(None);
    played.until(|event| *event == VideoEvent::Hidden);
    assert!(!alive(pid), "FFmpeg still runs");
    assert_eq!(played.files().len(), 1);
    assert!(played.closed.load(Ordering::SeqCst));
    drop(playback);

    // Its owner drops it: nothing to tell, the same cleanup when it returns.
    let (mut playback, played) = play(fake("hangs"), some_bytes(), false, FIRST_FRAME);
    playback.show(Some(spot(4)));
    played.until(|event| *event == VideoEvent::Playing);
    let pid = played.pid();
    drop(playback);
    assert!(!alive(pid), "FFmpeg still runs");
    assert_eq!(played.files().len(), 1);
    assert!(played.closed.load(Ordering::SeqCst));
    assert!(
        played
            .events
            .try_iter()
            .all(|event| event != VideoEvent::Ended)
    );
}

#[test]
fn herdr_hiding_the_pane_ends_the_video() {
    let test = tempfile::tempdir().unwrap();
    let cache = Arc::new(Cache::new(test.path().join("videos")));
    let (sender, events) = mpsc::channel();
    let visible = Arc::new(AtomicBool::new(true));
    let mut playback = Playback::start(Video {
        start: 0,
        url: "https://github.com/user-attachments/assets/demo".into(),
        looped: false,
        fit: FIT,
        cache,
        ffmpeg: fake("hangs"),
        download: Arc::new(some_bytes()),
        open: Box::new(|| {
            Ok(Box::new(FakeLayer {
                shown: Arc::default(),
                closed: Arc::default(),
            }) as Box<dyn Frames>)
        }),
        visible: visible.clone(),
        first_frame: FIRST_FRAME,
        events: Arc::new(move |event| {
            let _ = sender.send(event);
        }),
    });
    playback.show(Some(spot(4)));
    let timeout = Duration::from_secs(20);
    loop {
        if events.recv_timeout(timeout).unwrap() == VideoEvent::Playing {
            break;
        }
    }
    visible.store(false, Ordering::SeqCst);
    loop {
        if events.recv_timeout(timeout).unwrap() == VideoEvent::Hidden {
            break;
        }
    }
    let pid: i32 = fs::read_to_string(test.path().join("pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(!alive(pid));
}

struct FailureWithPendingHeader {
    started: Arc<Barrier>,
    cancelled: mpsc::Sender<()>,
    release: Mutex<Receiver<()>>,
}

impl Download for FailureWithPendingHeader {
    fn download(
        &self,
        _: &str,
        _: u64,
        file: &mut File,
        _: &Cancellation,
        _: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<(), String> {
        self.started.wait();
        file.write_all(b"partial")
            .map_err(|error| error.to_string())?;
        Err("network down".into())
    }

    fn streaming_header(
        &self,
        _: &str,
        stop: &Cancellation,
    ) -> Result<Option<StreamHeader>, String> {
        self.started.wait();
        while !stop.cancelled() {
            std::thread::sleep(Duration::from_millis(1));
        }
        let _ = self.cancelled.send(());
        if self.release.lock().unwrap().recv().is_err() {
            return Ok(None);
        }
        // A metadata answer already in flight may finish during cancellation.
        Ok(Some(StreamHeader {
            prefix: vec![0; 8],
            offset: 0,
            end: 7,
            duration: None,
        }))
    }
}

#[test]
fn a_download_error_is_visible_only_after_partial_video_and_late_metadata_cleanup() {
    let folder = tempfile::tempdir().unwrap();
    let cache = Cache::new(folder.path().to_path_buf());
    // Dropping this sender on panic also releases the header before Cache joins it.
    let (release, receive) = mpsc::channel();
    let (cancelled, stopped) = mpsc::channel();
    let file = cache
        .get(
            "https://github.com/user-attachments/assets/failure",
            Arc::new(FailureWithPendingHeader {
                started: Arc::new(Barrier::new(2)),
                cancelled,
                release: Mutex::new(receive),
            }),
        )
        .unwrap();
    stopped.recv_timeout(Duration::from_secs(20)).unwrap();
    let premature_error = file.progress().is_err();
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while file.progress().is_ok() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        !premature_error,
        "error published while header cleanup was pending"
    );
    assert_eq!(file.progress().unwrap_err(), "network down");
    assert!(
        !file.path.exists(),
        "partial video remained after the error"
    );
    assert!(
        !file.path.with_extension("stream").exists(),
        "late metadata remained after the error"
    );
}

#[test]
fn decoder_failures_tell_why_and_still_allow_cached_retry() {
    let failed = |event: &VideoEvent| matches!(event, VideoEvent::Failed(_));

    let (mut playback, played) = play(fake("fails"), some_bytes(), false, FIRST_FRAME);
    playback.show(Some(spot(4)));
    let events = played.until(failed);
    assert_eq!(
        events.last(),
        Some(&VideoEvent::Failed(
            "Invalid data found when processing input".into()
        ))
    );
    assert_eq!(played.files().len(), 1);

    // Long enough for the fake to note its pid, even on a busy machine.
    let (mut playback, played) = play(fake("silent"), some_bytes(), false, Duration::from_secs(2));
    playback.show(Some(spot(4)));
    let events = played.until(failed);
    assert_eq!(
        events.last(),
        Some(&VideoEvent::Failed("no frame after 2 s".into()))
    );
    assert!(!alive(played.pid()), "FFmpeg still runs");
    assert_eq!(played.files().len(), 1);

    let (mut playback, played) = play(
        fake("frames"),
        Copy(Err("network down".into())),
        false,
        FIRST_FRAME,
    );
    playback.show(Some(spot(4)));
    let events = played.until(failed);
    assert_eq!(events, [VideoEvent::Failed("network down".into())]);
    assert_eq!(played.runs(), 0, "FFmpeg ran without a video");
    assert!(played.files().is_empty());
}

#[test]
fn files_left_by_gone_panes_are_deleted_never_a_new_one_of_a_living_one() {
    // A fake runs first: a program still open for writing cannot run.
    fake("frames");
    let folder = tempfile::tempdir().unwrap();
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let gone = child.id();
    child.wait().unwrap();
    let own = std::process::id();
    let dead = folder.path().join(format!("{gone}-1.mp4"));
    let old = folder.path().join(format!("{own}-2.mp4"));
    let new = folder.path().join(format!("{own}-3.mp4"));
    for path in [&dead, &old, &new] {
        fs::write(path, "video").unwrap();
    }
    File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(2 * 60 * 60))
        .unwrap();
    clean_stale(folder.path());
    assert!(!dead.exists(), "the file of a gone process stays");
    assert!(old.exists(), "the cache of a living pane is deleted");
    assert!(new.exists(), "the new file of a living process is deleted");
}

#[test]
fn replacing_details_clients_preserves_downloads_until_the_sidebar_server_closes() {
    let test = tempfile::tempdir().unwrap();
    let folder = test.path().join("videos");
    let count = Arc::new(AtomicUsize::new(0));
    let server = CacheServer::start(folder.clone(), Arc::new(Counted(count.clone()))).unwrap();
    let mut first = None;
    for url in ["A", "B", "A"] {
        let client = Cache::shared(folder.clone());
        let file = client.get(url, Arc::new(some_bytes())).unwrap();
        if url == "A" {
            if let Some(path) = &first {
                assert_eq!(&file.path, path);
            }
            first = Some(file.path);
        }
        drop(client);
    }
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert!(first.unwrap().exists());
    drop(server);
    assert!(!folder.exists());
}

#[test]
fn pause_freezes_the_frame_position_and_seek_restarts_only_the_decoder() {
    let (mut playback, played) = play(fake("frames"), some_bytes(), true, FIRST_FRAME);
    playback.show(Some(spot(4)));
    played.until(|event| *event == VideoEvent::Playing);
    playback.pause(true);
    played.until(|event| *event == VideoEvent::Paused(true));
    let shown = played.shown.lock().unwrap().len();
    std::thread::sleep(Duration::from_millis(350));
    assert_eq!(played.shown.lock().unwrap().len(), shown);
    playback.seek(500);
    played.until(|event| {
        matches!(
            event,
            VideoEvent::Position {
                milliseconds: 500,
                ..
            }
        )
    });
    assert!(played.runs() >= 2);
    assert_eq!(played.files().len(), 1);
    playback.pause(false);
    played.until(|event| *event == VideoEvent::Paused(false));
    played.until(|event| {
        matches!(
            event,
            VideoEvent::Position {
                milliseconds: 600,
                ..
            }
        )
    });
}

struct GatedVideo {
    bytes: Vec<u8>,
    before_finish: Option<usize>,
    finish: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
}

impl Download for GatedVideo {
    fn streaming_header(&self, _: &str, _: &Cancellation) -> Result<Option<StreamHeader>, String> {
        let (offset, end) = media_range(&self.bytes).unwrap();
        Ok(relocate_header(
            &self.bytes,
            &self.bytes[end as usize..],
            offset,
            end,
        ))
    }

    fn download(
        &self,
        _: &str,
        _: u64,
        file: &mut File,
        stop: &Cancellation,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<(), String> {
        let (_, end) = media_range(&self.bytes).unwrap();
        let prefix = self.before_finish.unwrap_or(end as usize);
        file.write_all(&self.bytes[..prefix]).unwrap();
        progress(prefix as u64, Some(self.bytes.len() as u64));
        while !self.finish.load(Ordering::Acquire) {
            if stop.cancelled() {
                return Err("cancelled".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        file.write_all(&self.bytes[prefix..]).unwrap();
        progress(self.bytes.len() as u64, Some(self.bytes.len() as u64));
        self.finished.store(true, Ordering::Release);
        Ok(())
    }
}

#[test]
#[ignore = "needs the private FFmpeg: HERDR_MARKETPLACE_FFMPEG=target/ffmpeg/ffmpeg"]
fn a_tail_metadata_mp4_plays_before_completion_and_resumes_after_a_network_stall() {
    let test = tempfile::tempdir().unwrap();
    let cache = Arc::new(Cache::new(test.path().join("videos")));
    let finish = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let (sender, events) = mpsc::channel();
    let shown = Arc::new(Mutex::new(Vec::new()));
    let layer = FakeLayer {
        shown: shown.clone(),
        closed: Arc::default(),
    };
    let mut playback = Playback::start(Video {
        start: 0,
        url: "tail-loaded-mp4".into(),
        looped: false,
        fit: FIT,
        cache: cache.clone(),
        ffmpeg: ffmpeg_bin().expect("private FFmpeg"),
        download: Arc::new(GatedVideo {
            before_finish: Some(8069),
            bytes: fs::read(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/tiny.mp4"
            ))
            .unwrap(),
            finish: finish.clone(),
            finished: finished.clone(),
        }),
        open: Box::new(move || Ok(Box::new(layer))),
        visible: Arc::new(AtomicBool::new(true)),
        first_frame: FIRST_FRAME,
        events: Arc::new(move |event| {
            let _ = sender.send(event);
        }),
    });
    playback.show(Some(spot(4)));
    loop {
        match events.recv_timeout(Duration::from_secs(15)).unwrap() {
            VideoEvent::Playing => break,
            VideoEvent::Failed(error) => panic!("{error}"),
            _ => {}
        }
    }
    assert!(!finished.load(Ordering::Acquire));
    assert!(!shown.lock().unwrap().is_empty());
    loop {
        match events.recv_timeout(Duration::from_secs(5)).unwrap() {
            VideoEvent::Buffering { .. } => break,
            VideoEvent::Failed(error) => panic!("{error}"),
            _ => {}
        }
    }
    // A network pause must not make the remaining frames seem too late.
    std::thread::sleep(Duration::from_millis(700));
    finish.store(true, Ordering::Release);
    loop {
        match events.recv_timeout(Duration::from_secs(15)).unwrap() {
            VideoEvent::Ended => break,
            VideoEvent::Failed(error) => panic!("{error}"),
            _ => {}
        }
    }
    assert_eq!(shown.lock().unwrap().len(), 10);
    drop(playback);
    drop(cache);
    assert_eq!(fs::read_dir(test.path().join("videos")).unwrap().count(), 0);
}

#[test]
#[ignore = "needs the private FFmpeg: HERDR_MARKETPLACE_FFMPEG=target/ffmpeg/ffmpeg"]
fn a_seek_waits_for_missing_stream_data_instead_of_failing_the_decoder() {
    let test = tempfile::tempdir().unwrap();
    let cache = Arc::new(Cache::new(test.path().join("videos")));
    let finish = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let (sender, events) = mpsc::channel();
    let layer = FakeLayer {
        shown: Arc::default(),
        closed: Arc::default(),
    };
    let mut playback = Playback::start(Video {
        start: 900,
        url: "partial-seek-mp4".into(),
        looped: false,
        fit: FIT,
        cache: cache.clone(),
        ffmpeg: ffmpeg_bin().expect("private FFmpeg"),
        download: Arc::new(GatedVideo {
            bytes: fs::read(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/tiny.mp4"
            ))
            .unwrap(),
            // The last frames have not arrived, but metadata is available.
            before_finish: Some(8069),
            finish: finish.clone(),
            finished: finished.clone(),
        }),
        open: Box::new(move || Ok(Box::new(layer))),
        visible: Arc::new(AtomicBool::new(true)),
        first_frame: Duration::from_millis(200),
        events: Arc::new(move |event| {
            let _ = sender.send(event);
        }),
    });
    playback.show(Some(spot(4)));
    let until = Instant::now() + Duration::from_millis(700);
    while Instant::now() < until {
        if let Ok(event) = events.recv_timeout(Duration::from_millis(20)) {
            assert!(!matches!(event, VideoEvent::Failed(_)), "{event:?}");
        }
    }
    assert!(!finished.load(Ordering::Acquire));
    // Returning to cached data remains possible while a forward seek waits.
    playback.seek(0);
    loop {
        match events.recv_timeout(Duration::from_secs(5)).unwrap() {
            VideoEvent::Position {
                milliseconds: 0, ..
            } => break,
            VideoEvent::Failed(error) => panic!("{error}"),
            _ => {}
        }
    }
    playback.pause(true);
    loop {
        match events.recv_timeout(Duration::from_secs(5)).unwrap() {
            VideoEvent::Paused(true) => break,
            VideoEvent::Failed(error) => panic!("{error}"),
            _ => {}
        }
    }
    playback.seek(900);
    loop {
        match events.recv_timeout(Duration::from_secs(5)).unwrap() {
            VideoEvent::Buffering {
                milliseconds: 900,
                paused: true,
            } => break,
            VideoEvent::Failed(error) => panic!("{error}"),
            _ => {}
        }
    }
    let until = Instant::now() + Duration::from_millis(700);
    while Instant::now() < until {
        if let Ok(event) = events.recv_timeout(Duration::from_millis(20)) {
            assert!(!matches!(event, VideoEvent::Failed(_)), "{event:?}");
        }
    }
    finish.store(true, Ordering::Release);
    loop {
        match events.recv_timeout(Duration::from_secs(5)).unwrap() {
            VideoEvent::Position {
                milliseconds: 900, ..
            } => break,
            VideoEvent::Failed(error) => panic!("{error}"),
            _ => {}
        }
    }
    assert_eq!(
        events.recv_timeout(Duration::from_secs(5)).unwrap(),
        VideoEvent::Paused(true)
    );
    drop(playback);
    drop(cache);
    assert_eq!(fs::read_dir(test.path().join("videos")).unwrap().count(), 0);
}

#[test]
#[ignore = "needs the private FFmpeg: HERDR_MARKETPLACE_FFMPEG=target/ffmpeg/ffmpeg"]
fn the_real_ffmpeg_plays_the_ten_frames_of_tiny_mp4() {
    let ffmpeg = ffmpeg_bin().expect("HERDR_MARKETPLACE_FFMPEG names the private FFmpeg");
    let tiny = fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/tiny.mp4"
    ))
    .unwrap();
    let (mut playback, mut played) = play(ffmpeg, Copy(Ok(tiny)), false, FIRST_FRAME);
    playback.show(Some(spot(4)));
    played.until(|event| *event == VideoEvent::Ended);
    let shown = played.shown.lock().unwrap().clone();
    assert_eq!(shown.len(), 10, "{shown:?}");
    assert!(
        shown
            .iter()
            .all(|(width, height, _)| (*width, *height) == (160, 90))
    );
    assert_eq!(played.files().len(), 1);
    drop(playback);
    played.close_cache();
    assert!(played.files().is_empty());
}

#[test]
#[ignore = "requires the public GitHub video endpoint"]
fn a_github_tail_metadata_video_supports_streaming_ranges() {
    use herdr_marketplace::adapters::image_fetch::ImageFetcher;
    let start = Instant::now();
    let header = ImageFetcher::default()
        .streaming_header(
            "https://github.com/user-attachments/assets/abe2f43e-fc50-4866-b753-33388967945d",
            &Cancellation::default(),
        )
        .unwrap()
        .expect("a relocatable moov");
    println!(
        "metadata_ms={} prefix_bytes={} duration_ms={:?}",
        start.elapsed().as_millis(),
        header.prefix.len(),
        header.duration
    );
}
