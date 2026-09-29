//! Sidebar keyboard and rendering, offline.

mod support;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use herdr_marketplace::adapters::tui::sidebar::{Counts, Filter, Intent, LoadState, SidebarApp};
use herdr_marketplace::adapters::tui::sidebar_view;
use herdr_marketplace::application::load_catalog::LoadedCatalog;
use herdr_marketplace::application::load_listing::LoadedListing;
use herdr_marketplace::application::open_details::Reveal;
use herdr_marketplace::domain::compat::Platform;
use herdr_marketplace::domain::registry::parse_registry;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use serde_json::json;
use support::*;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn click(column: u16, row: u16) -> MouseEvent {
    mouse(MouseEventKind::Down(MouseButton::Left), column, row)
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
        not_refreshed: false,
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
    assert!(!app.intents.contains(&Intent::Load), "{:?}", app.intents);
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

fn visible_repos(app: &SidebarApp) -> Vec<&str> {
    app.visible
        .iter()
        .map(|&index| app.rows()[index].entry.source.repo.as_str())
        .collect()
}

#[test]
fn a_search_filters_and_ranks_the_list() {
    let mut app = loaded_app();
    type_text(&mut app, "PLUGIN-1");
    assert_eq!(
        visible_repos(&app),
        ["plugin-10", "plugin-11", "plugin-01"],
        "the word as typed first, then with a letter in between"
    );
    assert_eq!(selected_repo(&app), "plugin-10");
}

#[test]
fn a_new_search_selects_its_most_relevant_result() {
    let mut app = loaded_app();
    app.handle_key(key(KeyCode::End));
    assert_eq!(selected_repo(&app), "plugin-11");
    type_text(&mut app, "plugin-0");
    assert_eq!(selected_repo(&app), "plugin-00", "not the old selection");
    assert_eq!(app.offset, 0);
}

#[test]
fn the_hidden_count_follows_the_search() {
    let mut linux = manifest("herdr-plugin.toml", "someone.linux-tool");
    linux["platforms"] = json!(["linux"]);
    let mut other = manifest("herdr-plugin.toml", "someone.other-tool");
    other["platforms"] = json!(["linux"]);
    let mut app = ready(
        vec![
            repo(
                "acme",
                "plugin",
                3,
                vec![manifest("herdr-plugin.toml", "acme.plugin")],
            ),
            repo("someone", "linux-tool", 2, vec![linux]),
            repo("someone", "other-tool", 1, vec![other]),
        ],
        vec![],
    );
    assert_eq!(app.hidden, 2);
    type_text(&mut app, "linux");
    assert_eq!(app.hidden, 1);
    assert!(app.visible.is_empty());
    type_text(&mut app, "zzz");
    assert_eq!(app.hidden, 0);
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
    app.registry_refreshed(Ok(vec![]), Platform::Macos);
    assert_eq!(
        app.selected_row().unwrap().entry.source.owner,
        "two",
        "rows rebuilt after an operation keep the selection"
    );
}

#[test]
fn a_failed_load_is_retried_with_enter() {
    let mut app = SidebarApp::new();
    app.intents.clear();
    app.loaded(Err("cannot download the index: not found".into()));
    assert!(matches!(app.state, LoadState::Failed(_)));
    type_text(&mut app, "x");
    assert!(app.intents.is_empty());
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.state, LoadState::Loading);
    assert_eq!(app.intents, [Intent::Load]);
}

