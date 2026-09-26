//! README Markdown drawn in a terminal pane the way GitHub shows it, within
//! terminal limits: one font size, colors, box drawing and images. Nothing
//! is executed: control characters are removed before parsing, HTML only
//! keeps its text, links and images, and images arrive as decoded pixels.

mod code;
mod html;

use std::collections::HashMap;

use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use self::html::Token;
use super::graphics::Pictures;
use super::style::{ACCENT, MUTED, OK};
use crate::domain::readme::{LinkTarget, ReadmePlace, absolute_image, absolute_link, anchor};

/// Inline code and image chips: light text on grey, readable on any theme.
const CHIP_FG: Color = Color::Rgb(0xe6, 0xed, 0xf3);
const CHIP_BG: Color = Color::Rgb(0x3d, 0x44, 0x4d);
/// Text of a quote, GitHub's grey.
const QUOTE: Color = Color::Rgb(0x9d, 0xa5, 0xb0);
const LIST_MARKERS: [&str; 3] = ["• ", "◦ ", "▪ "];

/// A README laid out for a width.
#[derive(Debug, Clone, Default)]
pub struct Rendered {
    pub lines: Vec<Line<'static>>,
    /// Parts of `lines` a click opens.
    pub links: Vec<LinkArea>,
    /// First line of each heading, by GitHub anchor.
    pub anchors: Vec<(String, usize)>,
    /// Images the README shows, in order, loaded or not.
    pub images: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkArea {
    pub line: usize,
    /// Columns `start..end` of the line.
    pub start: usize,
    pub end: usize,
    pub target: LinkTarget,
}

/// `markdown` without a README place nor images.
pub fn render(markdown: &str, width: usize) -> Vec<Line<'static>> {
    render_readme(markdown, width, None, &mut Pictures::default()).lines
}

/// `place` resolves relative links and images; without it, only absolute
/// ones work. `pictures` draws the images already loaded, in place of their
/// alternative text.
pub fn render_readme(
    markdown: &str,
    width: usize,
    place: Option<&ReadmePlace>,
    pictures: &mut Pictures,
) -> Rendered {
    let source = without_controls(markdown);
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM;
    let mut renderer = Renderer::new(width.max(10), place, pictures);
    for event in Parser::new_ext(&source, options) {
        renderer.event(event);
    }
    renderer.finish()
}

/// Tabs become spaces and every other control character but the line feed
/// disappears, so no escape sequence of the README reaches the terminal.
fn without_controls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\n' => out.push('\n'),
            '\t' => out.push_str("    "),
            ch if ch.is_control() => {}
            ch => out.push(ch),
        }
    }
    out
}

/// Inline content waiting for its line.
#[derive(Debug, Clone)]
enum Piece {
    Text {
        text: String,
        style: Style,
        link: Option<usize>,
    },
    Image {
        url: Option<String>,
        alt: String,
        /// Width the HTML asks for, in pixels.
        hint: Option<u32>,
        link: Option<usize>,
    },
}

/// Text of a line, and the link it opens.
#[derive(Debug, Clone)]
struct Seg {
    text: String,
    style: Style,
    link: Option<usize>,
}

impl Seg {
    fn new(text: impl Into<String>, style: Style, link: Option<usize>) -> Self {
        Self {
            text: text.into(),
            style,
            link,
        }
    }
}

struct Item {
    marker: String,
    style: Style,
    shown: bool,
}

struct Table {
    alignments: Vec<Alignment>,
    rows: Vec<Vec<Vec<Seg>>>,
    header_rows: usize,
}

struct Renderer<'a> {
    width: usize,
    place: Option<&'a ReadmePlace>,
    pictures: &'a mut Pictures,
    out: Rendered,
    /// Where each link of the README goes; `None` when nowhere.
    targets: Vec<Option<LinkTarget>>,
    pieces: Vec<Piece>,
    /// The last line pushed is a blank one.
    blank: bool,
    strong: usize,
    emphasis: usize,
    strike: usize,
    code: usize,
    heading: Option<HeadingLevel>,
    heading_text: String,
    anchors: HashMap<String, usize>,
    open_links: Vec<usize>,
    /// Source and alternative text of the Markdown image being read.
    image: Option<(String, String)>,
    /// Next number of each open list; `None` for bullets.
    lists: Vec<Option<u64>>,
    items: Vec<Item>,
    quotes: Vec<Option<BlockQuoteKind>>,
    /// Language and text of the code block being read.
    code_block: Option<(String, String)>,
    html_block: Option<String>,
    table: Option<Table>,
    /// HTML elements open, and whether they center their content.
    centered: Vec<(String, bool)>,
    /// Dark variant of the `<picture>` being read: terminals are dark.
    picture: Option<String>,
}

