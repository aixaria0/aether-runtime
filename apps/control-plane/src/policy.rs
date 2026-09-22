use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const POLICY_REVISION: u32 = 1;
pub const MAX_PAYLOAD_BYTES: usize = 65_536;
pub const MAX_OUTPUT_BYTES: usize = 65_536;
pub const MAX_DEADLINE_MS: u64 = 10_000;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Operation { Echo, Uppercase, Sha256 }
impl Operation {
    pub fn as_str(self) -> &'static str {
        match self { Self::Echo => "echo", Self::Uppercase => "uppercase", Self::Sha256 => "sha256" }
    }
    pub fn expected(self, payload: &str) -> Option<String> {
        match self {
            Self::Echo => Some(payload.to_owned()),
            Self::Sha256 => Some(hex::encode(Sha256::digest(payload.as_bytes()))),
            Self::Uppercase if payload.is_ascii() => Some(payload.to_ascii_uppercase()),
            Self::Uppercase => None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskInput {
    pub operation: Operation,
    pub payload: String,
    #[serde(default)]
    pub task_id: Option<Uuid>,
    #[serde(default = "default_deadline")]
    pub deadline_ms: u64,
}
fn default_deadline() -> u64 { MAX_DEADLINE_MS }

#[derive(Clone, Debug, Serialize)]
pub struct ExecutionPermit {
    pub task_id: String,
    pub operation: Operation,
    pub input_sha256: String,
    pub policy_sha256: String,
    pub policy_revision: u32,
    pub deadline_ms: u64,
    pub max_output_bytes: usize,
    pub route: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct ExecutionReceipt {
    pub version: u32,
    pub task_id: String,
    pub operation: Operation,
    pub input_sha256: String,
    pub output_sha256: String,
    pub policy_sha256: String,
    pub policy_revision: u32,
    pub elapsed_ms: u64,
    pub verified: bool,
    pub route: &'static str,
}

pub fn authorize(task: &TaskInput) -> Result<ExecutionPermit, &'static str> {
    if task.payload.len() > MAX_PAYLOAD_BYTES { return Err("payload exceeds 64 KiB"); }
    if task.deadline_ms == 0 || task.deadline_ms > MAX_DEADLINE_MS {
        return Err("deadline_ms must be between 1 and 10000");
    }
    if task.operation == Operation::Uppercase && !task.payload.is_ascii() {
        return Err("uppercase currently accepts ASCII input only");
    }
    let task_id = task.task_id.unwrap_or_else(Uuid::new_v4).to_string();
    let input_sha256 = hex::encode(Sha256::digest(task.payload.as_bytes()));
    // Versioned, deterministic policy descriptor; this hash is not an authentication token.
    let policy = format!(
        "aether-policy-v{}|{}|max-input={}|max-output={}|deadline-ms={}|route=local",
        POLICY_REVISION, task.operation.as_str(), MAX_PAYLOAD_BYTES, MAX_OUTPUT_BYTES,
        task.deadline_ms,
    );
    Ok(ExecutionPermit {
        task_id,
        operation: task.operation,
        input_sha256,
        policy_sha256: hex::encode(Sha256::digest(policy.as_bytes())),
        policy_revision: POLICY_REVISION,
        deadline_ms: task.deadline_ms,
        max_output_bytes: MAX_OUTPUT_BYTES,
        route: "local",
    })
}

pub fn make_receipt(permit: &ExecutionPermit, output: &str, elapsed_ms: u64, verified: bool) -> ExecutionReceipt {
    ExecutionReceipt {
        version: 1,
        task_id: permit.task_id.clone(),
        operation: permit.operation,
        input_sha256: permit.input_sha256.clone(),
        output_sha256: hex::encode(Sha256::digest(output.as_bytes())),
        policy_sha256: permit.policy_sha256.clone(),
        policy_revision: permit.policy_revision,
        elapsed_ms,
        verified,
        route: permit.route,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn task(operation: Operation, payload: &str) -> TaskInput {
        TaskInput {operation, payload:payload.into(), task_id:None, deadline_ms:1000}
    }
    #[test] fn denies_missing_deadline_and_nonascii_uppercase() {
        let mut t=task(Operation::Echo,"ok");t.deadline_ms=0;
        assert!(authorize(&t).is_err());
        t.deadline_ms=10_001;assert!(authorize(&t).is_err());
        assert!(authorize(&task(Operation::Uppercase,"é")).is_err());
    }
    #[test] fn deterministic_policy_and_input_hash() {
        let t=task(Operation::Sha256,"abc");
        let a=authorize(&t).unwrap();let b=authorize(&t).unwrap();
        assert_eq!(a.policy_sha256,b.policy_sha256);
        assert_eq!(a.input_sha256,b.input_sha256);
        assert_eq!(a.operation.expected("abc").unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
    #[test] fn verifies_empty_output_and_detects_tampering() {
        let p=authorize(&task(Operation::Echo,"")).unwrap();
        let good=make_receipt(&p,"",3,true);
        let bad=make_receipt(&p,"tampered",3,false);
        assert!(good.verified && !bad.verified);
        assert_ne!(good.output_sha256,bad.output_sha256);
    }
}