#[test]
fn enter_asks_for_the_details_of_the_selected_plugin() {
    let mut app = loaded_app();
    app.handle_key(key(KeyCode::Down));
    app.intents.clear();
    app.handle_key(key(KeyCode::Enter));
    match app.intents.as_slice() {
        [Intent::Open(row, Reveal::Focus)] => assert_eq!(row.entry.source.repo, "plugin-01"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn moving_with_the_keyboard_previews_the_selected_plugin() {
    let mut app = loaded_app();
    let previewed = |app: &mut SidebarApp| match std::mem::take(&mut app.intents).as_slice() {
        [Intent::Preview(row)] => row.entry.source.repo.clone(),
        other => panic!("{other:?}"),
    };
    app.handle_key(key(KeyCode::Down));
    assert_eq!(previewed(&mut app), "plugin-01");
    app.handle_key(key(KeyCode::PageDown));
    assert_eq!(previewed(&mut app), "plugin-04");
    app.handle_key(key(KeyCode::End));
    assert_eq!(previewed(&mut app), "plugin-11");
    app.handle_key(key(KeyCode::Home));
    assert_eq!(previewed(&mut app), "plugin-00");
    app.handle_key(key(KeyCode::Up));
    assert_eq!(
        previewed(&mut app),
        "plugin-00",
        "the first plugin stays shown"
    );

    type_text(&mut app, "zz");
    app.intents.clear();
    app.handle_key(key(KeyCode::Down));
    assert!(
        app.intents.is_empty(),
        "no plugin to show: {:?}",
        app.intents
    );
}

#[test]
fn a_search_asks_open_details_to_follow_its_first_result() {
    let mut app = loaded_app();
    let followed = |app: &mut SidebarApp| -> Vec<String> {
        std::mem::take(&mut app.intents)
            .into_iter()
            .map(|intent| match intent {
                Intent::Follow(row) => row.entry.source.repo,
                other => panic!("{other:?}"),
            })
            .collect()
    };
    type_text(&mut app, "plugin-1");
    assert_eq!(
        followed(&mut app).last().map(String::as_str),
        Some("plugin-10")
    );
    app.handle_key(key(KeyCode::Backspace));
    assert_eq!(followed(&mut app), ["plugin-00"]);
    app.handle_key(key(KeyCode::Tab));
    assert!(
        followed(&mut app).is_empty(),
        "no installed plugin, nothing to show"
    );
    app.handle_key(key(KeyCode::Esc));
    assert!(
        followed(&mut app).is_empty(),
        "search cleared, still Installed"
    );
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(followed(&mut app), ["plugin-00"], "back to All");
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

    let mut terminal = Terminal::new(TestBackend::new(40, 18)).unwrap();
    terminal
        .draw(|frame| sidebar_view::render(frame, &app))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let text: Vec<String> = buffer
        .content()
        .chunks(40)
        .map(|line| line.iter().map(|cell| cell.symbol()).collect())
        .collect();
    let screen = text.join("\n");

    assert!(text[0].starts_with("╭──"), "{screen}");
    assert!(
        text[1].starts_with("│ Search name, topic, author") && text[1].ends_with(" │"),
        "{screen}"
    );
    assert!(text[2].starts_with("╰──"), "{screen}");
    assert!(text[3].starts_with(" All 2   Installed 1 "), "{screen}");
    assert!(
        text[4].starts_with("1 incompatible plugin hidden"),
        "{screen}"
    );
    assert_eq!(text[5], "─".repeat(40), "a line sets the list apart");
    // Each plugin is a card: name and stars on its top edge.
    assert!(
        text[6].starts_with("╭ Terminal Browser ─") && text[6].ends_with(" ★ 3403 ╮"),
        "{screen}"
    );
    assert_eq!(
        buffer[(text[6].chars().position(|ch| ch == '★').unwrap() as u16, 6)].fg,
        herdr_marketplace::adapters::tui::style::GOLD,
        "a golden star"
    );
    assert!(
        text[7].starts_with("│ zenbu-labs/") && text[7].contains("…"),
        "a long owner/repo keeps its ends: {screen}"
    );
    assert!(text[7].ends_with(" │"), "{screen}");
    assert!(
        text[8].starts_with("│ Open a browser inside herdr[31m"),
        "{screen}"
    );
    assert!(
        text[9].starts_with("╰──") && text[9].ends_with("─╯"),
        "{screen}"
    );
    assert!(text[10].starts_with("╭ Next Agent ─"), "{screen}");
    assert!(
        text[11].starts_with("│ martin-ro/herdr-next-agent"),
        "{screen}"
    );
    assert!(
        text[12].starts_with("│ installed · incompatible · Jump"),
        "{screen}"
    );
    assert_eq!(
        buffer[(0, 6)].fg,
        herdr_marketplace::adapters::tui::style::ACCENT,
        "the selected card's frame is blue"
    );
    assert_eq!(
        buffer[(0, 10)].fg,
        herdr_marketplace::adapters::tui::style::CARD,
        "the others are light"
    );
    assert!(!screen.contains('\u{1b}'), "{screen:?}");
}

#[test]
fn one_click_on_a_plugin_selects_it_and_shows_its_details_without_the_focus() {
    let mut app = loaded_app();
    // 40 × 19: search box, filters and separator, 3 cards of 4 lines, the
    // arrow, footer.
    app.set_page(sidebar_view::page_rows(&app, 40, 19));
    assert_eq!(app.page, 3);
    app.handle_mouse(click(5, 5 + 4 + 2), 40, 19);
    assert_eq!(selected_repo(&app), "plugin-01");
    match app.intents.as_slice() {
        [Intent::Open(row, Reveal::Preview)] => assert_eq!(row.entry.source.repo, "plugin-01"),
        other => panic!("{other:?}"),
    }
    app.intents.clear();
    for row in [0, 1, 4, 17, 18] {
        app.handle_mouse(click(5, row), 40, 19);
    }
    assert!(
        app.intents.is_empty(),
        "search, separator, arrow and footer open nothing"
    );
}

#[test]
fn the_wheel_scrolls_the_list_without_moving_the_selection() {
    let mut app = loaded_app();
    app.set_page(sidebar_view::page_rows(&app, 40, 19));
    app.handle_mouse(mouse(MouseEventKind::ScrollDown, 5, 7), 40, 19);
    app.handle_mouse(mouse(MouseEventKind::ScrollDown, 5, 7), 40, 19);
    assert_eq!(app.offset, 2);
    assert_eq!(selected_repo(&app), "plugin-00");
    app.set_page(sidebar_view::page_rows(&app, 40, 19));
    assert_eq!(app.offset, 2, "drawing again keeps the scrolled list");
    app.handle_mouse(click(5, 5), 40, 19);
    assert_eq!(selected_repo(&app), "plugin-02", "the first plugin shown");
    for _ in 0..20 {
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 5, 7), 40, 19);
    }
    assert_eq!(app.offset, 9, "no further than the last page");
    app.handle_mouse(mouse(MouseEventKind::ScrollUp, 5, 7), 40, 19);
    assert_eq!(app.offset, 8);
}

/// The lines of `app` drawn in a 40 × `height` pane, its page set first as
/// the sidebar loop does.
fn drawn(app: &mut SidebarApp, height: u16) -> Vec<String> {
    app.set_page(sidebar_view::page_rows(app, 40, height));
    let mut terminal = Terminal::new(TestBackend::new(40, height)).unwrap();
    terminal
        .draw(|frame| sidebar_view::render(frame, app))
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .chunks(40)
        .map(|line| line.iter().map(|cell| cell.symbol()).collect())
        .collect()
}

#[test]
fn an_arrow_under_the_cards_says_more_plugins_follow() {
    let mut app = loaded_app();
    // 40 × 19: search box, filters and separator, 3 cards, the arrow, footer.
    let text = drawn(&mut app, 19);
    assert_eq!(app.page, 3);
    assert_eq!(text[17].trim(), "↓", "{text:#?}");
    assert_eq!(text[17].find('↓'), Some(20), "centered: {text:#?}");
    // 40 × 21: two lines left under the cards, the arrow sits on the last.
    let text = drawn(&mut app, 21);
    assert_eq!(text[17].trim(), "", "{text:#?}");
    assert_eq!(text[19].trim(), "↓", "just above the footer: {text:#?}");
    app.handle_key(key(KeyCode::End));
    let text = drawn(&mut app, 19);
    assert!(text[16].starts_with('╰'), "{text:#?}");
    assert_eq!(text[17].trim(), "", "the last plugin is shown: {text:#?}");

    // Three plugins fill 12 lines; twelve keep one of them for the arrow.
    let few = (0..3)
        .map(|index| {
            repo(
                "acme",
                &format!("plugin-{index:02}"),
                10,
                vec![manifest(
                    "herdr-plugin.toml",
                    &format!("acme.plugin-{index:02}"),
                )],
            )
        })
        .collect();
    let mut few = ready(few, vec![]);
    let text = drawn(&mut few, 18);
    assert_eq!(few.page, 3);
    assert!(!text.join("\n").contains('↓'), "{text:#?}");
    assert_eq!(sidebar_view::page_rows(&loaded_app(), 40, 18), 2);
}

#[test]
fn a_click_on_retry_loads_the_catalogue_again() {
    let mut app = SidebarApp::new();
    app.intents.clear();
    app.loaded(Err("cannot download the index: not found".into()));
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
    let retry = text
        .iter()
        .position(|line| line.starts_with(" Retry (Enter) "))
        .unwrap_or_else(|| panic!("{text:#?}"));
    app.handle_mouse(click(30, retry as u16), 40, 12);
    assert!(app.intents.is_empty(), "beside the button");
    app.handle_mouse(click(3, retry as u16), 40, 12);
    assert_eq!(app.state, LoadState::Loading);
    assert_eq!(app.intents, [Intent::Load]);
}

/// Twelve plugins, the fourth and the tenth installed.
fn installed_app() -> SidebarApp {
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
    ready(
        repos,
        vec![
            github_plugin("acme.plugin-03", "acme", "plugin-03", None, SHA_A),
            github_plugin("acme.plugin-09", "acme", "plugin-09", None, SHA_A),
        ],
    )
}

#[test]
fn the_installed_filter_shows_only_installed_plugins() {
    let mut app = installed_app();
    assert_eq!(
        app.counts,
        Counts {
            all: 12,
            installed: 2
        }
    );
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.filter, Filter::Installed);
    assert_eq!(visible_repos(&app), ["plugin-03", "plugin-09"]);
    assert_eq!(
        selected_repo(&app),
        "plugin-03",
        "starts from its first plugin"
    );
    type_text(&mut app, "09");
    assert_eq!(visible_repos(&app), ["plugin-09"]);
    assert_eq!(
        app.counts,
        Counts {
            all: 1,
            installed: 1
        },
        "counts follow the search"
    );
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.filter, Filter::All);
}

