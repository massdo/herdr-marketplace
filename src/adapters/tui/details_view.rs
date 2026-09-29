use std::iter;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

use super::animation::Spot;
use super::details::{
    Button, Command, DetailsApp, InstallState, InstalledView, ReadmeState, RemovalState,
    UpdateState,
};
use super::graphics::without_image;
use super::selection::{self, Flow, Selection};
use super::style::{
    ERROR, MUTED, OK, WARN, bold, button_text, ellipsize, ellipsize_middle, muted, wrap,
};
use crate::adapters::pane_graphics::Cells;
use crate::domain::PLUGIN_ID;
use crate::domain::operation::{Confirmation, OperationKind, Status};
use crate::domain::text::clean;

const FOOTER: &str = "s: full SHA · q: close · ↑↓ PgUp PgDn Home End";
const PREVIEW_FOOTER: &str = "Enter: confirm · Esc: cancel · ↑↓ PgUp PgDn";
/// Header lines: title, source, commit, then the action bar.
const COMMIT_LINE: usize = 2;
const BAR_LINE: usize = 3;
const BUTTON_GAP: usize = 2;

/// Body lines visible after the compact identity header and footer.
pub fn page_rows(app: &DetailsApp, width: u16, height: u16) -> usize {
    let header = header(app, width as usize)
        .len()
        .min(height.saturating_sub(2) as usize);
    (height as usize).saturating_sub(header + 1)
}

pub fn render(frame: &mut Frame, app: &DetailsApp) {
    let area = frame.area();
    let (lines, flows) = rows(app, area.width, area.height);
    frame.render_widget(Paragraph::new(lines), area);
    if let Some(selection) = &app.selection {
        selection::highlight(frame.buffer_mut(), &flows, selection);
    }
}

/// The text `selection` covers in a `width` × `height` pane, as a copy
/// reads it.
pub fn selected_text(app: &DetailsApp, selection: &Selection, width: u16, height: u16) -> String {
    let (lines, flows) = rows(app, width, height);
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    Paragraph::new(lines).render(area, &mut buffer);
    selection::copy(&buffer, &flows, selection)
}

/// The rows of the pane, as drawn, and how each reads in a copy.
fn rows(app: &DetailsApp, width: u16, height: u16) -> (Vec<Line<'static>>, Vec<Flow>) {
    let cells = width as usize;
    let mut lines = header(app, cells);
    // The rule under the header is drawn for looks.
    let mut flows = vec![Flow::default(); lines.len().saturating_sub(1)];
    flows.push(Flow::DECOR);
    lines.truncate(height.saturating_sub(2) as usize);
    flows.truncate(lines.len());
    let body_height = page_rows(app, width, height);
    let scroll = if app.showing_confirmation() {
        app.preview_scroll
    } else {
        app.scroll
    };
    let (prefix, content, content_flows) = body_lines(app, cells);
    // Body lines of the images an animation covers.
    let covered: Vec<_> = if showing_readme(app) {
        app.places
            .iter()
            .filter(|place| app.covered.contains(&place.url))
            .map(|place| {
                let first = prefix.len() + place.line;
                first..first + usize::from(place.rows)
            })
            .collect()
    } else {
        Vec::new()
    };
    let body_flows = iter::repeat_n(Flow::default(), prefix.len())
        .chain(content_flows.iter().copied())
        .chain(iter::repeat(Flow::default()));
    let body = prefix
        .into_iter()
        .chain(content.iter().cloned())
        .zip(body_flows)
        .enumerate()
        .skip(scroll)
        .take(body_height);
    let first = lines.len();
    for (index, (line, flow)) in body {
        if covered.iter().any(|rows| rows.contains(&index)) {
            lines.push(without_image(line));
        } else {
            lines.push(line);
        }
        flows.push(flow);
    }
    // The first row of the body never goes on from the header above it.
    if let Some(flow) = flows.get_mut(first) {
        flow.joins = None;
    }
    lines.resize(first + body_height, Line::default());
    flows.resize(first + body_height, Flow::default());
    let footer = if app.showing_confirmation() {
        if body_height == 0 {
            "Enlarge pane to confirm · Esc: cancel"
        } else {
            PREVIEW_FOOTER
        }
    } else {
        FOOTER
    };
    lines.push(Line::styled(ellipsize(footer, cells), muted()));
    flows.push(Flow::default());
    (lines, flows)
}

