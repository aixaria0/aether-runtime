//! Bounded, read-only export from one SQLite snapshot. Only receipts are signed.
use crate::{
    evidence::ExecutionReceipt,
    journal::{BoundedSnapshot, StoredExecution},
    lattice,
};
use serde::Serialize;

pub const MAX_BYTES: usize = 1024 * 1024;

#[derive(Serialize)]
pub struct RawExecution {
    task_id: String,
    parent_task_id: Option<String>,
    operation: String,
    payload: String,
    output: String,
    receipt: ExecutionReceipt,
}

impl From<&StoredExecution> for RawExecution {
    fn from(record: &StoredExecution) -> Self {
        Self {
            task_id: record.task_id.clone(),
            parent_task_id: record.parent_task_id.clone(),
            operation: record.operation.clone(),
            payload: record.payload.clone(),
            output: record.output.clone(),
            receipt: record.receipt.clone(),
        }
    }
}

#[derive(Serialize)]
struct Event {
    sequence: i64,
    event_id: String,
    task_id: String,
    event_type: String,
    timestamp_ns_decimal: String,
    payload_json: String,
    previous_hash: Option<String>,
    event_hash: String,
}

#[derive(Serialize)]
struct Lineage {
    schema: &'static str,
    certificate: lattice::LineageCertificate,
    execution: RawExecution,
    parent: Option<RawExecution>,
}

#[derive(Serialize)]
struct Events {
    schema: &'static str,
    coverage: &'static str,
    events: Vec<Event>,
}

#[derive(Serialize)]
struct Export {
    schema: &'static str,
    lineage: Lineage,
    journal: Events,
}

pub fn encode(
    snapshot: BoundedSnapshot,
    signer: &str,
) -> Result<Vec<u8>, crate::journal::ExportError> {
    let execution = RawExecution::from(&snapshot.snapshot.execution);
    let parent = snapshot.snapshot.parent.as_ref().map(RawExecution::from);
    let certificate = lattice::assess(snapshot.snapshot, signer);
    let events = snapshot
        .journal_events
        .into_iter()
        .map(|e| Event {
            sequence: e.sequence,
            event_id: e.event_id,
            task_id: e.task_id,
            event_type: e.event_type,
            timestamp_ns_decimal: e.timestamp_ns.to_string(),
            payload_json: e.payload_json,
            previous_hash: e.previous_hash,
            event_hash: e.event_hash,
        })
        .collect();
    let export = Export {
        schema: "aether-journal-export/v1",
        lineage: Lineage {
            schema: "chimera-aether-lineage-evidence/v1",
            certificate,
            execution,
            parent,
        },
        journal: Events {
            schema: "aether-journal-events/v1",
            coverage: "genesis_to_snapshot_head",
            events,
        },
    };
    let bytes = serde_json::to_vec(&export)
        .map_err(|e| crate::journal::ExportError::Storage(e.to_string()))?;
    if bytes.len() > MAX_BYTES {
        return Err(crate::journal::ExportError::Limit);
    }
    Ok(bytes)
}
