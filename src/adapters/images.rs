//! README images, decoded and kept small enough to draw: PNG, JPEG, GIF
//! (its first frame), WebP and SVG. Only pixels come out; nothing of the file
//! reaches the terminal as is.

use std::io::Cursor;
use std::sync::{Arc, OnceLock};

use crate::application::ports::Fetcher;
use image::imageops::FilterType;
use image::{ImageFormat, ImageReader, Limits, Rgba, RgbaImage};
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

/// A decoded image and the PNG that sends it to the terminal.
#[derive(Debug, Clone, PartialEq)]
pub struct Picture {
    pub rgba: RgbaImage,
    pub png: Vec<u8>,
    /// Size a browser shows, in pixels: the file's own, before shrinking.
    pub width: u32,
    pub height: u32,
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
    // A PNG small enough is sent as it came.
    if format == Some(ImageFormat::Png) && width.max(height) <= LONGEST_SIDE {
        return Ok(Picture {
            rgba,
            png: bytes.to_vec(),
            width,
            height,
        });
    }
    from_rgba(rgba, width, height)
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
    })
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
