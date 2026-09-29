//! Animated README images play in the details pane: their frames decode,
//! Herdr draws them in turn on a layer over the first frame, on the rows
//! the pane shows, and they leave when the picture no longer shows. Offline:
//! Herdr is a socket of the test.

mod support;

use std::io::{BufRead, BufReader, Cursor, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use herdr_marketplace::adapters::images::{
    Animation, AnimationFrame, FRAMES_BUDGET, crop_bands, decode, frames, frames_within,
};
use herdr_marketplace::adapters::pane_graphics::{Cells, Layer};
use herdr_marketplace::adapters::tui::animation::{Player, Spot, frame_at};
use herdr_marketplace::adapters::tui::details::DetailsApp;
use herdr_marketplace::adapters::tui::details_view;
use herdr_marketplace::adapters::tui::graphics::{Graphics, Pictures};
use herdr_marketplace::application::load_readme::Readme;
use herdr_marketplace::domain::details::DetailsTarget;
use herdr_marketplace::domain::registry::parse_registry;
use herdr_marketplace::domain::source::PluginSource;
use herdr_marketplace::domain::uninstall::plan_removal;
use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, Frame, ImageFormat, Rgba, RgbaImage};
use serde_json::{Value, json};
use support::*;

const RED: [u8; 4] = [200, 30, 30, 255];
const GREEN: [u8; 4] = [30, 200, 30, 255];
const BLUE: [u8; 4] = [30, 30, 200, 255];
const DEMO: &str = "https://example.com/demo.gif";
const PLACEHOLDER: char = '\u{10EEEE}';

/// A GIF of `width` × `height` pixels: one frame per color, shown for its
/// milliseconds.
fn gif(width: u32, height: u32, frames: &[([u8; 4], u32)], repeat: Repeat) -> Vec<u8> {
    let images = frames
        .iter()
        .map(|(color, delay)| (RgbaImage::from_pixel(width, height, Rgba(*color)), *delay));
    gif_of(images, repeat)
}

fn gif_of(frames: impl Iterator<Item = (RgbaImage, u32)>, repeat: Repeat) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = GifEncoder::new(&mut bytes);
        encoder.set_repeat(repeat).unwrap();
        for (image, delay) in frames {
            let delay = Delay::from_numer_denom_ms(delay, 1);
            encoder
                .encode_frame(Frame::from_parts(image, 0, 0, delay))
                .unwrap();
        }
    }
    bytes
}

fn png(image: &RgbaImage) -> Vec<u8> {
    let mut bytes = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .unwrap();
    bytes
}

fn pixels(png: &[u8]) -> RgbaImage {
    image::load_from_memory_with_format(png, ImageFormat::Png)
        .unwrap()
        .into_rgba8()
}

/// GIF palettes round colors a little.
fn near(pixel: &Rgba<u8>, color: [u8; 4]) -> bool {
    pixel
        .0
        .iter()
        .zip(color)
        .all(|(got, want)| got.abs_diff(want) <= 8)
}

fn delays(animation: &Animation) -> Vec<u128> {
    animation
        .frames
        .iter()
        .map(|frame| frame.delay.as_millis())
        .collect()
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap()
}

#[test]
fn an_animated_gif_keeps_its_frames_and_how_long_each_shows() {
    let bytes = gif(
        8,
        4,
        &[(RED, 50), (GREEN, 0), (BLUE, 200)],
        Repeat::Finite(2),
    );
    let poster = decode(&bytes).unwrap();
    assert!(poster.animated.is_some(), "its file stays for the frames");
    assert!(
        near(poster.rgba.get_pixel(0, 0), RED),
        "drawn as its first frame"
    );

    let animation = frames(&bytes, (100, 100)).unwrap();
    assert_eq!(
        delays(&animation),
        [50, 100, 200],
        "a frame without delay shows 100 ms, as in browsers"
    );
    assert_eq!(animation.plays, Some(2));
    assert!(animation.opaque);
    assert_eq!((animation.width, animation.height), (8, 4));
    let colors: Vec<bool> = animation
        .frames
        .iter()
        .zip([RED, GREEN, BLUE])
        .map(|(frame, color)| near(pixels(&frame.png).get_pixel(3, 2), color))
        .collect();
    assert_eq!(colors, [true, true, true], "each frame in turn");
}

