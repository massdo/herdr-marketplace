//! Install preview and confirmation, with a simulated registry and network.

mod support;

use std::cell::RefCell;
use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use herdr_marketplace::adapters::tui::details::{
    DetailsApp, DetailsIntent, InstallState, InstalledView, UpdateState,
};
use herdr_marketplace::adapters::tui::details_view;
use herdr_marketplace::application::ports::{FetchError, Fetcher};
use herdr_marketplace::application::prepare_install::{InstallPreview, Prepared, prepare_install};
use herdr_marketplace::application::update_plugin::prepare_update;
use herdr_marketplace::domain::compat::Platform;
use herdr_marketplace::domain::details::DetailsTarget;
use herdr_marketplace::domain::install::{Plan, install_args};
use herdr_marketplace::domain::source::PluginSource;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::json;
use support::*;

const RAW: &str = "https://raw.githubusercontent.com";
const FIXTURE_MANIFEST: &str = r#"
id = "herdr-marketplace-fixture"
name = "herdr-marketplace fixture"
version = "1.0.0"
description = "Test fixture for herdr-marketplace end-to-end tests. Not a real plugin."
min_herdr_version = "0.9.1"
platforms = ["linux", "macos"]

[[build]]
command = ["/bin/sh", "build.sh"]

[[actions]]
id = "hello"
title = "Say hello"
command = ["/bin/echo", "hello from herdr-marketplace-fixture"]
"#;
const ALT_MANIFEST: &str = r#"
id = "herdr-marketplace-fixture"
name = "herdr-marketplace fixture (alt)"
version = "1.0.0"
min_herdr_version = "0.9.1"
platforms = ["linux", "macos"]
"#;

/// Serves manifests by URL; anything else is a 404.
struct FakeWeb {
    pages: HashMap<String, String>,
    asked: RefCell<Vec<String>>,
}

impl FakeWeb {
    fn new(pages: &[(&str, &str)]) -> Self {
        Self {
            pages: pages
                .iter()
                .map(|(url, body)| (url.to_string(), body.to_string()))
                .collect(),
            asked: RefCell::new(Vec::new()),
        }
    }

    /// The fixture's manifests at `SHA_A`, root and `alt/`.
    fn fixture() -> Self {
        Self::new(&[
            (&manifest_url("", SHA_A), FIXTURE_MANIFEST),
            (&manifest_url("alt", SHA_A), ALT_MANIFEST),
        ])
    }
}

impl Fetcher for FakeWeb {
    fn fetch(&self, url: &str, _limit: u64) -> Result<Vec<u8>, FetchError> {
        self.asked.borrow_mut().push(url.to_string());
        self.pages
            .get(url)
            .map(|body| body.as_bytes().to_vec())
            .ok_or(FetchError::NotFound)
    }
}

fn manifest_url(subdir: &str, sha: &str) -> String {
    let folder = if subdir.is_empty() {
        String::new()
    } else {
        format!("{subdir}/")
    };
    format!("{RAW}/massdo/herdr-marketplace-fixture/{sha}/{folder}herdr-plugin.toml")
}

fn target(subdir: &str) -> DetailsTarget {
    DetailsTarget {
        source: PluginSource {
            owner: "massdo".into(),
            repo: "herdr-marketplace-fixture".into(),
            subdir: subdir.into(),
        },
        commit: SHA_A.into(),
        id: "herdr-marketplace-fixture".into(),
        name: "herdr-marketplace fixture".into(),
        version: Some("1.0.0".into()),
        in_catalog: true,
        compatible: true,
    }
}

fn prepare(web: &FakeWeb, installed: Vec<serde_json::Value>, target: &DetailsTarget) -> Prepared {
    prepare_install(
        web,
        &FakeHerdr::with_registry(installed),
        target,
        Platform::Macos,
    )
}

fn preview(prepared: Prepared) -> InstallPreview {
    match prepared {
        Prepared::Preview(preview) => *preview,
        other => panic!("expected a preview, got {other:?}"),
    }
}

