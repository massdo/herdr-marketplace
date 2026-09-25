use std::time::{SystemTime, UNIX_EPOCH};

use crate::application::load_listing::read_registry;
use crate::application::ports::{HerdrCli, Operations};
use crate::domain::operation::{
    OperationRecord, OperationRequest, Status, install_status, registry_state,
};
use crate::domain::source::PluginSource;

/// Herdr's output kept in a result, from the end.
pub const OUTPUT_LIMIT: usize = 64 * 1024;

/// Runs a confirmed request to its end and keeps the result, whoever
/// watches. Refused while another marketplace operation runs.
pub fn run_operation<H: HerdrCli, O: Operations>(
    herdr: &H,
    operations: &O,
    request: &OperationRequest,
) -> OperationRecord {
    let mut record = OperationRecord::running(request);
    let _guard = match operations.try_begin() {
        Ok(Some(guard)) => guard,
        Ok(None) => {
            return finish(
                operations,
                record,
                Status::Refused,
                "une autre opération de la marketplace est en cours".into(),
            );
        }
        Err(error) => return finish(operations, record, Status::Refused, error),
    };
    let _ = operations.save(&record);

    let run = herdr.run(&request.args);
    let registry = read_registry(herdr);
    let (code, output) = match run {
        Ok(output) => (output.code, output.output),
        Err(error) => (None, error),
    };
    record.exit_code = code;
    record.registry_after = Some(registry_state(&registry, &request.source));
    let status = install_status(code, &registry, request);
    finish(operations, record, status, tail(&output))
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
    let _ = operations.save(&record);
    record
}

/// The kept result of a source. A result still marked running while nobody
/// holds the lock belongs to an operation that died: its outcome is unknown.
pub fn current_operation<O: Operations>(
    operations: &O,
    source: &PluginSource,
) -> Option<OperationRecord> {
    let mut record = operations.load(source)?;
    if record.status == Status::Running && matches!(operations.try_begin(), Ok(Some(_))) {
        record.status = Status::Unconfirmed;
        record.output = "l'opération s'est arrêtée avant de rendre son résultat".into();
    }
    Some(record)
}

/// Whether a marketplace operation is running right now.
pub fn operation_running<O: Operations>(operations: &O) -> bool {
    matches!(operations.try_begin(), Ok(None))
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