#[test]
fn still_images_have_no_frames_to_play() {
    let single = gif(8, 4, &[(RED, 100)], Repeat::Infinite);
    assert!(decode(&single).unwrap().animated.is_none());
    assert!(frames(&single, (100, 100)).is_err());
    let still = png(&RgbaImage::from_pixel(8, 4, Rgba(RED)));
    let picture = decode(&still).unwrap();
    assert!(picture.animated.is_none());
    assert_eq!(picture.png, still, "a still PNG is still sent as it came");
    assert!(frames(&still, (100, 100)).is_err());
}

#[test]
fn animated_webp_and_png_play_like_a_gif() {
    // Three 6 × 4 frames of 60, 120 and 180 ms, written by Pillow:
    // `frames[0].save(name, save_all=True, append_images=frames[1:],
    // duration=[60, 120, 180], loop=…)`, looping forever or three times.
    for (name, plays) in [("blink.webp", None), ("blink.png", Some(3))] {
        let bytes = fixture(name);
        let poster = decode(&bytes).unwrap();
        assert!(poster.animated.is_some(), "{name}");
        assert_eq!(
            poster.rgba.get_pixel(0, 0),
            &Rgba([220, 40, 40, 255]),
            "{name}"
        );
        assert!(
            decode(&poster.png).unwrap().animated.is_none(),
            "{name}: the terminal gets the first frame, not the whole file"
        );
        let animation = frames(&bytes, (100, 100)).unwrap();
        assert_eq!(delays(&animation), [60, 120, 180], "{name}");
        assert_eq!(animation.plays, plays, "{name}");
        let last = pixels(&animation.frames[2].png);
        assert!(near(last.get_pixel(5, 3), [40, 80, 220, 255]), "{name}");
    }
}

#[test]
fn frames_fit_the_cells_of_their_picture_and_are_never_enlarged() {
    let bytes = gif(64, 32, &[(RED, 100), (GREEN, 100)], Repeat::Infinite);
    let small = frames(&bytes, (20, 20)).unwrap();
    assert_eq!((small.width, small.height), (20, 10));
    assert_eq!(pixels(&small.frames[1].png).dimensions(), (20, 10));
    let large = frames(&bytes, (1000, 1000)).unwrap();
    assert_eq!((large.width, large.height), (64, 32));
}

#[test]
fn past_the_budget_every_other_frame_goes_and_the_time_stays() {
    // Noise does not compress: each frame weighs about the same.
    let mut seed = 7u32;
    let mut noise = || {
        let mut image = RgbaImage::new(48, 48);
        for pixel in image.pixels_mut() {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let [a, b, c, _] = seed.to_be_bytes();
            *pixel = Rgba([a, b, c, 255]);
        }
        (image, 100)
    };
    let bytes = gif_of((0..8).map(|_| noise()), Repeat::Infinite);
    let all = frames_within(&bytes, (100, 100), usize::MAX).unwrap();
    assert_eq!(all.frames.len(), 8);
    let budget = all.frames[0].png.len() * 3;
    let kept = frames_within(&bytes, (100, 100), budget).unwrap();
    assert!(
        (2..8).contains(&kept.frames.len()),
        "{} frames",
        kept.frames.len()
    );
    assert!(kept.size() <= budget, "{} > {budget}", kept.size());
    assert_eq!(
        delays(&kept).iter().sum::<u128>(),
        800,
        "the animation lasts as long"
    );
    assert!(delays(&kept).iter().all(|delay| delay % 100 == 0));
    assert_eq!(
        kept.frames[0].png, all.frames[0].png,
        "it still starts on the first frame"
    );
}

#[test]
fn a_transparent_animation_is_not_opaque() {
    let mut half = RgbaImage::from_pixel(8, 4, Rgba(RED));
    for y in 0..4 {
        for x in 4..8 {
            half.put_pixel(x, y, Rgba([0, 0, 0, 0]));
        }
    }
    let bytes = gif_of(
        [(half, 100), (RgbaImage::from_pixel(8, 4, Rgba(GREEN)), 100)].into_iter(),
        Repeat::Infinite,
    );
    let animation = frames(&bytes, (100, 100)).unwrap();
    assert!(!animation.opaque);
    assert_eq!(pixels(&animation.frames[0].png).get_pixel(6, 1)[3], 0);
}

