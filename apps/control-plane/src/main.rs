use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    env,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, Semaphore};
use tonic::{
    transport::{Channel, Endpoint},
    Request,
};

mod evidence;
mod identity;
mod journal;
mod journal_export;
mod kernel;
mod lattice;
mod scheduler;

use evidence::{compare_receipts, ExecutionReceipt, ReceiptVerification, ReplayComparison};
use identity::{Identity, IdentityInfo};
use journal::{now_ns, Journal, JournalEvent, JournalVerification, StoredExecution};
use kernel::{authorize_for_executor, ExecuteInput, ExecutionPermit};
use scheduler::{AdaptiveScheduler, SchedulerSnapshot, CPP_EXECUTOR, RUST_EXECUTOR};
use uuid::Uuid;

#[allow(clippy::result_large_err)]
pub mod proto {
    tonic::include_proto!("aether.v1");
}
use proto::{execution_service_client::ExecutionServiceClient, ExecuteRequest};

#[derive(Clone)]
struct AppState {
    engine: Channel,
    permits: Arc<Semaphore>,
    max_concurrent_tasks: usize,
    stability: Arc<Mutex<Stability>>,
    journal: Journal,
    scheduler: Arc<Mutex<AdaptiveScheduler>>,
    identity: Identity,
}

#[derive(Debug)]
struct Stability {
    successes: u64,
    failures: u64,
    delta: f64,
}

impl Default for Stability {
    fn default() -> Self {
        Self {
            successes: 0,
            failures: 0,
            delta: 1.0,
        }
    }
}

impl Stability {
    fn observe(&mut self, success: bool) {
        if success {
            self.successes += 1;
        } else {
            self.failures += 1;
        }
        // Process-local EWMA health indicator, not a consistency proof.
        let signal = if success { 1.0 } else { 0.0 };
        self.delta = (0.9 * self.delta + 0.1 * signal).clamp(0.0, 1.0);
    }
}

#[derive(Clone, Debug, Serialize)]
struct ExecuteOutput {
    task_id: String,
    operation: String,
    output: String,
    success: bool,
    error: String,
    output_sha256: String,
    duration_ms: u64,
    delta: f64,
    verified: bool,
    receipt: ExecutionReceipt,
}

#[derive(Serialize)]
struct SystemStatus {
    ready: bool,
    delta: f64,
    successes: u64,
    failures: u64,
    active_tasks: usize,
    capacity: usize,
    journal_chain_valid: bool,
    journal_events: u64,
    journal_head: Option<String>,
    scheduler: SchedulerSnapshot,
    identity: IdentityInfo,
}

#[derive(Serialize)]
struct EvidenceResponse {
    execution: StoredExecution,
    events: Vec<JournalEvent>,
    chain: JournalVerification,
    receipt_verification: ReceiptVerification,
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_id: Option<String>,
}

type ApiError = (StatusCode, Json<ErrorBody>);

fn api_error(status: StatusCode, message: impl Into<String>) -> ApiError {
    (
        status,
        Json(ErrorBody {
            error: message.into(),
            task_id: None,
        }),
    )
}

fn task_error(status: StatusCode, task_id: &str, message: impl Into<String>) -> ApiError {
    (
        status,
        Json(ErrorBody {
            error: message.into(),
            task_id: Some(task_id.to_string()),
        }),
    )
}

fn payload(entries: &[(&str, String)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect()
}

async fn append_event(
    journal: &Journal,
    task_id: &str,
    event_type: &str,
    event_payload: BTreeMap<String, String>,
) -> Result<(), ApiError> {
    let journal = journal.clone();
    let task_id = task_id.to_string();
    let event_type = event_type.to_string();
    tokio::task::spawn_blocking(move || journal.append_event(&task_id, &event_type, event_payload))
        .await
        .map_err(|e| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("journal task: {e}"),
            )
        })?
        .map(|_| ())
        .map_err(|e| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("journal write: {e}"),
            )
        })
}

async fn save_execution(journal: &Journal, record: StoredExecution) -> Result<(), ApiError> {
    let journal = journal.clone();
    tokio::task::spawn_blocking(move || journal.save_execution(&record))
        .await
        .map_err(|e| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("journal task: {e}"),
            )
        })?
        .map_err(|e| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("journal write: {e}"),
            )
        })
}

