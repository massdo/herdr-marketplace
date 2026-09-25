//! Sidebar keyboard and rendering, offline.

mod support;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use herdr_marketplace::adapters::tui::sidebar::{Intent, LoadState, SidebarApp};
use herdr_marketplace::adapters::tui::sidebar_view;
use herdr_marketplace::application::load_catalog::LoadedCatalog;
use herdr_marketplace::application::load_listing::LoadedListing;
use herdr_marketplace::domain::compat::Platform;
use herdr_marketplace::domain::registry::parse_registry;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::json;
use support::*;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn type_text(app: &mut SidebarApp, text: &str) {
    for ch in text.chars() {
        assert!(!app.handle_key(key(KeyCode::Char(ch))));
    }
}

/// Twelve plugins, the last one with the fewest stars.
fn loaded_app() -> SidebarApp {
    let repos = (0..12)
        .map(|index| {
            repo(
                "acme",
                &format!("plugin-{index:02}"),
                100 - index,
                vec![manifest(
                    "herdr-plugin.toml",
                    &format!("acme.plugin-{index:02}"),
                )],
            )
        })
        .collect();
    ready(repos, vec![])
}

fn ready(repos: Vec<serde_json::Value>, installed: Vec<serde_json::Value>) -> SidebarApp {
    let installed = parse_registry(&registry(installed));
    let loaded = LoadedCatalog {
        catalog: catalog(repos),
        herdr: HERDR,
    };
    let mut app = SidebarApp::new();
    app.intents.clear();
    app.set_page(3);
    app.loaded(Ok(LoadedListing::new(loaded, installed, Platform::Macos)));
    app
}

fn selected_repo(app: &SidebarApp) -> String {
    app.selected_row().unwrap().entry.source.repo.clone()
}

#[test]
fn the_catalogue_is_requested_once_when_the_sidebar_opens() {
    let app = SidebarApp::new();
    assert_eq!(app.state, LoadState::Loading);
    assert_eq!(app.intents, [Intent::Load]);
}

#[test]
fn typing_never_reloads_the_catalogue() {
    let mut app = loaded_app();
    type_text(&mut app, "plugin-1");
    app.handle_key(key(KeyCode::Backspace));
    app.handle_key(key(KeyCode::Down));
    type_text(&mut app, "zz");
    assert!(app.intents.is_empty(), "{:?}", app.intents);
}

#[test]
fn typed_letters_go_to_the_search_even_j_and_k() {
    let mut app = loaded_app();
    type_text(&mut app, "j");
    assert_eq!(app.query, "j");
    assert!(app.visible.is_empty());
    type_text(&mut app, "k");
    assert_eq!(app.query, "jk");
    app.handle_key(key(KeyCode::Backspace));
    app.handle_key(key(KeyCode::Backspace));
    assert_eq!(app.visible.len(), 12);
    assert_eq!(selected_repo(&app), "plugin-00");
}

#[test]
fn a_search_filters_the_list_and_the_counter() {
    let mut app = loaded_app();
    type_text(&mut app, "PLUGIN-1");
    let repos: Vec<&str> = app
        .visible
        .iter()
        .map(|&index| app.rows()[index].entry.source.repo.as_str())
        .collect();
    assert_eq!(repos, ["plugin-10", "plugin-11"]);
    assert_eq!(selected_repo(&app), "plugin-10");
}

#[test]
fn arrows_move_the_selection_down_to_the_last_entry() {
    let mut app = loaded_app();
    assert_eq!(selected_repo(&app), "plugin-00");
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Down));
    assert_eq!(selected_repo(&app), "plugin-02");
    app.handle_key(key(KeyCode::Up));
    assert_eq!(selected_repo(&app), "plugin-01");
    for _ in 0..20 {
        app.handle_key(key(KeyCode::Down));
    }
    assert_eq!(selected_repo(&app), "plugin-11");
    assert_eq!(app.offset, 9, "the last entry is on screen");
    app.handle_key(key(KeyCode::Home));
    assert_eq!(selected_repo(&app), "plugin-00");
    assert_eq!(app.offset, 0);
    app.handle_key(key(KeyCode::End));
    assert_eq!(selected_repo(&app), "plugin-11");
    app.handle_key(key(KeyCode::PageUp));
    assert_eq!(selected_repo(&app), "plugin-08");
    app.handle_key(key(KeyCode::PageDown));
    assert_eq!(selected_repo(&app), "plugin-11");
}