impl<'a> Renderer<'a> {
    fn new(width: usize, place: Option<&'a ReadmePlace>, pictures: &'a mut Pictures) -> Self {
        Self {
            width,
            place,
            pictures,
            out: Rendered::default(),
            targets: Vec::new(),
            pieces: Vec::new(),
            blank: true,
            strong: 0,
            emphasis: 0,
            strike: 0,
            code: 0,
            heading: None,
            heading_text: String::new(),
            anchors: HashMap::new(),
            open_links: Vec::new(),
            image: None,
            lists: Vec::new(),
            items: Vec::new(),
            quotes: Vec::new(),
            code_block: None,
            html_block: None,
            table: None,
            centered: Vec::new(),
            picture: None,
        }
    }

    fn event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text),
            Event::Code(code) => {
                self.code += 1;
                self.text(&code);
                self.code -= 1;
            }
            Event::Html(html) => match &mut self.html_block {
                Some(block) => block.push_str(&html),
                None => self.html(&html),
            },
            Event::InlineHtml(html) => self.html(&html),
            Event::SoftBreak => self.text(" "),
            Event::HardBreak if self.table.is_some() => self.text(" "),
            Event::HardBreak => self.flush(),
            Event::Rule => {
                self.flush();
                self.blank();
                self.rule('─');
                self.blank();
            }
            Event::TaskListMarker(done) => {
                if let Some(item) = self.items.last_mut() {
                    (item.marker, item.style) = if done {
                        ("☑ ".to_string(), Style::default().fg(OK))
                    } else {
                        ("☐ ".to_string(), Style::default().fg(MUTED))
                    };
                }
            }
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {}
            Tag::Heading { level, .. } => self.start_heading(level),
            Tag::BlockQuote(kind) => {
                self.flush();
                self.blank();
                self.quotes.push(kind);
                if let Some(kind) = kind {
                    let (title, color) = alert(kind);
                    let mut spans = self.prefix();
                    spans.push(Span::styled(
                        title,
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ));
                    self.push(Line::from(spans));
                }
            }
            Tag::CodeBlock(kind) => {
                self.flush();
                self.blank();
                let language = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split(|ch: char| ch.is_whitespace() || ch == ',' || ch == '{')
                        .find(|word| !word.is_empty())
                        .unwrap_or("")
                        .to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code_block = Some((language, String::new()));
            }
            Tag::HtmlBlock => {
                self.flush();
                self.html_block = Some(String::new());
            }
            Tag::List(start) => {
                self.flush();
                if self.items.is_empty() {
                    self.blank();
                }
                self.lists.push(start);
            }
            Tag::Item => {
                self.flush();
                let depth = self.lists.len().saturating_sub(1);
                let marker = match self.lists.last_mut() {
                    Some(Some(number)) => {
                        *number += 1;
                        format!("{}. ", *number - 1)
                    }
                    _ => LIST_MARKERS[depth % LIST_MARKERS.len()].to_string(),
                };
                self.items.push(Item {
                    marker,
                    style: Style::default().fg(MUTED),
                    shown: false,
                });
            }
            Tag::Table(alignments) => {
                self.flush();
                self.blank();
                self.table = Some(Table {
                    alignments,
                    rows: Vec::new(),
                    header_rows: 0,
                });
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(table) = &mut self.table {
                    table.rows.push(Vec::new());
                }
            }
            Tag::Emphasis => self.emphasis += 1,
            Tag::Strong => self.strong += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } => self.open_link(&dest_url),
            Tag::Image { dest_url, .. } => self.image = Some((dest_url.to_string(), String::new())),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush();
                if self.items.is_empty() {
                    self.blank();
                }
            }
            TagEnd::Heading(level) => self.end_heading(level),
            TagEnd::BlockQuote(_) => {
                self.flush();
                // The quote ends on its text, not on an empty barred line.
                if self.blank && !self.out.lines.is_empty() {
                    self.out.lines.pop();
                    self.blank = false;
                }
                self.quotes.pop();
                self.blank();
            }
            TagEnd::CodeBlock => {
                if let Some((language, text)) = self.code_block.take() {
                    self.code_lines(&text, &language);
                }
                self.blank();
            }
            TagEnd::HtmlBlock => {
                if let Some(html) = self.html_block.take() {
                    self.html(&html);
                }
                self.flush();
                self.blank();
            }
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
                if self.items.is_empty() {
                    self.blank();
                }
            }
            TagEnd::Item => {
                self.flush();
                self.items.pop();
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.table_lines(table);
                }
                self.blank();
            }
            TagEnd::TableHead => {
                if let Some(table) = &mut self.table {
                    table.header_rows = table.rows.len();
                }
            }
            TagEnd::TableCell => {
                let pieces = std::mem::take(&mut self.pieces);
                let cell = self.segments(pieces);
                if let Some(row) = self.table.as_mut().and_then(|table| table.rows.last_mut()) {
                    row.push(cell);
                }
            }
            TagEnd::Emphasis => self.emphasis = self.emphasis.saturating_sub(1),
            TagEnd::Strong => self.strong = self.strong.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => {
                self.open_links.pop();
            }
            TagEnd::Image => {
                if let Some((src, alt)) = self.image.take() {
                    self.push_image(&src, alt, None);
                }
            }
            _ => {}
        }
    }

    fn text(&mut self, text: &str) {
        if let Some((_, code)) = &mut self.code_block {
            code.push_str(text);
            return;
        }
        if let Some((_, alt)) = &mut self.image {
            alt.push_str(text);
            return;
        }
        if self.heading.is_some() {
            self.heading_text.push_str(text);
        }
        let style = self.style();
        let link = self.open_links.last().copied();
        self.pieces.push(Piece::Text {
            text: text.to_string(),
            style,
            link,
        });
    }

    fn style(&self) -> Style {
        let mut style = Style::default();
        match self.heading {
            Some(HeadingLevel::H1 | HeadingLevel::H2 | HeadingLevel::H3) => {
                style = style.fg(ACCENT).add_modifier(Modifier::BOLD);
            }
            Some(HeadingLevel::H6) => style = style.fg(MUTED).add_modifier(Modifier::BOLD),
            Some(_) => style = style.add_modifier(Modifier::BOLD),
            None => {}
        }
        if self.strong > 0 {
            style = style.add_modifier(Modifier::BOLD);
        }
        if self.emphasis > 0 {
            style = style.add_modifier(Modifier::ITALIC);
        }
        if self.strike > 0 {
            style = style.add_modifier(Modifier::CROSSED_OUT);
        }
        if self.code > 0 {
            style = style.fg(CHIP_FG).bg(CHIP_BG);
        }
        let linked = self
            .open_links
            .last()
            .is_some_and(|&link| self.targets.get(link).is_some_and(Option::is_some));
        if linked {
            style = style.fg(ACCENT).add_modifier(Modifier::UNDERLINED);
        }
        if style.fg.is_none() && self.quotes.iter().any(Option::is_none) {
            style = style.fg(QUOTE);
        }
        style
    }

    fn open_link(&mut self, href: &str) {
        let target = match self.place {
            Some(place) => place.link(href),
            None => absolute_link(href),
        };
        self.targets.push(target);
        self.open_links.push(self.targets.len() - 1);
    }

    fn push_image(&mut self, src: &str, alt: String, hint: Option<u32>) {
        let url = match self.place {
            Some(place) => place.image(src),
            None => absolute_image(src),
        };
        let link = self.open_links.last().copied();
        self.pieces.push(Piece::Image {
            url,
            alt: alt.trim().to_string(),
            hint,
            link,
        });
    }

    fn start_heading(&mut self, level: HeadingLevel) {
        self.flush();
        self.blank();
        self.heading = Some(level);
        self.heading_text.clear();
    }

    /// GitHub draws a line under the first two levels.
    fn end_heading(&mut self, level: HeadingLevel) {
        let first = self.out.lines.len();
        self.flush();
        let slug = anchor(&self.heading_text);
        let seen = self.anchors.entry(slug.clone()).or_insert(0);
        let slug = if *seen == 0 {
            slug
        } else {
            format!("{slug}-{seen}")
        };
        *seen += 1;
        self.out.anchors.push((slug, first));
        self.heading = None;
        match level {
            HeadingLevel::H1 => self.rule('━'),
            HeadingLevel::H2 => self.rule('─'),
            _ => {}
        }
        self.blank();
    }

    fn html(&mut self, html: &str) {
        for token in html::tokens(html) {
            match token {
                Token::Text(text) => {
                    let text = collapse_spaces(&text);
                    if !text.is_empty() {
                        self.text(&text);
                    }
                }
                Token::Open { name, attributes } => self.html_open(&name, &attributes),
                Token::Close { name } => self.html_close(&name),
            }
        }
    }

    fn html_open(&mut self, name: &str, attributes: &str) {
        match name {
            "img" => {
                let src = self
                    .picture
                    .clone()
                    .or_else(|| html::attribute(attributes, "src"))
                    .unwrap_or_default();
                let alt = html::attribute(attributes, "alt").unwrap_or_default();
                let hint =
                    html::attribute(attributes, "width").and_then(|width| width_hint(&width));
                self.push_image(&src, alt, hint);
            }
            "picture" => self.picture = None,
            "source" => {
                let dark = html::attribute(attributes, "media")
                    .is_some_and(|media| media.contains("dark"));
                if dark {
                    self.picture = html::attribute(attributes, "srcset")
                        .and_then(|set| set.split_whitespace().next().map(str::to_string));
                }
            }
            "a" => {
                let href = html::attribute(attributes, "href").unwrap_or_default();
                self.open_link(&href);
            }
            "video" => {
                if let Some(src) = html::attribute(attributes, "src") {
                    self.open_link(&src);
                    let link = self.open_links.last().copied();
                    self.pieces.push(Piece::Text {
                        text: "\u{a0}▶\u{a0}video\u{a0}".into(),
                        style: Style::default().fg(CHIP_FG).bg(CHIP_BG),
                        link,
                    });
                    self.open_links.pop();
                }
            }
            "br" => self.flush(),
            "p" | "div" | "center" => {
                self.flush();
                let centered = name == "center"
                    || html::attribute(attributes, "align")
                        .is_some_and(|align| align.eq_ignore_ascii_case("center"));
                self.centered.push((name.to_string(), centered));
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => self.start_heading(heading_level(name)),
            "b" | "strong" => self.strong += 1,
            "i" | "em" => self.emphasis += 1,
            "s" | "del" | "strike" => self.strike += 1,
            "code" | "kbd" | "tt" | "samp" => self.code += 1,
            "summary" => {
                self.flush();
                self.pieces.push(Piece::Text {
                    text: "▸ ".into(),
                    style: Style::default().fg(MUTED),
                    link: None,
                });
                self.strong += 1;
            }
            "li" => {
                self.flush();
                self.text("• ");
            }
            "hr" => {
                self.flush();
                self.rule('─');
            }
            "tr" => self.flush(),
            _ => {}
        }
    }

    fn html_close(&mut self, name: &str) {
        match name {
            "a" => {
                self.open_links.pop();
            }
            "picture" => self.picture = None,
            "p" | "div" | "center" => {
                self.flush();
                if let Some(open) = self.centered.iter().rposition(|(open, _)| open == name) {
                    self.centered.truncate(open);
                }
                self.blank();
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => self.end_heading(heading_level(name)),
            "b" | "strong" => self.strong = self.strong.saturating_sub(1),
            "i" | "em" => self.emphasis = self.emphasis.saturating_sub(1),
            "s" | "del" | "strike" => self.strike = self.strike.saturating_sub(1),
            "code" | "kbd" | "tt" | "samp" => self.code = self.code.saturating_sub(1),
            "summary" => {
                self.strong = self.strong.saturating_sub(1);
                self.flush();
            }
            "details" | "table" => {
                self.flush();
                self.blank();
            }
            "li" | "tr" | "ul" | "ol" => self.flush(),
            "td" | "th" => self.text(" "),
            _ => {}
        }
    }

    fn centered(&self) -> bool {
        self.centered.iter().any(|(_, centered)| *centered)
    }

    /// Prefix of a block line: quote bars, then list indentation or marker.
    fn prefix(&mut self) -> Vec<Span<'static>> {
        let mut prefix = self.quote_bars();
        let depth = self.items.len();
        for (index, item) in self.items.iter_mut().enumerate() {
            if index + 1 == depth && !item.shown {
                item.shown = true;
                prefix.push(Span::styled(item.marker.clone(), item.style));
            } else {
                prefix.push(Span::raw(" ".repeat(item.marker.width())));
            }
        }
        prefix
    }

    fn quote_bars(&self) -> Vec<Span<'static>> {
        self.quotes
            .iter()
            .map(|kind| {
                let color = kind.map_or(MUTED, |kind| alert(kind).1);
                Span::styled("▎ ", Style::default().fg(color))
            })
            .collect()
    }

    fn prefix_width(&self) -> usize {
        self.quotes.len() * 2
            + self
                .items
                .iter()
                .map(|item| item.marker.width())
                .sum::<usize>()
    }

    /// Columns left after the prefix.
    fn room(&self) -> usize {
        self.width.saturating_sub(self.prefix_width()).max(1)
    }

    fn push(&mut self, line: Line<'static>) {
        self.out.lines.push(line);
        self.blank = false;
    }

    /// One blank line between blocks; quote bars stay continuous.
    fn blank(&mut self) {
        if !self.blank {
            let bars = self.quote_bars();
            self.out.lines.push(Line::from(bars));
            self.blank = true;
        }
    }

    fn rule(&mut self, ch: char) {
        let room = self.room();
        let mut spans = self.prefix();
        spans.push(Span::styled(
            ch.to_string().repeat(room),
            Style::default().fg(MUTED),
        ));
        self.push(Line::from(spans));
    }

    fn area(&mut self, line: usize, start: usize, end: usize, link: Option<usize>) {
        let Some(target) = link.and_then(|link| self.targets.get(link).cloned().flatten()) else {
            return;
        };
        if let Some(last) = self.out.links.last_mut()
            && last.line == line
            && last.end == start
            && last.target == target
        {
            last.end = end;
            return;
        }
        self.out.links.push(LinkArea {
            line,
            start,
            end,
            target,
        });
    }

    /// Lays out the pending inline content. A paragraph of images only draws
    /// them; badges, and images among text, become small labels.
    fn flush(&mut self) {
        let pieces = std::mem::take(&mut self.pieces);
        if pieces.is_empty() {
            return;
        }
        let pictures_only = pieces.iter().all(|piece| match piece {
            Piece::Image { .. } => true,
            Piece::Text { text, .. } => text.trim().is_empty(),
        }) && pieces.iter().any(|piece| {
            matches!(piece, Piece::Image { url, .. } if !url.as_deref().is_some_and(is_badge))
        });
        if !pictures_only {
            let segments = self.segments(pieces);
            self.push_text(segments);
            return;
        }
        let mut labels = Vec::new();
        for piece in pieces {
            if let Piece::Image {
                url,
                alt,
                hint,
                link,
            } = &piece
                && !url.as_deref().is_some_and(is_badge)
            {
                let before = self.segments(std::mem::take(&mut labels));
                self.push_text(before);
                self.picture(url.clone(), alt, *hint, *link);
            } else {
                labels.push(piece);
            }
        }
        let after = self.segments(labels);
        self.push_text(after);
    }

    /// Text segments of inline content: images become labels.
    fn segments(&self, pieces: Vec<Piece>) -> Vec<Seg> {
        let mut segments = Vec::new();
        for piece in pieces {
            match piece {
                Piece::Text { text, style, link } => segments.push(Seg::new(text, style, link)),
                Piece::Image { url, alt, link, .. } => {
                    let badge = url.as_deref().is_some_and(is_badge);
                    if alt.is_empty() && badge {
                        continue;
                    }
                    let alt = if alt.is_empty() {
                        "image".to_string()
                    } else {
                        alt
                    };
                    let label = format!("\u{a0}{}\u{a0}", alt.replace(' ', "\u{a0}"));
                    let mut style = Style::default().fg(CHIP_FG).bg(CHIP_BG);
                    if link.is_some_and(|link| self.targets.get(link).is_some_and(Option::is_some))
                    {
                        style = style.add_modifier(Modifier::UNDERLINED);
                    }
                    segments.push(Seg::new(label, style, link));
                    segments.push(Seg::new(" ", Style::default(), None));
                }
            }
        }
        segments
    }

    fn push_text(&mut self, segments: Vec<Seg>) {
        if segments
            .iter()
            .all(|segment| segment.text.trim().is_empty())
        {
            return;
        }
        let room = self.room();
        for line in wrap(segments, room) {
            let width: usize = line.iter().map(|segment| segment.text.width()).sum();
            let pad = if self.centered() {
                room.saturating_sub(width) / 2
            } else {
                0
            };
            self.push_segments(line, pad);
        }
    }

    fn push_segments(&mut self, segments: Vec<Seg>, pad: usize) {
        let line = self.out.lines.len();
        let mut spans = self.prefix();
        let mut column = self.prefix_width() + pad;
        if pad > 0 {
            spans.push(Span::raw(" ".repeat(pad)));
        }
        for segment in segments {
            let width = segment.text.width();
            self.area(line, column, column + width, segment.link);
            column += width;
            spans.push(Span::styled(segment.text, segment.style));
        }
        self.push(Line::from(spans));
    }

    /// The image once loaded, its alternative text before, if it fails or
    /// when it has no address.
    fn picture(&mut self, url: Option<String>, alt: &str, hint: Option<u32>, link: Option<usize>) {
        if let Some(url) = &url
            && !self.out.images.contains(url)
        {
            self.out.images.push(url.clone());
        }
        let room = self.room();
        let lines = url.and_then(|url| self.pictures.lines(&url, hint, room));
        let Some(lines) = lines else {
            let text = if alt.is_empty() {
                "[image]".to_string()
            } else {
                format!("[image: {alt}]")
            };
            let style = Style::default().fg(MUTED);
            self.push_text(vec![Seg::new(text, style, link)]);
            return;
        };
        for image_line in lines {
            let width = image_line.width();
            let pad = if self.centered() {
                room.saturating_sub(width) / 2
            } else {
                0
            };
            let line = self.out.lines.len();
            let start = self.prefix_width() + pad;
            self.area(line, start, start + width, link);
            let mut spans = self.prefix();
            spans.push(Span::raw(" ".repeat(pad)));
            spans.extend(image_line.spans);
            self.push(Line::from(spans));
        }
    }

    /// Colored code on its own background, one cell of padding around it;
    /// long lines are cut.
    fn code_lines(&mut self, text: &str, language: &str) {
        let room = self.room();
        let inner = room.saturating_sub(2).max(1);
        let background = Style::default().bg(code::background());
        self.fill(room, background);
        for pieces in code::highlight(text.trim_end_matches('\n'), language) {
            for chunk in cut_colored(pieces, inner) {
                let used: usize = chunk.iter().map(|(text, _)| text.width()).sum();
                let mut spans = self.prefix();
                spans.push(Span::styled(" ", background));
                for (text, color) in chunk {
                    spans.push(Span::styled(text, background.fg(color)));
                }
                spans.push(Span::styled(
                    " ".repeat(room.saturating_sub(1 + used)),
                    background,
                ));
                self.push(Line::from(spans));
            }
        }
        self.fill(room, background);
    }

    fn fill(&mut self, room: usize, style: Style) {
        let mut spans = self.prefix();
        spans.push(Span::styled(" ".repeat(room), style));
        self.push(Line::from(spans));
    }

    /// A boxed table; cells wrap when the pane is narrow.
    fn table_lines(&mut self, table: Table) {
        let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        if columns == 0 {
            return;
        }
        let border = Style::default().fg(MUTED);
        let mut widths = vec![1; columns];
        for row in &table.rows {
            for (column, cell) in row.iter().enumerate() {
                let width: usize = cell.iter().map(|segment| segment.text.width()).sum();
                widths[column] = widths[column].max(width);
            }
        }
        let available = self.room().saturating_sub(3 * columns + 1).max(columns);
        while widths.iter().sum::<usize>() > available {
            let Some(widest) = widths.iter_mut().filter(|width| **width > 1).max() else {
                break;
            };
            *widest -= 1;
        }
        let edge = |left: &str, middle: &str, right: &str| {
            let parts: Vec<String> = widths.iter().map(|width| "─".repeat(width + 2)).collect();
            format!("{left}{}{right}", parts.join(middle))
        };
        let (top, separator, bottom) = (
            edge("┌", "┬", "┐"),
            edge("├", "┼", "┤"),
            edge("└", "┴", "┘"),
        );
        let mut spans = self.prefix();
        spans.push(Span::styled(top, border));
        self.push(Line::from(spans));
        for (index, row) in table.rows.iter().enumerate() {
            let header = index < table.header_rows;
            let cells: Vec<Vec<Vec<Seg>>> = (0..columns)
                .map(|column| wrap(row.get(column).cloned().unwrap_or_default(), widths[column]))
                .collect();
            let height = cells.iter().map(Vec::len).max().unwrap_or(1).max(1);
            for part in 0..height {
                let line = self.out.lines.len();
                let mut spans = self.prefix();
                let mut column = self.prefix_width() + 2;
                spans.push(Span::styled("│ ", border));
                for (index, cell) in cells.iter().enumerate() {
                    let segments = cell.get(part).cloned().unwrap_or_default();
                    let used: usize = segments.iter().map(|segment| segment.text.width()).sum();
                    let pad = widths[index].saturating_sub(used);
                    let (left, right) = match table.alignments.get(index) {
                        Some(Alignment::Right) => (pad, 0),
                        Some(Alignment::Center) => (pad / 2, pad - pad / 2),
                        _ => (0, pad),
                    };
                    spans.push(Span::raw(" ".repeat(left)));
                    column += left;
                    for segment in segments {
                        let width = segment.text.width();
                        self.area(line, column, column + width, segment.link);
                        column += width;
                        let style = if header {
                            segment.style.add_modifier(Modifier::BOLD)
                        } else {
                            segment.style
                        };
                        spans.push(Span::styled(segment.text, style));
                    }
                    spans.push(Span::raw(" ".repeat(right)));
                    let closing = if index + 1 < columns { " │ " } else { " │" };
                    spans.push(Span::styled(closing, border));
                    column += right + closing.width();
                }
                self.push(Line::from(spans));
            }
            if header && index + 1 == table.header_rows {
                let mut spans = self.prefix();
                spans.push(Span::styled(separator.clone(), border));
                self.push(Line::from(spans));
            }
        }
        let mut spans = self.prefix();
        spans.push(Span::styled(bottom, border));
        self.push(Line::from(spans));
    }

    fn finish(mut self) -> Rendered {
        self.flush();
        while self.out.lines.last().is_some_and(|line| {
            line.spans
                .iter()
                .all(|span| span.content.trim().is_empty() && span.style.bg.is_none())
        }) {
            self.out.lines.pop();
        }
        // Lines are split on line feeds above; this keeps the promise that no
        // control character of the README is ever drawn.
        for span in self
            .out
            .lines
            .iter_mut()
            .flat_map(|line| line.spans.iter_mut())
        {
            if span.content.chars().any(char::is_control) {
                span.content = span.content.replace(char::is_control, " ").into();
            }
        }
        self.out
    }
}

