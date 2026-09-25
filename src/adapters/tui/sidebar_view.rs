use ratatui::Frame;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::sidebar::{LoadState, SidebarApp};
use super::style::{
    ACCENT, ERROR, MUTED, OK, SELECTION_BG, SELECTION_FG, WARN, bold, ellipsize, ellipsize_middle,
    muted, wrap,
};
use crate::domain::listing::Row;
use crate::domain::text::clean;

/// Terminal lines per plugin: name, owner/repo, marks and description.
pub const ROW_HEIGHT: usize = 3;
const FOOTER: &str = "Entrée fiche · Échap fermer";

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
        LoadState::Loading => vec![Line::styled("Chargement du catalogue…", muted())],
        LoadState::Failed(error) => failure(error, width),
        LoadState::Ready(_) => list(app, width),
    };
    body.truncate(list_height);
    body.resize(list_height, Line::default());
    lines.extend(body);
    lines.push(Line::styled(ellipsize(FOOTER, width), muted()));
    frame.render_widget(Paragraph::new(lines), area);
}

fn header(app: &SidebarApp, width: usize) -> Vec<Line<'static>> {
    let search = if app.query.is_empty() {
        Line::from(vec![
            Span::styled("> ", Style::default().fg(ACCENT)),
            Span::styled("Rechercher un plugin", muted()),
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
        let plural = if count > 1 { "s" } else { "" };
        lines.push(Line::styled(
            ellipsize(&format!("{count} résultat{plural}"), width),
            muted(),
        ));
        let hidden = loaded.listing.hidden_incompatible;
        if hidden > 0 {
            let plural = if hidden > 1 { "s" } else { "" };
            lines.push(Line::styled(
                ellipsize(
                    &format!("{hidden} incompatible{plural} masqué{plural}"),
                    width,
                ),
                muted(),
            ));
        }
        if loaded.registry_error.is_some() {
            lines.push(Line::styled(
                ellipsize(
                    "Registre Herdr illisible : état d'installation inconnu",
                    width,
                ),
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

fn failure(error: &str, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![Line::styled(
        "Échec du chargement",
        Style::default().fg(ERROR),
    )];
    lines.extend(
        wrap(&clean(error), width)
            .into_iter()
            .map(|line| Line::styled(line, muted())),
    );
    lines.push(Line::default());
    lines.push(Line::styled("Entrée : réessayer", bold()));
    lines
}

fn list(app: &SidebarApp, width: usize) -> Vec<Line<'static>> {
    if app.visible.is_empty() {
        return vec![Line::styled("Aucun plugin ne correspond", muted())];
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
        marks.push(("installé", OK));
    }
    if !row.compatible {
        marks.push(("incompatible", WARN));
    }
    if !row.in_catalog {
        marks.push(("hors catalogue", WARN));
    }
    marks
}
