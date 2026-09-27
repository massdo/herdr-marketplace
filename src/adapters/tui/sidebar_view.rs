use ratatui::Frame;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::sidebar::{Filter, LoadState, SidebarApp};
use super::style::{
    ACCENT, CARD, ERROR, GOLD, MUTED, OK, SELECTION_BG, SELECTION_FG, Tone, WARN, bold,
    button_text, ellipsize, ellipsize_middle, muted, wrap,
};
use crate::domain::listing::Row;
use crate::domain::text::clean;

/// Terminal lines per plugin: a card whose top edge carries the name and
/// the stars, then owner/repo, marks and description, then its bottom edge.
pub const ROW_HEIGHT: usize = 4;
/// The narrowest pane that draws cards; narrower, plugins are plain lines.
const CARD_WIDTH: usize = 16;
const PLACEHOLDER: &str = "Search name, topic, author";
const FOOTER: &str = "↵ open · ⇥ filter · Esc close";
const RETRY: &str = "Retry";
/// The narrowest pane that draws the search box around its text.
const BOX_WIDTH: usize = 8;

/// What a click lands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// This position in the list of results.
    Row(usize),
    Retry,
    Filter(Filter),
    /// The × that empties the search.
    Clear,
}

/// Plugins the list area of a `width` × `height` pane shows.
pub fn page_rows(app: &SidebarApp, width: u16, height: u16) -> usize {
    let used = header(app, width as usize).len() + 1;
    (height as usize).saturating_sub(used) / ROW_HEIGHT
}

pub fn render(frame: &mut Frame, app: &SidebarApp) {
    let area = frame.area();
    let width = area.width as usize;
    let mut lines = header(app, width);
    let list_height = (area.height as usize).saturating_sub(lines.len() + 1);
    let mut body = match &app.state {
        LoadState::Loading => vec![Line::styled("Loading catalog…", muted())],
        LoadState::Failed(error) => failure(error, width),
        LoadState::Ready(_) => list(app, width),
    };
    body.truncate(list_height);
    body.resize(list_height, Line::default());
    lines.extend(body);
    lines.push(Line::styled(ellipsize(FOOTER, width), muted()));
    frame.render_widget(Paragraph::new(lines), area);
}

/// What a click at `column`, `row` of a `width` × `height` pane lands on,
/// laid out as `render` draws it.
pub fn hit(app: &SidebarApp, width: u16, height: u16, column: u16, row: u16) -> Option<Hit> {
    let (cells, line) = (column as usize, row as usize);
    let boxed = box_height(width as usize);
    if boxed == 3
        && line == 1
        && !app.query.is_empty()
        && (width as usize - 4..width as usize - 1).contains(&cells)
    {
        return Some(Hit::Clear);
    }
    if line == boxed && matches!(app.state, LoadState::Ready(_)) {
        return tab_cells(app)
            .into_iter()
            .find(|(_, start, end)| (*start..*end).contains(&cells))
            .map(|(filter, _, _)| Hit::Filter(filter));
    }
    let top = header(app, width as usize).len();
    let list_height = (height as usize).saturating_sub(top + 1);
    let line = (row as usize)
        .checked_sub(top)
        .filter(|&line| line < list_height)?;
    match &app.state {
        LoadState::Ready(_) => {
            let shown = line / ROW_HEIGHT;
            let position = app.offset + shown;
            (shown < app.page && position < app.visible.len()).then_some(Hit::Row(position))
        }
        LoadState::Failed(error) => {
            let retry = failure(error, width as usize).len() - 1;
            let cells = button_text(RETRY, "Enter").width();
            (line == retry && (column as usize) < cells).then_some(Hit::Retry)
        }
        LoadState::Loading => None,
    }
}

/// The search box, the filters with their counts, notes, then a line that
/// sets the list apart.
fn header(app: &SidebarApp, width: usize) -> Vec<Line<'static>> {
    let mut lines = search_box(app, width);
    if let LoadState::Ready(loaded) = &app.state {
        lines.push(tabs(app, width));
        let hidden = app.hidden;
        if hidden > 0 && app.filter == Filter::All {
            let plural = if hidden == 1 { "" } else { "s" };
            lines.push(Line::styled(
                ellipsize(
                    &format!("{hidden} incompatible plugin{plural} hidden"),
                    width,
                ),
                muted(),
            ));
        }
        if loaded.registry_error.is_some() {
            lines.push(Line::styled(
                ellipsize("Herdr registry unreadable: install state unknown", width),
                Style::default().fg(WARN),
            ));
        }
    }
    if let Some(notice) = &app.notice {
        lines.extend(
            wrap(&clean(notice), width)
                .into_iter()
                .map(|line| Line::styled(line, Style::default().fg(ERROR))),
        );
    }
    lines.push(Line::styled("─".repeat(width), muted()));
    lines
}

