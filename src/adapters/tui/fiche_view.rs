use ratatui::Frame;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::fiche::{FicheApp, InstallState, ReadmeState};
use super::style::{ERROR, MUTED, OK, WARN, bold, ellipsize, ellipsize_middle, muted, wrap};
use crate::domain::text::clean;

const FOOTER: &str =
    "i : installer · s : SHA complet · Échap : fermer · ↑↓ PgPréc PgSuiv Début Fin";
const PREVIEW_FOOTER: &str = "Entrée : confirmer · Échap : annuler · ↑↓ PgPréc PgSuiv";

/// README lines a `width` × `height` pane shows.
pub fn page_rows(app: &FicheApp, width: u16, height: u16) -> usize {
    (height as usize).saturating_sub(header(app, width as usize).len() + 1)
}

pub fn render(frame: &mut Frame, app: &FicheApp) {
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
        ReadmeState::Loading => vec![Line::styled("Chargement du README…", muted())],
        ReadmeState::NotFound => vec![Line::styled(
            "README.md introuvable",
            Style::default().fg(WARN),
        )],
        ReadmeState::NetworkError(error) => {
            let mut lines = vec![Line::styled("Erreur réseau", Style::default().fg(ERROR))];
            lines.extend(
                wrap(&clean(error), width)
                    .into_iter()
                    .map(|line| Line::styled(line, muted())),
            );
            lines.push(Line::default());
            lines.push(Line::styled("Entrée : réessayer", bold()));
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
    let footer = if app.showing_preview() {
        PREVIEW_FOOTER
    } else {
        FOOTER
    };
    lines.push(Line::styled(ellipsize(footer, width), muted()));
    frame.render_widget(Paragraph::new(lines), area);
}

fn header(app: &FicheApp, width: usize) -> Vec<Line<'static>> {
    let target = &app.target;
    let mut title = vec![Span::styled(clean(&target.name), bold())];
    if let Some(version) = &target.version {
        title.push(Span::styled(format!("  {}", clean(version)), muted()));
    }
    if !target.compatible {
        title.push(Span::styled("  incompatible", Style::default().fg(WARN)));
    }
    if !target.in_catalog {
        title.push(Span::styled("  hors catalogue", Style::default().fg(WARN)));
    }
    let commit = if app.full_sha {
        target.commit.clone()
    } else {
        target.commit.chars().take(7).collect()
    };
    let mut lines = vec![
        fit(Line::from(title), width),
        Line::styled(
            ellipsize_middle(&clean(&target.source.to_string()), width),
            muted(),
        ),
        Line::from(vec![
            Span::raw(format!("commit {}", clean(&commit))),
            Span::styled("  (s : SHA complet)", muted()),
        ]),
    ];
    if let ReadmeState::Found { fallback: true, .. } = app.readme {
        let notice = format!(
            "Pas de README.md dans {}/ : README.md racine du dépôt affiché",
            clean(&target.source.subdir)
        );
        lines.extend(
            wrap(&notice, width)
                .into_iter()
                .map(|line| Line::styled(line, Style::default().fg(WARN))),
        );
    }
    let status = match &app.install {
        InstallState::Idle | InstallState::Preview(_) => None,
        InstallState::Preparing => Some(("Préparation de l'aperçu…".to_string(), MUTED)),
        InstallState::UpToDate => Some(("Installé : ce commit est déjà installé".to_string(), OK)),
        InstallState::Refused(reason) => {
            Some((format!("Installation refusée : {}", clean(reason)), ERROR))
        }
    };
    if let Some((status, color)) = status {
        lines.extend(
            wrap(&status, width)
                .into_iter()
                .map(|line| Line::styled(line, Style::default().fg(color))),
        );
    }
    lines.push(Line::styled("─".repeat(width), Style::default().fg(MUTED)));
    lines
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
