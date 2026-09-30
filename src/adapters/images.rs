//! README images, decoded and kept small enough to draw: PNG, JPEG, GIF,
//! WebP and SVG. Only pixels come out; nothing of the file reaches the
//! terminal as is. An animated GIF, WebP or PNG is drawn as its first frame
//! and keeps its file, whose frames `frames` decodes to play them.

use std::fmt;
use std::io::Cursor;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::application::ports::Fetcher;
use image::buffer::ConvertBuffer;
use image::codecs::gif::GifDecoder;
use image::codecs::png::{CompressionType, FilterType as PngFilter, PngDecoder, PngEncoder};
use image::codecs::webp::WebPDecoder;
use image::imageops::FilterType;
use image::metadata::LoopCount;
use image::{
    AnimationDecoder, ExtendedColorType, ImageDecoder, ImageEncoder, ImageFormat, ImageReader,
    Limits, RgbImage, Rgba, RgbaImage,
};
use resvg::{tiny_skia, usvg};

/// One queue per pane, shared by two workers even across README reloads.
pub fn load_in_background(
    fetcher: impl Fetcher + Send + Sync + 'static,
    completed: impl Fn(String, Result<Arc<Picture>, String>) + Send + Sync + 'static,
) -> std::sync::mpsc::Sender<String> {
    let (sender, receiver) = std::sync::mpsc::channel::<String>();
    let receiver = Arc::new(std::sync::Mutex::new(receiver));
    let fetcher = Arc::new(fetcher);
    let completed = Arc::new(completed);
    for _ in 0..2 {
        let receiver = receiver.clone();
        let fetcher = fetcher.clone();
        let completed = completed.clone();
        std::thread::spawn(move || {
            loop {
                let next = receiver.lock().expect("image queue").recv();
                let Ok(url) = next else { break };
                let picture = fetcher
                    .fetch(&url, IMAGE_LIMIT)
                    .map_err(|error| error.to_string())
                    .and_then(|bytes| decode(&bytes))
                    .map(Arc::new);
                completed(url, picture);
            }
        });
    }
    sender
}

/// Upper bound for one downloaded image.
pub const IMAGE_LIMIT: u64 = 16 * 1024 * 1024;
/// Longest side kept: enough for a pane, light to send to the terminal.
const LONGEST_SIDE: u32 = 1280;
/// Decoding refuses larger declared sizes before allocating them.
const DECODED_SIDE: u32 = 8192;
const DECODED_BYTES: u64 = 64 * 1024 * 1024;

/// PNG bytes the frames of one animation may take: beyond, every other frame
/// goes, and the one before it shows as long as both did.
pub const FRAMES_BUDGET: usize = 64 * 1024 * 1024;
/// Frames an animation may have, and bytes of frames decoding may compose,
/// before it stays still.
const MAX_FRAMES: usize = 1000;
const COMPOSED_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Browsers show a frame of 10 ms or less for 100 ms.
const INSTANT: Duration = Duration::from_millis(10);
const INSTANT_SHOWN: Duration = Duration::from_millis(100);

/// A decoded image and the PNG that sends it to the terminal.
#[derive(Debug, Clone, PartialEq)]
pub struct Picture {
    pub rgba: RgbaImage,
    pub png: Vec<u8>,
    /// Size a browser shows, in pixels: the file's own, before shrinking.
    pub width: u32,
    pub height: u32,
    /// File of an animated image, whose first frame this is.
    pub animated: Option<Arc<[u8]>>,
}

/// The frames of an animated image, each a PNG that replaces the one before.
#[derive(Clone, PartialEq)]
pub struct Animation {
    pub frames: Vec<AnimationFrame>,
    /// Size of every frame, in pixels.
    pub width: u32,
    pub height: u32,
    /// Times it plays; `None`: without end.
    pub plays: Option<u32>,
    /// No transparent pixel: nothing under a frame shows through it.
    pub opaque: bool,
}

#[derive(Clone, PartialEq)]
pub struct AnimationFrame {
    pub png: Vec<u8>,
    /// How long the frame shows.
    pub delay: Duration,
}