#[test]
fn the_selection_follows_the_identity_not_the_id() {
    let mut app = ready(
        vec![
            repo(
                "one",
                "twin",
                5,
                vec![manifest("herdr-plugin.toml", "same.id")],
            ),
            repo(
                "two",
                "twin",
                4,
                vec![manifest("herdr-plugin.toml", "same.id")],
            ),
            repo(
                "acme",
                "other",
                3,
                vec![manifest("herdr-plugin.toml", "other")],
            ),
        ],
        vec![],
    );
    assert_eq!(app.visible.len(), 3, "same id, two rows");
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.selected_row().unwrap().entry.source.owner, "two");
    type_text(&mut app, "same");
    assert_eq!(app.visible.len(), 2);
    assert_eq!(app.selected_row().unwrap().entry.source.owner, "two");
}

#[test]
fn a_failed_load_is_retried_with_enter() {
    let mut app = SidebarApp::new();
    app.intents.clear();
    app.loaded(Err(
        "téléchargement de l'index impossible : introuvable".into()
    ));
    assert!(matches!(app.state, LoadState::Failed(_)));
    type_text(&mut app, "x");
    assert!(app.intents.is_empty());
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.state, LoadState::Loading);
    assert_eq!(app.intents, [Intent::Load]);
}

#[test]
fn enter_asks_for_the_fiche_of_the_selected_plugin() {
    let mut app = loaded_app();
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter));
    match app.intents.as_slice() {
        [Intent::Open(row)] => assert_eq!(row.entry.source.repo, "plugin-01"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn escape_clears_the_search_then_closes() {
    let mut app = loaded_app();
    type_text(&mut app, "plugin-11");
    assert!(!app.handle_key(key(KeyCode::Esc)));
    assert_eq!(app.query, "");
    assert_eq!(app.visible.len(), 12);
    assert!(app.handle_key(key(KeyCode::Esc)));
}

#[test]
fn a_row_shows_the_name_owner_repo_marks_and_description() {
    let mut installed_linux = github_plugin(
        "martinro.next-agent",
        "martin-ro",
        "herdr-next-agent",
        None,
        SHA_A,
    );
    installed_linux["platforms"] = json!(["linux"]);
    let mut next_agent = manifest("herdr-plugin.toml", "martinro.next-agent");
    next_agent["platforms"] = json!(["linux"]);
    next_agent["name"] = json!("Next Agent");
    next_agent["description"] = json!("Jump to the next agent");
    let mut hidden = manifest("herdr-plugin.toml", "hidden");
    hidden["platforms"] = json!(["linux"]);
    let mut browser = manifest(
        "herdr-plugin/herdr-plugin.toml",
        "zenbu-labs.terminal-browser",
    );
    browser["name"] = json!("Terminal Browser");
    browser["description"] = json!("Open a browser inside herdr\u{1b}[31m");
    let mut app = ready(
        vec![
            repo("zenbu-labs", "terminal-browser", 3403, vec![browser]),
            repo("martin-ro", "herdr-next-agent", 12, vec![next_agent]),
            repo("someone", "hidden", 1, vec![hidden]),
        ],
        vec![installed_linux],
    );
    app.set_page(2);

    let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
    terminal
        .draw(|frame| sidebar_view::render(frame, &app))
        .unwrap();
    let text: Vec<String> = terminal
        .backend()
        .buffer()
        .content()
        .chunks(40)
        .map(|line| line.iter().map(|cell| cell.symbol()).collect())
        .collect();
    let screen = text.join("\n");

    assert!(text[1].starts_with("2 résultats"), "{screen}");
    assert!(text[2].starts_with("1 incompatible masqué"), "{screen}");
    assert!(
        text[3].starts_with("Terminal Browser") && text[3].contains("★ 3403"),
        "{screen}"
    );
    assert!(
        text[4].starts_with("zenbu-labs/terminal-browser/herdr-plugin"),
        "{screen}"
    );
    assert!(
        text[5].starts_with("Open a browser inside herdr[31m"),
        "{screen}"
    );
    assert!(text[6].starts_with("Next Agent"), "{screen}");
    assert!(
        text[7].starts_with("martin-ro/herdr-next-agent"),
        "{screen}"
    );
    assert!(
        text[8].starts_with("installé · incompatible · Jump"),
        "{screen}"
    );
    assert!(!screen.contains('\u{1b}'), "{screen:?}");
}