fn box_height(width: usize) -> usize {
    if width >= BOX_WIDTH { 3 } else { 1 }
}

/// An input field: a frame, colored while the pane has the focus, around
/// the placeholder, or the search with its caret and its × to empty it.
fn search_box(app: &SidebarApp, width: usize) -> Vec<Line<'static>> {
    let frame = Style::default().fg(if app.focused { ACCENT } else { MUTED });
    let caret = Span::styled("▏", Style::default().fg(ACCENT));
    if box_height(width) == 1 {
        let text = if app.query.is_empty() {
            Span::styled(ellipsize(PLACEHOLDER, width), muted())
        } else {
            Span::raw(tail(&clean(&app.query), width.saturating_sub(1)))
        };
        return vec![Line::from(vec![text, caret])];
    }
    let inner = width - 4;
    let mut middle = vec![Span::styled("│ ", frame)];
    if app.query.is_empty() {
        let placeholder = ellipsize(PLACEHOLDER, inner);
        let pad = inner.saturating_sub(placeholder.width());
        middle.push(Span::styled(placeholder, muted()));
        middle.push(Span::raw(" ".repeat(pad)));
    } else {
        let query = tail(&clean(&app.query), inner.saturating_sub(3));
        let pad = inner.saturating_sub(query.width() + 2);
        middle.push(Span::raw(query));
        middle.push(if app.focused { caret } else { Span::raw(" ") });
        middle.push(Span::raw(" ".repeat(pad)));
        middle.push(Span::styled("×", muted()));
    }
    middle.push(Span::styled(" │", frame));
    vec![
        Line::styled(format!("╭{}╮", "─".repeat(width - 2)), frame),
        Line::from(middle),
        Line::styled(format!("╰{}╯", "─".repeat(width - 2)), frame),
    ]
}

/// The last `width` cells of `text`: the end of a long search stays in view.
fn tail(text: &str, width: usize) -> String {
    let mut kept: Vec<char> = Vec::new();
    let mut used = 0;
    for ch in text.chars().rev() {
        let cells = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + cells > width {
            break;
        }
        used += cells;
        kept.push(ch);
    }
    kept.into_iter().rev().collect()
}

/// Filter tabs and the columns each covers; the one shown is filled.
fn tab_cells(app: &SidebarApp) -> Vec<(Filter, usize, usize)> {
    let mut cells = Vec::new();
    let mut used = 0;
    for (filter, label) in tab_labels(app) {
        let start = if cells.is_empty() { 0 } else { used + 1 };
        let end = start + label.width();
        cells.push((filter, start, end));
        used = end;
    }
    cells
}

fn tab_labels(app: &SidebarApp) -> [(Filter, String); 2] {
    [
        (Filter::All, format!(" All {} ", app.counts.all)),
        (
            Filter::Installed,
            format!(" Installed {} ", app.counts.installed),
        ),
    ]
}

fn tabs(app: &SidebarApp, width: usize) -> Line<'static> {
    let mut spans = Vec::new();
    let mut used = 0;
    for ((filter, label), (_, start, end)) in tab_labels(app).into_iter().zip(tab_cells(app)) {
        if end > width {
            break;
        }
        spans.push(Span::raw(" ".repeat(start - used)));
        let tone = if filter == app.filter {
            Tone::Primary
        } else {
            Tone::Plain
        };
        spans.push(Span::styled(label, tone.style()));
        used = end;
    }
    Line::from(spans)
}

/// The error, then a Retry button on the last line.
fn failure(error: &str, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![Line::styled("Loading failed", Style::default().fg(ERROR))];
    lines.extend(
        wrap(&clean(error), width)
            .into_iter()
            .map(|line| Line::styled(line, muted())),
    );
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        button_text(RETRY, "Enter"),
        Tone::Plain.style(),
    )));
    lines
}

fn list(app: &SidebarApp, width: usize) -> Vec<Line<'static>> {
    if app.visible.is_empty() {
        let empty = if app.filter == Filter::Installed && app.query.trim().is_empty() {
            "No plugin installed from GitHub"
        } else {
            "No matching plugin"
        };
        return vec![Line::styled(ellipsize(empty, width), muted())];
    }
    let rows = app.rows();
    let selected = app.selected_position();
    app.visible
        .iter()
        .enumerate()
        .skip(app.offset)
        .take(app.page)
        .flat_map(|(position, &index)| row_lines(&rows[index], width, selected == Some(position)))
        .collect()
}

