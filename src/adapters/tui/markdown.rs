//! README Markdown rendered for a terminal pane, the way VS Code shows an
//! extension page, within terminal limits. Nothing is executed: control
//! characters are removed before parsing and HTML only keeps its text.

use pulldown_cmark::{Alignment, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::style::{ACCENT, MUTED, ellipsize};

const CODE: Color = Color::Cyan;

pub fn render(markdown: &str, width: usize) -> Vec<Line<'static>> {
    let source = without_controls(markdown);
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut renderer = Renderer::new(width.max(10));
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

struct Item {
    marker: String,
    marker_shown: bool,
}

struct Table {
    alignments: Vec<Alignment>,
    rows: Vec<Vec<String>>,
    header_rows: usize,
}

struct Renderer {
    width: usize,
    lines: Vec<Line<'static>>,
    spans: Vec<Span<'static>>,
    strong: usize,
    emphasis: usize,
    strike: usize,
    heading: Option<HeadingLevel>,
    /// Destination and position of the first span of each open link.
    links: Vec<(String, usize)>,
    image_alt: Option<String>,
    /// Next number of each open list; `None` for bullets.
    lists: Vec<Option<u64>>,
    items: Vec<Item>,
    quote: usize,
    code: Option<String>,
    html: Option<String>,
    table: Option<Table>,
    cell: Option<String>,
}

impl Renderer {
    fn new(width: usize) -> Self {
        Self {
            width,
            lines: Vec::new(),
            spans: Vec::new(),
            strong: 0,
            emphasis: 0,
            strike: 0,
            heading: None,
            links: Vec::new(),
            image_alt: None,
            lists: Vec::new(),
            items: Vec::new(),
            quote: 0,
            code: None,
            html: None,
            table: None,
            cell: None,
        }
    }

    fn event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text),
            Event::Code(code) => {
                if let Some(cell) = &mut self.cell {
                    cell.push_str(&code);
                } else {
                    let style = self.style().fg(CODE);
                    self.spans.push(Span::styled(code.to_string(), style));
                }
            }
            Event::Html(html) => {
                if let Some(block) = &mut self.html {
                    block.push_str(&html);
                }
            }
            Event::InlineHtml(html) => self.inline_html(&html),
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => self.flush(),
            Event::Rule => {
                self.flush();
                self.blank();
                let width = self.width;
                self.lines
                    .push(Line::styled("─".repeat(width), Style::default().fg(MUTED)));
                self.blank();
            }
            Event::TaskListMarker(done) => {
                if let Some(item) = self.items.last_mut() {
                    item.marker = if done { "[x] " } else { "[ ] " }.to_string();
                }
            }
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {}
            Tag::Heading { level, .. } => {
                self.flush();
                self.blank();
                self.heading = Some(level);
            }
            Tag::BlockQuote(_) => {
                self.flush();
                self.blank();
                self.quote += 1;
            }
            Tag::CodeBlock(_) => {
                self.flush();
                self.blank();
                self.code = Some(String::new());
            }
            Tag::HtmlBlock => {
                self.flush();
                self.html = Some(String::new());
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
                let marker = match self.lists.last_mut() {
                    Some(Some(number)) => {
                        *number += 1;
                        format!("{}. ", *number - 1)
                    }
                    _ => "• ".to_string(),
                };
                self.items.push(Item {
                    marker,
                    marker_shown: false,
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
            Tag::TableCell => self.cell = Some(String::new()),
            Tag::Emphasis => self.emphasis += 1,
            Tag::Strong => self.strong += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } => self.links.push((dest_url.to_string(), self.spans.len())),
            Tag::Image { .. } => self.image_alt = Some(String::new()),
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
            TagEnd::Heading(_) => {
                self.flush();
                self.heading = None;
                self.blank();
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.quote = self.quote.saturating_sub(1);
                self.blank();
            }
            TagEnd::CodeBlock => {
                let code = self.code.take().unwrap_or_default();
                self.code_block(&code);
                self.blank();
            }
            TagEnd::HtmlBlock => {
                let html = self.html.take().unwrap_or_default();
                for line in html_text(&html).lines() {
                    self.text(line);
                    self.flush();
                }
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
                let cell = self.cell.take().unwrap_or_default();
                if let Some(row) = self.table.as_mut().and_then(|table| table.rows.last_mut()) {
                    row.push(cell);
                }
            }
            TagEnd::Emphasis => self.emphasis = self.emphasis.saturating_sub(1),
            TagEnd::Strong => self.strong = self.strong.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => {
                if let Some((url, first)) = self.links.pop() {
                    if let Some(cell) = &mut self.cell {
                        cell.push_str(&format!(" ({url})"));
                        return;
                    }
                    let text: String = self.spans[first.min(self.spans.len())..]
                        .iter()
                        .map(|span| span.content.as_ref())
                        .collect();
                    if text != url {
                        self.spans.push(Span::styled(
                            format!(" ({url})"),
                            Style::default().fg(MUTED),
                        ));
                    }
                }
            }
            TagEnd::Image => {
                let alt = image(&self.image_alt.take().unwrap_or_default());
                match &mut self.cell {
                    Some(cell) => cell.push_str(&alt.content),
                    None => self.spans.push(alt),
                }
            }
            _ => {}
        }
    }

    fn text(&mut self, text: &str) {
        if let Some(code) = &mut self.code {
            code.push_str(text);
        } else if let Some(alt) = &mut self.image_alt {
            alt.push_str(text);
        } else if let Some(cell) = &mut self.cell {
            cell.push_str(text);
        } else {
            let style = self.style();
            self.spans.push(Span::styled(text.to_string(), style));
        }
    }

    fn inline_html(&mut self, html: &str) {
        for (index, part) in html_text(html).split('\n').enumerate() {
            if index > 0 {
                self.flush();
            }
            if !part.is_empty() {
                self.text(part);
            }
        }
    }

    fn style(&self) -> Style {
        let mut style = Style::default();
        if let Some(level) = self.heading {
            style = style.fg(ACCENT).add_modifier(Modifier::BOLD);
            if level == HeadingLevel::H1 {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
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
        if !self.links.is_empty() {
            style = style.fg(ACCENT).add_modifier(Modifier::UNDERLINED);
        }
        style
    }

    /// Prefix of a block line: quote bars, then list indentation or marker.
    fn prefix(&mut self) -> Vec<Span<'static>> {
        let mut prefix = Vec::new();
        if self.quote > 0 {
            prefix.push(Span::styled(
                "│ ".repeat(self.quote),
                Style::default().fg(MUTED),
            ));
        }
        let depth = self.items.len();
        for (index, item) in self.items.iter_mut().enumerate() {
            if index + 1 == depth && !item.marker_shown {
                item.marker_shown = true;
                prefix.push(Span::styled(
                    item.marker.clone(),
                    Style::default().fg(ACCENT),
                ));
            } else {
                prefix.push(Span::raw(" ".repeat(item.marker.width())));
            }
        }
        prefix
    }

    fn prefix_width(&self) -> usize {
        self.quote * 2
            + self
                .items
                .iter()
                .map(|item| item.marker.width())
                .sum::<usize>()
    }

    /// Wraps the pending inline spans to the pane width.
    fn flush(&mut self) {
        if self.spans.is_empty() {
            return;
        }
        let spans = std::mem::take(&mut self.spans);
        let room = self.width.saturating_sub(self.prefix_width()).max(1);
        for line in wrap_spans(spans, room) {
            let mut full = self.prefix();
            full.extend(line);
            self.lines.push(Line::from(full));
        }
    }

    fn blank(&mut self) {
        if self.lines.last().is_some_and(|line| line.width() > 0) {
            self.lines.push(Line::default());
        }
    }

    fn code_block(&mut self, code: &str) {
        let room = self.width.saturating_sub(self.prefix_width() + 2).max(1);
        for line in code.trim_end_matches('\n').split('\n') {
            for part in split_cells(line, room) {
                let mut full = self.prefix();
                full.push(Span::raw("  "));
                full.push(Span::styled(part, Style::default().fg(CODE)));
                self.lines.push(Line::from(full));
            }
        }
    }

    fn table_lines(&mut self, table: Table) {
        let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        if columns == 0 {
            return;
        }
        let mut widths = vec![1; columns];
        for row in &table.rows {
            for (column, cell) in row.iter().enumerate() {
                widths[column] = widths[column].max(cell.width());
            }
        }
        let room = self
            .width
            .saturating_sub(self.prefix_width() + 3 * (columns - 1));
        while widths.iter().sum::<usize>() > room && widths.iter().any(|&width| width > 3) {
            if let Some(widest) = widths.iter_mut().max() {
                *widest -= 1;
            }
        }
        for (index, row) in table.rows.iter().enumerate() {
            let header = index < table.header_rows;
            let mut spans = self.prefix();
            for (column, &width) in widths.iter().enumerate() {
                if column > 0 {
                    spans.push(Span::styled(" │ ", Style::default().fg(MUTED)));
                }
                let cell = row.get(column).map(String::as_str).unwrap_or("");
                let alignment = table
                    .alignments
                    .get(column)
                    .copied()
                    .unwrap_or(Alignment::None);
                let text = align(&ellipsize(cell, width), width, alignment);
                let style = if header {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                spans.push(Span::styled(text, style));
            }
            self.lines.push(Line::from(spans));
            if header && index + 1 == table.header_rows {
                let rule: Vec<String> = widths.iter().map(|&width| "─".repeat(width)).collect();
                let mut spans = self.prefix();
                spans.push(Span::styled(rule.join("─┼─"), Style::default().fg(MUTED)));
                self.lines.push(Line::from(spans));
            }
        }
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        self.flush();
        while self.lines.last().is_some_and(|line| line.width() == 0) {
            self.lines.pop();
        }
        // Lines are split on line feeds above; this keeps the promise that no
        // control character of the README is ever drawn.
        for span in self.lines.iter_mut().flat_map(|line| line.spans.iter_mut()) {
            if span.content.chars().any(char::is_control) {
                span.content = span.content.replace(char::is_control, " ").into();
            }
        }
        self.lines
    }
}

fn image(alt: &str) -> Span<'static> {
    let alt = alt.trim();
    let text = if alt.is_empty() {
        "[image]".to_string()
    } else {
        format!("[image: {alt}]")
    };
    Span::styled(text, Style::default().fg(MUTED))
}

fn align(text: &str, width: usize, alignment: Alignment) -> String {
    let pad = width.saturating_sub(text.width());
    match alignment {
        Alignment::Right => format!("{}{text}", " ".repeat(pad)),
        Alignment::Center => format!("{}{text}{}", " ".repeat(pad / 2), " ".repeat(pad - pad / 2)),
        Alignment::Left | Alignment::None => format!("{text}{}", " ".repeat(pad)),
    }
}

/// Greedy word wrap that keeps each word's style; a word wider than the line
/// is cut.
fn wrap_spans(spans: Vec<Span<'static>>, width: usize) -> Vec<Vec<Span<'static>>> {
    let mut lines: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    let mut used = 0;
    let mut space: Option<Style> = None;
    for span in spans {
        let style = span.style;
        for (index, word) in span.content.split(' ').enumerate() {
            if index > 0 && used > 0 {
                space = Some(style);
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
            if let Some(space_style) = space.take() {
                lines
                    .last_mut()
                    .unwrap()
                    .push(Span::styled(" ", space_style));
                used += 1;
            }
            let mut parts = split_cells(word, width.saturating_sub(used).max(1)).into_iter();
            if let Some(first) = parts.next() {
                used += first.width();
                lines.last_mut().unwrap().push(Span::styled(first, style));
            }
            for part in parts {
                used = part.width();
                lines.push(vec![Span::styled(part, style)]);
            }
        }
    }
    lines.retain(|line| !line.is_empty());
    lines
}

/// `text` cut into pieces of at most `width` cells; the first piece may be
/// shorter than the following ones only because of `width`.
fn split_cells(text: &str, width: usize) -> Vec<String> {
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

/// HTML reduced to its text: `<img>` becomes `[image: alt]`, `<br>` and
/// block ends become line breaks, comments and every other tag disappear.
fn html_text(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(open) = rest.find('<') {
        out.push_str(&decode_entities(&rest[..open]));
        rest = &rest[open..];
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.find("-->").map_or("", |end| &after[end + 3..]);
            continue;
        }
        let Some(close) = rest.find('>') else {
            rest = "";
            break;
        };
        let tag = &rest[1..close];
        rest = &rest[close + 1..];
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        match name.as_str() {
            "img" => out.push_str(&image(&decode_entities(&attribute(tag, "alt"))).content),
            "br" => out.push('\n'),
            "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
                if tag.starts_with('/') =>
            {
                out.push('\n')
            }
            _ => {}
        }
    }
    out.push_str(&decode_entities(rest));
    out
}

/// Value of `name="…"` (or single quotes) in a tag.
fn attribute(tag: &str, name: &str) -> String {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = lower[from..].find(name) {
        let start = from + found;
        from = start + name.len();
        let before = lower[..start].chars().last();
        if !before.is_some_and(char::is_whitespace) {
            continue;
        }
        let rest = tag[from..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(quote) = rest.chars().next().filter(|ch| *ch == '"' || *ch == '\'') else {
            return rest.split_whitespace().next().unwrap_or("").to_string();
        };
        return rest[1..].split(quote).next().unwrap_or("").to_string();
    }
    String::new()
}

fn decode_entities(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}
