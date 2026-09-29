//! Pictures in a terminal pane. Through Herdr, the kitty graphics protocol
//! draws real images, placed with unicode placeholders: each row of an image
//! is a row of text cells, so it scrolls and clips with the README. Without
//! it, half blocks draw two pixels per cell. An animated image is drawn as
//! its first frame, and `animation` plays the others over it.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use image::imageops::FilterType;
use image::{Rgba, RgbaImage};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::adapters::images::{Animation, FRAMES_BUDGET, Picture};

/// `kitty`, `blocks` or `off` forces how images are drawn.
pub const IMAGES_ENV: &str = "HERDR_MARKETPLACE_IMAGES";
/// Asks for the kitty graphics protocol and the cell size in pixels, then
/// for the device attributes, which every terminal answers last.
pub const QUERY: &str = "\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[16t\x1b[c";
/// Images a README may load.
pub const MAX_PICTURES: usize = 16;
/// Bytes the frames of a README's animations may take together.
pub const ANIMATIONS_BUDGET: usize = 2 * FRAMES_BUDGET;
/// Less room than this for frames: the animation stays still.
const MIN_FRAMES_BUDGET: usize = 1024 * 1024;
/// How long a new pane waits for Herdr to give it its size in pixels.
pub const LAYOUT_WAIT: Duration = Duration::from_secs(3);
/// Rows an image may take, so that a tall one does not fill the pane.
const MAX_ROWS: usize = 28;
/// GitHub's README column, in pixels: wider images are shrunk to it.
const COLUMN_PIXELS: u32 = 830;
/// Pixels of a cell for GitHub-sized images.
const CELL_PIXELS: u32 = 8;
const PLACEHOLDER: char = '\u{10EEEE}';

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Graphics {
    /// Kitty graphics protocol; cell size in pixels.
    Kitty { cell_width: u16, cell_height: u16 },
    /// Unicode half blocks.
    Blocks,
    /// Images stay alternative text.
    Off,
}

impl Graphics {
    /// Width over height of a cell.
    fn cell_ratio(self) -> f32 {
        match self {
            Self::Kitty {
                cell_width,
                cell_height,
            } if cell_width > 0 && cell_height > 0 => {
                f32::from(cell_width) / f32::from(cell_height)
            }
            _ => 0.5,
        }
    }
}

/// What a pane learns of its terminal when it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    Known(Graphics),
    /// Herdr draws kitty images, but gives a new pane its size in pixels
    /// only with its first layout: `settle` decides then, or falls back.
    Waiting {
        fallback: Graphics,
    },
}

/// How this pane draws images: `IMAGES_ENV` first, then the terminal's
/// answers.
pub fn probe() -> Probe {
    let forced = std::env::var(IMAGES_ENV).ok();
    match forced.as_deref().map(str::trim) {
        Some("off") => Probe::Known(Graphics::Off),
        Some("blocks") => Probe::Known(Graphics::Blocks),
        _ => from_replies(&ask_terminal(Duration::from_millis(500)), forced.as_deref()),
    }
}

/// What the answers to `QUERY` allow. Herdr answers the graphics query from
/// its own terminal whatever its client, and the cell size only once it
/// knows the pane's pixels, which it gives only to clients it draws images
/// for.
pub fn from_replies(replies: &[u8], forced: Option<&str>) -> Probe {
    let text = String::from_utf8_lossy(replies);
    let kitty = text.contains("\x1b_Gi=31;OK");
    let forced = forced.map(str::trim);
    match (forced, kitty, cell_size(&text)) {
        (Some("off"), _, _) => Probe::Known(Graphics::Off),
        (Some("blocks"), _, _) => Probe::Known(Graphics::Blocks),
        (Some("kitty"), _, Some((cell_width, cell_height)))
        | (_, true, Some((cell_width, cell_height))) => Probe::Known(Graphics::Kitty {
            cell_width,
            cell_height,
        }),
        (Some("kitty"), _, None) => Probe::Waiting {
            fallback: Graphics::Kitty {
                cell_width: 8,
                cell_height: 16,
            },
        },
        (_, true, None) => Probe::Waiting {
            fallback: Graphics::Blocks,
        },
        _ => Probe::Known(Graphics::Blocks),
    }
}

