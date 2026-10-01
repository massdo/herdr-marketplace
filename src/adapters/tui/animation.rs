//! Animated README pictures, played over their first frame. The pane draws
//! that frame with the kitty protocol, like any picture; Herdr lays the
//! following ones over it, sent as PNG on a layer of the pane (see
//! `pane_graphics`). One thread per animation keeps its time, and follows
//! the rows of the picture the pane shows.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crate::adapters::images::{Animation, crop_bands};
use crate::adapters::pane_graphics::{Cells, Layer, pane_visible};

/// How often Herdr is asked whether it shows the pane.
const VISIBILITY_CHECK: Duration = Duration::from_secs(1);
/// How often an animation of a hidden pane looks again whether it may play.
const HIDDEN_CHECK: Duration = Duration::from_millis(50);

/// Where an animated picture shows: the cells of the rows the pane shows,
/// rows `first..first + cells.rows` of its `rows`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spot {
    pub cells: Cells,
    pub first: u16,
    pub rows: u16,
}

impl Spot {
    fn whole(&self) -> bool {
        self.first == 0 && self.cells.rows >= self.rows
    }
}

/// The animations of a details pane, each on its own layer of it.
pub struct Player {
    socket: PathBuf,
    pane: String,
    /// Whether Herdr shows the pane; animations wait while it does not, and
    /// until it tells.
    visible: Arc<AtomicBool>,
    /// Keeps asking Herdr whether it shows the pane, from the first
    /// animation on.
    watching: Option<Arc<AtomicBool>>,
    tracks: HashMap<String, Track>,
    /// Layers opened so far, which name the next one.
    layers: usize,
}

struct Track {
    animation: Arc<Animation>,
    spots: Sender<Option<Spot>>,
    /// The last spot sent to its thread.
    spot: Option<Spot>,
    /// Herdr shows its frames.
    live: Arc<AtomicBool>,
}

impl Player {
    /// Animations of pane `pane`, through the Herdr socket at `socket`.
    pub fn new(socket: PathBuf, pane: String) -> Self {
        Self {
            socket,
            pane,
            visible: Arc::new(AtomicBool::new(false)),
            watching: None,
            tracks: HashMap::new(),
            layers: 0,
        }
    }

    /// Plays these animations at their spots; the others leave the pane.
    /// An animation starts when it first shows, then keeps its time while it
    /// does not.
    pub fn show(&mut self, shown: &[(String, Arc<Animation>, Spot)]) {
        for (url, track) in &mut self.tracks {
            if track.spot.is_some() && !shown.iter().any(|(wanted, ..)| wanted == url) {
                track.spot = None;
                let _ = track.spots.send(None);
            }
        }
        for (url, animation, spot) in shown {
            // New frames, for another size of the cells, start over.
            if self
                .tracks
                .get(url)
                .is_none_or(|track| !Arc::ptr_eq(&track.animation, animation))
            {
                let track = self.start(animation.clone());
                self.tracks.insert(url.clone(), track);
            }
            if let Some(track) = self.tracks.get_mut(url)
                && track.spot != Some(*spot)
            {
                track.spot = Some(*spot);
                let _ = track.spots.send(Some(*spot));
            }
        }
    }

    /// Whether Herdr shows the frames of `url` over its first one.
    pub fn live(&self, url: &str) -> bool {
        self.tracks
            .get(url)
            .is_some_and(|track| track.live.load(Ordering::Acquire))
    }

    /// The Herdr socket and the pane, for another layer of it.
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    pub fn pane(&self) -> &str {
        &self.pane
    }

    /// Whether Herdr shows the pane, asked from now on: false until it
    /// tells.
    pub fn visible(&mut self) -> Arc<AtomicBool> {
        self.watch();
        self.visible.clone()
    }

    /// Asks Herdr every second whether it shows the pane, from the first
    /// call on.
    fn watch(&mut self) {
        if self.watching.is_some() {
            return;
        }
        let watching = Arc::new(AtomicBool::new(true));
        let (socket, pane) = (self.socket.clone(), self.pane.clone());
        let (visible, going) = (self.visible.clone(), watching.clone());
        thread::spawn(move || {
            let mut known = false;
            while going.load(Ordering::Acquire) {
                match pane_visible(&socket, &pane) {
                    Ok(shown) => {
                        visible.store(shown, Ordering::Release);
                        known = true;
                    }
                    // Herdr may not tell: then the animations play.
                    Err(_) if !known => visible.store(true, Ordering::Release),
                    Err(_) => {}
                }
                thread::sleep(VISIBILITY_CHECK);
            }
        });
        self.watching = Some(watching);
    }

