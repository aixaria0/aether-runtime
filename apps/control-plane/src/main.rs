use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Html,
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
mod journal;
mod kernel;

use evidence::{compare_receipts, ExecutionReceipt, ReplayComparison};
use journal::{now_ns, Journal, JournalEvent, JournalVerification, StoredExecution};
use kernel::{authorize, ExecuteInput};

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
}

#[derive(Serialize)]
struct EvidenceResponse {
    execution: StoredExecution,
    events: Vec<JournalEvent>,
    chain: JournalVerification,
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

async fn run_execution(
    state: &Arc<AppState>,
    input: ExecuteInput,
    parent_task_id: Option<String>,
) -> Result<ExecuteOutput, ApiError> {
    let permit = authorize(&input).map_err(|e| api_error(StatusCode::BAD_REQUEST, e))?;
    let task_id = permit.task_id.clone();
    let operation = permit.operation.as_str().to_string();
    let expected = permit
        .operation
        .expected(&input.payload)
        .map_err(|e| task_error(StatusCode::BAD_REQUEST, &task_id, e))?;

    append_event(
        &state.journal,
        &task_id,
        "TASK_ACCEPTED",
        payload(&[
            ("operation", operation.clone()),
            ("input_sha256", permit.input_sha256.clone()),
        ]),
    )
    .await?;
    append_event(
        &state.journal,
        &task_id,
        "POLICY_AUTHORIZED",
        payload(&[
            ("permit_id", permit.permit_id.clone()),
            ("policy_sha256", permit.policy_sha256.clone()),
            ("capabilities", permit.capability_names.join(",")),
        ]),
    )
    .await?;

    let _slot = match state.permits.clone().try_acquire_owned() {
        Ok(slot) => slot,
        Err(_) => {
            append_event(
                &state.journal,
                &task_id,
                "CAPACITY_REJECTED",
                BTreeMap::new(),
            )
            .await?;
            return Err(task_error(
                StatusCode::TOO_MANY_REQUESTS,
                &task_id,
                "execution capacity exceeded",
            ));
        }
    };

    append_event(
        &state.journal,
        &task_id,
        "EXECUTION_STARTED",
        payload(&[("executor_id", permit.executor_id.clone())]),
    )
    .await?;

    let start = Instant::now();
    let mut request = Request::new(ExecuteRequest {
        task_id: task_id.clone(),
        operation: operation.clone(),
        payload: input.payload.clone(),
    });
    request.set_timeout(Duration::from_millis(permit.deadline_ms));

    let mut client = ExecutionServiceClient::new(state.engine.clone());
    let response = tokio::time::timeout(
        Duration::from_millis(permit.deadline_ms),
        client.execute(request),
    )
    .await;

    let response = match response {
        Ok(Ok(response)) => response.into_inner(),
        Ok(Err(error)) => {
            append_event(
                &state.journal,
                &task_id,
                "EXECUTION_FAILED",
                payload(&[("grpc_code", error.code().to_string())]),
            )
            .await?;
            state.stability.lock().await.observe(false);
            return Err(task_error(
                StatusCode::BAD_GATEWAY,
                &task_id,
                format!("execution service: {}", error.code()),
            ));
        }
        Err(_) => {
            append_event(
                &state.journal,
                &task_id,
                "EXECUTION_FAILED",
                payload(&[("reason", "deadline_exceeded".into())]),
            )
            .await?;
            state.stability.lock().await.observe(false);
            return Err(task_error(
                StatusCode::GATEWAY_TIMEOUT,
                &task_id,
                "execution timeout",
            ));
        }
    };

    let elapsed_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;
    let verified = response.success
        && response.task_id == task_id
        && response.output.len() <= permit.max_output_bytes
        && response.output == expected;

    let receipt = ExecutionReceipt::new(
        &permit,
        parent_task_id.clone(),
        &response.output,
        elapsed_ms,
        verified,
    );

    save_execution(
        &state.journal,
        StoredExecution {
            task_id: task_id.clone(),
            parent_task_id,
            operation: operation.clone(),
            payload: input.payload,
            output: response.output.clone(),
            receipt: receipt.clone(),
            created_at_ns: now_ns(),
        },
    )
    .await?;

    append_event(
        &state.journal,
        &task_id,
        if verified {
            "VERIFICATION_PASSED"
        } else {
            "VERIFICATION_FAILED"
        },
        payload(&[
            ("receipt_sha256", receipt.receipt_sha256.clone()),
            ("output_sha256", receipt.output_sha256.clone()),
        ]),
    )
    .await?;

    let mut stability = state.stability.lock().await;
    stability.observe(verified);
    let delta = stability.delta;
    drop(stability);

    if !verified {
        return Err(task_error(
            StatusCode::BAD_GATEWAY,
            &task_id,
            "execution result failed independent verification",
        ));
    }

    Ok(ExecuteOutput {
        task_id,
        operation,
        output: response.output,
        success: response.success,
        error: response.error,
        output_sha256: receipt.output_sha256.clone(),
        duration_ms: elapsed_ms,
        delta,
        verified,
        receipt,
    })
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
    Ok(Json(EvidenceResponse {
        execution,
        events,
        chain,
    }))
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
    let mut client = ExecutionServiceClient::new(state.engine.clone());
    match tokio::time::timeout(
        Duration::from_secs(2),
        client.health(Request::new(proto::HealthRequest {})),
    )
    .await
    {
        Ok(Ok(response)) if response.get_ref().ready => StatusCode::OK,
        _ => StatusCode::SERVICE_UNAVAILABLE,
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

    Ok(Json(SystemStatus {
        ready: available <= state.max_concurrent_tasks && journal.valid,
        delta,
        successes,
        failures,
        active_tasks: state.max_concurrent_tasks - available,
        capacity: state.max_concurrent_tasks,
        journal_chain_valid: journal.valid,
        journal_events: journal.event_count,
        journal_head: journal.head_hash,
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

    let channel = Endpoint::from_shared(engine_url)?.connect_lazy();
    let journal = Journal::open(journal_path).map_err(std::io::Error::other)?;
    let state = Arc::new(AppState {
        engine: channel,
        permits: Arc::new(Semaphore::new(capacity)),
        max_concurrent_tasks: capacity,
        stability: Arc::new(Mutex::new(Stability::default())),
        journal,
    });

    let app = Router::new()
        .route("/", get(console))
        .route("/health", get(health))
        .route("/system/status", get(status))
        .route("/journal/verify", get(journal_verify))
        .route("/execute", post(execute))
        .route("/evidence/{task_id}", get(evidence))
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
