//! Real detached worker, with a fake Herdr and an isolated state directory.
mod support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use herdr_marketplace::adapters::operations::{FsOperations, spawn_operation};
use herdr_marketplace::application::ports::Operations;
use herdr_marketplace::application::run_operation::current_operation;
use herdr_marketplace::domain::details::DetailsTarget;
use herdr_marketplace::domain::install::{Plan, install_args};
use herdr_marketplace::domain::manifest::parse_manifest;
use herdr_marketplace::domain::operation::{
    Confirmation, OperationKind, OperationRecord, OperationRequest, Status, record_file_name,
};
use herdr_marketplace::domain::source::PluginSource;
use support::*;

struct Fixture {
    dir: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "herdr-marketplace-worker-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("registry.json"), registry(vec![])).unwrap();
        fs::write(
            dir.join("after.json"),
            registry(vec![github_plugin(
                "fixture", "acme", "plugin", None, SHA_A,
            )]),
        )
        .unwrap();
        executable(
            &dir.join("herdr"),
            r#"#!/bin/sh
case "$1 $2" in
  '--version ') echo 'herdr 0.9.1' ;;
  'plugin list') cat "$WORKER_TEST_DIR/registry.json" ;;
  'plugin install')
    touch "$WORKER_TEST_DIR/started"
    attempt=0
    while [ ! -f "$WORKER_TEST_DIR/release" ]; do
      attempt=$((attempt + 1))
      if [ "$attempt" -gt 500 ]; then exit 2; fi
      sleep 0.01
    done
    cp "$WORKER_TEST_DIR/after.json" "$WORKER_TEST_DIR/registry.json"
    echo Installed ;;
  *) exit 3 ;;
esac
"#,
        );
        let binary = env!("CARGO_BIN_EXE_herdr-marketplace").replace('\'', "'\\''");
        executable(
            &dir.join("worker"),
            &format!(
                r#"#!/bin/sh
export WORKER_TEST_DIR="$(dirname "$0")"
export HERDR_BIN_PATH="$WORKER_TEST_DIR/herdr"
export HERDR_PLUGIN_STATE_DIR="$WORKER_TEST_DIR"
export HERDR_SOCKET_PATH="$WORKER_TEST_DIR/unused.sock"
exec '{binary}' "$@"
"#
            ),
        );
        Self { dir }
    }
    fn operations(&self) -> FsOperations {
        FsOperations::new(self.dir.clone())
    }
    fn wait_started(&self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.dir.join("started").exists() {
            assert!(
                Instant::now() < deadline,
                "worker did not run the fake Herdr"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn release(&self) {
        fs::write(self.dir.join("release"), "").unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}
fn executable(path: &Path, script: &str) {
    fs::write(path, script).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}
fn request() -> OperationRequest {
    let source = PluginSource {
        owner: "acme".into(),
        repo: "plugin".into(),
        subdir: String::new(),
    };
    let target = DetailsTarget {
        source: source.clone(),
        commit: SHA_A.into(),
        id: "fixture".into(),
        name: "fixture".into(),
        version: None,
        in_catalog: true,
        compatible: true,
    };
    OperationRequest {
        id: "worker-test".into(),
        kind: OperationKind::Install,
        source: source.clone(),
        commit: SHA_A.into(),
        args: install_args(&source, SHA_A),
        confirmation: Some(Confirmation::Install {
            target,
            manifest: parse_manifest(
                "id = 'fixture'\nname = 'fixture'\nversion = '1.0.0'\nmin_herdr_version = '0.9.1'",
            )
            .unwrap()
            .into(),
            plan: Plan::Install,
        }),
    }
}

#[test]
fn reservation_is_inherited_and_installation_survives_the_pane_closing() {
    let fixture = Fixture::new();
    let operations = fixture.operations();
    let request = request();
    let guard = operations.try_begin().unwrap().unwrap();
    let mut worker = spawn_operation(&request, guard, &fixture.dir.join("worker")).unwrap();
    assert!(
        operations.try_begin().unwrap().is_none(),
        "reservation survives spawn"
    );
    fixture.wait_started();
    assert!(
        operations.try_begin().unwrap().is_none(),
        "worker still owns the reservation"
    );
    assert_eq!(
        current_operation(&operations, &request.source)
            .unwrap()
            .status,
        Status::Running
    );
    drop(worker.stdout.take());
    drop(worker.stderr.take());
    fixture.release();
    assert!(worker.wait().unwrap().success());
    assert_eq!(
        operations.load(&request.source).unwrap().status,
        Status::Succeeded
    );
    assert!(operations.try_begin().unwrap().is_some());
}

#[test]
fn a_worker_reports_a_failed_initial_write_without_running_herdr() {
    let fixture = Fixture::new();
    let operations = fixture.operations();
    let request = request();
    let guard = operations.try_begin().unwrap().unwrap();
    fs::create_dir(
        fixture
            .dir
            .join("operations")
            .join(record_file_name(&request.source)),
    )
    .unwrap();
    let worker = spawn_operation(&request, guard, &fixture.dir.join("worker")).unwrap();
    let output = worker.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let record: OperationRecord = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(record.status, Status::Refused);
    assert!(record.persistence_error.is_some());
    assert!(!fixture.dir.join("started").exists());
    assert!(operations.try_begin().unwrap().is_some());
}

#[test]
fn a_dead_worker_is_unconfirmed_and_triggers_a_sidebar_refresh() {
    let fixture = Fixture::new();
    let operations = fixture.operations();
    let request = request();
    let guard = operations.try_begin().unwrap().unwrap();
    let mut worker = spawn_operation(&request, guard, &fixture.dir.join("worker")).unwrap();
    fixture.wait_started();
    assert_eq!(operations.latest_finish(), 0);
    worker.kill().unwrap();
    worker.wait().unwrap();
    assert!(
        operations.try_begin().unwrap().is_none(),
        "Herdr still owns the reservation after the worker dies"
    );
    fixture.release();
    assert_eq!(
        current_operation(&operations, &request.source)
            .unwrap()
            .status,
        Status::Unconfirmed
    );
    assert!(operations.latest_finish() > 0);
    let deadline = Instant::now() + Duration::from_secs(5);
    while operations.try_begin().unwrap().is_none() {
        assert!(
            Instant::now() < deadline,
            "Herdr did not release the reservation"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn confirmed_manifests_are_not_limited_by_the_os_argument_size() {
    let fixture = Fixture::new();
    let operations = fixture.operations();
    let mut request = request();
    if let Some(Confirmation::Install { manifest, .. }) = &mut request.confirmation {
        manifest.name = "long manifest ".repeat(25_000);
    }
    fixture.release();
    let guard = operations.try_begin().unwrap().unwrap();
    let output = spawn_operation(&request, guard, &fixture.dir.join("worker"))
        .unwrap()
        .wait_with_output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let record: OperationRecord = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(record.status, Status::Succeeded);
}
