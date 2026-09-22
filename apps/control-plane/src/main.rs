use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{env, sync::Arc, time::{Duration, Instant}};
use tokio::sync::{Mutex, Semaphore};
use tonic::{transport::{Channel, Endpoint}, Request};
use uuid::Uuid;

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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecuteInput {
    operation: String,
    payload: String,
    #[serde(default)]
    task_id: Option<Uuid>,
}

#[derive(Serialize)]
struct ExecuteOutput {
    task_id: String,
    operation: String,
    output: String,
    success: bool,
    error: String,
    output_sha256: String,
    duration_ms: u64,
    delta: f64,
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

fn validate(input: &ExecuteInput) -> Result<(), &'static str> {
    if input.payload.len() > MAX_PAYLOAD_BYTES { return Err("payload exceeds 64 KiB"); }
    if !matches!(input.operation.as_str(), "echo" | "uppercase" | "sha256") {
        return Err("unsupported operation");
    }
    Ok(())
}

async fn execute(State(state): State<Arc<AppState>>, Json(input): Json<ExecuteInput>)
    -> Result<Json<ExecuteOutput>, ApiError>
{
    validate(&input).map_err(|e| api_error(StatusCode::BAD_REQUEST, e))?;
    let _permit = state.permits.clone().try_acquire_owned()
        .map_err(|_| api_error(StatusCode::TOO_MANY_REQUESTS, "execution capacity exceeded"))?;
    let task_id = input.task_id.unwrap_or_else(Uuid::new_v4).to_string();
    let operation = input.operation.clone();
    let start = Instant::now();
    let request = Request::new(ExecuteRequest {
        task_id: task_id.clone(),
        operation: input.operation,
        payload: input.payload,
    });
    let mut client = ExecutionServiceClient::new(state.engine.clone());
    let response = tokio::time::timeout(Duration::from_secs(10), client.execute(request)).await;
    let outcome = match response {
        Ok(Ok(r)) => Ok(r.into_inner()),
        Ok(Err(e)) => Err(api_error(StatusCode::BAD_GATEWAY, format!("execution service: {}", e.code()))),
        Err(_) => Err(api_error(StatusCode::GATEWAY_TIMEOUT, "execution timeout")),
    };
    let mut stability = state.stability.lock().await;
    stability.observe(outcome.as_ref().map(|r| r.success).unwrap_or(false));
    let delta = stability.delta;
    drop(stability);
    let response = outcome?;
    let digest = hex::encode(Sha256::digest(response.output.as_bytes()));
    Ok(Json(ExecuteOutput {
        task_id,
        operation,
        output: response.output,
        success: response.success,
        error: response.error,
        output_sha256: digest,
        duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
        delta,
    }))
}

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
    fn request(operation: &str, payload: &str) -> ExecuteInput {
        ExecuteInput { operation: operation.into(), payload: payload.into(), task_id: None }
    }
    #[test] fn only_known_operations_are_allowed() {
        assert!(validate(&request("echo", "hello")).is_ok());
        assert!(validate(&request("uppercase", "hello")).is_ok());
        assert!(validate(&request("sha256", "hello")).is_ok());
        assert!(validate(&request("shell", "hello")).is_err());
    }
    #[test] fn validates_payload_length() {
        assert!(validate(&request("echo", &"x".repeat(MAX_PAYLOAD_BYTES))).is_ok());
        assert!(validate(&request("echo", &"x".repeat(MAX_PAYLOAD_BYTES + 1))).is_err());
    }
    #[test] fn stability_is_bounded_and_sensitive_to_failure() {
        let mut health = Stability::default();
        health.observe(false);
        assert!(health.delta < 1.0 && health.delta >= 0.0);
        health.observe(true);
        assert_eq!((health.successes, health.failures), (1, 1));
        assert!(health.delta <= 1.0);
    }
}