fn refusal(prepared: Prepared) -> String {
    match prepared {
        Prepared::Refused(reason) => reason,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn fixture_installed(subdir: Option<&str>, sha: &str) -> serde_json::Value {
    github_plugin(
        "herdr-marketplace-fixture",
        "massdo",
        "herdr-marketplace-fixture",
        subdir,
        sha,
    )
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn the_request_is_herdr_plugin_install_with_separate_arguments() {
    let source = PluginSource {
        owner: "zenbu-labs".into(),
        repo: "terminal-browser".into(),
        subdir: "plugins/browser".into(),
    };
    assert_eq!(
        install_args(&source, SHA_A),
        [
            "plugin",
            "install",
            "zenbu-labs/terminal-browser/plugins/browser",
            "--ref",
            SHA_A,
            "--yes"
        ]
    );
}

#[test]
fn a_root_plugin_preview_reads_the_manifest_at_the_commit_shown() {
    let web = FakeWeb::fixture();
    let preview = preview(prepare(
        &web,
        vec![local_plugin("herdr-marketplace")],
        &target(""),
    ));
    assert_eq!(*web.asked.borrow(), [manifest_url("", SHA_A)]);
    assert_eq!(preview.plan, Plan::Install);
    assert_eq!(preview.replaces, None);
    assert_eq!(
        preview.args,
        [
            "plugin",
            "install",
            "massdo/herdr-marketplace-fixture",
            "--ref",
            SHA_A,
            "--yes"
        ]
    );
    assert_eq!(preview.manifest.build[0].command, ["/bin/sh", "build.sh"]);
    assert_eq!(preview.manifest.actions[0].name, "hello");
}

#[test]
fn a_subfolder_plugin_preview_reads_its_own_manifest() {
    let web = FakeWeb::fixture();
    let preview = preview(prepare(&web, vec![], &target("alt")));
    assert_eq!(*web.asked.borrow(), [manifest_url("alt", SHA_A)]);
    assert_eq!(preview.args[2], "massdo/herdr-marketplace-fixture/alt");
    assert_eq!(preview.manifest.name, "herdr-marketplace fixture (alt)");
}

#[test]
fn the_same_source_at_the_same_commit_is_already_installed() {
    let web = FakeWeb::fixture();
    let prepared = prepare(&web, vec![fixture_installed(None, SHA_A)], &target(""));
    assert_eq!(prepared, Prepared::UpToDate);
    assert!(web.asked.borrow().is_empty(), "nothing to fetch");
}

#[test]
fn another_commit_installed_becomes_a_switch_showing_both_commits() {
    let web = FakeWeb::fixture();
    let preview = preview(prepare(
        &web,
        vec![fixture_installed(None, SHA_B)],
        &target(""),
    ));
    assert_eq!(preview.plan, Plan::Switch { from: SHA_B.into() });
    assert_eq!(preview.commit, SHA_A);
    let replaced = preview.replaces.expect("the installed plugin is replaced");
    assert_eq!(replaced.resolved_commit(), Some(SHA_B));
}

#[test]
fn the_same_id_installed_from_another_source_is_refused() {
    let reason = refusal(prepare(
        &FakeWeb::fixture(),
        vec![fixture_installed(Some("alt"), SHA_A)],
        &target(""),
    ));
    assert!(
        reason.contains("massdo/herdr-marketplace-fixture/alt"),
        "{reason}"
    );
}

#[test]
fn the_same_id_linked_locally_is_refused() {
    let reason = refusal(prepare(
        &FakeWeb::fixture(),
        vec![local_plugin("herdr-marketplace-fixture")],
        &target(""),
    ));
    assert!(reason.contains("linked locally"), "{reason}");
}

#[test]
fn a_manifest_id_that_differs_from_the_index_is_refused() {
    let mut target = target("");
    target.id = "someone.else".into();
    let reason = refusal(prepare(&FakeWeb::fixture(), vec![], &target));
    assert!(reason.contains("someone.else"), "{reason}");
}

#[test]
fn a_source_installed_under_another_id_is_refused() {
    let mut installed = fixture_installed(None, SHA_B);
    installed["plugin_id"] = json!("fixture-renamed");
    let reason = refusal(prepare(&FakeWeb::fixture(), vec![installed], &target("")));
    assert!(reason.contains("fixture-renamed"), "{reason}");
}

#[test]
fn several_installed_plugins_for_one_source_are_refused() {
    let mut second = fixture_installed(None, SHA_B);
    second["plugin_id"] = json!("fixture-copy");
    let reason = refusal(prepare(
        &FakeWeb::fixture(),
        vec![fixture_installed(None, SHA_A), second],
        &target(""),
    ));
    assert!(reason.contains("several"), "{reason}");
}

#[test]
fn an_unreadable_registry_is_refused() {
    let herdr = FakeHerdr {
        version: "herdr 0.9.1".into(),
        registry: Err("socket closed".into()),
    };
    let prepared = prepare_install(&FakeWeb::fixture(), &herdr, &target(""), Platform::Macos);
    assert!(refusal(prepared).contains("registry"));
}

#[test]
fn a_missing_or_invalid_manifest_is_refused() {
    let reason = refusal(prepare(&FakeWeb::new(&[]), vec![], &target("")));
    assert!(reason.contains("not found"), "{reason}");

    let invalid = FIXTURE_MANIFEST.replace("min_herdr_version = \"0.9.1\"\n", "");
    let reason = refusal(prepare(
        &FakeWeb::new(&[(&manifest_url("", SHA_A), &invalid)]),
        vec![],
        &target(""),
    ));
    assert!(reason.contains("invalid"), "{reason}");
}

#[test]
fn an_incompatible_target_is_refused() {
    for replaced in [
        FIXTURE_MANIFEST.replace("[\"linux\", \"macos\"]", "[\"linux\"]"),
        FIXTURE_MANIFEST.replace("\"0.9.1\"", "\"9.9.9\""),
    ] {
        let reason = refusal(prepare(
            &FakeWeb::new(&[(&manifest_url("", SHA_A), &replaced)]),
            vec![],
            &target(""),
        ));
        assert!(reason.contains("incompatible"), "{reason}");
    }
    let mut incompatible = target("");
    incompatible.compatible = false;
    let web = FakeWeb::fixture();
    assert!(refusal(prepare(&web, vec![], &incompatible)).contains("can only be removed"));
    let mut off_catalogue = target("");
    off_catalogue.in_catalog = false;
    assert!(refusal(prepare(&web, vec![], &off_catalogue)).contains("can only be removed"));
    assert!(web.asked.borrow().is_empty());
}

#[test]
fn owner_and_repo_case_may_differ_between_catalogue_and_registry() {
    let upper = |sha| {
        github_plugin(
            "herdr-marketplace-fixture",
            "MASSDO",
            "Herdr-Marketplace-Fixture",
            None,
            sha,
        )
    };
    assert_eq!(
        prepare(&FakeWeb::fixture(), vec![upper(SHA_A)], &target("")),
        Prepared::UpToDate
    );
    let preview = preview(prepare(
        &FakeWeb::fixture(),
        vec![upper(SHA_B)],
        &target(""),
    ));
    assert_eq!(preview.plan, Plan::Switch { from: SHA_B.into() });
    assert_eq!(preview.args[2], "massdo/herdr-marketplace-fixture");
}

#[test]
fn a_source_outside_herdr_rules_is_refused_before_any_request() {
    let web = FakeWeb::fixture();
    for (owner, subdir) in [("bad owner", ""), ("massdo", "../up"), ("massdo", "a//b")] {
        let mut target = target(subdir);
        target.source.owner = owner.into();
        assert!(refusal(prepare(&web, vec![], &target)).contains("Herdr's rules"));
    }
    assert!(web.asked.borrow().is_empty());
}

#[test]
fn trapped_text_in_the_manifest_never_reaches_the_terminal() {
    let trapped = FIXTURE_MANIFEST
        .replace(
            "name = \"herdr-marketplace fixture\"",
            "name = \"fixture\\u001b]0;PWNED\\u0007\\u001b[2J\"",
        )
        .replace(
            "\"hello from herdr-marketplace-fixture\"",
            "\"hi\\u001b[31m\\u009b6n\"",
        );
    let preview = preview(prepare(
        &FakeWeb::new(&[(&manifest_url("", SHA_A), &trapped)]),
        vec![],
        &target(""),
    ));
    assert!(
        preview.manifest.name.contains('\u{1b}'),
        "the manifest keeps its text"
    );

    let mut app = DetailsApp::new(target(""));
    app.handle_key(key(KeyCode::Char('i')));
    app.install_prepared(1, Prepared::Preview(Box::new(preview)));
    let mut terminal = Terminal::new(TestBackend::new(90, 40)).unwrap();
    terminal
        .draw(|frame| details_view::render(frame, &app))
        .unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(!screen.chars().any(char::is_control), "{screen:?}");
    assert!(
        screen.contains("name: fixture⟨U+001B⟩]0;PWNED⟨U+0007⟩⟨U+001B⟩[2J"),
        "{screen}"
    );
    assert!(
        screen.contains(r#"hello: /bin/echo "hi\u001b[31m⟨U+009B⟩6n""#),
        "{screen}"
    );
}

#[test]
fn the_preview_shows_source_commit_commands_and_warnings() {
    let preview = preview(prepare(&FakeWeb::fixture(), vec![], &target("")));
    let mut app = DetailsApp::new(target(""));
    app.handle_key(key(KeyCode::Char('i')));
    app.install_prepared(1, Prepared::Preview(Box::new(preview)));
    let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
    terminal
        .draw(|frame| details_view::render(frame, &app))
        .unwrap();
    let screen: Vec<String> = terminal
        .backend()
        .buffer()
        .content()
        .chunks(100)
        .map(|line| line.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect();
    let has = |expected: &str| {
        assert!(
            screen.iter().any(|line| line.contains(expected)),
            "{expected:?} missing from {screen:#?}"
        )
    };
    has("Install");
    has("id: herdr-marketplace-fixture");
    has("source: massdo/herdr-marketplace-fixture");
    has(&format!("commit: {SHA_A}"));
    has("build commands (1)");
    has("• /bin/sh build.sh");
    has("startup commands (0)");
    has("events (0)");
    has("actions (1)");
    has("• hello: /bin/echo \"hello from herdr-marketplace-fixture\"");
    has("panes (0)");
    has("This plugin will run code with your permissions.");
    has("The SHA pins the repository, not what the build downloads.");
    has("Enter: confirm · Esc: cancel");
}

#[test]
fn nothing_is_requested_without_a_second_explicit_key() {
    let preview = preview(prepare(&FakeWeb::fixture(), vec![], &target("")));
    let mut app = DetailsApp::new(target(""));
    app.intents.clear();

    app.handle_key(key(KeyCode::Char('i')));
    assert_eq!(app.intents, [DetailsIntent::PrepareInstall(1)]);
    app.intents.clear();
    app.install_prepared(1, Prepared::Preview(Box::new(preview.clone())));
    assert!(app.showing_preview());

    assert!(
        !app.handle_key(key(KeyCode::Esc)),
        "escape cancels, the details pane stays"
    );
    assert_eq!(app.install, InstallState::Idle);
    assert!(app.intents.is_empty(), "cancelling launches nothing");

    app.handle_key(key(KeyCode::Char('i')));
    app.install_prepared(1, Prepared::Preview(Box::new(preview.clone())));
    assert_eq!(app.install, InstallState::Preparing, "answer 1 is stale");
    app.install_prepared(2, Prepared::Preview(Box::new(preview.clone())));
    app.intents.clear();
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.intents, [DetailsIntent::Install(Box::new(preview))]);
}

#[test]
fn a_plugin_without_readme_stays_installable() {
    let mut app = DetailsApp::new(target(""));
    app.readme_loaded(
        1,
        Ok(herdr_marketplace::application::load_readme::Readme::NotFound),
    );
    app.intents.clear();
    app.handle_key(key(KeyCode::Char('i')));
    assert_eq!(app.intents, [DetailsIntent::PrepareInstall(1)]);
    let preview = preview(prepare(&FakeWeb::fixture(), vec![], &target("")));
    app.install_prepared(1, Prepared::Preview(Box::new(preview)));
    assert!(app.showing_preview());
}

#[test]
fn a_short_pane_keeps_the_preview_reachable_after_a_long_operation_error() {
    use herdr_marketplace::domain::operation::{
        OperationKind, OperationRecord, OperationRequest, Status,
    };
    let preview = preview(prepare(&FakeWeb::fixture(), vec![], &target("")));
    let mut app = DetailsApp::new(target(""));
    let request = OperationRequest {
        id: "old".into(),
        kind: OperationKind::Install,
        source: app.target.source.clone(),
        commit: SHA_A.into(),
        args: vec![],
        confirmation: None,
    };
    let mut record = OperationRecord::running(&request);
    record.status = Status::Failed;
    record.output = "old build error with a long explanation\n".repeat(30);
    app.operation_seen(Some(record));
    app.handle_key(key(KeyCode::Char('i')));
    app.install_prepared(1, Prepared::Preview(Box::new(preview)));
    let page = details_view::page_rows(&app, 44, 10);
    assert!(page > 0, "history must not consume the entire pane");
    app.set_viewport(44, page);
    let screen = |app: &DetailsApp| {
        let mut terminal = Terminal::new(TestBackend::new(44, 10)).unwrap();
        terminal
            .draw(|frame| details_view::render(frame, app))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    };
    assert!(screen(&app).contains("id: herdr-marketplace-fixture"));
    assert!(!screen(&app).contains("old build error"));
    app.handle_key(key(KeyCode::End));
    assert!(app.preview_scroll > 0);
    assert!(
        screen(&app).contains("build downloads."),
        "{}",
        screen(&app)
    );
    app.set_viewport(44, 0);
    app.intents.clear();
    app.handle_key(key(KeyCode::Enter));
    assert!(
        app.intents.is_empty(),
        "a hidden confirmation cannot execute"
    );
}

fn click(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

/// Lines of a 90 × 30 details pane.
fn lines(app: &DetailsApp) -> Vec<String> {
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
        .collect()
}

/// Column of the middle of `label` on the action bar, the fourth line.
fn button_column(app: &DetailsApp, label: &str) -> u16 {
    let bar = &lines(app)[3];
    let start = bar
        .find(label)
        .unwrap_or_else(|| panic!("{label:?} not on the action bar {bar:?}"));
    (start + label.len() / 2) as u16
}

#[test]
fn the_install_button_opens_the_preview_and_its_confirm_button_installs() {
    let preview = preview(prepare(&FakeWeb::fixture(), vec![], &target("")));
    let mut app = DetailsApp::new(target(""));
    app.registry_read(1, InstalledView::NotInstalled);
    app.set_viewport(90, details_view::page_rows(&app, 90, 30));
    app.intents.clear();
    assert!(
        lines(&app)[3].starts_with(" Install (i) "),
        "{:#?}",
        lines(&app)
    );
    app.handle_mouse(click(40, 3), 90, 30);
    assert!(app.intents.is_empty(), "beside the button");

    app.handle_mouse(click(button_column(&app, "Install (i)"), 3), 90, 30);
    assert_eq!(app.intents, [DetailsIntent::PrepareInstall(1)]);
    app.install_prepared(1, Prepared::Preview(Box::new(preview.clone())));
    let screen = lines(&app);
    assert!(
        screen[3].starts_with(" Confirm install (Enter) "),
        "{screen:#?}"
    );
    assert!(screen[3].contains(" Cancel (Esc) "), "{screen:#?}");
    assert!(
        screen
            .iter()
            .any(|line| line.starts_with("Install this plugin?")),
        "{screen:#?}"
    );

    app.intents.clear();
    app.handle_mouse(click(button_column(&app, "Cancel (Esc)"), 3), 90, 30);
    assert_eq!(app.install, InstallState::Idle);
    assert!(app.intents.is_empty(), "cancelling launches nothing");

    app.handle_mouse(click(button_column(&app, "Install (i)"), 3), 90, 30);
    app.install_prepared(2, Prepared::Preview(Box::new(preview.clone())));
    app.intents.clear();
    app.handle_mouse(
        click(button_column(&app, "Confirm install (Enter)"), 3),
        90,
        30,
    );
    assert_eq!(app.intents, [DetailsIntent::Install(Box::new(preview))]);
    assert!(
        !lines(&app)[3].contains("Confirm"),
        "no second confirmation: {:#?}",
        lines(&app)
    );
}

/// The registry shows the plugin installed at `commit`, in `version`.
fn installed_at(commit: &str, version: &str) -> InstalledView {
    InstalledView::At {
        commit: commit.into(),
        version: version.into(),
    }
}

#[test]
fn another_installed_commit_offers_a_switch_and_a_removal() {
    let mut app = DetailsApp::new(target(""));
    app.registry_read(1, installed_at(SHA_B, "1.0.0"));
    let bar = &lines(&app)[3];
    assert!(bar.starts_with(" Switch to c8268d4 (i) "), "{bar:?}");
    assert!(bar.contains(" Remove (r) "), "{bar:?}");

    app.registry_read(1, installed_at(SHA_A, "1.0.0"));
    let bar = &lines(&app)[3];
    assert!(
        bar.starts_with(" Remove (r) "),
        "installed at this commit: {bar:?}"
    );
    assert!(!bar.contains("Install"), "{bar:?}");
}

#[test]
fn a_running_operation_leaves_only_the_github_page() {
    let mut app = DetailsApp::new(target(""));
    app.registry_read(1, InstalledView::NotInstalled);
    app.operation_launched(
        "op".into(),
        herdr_marketplace::domain::operation::OperationKind::Install,
    );
    let labels: Vec<String> = app
        .buttons()
        .into_iter()
        .map(|button| button.label)
        .collect();
    assert_eq!(labels, ["Open on GitHub"]);
    let screen = lines(&app);
    assert!(screen[3].starts_with(" Open on GitHub (o) "), "{screen:#?}");
    assert!(screen[4].starts_with("Installing…"), "{screen:#?}");
    app.intents.clear();
    app.handle_mouse(click(40, 3), 90, 30);
    assert!(app.intents.is_empty(), "beside the button");
}

#[test]
fn buttons_look_like_buttons() {
    use herdr_marketplace::adapters::tui::style::{ACCENT, BUTTON_BG};
    let mut app = DetailsApp::new(target(""));
    app.registry_read(1, installed_at(SHA_B, "1.0.0"));
    let mut terminal = Terminal::new(TestBackend::new(90, 30)).unwrap();
    terminal
        .draw(|frame| details_view::render(frame, &app))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let bar = lines(&app)[3].clone();
    let switch = bar.find("Switch to").unwrap() as u16;
    let remove = bar.find("Remove (r)").unwrap() as u16;
    assert_eq!(buffer[(switch, 3)].bg, ACCENT, "the main action is blue");
    assert_eq!(buffer[(remove, 3)].bg, BUTTON_BG, "the other one is grey");
    assert_eq!(
        buffer[(remove - 2, 3)].bg,
        ratatui::style::Color::Reset,
        "a gap between them"
    );
}

#[test]
fn installation_preview_reveals_unicode_format_characters() {
    use herdr_marketplace::adapters::tui::preview::preview_lines;
    use herdr_marketplace::domain::text::{clean, is_format};
    let mut preview = preview(prepare(&FakeWeb::fixture(), vec![], &target("")));
    preview.manifest.name = "safe\u{202e}name\u{200b}".into();
    preview.manifest.build[0].command = vec!["echo".into(), "one\u{2066}two\u{feff}".into()];
    let shown = preview_lines(&preview, 120, Platform::Macos)
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(shown.contains("safe⟨U+202E⟩name⟨U+200B⟩"), "{shown}");
    assert!(shown.contains("echo one⟨U+2066⟩two⟨U+FEFF⟩"), "{shown}");
    assert!(!shown.chars().any(is_format));
    assert_eq!(clean("safe\u{202e}name\u{200b}"), "safename");
    assert!(
        preview.manifest.name.contains('\u{202e}'),
        "the executed manifest is unchanged"
    );
}

#[test]
fn preview_preserves_argument_boundaries_and_reveals_shell_newlines() {
    use herdr_marketplace::adapters::tui::preview::preview_lines;
    let mut preview = preview(prepare(&FakeWeb::fixture(), vec![], &target("")));
    let command = vec![
        "sh".into(),
        "-c".into(),
        "# harmless comment\nprintf dangerous".into(),
    ];
    preview.manifest.build[0].command = command.clone();
    let shown = preview_lines(&preview, 160, Platform::Macos)
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        shown.contains(r##"sh -c "# harmless comment\nprintf dangerous""##),
        "{shown}"
    );
    assert_eq!(preview.manifest.build[0].command, command);
}

/// The fixture's root manifest at `SHA_A`, declaring `version`.
fn manifest_in(version: &str) -> FakeWeb {
    let manifest =
        FIXTURE_MANIFEST.replace("version = \"1.0.0\"", &format!("version = \"{version}\""));
    FakeWeb::new(&[(&manifest_url("", SHA_A), &manifest)])
}

fn update(
    web: &FakeWeb,
    installed: Vec<serde_json::Value>,
    target: &DetailsTarget,
) -> Result<InstallPreview, String> {
    prepare_update(
        web,
        &FakeHerdr::with_registry(installed),
        target,
        Platform::Macos,
    )
}

#[test]
fn an_update_replaces_the_installed_plugin_with_the_newer_manifest() {
    let preview = update(
        &manifest_in("1.1.0"),
        vec![fixture_installed(None, SHA_B)],
        &target(""),
    )
    .unwrap();
    assert_eq!(preview.plan, Plan::Switch { from: SHA_B.into() });
    assert_eq!(preview.manifest.version, "1.1.0");
    assert_eq!(preview.args, install_args(&target("").source, SHA_A));
}

#[test]
fn an_update_is_refused_unless_it_replaces_an_older_version_from_this_source() {
    let at_b = || vec![fixture_installed(None, SHA_B)];
    let refused = |web: &FakeWeb, installed, target: &DetailsTarget| {
        update(web, installed, target).unwrap_err()
    };
    let web = manifest_in("1.1.0");
    assert_eq!(
        refused(&web, vec![fixture_installed(None, SHA_A)], &target("")),
        "already installed at this commit"
    );
    assert_eq!(
        refused(&web, vec![], &target("")),
        "not installed from this source"
    );
    for version in ["1.0.0", "0.9.0"] {
        assert_eq!(
            refused(&manifest_in(version), at_b(), &target("")),
            format!(
                "the manifest at this commit declares {version}, not newer than the installed 1.0.0"
            )
        );
    }

    // The preview's own refusals.
    let mut renamed = target("");
    renamed.id = "someone.else".into();
    assert!(refused(&web, at_b(), &renamed).contains("someone.else"));
    let linux = FakeWeb::new(&[(
        &manifest_url("", SHA_A),
        &FIXTURE_MANIFEST.replace("[\"linux\", \"macos\"]", "[\"linux\"]"),
    )]);
    assert!(refused(&linux, at_b(), &target("")).contains("incompatible"));
    assert!(refused(&FakeWeb::new(&[]), at_b(), &target("")).contains("not found"));
    let reason = refused(
        &web,
        vec![fixture_installed(Some("alt"), SHA_B)],
        &target(""),
    );
    assert!(
        reason.contains("already installed from massdo/herdr-marketplace-fixture/alt"),
        "{reason}"
    );
}

/// The fixture announced in 1.1.0 at `SHA_A`, installed in 1.0.0 at `SHA_B`.
fn outdated() -> DetailsApp {
    let mut target = target("");
    target.version = Some("1.1.0".into());
    let mut app = DetailsApp::new(target);
    app.registry_read(1, installed_at(SHA_B, "1.0.0"));
    app.set_viewport(90, details_view::page_rows(&app, 90, 30));
    app.intents.clear();
    app
}

/// What a switch to `SHA_A` confirms: an update's checked request.
fn switch() -> InstallPreview {
    preview(prepare(
        &FakeWeb::fixture(),
        vec![fixture_installed(None, SHA_B)],
        &target(""),
    ))
}

#[test]
fn a_newer_catalogue_version_offers_an_update_instead_of_a_switch() {
    use herdr_marketplace::adapters::tui::style::ACCENT;
    let app = outdated();
    let mut terminal = Terminal::new(TestBackend::new(90, 30)).unwrap();
    terminal
        .draw(|frame| details_view::render(frame, &app))
        .unwrap();
    let bar = &lines(&app)[3];
    assert!(bar.starts_with(" Update to 1.1.0 (u) "), "{bar:?}");
    let remove = bar.find(" Remove (r) ").expect("Remove follows");
    assert!(remove > bar.find("Update").unwrap(), "{bar:?}");
    assert!(!bar.contains("Switch"), "{bar:?}");
    assert_eq!(terminal.backend().buffer()[(1, 3)].bg, ACCENT, "blue");
}

#[test]
fn u_or_a_click_asks_for_the_update_only_when_one_is_offered() {
    let mut app = outdated();
    app.handle_key(key(KeyCode::Char('u')));
    assert_eq!(app.intents, [DetailsIntent::PrepareUpdate(1)]);
    let screen = lines(&app);
    assert!(screen[3].starts_with(" Cancel (Esc) "), "{screen:#?}");
    assert!(
        screen.iter().any(|line| line.contains("Preparing update…")),
        "{screen:#?}"
    );

    let mut app = outdated();
    app.handle_mouse(click(button_column(&app, "Update to 1.1.0 (u)"), 3), 90, 30);
    assert_eq!(app.intents, [DetailsIntent::PrepareUpdate(1)]);

    let mut same = DetailsApp::new(target(""));
    same.registry_read(1, installed_at(SHA_B, "1.0.0"));
    same.intents.clear();
    same.handle_key(key(KeyCode::Char('u')));
    assert!(same.intents.is_empty(), "same version: no update");

    let mut running = outdated();
    running.operation_launched(
        "op".into(),
        herdr_marketplace::domain::operation::OperationKind::Install,
    );
    running.handle_key(key(KeyCode::Char('u')));
    assert!(running.intents.is_empty(), "an operation runs");
}

#[test]
fn a_checked_update_runs_without_a_preview_and_a_refused_one_says_why() {
    let mut app = outdated();
    app.handle_key(key(KeyCode::Char('u')));
    app.intents.clear();
    app.update_prepared(1, Ok(switch()));
    assert_eq!(app.intents, [DetailsIntent::Update(Box::new(switch()))]);
    assert!(!app.showing_preview());
    assert_eq!(app.update, UpdateState::Idle);

    app.intents.clear();
    app.handle_key(key(KeyCode::Char('u')));
    app.update_prepared(2, Err("network error: timed out".into()));
    let screen = lines(&app);
    assert!(
        screen
            .iter()
            .any(|line| line.contains("Update refused: network error: timed out")),
        "{screen:#?}"
    );
    assert!(
        screen[3].starts_with(" Update to 1.1.0 (u) "),
        "the button stays, to try again: {screen:#?}"
    );

    app.handle_key(key(KeyCode::Char('u')));
    app.intents.clear();
    app.update_prepared(2, Ok(switch()));
    assert_eq!(app.update, UpdateState::Preparing, "answer 2 is stale");
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.update, UpdateState::Idle);
    app.update_prepared(3, Ok(switch()));
    assert!(app.intents.is_empty(), "an answer after Esc runs nothing");
}

#[test]
fn i_still_opens_the_switch_preview_when_an_update_is_offered() {
    let mut app = outdated();
    app.handle_key(key(KeyCode::Char('i')));
    assert_eq!(app.intents, [DetailsIntent::PrepareInstall(1)]);
    app.install_prepared(1, Prepared::Preview(Box::new(switch())));
    assert!(app.showing_preview());

    // The last key wins: i drops an update being checked.
    let mut app = outdated();
    app.handle_key(key(KeyCode::Char('u')));
    app.handle_key(key(KeyCode::Char('i')));
    app.intents.clear();
    app.update_prepared(1, Ok(switch()));
    assert!(app.intents.is_empty(), "{:?}", app.intents);
}
