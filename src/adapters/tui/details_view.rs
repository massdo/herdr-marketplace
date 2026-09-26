use ratatui::Frame;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::details::{DetailsApp, InstallState, InstalledView, ReadmeState, RemovalState};
use super::style::{ERROR, MUTED, OK, WARN, bold, ellipsize, ellipsize_middle, muted, wrap};
use crate::domain::operation::{OperationKind, Status};
use crate::domain::text::clean;

const FOOTER: &str = "i: install · r: remove · s: full SHA · Esc: close · ↑↓ PgUp PgDn Home End";
const PREVIEW_FOOTER: &str = "Enter: confirm · Esc: cancel · ↑↓ PgUp PgDn";

/// README lines a `width` × `height` pane shows.
pub fn page_rows(app: &DetailsApp, width: u16, height: u16) -> usize {
    (height as usize).saturating_sub(header(app, width as usize).len() + 1)
}

pub fn render(frame: &mut Frame, app: &DetailsApp) {
    let area = frame.area();
    let width = area.width as usize;
    let mut lines = header(app, width);
    let body_height = (area.height as usize).saturating_sub(lines.len() + 1);
    let mut body = match &app.readme {
        _ if app.showing_preview() => app
            .preview
            .iter()
            .skip(app.preview_scroll)
            .take(body_height)
            .cloned()
            .collect(),
        _ if matches!(app.removal, RemovalState::Confirm(_)) => removal_lines(app, width),
        ReadmeState::Loading => vec![Line::styled("Loading README…", muted())],
        ReadmeState::NotFound => vec![Line::styled(
            "README.md not found",
            Style::default().fg(WARN),
        )],
        ReadmeState::NetworkError(error) => {
            let mut lines = vec![Line::styled("Network error", Style::default().fg(ERROR))];
            lines.extend(
                wrap(&clean(error), width)
                    .into_iter()
                    .map(|line| Line::styled(line, muted())),
            );
            lines.push(Line::default());
            lines.push(Line::styled("Enter: retry", bold()));
            lines
        }
        ReadmeState::Found { .. } => app
            .lines
            .iter()
            .skip(app.scroll)
            .take(body_height)
            .cloned()
            .collect(),
    };
    body.truncate(body_height);
    body.resize(body_height, Line::default());
    lines.extend(body);
    let footer = if app.showing_preview() || matches!(app.removal, RemovalState::Confirm(_)) {
        PREVIEW_FOOTER
    } else {
        FOOTER
    };
    lines.push(Line::styled(ellipsize(footer, width), muted()));
    frame.render_widget(Paragraph::new(lines), area);
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
    let mut lines = vec![Line::styled("Remove", bold().fg(ERROR))];
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
    if let ReadmeState::Found { fallback: true, .. } = app.readme {
        let notice = format!(
            "No README.md in {}/: showing the repository root README.md",
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
    lines.extend(operation_lines(app, width));
    lines.push(Line::styled("─".repeat(width), Style::default().fg(MUTED)));
    lines
}

/// Last output lines of a failed or unconfirmed operation.
const OUTPUT_LINES: usize = 8;

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
        let start = output.len().saturating_sub(OUTPUT_LINES);
        for line in &output[start..] {
            lines.extend(
                wrap(line, width)
                    .into_iter()
                    .map(|line| Line::styled(line, muted())),
            );
        }
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
