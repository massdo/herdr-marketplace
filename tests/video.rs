//! README videos play with a private FFmpeg: its frames, at their pace, on
//! a layer of the pane, and every stop kills FFmpeg and deletes the file.
//! Offline: FFmpeg, the download and the layer are simulated.

use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use herdr_marketplace::adapters::env::{ffmpeg_bin, ffmpeg_from};
use herdr_marketplace::adapters::pane_graphics::Cells;
use herdr_marketplace::adapters::tui::animation::Spot;
use herdr_marketplace::adapters::tui::video::{
    Download, FIRST_FRAME, Frames, Playback, Video, VideoEvent, clean_stale, ffmpeg_args,
    frame_size, late,
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
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<(), String> {
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
    events: Receiver<VideoEvent>,
    shown: Arc<Mutex<Vec<(u32, u32, Cells)>>>,
    closed: Arc<AtomicBool>,
}

impl Played {
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
            .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
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
    let (sender, events) = mpsc::channel();
    let shown = Arc::new(Mutex::new(Vec::new()));
    let closed = Arc::new(AtomicBool::new(false));
    let layer = FakeLayer {
        shown: shown.clone(),
        closed: closed.clone(),
    };
    let playback = Playback::start(Video {
        url: "https://github.com/user-attachments/assets/demo".into(),
        looped,
        fit: FIT,
        folder: test.path().join("videos"),
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
        events,
        shown,
        closed,
    };
    (playback, played)
}

fn some_bytes() -> Copy {
    Copy(Ok(b"not really a video".to_vec()))
}

#[test]
fn a_video_downloads_then_plays_to_its_end_and_leaves_nothing() {
    let (mut playback, played) = play(fake("frames"), some_bytes(), false, FIRST_FRAME);
    playback.show(Some(spot(4)));
    let events = played.until(|event| *event == VideoEvent::Ended);
    assert_eq!(events[0], VideoEvent::Downloading(18, Some(18)));
    assert!(events.contains(&VideoEvent::Playing), "{events:?}");
    let shown = played.shown.lock().unwrap().clone();
    assert!(!shown.is_empty() && shown.len() <= 10, "{shown:?}");
    assert!(
        shown
            .iter()
            .all(|(width, height, cells)| (*width, *height) == FIT && *cells == spot(4).cells)
    );
    assert_eq!(played.runs(), 1);
    assert!(played.files().is_empty(), "{:?}", played.files());
    assert!(played.closed.load(Ordering::SeqCst));
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
    assert!(played.files().is_empty());
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
fn a_stop_kills_ffmpeg_deletes_the_file_and_closes_the_layer() {
    // The block leaves the view: the video ends.
    let (mut playback, played) = play(fake("hangs"), some_bytes(), false, FIRST_FRAME);
    playback.show(Some(spot(4)));
    played.until(|event| *event == VideoEvent::Playing);
    let pid = played.pid();
    assert!(alive(pid));
    assert_eq!(played.files().len(), 1);
    playback.show(None);
    played.until(|event| *event == VideoEvent::Ended);
    assert!(!alive(pid), "FFmpeg still runs");
    assert!(played.files().is_empty());
    assert!(played.closed.load(Ordering::SeqCst));
    drop(playback);

    // Its owner drops it: nothing to tell, the same cleanup when it returns.
    let (mut playback, played) = play(fake("hangs"), some_bytes(), false, FIRST_FRAME);
    playback.show(Some(spot(4)));
    played.until(|event| *event == VideoEvent::Playing);
    let pid = played.pid();
    drop(playback);
    assert!(!alive(pid), "FFmpeg still runs");
    assert!(played.files().is_empty());
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
    let (sender, events) = mpsc::channel();
    let visible = Arc::new(AtomicBool::new(true));
    let mut playback = Playback::start(Video {
        url: "https://github.com/user-attachments/assets/demo".into(),
        looped: false,
        fit: FIT,
        folder: test.path().join("videos"),
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
    assert_eq!(
        events.recv_timeout(timeout),
        Ok(VideoEvent::Downloading(18, Some(18)))
    );
    assert_eq!(events.recv_timeout(timeout), Ok(VideoEvent::Playing));
    visible.store(false, Ordering::SeqCst);
    assert_eq!(events.recv_timeout(timeout), Ok(VideoEvent::Ended));
    let pid: i32 = fs::read_to_string(test.path().join("pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(!alive(pid));
}

#[test]
fn failures_tell_why_and_leave_nothing() {
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
    assert!(played.files().is_empty());

    // Long enough for the fake to note its pid, even on a busy machine.
    let (mut playback, played) = play(fake("silent"), some_bytes(), false, Duration::from_secs(2));
    playback.show(Some(spot(4)));
    let events = played.until(failed);
    assert_eq!(
        events.last(),
        Some(&VideoEvent::Failed("no frame after 2 s".into()))
    );
    assert!(!alive(played.pid()), "FFmpeg still runs");
    assert!(played.files().is_empty());

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
    assert!(!old.exists(), "a file of two hours stays");
    assert!(new.exists(), "the new file of a living process is deleted");
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
    let (mut playback, played) = play(ffmpeg, Copy(Ok(tiny)), false, FIRST_FRAME);
    playback.show(Some(spot(4)));
    played.until(|event| *event == VideoEvent::Ended);
    let shown = played.shown.lock().unwrap().clone();
    assert_eq!(shown.len(), 10, "{shown:?}");
    assert!(
        shown
            .iter()
            .all(|(width, height, _)| (*width, *height) == (160, 90))
    );
    assert!(played.files().is_empty());
}
