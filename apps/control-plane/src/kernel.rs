use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const MAX_INPUT_BYTES: usize = 65_536;
pub const MAX_OUTPUT_BYTES: usize = 65_536;
pub const MAX_DEADLINE_MS: u64 = 10_000;
pub const POLICY_REVISION: u32 = 1;
pub const EXECUTOR_ID: &str = "cpp-grpc-v1";

pub const CAP_COMPUTE: u64 = 1 << 0;
pub const CAP_FILE_READ: u64 = 1 << 1;
pub const CAP_FILE_WRITE: u64 = 1 << 2;
pub const CAP_NETWORK: u64 = 1 << 3;
pub const CAP_SPAWN_PROCESS: u64 = 1 << 4;
pub const CAP_GPU: u64 = 1 << 5;
pub const CAP_REMOTE_EXECUTE: u64 = 1 << 6;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Operation {
    Echo,
    Uppercase,
    Sha256,
}

impl Operation {
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        match value {
            "echo" => Ok(Self::Echo),
            "uppercase" => Ok(Self::Uppercase),
            "sha256" => Ok(Self::Sha256),
            _ => Err("unsupported operation"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Echo => "echo",
            Self::Uppercase => "uppercase",
            Self::Sha256 => "sha256",
        }
    }

    pub fn expected(self, payload: &str) -> Result<String, &'static str> {
        match self {
            Self::Echo => Ok(payload.to_owned()),
            Self::Uppercase if payload.is_ascii() => Ok(payload.to_ascii_uppercase()),
            Self::Uppercase => Err("uppercase currently accepts ASCII input only"),
            Self::Sha256 => Ok(sha256_hex(payload.as_bytes())),
        }
    }

    pub fn capabilities(self) -> u64 {
        match self {
            Self::Echo | Self::Uppercase | Self::Sha256 => CAP_COMPUTE,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecuteInput {
    pub operation: String,
    pub payload: String,
    #[serde(default)]
    pub task_id: Option<Uuid>,
    #[serde(default = "default_deadline_ms")]
    pub deadline_ms: u64,
}

fn default_deadline_ms() -> u64 {
    MAX_DEADLINE_MS
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionPermit {
    pub permit_id: String,
    pub task_id: String,
    pub operation: Operation,
    pub capabilities: u64,
    pub capability_names: Vec<String>,
    pub input_sha256: String,
    pub policy_sha256: String,
    pub policy_revision: u32,
    pub deadline_ms: u64,
    pub max_output_bytes: usize,
    pub executor_id: String,
    pub executor_identity_sha256: String,
}

pub fn authorize(input: &ExecuteInput) -> Result<ExecutionPermit, &'static str> {
    if input.payload.len() > MAX_INPUT_BYTES {
        return Err("payload exceeds 64 KiB");
    }
    if input.deadline_ms == 0 || input.deadline_ms > MAX_DEADLINE_MS {
        return Err("deadline_ms must be between 1 and 10000");
    }

    let operation = Operation::parse(&input.operation)?;
    operation.expected(&input.payload)?;
    let capabilities = operation.capabilities();
    let task_id = input.task_id.unwrap_or_else(Uuid::new_v4).to_string();
    let input_sha256 = sha256_hex(input.payload.as_bytes());
    let executor_identity_sha256 = sha256_hex(EXECUTOR_ID.as_bytes());

    let policy_descriptor = format!(
        "aether-policy-v{}|operation={}|capabilities={}|max-input={}|max-output={}|deadline-ms={}|executor={}",
        POLICY_REVISION,
        operation.as_str(),
        capabilities,
        MAX_INPUT_BYTES,
        MAX_OUTPUT_BYTES,
        input.deadline_ms,
        EXECUTOR_ID,
    );

    Ok(ExecutionPermit {
        permit_id: Uuid::new_v4().to_string(),
        task_id,
        operation,
        capabilities,
        capability_names: capability_names(capabilities),
        input_sha256,
        policy_sha256: sha256_hex(policy_descriptor.as_bytes()),
        policy_revision: POLICY_REVISION,
        deadline_ms: input.deadline_ms,
        max_output_bytes: MAX_OUTPUT_BYTES,
        executor_id: EXECUTOR_ID.to_string(),
        executor_identity_sha256,
    })
}

pub fn capability_names(mask: u64) -> Vec<String> {
    [
        (CAP_COMPUTE, "compute"),
        (CAP_FILE_READ, "file_read"),
        (CAP_FILE_WRITE, "file_write"),
        (CAP_NETWORK, "network"),
        (CAP_SPAWN_PROCESS, "spawn_process"),
        (CAP_GPU, "gpu"),
        (CAP_REMOTE_EXECUTE, "remote_execute"),
    ]
    .into_iter()
    .filter_map(|(bit, name)| (mask & bit != 0).then(|| name.to_string()))
    .collect()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(operation: &str, payload: &str, deadline_ms: u64) -> ExecuteInput {
        ExecuteInput {
            operation: operation.into(),
            payload: payload.into(),
            task_id: None,
            deadline_ms,
        }
    }

    #[test]
    fn admission_is_bounded_and_typed() {
        assert!(authorize(&input("echo", "ok", 1_000)).is_ok());
        assert!(authorize(&input("shell", "id", 1_000)).is_err());
        assert!(authorize(&input("echo", "ok", 0)).is_err());
        assert!(authorize(&input("echo", "ok", 10_001)).is_err());
        assert!(authorize(&input("uppercase", "é", 1_000)).is_err());
    }

    #[test]
    fn permit_fingerprints_are_deterministic_for_policy_and_input() {
        let a = authorize(&input("sha256", "abc", 5_000)).unwrap();
        let b = authorize(&input("sha256", "abc", 5_000)).unwrap();
        assert_eq!(a.input_sha256, b.input_sha256);
        assert_eq!(a.policy_sha256, b.policy_sha256);
        assert_eq!(a.executor_identity_sha256, b.executor_identity_sha256);
        assert_eq!(a.capability_names, vec!["compute"]);
    }
}
