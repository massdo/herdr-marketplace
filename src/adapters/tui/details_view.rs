use std::iter;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::animation::Spot;
use super::details::{
    Button, Command, DetailsApp, InstallState, InstalledView, ReadmeState, RemovalState,
    VideoStatus,
};
use super::graphics::without_image;
use super::markdown::{CHIP_BG, CHIP_FG, VideoPlace};
use super::selection::{self, Flow, Selection};
use super::style::{
    ERROR, MUTED, OK, WARN, bold, button_text, ellipsize, ellipsize_middle, megabytes, muted, wrap,
};
use crate::adapters::pane_graphics::Cells;
use crate::domain::operation::{OperationKind, Status};
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
    let video = if showing_readme(app) {
        video_lines(app, prefix.len())
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
        if let Some((_, drawn)) = video.iter().find(|(at, _)| *at == index) {
            lines.push(drawn.clone());
        } else if covered.iter().any(|rows| rows.contains(&index)) {
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
        if let Some(spot) = spot(
            app,
            top,
            page,
            first,
            place.column,
            place.columns,
            place.rows,
        ) {
            spots.push((place.url.clone(), spot));
        }
    }
    spots
}

/// Where the block of the video that plays shows, as `spots` finds the
/// animations; `None` when the body does not show it.
pub fn video_spot(app: &DetailsApp, width: u16, height: u16) -> Option<Spot> {
    let video = app.video.as_ref()?;
    if !showing_readme(app) {
        return None;
    }
    let place = app
        .video_places
        .iter()
        .find(|place| place.url == video.url)?;
    let top = header(app, width as usize)
        .len()
        .min(height.saturating_sub(2) as usize);
    let page = page_rows(app, width, height);
    let (prefix, _, _) = body_lines(app, width as usize);
    let first = prefix.len() + place.line;
    spot(
        app,
        top,
        page,
        first,
        place.column,
        place.columns,
        place.rows,
    )
}

/// The cells of the rows the body shows of a block: body lines
/// `first..first + rows`, `columns` wide from `column`, under `top` rows
/// of header and within `page` rows of body.
fn spot(
    app: &DetailsApp,
    top: usize,
    page: usize,
    first: usize,
    column: usize,
    columns: u16,
    rows: u16,
) -> Option<Spot> {
    let from = app.scroll.max(first);
    let to = (app.scroll + page).min(first + usize::from(rows));
    (from < to).then(|| Spot {
        cells: Cells {
            column: column as u16,
            row: (top + from - app.scroll) as u16,
            columns,
            rows: (to - from) as u16,
        },
        first: (from - first) as u16,
        rows,
    })
}

/// Body lines of the video of the pane, by index, drawn otherwise than in
/// the README: its block left empty while it plays, its button showing the
/// download, or the reason it failed under its button.
fn video_lines(app: &DetailsApp, prefix: usize) -> Vec<(usize, Line<'static>)> {
    let Some(video) = &app.video else {
        return Vec::new();
    };
    let Some(place) = app.video_places.iter().find(|place| place.url == video.url) else {
        return Vec::new();
    };
    let block = place.line..place.line + usize::from(place.rows);
    let Some(lines) = app.lines.get(block.clone()) else {
        return Vec::new();
    };
    let empty = |line: &Line<'static>| Line::from(cells_before(line, place.column));
    // The line without the block, `text` centered in it instead.
    let centered = |line: &Line<'static>, text: &str, style: Style| {
        let mut spans = cells_before(line, place.column);
        let pad = usize::from(place.columns).saturating_sub(text.width()) / 2;
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(Span::styled(text.to_string(), style));
        Line::from(spans)
    };
    let mut drawn = match &video.status {
        VideoStatus::Playing | VideoStatus::Paused | VideoStatus::Buffering { .. } => block
            .clone()
            .zip(lines)
            .map(|(at, line)| (prefix + at, empty(line)))
            .collect(),
        VideoStatus::Ended => Vec::new(),
        VideoStatus::Starting | VideoStatus::Downloading(..) => {
            let progress = match &video.status {
                VideoStatus::Starting => " Starting video… ".into(),
                VideoStatus::Downloading(received, Some(total)) if *total > 0 => {
                    format!(" Downloading video… {}% ", received * 100 / total)
                }
                VideoStatus::Downloading(received, _) => {
                    format!(" Downloading video… {} ", megabytes(*received))
                }
                _ => unreachable!(),
            };
            let chip = Style::default().fg(CHIP_FG).bg(CHIP_BG);
            block
                .clone()
                .zip(lines)
                .map(|(at, line)| {
                    let drawn = if at == place.button_line {
                        centered(line, &progress, chip)
                    } else {
                        empty(line)
                    };
                    (prefix + at, drawn)
                })
                .collect()
        }
        VideoStatus::Failed(reason) => {
            // Under the button, on the first line of the block without its
            // link.
            let free = (place.button_line + 1..block.end)
                .chain(block.start..place.button_line)
                .find(|at| !app.links.iter().any(|area| area.line == *at));
            let Some(at) = free else {
                return Vec::new();
            };
            let text = ellipsize(
                &format!("Could not play the video: {}", clean(reason)),
                usize::from(place.columns),
            );
            let red = Style::default().fg(ERROR);
            vec![(prefix + at, centered(&app.lines[at], &text, red))]
        }
    };
    if !matches!(video.status, VideoStatus::Failed(_)) {
        let bar = control_bar(video, usize::from(place.columns));
        let mut spans = app
            .lines
            .get(block.end)
            .map_or_else(Vec::new, |line| cells_before(line, place.column));
        spans.push(Span::styled(
            bar.text,
            Style::default().fg(CHIP_FG).bg(CHIP_BG),
        ));
        drawn.push((prefix + block.end, Line::from(spans)));
    }
    drawn
}

