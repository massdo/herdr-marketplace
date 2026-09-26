//! Install execution and kept results, with a simulated Herdr.

mod support;

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use herdr_marketplace::adapters::operations::FsOperations;
use herdr_marketplace::adapters::tui::details::{DetailsApp, DetailsIntent};
use herdr_marketplace::adapters::tui::details_view;
use herdr_marketplace::application::ports::{CommandOutput, HerdrCli, Operations};
use herdr_marketplace::application::run_operation::{current_operation, run_operation};
use herdr_marketplace::domain::details::DetailsTarget;
use herdr_marketplace::domain::install::install_args;
use herdr_marketplace::domain::operation::{
    Confirmation, OperationKind, OperationRecord, OperationRequest, Status, record_file_name,
};
use herdr_marketplace::domain::source::PluginSource;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use support::*;

/// `herdr` whose `plugin install` answers `code` and leaves `after` as the
/// registry.
struct FakeInstall {
    registry: RefCell<Result<String, String>>,
    code: Option<i32>,
    output: String,
    after: Result<String, String>,
    ran: RefCell<Vec<Vec<String>>>,
}

impl FakeInstall {
    fn new(code: Option<i32>, output: &str, after: Result<String, String>) -> Self {
        Self {
            registry: RefCell::new(Ok(registry(vec![]))),
            code,
            output: output.into(),
            after,
            ran: RefCell::new(Vec::new()),
        }
    }
}

impl HerdrCli for FakeInstall {
    fn version(&self) -> Result<String, String> {
        Ok("herdr 0.9.1".into())
    }

    fn plugin_list(&self) -> Result<String, String> {
        self.registry.borrow().clone()
    }

    fn run(&self, args: &[String]) -> Result<CommandOutput, String> {
        self.ran.borrow_mut().push(args.to_vec());
        *self.registry.borrow_mut() = self.after.clone();
        Ok(CommandOutput {
            code: self.code,
            output: self.output.clone(),
        })
    }
}

fn state_dir() -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "herdr-marketplace-operations-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn source() -> PluginSource {
    PluginSource {
        owner: "massdo".into(),
        repo: "herdr-marketplace-fixture".into(),
        subdir: String::new(),
    }
}

fn request(commit: &str) -> OperationRequest {
    OperationRequest {
        id: format!("op-{commit}"),
        kind: OperationKind::Install,
        source: source(),
        commit: commit.into(),
        args: install_args(&source(), commit),
        confirmation: Some(Confirmation::Install {
            target: DetailsTarget {
                commit: commit.into(),
                ..target()
            },
            manifest: herdr_marketplace::domain::manifest::parse_manifest(
                r#"
id = "herdr-marketplace-fixture"
name = "fixture"
version = "1.0.0"
min_herdr_version = "0.9.1"
"#,
            )
            .unwrap()
            .into(),
            plan: herdr_marketplace::domain::install::Plan::Install,
        }),
    }
}

fn installed_at(sha: &str) -> Result<String, String> {
    Ok(registry(vec![github_plugin(
        "herdr-marketplace-fixture",
        "massdo",
        "herdr-marketplace-fixture",
        None,
        sha,
    )]))
}