#[test]
fn the_bands_shown_are_cut_out_of_a_frame() {
    let colors = [RED, GREEN, BLUE, [250, 250, 250, 255]];
    let mut image = RgbaImage::new(4, 8);
    for (_, y, pixel) in image.enumerate_pixels_mut() {
        *pixel = Rgba(colors[y as usize / 2]);
    }
    let (part, width, height) = crop_bands(&png(&image), 1, 3, 4).unwrap();
    assert_eq!((width, height), (4, 4));
    let part = pixels(&part);
    assert_eq!(part.dimensions(), (4, 4));
    assert_eq!(part.get_pixel(0, 0), &Rgba(GREEN));
    assert_eq!(part.get_pixel(3, 3), &Rgba(BLUE));
}

#[test]
fn frames_decode_once_for_each_size_of_their_cells() {
    let bytes = gif(16, 8, &[(RED, 100), (GREEN, 100)], Repeat::Infinite);
    let mut pictures = Pictures::new(Graphics::Kitty {
        cell_width: 8,
        cell_height: 16,
    });
    assert!(pictures.request(DEMO));
    pictures.loaded(DEMO, Ok(Arc::new(decode(&bytes).unwrap())));
    assert!(pictures.animated(DEMO));
    let (file, budget) = pictures.frames_to_decode(DEMO, (16, 16)).unwrap();
    assert_eq!(&*file, &bytes[..], "the file of the picture");
    assert_eq!(budget, FRAMES_BUDGET);
    assert!(pictures.frames_to_decode(DEMO, (16, 16)).is_none(), "once");
    assert!(
        pictures.frames_to_decode(DEMO, (24, 24)).is_none(),
        "a pane resized while they decode waits for them"
    );
    let small = Arc::new(frames(&file, (16, 16)).unwrap());
    pictures.frames_decoded(DEMO, (16, 16), Ok(small.clone()));
    assert!(pictures.animation(DEMO, (16, 16)).is_some());

    // A wider pane: the frames decode again, the smaller ones no longer show.
    assert!(pictures.frames_to_decode(DEMO, (32, 32)).is_some());
    assert!(pictures.animation(DEMO, (16, 16)).is_none());
    pictures.frames_decoded(DEMO, (16, 16), Ok(small));
    assert!(
        pictures.animation(DEMO, (16, 16)).is_none(),
        "frames for an earlier size are dropped"
    );
    pictures.frames_decoded(DEMO, (32, 32), Err("broken".into()));
    assert!(
        pictures.frames_to_decode(DEMO, (64, 64)).is_none(),
        "an animation that failed stays still"
    );

    let still = png(&RgbaImage::from_pixel(8, 4, Rgba(RED)));
    let url = "https://example.com/still.png";
    assert!(pictures.request(url));
    pictures.loaded(url, Ok(Arc::new(decode(&still).unwrap())));
    assert!(!pictures.animated(url));
    assert!(pictures.frames_to_decode(url, (16, 16)).is_none());
}

#[test]
fn the_frame_shown_follows_the_time_and_the_plays() {
    let delays = [100, 200, 300].map(Duration::from_millis);
    let at = |ms, plays| frame_at(&delays, plays, Duration::from_millis(ms));
    let ms = Duration::from_millis;
    assert_eq!(at(0, None), (0, Some(ms(100))));
    assert_eq!(at(150, None), (1, Some(ms(150))));
    assert_eq!(at(599, None), (2, Some(ms(1))));
    assert_eq!(at(650, None), (0, Some(ms(50))), "it starts over");
    assert_eq!(at(650, Some(2)), (0, Some(ms(50))));
    assert_eq!(
        at(1250, Some(2)),
        (2, None),
        "its last play ends on its last frame"
    );
}

fn target() -> DetailsTarget {
    DetailsTarget {
        source: PluginSource {
            owner: "massdo".into(),
            repo: "herdr-marketplace-fixture".into(),
            subdir: String::new(),
        },
        commit: SHA_A.into(),
        id: "herdr-marketplace-fixture".into(),
        name: "herdr-marketplace fixture".into(),
        version: Some("1.0.0".into()),
        in_catalog: true,
        compatible: true,
    }
}