/// Where the animated images of the README show in a `width` × `height`
/// pane, laid out as `render` draws it: the cells of the rows the body shows
/// of each, over its first frame.
pub fn spots(app: &DetailsApp, width: u16, height: u16) -> Vec<(String, Spot)> {
    if !showing_readme(app) {
        return Vec::new();
    }
    let top = header(app, width as usize)
        .len()
        .min(height.saturating_sub(2) as usize);
    let page = page_rows(app, width, height);
    let (prefix, _, _) = body_lines(app, width as usize);
    let mut spots: Vec<(String, Spot)> = Vec::new();
    for place in &app.places {
        if !app.pictures.animated(&place.url) || spots.iter().any(|(url, _)| *url == place.url) {
            continue;
        }
        let first = prefix.len() + place.line;
        let from = app.scroll.max(first);
        let to = (app.scroll + page).min(first + usize::from(place.rows));
        if from >= to {
            continue;
        }
        let spot = Spot {
            cells: Cells {
                column: place.column as u16,
                row: (top + from - app.scroll) as u16,
                columns: place.columns,
                rows: (to - from) as u16,
            },
            first: (from - first) as u16,
            rows: place.rows,
        };
        spots.push((place.url.clone(), spot));
    }
    spots
}

/// The body shows the README, which no confirmation replaces.
fn showing_readme(app: &DetailsApp) -> bool {
    !app.showing_confirmation() && matches!(app.readme, ReadmeState::Found { .. })
}

/// What a click at `column`, `row` of a `width` × `height` pane runs, laid
/// out as `render` draws it: a button of the action bar, or the commit,
/// which shows the full SHA.
pub fn hit(app: &DetailsApp, width: u16, height: u16, column: u16, row: u16) -> Option<Command> {
    let shown = header(app, width as usize)
        .len()
        .min(height.saturating_sub(2) as usize);
    let (column, row) = (column as usize, row as usize);
    if row >= shown {
        return link_at(app, width, height, column, row - shown);
    }
    if row == COMMIT_LINE {
        return Some(Command::ToggleSha);
    }
    let buttons = app.buttons();
    if row != BAR_LINE || buttons.is_empty() {
        return None;
    }
    buttons
        .iter()
        .zip(bar_layout(&buttons, width as usize))
        .find(|(_, (start, end))| (*start..*end).contains(&column))
        .map(|(button, _)| button.command)
}

/// The README link under a click on body line `row`.
fn link_at(
    app: &DetailsApp,
    width: u16,
    height: u16,
    column: usize,
    row: usize,
) -> Option<Command> {
    if app.showing_confirmation() || row >= page_rows(app, width, height) {
        return None;
    }
    let (prefix, _, _) = body_lines(app, width as usize);
    let line = (app.scroll + row).checked_sub(prefix.len())?;
    app.links
        .iter()
        .position(|area| area.line == line && (area.start..area.end).contains(&column))
        .map(Command::OpenLink)
}

/// Cells each button covers, from the left; the buttons that do not fit are
/// left out.
fn bar_layout(buttons: &[Button], width: usize) -> Vec<(usize, usize)> {
    let mut cells = Vec::new();
    let mut used = 0;
    for button in buttons {
        let start = if cells.is_empty() {
            0
        } else {
            used + BUTTON_GAP
        };
        let end = start + button_text(&button.label, button.key).width();
        if end > width {
            break;
        }
        cells.push((start, end));
        used = end;
    }
    cells
}

fn bar(buttons: &[Button], width: usize) -> Line<'static> {
    let mut spans = Vec::new();
    let mut used = 0;
    for (button, (start, end)) in buttons.iter().zip(bar_layout(buttons, width)) {
        spans.push(Span::raw(" ".repeat(start - used)));
        spans.push(Span::styled(
            button_text(&button.label, button.key),
            button.tone.style(),
        ));
        used = end;
    }
    Line::from(spans)
}

/// All variable-length content scrolls. A confirmation starts at its first
/// line and is never pushed below old operation output. The README comes
/// with how its lines read in a copy.
pub(super) fn body_lines(
    app: &DetailsApp,
    width: usize,
) -> (Vec<Line<'static>>, &[Line<'static>], &[Flow]) {
    if app.showing_preview() {
        return (Vec::new(), &app.preview, &[]);
    }
    if matches!(app.removal, RemovalState::Confirm(_)) {
        return (removal_lines(app, width), &[], &[]);
    }
    let mut lines = notices(app, width);
    lines.extend(operation_lines(app, width));
    if !lines.is_empty() {
        lines.push(Line::default());
    }
    match &app.readme {
        ReadmeState::Loading => lines.push(Line::styled("Loading README…", muted())),
        ReadmeState::NotFound => {
            lines.push(Line::styled("README not found", Style::default().fg(WARN)))
        }
        ReadmeState::NetworkError(error) => {
            lines.push(Line::styled(
                "README not loaded: network error",
                Style::default().fg(ERROR),
            ));
            lines.extend(
                wrap(&clean(error), width)
                    .into_iter()
                    .map(|line| Line::styled(line, muted())),
            );
        }
        ReadmeState::Found { .. } => return (lines, &app.lines, &app.flows),
    }
    (lines, &[], &[])
}