fn details_text(app: &DetailsApp) -> String {
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

fn target() -> DetailsTarget {
    DetailsTarget {
        source: source(),
        commit: SHA_A.into(),
        id: "herdr-marketplace-fixture".into(),
        name: "herdr-marketplace fixture".into(),
        version: Some("1.0.0".into()),
        in_catalog: true,
        compatible: true,
    }
}

#[test]
fn code_zero_confirmed_by_the_registry_is_a_success() {
    let operations = FsOperations::new(state_dir());
    let herdr = FakeInstall::new(
        Some(0),
        "Installed herdr-marketplace-fixture",
        installed_at(SHA_A),
    );
    let record = run_operation(&herdr, &operations, &request(SHA_A));
    assert_eq!(record.status, Status::Succeeded);
    assert_eq!(record.exit_code, Some(0));
    assert_eq!(
        record.registry_after.as_deref(),
        Some(format!("installed at {SHA_A}").as_str())
    );
    assert_eq!(
        *herdr.ran.borrow(),
        [vec![
            "plugin",
            "install",
            "massdo/herdr-marketplace-fixture",
            "--ref",
            SHA_A,
            "--yes"
        ]]
    );
}

#[test]
fn code_zero_without_the_registry_confirmation_is_unconfirmed() {
    let operations = FsOperations::new(state_dir());
    for after in [Ok(registry(vec![])), installed_at(SHA_B)] {
        let herdr = FakeInstall::new(Some(0), "", after);
        let record = run_operation(&herdr, &operations, &request(SHA_A));
        assert_eq!(record.status, Status::Unconfirmed);
    }
}

#[test]
fn a_non_zero_code_is_a_failure_with_herdr_output_and_the_registry_state() {
    let operations = FsOperations::new(state_dir());
    let herdr = FakeInstall::new(
        Some(1),
        "error: plugin build failed\nfixture build 1.2.0: deliberate failure\nPlugin was not installed.\n",
        installed_at(SHA_B),
    );
    let record = run_operation(&herdr, &operations, &request(SHA_A));
    assert_eq!(record.status, Status::Failed);
    assert_eq!(record.exit_code, Some(1));
    assert!(record.output.contains("plugin build failed"));
    assert_eq!(
        record.registry_after.as_deref(),
        Some(format!("installed at {SHA_B}").as_str())
    );

    let mut app = DetailsApp::new(target());
    app.operation_seen(Some(record));
    let text = details_text(&app);
    assert!(
        text.contains("Install of c8268d4 failed (code 1)"),
        "{text}"
    );
    assert!(
        text.contains(&format!("Registry: installed at {SHA_B}")),
        "{text}"
    );
    assert!(text.contains("Plugin was not installed."), "{text}");
}

#[test]
fn an_unreadable_registry_after_the_command_leaves_the_state_unknown() {
    let operations = FsOperations::new(state_dir());
    let herdr = FakeInstall::new(Some(0), "", Err("socket closed".into()));
    let record = run_operation(&herdr, &operations, &request(SHA_A));
    assert_eq!(record.status, Status::Unconfirmed);
    assert!(record.registry_after.unwrap().starts_with("unknown state"));

    let herdr = FakeInstall::new(Some(1), "boom", Err("socket closed".into()));
    let record = run_operation(&herdr, &operations, &request(SHA_A));
    assert_eq!(record.status, Status::Failed);
    assert!(record.registry_after.unwrap().starts_with("unknown state"));
}

#[test]
fn a_second_request_during_an_operation_is_refused() {
    let dir = state_dir();
    let operations = FsOperations::new(dir.clone());
    let running = operations.try_begin().unwrap().expect("the lock is free");
    let winner = OperationRecord::running(&request(SHA_A));
    operations.save(&winner).unwrap();

    let herdr = FakeInstall::new(Some(0), "", installed_at(SHA_A));
    let record = run_operation(&herdr, &FsOperations::new(dir.clone()), &request(SHA_A));
    assert_eq!(record.status, Status::Refused);
    assert!(herdr.ran.borrow().is_empty(), "nothing ran");
    assert_eq!(
        operations.load(&source()),
        Some(winner),
        "the loser must not overwrite the winner"
    );

    drop(running);
    assert!(operations.try_begin().unwrap().is_some());
}

#[test]
fn a_details_pane_probing_the_lock_does_not_refuse_an_operation() {
    let operations = FsOperations::new(state_dir());
    let mut running = OperationRecord::running(&request(SHA_A));
    running.worker_pid = Some(std::process::id());
    operations.save(&running).unwrap();
    for _ in 0..100 {
        assert_eq!(
            current_operation(&operations, &source()).unwrap().status,
            Status::Running
        );
        assert!(
            operations.try_begin().unwrap().is_some(),
            "a reader takes no reservation"
        );
    }
    let herdr = FakeInstall::new(Some(0), "", installed_at(SHA_A));
    assert_eq!(
        run_operation(&herdr, &operations, &request(SHA_A)).status,
        Status::Succeeded
    );
}

#[test]
fn a_new_details_pane_reads_the_kept_result_again() {
    let dir = state_dir();
    let herdr = FakeInstall::new(Some(0), "", installed_at(SHA_A));
    run_operation(&herdr, &FsOperations::new(dir.clone()), &request(SHA_A));

    let mut upper = source();
    upper.owner = "MASSDO".into();
    let record = current_operation(&FsOperations::new(dir), &upper).expect("kept result");
    assert_eq!(record.status, Status::Succeeded);

    let mut app = DetailsApp::new(target());
    app.operation_seen(Some(record));
    assert!(
        details_text(&app).contains("Install of c8268d4 succeeded"),
        "{}",
        details_text(&app)
    );
}

#[test]
fn a_result_left_running_by_a_dead_operation_is_unconfirmed() {
    let operations = FsOperations::new(state_dir());
    operations
        .save(&OperationRecord::running(&request(SHA_A)))
        .unwrap();
    let record = current_operation(&operations, &source()).unwrap();
    assert_eq!(record.status, Status::Unconfirmed);
}

#[test]
fn the_details_pane_shows_the_operation_in_progress_then_reads_the_registry() {
    let mut app = DetailsApp::new(target());
    app.intents.clear();
    app.operation_launched("op-1".into(), OperationKind::Install);
    app.operation_seen(None);
    assert!(details_text(&app).contains("Installing…"));
    assert!(app.intents.is_empty());

    let mut running = OperationRecord::running(&request(SHA_A));
    running.request.id = "op-1".into();
    app.operation_seen(Some(running.clone()));
    assert!(details_text(&app).contains("Installing c8268d4…"));
    assert!(app.intents.is_empty());

    let mut done = running;
    done.status = Status::Succeeded;
    app.operation_seen(Some(done));
    assert_eq!(app.intents, [DetailsIntent::ReadRegistry(2)]);
}

#[test]
fn results_are_kept_per_source_whatever_the_case_of_owner_and_repo() {
    let mut upper = source();
    upper.owner = "MASSDO".into();
    upper.repo = "Herdr-Marketplace-Fixture".into();
    assert_eq!(record_file_name(&upper), record_file_name(&source()));
    let mut alt = source();
    alt.subdir = "alt".into();
    let mut alt_upper = source();
    alt_upper.subdir = "ALT".into();
    assert_ne!(record_file_name(&alt), record_file_name(&alt_upper));
    assert_eq!(
        record_file_name(&alt),
        "massdo%2Fherdr-marketplace-fixture%2Falt.json"
    );
}

#[test]
fn a_stale_install_preview_cannot_replace_a_new_source_or_local_link() {
    let other = github_plugin("herdr-marketplace-fixture", "other", "plugin", None, SHA_B);
    let mut local = other.clone();
    local["source"] = serde_json::json!({"kind": "local"});
    for installed in [other, local] {
        let herdr = FakeInstall::new(Some(0), "", installed_at(SHA_A));
        *herdr.registry.borrow_mut() = Ok(registry(vec![installed]));
        let record = run_operation(&herdr, &FsOperations::new(state_dir()), &request(SHA_A));
        assert_eq!(record.status, Status::Refused);
        assert!(herdr.ran.borrow().is_empty());
    }
}

#[test]
fn a_changed_switch_commit_or_install_plan_needs_a_new_confirmation() {
    use herdr_marketplace::domain::install::Plan;
    for confirmed in [
        Plan::Install,
        Plan::Switch {
            from: "d".repeat(40),
        },
    ] {
        let mut request = request(SHA_A);
        if let Some(Confirmation::Install { plan, .. }) = &mut request.confirmation {
            *plan = confirmed;
        }
        let herdr = FakeInstall::new(Some(0), "", installed_at(SHA_A));
        *herdr.registry.borrow_mut() = installed_at(SHA_B);
        let record = run_operation(&herdr, &FsOperations::new(state_dir()), &request);
        assert_eq!(record.status, Status::Refused);
        assert!(record.output.contains("changed since the preview"));
        assert!(herdr.ran.borrow().is_empty());
    }
}

#[test]
fn a_confirmed_switch_still_runs_when_the_registry_matches() {
    let mut request = request(SHA_A);
    if let Some(Confirmation::Install { plan, .. }) = &mut request.confirmation {
        *plan = herdr_marketplace::domain::install::Plan::Switch { from: SHA_B.into() };
    }
    let herdr = FakeInstall::new(Some(0), "", installed_at(SHA_A));
    *herdr.registry.borrow_mut() = installed_at(SHA_B);
    assert_eq!(
        run_operation(&herdr, &FsOperations::new(state_dir()), &request).status,
        Status::Succeeded
    );
    assert_eq!(herdr.ran.borrow().len(), 1);
}

#[test]
fn an_unreadable_registry_or_unconfirmed_arguments_never_run() {
    for case in 0..3 {
        let herdr = FakeInstall::new(Some(0), "", installed_at(SHA_A));
        let mut request = request(SHA_A);
        match case {
            0 => *herdr.registry.borrow_mut() = Err("socket closed".into()),
            1 => request.args[2] = "other/plugin".into(),
            _ => request.confirmation = None,
        }
        assert_eq!(
            run_operation(&herdr, &FsOperations::new(state_dir()), &request).status,
            Status::Refused
        );
        assert!(herdr.ran.borrow().is_empty());
    }
}

struct FailingSave {
    inner: FsOperations,
    calls: std::cell::Cell<usize>,
    fail_at: usize,
}

impl Operations for FailingSave {
    type Guard = std::fs::File;
    fn try_begin(&self) -> Result<Option<Self::Guard>, String> {
        self.inner.try_begin()
    }
    fn load(&self, source: &PluginSource) -> Option<OperationRecord> {
        self.inner.load(source)
    }
    fn save(&self, record: &OperationRecord) -> Result<(), String> {
        self.calls.set(self.calls.get() + 1);
        if self.calls.get() == self.fail_at {
            Err("disk full".into())
        } else {
            self.inner.save(record)
        }
    }
    fn latest_finish(&self) -> u64 {
        self.inner.latest_finish()
    }
    fn worker_running(&self, pid: u32) -> bool {
        self.inner.worker_running(pid)
    }
}

#[test]
fn a_failed_initial_save_runs_nothing_and_clears_the_launch_marker() {
    let operations = FailingSave {
        inner: FsOperations::new(state_dir()),
        calls: 0.into(),
        fail_at: 1,
    };
    let old = OperationRecord {
        status: Status::Succeeded,
        finished_unix_ms: Some(1),
        ..OperationRecord::running(&request(SHA_B))
    };
    operations.inner.save(&old).unwrap();
    let herdr = FakeInstall::new(Some(0), "", installed_at(SHA_A));
    let request = request(SHA_A);
    let record = run_operation(&herdr, &operations, &request);
    assert_eq!(record.status, Status::Refused);
    assert_eq!(record.persistence_error.as_deref(), Some("disk full"));
    assert!(herdr.ran.borrow().is_empty());
    let mut app = DetailsApp::new(target());
    app.operation_launched(request.id, request.kind);
    app.operation_finished(record);
    app.operation_seen(operations.load(&source()));
    assert!(!app.operation_running());
    assert!(details_text(&app).contains("disk full"));
}

#[test]
fn a_failed_final_save_is_reported_and_not_replaced_by_the_running_file() {
    let operations = FailingSave {
        inner: FsOperations::new(state_dir()),
        calls: 0.into(),
        fail_at: 2,
    };
    let herdr = FakeInstall::new(Some(0), "", installed_at(SHA_A));
    let record = run_operation(&herdr, &operations, &request(SHA_A));
    assert_eq!(
        record.status,
        Status::Succeeded,
        "the registry confirmed the installation"
    );
    assert_eq!(record.persistence_error.as_deref(), Some("disk full"));
    assert_eq!(operations.load(&source()).unwrap().status, Status::Running);
    let mut app = DetailsApp::new(target());
    app.operation_finished(record);
    app.operation_seen(operations.load(&source()));
    assert!(!app.operation_running());
    assert!(details_text(&app).contains("Could not save result: disk full"));
}

#[test]
fn contention_is_refused_on_the_first_attempt_even_if_the_next_attempt_would_succeed() {
    struct BusyOnce {
        inner: FsOperations,
        attempts: std::cell::Cell<usize>,
    }
    impl Operations for BusyOnce {
        type Guard = std::fs::File;
        fn try_begin(&self) -> Result<Option<Self::Guard>, String> {
            self.attempts.set(self.attempts.get() + 1);
            if self.attempts.get() == 1 {
                Ok(None)
            } else {
                self.inner.try_begin()
            }
        }
        fn load(&self, source: &PluginSource) -> Option<OperationRecord> {
            self.inner.load(source)
        }
        fn save(&self, record: &OperationRecord) -> Result<(), String> {
            self.inner.save(record)
        }
        fn latest_finish(&self) -> u64 {
            self.inner.latest_finish()
        }
        fn worker_running(&self, pid: u32) -> bool {
            self.inner.worker_running(pid)
        }
    }
    let operations = BusyOnce {
        inner: FsOperations::new(state_dir()),
        attempts: 0.into(),
    };
    let herdr = FakeInstall::new(Some(0), "", installed_at(SHA_A));
    assert_eq!(
        run_operation(&herdr, &operations, &request(SHA_A)).status,
        Status::Refused
    );
    assert_eq!(operations.attempts.get(), 1);
    assert!(herdr.ran.borrow().is_empty());
}

#[test]
fn operation_output_and_the_readme_share_a_scrollable_body() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use herdr_marketplace::application::load_readme::Readme;
    let mut app = DetailsApp::new(target());
    app.readme_loaded(
        1,
        Ok(Readme::Found {
            text: "Last README line".into(),
            fallback: false,
        }),
    );
    let mut record = OperationRecord::running(&request(SHA_A));
    record.status = Status::Failed;
    record.output = (0..30).map(|i| format!("diagnostic {i}\n")).collect();
    app.operation_seen(Some(record));
    let page = details_view::page_rows(&app, 44, 10);
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
    assert!(screen(&app).contains("diagnostic 0"));
    app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
    assert!(screen(&app).contains("diagnostic 4"));
    app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
    assert!(screen(&app).contains("diagnostic 29"));
    assert!(screen(&app).contains("Last README line"));
}