async fn load_execution(
    journal: &Journal,
    task_id: String,
) -> Result<Option<StoredExecution>, ApiError> {
    let journal = journal.clone();
    tokio::task::spawn_blocking(move || journal.load_execution(&task_id))
        .await
        .map_err(|e| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("journal task: {e}"),
            )
        })?
        .map_err(|e| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("journal read: {e}"),
            )
        })
}

async fn events_for_task(
    journal: &Journal,
    task_id: String,
) -> Result<Vec<JournalEvent>, ApiError> {
    let journal = journal.clone();
    tokio::task::spawn_blocking(move || journal.events_for_task(&task_id))
        .await
        .map_err(|e| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("journal task: {e}"),
            )
        })?
        .map_err(|e| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("journal read: {e}"),
            )
        })
}

async fn verify_journal(journal: &Journal) -> Result<JournalVerification, ApiError> {
    let journal = journal.clone();
    tokio::task::spawn_blocking(move || journal.verify_chain())
        .await
        .map_err(|e| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("journal task: {e}"),
            )
        })?
        .map_err(|e| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("journal verify: {e}"),
            )
        })
}

async fn persist_executor_stats(
    journal: &Journal,
    stats: scheduler::ExecutorStats,
) -> Result<(), ApiError> {
    let journal = journal.clone();
    tokio::task::spawn_blocking(move || journal.save_executor_stats(&stats))
        .await
        .map_err(|e| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("scheduler persistence task: {e}"),
            )
        })?
        .map_err(|e| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("scheduler persistence: {e}"),
            )
        })
}

struct ExecutorAttempt {
    output: String,
    success: bool,
    error: String,
    elapsed_ms: u64,
}

async fn execute_on_executor(
    state: &Arc<AppState>,
    permit: &ExecutionPermit,
    payload_text: &str,
) -> Result<ExecutorAttempt, String> {
    let start = Instant::now();
    if permit.executor_id == RUST_EXECUTOR {
        let output = permit
            .operation
            .expected(payload_text)
            .map_err(str::to_string)?;
        return Ok(ExecutorAttempt {
            output,
            success: true,
            error: String::new(),
            elapsed_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
        });
    }
    if permit.executor_id != CPP_EXECUTOR {
        return Err(format!("unknown executor {}", permit.executor_id));
    }

    let mut request = Request::new(ExecuteRequest {
        task_id: permit.task_id.clone(),
        operation: permit.operation.as_str().to_string(),
        payload: payload_text.to_string(),
    });
    request.set_timeout(Duration::from_millis(permit.deadline_ms));
    let mut client = ExecutionServiceClient::new(state.engine.clone());
    match tokio::time::timeout(
        Duration::from_millis(permit.deadline_ms),
        client.execute(request),
    )
    .await
    {
        Ok(Ok(response)) => {
            let response = response.into_inner();
            if response.task_id != permit.task_id {
                return Err("executor returned mismatched task id".into());
            }
            Ok(ExecutorAttempt {
                output: response.output,
                success: response.success,
                error: response.error,
                elapsed_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
            })
        }
        Ok(Err(error)) => Err(format!("grpc:{}", error.code())),
        Err(_) => Err("deadline_exceeded".into()),
    }
}