/// What the removal will do, before its confirmation.
fn removal_lines(app: &DetailsApp, width: usize) -> Vec<Line<'static>> {
    let RemovalState::Confirm(plan) = &app.removal else {
        return Vec::new();
    };
    let installed = &plan.installed;
    let source = installed
        .github_source()
        .map(ToString::to_string)
        .unwrap_or_default();
    let mut lines = vec![Line::styled("Remove this plugin?", bold())];
    for text in [
        format!("id: {}", installed.plugin_id),
        format!("source: {source}"),
        format!(
            "installed commit: {}",
            installed.resolved_commit().unwrap_or_default()
        ),
        "Herdr also deletes its checkout.".to_string(),
    ] {
        lines.extend(wrap(&clean(&text), width).into_iter().map(Line::raw));
    }
    lines
}

fn header(app: &DetailsApp, width: usize) -> Vec<Line<'static>> {
    let target = &app.target;
    let mut title = vec![Span::styled(clean(&target.name), bold())];
    if let Some(version) = &target.version {
        title.push(Span::styled(format!("  {}", clean(version)), muted()));
    }
    if !target.compatible {
        title.push(Span::styled("  incompatible", Style::default().fg(WARN)));
    }
    if !target.in_catalog {
        title.push(Span::styled("  not in catalog", Style::default().fg(WARN)));
    }
    let commit = if app.full_sha {
        target.commit.clone()
    } else {
        target.commit.chars().take(7).collect()
    };
    let installed = match &app.installed {
        InstalledView::Unknown => Span::raw(""),
        InstalledView::At { commit, .. } if *commit == target.commit => {
            Span::styled(" · installed", Style::default().fg(OK))
        }
        InstalledView::At { commit, .. } => Span::styled(
            format!(" · installed at {}", short(commit)),
            Style::default().fg(OK),
        ),
        InstalledView::NotInstalled => Span::styled(" · not installed", muted()),
        InstalledView::Uncertain(_) => Span::styled(" · unknown state", Style::default().fg(WARN)),
    };
    let mut lines = vec![
        fit(Line::from(title), width),
        Line::styled(
            ellipsize_middle(&clean(&target.source.to_string()), width),
            muted(),
        ),
        fit(
            Line::from(vec![
                Span::raw(format!("commit {}", clean(&commit))),
                installed,
                Span::styled("  (s: full SHA)", muted()),
            ]),
            width,
        ),
    ];
    let buttons = app.buttons();
    if !buttons.is_empty() {
        lines.push(bar(&buttons, width));
    }
    if let Some(status) = operation_lines(app, width).into_iter().next() {
        lines.push(fit(status, width));
    }
    lines.push(Line::styled("─".repeat(width), Style::default().fg(MUTED)));
    lines
}