impl Animation {
    /// Bytes its frames take.
    pub fn size(&self) -> usize {
        self.frames.iter().map(|frame| frame.png.len()).sum()
    }
}

impl fmt::Debug for Animation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Animation")
            .field("frames", &self.frames)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("plays", &self.plays)
            .field("opaque", &self.opaque)
            .finish()
    }
}

impl fmt::Debug for AnimationFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} PNG bytes for {:?}", self.png.len(), self.delay)
    }
}

pub fn decode(bytes: &[u8]) -> Result<Picture, String> {
    if bytes.len() as u64 > IMAGE_LIMIT {
        return Err("image file too large".into());
    }
    if is_svg(bytes) {
        let (rgba, (width, height)) = svg(bytes)?;
        return from_rgba(rgba, width, height);
    }
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| error.to_string())?;
    let (width, height) = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| error.to_string())?
        .into_dimensions()
        .map_err(|error| error.to_string())?;
    if width > DECODED_SIDE
        || height > DECODED_SIDE
        || u64::from(width) * u64::from(height) * 4 > DECODED_BYTES
    {
        return Err("decoded image too large".into());
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(DECODED_SIDE);
    limits.max_image_height = Some(DECODED_SIDE);
    limits.max_alloc = Some(DECODED_BYTES);
    reader.limits(limits);
    let format = reader.format();
    let rgba = reader
        .decode()
        .map_err(|error| error.to_string())?
        .into_rgba8();
    let (width, height) = rgba.dimensions();
    let animated = format
        .is_some_and(|format| is_animated(bytes, format))
        .then(|| Arc::from(bytes));
    // A still PNG small enough is sent as it came.
    if format == Some(ImageFormat::Png) && width.max(height) <= LONGEST_SIDE && animated.is_none() {
        return Ok(Picture {
            rgba,
            png: bytes.to_vec(),
            width,
            height,
            animated,
        });
    }
    let mut picture = from_rgba(rgba, width, height)?;
    picture.animated = animated;
    Ok(picture)
}

fn from_rgba(rgba: RgbaImage, width: u32, height: u32) -> Result<Picture, String> {
    let rgba = shrink(rgba);
    let mut png = Vec::new();
    rgba.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    Ok(Picture {
        rgba,
        png,
        width,
        height,
        animated: None,
    })
}

/// A GIF with a second frame, an animated WebP or an APNG.
fn is_animated(bytes: &[u8], format: ImageFormat) -> bool {
    match format {
        ImageFormat::Gif => GifDecoder::new(Cursor::new(bytes))
            .is_ok_and(|decoder| decoder.into_frames().take(2).count() == 2),
        ImageFormat::WebP => {
            WebPDecoder::new(Cursor::new(bytes)).is_ok_and(|decoder| decoder.has_animation())
        }
        ImageFormat::Png => PngDecoder::new(Cursor::new(bytes))
            .is_ok_and(|decoder| decoder.is_apng().unwrap_or(false)),
        _ => false,
    }
}

/// The frames of an animated image, at most `fit` pixels each and all
/// within `FRAMES_BUDGET`.
pub fn frames(bytes: &[u8], fit: (u32, u32)) -> Result<Animation, String> {
    frames_within(bytes, fit, FRAMES_BUDGET)
}

