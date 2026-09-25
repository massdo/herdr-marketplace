use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::style::{ACCENT, WARN, bold, muted, wrap};
use crate::application::prepare_install::InstallPreview;
use crate::domain::compat::Platform;
use crate::domain::install::Plan;
use crate::domain::manifest::Step;
use crate::domain::text::clean;

/// At least what Herdr's own preview prints, every third-party text cleaned
/// of control characters and wrapped to `width`.
pub fn preview_lines(preview: &InstallPreview, width: usize, host: Platform) -> Vec<Line<'static>> {
    let manifest = &preview.manifest;
    let title = match preview.plan {
        Plan::Install => "Installation",
        Plan::Switch { .. } => "Changement de commit",
    };
    let mut lines = vec![Line::styled(
        title,
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
    )];
    let mut field = |label: &str, value: &str| {
        lines.extend(
            wrap(&format!("{label} : {}", clean(value)), width)
                .into_iter()
                .map(Line::raw),
        );
    };
    field("id", &manifest.id);
    field("nom", &manifest.name);
    field("version", &manifest.version);
    field("source", &preview.source.to_string());
    field("commit", &preview.commit);
    if let Plan::Switch { from } = &preview.plan {
        field("commit installé", from);
    }
    if let Some(replaced) = &preview.replaces {
        let source = replaced
            .github_source()
            .map(ToString::to_string)
            .unwrap_or_default();
        let commit = replaced.resolved_commit().unwrap_or_default();
        field(
            "remplace",
            &format!("{} depuis {source} @ {commit}", replaced.plugin_id),
        );
    }

    let plugin_platforms = manifest.platforms.as_deref();
    let build: Vec<String> = manifest
        .build
        .iter()
        .map(|step| {
            let skipped = !runs_on(step, plugin_platforms, host);
            let note = if skipped {
                format!(" (ignorée sur {})", host.name())
            } else {
                String::new()
            };
            format!("{}{note}", step.command.join(" "))
        })
        .collect();
    let startup: Vec<String> = manifest
        .startup
        .iter()
        .map(|step| step.command.join(" "))
        .collect();
    let hooks = |hooks: &[crate::domain::manifest::Hook]| -> Vec<String> {
        hooks
            .iter()
            .map(|hook| format!("{} : {}", hook.name, hook.command.join(" ")))
            .collect()
    };
    section(&mut lines, "commandes de build", &build, width);
    section(&mut lines, "commandes de démarrage", &startup, width);
    section(&mut lines, "événements", &hooks(&manifest.events), width);
    section(&mut lines, "actions", &hooks(&manifest.actions), width);
    section(&mut lines, "panes", &hooks(&manifest.panes), width);
    section(
        &mut lines,
        "gestionnaires de liens",
        &manifest.link_handlers,
        width,
    );

    lines.push(Line::default());
    for warning in [
        "Ce plugin exécutera du code avec vos droits.",
        "Le SHA fige le dépôt, pas ce que le build télécharge.",
    ] {
        lines.extend(
            wrap(warning, width)
                .into_iter()
                .map(|line| Line::styled(line, Style::default().fg(WARN))),
        );
    }
    lines
}

fn section(lines: &mut Vec<Line<'static>>, label: &str, items: &[String], width: usize) {
    lines.push(Line::from(vec![
        Span::styled(format!("{label} "), bold()),
        Span::styled(format!("({})", items.len()), muted()),
    ]));
    for item in items {
        for (index, line) in wrap(&clean(item), width.saturating_sub(4))
            .into_iter()
            .enumerate()
        {
            let indent = if index == 0 { "  • " } else { "    " };
            lines.push(Line::raw(format!("{indent}{line}")));
        }
    }
}

/// Herdr's `build_platform_supported`: the step's platforms, else the
/// plugin's; undeclared means everywhere.
fn runs_on(step: &Step, plugin_platforms: Option<&[String]>, host: Platform) -> bool {
    step.platforms
        .as_deref()
        .or(plugin_platforms)
        .is_none_or(|platforms| platforms.iter().any(|name| name == host.name()))
}