struct ControlBar {
    text: String,
    buttons: Vec<(std::ops::Range<usize>, Command)>,
    seek: Option<(std::ops::Range<usize>, u64)>,
}

fn control_bar(video: &super::details::VideoView, width: usize) -> ControlBar {
    let (label, command) = match video.status {
        VideoStatus::Starting | VideoStatus::Downloading(..) => ("Cancel", Command::StopVideo),
        VideoStatus::Playing | VideoStatus::Buffering { paused: false } => {
            ("Pause", Command::ToggleVideoPause)
        }
        _ => ("Play", Command::ToggleVideoPause),
    };
    let mut text = format!("[{label}]");
    let mut buttons = vec![(0..text.len(), command)];
    for (label, milliseconds) in [
        ("-5s", video.position.saturating_sub(5000)),
        ("+5s", video.position.saturating_add(5000)),
    ] {
        text.push(' ');
        let start = text.len();
        text.push_str(&format!("[{label}]"));
        buttons.push((start..text.len(), Command::SeekVideo(milliseconds)));
    }
    if matches!(video.status, VideoStatus::Buffering { .. }) {
        text.push_str(" Buffering…");
    }
    let time = |ms: u64| format!("{}:{:02}", ms / 60_000, ms / 1000 % 60);
    text.push_str(&format!(
        " {}/{} ",
        time(video.position),
        video.duration.map_or_else(|| "…".into(), time)
    ));
    let mut seek = None;
    let room = width.saturating_sub(text.width());
    if let Some(duration) = video.duration.filter(|duration| *duration > 0)
        && room >= 5
    {
        text.push('[');
        let start = text.width();
        let length = room - 2;
        let position = (video.position.min(duration) * (length - 1) as u64 / duration) as usize;
        for column in 0..length {
            text.push(if column == position {
                '|'
            } else if column < position {
                '='
            } else {
                '-'
            });
        }
        seek = Some((start..start + length, duration));
        text.push(']');
    }
    ControlBar {
        text: ellipsize(&text, width),
        buttons,
        seek,
    }
}

/// The cells of `line` left of `column`.
fn cells_before(line: &Line<'static>, column: usize) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut used = 0;
    'spans: for span in &line.spans {
        let mut text = String::new();
        for ch in span.content.chars() {
            let width = ch.width().unwrap_or(0);
            let past = used + width > column;
            if !past {
                text.push(ch);
                used += width;
            }
            if past || used == column {
                spans.push(Span::styled(text, span.style));
                break 'spans;
            }
        }
        spans.push(Span::styled(text, span.style));
    }
    spans
}

/// The body shows the README, which no confirmation replaces.
fn showing_readme(app: &DetailsApp) -> bool {
    !app.showing_confirmation() && matches!(app.readme, ReadmeState::Found { .. })
}