#[test]
fn at_installed_typed_in_the_search_switches_the_filter_as_in_vs_code() {
    let mut app = installed_app();
    type_text(&mut app, "@installed");
    assert_eq!(app.filter, Filter::Installed);
    assert_eq!(app.query, "", "the word leaves the search");
    type_text(&mut app, " 03");
    assert_eq!(visible_repos(&app), ["plugin-03"]);
}

#[test]
fn a_click_on_a_filter_tab_switches_it() {
    let mut app = installed_app();
    app.set_page(sidebar_view::page_rows(&app, 40, 15));
    // " All 12   Installed 2 ": the second tab starts at column 9.
    app.handle_mouse(click(12, 3), 40, 15);
    assert_eq!(app.filter, Filter::Installed);
    app.handle_mouse(click(2, 3), 40, 15);
    assert_eq!(app.filter, Filter::All);
    assert!(
        app.intents
            .iter()
            .all(|intent| matches!(intent, Intent::Follow(_))),
        "{:?}",
        app.intents
    );
}

#[test]
fn escape_clears_the_search_then_the_filter_then_closes() {
    let mut app = installed_app();
    app.handle_key(key(KeyCode::Tab));
    type_text(&mut app, "03");
    assert!(!app.handle_key(key(KeyCode::Esc)));
    assert_eq!((app.query.as_str(), app.filter), ("", Filter::Installed));
    assert!(!app.handle_key(key(KeyCode::Esc)));
    assert_eq!(app.filter, Filter::All);
    assert!(app.handle_key(key(KeyCode::Esc)));
}