async fn run_execution(
    state: &Arc<AppState>,
    mut input: ExecuteInput,
    parent_task_id: Option<String>,
) -> Result<ExecuteOutput, ApiError> {
    let mut excluded: Vec<String> = Vec::new();
    let mut first_task_id: Option<String> = None;
    let mut last_error = String::from("no executor available");

    let _slot = state
        .permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| api_error(StatusCode::TOO_MANY_REQUESTS, "execution capacity exceeded"))?;

    for attempt_index in 0..2 {
        let executor_id = {
            let scheduler = state.scheduler.lock().await;
            let excluded_refs: Vec<&str> = excluded.iter().map(String::as_str).collect();
            scheduler.select(&excluded_refs)
        }
        .ok_or_else(|| {
            api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "no healthy executor available",
            )
        })?;

        let permit = authorize_for_executor(&input, &executor_id)
            .map_err(|e| api_error(StatusCode::BAD_REQUEST, e))?;
        if first_task_id.is_none() {
            first_task_id = Some(permit.task_id.clone());
            input.task_id = Uuid::parse_str(&permit.task_id).ok();
            append_event(
                &state.journal,
                &permit.task_id,
                "TASK_ACCEPTED",
                payload(&[
                    ("operation", permit.operation.as_str().to_string()),
                    ("input_sha256", permit.input_sha256.clone()),
                ]),
            )
            .await?;
        }
        let task_id = first_task_id
            .clone()
            .unwrap_or_else(|| permit.task_id.clone());

        append_event(
            &state.journal,
            &task_id,
            "POLICY_AUTHORIZED",
            payload(&[
                ("executor_id", executor_id.clone()),
                ("policy_sha256", permit.policy_sha256.clone()),
                ("capabilities", permit.capability_names.join(",")),
            ]),
        )
        .await?;

        append_event(
            &state.journal,
            &task_id,
            if attempt_index == 0 {
                "EXECUTOR_SELECTED"
            } else {
                "FAILOVER_SELECTED"
            },
            payload(&[
                ("executor_id", executor_id.clone()),
                ("policy_sha256", permit.policy_sha256.clone()),
                ("attempt", attempt_index.to_string()),
            ]),
        )
        .await?;
        append_event(
            &state.journal,
            &task_id,
            "EXECUTION_STARTED",
            payload(&[("executor_id", executor_id.clone())]),
        )
        .await?;

        let result = execute_on_executor(state, &permit, &input.payload).await;

        match result {
            Err(error) => {
                last_error = error.clone();
                let observed = {
                    let mut scheduler = state.scheduler.lock().await;
                    scheduler.observe(&executor_id, false, 0)
                };
                persist_executor_stats(&state.journal, observed).await?;
                append_event(
                    &state.journal,
                    &task_id,
                    "EXECUTOR_FAILED",
                    payload(&[("executor_id", executor_id.clone()), ("reason", error)]),
                )
                .await?;
                excluded.push(executor_id);
                continue;
            }
            Ok(result) => {
                let expected = permit
                    .operation
                    .expected(&input.payload)
                    .map_err(|e| task_error(StatusCode::BAD_REQUEST, &task_id, e))?;
                let verified = result.success
                    && result.output.len() <= permit.max_output_bytes
                    && result.output == expected;
                if !verified {
                    last_error = "independent verification failed".into();
                    let observed = {
                        let mut scheduler = state.scheduler.lock().await;
                        scheduler.observe(&executor_id, false, result.elapsed_ms)
                    };
                    persist_executor_stats(&state.journal, observed).await?;
                    append_event(
                        &state.journal,
                        &task_id,
                        "VERIFICATION_FAILED",
                        payload(&[("executor_id", executor_id.clone())]),
                    )
                    .await?;
                    excluded.push(executor_id);
                    continue;
                }

                let observed = {
                    let mut scheduler = state.scheduler.lock().await;
                    scheduler.observe(&executor_id, true, result.elapsed_ms)
                };
                persist_executor_stats(&state.journal, observed).await?;

                let mut receipt = ExecutionReceipt::new(
                    &permit,
                    parent_task_id.clone(),
                    &result.output,
                    result.elapsed_ms,
                    true,
                );
                receipt.attest(&state.identity);
                save_execution(
                    &state.journal,
                    StoredExecution {
                        task_id: task_id.clone(),
                        parent_task_id,
                        operation: permit.operation.as_str().to_string(),
                        payload: input.payload,
                        output: result.output.clone(),
                        receipt: receipt.clone(),
                        created_at_ns: now_ns(),
                    },
                )
                .await?;
                let attestation = receipt
                    .attestation
                    .as_ref()
                    .expect("new receipts are always attested");
                append_event(
                    &state.journal,
                    &task_id,
                    "RECEIPT_SIGNED",
                    payload(&[
                        ("algorithm", attestation.algorithm.clone()),
                        (
                            "signer_fingerprint",
                            attestation.key_fingerprint_sha256.clone(),
                        ),
                        ("receipt_sha256", receipt.receipt_sha256.clone()),
                    ]),
                )
                .await?;
                append_event(
                    &state.journal,
                    &task_id,
                    "VERIFICATION_PASSED",
                    payload(&[
                        ("executor_id", executor_id),
                        ("receipt_sha256", receipt.receipt_sha256.clone()),
                    ]),
                )
                .await?;

                let mut stability = state.stability.lock().await;
                stability.observe(true);
                let delta = stability.delta;
                drop(stability);
                return Ok(ExecuteOutput {
                    task_id,
                    operation: permit.operation.as_str().to_string(),
                    output: result.output,
                    success: true,
                    error: result.error,
                    output_sha256: receipt.output_sha256.clone(),
                    duration_ms: result.elapsed_ms,
                    delta,
                    verified: true,
                    receipt,
                });
            }
        }
    }

    state.stability.lock().await.observe(false);
    let task_id = first_task_id.unwrap_or_else(|| "unknown".into());
    Err(task_error(
        StatusCode::BAD_GATEWAY,
        &task_id,
        format!("all executors failed: {last_error}"),
    ))
}