/// A details pane whose README shows an animated 160 × 80 GIF, 20 × 5 cells
/// at 8 × 16 pixels a cell, from its third line.
fn pane_with_demo(after: &str) -> DetailsApp {
    let mut app = DetailsApp::new(target());
    app.set_graphics(Graphics::Kitty {
        cell_width: 8,
        cell_height: 16,
    });
    app.readme_loaded(
        1,
        Ok(Readme::Found {
            text: format!("Intro\n\n![demo]({DEMO})\n\n{after}"),
            fallback: false,
        }),
    );
    let bytes = gif(160, 80, &[(RED, 100), (GREEN, 100)], Repeat::Infinite);
    app.picture_loaded(DEMO, Ok(Arc::new(decode(&bytes).unwrap())));
    app
}

fn screen(app: &mut DetailsApp, width: u16, height: u16) -> Vec<String> {
    app.set_viewport(width as usize, details_view::page_rows(app, width, height));
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| details_view::render(frame, app))
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .chunks(width as usize)
        .map(|line| line.iter().map(|cell| cell.symbol()).collect())
        .collect()
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn an_animation_shows_on_the_rows_of_its_picture() {
    let mut app = pane_with_demo(&"More text.\n\n".repeat(20));
    let lines = screen(&mut app, 60, 20);
    let top = lines
        .iter()
        .position(|line| line.contains(PLACEHOLDER))
        .unwrap();
    assert_eq!(
        details_view::spots(&app, 60, 20),
        [(
            DEMO.to_string(),
            Spot {
                cells: Cells {
                    column: 0,
                    row: top as u16,
                    columns: 20,
                    rows: 5,
                },
                first: 0,
                rows: 5,
            }
        )]
    );

    // Two rows under the header: the last three rows show.
    for _ in 0..4 {
        app.handle_key(key(KeyCode::Down));
    }
    let lines = screen(&mut app, 60, 20);
    let body = lines
        .iter()
        .position(|line| line.contains(PLACEHOLDER))
        .unwrap();
    assert_eq!(body, top - 2, "the body starts with its fifth line");
    let [(_, spot)] = details_view::spots(&app, 60, 20).try_into().unwrap();
    assert_eq!(
        spot,
        Spot {
            cells: Cells {
                column: 0,
                row: body as u16,
                columns: 20,
                rows: 3,
            },
            first: 2,
            rows: 5,
        }
    );
}

#[test]
fn a_short_pane_shows_the_top_of_an_animation_and_none_once_scrolled_past() {
    let mut app = pane_with_demo(&"More text.\n\n".repeat(20));
    // Two picture rows fit under the header.
    let lines = screen(&mut app, 60, 20);
    let top = lines
        .iter()
        .position(|line| line.contains(PLACEHOLDER))
        .unwrap() as u16;
    let height = top + 2 + 1;
    screen(&mut app, 60, height);
    let [(_, spot)] = details_view::spots(&app, 60, height).try_into().unwrap();
    assert_eq!((spot.cells.row, spot.cells.rows, spot.first), (top, 2, 0));

    app.handle_key(key(KeyCode::End));
    screen(&mut app, 60, height);
    assert!(details_view::spots(&app, 60, height).is_empty());
}

#[test]
fn a_confirmation_over_the_readme_hides_its_animations() {
    let mut app = pane_with_demo("Outro");
    screen(&mut app, 60, 20);
    assert_eq!(details_view::spots(&app, 60, 20).len(), 1);
    let registry = registry(vec![github_plugin(
        "herdr-marketplace-fixture",
        "massdo",
        "herdr-marketplace-fixture",
        None,
        SHA_A,
    )]);
    let plan = plan_removal(&parse_registry(&registry).unwrap(), &target().source).unwrap();
    app.handle_key(key(KeyCode::Char('r')));
    app.removal_prepared(1, Ok(plan));
    screen(&mut app, 60, 20);
    assert!(details_view::spots(&app, 60, 20).is_empty());
}

#[test]
fn a_covered_picture_leaves_its_cells_empty() {
    let mut app = pane_with_demo("Outro");
    let before = screen(&mut app, 60, 20);
    let rows: Vec<usize> = (0..before.len())
        .filter(|row| before[*row].contains(PLACEHOLDER))
        .collect();
    assert_eq!(rows.len(), 5);
    app.covered.insert(DEMO.to_string());
    let after = screen(&mut app, 60, 20);
    for row in &rows {
        assert_eq!(after[*row].trim(), "", "row {row}: {}", after[*row]);
    }
    assert!(after.iter().any(|line| line.starts_with("Outro")));
}

