//! Removal of an installed plugin, with a simulated Herdr.

mod support;

use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use herdr_marketplace::adapters::operations::FsOperations;
use herdr_marketplace::adapters::tui::details::{
    DetailsApp, DetailsIntent, InstalledView, RemovalState,
};
use herdr_marketplace::adapters::tui::details_view;
use herdr_marketplace::application::ports::{CommandOutput, HerdrCli};
use herdr_marketplace::application::prepare_removal::prepare_removal;
use herdr_marketplace::application::run_operation::run_operation;
use herdr_marketplace::domain::details::DetailsTarget;
use herdr_marketplace::domain::operation::{Confirmation, OperationKind, OperationRequest, Status};
use herdr_marketplace::domain::registry::parse_registry;
use herdr_marketplace::domain::source::PluginSource;
use herdr_marketplace::domain::uninstall::plan_removal;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::json;
use support::*;

/// `herdr` whose `plugin uninstall` answers `code` and leaves `after`.
struct FakeUninstall {
    registry: RefCell<Result<String, String>>,
    code: Option<i32>,
    after: Result<String, String>,
}

impl HerdrCli for FakeUninstall {
    fn version(&self) -> Result<String, String> {
        Ok("herdr 0.9.1".into())
    }

    fn plugin_list(&self) -> Result<String, String> {
        self.registry.borrow().clone()
    }

    fn run(&self, _args: &[String]) -> Result<CommandOutput, String> {
        *self.registry.borrow_mut() = self.after.clone();
        Ok(CommandOutput {
            code: self.code,
            output: "Error: plugin not installed".into(),
        })
    }
}

fn source(subdir: &str) -> PluginSource {
    PluginSource {
        owner: "massdo".into(),
        repo: "herdr-marketplace-fixture".into(),
        subdir: subdir.into(),
    }
}

fn fixture(subdir: Option<&str>, owner: &str, repo: &str) -> serde_json::Value {
    github_plugin("herdr-marketplace-fixture", owner, repo, subdir, SHA_A)
}

fn installed(
    plugins: Vec<serde_json::Value>,
) -> Vec<herdr_marketplace::domain::registry::InstalledPlugin> {
    parse_registry(&registry(plugins)).unwrap()
}

fn removal(
    code: Option<i32>,
    after: Result<String, String>,
) -> herdr_marketplace::domain::operation::OperationRecord {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "herdr-marketplace-removal-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let herdr = FakeUninstall {
        registry: RefCell::new(Ok(registry(vec![fixture(
            None,
            "massdo",
            "herdr-marketplace-fixture",
        )]))),
        code,
        after,
    };
    let request = OperationRequest {
        id: "op-removal".into(),
        kind: OperationKind::Uninstall,
        confirmation: Some(Confirmation::Uninstall {
            installed: installed(vec![fixture(None, "massdo", "herdr-marketplace-fixture")])
                .remove(0),
        }),
        source: source(""),
        commit: SHA_A.into(),
        args: vec![
            "plugin".into(),
            "uninstall".into(),
            "massdo/herdr-marketplace-fixture".into(),
        ],
    };
    run_operation(&herdr, &FsOperations::new(dir), &request)
}