fn alert(kind: BlockQuoteKind) -> (&'static str, Color) {
    match kind {
        BlockQuoteKind::Note => ("Note", Color::Rgb(0x4f, 0x9c, 0xf9)),
        BlockQuoteKind::Tip => ("Tip", Color::Rgb(0x3f, 0xb9, 0x50)),
        BlockQuoteKind::Important => ("Important", Color::Rgb(0xab, 0x7d, 0xf8)),
        BlockQuoteKind::Warning => ("Warning", Color::Rgb(0xd2, 0x99, 0x22)),
        BlockQuoteKind::Caution => ("Caution", Color::Rgb(0xf8, 0x51, 0x49)),
    }
}

fn heading_level(name: &str) -> HeadingLevel {
    match name {
        "h1" => HeadingLevel::H1,
        "h2" => HeadingLevel::H2,
        "h3" => HeadingLevel::H3,
        "h4" => HeadingLevel::H4,
        "h5" => HeadingLevel::H5,
        _ => HeadingLevel::H6,
    }
}

/// Width an HTML `width` asks for, in pixels; a percentage of GitHub's
/// README column.
fn width_hint(width: &str) -> Option<u32> {
    let width = width.trim();
    if let Some(percent) = width.strip_suffix('%') {
        return percent
            .trim()
            .parse::<u32>()
            .ok()
            .map(|percent| 830 * percent.min(100) / 100);
    }
    width.trim_end_matches("px").trim().parse().ok()
}