/// What a layer of the fake Herdr received.
#[derive(Debug)]
enum Received {
    Frame {
        layer: String,
        header: Value,
        png: Vec<u8>,
    },
    Closed {
        layer: String,
    },
}

/// Herdr's socket, as the test plays it: `pane.graphics.info` answers
/// `visible`, and `pane.graphics.stream` opens a layer, or refuses it with
/// `refusal`.
struct FakeHerdr {
    path: PathBuf,
    visible: Arc<AtomicBool>,
    requests: mpsc::Receiver<Value>,
    received: mpsc::Receiver<Received>,
    _directory: tempfile::TempDir,
}

impl FakeHerdr {
    fn start(refusal: Option<&'static str>) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("herdr.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let visible = Arc::new(AtomicBool::new(true));
        let (request_sender, requests) = mpsc::channel();
        let (sender, received) = mpsc::channel();
        let shown = visible.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let (requests, sender, shown) =
                    (request_sender.clone(), sender.clone(), shown.clone());
                thread::spawn(move || serve(stream, refusal, &shown, &requests, &sender));
            }
        });
        Self {
            path,
            visible,
            requests,
            received,
            _directory: directory,
        }
    }

    fn next(&self) -> Received {
        self.received
            .recv_timeout(Duration::from_secs(5))
            .expect("Herdr received nothing")
    }

    fn nothing_for(&self, wait: Duration) -> bool {
        self.received.recv_timeout(wait).is_err()
    }
}

fn serve(
    stream: UnixStream,
    refusal: Option<&str>,
    visible: &AtomicBool,
    requests: &mpsc::Sender<Value>,
    received: &mpsc::Sender<Received>,
) {
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let request: Value = serde_json::from_str(&line).unwrap();
    let _ = requests.send(request.clone());
    let id = request["id"].clone();
    let answer = match (request["method"].as_str(), refusal) {
        (Some("pane.graphics.info"), _) => json!({"id": id, "result": {
            "type": "pane_graphics_info", "cell_width_px": 8, "cell_height_px": 16,
            "pane_visible": visible.load(Ordering::Acquire),
        }}),
        (Some("pane.graphics.stream"), None) => json!({"id": id, "result": {"type": "ok"}}),
        (Some("pane.graphics.stream"), Some(code)) => json!({"id": id, "error": {
            "code": code, "message": "pane graphics are disabled",
        }}),
        _ => panic!("unexpected request {request}"),
    };
    writer.write_all(format!("{answer}\n").as_bytes()).unwrap();
    if request["method"] != "pane.graphics.stream" || refusal.is_some() {
        return;
    }
    let layer = request["params"]["layer_id"].as_str().unwrap().to_string();
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 {
            let _ = received.send(Received::Closed { layer });
            return;
        }
        let header: Value = serde_json::from_str(&header).unwrap();
        let mut png = vec![0; header["data_length"].as_u64().unwrap() as usize];
        reader.read_exact(&mut png).unwrap();
        let _ = received.send(Received::Frame {
            layer: layer.clone(),
            header,
            png,
        });
    }
}

#[test]
fn a_layer_opens_over_its_pane_and_sends_each_png_with_its_cells() {
    let herdr = FakeHerdr::start(None);
    let mut layer = Layer::open(&herdr.path, "w1:p2", "animation-1").unwrap();
    let request = herdr.requests.recv().unwrap();
    assert_eq!(request["method"], "pane.graphics.stream");
    assert_eq!(
        request["params"],
        json!({"pane_id": "w1:p2", "layer_id": "animation-1", "z_index": 1})
    );
    let cells = Cells {
        column: 3,
        row: 4,
        columns: 5,
        rows: 2,
    };
    layer.show(b"PNG bytes", 20, 10, cells).unwrap();
    let Received::Frame { header, png, .. } = herdr.next() else {
        panic!("no frame")
    };
    assert_eq!(
        header,
        json!({
            "format": "png", "image_width": 20, "image_height": 10, "data_length": 9,
            "placement": {"viewport_col": 3, "viewport_row": 4, "grid_cols": 5, "grid_rows": 2},
        })
    );
    assert_eq!(png, b"PNG bytes");
    drop(layer);
    assert!(
        matches!(herdr.next(), Received::Closed { .. }),
        "the layer left"
    );
}