/// The frames of an animated image as PNG, each shrunk like still images
/// and to at most `fit` pixels: Herdr shows a layer larger than its cells
/// cut, not scaled. All within `budget` bytes: past it, every other frame
/// goes, and the frame before it shows as long as both did.
pub fn frames_within(bytes: &[u8], fit: (u32, u32), budget: usize) -> Result<Animation, String> {
    let error = |error: image::ImageError| error.to_string();
    let mut limits = Limits::default();
    limits.max_image_width = Some(DECODED_SIDE);
    limits.max_image_height = Some(DECODED_SIDE);
    limits.max_alloc = Some(DECODED_BYTES);
    let format = image::guess_format(bytes).map_err(error)?;
    let (plays, frames) = match format {
        ImageFormat::Gif => {
            let mut decoder = GifDecoder::new(Cursor::new(bytes)).map_err(error)?;
            decoder.set_limits(limits).map_err(error)?;
            (decoder.loop_count(), decoder.into_frames())
        }
        ImageFormat::WebP => {
            let mut decoder = WebPDecoder::new(Cursor::new(bytes)).map_err(error)?;
            decoder.set_limits(limits).map_err(error)?;
            (decoder.loop_count(), decoder.into_frames())
        }
        ImageFormat::Png => {
            let decoder = PngDecoder::with_limits(Cursor::new(bytes), limits)
                .map_err(error)?
                .apng()
                .map_err(error)?;
            (decoder.loop_count(), decoder.into_frames())
        }
        _ => return Err("not an animated image".into()),
    };
    let mut kept: Vec<AnimationFrame> = Vec::new();
    // Frames kept: one in `stride`, as the budget allows.
    let mut stride = 1;
    let mut size = 0;
    let mut composed = 0u64;
    let mut opaque = true;
    let (mut width, mut height) = (0, 0);
    for (index, frame) in frames.enumerate() {
        if index >= MAX_FRAMES {
            return Err("too many frames to animate".into());
        }
        let frame = frame.map_err(error)?;
        composed += frame.buffer().as_raw().len() as u64;
        if composed > COMPOSED_BYTES {
            return Err("animation too large to play".into());
        }
        let (numerator, denominator) = frame.delay().numer_denom_ms();
        let delay = match Duration::from_micros(
            u64::from(numerator) * 1000 / u64::from(denominator.max(1)),
        ) {
            delay if delay <= INSTANT => INSTANT_SHOWN,
            delay => delay,
        };
        if index % stride != 0 {
            if let Some(last) = kept.last_mut() {
                last.delay += delay;
            }
            continue;
        }
        let rgba = fit_within(frame.into_buffer(), fit);
        (width, height) = rgba.dimensions();
        let (png, transparent) = frame_png(&rgba)?;
        opaque &= !transparent;
        size += png.len();
        kept.push(AnimationFrame { png, delay });
        while size > budget {
            if kept.len() < 2 {
                return Err("frames too large to animate".into());
            }
            kept = every_other(kept);
            size = kept.iter().map(|frame| frame.png.len()).sum();
            stride *= 2;
        }
    }
    if kept.len() < 2 {
        return Err("a single frame".into());
    }
    Ok(Animation {
        frames: kept,
        width,
        height,
        plays: match plays {
            LoopCount::Infinite => None,
            LoopCount::Finite(plays) => Some(plays.get()),
        },
        opaque,
    })
}

/// `rgba` within `fit` pixels and `LONGEST_SIDE`, keeping its proportions;
/// never enlarged.
fn fit_within(rgba: RgbaImage, (fit_width, fit_height): (u32, u32)) -> RgbaImage {
    let (width, height) = rgba.dimensions();
    let scale = [
        LONGEST_SIDE as f64 / f64::from(width.max(height)),
        f64::from(fit_width.max(1)) / f64::from(width),
        f64::from(fit_height.max(1)) / f64::from(height),
    ]
    .into_iter()
    .fold(1.0, f64::min);
    if scale >= 1.0 {
        return rgba;
    }
    let width = ((f64::from(width) * scale) as u32).max(1);
    let height = ((f64::from(height) * scale) as u32).max(1);
    image::imageops::resize(&rgba, width, height, FilterType::Triangle)
}

/// Every other frame, each showing as long as it and the one it replaces.
fn every_other(frames: Vec<AnimationFrame>) -> Vec<AnimationFrame> {
    let mut kept: Vec<AnimationFrame> = Vec::with_capacity(frames.len().div_ceil(2));
    for (position, frame) in frames.into_iter().enumerate() {
        match kept.last_mut() {
            Some(last) if position % 2 == 1 => last.delay += frame.delay,
            _ => kept.push(frame),
        }
    }
    kept
}