/// What a click at `column`, `row` of a `width` × `height` pane runs, laid
/// out as `render` draws it: a button of the action bar, or the commit,
/// which shows the full SHA; in the README, a video or a link.
pub fn hit(app: &DetailsApp, width: u16, height: u16, column: u16, row: u16) -> Option<Command> {
    let shown = header(app, width as usize)
        .len()
        .min(height.saturating_sub(2) as usize);
    let (column, row) = (column as usize, row as usize);
    if row >= shown {
        return video_at(app, width, height, column, row - shown)
            .or_else(|| link_at(app, width, height, column, row - shown));
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

/// What a click on body line `row` does to a video: in the block of the one
/// that plays or downloads, stops it; on a play button, plays its video.
fn video_at(
    app: &DetailsApp,
    width: u16,
    height: u16,
    column: usize,
    row: usize,
) -> Option<Command> {
    if !showing_readme(app) || row >= page_rows(app, width, height) {
        return None;
    }
    let (prefix, _, _) = body_lines(app, width as usize);
    let line = (app.scroll + row).checked_sub(prefix.len())?;
    let within = |place: &VideoPlace| {
        (place.line..place.line + usize::from(place.rows)).contains(&line)
            && (place.column..place.column + usize::from(place.columns)).contains(&column)
    };
    if let Some(video) = &app.video
        && let Some(place) = app.video_places.iter().find(|place| place.url == video.url)
        && line == place.line + usize::from(place.rows)
        && (place.column..place.column + usize::from(place.columns)).contains(&column)
    {
        let relative = column - place.column;
        let bar = control_bar(video, usize::from(place.columns));
        if let Some((range, duration)) = bar.seek
            && range.contains(&relative)
        {
            return Some(Command::SeekVideo(
                duration * (relative - range.start) as u64 / (range.len() - 1) as u64,
            ));
        }
        return bar
            .buttons
            .into_iter()
            .find(|(range, _)| range.contains(&relative))
            .map(|(_, command)| command);
    }
    if let Some(video) = &app.video
        && !matches!(video.status, VideoStatus::Failed(_) | VideoStatus::Ended)
        && app
            .video_places
            .iter()
            .any(|place| place.url == video.url && within(place))
    {
        return Some(
            if matches!(
                video.status,
                VideoStatus::Playing | VideoStatus::Paused | VideoStatus::Buffering { .. }
            ) {
                Command::ToggleVideoPause
            } else {
                Command::StopVideo
            },
        );
    }
    app.video_places
        .iter()
        .position(|place| {
            place.button_line == line && (place.button.0..place.button.1).contains(&column)
        })
        .map(Command::PlayVideo)
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
        InstalledView::At(sha) if *sha == target.commit => {
            Span::styled(" · installed", Style::default().fg(OK))
        }
        InstalledView::At(sha) => Span::styled(
            format!(" · installed at {}", short(sha)),
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
    if let Some((status, color)) = status {
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
            OperationKind::Uninstall => "Removing…",
        };
        return vec![Line::styled(text, Style::default().fg(WARN))];
    }
    let Some(record) = &app.operation else {
        return Vec::new();
    };
    let sha = short(&record.request.commit);
    let install = record.request.kind == OperationKind::Install;
    let registry = record
        .registry_after
        .as_deref()
        .map(|state| format!("Registry: {}", clean(state)));
    let code = record
        .exit_code
        .map_or("no exit code".to_string(), |code| format!("code {code}"));
    let (headline, color, details) = match (record.status, install) {
        (Status::Running, true) => (format!("Installing {sha}…"), WARN, None),
        (Status::Running, false) => ("Removing…".to_string(), WARN, None),
        (Status::Succeeded, true) => (format!("Install of {sha} succeeded"), OK, None),
        (Status::Succeeded, false) => ("Removal succeeded".to_string(), OK, registry),
        (Status::Failed, true) => (format!("Install of {sha} failed ({code})"), ERROR, registry),
        (Status::Failed, false) => (format!("Removal failed ({code})"), ERROR, registry),
        (Status::Unconfirmed, true) => (
            format!("Install of {sha}: result not confirmed"),
            WARN,
            registry,
        ),
        (Status::Unconfirmed, false) => {
            ("Removal: result not confirmed".to_string(), WARN, registry)
        }
        (Status::Refused, true) => (format!("Install of {sha} refused"), ERROR, None),
        (Status::Refused, false) => ("Removal refused".to_string(), ERROR, None),
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
