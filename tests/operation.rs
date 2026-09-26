//! Install execution and kept results, with a simulated Herdr.

mod support;

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use herdr_marketplace::adapters::operations::FsOperations;
use herdr_marketplace::adapters::tui::details::{DetailsApp, DetailsIntent};
use herdr_marketplace::adapters::tui::details_view;
use herdr_marketplace::application::ports::{CommandOutput, HerdrCli, Operations};
use herdr_marketplace::application::run_operation::{
    current_operation, operation_running, run_operation,
};
use herdr_marketplace::domain::details::DetailsTarget;
use herdr_marketplace::domain::install::install_args;
use herdr_marketplace::domain::operation::{
    OperationKind, OperationRecord, OperationRequest, Status, record_file_name,
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
    assert!(operation_running(&FsOperations::new(dir.clone())));

    let herdr = FakeInstall::new(Some(0), "", installed_at(SHA_A));
    let record = run_operation(&herdr, &FsOperations::new(dir.clone()), &request(SHA_A));
    assert_eq!(record.status, Status::Refused);
    assert!(herdr.ran.borrow().is_empty(), "nothing ran");

    drop(running);
    assert!(!operation_running(&operations));
}

#[test]
fn a_details_pane_probing_the_lock_does_not_refuse_an_operation() {
    let dir = state_dir();
    let probe = FsOperations::new(dir.clone())
        .try_begin()
        .unwrap()
        .expect("the lock is free");
    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        drop(probe);
    });
    let herdr = FakeInstall::new(Some(0), "", installed_at(SHA_A));
    let record = run_operation(&herdr, &FsOperations::new(dir), &request(SHA_A));
    release.join().unwrap();
    assert_eq!(record.status, Status::Succeeded);
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
