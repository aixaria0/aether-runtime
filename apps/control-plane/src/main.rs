use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router, response::Html,
};
use serde::Serialize;
mod policy;
use policy::{TaskInput, ExecutionReceipt, authorize, make_receipt};
use std::{env, sync::Arc, time::{Duration, Instant}};
use tokio::sync::{Mutex, Semaphore};
use tonic::{transport::{Channel, Endpoint}, Request};

pub mod proto { tonic::include_proto!("aether.v1"); }
use proto::{execution_service_client::ExecutionServiceClient, ExecuteRequest};

const MAX_PAYLOAD_BYTES: usize = 65_536;

#[derive(Clone)]
struct AppState {
    engine: Channel,
    permits: Arc<Semaphore>,
    max_concurrent_tasks: usize,
    stability: Arc<Mutex<Stability>>,
}

#[derive(Debug)]
struct Stability {
    successes: u64,
    failures: u64,
    delta: f64,
}

impl Default for Stability {
    fn default() -> Self {
        Self { successes: 0, failures: 0, delta: 1.0 }
    }
}

impl Stability {
    fn observe(&mut self, success: bool) {
        if success { self.successes += 1; } else { self.failures += 1; }
        // An exponentially weighted runtime-health indicator, NOT a consistency proof.
        let signal = if success { 1.0 } else { 0.0 };
        self.delta = (0.9 * self.delta + 0.1 * signal).clamp(0.0, 1.0);
    }
}

#[derive(Serialize)]
struct ExecuteOutput {
    task_id: String,
    operation: policy::Operation,
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
}

#[derive(Serialize)]
struct ErrorBody { error: String }

type ApiError = (StatusCode, Json<ErrorBody>);
fn api_error(status: StatusCode, message: impl Into<String>) -> ApiError {
    (status, Json(ErrorBody { error: message.into() }))
}

async fn execute(State(state): State<Arc<AppState>>, Json(input): Json<TaskInput>)
    -> Result<Json<ExecuteOutput>, ApiError>
{
    let permit = authorize(&input).map_err(|e| api_error(StatusCode::BAD_REQUEST, e))?;
    let _slot = state.permits.clone().try_acquire_owned()
        .map_err(|_| api_error(StatusCode::TOO_MANY_REQUESTS, "execution capacity exceeded"))?;
    let operation = permit.operation;
    let expected = operation.expected(&input.payload).ok_or_else(||
        api_error(StatusCode::BAD_REQUEST, "unsupported payload semantics"))?;
    let start = Instant::now();
    let mut request = Request::new(ExecuteRequest {
        task_id: permit.task_id.clone(),
        operation: operation.as_str().into(),
        payload: input.payload,
    });
    request.set_timeout(Duration::from_millis(permit.deadline_ms));
    let mut client = ExecutionServiceClient::new(state.engine.clone());
    let response = tokio::time::timeout(Duration::from_millis(permit.deadline_ms),
        client.execute(request)).await;
    let outcome = match response {
        Ok(Ok(r)) => Ok(r.into_inner()),
        Ok(Err(e)) => Err(api_error(StatusCode::BAD_GATEWAY, format!("execution service: {}", e.code()))),
        Err(_) => Err(api_error(StatusCode::GATEWAY_TIMEOUT, "execution timeout")),
    };
    let mut stability = state.stability.lock().await;
    let verified = outcome.as_ref().map(|r|
        r.success && r.task_id == permit.task_id
        && r.output.len() <= permit.max_output_bytes
        && r.output == expected).unwrap_or(false);
    stability.observe(verified);
    let delta = stability.delta;
    drop(stability);
    let response = outcome?;
    if !verified {
        return Err(api_error(StatusCode::BAD_GATEWAY, "execution result failed independent verification"));
    }
    let elapsed = start.elapsed().as_millis().min(u64::MAX as u128) as u64;
    let receipt = make_receipt(&permit, &response.output, elapsed, verified);
    Ok(Json(ExecuteOutput {
        task_id: permit.task_id.clone(),
        operation,
        output: response.output,
        success: response.success,
        error: response.error,
        output_sha256: receipt.output_sha256.clone(),
        duration_ms: elapsed,
        delta,
        verified,
        receipt,
    }))
}

async fn console() -> Html<&'static str> { Html(include_str!("../static/index.html")) }

async fn health(State(state): State<Arc<AppState>>) -> StatusCode {
    let mut client = ExecutionServiceClient::new(state.engine.clone());
    match tokio::time::timeout(Duration::from_secs(2),
        client.health(Request::new(proto::HealthRequest {}))).await {
        Ok(Ok(r)) if r.get_ref().ready => StatusCode::OK,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    }
}

async fn status(State(state): State<Arc<AppState>>) -> Json<SystemStatus> {
    let stability = state.stability.lock().await;
    let available = state.permits.available_permits();
    Json(SystemStatus {
        ready: available <= state.max_concurrent_tasks,
        delta: stability.delta,
        successes: stability.successes,
        failures: stability.failures,
        active_tasks: state.max_concurrent_tasks - available,
        capacity: state.max_concurrent_tasks,
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    let engine_url = env::var("ENGINE_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:50051".into());
    let capacity = env::var("MAX_CONCURRENT_TASKS")
        .ok().and_then(|s| s.parse::<usize>().ok())
        .filter(|v| (1..=4096).contains(v)).unwrap_or(32);
    let channel = Endpoint::from_shared(engine_url)?.connect_lazy();
    let state = Arc::new(AppState {
        engine: channel,
        permits: Arc::new(Semaphore::new(capacity)),
        max_concurrent_tasks: capacity,
        stability: Arc::new(Mutex::new(Stability::default())),
    });
    let app = Router::new()
        .route("/", get(console))
        .route("/health", get(health))
        .route("/system/status", get(status))
        .route("/execute", post(execute))
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
    fn response_contract_includes_receipt() {
        let task: TaskInput = serde_json::from_str(r#"{"operation":"echo","payload":"abc"}"#).unwrap();
        let permit = authorize(&task).unwrap();
        let receipt = make_receipt(&permit,"abc",3,true);
        let json = serde_json::to_value(receipt).unwrap();
        assert_eq!(json["policy_revision"], 1);
        assert_eq!(json["route"], "local");
    }
    #[test]
    fn unknown_operation_is_rejected_during_deserialization() {
        assert!(serde_json::from_str::<TaskInput>(r#"{"operation":"shell","payload":"id"}"#).is_err());
    }
}