    fn start(&mut self, animation: Arc<Animation>) -> Track {
        self.watch();
        let (spots, received) = mpsc::channel();
        let live = Arc::new(AtomicBool::new(false));
        self.layers += 1;
        let layer = format!("animation-{}", self.layers);
        let (socket, pane) = (self.socket.clone(), self.pane.clone());
        let visible = self.visible.clone();
        let playing = live.clone();
        let frames = animation.clone();
        thread::spawn(move || {
            let open = || Layer::open(&socket, &pane, &layer);
            play(&frames, &open, &received, &visible, &playing);
            playing.store(false, Ordering::Release);
        });
        Track {
            animation,
            spots,
            spot: None,
            live,
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        if let Some(watching) = &self.watching {
            watching.store(false, Ordering::Release);
        }
    }
}

/// Shows the frame of the time where the pane shows the picture, until the
/// player goes. A layer that fails leaves the first frame alone.
fn play(
    animation: &Animation,
    open: &dyn Fn() -> Result<Layer, String>,
    spots: &Receiver<Option<Spot>>,
    visible: &AtomicBool,
    live: &AtomicBool,
) {
    let delays: Vec<Duration> = animation.frames.iter().map(|frame| frame.delay).collect();
    // Its time runs from its first frame shown.
    let mut start = None::<Instant>;
    let mut spot = None::<Spot>;
    let mut layer = None::<Layer>;
    // The frame and spot Herdr shows.
    let mut shown = None::<(usize, Spot)>;
    loop {
        let elapsed = start.map_or(Duration::ZERO, |start| start.elapsed());
        let (index, left) = frame_at(&delays, animation.plays, elapsed);
        let wait = match spot {
            None => None,
            Some(_) if !visible.load(Ordering::Acquire) => Some(HIDDEN_CHECK),
            Some(spot) => {
                if shown != Some((index, spot)) {
                    let sent = match &mut layer {
                        Some(layer) => send(layer, animation, index, spot),
                        None => open()
                            .and_then(|opened| send(layer.insert(opened), animation, index, spot)),
                    };
                    if sent.is_err() {
                        return;
                    }
                    shown = Some((index, spot));
                    start.get_or_insert_with(Instant::now);
                    live.store(true, Ordering::Release);
                }
                left
            }
        };
        let next = match wait {
            Some(wait) => spots.recv_timeout(wait),
            None => spots.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match next {
            Ok(mut next) => {
                // Only the latest spot counts.
                while let Ok(later) = spots.try_recv() {
                    next = later;
                }
                spot = next;
                if spot.is_none() {
                    live.store(false, Ordering::Release);
                    layer = None;
                    shown = None;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Frame `index` over `spot`: whole, or the part of it the visible rows
/// show.
fn send(layer: &mut Layer, animation: &Animation, index: usize, spot: Spot) -> Result<(), String> {
    let frame = &animation.frames[index];
    if spot.whole() {
        return layer.show(&frame.png, animation.width, animation.height, spot.cells);
    }
    let last = spot.first + spot.cells.rows;
    let (png, width, height) = crop_bands(&frame.png, spot.first, last, spot.rows)?;
    layer.show(&png, width, height, spot.cells)
}

/// The frame shown `elapsed` after the start, with frames showing for
/// `delays` and the whole playing `plays` times, or without end; and how
/// long it still shows, `None` when the last play ended on it.
pub fn frame_at(
    delays: &[Duration],
    plays: Option<u32>,
    elapsed: Duration,
) -> (usize, Option<Duration>) {
    let total = delays.iter().sum::<Duration>().as_nanos();
    let last = delays.len().saturating_sub(1);
    if total == 0 {
        return (last, None);
    }
    let elapsed = elapsed.as_nanos();
    if plays.is_some_and(|plays| elapsed / total >= u128::from(plays)) {
        return (last, None);
    }
    let mut within = elapsed % total;
    for (index, delay) in delays.iter().enumerate() {
        let delay = delay.as_nanos();
        if within < delay {
            let left = u64::try_from(delay - within).unwrap_or(u64::MAX);
            return (index, Some(Duration::from_nanos(left)));
        }
        within -= delay;
    }
    (last, Some(Duration::ZERO))
}