fn target(in_catalog: bool, compatible: bool) -> DetailsTarget {
    DetailsTarget {
        source: source(""),
        commit: SHA_A.into(),
        id: "herdr-marketplace-fixture".into(),
        name: "herdr-marketplace fixture".into(),
        version: Some("1.0.0".into()),
        in_catalog,
        compatible,
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn screen(app: &DetailsApp) -> String {
    let mut terminal = Terminal::new(TestBackend::new(90, 30)).unwrap();
    terminal
        .draw(|frame| details_view::render(frame, app))
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .chunks(90)
        .map(|line| line.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_namesake_from_another_source_is_never_removed() {
    let only_alt = installed(vec![fixture(
        Some("alt"),
        "massdo",
        "herdr-marketplace-fixture",
    )]);
    let refusal = plan_removal(&only_alt, &source("")).unwrap_err();
    assert!(refusal.contains("no plugin installed"), "{refusal}");

    let mut root = fixture(None, "massdo", "herdr-marketplace-fixture");
    root["plugin_id"] = json!("fixture-root");
    let both = installed(vec![
        root,
        fixture(Some("alt"), "massdo", "herdr-marketplace-fixture"),
    ]);
    let plan = plan_removal(&both, &source("")).unwrap();
    assert_eq!(
        plan.args,
        ["plugin", "uninstall", "massdo/herdr-marketplace-fixture"]
    );
    assert_eq!(plan.installed.plugin_id, "fixture-root");
}

#[test]
fn a_locally_linked_plugin_is_never_removed() {
    let local = installed(vec![local_plugin("herdr-marketplace-fixture")]);
    assert!(plan_removal(&local, &source("")).is_err());
}

#[test]
fn the_source_is_recognised_without_case_and_removed_as_the_registry_spells_it() {
    let upper = installed(vec![fixture(None, "MASSDO", "Herdr-Marketplace-Fixture")]);
    let plan = plan_removal(&upper, &source("")).unwrap();
    assert_eq!(
        plan.args,
        ["plugin", "uninstall", "MASSDO/Herdr-Marketplace-Fixture"]
    );

    let lower_subdir = installed(vec![fixture(
        Some("alt"),
        "massdo",
        "herdr-marketplace-fixture",
    )]);
    assert!(
        plan_removal(&lower_subdir, &source("ALT")).is_err(),
        "the subdir keeps its case"
    );
}

#[test]
fn an_off_catalogue_or_incompatible_plugin_can_be_removed() {
    let herdr =
        FakeHerdr::with_registry(vec![fixture(None, "massdo", "herdr-marketplace-fixture")]);
    let plan = prepare_removal(&herdr, &source("")).unwrap();

    let mut app = DetailsApp::new(target(false, false));
    app.intents.clear();
    app.handle_key(key(KeyCode::Char('r')));
    assert_eq!(app.intents, [DetailsIntent::PrepareRemoval(1)]);
    app.removal_prepared(1, Ok(plan));
    assert!(matches!(app.removal, RemovalState::Confirm(_)));
}

#[test]
fn a_removal_needs_a_second_explicit_key() {
    let plan = plan_removal(
        &installed(vec![fixture(None, "massdo", "herdr-marketplace-fixture")]),
        &source(""),
    )
    .unwrap();
    let mut app = DetailsApp::new(target(true, true));
    app.intents.clear();
    app.handle_key(key(KeyCode::Char('r')));
    app.removal_prepared(1, Ok(plan.clone()));
    let shown = screen(&app);
    assert!(shown.contains("Remove this plugin?"), "{shown}");
    assert!(
        shown.contains("source: massdo/herdr-marketplace-fixture"),
        "{shown}"
    );
    assert!(shown.contains("Enter: confirm · Esc: cancel"), "{shown}");

    app.intents.clear();
    assert!(
        !app.handle_key(key(KeyCode::Esc)),
        "escape cancels, the details pane stays"
    );
    assert_eq!(app.removal, RemovalState::Idle);
    assert!(app.intents.is_empty(), "cancelling removes nothing");

    app.handle_key(key(KeyCode::Char('r')));
    app.removal_prepared(2, Ok(plan.clone()));
    app.intents.clear();
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.intents, [DetailsIntent::Uninstall(Box::new(plan))]);
}

#[test]
fn success_needs_the_plugin_gone_from_the_registry() {
    let gone = removal(Some(0), Ok(registry(vec![])));
    assert_eq!(gone.status, Status::Succeeded);
    assert_eq!(gone.registry_after.as_deref(), Some("not installed"));

    let still_there = removal(
        Some(0),
        Ok(registry(vec![fixture(
            None,
            "massdo",
            "herdr-marketplace-fixture",
        )])),
    );
    assert_eq!(still_there.status, Status::Failed);
}

#[test]
fn a_failed_removal_shows_herdr_output_and_the_registry_state() {
    let failed = removal(
        Some(1),
        Ok(registry(vec![fixture(
            None,
            "massdo",
            "herdr-marketplace-fixture",
        )])),
    );
    assert_eq!(failed.status, Status::Failed);
    let mut app = DetailsApp::new(target(true, true));
    app.operation_seen(Some(failed));
    let shown = screen(&app);
    assert!(shown.contains("Removal failed (code 1)"), "{shown}");
    assert!(
        shown.contains(&format!("Registry: installed at {SHA_A}")),
        "{shown}"
    );
    assert!(shown.contains("Error: plugin not installed"), "{shown}");
}

#[test]
fn an_unreadable_registry_after_a_failure_is_an_unknown_state() {
    let failed = removal(Some(1), Err("socket closed".into()));
    assert_eq!(failed.status, Status::Failed);
    assert!(
        failed
            .registry_after
            .as_deref()
            .is_some_and(|state| state.starts_with("unknown state")),
        "{:?}",
        failed.registry_after
    );
}

#[test]
fn a_removal_is_refused_if_the_installed_commit_changed_after_confirmation() {
    let confirmed = installed(vec![fixture(None, "massdo", "herdr-marketplace-fixture")]).remove(0);
    let herdr = FakeUninstall {
        registry: RefCell::new(Ok(registry(vec![github_plugin(
            "herdr-marketplace-fixture",
            "massdo",
            "herdr-marketplace-fixture",
            None,
            SHA_B,
        )]))),
        code: Some(0),
        after: Ok(registry(vec![])),
    };
    let request = OperationRequest {
        id: "stale-removal".into(),
        kind: OperationKind::Uninstall,
        source: source(""),
        commit: SHA_A.into(),
        args: vec!["plugin".into(), "uninstall".into(), source("").to_string()],
        confirmation: Some(Confirmation::Uninstall {
            installed: confirmed,
        }),
    };
    let dir = std::env::temp_dir().join(format!(
        "herdr-marketplace-stale-removal-{}",
        std::process::id()
    ));
    let record = run_operation(&herdr, &FsOperations::new(dir.clone()), &request);
    assert_eq!(record.status, Status::Refused);
    assert!(record.output.contains("changed since the preview"));
    assert!(
        herdr.plugin_list().unwrap().contains(SHA_B),
        "nothing was removed"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_removal_confirmation_scrolls_to_its_last_line_in_a_short_pane() {
    let plan = plan_removal(
        &installed(vec![fixture(None, "massdo", "herdr-marketplace-fixture")]),
        &source(""),
    )
    .unwrap();
    let mut app = DetailsApp::new(target(true, true));
    app.handle_key(key(KeyCode::Char('r')));
    app.removal_prepared(1, Ok(plan));
    let page = details_view::page_rows(&app, 40, 8);
    app.set_viewport(40, page);
    app.handle_key(key(KeyCode::End));
    assert!(app.preview_scroll > 0);
    let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
    terminal
        .draw(|frame| details_view::render(frame, &app))
        .unwrap();
    let text = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(text.contains("Herdr also deletes its checkout."), "{text}");
}

fn click(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

/// Column of the middle of `label` on the action bar, the fourth line.
fn button_column(app: &DetailsApp, label: &str) -> u16 {
    let bar = screen(app).lines().nth(3).unwrap().to_string();
    let start = bar
        .find(label)
        .unwrap_or_else(|| panic!("{label:?} not on the action bar {bar:?}"));
    (start + label.len() / 2) as u16
}

#[test]
fn the_remove_button_asks_and_confirm_removal_removes() {
    let plan = plan_removal(
        &installed(vec![fixture(None, "massdo", "herdr-marketplace-fixture")]),
        &source(""),
    )
    .unwrap();
    let mut app = DetailsApp::new(target(true, true));
    app.registry_read(
        1,
        InstalledView::At {
            commit: SHA_A.into(),
            version: "1.0.0".into(),
        },
    );
    app.set_viewport(90, details_view::page_rows(&app, 90, 30));
    app.intents.clear();

    app.handle_mouse(click(button_column(&app, "Remove (r)"), 3), 90, 30);
    assert_eq!(app.intents, [DetailsIntent::PrepareRemoval(1)]);
    app.removal_prepared(1, Ok(plan.clone()));
    let bar = screen(&app).lines().nth(3).unwrap().to_string();
    assert!(bar.starts_with(" Confirm removal (Enter) "), "{bar:?}");

    app.intents.clear();
    app.handle_mouse(click(button_column(&app, "Cancel (Esc)"), 3), 90, 30);
    assert_eq!(app.removal, RemovalState::Idle);
    assert!(app.intents.is_empty(), "cancelling removes nothing");

    app.handle_mouse(click(button_column(&app, "Remove (r)"), 3), 90, 30);
    app.removal_prepared(2, Ok(plan.clone()));
    app.intents.clear();
    app.handle_mouse(
        click(button_column(&app, "Confirm removal (Enter)"), 3),
        90,
        30,
    );
    assert_eq!(app.intents, [DetailsIntent::Uninstall(Box::new(plan))]);
}

#[test]
fn a_successful_removal_with_an_unreadable_registry_is_unconfirmed() {
    let record = removal(Some(0), Err("socket closed".into()));
    assert_eq!(record.status, Status::Unconfirmed);
    let mut app = DetailsApp::new(target(true, true));
    app.operation_seen(Some(record));
    assert!(!screen(&app).contains("Removal failed (code 0)"));
}
