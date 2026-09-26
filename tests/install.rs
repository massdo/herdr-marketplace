//! Install preview and confirmation, with a simulated registry and network.

mod support;

use std::cell::RefCell;
use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use herdr_marketplace::adapters::tui::details::{DetailsApp, DetailsIntent, InstallState};
use herdr_marketplace::adapters::tui::details_view;
use herdr_marketplace::application::ports::{FetchError, Fetcher};
use herdr_marketplace::application::prepare_install::{InstallPreview, Prepared, prepare_install};
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
    assert!(screen.contains("name: fixture]0;PWNED[2J"), "{screen}");
    assert!(screen.contains("hello: /bin/echo hi[31m6n"), "{screen}");
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
    has("• hello: /bin/echo hello from herdr-marketplace-fixture");
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
    assert_eq!(app.intents, [DetailsIntent::Install(preview.args)]);
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