async fn execute(
    State(state): State<Arc<AppState>>,
    Json(input): Json<ExecuteInput>,
) -> Result<Json<ExecuteOutput>, ApiError> {
    run_execution(&state, input, None).await.map(Json)
}

async fn evidence(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> Result<Json<EvidenceResponse>, ApiError> {
    let execution = load_execution(&state.journal, task_id.clone())
        .await?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "task evidence not found"))?;
    let events = events_for_task(&state.journal, task_id).await?;
    let chain = verify_journal(&state.journal).await?;
    let receipt_verification = execution.receipt.verify_attestation();
    Ok(Json(EvidenceResponse {
        execution,
        events,
        chain,
        receipt_verification,
    }))
}

async fn identity_info(State(state): State<Arc<AppState>>) -> Json<IdentityInfo> {
    Json(state.identity.info())
}

async fn verify_receipt(Json(receipt): Json<ExecutionReceipt>) -> Json<ReceiptVerification> {
    Json(receipt.verify_attestation())
}

async fn lattice_ontology() -> Json<serde_json::Value> {
    Json(serde_json::from_str(lattice::ONTOLOGY).expect("embedded ontology is validated by tests"))
}

async fn lineage(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> Result<Json<lattice::LineageCertificate>, ApiError> {
    let journal = state.journal.clone();
    let trusted_fingerprint = state.identity.info().fingerprint_sha256;
    // SQLite reads, chain verification, signatures and recomputation run off
    // the async worker, using a single consistent journal read transaction.
    tokio::task::spawn_blocking(move || {
        journal.execution_snapshot(&task_id).map(|snapshot| {
            snapshot.map(|snapshot| lattice::assess(snapshot, &trusted_fingerprint))
        })
    })
    .await
    .map_err(|e| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("lineage task: {e}"),
        )
    })?
    .map_err(|e| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("lineage read: {e}"),
        )
    })?
    .map(Json)
    .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "task evidence not found"))
}

async fn lineage_journal(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> Result<Response, ApiError> {
    let journal = state.journal.clone();
    let signer = state.identity.info().fingerprint_sha256;
    let bytes = tokio::task::spawn_blocking(move || {
        journal
            .bounded_snapshot(&task_id)?
            .map(|snapshot| journal_export::encode(snapshot, &signer))
            .transpose()
    })
    .await
    .map_err(|e| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("journal export task: {e}"),
        )
    })?
    .map_err(|e| match e {
        journal::ExportError::Limit => api_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "journal export exceeds bounded evidence limits",
        ),
        journal::ExportError::Storage(message) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("journal export: {message}"),
        ),
    })?
    .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "task evidence not found"))?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        bytes,
    )
        .into_response())
}