#[test]
fn a_layer_herdr_refuses_is_an_error() {
    let herdr = FakeHerdr::start(Some("feature_disabled"));
    let error = Layer::open(&herdr.path, "w1:p2", "animation-1")
        .err()
        .unwrap();
    assert!(error.contains("feature_disabled"), "{error}");
}

/// Three frames of 4 × 4 pixels, red, green and blue, 100 ms each.
fn blink() -> Arc<Animation> {
    let frames = [RED, GREEN, BLUE]
        .into_iter()
        .map(|color| AnimationFrame {
            png: png(&RgbaImage::from_pixel(4, 4, Rgba(color))),
            delay: Duration::from_millis(100),
        })
        .collect();
    Arc::new(Animation {
        frames,
        width: 4,
        height: 4,
        plays: None,
        opaque: true,
    })
}

fn spot(first: u16, shown: u16) -> Spot {
    Spot {
        cells: Cells {
            column: 2,
            row: 6,
            columns: 4,
            rows: shown,
        },
        first,
        rows: 4,
    }
}

fn frame(received: Received) -> (String, Value, RgbaImage) {
    match received {
        Received::Frame { layer, header, png } => (layer, header, pixels(&png)),
        other => panic!("expected a frame, got {other:?}"),
    }
}

#[test]
fn the_player_shows_each_frame_in_turn_until_its_picture_leaves() {
    let herdr = FakeHerdr::start(None);
    let mut player = Player::new(herdr.path.clone(), "w1:p2".into());
    let animation = blink();
    player.show(&[(DEMO.to_string(), animation.clone(), spot(0, 4))]);
    let started = Instant::now();
    let colors: Vec<Rgba<u8>> = (0..4)
        .map(|_| {
            let (layer, header, image) = frame(herdr.next());
            assert_eq!(layer, "animation-1");
            assert_eq!(
                header["placement"],
                json!({"viewport_col": 2, "viewport_row": 6, "grid_cols": 4, "grid_rows": 4})
            );
            *image.get_pixel(0, 0)
        })
        .collect();
    assert_eq!(colors, [RED, GREEN, BLUE, RED].map(Rgba));
    assert!(
        started.elapsed() >= Duration::from_millis(250),
        "each in turn"
    );
    assert!(player.live(DEMO));

    // The same spot again sends nothing new; another one moves the frame.
    player.show(&[(DEMO.to_string(), animation.clone(), spot(0, 4))]);
    player.show(&[(DEMO.to_string(), animation.clone(), spot(2, 2))]);
    let (_, header, image) = loop {
        let (layer, header, image) = frame(herdr.next());
        if header["placement"]["grid_rows"] == 2 {
            break (layer, header, image);
        }
    };
    assert_eq!(header["placement"]["viewport_row"], 6);
    assert_eq!(image.dimensions(), (4, 2), "its lower half");

    player.show(&[]);
    loop {
        if let Received::Closed { layer } = herdr.next() {
            assert_eq!(layer, "animation-1");
            break;
        }
    }
    assert!(!player.live(DEMO));
}

#[test]
fn new_frames_for_other_cells_start_on_a_new_layer() {
    let herdr = FakeHerdr::start(None);
    let mut player = Player::new(herdr.path.clone(), "w1:p2".into());
    player.show(&[(DEMO.to_string(), blink(), spot(0, 4))]);
    assert_eq!(frame(herdr.next()).0, "animation-1");
    player.show(&[(DEMO.to_string(), blink(), spot(0, 4))]);
    let mut closed = false;
    let mut moved = false;
    while !(closed && moved) {
        match herdr.next() {
            Received::Closed { layer } => closed |= layer == "animation-1",
            Received::Frame { layer, .. } => moved |= layer == "animation-2",
        }
    }
}

#[test]
fn animations_wait_while_herdr_hides_their_pane() {
    let herdr = FakeHerdr::start(None);
    herdr.visible.store(false, Ordering::Release);
    let mut player = Player::new(herdr.path.clone(), "w1:p2".into());
    player.show(&[(DEMO.to_string(), blink(), spot(0, 4))]);
    assert!(herdr.nothing_for(Duration::from_millis(400)));
    herdr.visible.store(true, Ordering::Release);
    frame(herdr.next());
}