/// A plugin as a card with a light frame, blue when it is selected. The
/// top edge carries the name and the golden star; inside, owner/repo, then
/// the marks and the description.
fn row_lines(row: &Row, width: usize, selected: bool) -> Vec<Line<'static>> {
    if width < CARD_WIDTH {
        let mut lines = plain_row(row, width, selected);
        lines.push(Line::default());
        return lines;
    }
    let entry = &row.entry;
    let frame = Style::default().fg(if selected { ACCENT } else { CARD });
    let stars: Vec<Span<'static>> = if row.in_catalog {
        vec![
            Span::styled(" ★", Style::default().fg(GOLD)),
            Span::styled(format!(" {} ", entry.stars), muted()),
        ]
    } else {
        Vec::new()
    };
    let stars_width: usize = stars.iter().map(|span| span.content.width()).sum();
    // "╭ " + name + " " + "─" as needed + stars + "╮".
    let name = ellipsize(
        &clean(&entry.name),
        width.saturating_sub(4 + stars_width).max(1),
    );
    let dashes = width.saturating_sub(4 + name.width() + stars_width);
    let mut top = vec![
        Span::styled("╭ ", frame),
        Span::styled(name, bold()),
        Span::styled(format!(" {}", "─".repeat(dashes)), frame),
    ];
    top.extend(stars);
    top.push(Span::styled("╮", frame));

    let inner = width - 4;
    let source = vec![Span::styled(
        ellipsize_middle(&clean(&entry.source.to_string()), inner),
        muted(),
    )];
    let mut details = Vec::new();
    let mut used = 0;
    for (mark, color) in marks(row) {
        let text = if used == 0 {
            mark.to_string()
        } else {
            format!(" · {mark}")
        };
        used += text.width();
        details.push(Span::styled(text, Style::default().fg(color)));
    }
    let separator = if used == 0 { "" } else { " · " };
    let room = inner.saturating_sub(used + separator.width());
    if let Some(description) = entry.description.as_deref().filter(|_| room > 0) {
        details.push(Span::styled(
            format!("{separator}{}", ellipsize(&clean(description), room)),
            muted(),
        ));
    }
    let side = |content: Vec<Span<'static>>| {
        let used: usize = content.iter().map(|span| span.content.width()).sum();
        let mut inside = vec![Span::raw(" ")];
        inside.extend(content);
        inside.push(Span::raw(" ".repeat(inner.saturating_sub(used) + 1)));
        if selected {
            highlight(&mut inside);
        }
        let mut spans = vec![Span::styled("│", frame)];
        spans.extend(inside);
        spans.push(Span::styled("│", frame));
        Line::from(spans)
    };
    vec![
        Line::from(top),
        side(source),
        side(details),
        Line::styled(format!("╰{}╯", "─".repeat(width - 2)), frame),
    ]
}

/// Name and stars, owner/repo, marks and description on three plain lines,
/// for a pane too narrow for cards.
fn plain_row(row: &Row, width: usize, selected: bool) -> Vec<Line<'static>> {
    let entry = &row.entry;
    let stars = if row.in_catalog {
        format!(" ★ {}", entry.stars)
    } else {
        String::new()
    };
    let name = ellipsize(&clean(&entry.name), width.saturating_sub(stars.width()));
    let pad = width.saturating_sub(name.width() + stars.width());
    let mut lines = vec![
        Line::from(vec![
            Span::styled(name, bold()),
            Span::raw(" ".repeat(pad)),
            Span::styled(stars, Style::default().fg(GOLD)),
        ]),
        Line::styled(
            ellipsize_middle(&clean(&entry.source.to_string()), width),
            muted(),
        ),
        Line::styled(
            ellipsize(
                &clean(entry.description.as_deref().unwrap_or_default()),
                width,
            ),
            muted(),
        ),
    ];
    if selected {
        for line in &mut lines {
            highlight(&mut line.spans);
        }
    }
    lines
}

/// The selection's background, text kept readable on it.
fn highlight(spans: &mut [Span<'static>]) {
    for span in spans {
        let fg = match span.style.fg {
            None | Some(MUTED) => SELECTION_FG,
            Some(color) => color,
        };
        span.style = span.style.bg(SELECTION_BG).fg(fg);
    }
}

fn marks(row: &Row) -> Vec<(&'static str, ratatui::style::Color)> {
    let mut marks = Vec::new();
    if row.installed.is_some() {
        marks.push(("installed", OK));
    }
    if !row.compatible {
        marks.push(("incompatible", WARN));
    }
    if !row.in_catalog {
        marks.push(("not in catalog", WARN));
    }
    marks
}
