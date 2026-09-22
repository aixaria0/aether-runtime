use axum::{extract::State, http::StatusCode, routing::{get, post}, Json, Router};
use serde::{Deserialize, Serialize};
use std::{env, sync::Arc, time::Duration};
use tonic::{transport::{Channel, Endpoint}, Request};
use uuid::Uuid;

pub mod proto { tonic::include_proto!("aether.v1"); }
use proto::{execution_service_client::ExecutionServiceClient, ExecuteRequest};

#[derive(Clone)]
struct AppState { engine: Channel }
#[derive(Deserialize)]
struct ExecuteInput { operation: String, payload: String }
#[derive(Serialize)]
struct ExecuteOutput { task_id: String, output: String, success: bool, error: String, delta: f64 }
#[derive(Serialize)]
struct ErrorBody { error: String }

fn validate(input: &ExecuteInput) -> Result<(), &'static str> {
    if input.payload.len() > 65536 { return Err("payload exceeds 64 KiB"); }
    if !matches!(input.operation.as_str(), "echo" | "uppercase" | "sha256") { return Err("unsupported operation"); }
    Ok(())
}

async fn execute(State(state): State<Arc<AppState>>, Json(input): Json<ExecuteInput>)
  -> Result<Json<ExecuteOutput>, (StatusCode, Json<ErrorBody>)> {
    validate(&input).map_err(|e| (StatusCode::BAD_REQUEST, Json(ErrorBody{error:e.into()})))?;
    let mut client = ExecutionServiceClient::new(state.engine.clone());
    let task_id = Uuid::new_v4().to_string();
    let request = Request::new(ExecuteRequest { task_id: task_id.clone(), operation: input.operation, payload: input.payload });
    let response = tokio::time::timeout(Duration::from_secs(10), client.execute(request)).await
      .map_err(|_| (StatusCode::GATEWAY_TIMEOUT, Json(ErrorBody{error:"execution timeout".into()})))?
      .map_err(|e| (StatusCode::BAD_GATEWAY, Json(ErrorBody{error:e.to_string()})))?.into_inner();
    Ok(Json(ExecuteOutput { task_id, output: response.output, success: response.success,
      error: response.error, delta: if response.success {1.0} else {0.0} }))
}

async fn health(State(state): State<Arc<AppState>>) -> StatusCode {
    let mut client = ExecutionServiceClient::new(state.engine.clone());
    match tokio::time::timeout(Duration::from_secs(2), client.health(Request::new(proto::HealthRequest{}))).await {
        Ok(Ok(r)) if r.into_inner().ready => StatusCode::OK,
        _ => StatusCode::SERVICE_UNAVAILABLE
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    let engine_url = env::var("ENGINE_URL").unwrap_or_else(|_| "http://127.0.0.1:50051".into());
    let channel = Endpoint::from_shared(engine_url)?.connect_lazy();
    let app = Router::new().route("/health", get(health)).route("/execute", post(execute))
        .with_state(Arc::new(AppState { engine: channel }))
        .layer(tower_http::trace::TraceLayer::new_for_http());
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn rejects_unrecognized_action() {
        assert!(validate(&ExecuteInput {operation:"shell".into(),payload:"hello".into()}).is_err());
    }
    #[test] fn accepts_safe_operation() {
        assert!(validate(&ExecuteInput {operation:"echo".into(),payload:"hello".into()}).is_ok());
    }
}
