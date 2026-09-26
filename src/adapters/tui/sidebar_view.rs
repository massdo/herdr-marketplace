use ratatui::Frame;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::sidebar::{LoadState, SidebarApp};
use super::style::{
    ACCENT, ERROR, MUTED, OK, SELECTION_BG, SELECTION_FG, Tone, WARN, bold, button_text, ellipsize,
    ellipsize_middle, muted, wrap,
};
use crate::domain::listing::Row;
use crate::domain::text::clean;

/// Terminal lines per plugin: name, owner/repo, marks and description.
pub const ROW_HEIGHT: usize = 3;
const PLACEHOLDER: &str = "Search name, topic, author…";
const FOOTER: &str = "Click/Enter: open · Esc: close";
const RETRY: &str = "Retry";

/// What a click lands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// This position in the list of results.
    Row(usize),
    Retry,
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

fn header(app: &SidebarApp, width: usize) -> Vec<Line<'static>> {
    let search = if app.query.is_empty() {
        Line::from(vec![
            Span::styled("> ", Style::default().fg(ACCENT)),
            Span::styled(ellipsize(PLACEHOLDER, width.saturating_sub(2)), muted()),
        ])
    } else {
        Line::from(vec![
            Span::styled("> ", Style::default().fg(ACCENT)),
            Span::raw(ellipsize(&clean(&app.query), width.saturating_sub(2))),
        ])
    };
    let mut lines = vec![search];
    if let LoadState::Ready(loaded) = &app.state {
        let count = app.visible.len();
        let plural = if count == 1 { "" } else { "s" };
        let counter = if app.query.trim().is_empty() {
            format!("{count} plugin{plural} in catalog")
        } else {
            format!("{count} result{plural}")
        };
        lines.push(Line::styled(ellipsize(&counter, width), muted()));
        let hidden = app.hidden;
        if hidden > 0 {
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
    lines
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
        return vec![Line::styled("No matching plugin", muted())];
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

fn row_lines(row: &Row, width: usize, selected: bool) -> Vec<Line<'static>> {
    let entry = &row.entry;
    let stars = if row.in_catalog {
        format!(" ★ {}", entry.stars)
    } else {
        String::new()
    };
    let name = ellipsize(&clean(&entry.name), width.saturating_sub(stars.width()));
    let pad = width.saturating_sub(name.width() + stars.width());
    let first = Line::from(vec![
        Span::styled(name, bold()),
        Span::raw(" ".repeat(pad)),
        Span::styled(stars, muted()),
    ]);
    let second = Line::styled(
        ellipsize_middle(&clean(&entry.source.to_string()), width),
        muted(),
    );

    let mut third = Vec::new();
    let mut used = 0;
    for (mark, color) in marks(row) {
        let text = if used == 0 {
            mark.to_string()
        } else {
            format!(" · {mark}")
        };
        used += text.width();
        third.push(Span::styled(text, Style::default().fg(color)));
    }
    let separator = if used == 0 { "" } else { " · " };
    let room = width.saturating_sub(used + separator.width());
    if let Some(description) = entry.description.as_deref().filter(|_| room > 0) {
        third.push(Span::styled(
            format!("{separator}{}", ellipsize(&clean(description), room)),
            muted(),
        ));
    }

    let mut lines = vec![first, second, Line::from(third)];
    if selected {
        for line in &mut lines {
            let used = line.width();
            line.spans
                .push(Span::raw(" ".repeat(width.saturating_sub(used))));
            for span in &mut line.spans {
                let fg = match span.style.fg {
                    None | Some(MUTED) => SELECTION_FG,
                    Some(color) => color,
                };
                span.style = span.style.bg(SELECTION_BG).fg(fg);
            }
        }
    }
    lines
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