fn notices(app: &DetailsApp, width: usize) -> Vec<Line<'static>> {
    let target = &app.target;
    let mut lines = Vec::new();
    if let ReadmeState::Found { fallback: true, .. } = app.readme {
        let notice = format!(
            "No README in {}/: showing the repository root README",
            clean(&target.source.subdir)
        );
        lines.extend(
            wrap(&notice, width)
                .into_iter()
                .map(|line| Line::styled(line, Style::default().fg(WARN))),
        );
    }
    let status = match (&app.install, &app.removal) {
        (InstallState::Preparing, _) => Some(("Preparing preview…".to_string(), MUTED)),
        (InstallState::UpToDate, _) => Some((
            "Installed: this commit is already installed".to_string(),
            OK,
        )),
        (InstallState::Refused(reason), _) => {
            Some((format!("Install refused: {}", clean(reason)), ERROR))
        }
        (_, RemovalState::Preparing) => Some(("Preparing removal…".to_string(), MUTED)),
        (_, RemovalState::Refused(reason)) => {
            Some((format!("Cannot remove: {}", clean(reason)), ERROR))
        }
        _ => None,
    };
    let update = match &app.update {
        UpdateState::Idle => None,
        UpdateState::Preparing => Some(("Preparing update…".to_string(), MUTED)),
        UpdateState::Refused(reason) => Some((format!("Update refused: {}", clean(reason)), ERROR)),
    };
    for (status, color) in status.into_iter().chain(update) {
        lines.extend(
            wrap(&status, width)
                .into_iter()
                .map(|line| Line::styled(line, Style::default().fg(color))),
        );
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

/// State of the latest operation on this source: running, succeeded,
/// failed, unconfirmed or refused. Herdr's output is cleaned and cut.
fn operation_lines(app: &DetailsApp, width: usize) -> Vec<Line<'static>> {
    if let Some((_, kind)) = &app.launched {
        let text = match kind {
            OperationKind::Install => "Installing…",
            OperationKind::Update => "Updating…",
            OperationKind::Uninstall => "Removing…",
        };
        return vec![Line::styled(text, Style::default().fg(WARN))];
    }
    let Some(record) = &app.operation else {
        return Vec::new();
    };
    let sha = short(&record.request.commit);
    // An update is named by the version of the manifest it confirmed.
    let manifest = match &record.request.confirmation {
        Some(Confirmation::Install { manifest, .. }) => Some(manifest),
        _ => None,
    };
    let version = manifest.map_or_else(|| sha.clone(), |manifest| clean(&manifest.version));
    let registry = record
        .registry_after
        .as_deref()
        .map(|state| format!("Registry: {}", clean(state)));
    let code = record
        .exit_code
        .map_or("no exit code".to_string(), |code| format!("code {code}"));
    let (headline, color, details) = match (record.status, record.request.kind) {
        (Status::Running, OperationKind::Install) => (format!("Installing {sha}…"), WARN, None),
        (Status::Running, OperationKind::Update) => (format!("Updating to {version}…"), WARN, None),
        (Status::Running, OperationKind::Uninstall) => ("Removing…".to_string(), WARN, None),
        (Status::Succeeded, OperationKind::Install) => {
            (format!("Install of {sha} succeeded"), OK, None)
        }
        (Status::Succeeded, OperationKind::Update) => {
            // Herdr relaunches no pane: the new version runs once reopened.
            let reopen = if manifest.is_some_and(|manifest| manifest.id == PLUGIN_ID) {
                "Close and reopen the marketplace to use it."
            } else {
                "Reopen its panes to use it."
            };
            (
                format!("Update to {version} succeeded"),
                OK,
                Some(reopen.to_string()),
            )
        }
        (Status::Succeeded, OperationKind::Uninstall) => {
            ("Removal succeeded".to_string(), OK, registry)
        }
        (Status::Failed, OperationKind::Install) => {
            (format!("Install of {sha} failed ({code})"), ERROR, registry)
        }
        (Status::Failed, OperationKind::Update) => (
            format!("Update to {version} failed ({code})"),
            ERROR,
            registry,
        ),
        (Status::Failed, OperationKind::Uninstall) => {
            (format!("Removal failed ({code})"), ERROR, registry)
        }
        (Status::Unconfirmed, OperationKind::Install) => (
            format!("Install of {sha}: result not confirmed"),
            WARN,
            registry,
        ),
        (Status::Unconfirmed, OperationKind::Update) => (
            format!("Update to {version}: result not confirmed"),
            WARN,
            registry,
        ),
        (Status::Unconfirmed, OperationKind::Uninstall) => {
            ("Removal: result not confirmed".to_string(), WARN, registry)
        }
        (Status::Refused, OperationKind::Install) => {
            (format!("Install of {sha} refused"), ERROR, None)
        }
        (Status::Refused, OperationKind::Update) => {
            (format!("Update to {version} refused"), ERROR, None)
        }
        (Status::Refused, OperationKind::Uninstall) => ("Removal refused".to_string(), ERROR, None),
    };
    let mut lines: Vec<Line<'static>> = wrap(&headline, width)
        .into_iter()
        .map(|line| Line::styled(line, Style::default().fg(color)))
        .collect();
    if let Some(details) = details {
        lines.extend(wrap(&details, width).into_iter().map(Line::raw));
    }
    if record.status != Status::Succeeded && record.status != Status::Running {
        let output: Vec<String> = record
            .output
            .lines()
            .map(clean)
            .filter(|line| !line.trim().is_empty())
            .collect();
        for line in &output {
            lines.extend(
                wrap(line, width)
                    .into_iter()
                    .map(|line| Line::styled(line, muted())),
            );
        }
    }
    if let Some(error) = &record.persistence_error {
        lines.extend(
            wrap(&format!("Could not save result: {}", clean(error)), width)
                .into_iter()
                .map(|line| Line::styled(line, Style::default().fg(ERROR))),
        );
    }
    lines
}

fn short(sha: &str) -> String {
    sha.chars().take(7).collect()
}

/// Drops the spans that would overflow the pane.
fn fit(line: Line<'static>, width: usize) -> Line<'static> {
    let mut used = 0;
    let spans = line
        .spans
        .into_iter()
        .map_while(|span| {
            let room = width.saturating_sub(used);
            if room == 0 {
                return None;
            }
            let content = ellipsize(&span.content, room);
            used += content.width();
            Some(Span::styled(content, span.style))
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}