async fn replay(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> Result<Json<ReplayComparison>, ApiError> {
    let original = load_execution(&state.journal, task_id.clone())
        .await?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "task evidence not found"))?;

    let replay_output = run_execution(
        &state,
        ExecuteInput {
            operation: original.operation.clone(),
            payload: original.payload.clone(),
            task_id: None,
            deadline_ms: original.receipt.deadline_ms,
        },
        Some(task_id),
    )
    .await?;

    let comparison = compare_receipts(&original.receipt, &replay_output.receipt);
    let divergence = comparison
        .first_divergence
        .as_ref()
        .map(|value| format!("{value:?}"))
        .unwrap_or_else(|| "none".into());
    append_event(
        &state.journal,
        &comparison.replay_task_id,
        if comparison.matched {
            "REPLAY_MATCHED"
        } else {
            "REPLAY_DIVERGED"
        },
        payload(&[
            ("original_task_id", comparison.original_task_id.clone()),
            ("first_divergence", divergence),
        ]),
    )
    .await?;
    Ok(Json(comparison))
}

async fn console() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
}

async fn health(State(state): State<Arc<AppState>>) -> StatusCode {
    // Readiness is intentionally cheap: journal integrity is verified by /system/status
    // and /journal/verify rather than rescanning the full hash chain on every probe.
    if state.scheduler.lock().await.snapshot().selected.is_some() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

async fn status(State(state): State<Arc<AppState>>) -> Result<Json<SystemStatus>, ApiError> {
    let stability = state.stability.lock().await;
    let available = state.permits.available_permits();
    let delta = stability.delta;
    let successes = stability.successes;
    let failures = stability.failures;
    drop(stability);
    let journal = verify_journal(&state.journal).await?;
    let scheduler = state.scheduler.lock().await.snapshot();

    Ok(Json(SystemStatus {
        ready: scheduler.selected.is_some() && journal.valid,
        delta,
        successes,
        failures,
        active_tasks: state.max_concurrent_tasks - available,
        capacity: state.max_concurrent_tasks,
        journal_chain_valid: journal.valid,
        journal_events: journal.event_count,
        journal_head: journal.head_hash,
        scheduler,
        identity: state.identity.info(),
    }))
}

async fn journal_verify(
    State(state): State<Arc<AppState>>,
) -> Result<Json<JournalVerification>, ApiError> {
    verify_journal(&state.journal).await.map(Json)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    let engine_url = env::var("ENGINE_URL").unwrap_or_else(|_| "http://127.0.0.1:50051".into());
    let capacity = env::var("MAX_CONCURRENT_TASKS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| (1..=4096).contains(value))
        .unwrap_or(32);
    let journal_path =
        env::var("AETHER_JOURNAL_PATH").unwrap_or_else(|_| "./aether-journal.sqlite3".into());
    let identity_path =
        env::var("AETHER_IDENTITY_PATH").unwrap_or_else(|_| "./aether-identity.key".into());

    let channel = Endpoint::from_shared(engine_url)?.connect_lazy();
    let journal = Journal::open(journal_path).map_err(std::io::Error::other)?;
    let scheduler_seed = journal
        .load_executor_stats()
        .map_err(std::io::Error::other)?;
    let identity = Identity::load_or_create(identity_path).map_err(std::io::Error::other)?;
    let state = Arc::new(AppState {
        engine: channel,
        permits: Arc::new(Semaphore::new(capacity)),
        max_concurrent_tasks: capacity,
        stability: Arc::new(Mutex::new(Stability::default())),
        scheduler: Arc::new(Mutex::new(AdaptiveScheduler::new(scheduler_seed))),
        identity,
        journal,
    });

    let app = Router::new()
        .route("/", get(console))
        .route("/health", get(health))
        .route("/system/status", get(status))
        .route("/journal/verify", get(journal_verify))
        .route("/identity", get(identity_info))
        .route("/verify/receipt", post(verify_receipt))
        .route("/execute", post(execute))
        .route("/evidence/{task_id}", get(evidence))
        .route("/lattice/ontology", get(lattice_ontology))
        .route("/lattice/{task_id}", get(lineage))
        .route("/lattice/{task_id}/journal", get(lineage_journal))
        .route("/replay/{task_id}", post(replay))
        .with_state(state)
        .layer(tower_http::trace::TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stability_is_bounded_and_sensitive_to_failure() {
        let mut health = Stability::default();
        health.observe(false);
        assert!(health.delta < 1.0 && health.delta >= 0.0);
        health.observe(true);
        assert_eq!((health.successes, health.failures), (1, 1));
        assert!(health.delta <= 1.0);
    }
}