#[test]
fn the_search_box_empties_with_its_cross_and_shows_the_focus() {
    let mut app = installed_app();
    type_text(&mut app, "plugin-1");
    let draw = |app: &SidebarApp| {
        let mut terminal = Terminal::new(TestBackend::new(30, 12)).unwrap();
        terminal
            .draw(|frame| sidebar_view::render(frame, app))
            .unwrap();
        terminal.backend().buffer().clone()
    };
    let buffer = draw(&app);
    let middle: String = (0..30)
        .map(|x| buffer[(x, 1)].symbol().to_string())
        .collect();
    assert_eq!(middle, "│ plugin-1▏                × │");
    assert_eq!(
        buffer[(0, 0)].fg,
        herdr_marketplace::adapters::tui::style::ACCENT
    );
    app.focus(false);
    let buffer = draw(&app);
    assert_eq!(
        buffer[(0, 0)].fg,
        herdr_marketplace::adapters::tui::style::MUTED
    );
    app.handle_mouse(click(26, 1), 30, 12);
    assert_eq!(app.query, "");
    assert_eq!(app.visible.len(), 12);
}

#[test]
fn all_status_marks_fit_inside_a_narrow_card() {
    let mut plugin = github_plugin("missing", "acme", "missing", None, SHA_A);
    plugin["platforms"] = json!(["linux"]);
    let mut app = ready(vec![], vec![plugin]);
    app.set_page(sidebar_view::page_rows(&app, 32, 15));
    let mut terminal = Terminal::new(TestBackend::new(32, 15)).unwrap();
    terminal
        .draw(|frame| sidebar_view::render(frame, &app))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let rows = buffer
        .content()
        .chunks(32)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>();
    let marks = rows
        .iter()
        .find(|line| line.starts_with('│') && line.contains("installed"))
        .expect("installed mark");
    assert!(marks.contains("incompatible"), "{rows:#?}");
    assert!(
        marks.contains('…'),
        "the final mark is visibly truncated: {rows:#?}"
    );
    assert!(marks.ends_with('│'), "{rows:#?}");
}