/// A frame as PNG, fast to write, in RGB when it is opaque; and whether a
/// pixel of it is transparent.
fn frame_png(rgba: &RgbaImage) -> Result<(Vec<u8>, bool), String> {
    let transparent = rgba.pixels().any(|pixel| pixel[3] < u8::MAX);
    let mut png = Vec::new();
    let encoder =
        PngEncoder::new_with_quality(&mut png, CompressionType::Fast, PngFilter::Adaptive);
    let written = if transparent {
        encoder.write_image(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            ExtendedColorType::Rgba8,
        )
    } else {
        let rgb: RgbImage = rgba.convert();
        encoder.write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            ExtendedColorType::Rgb8,
        )
    };
    written.map_err(|error| error.to_string())?;
    Ok((png, transparent))
}

/// `width` × `height` pixels of raw RGB as a PNG, fast to write, like the
/// frames of an animation.
pub fn rgb_png(width: u32, height: u32, rgb: &[u8]) -> Result<Vec<u8>, String> {
    let mut png = Vec::new();
    PngEncoder::new_with_quality(&mut png, CompressionType::Fast, PngFilter::Adaptive)
        .write_image(rgb, width, height, ExtendedColorType::Rgb8)
        .map_err(|error| error.to_string())?;
    Ok(png)
}

/// Of a PNG frame cut into `bands` equal horizontal bands, bands
/// `first..last`, as a PNG, with its width and height: the part of an image
/// the visible rows of its cells show.
pub fn crop_bands(
    png: &[u8],
    first: u16,
    last: u16,
    bands: u16,
) -> Result<(Vec<u8>, u32, u32), String> {
    let frame = image::load_from_memory_with_format(png, ImageFormat::Png)
        .map_err(|error| error.to_string())?
        .into_rgba8();
    let (width, height) = frame.dimensions();
    let band = |edge: u16| (u64::from(edge) * u64::from(height) / u64::from(bands.max(1))) as u32;
    let top = band(first).min(height.saturating_sub(1));
    let bottom = band(last).clamp(top + 1, height);
    let part = image::imageops::crop_imm(&frame, 0, top, width, bottom - top).to_image();
    let (png, _) = frame_png(&part)?;
    Ok((png, width, bottom - top))
}

fn shrink(rgba: RgbaImage) -> RgbaImage {
    let longest = rgba.width().max(rgba.height());
    if longest <= LONGEST_SIDE {
        return rgba;
    }
    let scale = LONGEST_SIDE as f32 / longest as f32;
    let width = ((rgba.width() as f32 * scale).round() as u32).max(1);
    let height = ((rgba.height() as f32 * scale).round() as u32).max(1);
    image::imageops::resize(&rgba, width, height, FilterType::Triangle)
}

fn is_svg(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]).to_ascii_lowercase();
    let head = head.trim_start_matches('\u{feff}').trim_start();
    head.starts_with("<svg")
        || ((head.starts_with("<?xml") || head.starts_with("<!--")) && head.contains("<svg"))
}

/// Drawn at twice its size, within the kept size, so that it stays sharp on
/// dense screens; returned with its own size. Text uses the system fonts.
fn svg(bytes: &[u8]) -> Result<(RgbaImage, (u32, u32)), String> {
    let options = usvg::Options {
        fontdb: fonts(),
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_string: Box::new(|_, _| None),
            ..usvg::ImageHrefResolver::default()
        },
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_data(bytes, &options).map_err(|error| error.to_string())?;
    let size = tree.size();
    let longest = size.width().max(size.height()).max(1.0);
    let scale = (LONGEST_SIDE as f32 / longest).min(2.0);
    let width = ((size.width() * scale).ceil() as u32).max(1);
    let height = ((size.height() * scale).ceil() as u32).max(1);
    let mut pixmap = tiny_skia::Pixmap::new(width, height).ok_or("empty SVG image")?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let mut rgba = RgbaImage::new(width, height);
    for (pixel, color) in rgba.pixels_mut().zip(pixmap.pixels()) {
        let color = color.demultiply();
        *pixel = Rgba([color.red(), color.green(), color.blue(), color.alpha()]);
    }
    let own = (
        (size.width().ceil() as u32).max(1),
        (size.height().ceil() as u32).max(1),
    );
    Ok((rgba, own))
}

fn fonts() -> Arc<usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut fonts = usvg::fontdb::Database::new();
            fonts.load_system_fonts();
            Arc::new(fonts)
        })
        .clone()
}