/// Kitty images once the pane has its size in pixels, `columns` × `rows`
/// cells in `width` × `height` pixels; `fallback` after `LAYOUT_WAIT`.
pub fn settle(
    window: Option<(u16, u16, u16, u16)>,
    waited: Duration,
    fallback: Graphics,
) -> Option<Graphics> {
    match window
        .and_then(|(columns, rows, width, height)| cell_from_window(columns, rows, width, height))
    {
        Some((cell_width, cell_height)) => Some(Graphics::Kitty {
            cell_width,
            cell_height,
        }),
        None => (waited >= LAYOUT_WAIT).then_some(fallback),
    }
}

/// Cell size in pixels of a window, when its pixels are known.
pub fn cell_from_window(columns: u16, rows: u16, width: u16, height: u16) -> Option<(u16, u16)> {
    if columns == 0 || rows == 0 {
        return None;
    }
    let cell = (width / columns, height / rows);
    (cell.0 > 0 && cell.1 > 0).then_some(cell)
}

/// `ESC [ 6 ; height ; width t`.
fn cell_size(text: &str) -> Option<(u16, u16)> {
    let rest = &text[text.find("\x1b[6;")? + 4..];
    let (height, rest) = rest.split_once(';')?;
    let width = &rest[..rest.find('t')?];
    let (width, height) = (width.parse().ok()?, height.parse().ok()?);
    (width > 0 && height > 0).then_some((width, height))
}

/// Writes `QUERY` and reads the answers, before any other reader of the
/// input starts.
fn ask_terminal(timeout: Duration) -> Vec<u8> {
    use std::io::Write;
    let mut out = std::io::stdout();
    if out
        .write_all(QUERY.as_bytes())
        .and_then(|()| out.flush())
        .is_err()
    {
        return Vec::new();
    }
    let deadline = Instant::now() + timeout;
    let mut replies = Vec::new();
    let mut buffer = [0u8; 1024];
    while !answered(&replies) {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let mut input = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd, and a buffer of the length given.
        let read = unsafe {
            if libc::poll(&mut input, 1, left.as_millis().min(1000) as i32) <= 0 {
                break;
            }
            libc::read(libc::STDIN_FILENO, buffer.as_mut_ptr().cast(), buffer.len())
        };
        if read <= 0 {
            break;
        }
        replies.extend_from_slice(&buffer[..read as usize]);
    }
    replies
}

/// Whether the device attributes answer, `ESC [ ? … c`, has arrived.
fn answered(replies: &[u8]) -> bool {
    replies.windows(3).enumerate().any(|(start, window)| {
        window == b"\x1b[?"
            && replies[start + 3..]
                .iter()
                .find(|byte| !(byte.is_ascii_digit() || **byte == b';'))
                == Some(&b'c')
    })
}

/// Cells an image of `width` × `height` pixels takes: as wide as GitHub
/// shows it (`hint` from the HTML, at most its README column), within `room`
/// columns and `MAX_ROWS` rows, keeping its proportions.
pub fn fit(
    width: u32,
    height: u32,
    hint: Option<u32>,
    room: usize,
    graphics: Graphics,
) -> (u16, u16) {
    let shown = hint.unwrap_or(width).clamp(1, COLUMN_PIXELS);
    let room = room.clamp(1, DIACRITICS.len());
    let mut columns = (shown.div_ceil(CELL_PIXELS) as usize).clamp(1, room);
    let aspect = height.max(1) as f32 / width.max(1) as f32;
    let rows_for = |columns: usize| {
        (columns as f32 * graphics.cell_ratio() * aspect)
            .round()
            .max(1.0) as usize
    };
    let mut rows = rows_for(columns);
    if rows > MAX_ROWS {
        columns = ((columns * MAX_ROWS) / rows).max(1);
        rows = rows_for(columns).min(MAX_ROWS);
    }
    (columns as u16, rows as u16)
}

