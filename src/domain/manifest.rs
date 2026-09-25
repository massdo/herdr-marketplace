//! `herdr-plugin.toml` read with the rules of Herdr 0.9.1
//! (`app/api/plugins/manifest.rs`), for the install preview.

use std::collections::HashSet;

use serde::Deserialize;

use super::version::Version;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub min_herdr_version: String,
    pub platforms: Option<Vec<String>>,
    pub build: Vec<Step>,
    pub startup: Vec<Step>,
    pub actions: Vec<Hook>,
    pub events: Vec<Hook>,
    pub panes: Vec<Hook>,
    pub link_handlers: Vec<String>,
}

/// A build or startup command, limited to some platforms or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub command: Vec<String>,
    pub platforms: Option<Vec<String>>,
}

/// An action, an event hook or a pane: its id (or event name) and command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hook {
    pub name: String,
    pub command: Vec<String>,
}

#[derive(Deserialize)]
struct RawManifest {
    id: String,
    name: String,
    version: String,
    min_herdr_version: Option<String>,
    platforms: Option<Vec<String>>,
    #[serde(default)]
    build: Vec<RawStep>,
    #[serde(default)]
    startup: Vec<RawStep>,
    #[serde(default)]
    actions: Vec<RawHook>,
    #[serde(default)]
    events: Vec<RawEvent>,
    #[serde(default)]
    panes: Vec<RawHook>,
    #[serde(default)]
    link_handlers: Vec<RawLinkHandler>,
}

#[derive(Deserialize)]
struct RawStep {
    command: Vec<String>,
    platforms: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct RawHook {
    id: String,
    title: String,
    command: Vec<String>,
}

#[derive(Deserialize)]
struct RawEvent {
    on: String,
    command: Vec<String>,
}

#[derive(Deserialize)]
struct RawLinkHandler {
    id: String,
}

pub fn parse_manifest(text: &str) -> Result<Manifest, String> {
    let raw: RawManifest = toml::from_str(text).map_err(|error| error.message().to_string())?;
    let id = raw.id.trim();
    if !is_identifier(id, ".") {
        return Err(format!("id de plugin invalide : {id}"));
    }
    let min_herdr_version = raw
        .min_herdr_version
        .map(|value| value.trim().to_string())
        .filter(|value| Version::parse(value).is_some())
        .ok_or("min_herdr_version absente ou illisible")?;
    Ok(Manifest {
        id: id.to_string(),
        name: required(&raw.name, "name")?,
        version: required(&raw.version, "version")?,
        min_herdr_version,
        platforms: platforms(raw.platforms)?,
        build: raw.build.into_iter().map(step).collect::<Result<_, _>>()?,
        startup: raw
            .startup
            .into_iter()
            .map(step)
            .collect::<Result<_, _>>()?,
        actions: hooks(raw.actions, "action")?,
        events: raw
            .events
            .into_iter()
            .map(|event| {
                Ok(Hook {
                    name: required(&event.on, "on")?,
                    command: command(event.command)?,
                })
            })
            .collect::<Result<_, String>>()?,
        panes: hooks(raw.panes, "pane")?,
        link_handlers: raw
            .link_handlers
            .into_iter()
            .map(|handler| handler.id)
            .collect(),
    })
}

fn hooks(raw: Vec<RawHook>, kind: &str) -> Result<Vec<Hook>, String> {
    let mut seen = HashSet::new();
    raw.into_iter()
        .map(|hook| {
            let id = hook.id.trim().to_string();
            if !is_identifier(&id, "") {
                return Err(format!("id de {kind} invalide : {id}"));
            }
            if !seen.insert(id.clone()) {
                return Err(format!("id de {kind} en double : {id}"));
            }
            required(&hook.title, "title")?;
            Ok(Hook {
                name: id,
                command: command(hook.command)?,
            })
        })
        .collect()
}

fn step(raw: RawStep) -> Result<Step, String> {
    Ok(Step {
        command: command(raw.command)?,
        platforms: platforms(raw.platforms)?,
    })
}

fn command(argv: Vec<String>) -> Result<Vec<String>, String> {
    if argv.is_empty() || argv.iter().any(String::is_empty) {
        return Err("une commande doit contenir des arguments non vides".into());
    }
    Ok(argv)
}

fn platforms(raw: Option<Vec<String>>) -> Result<Option<Vec<String>>, String> {
    match raw {
        Some(list) if list.is_empty() => Err("platforms ne doit pas être vide".into()),
        Some(list) => match list
            .iter()
            .find(|name| !matches!(name.as_str(), "linux" | "macos" | "windows"))
        {
            Some(unknown) => Err(format!("plateforme inconnue : {unknown}")),
            None => Ok(Some(list)),
        },
        None => Ok(None),
    }
}

fn required(value: &str, field: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{field} manquant"));
    }
    Ok(value.to_string())
}

/// Herdr identifiers: ASCII letters, digits, `:`, `_`, `-` and the `extra`
/// characters, at most 120 of them.
fn is_identifier(value: &str, extra: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= 120
        && value.chars().all(|ch| {
            ch.is_ascii_alphanumeric() || matches!(ch, ':' | '_' | '-') || extra.contains(ch)
        })
}
