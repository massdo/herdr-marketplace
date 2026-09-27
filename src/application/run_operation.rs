use std::time::{SystemTime, UNIX_EPOCH};

use crate::application::load_listing::read_registry;
use crate::application::ports::{HerdrCli, Operations};
use crate::domain::compat::Platform;
use crate::domain::install::{check_target, install_args, plan};
use crate::domain::operation::{
    Confirmation, OperationKind, OperationRecord, OperationRequest, Status, operation_status,
    registry_state,
};
use crate::domain::source::PluginSource;
use crate::domain::uninstall::plan_removal;
use crate::domain::version::Version;

/// Herdr's output kept in a result, from the end.
pub const OUTPUT_LIMIT: usize = 64 * 1024;

/// Entry point for a worker without an inherited reservation. Refuse at once
/// when busy, without touching the running operation's result file.
pub fn run_operation<H: HerdrCli, O: Operations>(
    herdr: &H,
    operations: &O,
    request: &OperationRequest,
) -> OperationRecord {
    match operations.try_begin() {
        Ok(Some(guard)) => run_locked_operation(herdr, operations, request, guard),
        result => {
            let mut record = OperationRecord::running(request);
            record.status = Status::Refused;
            record.output = result
                .err()
                .unwrap_or_else(|| "another marketplace operation is running".into());
            record.finished_unix_ms = Some(now_ms());
            record
        }
    }
}

/// Holds the reservation taken by the confirming pane through validation,
/// execution and persistence. The worker owns it even if the pane closes.
pub fn run_locked_operation<H: HerdrCli, O: Operations>(
    herdr: &H,
    operations: &O,
    request: &OperationRequest,
    _guard: O::Guard,
) -> OperationRecord {
    let mut record = OperationRecord::running(request);
    record.worker_pid = Some(std::process::id());
    record.worker_started = operations.worker_started(std::process::id());
    if let Err(error) = operations.save(&record) {
        record.status = Status::Refused;
        record.output = "operation not started: could not save its initial state".into();
        record.persistence_error = Some(error);
        record.finished_unix_ms = Some(now_ms());
        return record;
    }
    let args = match revalidate(herdr, request) {
        Ok(args) => args,
        Err(error) => return finish(operations, record, Status::Refused, error),
    };
    let run = herdr.run(&args);
    let registry = read_registry(herdr);
    let (code, output) = match run {
        Ok(output) => (output.code, output.output),
        Err(error) => (None, error),
    };
    record.exit_code = code;
    record.registry_after = Some(registry_state(&registry, &request.source));
    let status = operation_status(code, &registry, request);
    finish(operations, record, status, tail(&output))
}

/// Rebuild the command from the confirmed target and a fresh registry. A
/// changed installation needs a new preview, including a switch whose old
/// commit changed while the preview was open.
fn revalidate<H: HerdrCli>(herdr: &H, request: &OperationRequest) -> Result<Vec<String>, String> {
    let changed = "installation changed since the preview; open it again to confirm";
    let args = match (&request.kind, &request.confirmation) {
        (
            OperationKind::Install,
            Some(Confirmation::Install {
                target,
                manifest,
                plan: confirmed,
            }),
        ) => {
            check_target(target)?;
            if target.source != request.source || target.commit != request.commit {
                return Err("request does not match the confirmed target".into());
            }
            let version = herdr
                .version()
                .ok()
                .and_then(|output| Version::from_herdr_output(&output))
                .ok_or("cannot determine the Herdr version")?;
            let registry = read_registry(herdr)?;
            let current = plan(target, manifest, &registry, Platform::current(), version)?;
            if current != *confirmed {
                return Err(changed.into());
            }
            install_args(&target.source, &target.commit)
        }
        (OperationKind::Uninstall, Some(Confirmation::Uninstall { installed })) => {
            let registry = read_registry(herdr)?;
            let current = plan_removal(&registry, &request.source)?;
            if current.installed != *installed {
                return Err(changed.into());
            }
            current.args
        }
        _ => return Err("missing or mismatched confirmation; open the preview again".into()),
    };
    if args != request.args {
        return Err("request arguments do not match the confirmation".into());
    }
    Ok(args)
}

fn finish<O: Operations>(
    operations: &O,
    mut record: OperationRecord,
    status: Status,
    output: String,
) -> OperationRecord {
    record.status = status;
    record.output = output;
    record.finished_unix_ms = Some(now_ms());
    if let Err(error) = operations.save(&record) {
        record.persistence_error = Some(error);
    }
    record
}

/// A dead worker's unfinished record is unconfirmed. Observing it never
/// acquires the operation lock, so polling cannot refuse an installation.
pub fn current_operation<O: Operations>(
    operations: &O,
    source: &PluginSource,
) -> Option<OperationRecord> {
    let mut record = operations.load(source)?;
    if record.status == Status::Running
        && !record
            .worker_pid
            .is_some_and(|pid| operations.worker_running(pid, record.worker_started))
    {
        record.status = Status::Unconfirmed;
        record.output = "the operation stopped before saving its result".into();
    }
    Some(record)
}

fn tail(output: &str) -> String {
    if output.len() <= OUTPUT_LIMIT {
        return output.to_string();
    }
    let mut start = output.len() - OUTPUT_LIMIT;
    while !output.is_char_boundary(start) {
        start += 1;
    }
    output[start..].to_string()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}