/// A plugin, then the fixture: installed at `installed` from `SHA_B` while
/// the catalogue announces `version` at `SHA_A`.
fn outdated_app(version: &str, installed: &str) -> SidebarApp {
    let mut fixture = manifest("herdr-plugin.toml", "herdr-marketplace-fixture");
    fixture["version"] = json!(version);
    let mut plugin = github_plugin(
        "herdr-marketplace-fixture",
        "massdo",
        "herdr-marketplace-fixture",
        None,
        SHA_B,
    );
    plugin["version"] = json!(installed);
    ready(
        vec![
            repo(
                "acme",
                "plugin",
                10,
                vec![manifest("herdr-plugin.toml", "acme.plugin")],
            ),
            repo("massdo", "herdr-marketplace-fixture", 5, vec![fixture]),
        ],
        vec![plugin],
    )
}

/// `app` drawn in a `width` × `height` pane, its page set first as the
/// sidebar loop does, and the text of each line.
fn draw(app: &mut SidebarApp, width: u16, height: u16) -> (Buffer, Vec<String>) {
    app.set_page(sidebar_view::page_rows(app, width, height));
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| sidebar_view::render(frame, app))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let text = buffer
        .content()
        .chunks(width as usize)
        .map(|line| line.iter().map(|cell| cell.symbol()).collect())
        .collect();
    (buffer, text)
}

#[test]
fn an_outdated_card_offers_its_update_instead_of_installed() {
    use herdr_marketplace::adapters::tui::style::ACCENT;
    let mut app = outdated_app("1.1.0", "1.0.0");
    // 32 × 15: search box, filters and separator, then the cards; the
    // fixture's is the second, lines 9 to 12.
    let (buffer, text) = draw(&mut app, 32, 15);
    assert!(text[10].starts_with("│ massdo/herdr"), "{text:#?}");
    assert!(
        text[11].starts_with("│  Update to 1.1.0  · herdr"),
        "the description follows the button: {text:#?}"
    );
    assert!((2..19).all(|column| buffer[(column, 11)].bg == ACCENT));
    assert_ne!(buffer[(19, 11)].bg, ACCENT, "{text:#?}");
    assert!(!text[9..13].join("\n").contains("installed"), "{text:#?}");

    let mut app = outdated_app("1.0.0-beta.1234567890", "0.9.0");
    let (buffer, text) = draw(&mut app, 32, 15);
    assert!(
        text[11].starts_with("│  Update  · herdr"),
        "a version too long for the card: {text:#?}"
    );
    assert!((2..10).all(|column| buffer[(column, 11)].bg == ACCENT));
}

#[test]
fn a_selected_card_keeps_the_colors_of_its_update_button() {
    use herdr_marketplace::adapters::tui::style::{ACCENT, SELECTION_BG};
    let mut app = outdated_app("1.1.0", "1.0.0");
    app.handle_key(key(KeyCode::Down));
    assert_eq!(selected_repo(&app), "herdr-marketplace-fixture");
    let (buffer, text) = draw(&mut app, 32, 15);
    assert!(text[11].starts_with("│  Update to 1.1.0 "), "{text:#?}");
    assert!((2..19).all(|column| buffer[(column, 11)].bg == ACCENT));
    assert_eq!(buffer[(1, 11)].bg, SELECTION_BG);
    assert!((19..31).all(|column| buffer[(column, 11)].bg == SELECTION_BG));
}

#[test]
fn a_narrow_row_offers_the_update_instead_of_the_description() {
    use herdr_marketplace::adapters::tui::style::ACCENT;
    let mut app = outdated_app("1.1.0", "1.0.0");
    // 14 columns: plain lines, the fixture's from line 9.
    let (buffer, text) = draw(&mut app, 14, 15);
    assert!(text[10].starts_with("massdo/"), "{text:#?}");
    assert!(text[11].starts_with(" Update "), "{text:#?}");
    assert!((0..8).all(|column| buffer[(column, 11)].bg == ACCENT));
}

#[test]
fn a_click_on_the_update_button_previews_the_plugin_as_the_card_does() {
    let mut app = outdated_app("1.1.0", "1.0.0");
    app.set_page(sidebar_view::page_rows(&app, 32, 15));
    app.handle_mouse(click(5, 11), 32, 15);
    match app.intents.as_slice() {
        [Intent::Open(row, Reveal::Preview)] => {
            assert_eq!(row.entry.source.repo, "herdr-marketplace-fixture")
        }
        other => panic!("{other:?}"),
    }
}