/// Sends `png` as image `id`, shown in `columns` × `rows` cells wherever
/// its placeholders are written. The terminal does not answer.
pub fn kitty_transmit(id: u32, png: &[u8], columns: u16, rows: u16) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(png);
    let chunks: Vec<&[u8]> = encoded.as_bytes().chunks(4096).collect();
    let mut out = String::with_capacity(encoded.len() + chunks.len() * 16 + 64);
    for (index, chunk) in chunks.iter().enumerate() {
        let more = u8::from(index + 1 < chunks.len());
        let chunk = std::str::from_utf8(chunk).unwrap_or_default();
        if index == 0 {
            let _ = write!(
                out,
                "\x1b_Ga=T,U=1,f=100,t=d,q=2,i={id},c={columns},r={rows},m={more};{chunk}\x1b\\"
            );
        } else {
            let _ = write!(out, "\x1b_Gq=2,m={more};{chunk}\x1b\\");
        }
    }
    out
}

/// Frees image `id` in the terminal.
pub fn kitty_delete(id: u32) -> String {
    format!("\x1b_Ga=d,d=I,q=2,i={id}\x1b\\")
}

/// `line` with the cells of its kitty image left empty.
pub fn without_image(line: Line<'static>) -> Line<'static> {
    let spans = line
        .spans
        .into_iter()
        .map(|span| {
            if span.content.starts_with(PLACEHOLDER) {
                let cells = span.content.chars().filter(|ch| *ch == PLACEHOLDER).count();
                Span::raw(" ".repeat(cells))
            } else {
                span
            }
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}

/// Row `row` of image `id`, `columns` cells wide: the first cell names the
/// row and column, the next ones follow it.
pub fn kitty_row(id: u32, row: u16, columns: u16) -> Line<'static> {
    let mut text = String::with_capacity(columns as usize * 4 + 8);
    text.push(PLACEHOLDER);
    text.push(DIACRITICS[usize::from(row).min(DIACRITICS.len() - 1)]);
    text.push(DIACRITICS[0]);
    for _ in 1..columns {
        text.push(PLACEHOLDER);
    }
    let [_, red, green, blue] = id.to_be_bytes();
    Line::from(Span::styled(
        text,
        Style::default().fg(Color::Rgb(red, green, blue)),
    ))
}

/// Image id of `url` shown in `columns` × `rows` cells: 24 bits, never 0.
pub fn image_id(url: &str, columns: u16, rows: u16) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in url
        .bytes()
        .chain(columns.to_be_bytes())
        .chain(rows.to_be_bytes())
    {
        hash = (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193);
    }
    ((hash ^ (hash >> 24)) & 0x00ff_ffff).max(1)
}

/// `columns` × `rows` cells of half blocks: the upper half of a cell is one
/// pixel, the lower half the next. Transparent pixels show the pane.
pub fn block_lines(rgba: &RgbaImage, columns: u16, rows: u16) -> Vec<Line<'static>> {
    let small = image::imageops::resize(
        rgba,
        u32::from(columns),
        u32::from(rows) * 2,
        FilterType::Triangle,
    );
    (0..u32::from(rows))
        .map(|row| {
            Line::from(
                (0..u32::from(columns))
                    .map(|column| {
                        half_block(
                            small.get_pixel(column, row * 2),
                            small.get_pixel(column, row * 2 + 1),
                        )
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

fn half_block(top: &Rgba<u8>, bottom: &Rgba<u8>) -> Span<'static> {
    let shown = |pixel: &Rgba<u8>| pixel[3] >= 128;
    let color = |pixel: &Rgba<u8>| Color::Rgb(pixel[0], pixel[1], pixel[2]);
    match (shown(top), shown(bottom)) {
        (true, true) => Span::styled("▀", Style::default().fg(color(top)).bg(color(bottom))),
        (true, false) => Span::styled("▀", Style::default().fg(color(top))),
        (false, true) => Span::styled("▄", Style::default().fg(color(bottom))),
        (false, false) => Span::raw(" "),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PictureState {
    Loading,
    Failed(String),
    Ready(Arc<Picture>),
}

/// The frames of an animated picture, made for cells of `fit` pixels.
#[derive(Debug, Clone, PartialEq)]
pub enum AnimationState {
    /// Decoding within `budget` bytes.
    Decoding { fit: (u32, u32), budget: usize },
    Ready {
        fit: (u32, u32),
        animation: Arc<Animation>,
    },
    /// It stays still.
    Failed(String),
}

/// Images of a details pane: loaded or not, drawn at the size of the last
/// layout, and the kitty images the terminal holds.
#[derive(Debug, Clone, Default)]
pub struct Pictures {
    /// How images are drawn; `None` until the terminal is known, when they
    /// already download but show their alternative text.
    graphics: Option<Graphics>,
    states: HashMap<String, PictureState>,
    animations: HashMap<String, AnimationState>,
    blocks: HashMap<(String, u16, u16), Vec<Line<'static>>>,
    /// Half-block images of the current layout: url and size.
    drawn: HashSet<(String, u16, u16)>,
    sent: HashSet<u32>,
    /// Kitty images of the current layout: id, url and size.
    wanted: HashMap<u32, (String, u16, u16)>,
}

impl Pictures {
    pub fn new(graphics: Graphics) -> Self {
        Self {
            graphics: Some(graphics),
            ..Self::default()
        }
    }

    pub fn graphics(&self) -> Option<Graphics> {
        self.graphics
    }

    pub fn decide(&mut self, graphics: Graphics) {
        self.graphics = Some(graphics);
    }

    /// Marks `url` as loading; true when its download must start.
    pub fn request(&mut self, url: &str) -> bool {
        if self.graphics == Some(Graphics::Off)
            || self.states.contains_key(url)
            || self.states.len() >= MAX_PICTURES
        {
            return false;
        }
        self.states.insert(url.to_string(), PictureState::Loading);
        true
    }

    pub fn loaded(&mut self, url: &str, picture: Result<Arc<Picture>, String>) {
        let state = match picture {
            Ok(picture) => PictureState::Ready(picture),
            Err(error) => PictureState::Failed(error),
        };
        self.states.insert(url.to_string(), state);
    }

    pub fn state(&self, url: &str) -> Option<&PictureState> {
        self.states.get(url)
    }

    /// Whether `url` is a loaded picture with frames to play.
    pub fn animated(&self, url: &str) -> bool {
        matches!(self.states.get(url), Some(PictureState::Ready(picture)) if picture.animated.is_some())
    }

    /// The file of animated picture `url` and the bytes its frames may take,
    /// when they must decode for cells of `fit` pixels: once per size of
    /// the cells, and one decoding at a time, so that resizing the pane does
    /// not pile them up; the last size decodes once the one under way ends.
    /// `None` otherwise, or when the animations of the README took the room.
    pub fn frames_to_decode(&mut self, url: &str, fit: (u32, u32)) -> Option<(Arc<[u8]>, usize)> {
        match self.animations.get(url) {
            Some(AnimationState::Failed(_) | AnimationState::Decoding { .. }) => return None,
            Some(AnimationState::Ready { fit: done, .. }) if *done == fit => return None,
            _ => {}
        }
        let Some(PictureState::Ready(picture)) = self.states.get(url) else {
            return None;
        };
        let file = picture.animated.clone()?;
        let taken: usize = self
            .animations
            .iter()
            .filter(|(other, _)| *other != url)
            .map(|(_, state)| match state {
                AnimationState::Decoding { budget, .. } => *budget,
                AnimationState::Ready { animation, .. } => animation.size(),
                AnimationState::Failed(_) => 0,
            })
            .sum();
        let budget = ANIMATIONS_BUDGET.saturating_sub(taken).min(FRAMES_BUDGET);
        if budget < MIN_FRAMES_BUDGET {
            self.animations.insert(
                url.to_string(),
                AnimationState::Failed("no room left for frames".into()),
            );
            return None;
        }
        self.animations
            .insert(url.to_string(), AnimationState::Decoding { fit, budget });
        Some((file, budget))
    }

    /// Frames of `url` decoded for cells of `fit` pixels; those of an
    /// earlier size are dropped.
    pub fn frames_decoded(
        &mut self,
        url: &str,
        fit: (u32, u32),
        frames: Result<Arc<Animation>, String>,
    ) {
        let current = matches!(
            self.animations.get(url),
            Some(AnimationState::Decoding { fit: asked, .. }) if *asked == fit
        );
        if !current {
            return;
        }
        let state = match frames {
            Ok(animation) => AnimationState::Ready { fit, animation },
            Err(error) => AnimationState::Failed(error),
        };
        self.animations.insert(url.to_string(), state);
    }

    /// The frames of `url` for cells of `fit` pixels, once decoded.
    pub fn animation(&self, url: &str, fit: (u32, u32)) -> Option<&Arc<Animation>> {
        match self.animations.get(url) {
            Some(AnimationState::Ready {
                fit: made,
                animation,
            }) if *made == fit => Some(animation),
            _ => None,
        }
    }

    /// A new layout starts: the images it does not draw again are freed.
    /// Half blocks keep the sizes of the layout before, for this one.
    pub fn begin_layout(&mut self) {
        self.wanted.clear();
        let drawn = std::mem::take(&mut self.drawn);
        self.blocks.retain(|key, _| drawn.contains(key));
    }

    /// Lines drawing `url` within `room` columns; `None` while it loads,
    /// when it failed, or when images are off.
    pub fn lines(
        &mut self,
        url: &str,
        hint: Option<u32>,
        room: usize,
    ) -> Option<Vec<Line<'static>>> {
        let graphics = self.graphics?;
        let Some(PictureState::Ready(picture)) = self.states.get(url) else {
            return None;
        };
        let (columns, rows) = fit(picture.width, picture.height, hint, room, graphics);
        match graphics {
            Graphics::Off => None,
            Graphics::Kitty { .. } => {
                let id = image_id(url, columns, rows);
                self.wanted.insert(id, (url.to_string(), columns, rows));
                Some((0..rows).map(|row| kitty_row(id, row, columns)).collect())
            }
            Graphics::Blocks => {
                let key = (url.to_string(), columns, rows);
                if !self.blocks.contains_key(&key) {
                    let lines = block_lines(&picture.rgba, columns, rows);
                    self.blocks.insert(key.clone(), lines);
                }
                self.drawn.insert(key.clone());
                self.blocks.get(&key).cloned()
            }
        }
    }

    /// What to write before drawing: the new images of the layout, and the
    /// deletion of those it no longer draws.
    pub fn take_commands(&mut self) -> String {
        let mut out = String::new();
        let stale: Vec<u32> = self
            .sent
            .iter()
            .filter(|id| !self.wanted.contains_key(id))
            .copied()
            .collect();
        for id in stale {
            out.push_str(&kitty_delete(id));
            self.sent.remove(&id);
        }
        for (id, (url, columns, rows)) in &self.wanted {
            if self.sent.contains(id) {
                continue;
            }
            if let Some(PictureState::Ready(picture)) = self.states.get(url) {
                out.push_str(&kitty_transmit(*id, &picture.png, *columns, *rows));
                self.sent.insert(*id);
            }
        }
        out
    }
}

/// Row and column diacritics of the kitty graphics protocol, in order:
/// <https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders>.
static DIACRITICS: [char; 297] = [
    '\u{305}',
    '\u{30D}',
    '\u{30E}',
    '\u{310}',
    '\u{312}',
    '\u{33D}',
    '\u{33E}',
    '\u{33F}',
    '\u{346}',
    '\u{34A}',
    '\u{34B}',
    '\u{34C}',
    '\u{350}',
    '\u{351}',
    '\u{352}',
    '\u{357}',
    '\u{35B}',
    '\u{363}',
    '\u{364}',
    '\u{365}',
    '\u{366}',
    '\u{367}',
    '\u{368}',
    '\u{369}',
    '\u{36A}',
    '\u{36B}',
    '\u{36C}',
    '\u{36D}',
    '\u{36E}',
    '\u{36F}',
    '\u{483}',
    '\u{484}',
    '\u{485}',
    '\u{486}',
    '\u{487}',
    '\u{592}',
    '\u{593}',
    '\u{594}',
    '\u{595}',
    '\u{597}',
    '\u{598}',
    '\u{599}',
    '\u{59C}',
    '\u{59D}',
    '\u{59E}',
    '\u{59F}',
    '\u{5A0}',
    '\u{5A1}',
    '\u{5A8}',
    '\u{5A9}',
    '\u{5AB}',
    '\u{5AC}',
    '\u{5AF}',
    '\u{5C4}',
    '\u{610}',
    '\u{611}',
    '\u{612}',
    '\u{613}',
    '\u{614}',
    '\u{615}',
    '\u{616}',
    '\u{617}',
    '\u{657}',
    '\u{658}',
    '\u{659}',
    '\u{65A}',
    '\u{65B}',
    '\u{65D}',
    '\u{65E}',
    '\u{6D6}',
    '\u{6D7}',
    '\u{6D8}',
    '\u{6D9}',
    '\u{6DA}',
    '\u{6DB}',
    '\u{6DC}',
    '\u{6DF}',
    '\u{6E0}',
    '\u{6E1}',
    '\u{6E2}',
    '\u{6E4}',
    '\u{6E7}',
    '\u{6E8}',
    '\u{6EB}',
    '\u{6EC}',
    '\u{730}',
    '\u{732}',
    '\u{733}',
    '\u{735}',
    '\u{736}',
    '\u{73A}',
    '\u{73D}',
    '\u{73F}',
    '\u{740}',
    '\u{741}',
    '\u{743}',
    '\u{745}',
    '\u{747}',
    '\u{749}',
    '\u{74A}',
    '\u{7EB}',
    '\u{7EC}',
    '\u{7ED}',
    '\u{7EE}',
    '\u{7EF}',
    '\u{7F0}',
    '\u{7F1}',
    '\u{7F3}',
    '\u{816}',
    '\u{817}',
    '\u{818}',
    '\u{819}',
    '\u{81B}',
    '\u{81C}',
    '\u{81D}',
    '\u{81E}',
    '\u{81F}',
    '\u{820}',
    '\u{821}',
    '\u{822}',
    '\u{823}',
    '\u{825}',
    '\u{826}',
    '\u{827}',
    '\u{829}',
    '\u{82A}',
    '\u{82B}',
    '\u{82C}',
    '\u{82D}',
    '\u{951}',
    '\u{953}',
    '\u{954}',
    '\u{F82}',
    '\u{F83}',
    '\u{F86}',
    '\u{F87}',
    '\u{135D}',
    '\u{135E}',
    '\u{135F}',
    '\u{17DD}',
    '\u{193A}',
    '\u{1A17}',
    '\u{1A75}',
    '\u{1A76}',
    '\u{1A77}',
    '\u{1A78}',
    '\u{1A79}',
    '\u{1A7A}',
    '\u{1A7B}',
    '\u{1A7C}',
    '\u{1B6B}',
    '\u{1B6D}',
    '\u{1B6E}',
    '\u{1B6F}',
    '\u{1B70}',
    '\u{1B71}',
    '\u{1B72}',
    '\u{1B73}',
    '\u{1CD0}',
    '\u{1CD1}',
    '\u{1CD2}',
    '\u{1CDA}',
    '\u{1CDB}',
    '\u{1CE0}',
    '\u{1DC0}',
    '\u{1DC1}',
    '\u{1DC3}',
    '\u{1DC4}',
    '\u{1DC5}',
    '\u{1DC6}',
    '\u{1DC7}',
    '\u{1DC8}',
    '\u{1DC9}',
    '\u{1DCB}',
    '\u{1DCC}',
    '\u{1DD1}',
    '\u{1DD2}',
    '\u{1DD3}',
    '\u{1DD4}',
    '\u{1DD5}',
    '\u{1DD6}',
    '\u{1DD7}',
    '\u{1DD8}',
    '\u{1DD9}',
    '\u{1DDA}',
    '\u{1DDB}',
    '\u{1DDC}',
    '\u{1DDD}',
    '\u{1DDE}',
    '\u{1DDF}',
    '\u{1DE0}',
    '\u{1DE1}',
    '\u{1DE2}',
    '\u{1DE3}',
    '\u{1DE4}',
    '\u{1DE5}',
    '\u{1DE6}',
    '\u{1DFE}',
    '\u{20D0}',
    '\u{20D1}',
    '\u{20D4}',
    '\u{20D5}',
    '\u{20D6}',
    '\u{20D7}',
    '\u{20DB}',
    '\u{20DC}',
    '\u{20E1}',
    '\u{20E7}',
    '\u{20E9}',
    '\u{20F0}',
    '\u{2CEF}',
    '\u{2CF0}',
    '\u{2CF1}',
    '\u{2DE0}',
    '\u{2DE1}',
    '\u{2DE2}',
    '\u{2DE3}',
    '\u{2DE4}',
    '\u{2DE5}',
    '\u{2DE6}',
    '\u{2DE7}',
    '\u{2DE8}',
    '\u{2DE9}',
    '\u{2DEA}',
    '\u{2DEB}',
    '\u{2DEC}',
    '\u{2DED}',
    '\u{2DEE}',
    '\u{2DEF}',
    '\u{2DF0}',
    '\u{2DF1}',
    '\u{2DF2}',
    '\u{2DF3}',
    '\u{2DF4}',
    '\u{2DF5}',
    '\u{2DF6}',
    '\u{2DF7}',
    '\u{2DF8}',
    '\u{2DF9}',
    '\u{2DFA}',
    '\u{2DFB}',
    '\u{2DFC}',
    '\u{2DFD}',
    '\u{2DFE}',
    '\u{2DFF}',
    '\u{A66F}',
    '\u{A67C}',
    '\u{A67D}',
    '\u{A6F0}',
    '\u{A6F1}',
    '\u{A8E0}',
    '\u{A8E1}',
    '\u{A8E2}',
    '\u{A8E3}',
    '\u{A8E4}',
    '\u{A8E5}',
    '\u{A8E6}',
    '\u{A8E7}',
    '\u{A8E8}',
    '\u{A8E9}',
    '\u{A8EA}',
    '\u{A8EB}',
    '\u{A8EC}',
    '\u{A8ED}',
    '\u{A8EE}',
    '\u{A8EF}',
    '\u{A8F0}',
    '\u{A8F1}',
    '\u{AAB0}',
    '\u{AAB2}',
    '\u{AAB3}',
    '\u{AAB7}',
    '\u{AAB8}',
    '\u{AABE}',
    '\u{AABF}',
    '\u{AAC1}',
    '\u{FE20}',
    '\u{FE21}',
    '\u{FE22}',
    '\u{FE23}',
    '\u{FE24}',
    '\u{FE25}',
    '\u{FE26}',
    '\u{10A0F}',
    '\u{10A38}',
    '\u{1D185}',
    '\u{1D186}',
    '\u{1D187}',
    '\u{1D188}',
    '\u{1D189}',
    '\u{1D1AA}',
    '\u{1D1AB}',
    '\u{1D1AC}',
    '\u{1D1AD}',
    '\u{1D242}',
    '\u{1D243}',
    '\u{1D244}',
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::images::decode;

    #[test]
    fn half_blocks_keep_only_the_sizes_of_the_last_layouts() {
        let mut png = Vec::new();
        RgbaImage::from_pixel(800, 80, Rgba([200, 30, 30, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let url = "https://example.com/logo.png";
        let mut pictures = Pictures::new(Graphics::Blocks);
        pictures.loaded(url, Ok(Arc::new(decode(&png).unwrap())));
        // 100 columns wide at most: each room draws another size.
        for room in [40, 30, 20, 10] {
            pictures.begin_layout();
            assert_eq!(pictures.lines(url, None, room).unwrap()[0].width(), room);
        }
        assert_eq!(
            pictures.blocks.len(),
            2,
            "a pane resized again and again keeps two sizes, not every one"
        );
        pictures.begin_layout();
        pictures.lines(url, None, 10).unwrap();
        pictures.begin_layout();
        assert_eq!(pictures.blocks.len(), 1);
    }
}