/// Shields and CI status images: shown as labels, like chips.
fn is_badge(url: &str) -> bool {
    const HOSTS: [&str; 14] = [
        "shields.io",
        "badgen.net",
        "badge.fury.io",
        "codecov.io",
        "coveralls.io",
        "travis-ci.org",
        "travis-ci.com",
        "circleci.com",
        "codacy.com",
        "deepwiki.com",
        "snyk.io",
        "sonarcloud.io",
        "api.netlify.com",
        "goreportcard.com",
    ];
    let lower = url.to_ascii_lowercase();
    let path = lower.split(['?', '#']).next().unwrap_or_default();
    let host = path
        .split_once("://")
        .map_or("", |(_, rest)| rest.split('/').next().unwrap_or_default());
    HOSTS
        .iter()
        .any(|known| host == *known || host.ends_with(&format!(".{known}")))
        || path.ends_with("badge.svg")
        || path.contains("/badge/")
        || path.ends_with("/badge")
}

/// HTML whitespace: every run is one space.
fn collapse_spaces(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for ch in text.chars() {
        if ch.is_whitespace() && ch != '\u{a0}' {
            if !space {
                out.push(' ');
            }
            space = true;
        } else {
            out.push(ch);
            space = false;
        }
    }
    out
}

/// Greedy word wrap that keeps each word's style and link; a word wider
/// than the line is cut.
fn wrap(segments: Vec<Seg>, width: usize) -> Vec<Vec<Seg>> {
    let width = width.max(1);
    let mut lines: Vec<Vec<Seg>> = vec![Vec::new()];
    let mut used = 0;
    let mut space: Option<(Style, Option<usize>)> = None;
    for segment in segments {
        let (style, link) = (segment.style, segment.link);
        for (index, word) in segment.text.split(' ').enumerate() {
            if index > 0 && used > 0 {
                space = Some((style, link));
            }
            if word.is_empty() {
                continue;
            }
            let needed = word.width() + usize::from(space.is_some());
            if used > 0 && used + needed > width {
                lines.push(Vec::new());
                used = 0;
                space = None;
            }
            if let Some((space_style, space_link)) = space.take() {
                lines
                    .last_mut()
                    .unwrap()
                    .push(Seg::new(" ", space_style, space_link));
                used += 1;
            }
            let mut parts = cut(word, width.saturating_sub(used).max(1)).into_iter();
            if let Some(first) = parts.next() {
                used += first.width();
                lines.last_mut().unwrap().push(Seg::new(first, style, link));
            }
            for part in parts {
                used = part.width();
                lines.push(vec![Seg::new(part, style, link)]);
            }
        }
    }
    lines.retain(|line| !line.is_empty());
    if lines.is_empty() {
        lines.push(Vec::new());
    }
    lines
}

/// `text` cut into pieces of at most `width` cells; the first piece may be
/// shorter than the following ones only because of `width`.
fn cut(text: &str, width: usize) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut used = 0;
    for ch in text.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if used + ch_width > width && used > 0 {
            parts.push(String::new());
            used = 0;
        }
        parts.last_mut().unwrap().push(ch);
        used += ch_width;
    }
    parts
}

/// Colored pieces of a code line cut into lines of at most `width` cells;
/// an empty line stays one empty line.
fn cut_colored(pieces: Vec<(String, Color)>, width: usize) -> Vec<Vec<(String, Color)>> {
    let mut lines: Vec<Vec<(String, Color)>> = vec![Vec::new()];
    let mut used = 0;
    for (text, color) in pieces {
        let mut current = String::new();
        for ch in text.chars() {
            let ch_width = ch.width().unwrap_or(0);
            if used + ch_width > width && used > 0 {
                if !current.is_empty() {
                    lines
                        .last_mut()
                        .unwrap()
                        .push((std::mem::take(&mut current), color));
                }
                lines.push(Vec::new());
                used = 0;
            }
            current.push(ch);
            used += ch_width;
        }
        if !current.is_empty() {
            lines.last_mut().unwrap().push((current, color));
        }
    }
    lines
}
