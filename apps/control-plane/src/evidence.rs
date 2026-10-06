use serde::{Deserialize, Serialize};

use crate::{
    identity::{Identity, RECEIPT_SIGNATURE_DOMAIN, SIGNING_ALGORITHM},
    kernel::{sha256_hex, ExecutionPermit, Operation},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReceiptAttestation {
    pub algorithm: String,
    pub domain: String,
    pub public_key_hex: String,
    pub key_fingerprint_sha256: String,
    pub signature_hex: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReceiptVerification {
    pub digest_matches: bool,
    pub signature_present: bool,
    pub algorithm_supported: bool,
    pub domain_matches: bool,
    pub fingerprint_matches: bool,
    pub signature_valid: bool,
    pub valid: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionReceipt {
    pub receipt_version: u32,
    pub receipt_sha256: String,
    pub task_id: String,
    pub permit_id: String,
    pub parent_task_id: Option<String>,
    pub operation: Operation,
    pub input_sha256: String,
    pub output_sha256: String,
    pub policy_sha256: String,
    pub policy_revision: u32,
    pub capabilities: u64,
    pub capability_names: Vec<String>,
    pub executor_id: String,
    pub executor_identity_sha256: String,
    pub deadline_ms: u64,
    pub elapsed_ms: u64,
    pub verified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attestation: Option<ReceiptAttestation>,
}

#[derive(Serialize)]
struct ReceiptDigest<'a> {
    receipt_version: u32,
    task_id: &'a str,
    permit_id: &'a str,
    parent_task_id: &'a Option<String>,
    operation: Operation,
    input_sha256: &'a str,
    output_sha256: &'a str,
    policy_sha256: &'a str,
    policy_revision: u32,
    capabilities: u64,
    capability_names: &'a [String],
    executor_id: &'a str,
    executor_identity_sha256: &'a str,
    deadline_ms: u64,
    elapsed_ms: u64,
    verified: bool,
}

impl ExecutionReceipt {
    pub fn new(
        permit: &ExecutionPermit,
        parent_task_id: Option<String>,
        output: &str,
        elapsed_ms: u64,
        verified: bool,
    ) -> Self {
        let mut receipt = Self {
            receipt_version: 2,
            receipt_sha256: String::new(),
            task_id: permit.task_id.clone(),
            permit_id: permit.permit_id.clone(),
            parent_task_id,
            operation: permit.operation,
            input_sha256: permit.input_sha256.clone(),
            output_sha256: sha256_hex(output.as_bytes()),
            policy_sha256: permit.policy_sha256.clone(),
            policy_revision: permit.policy_revision,
            capabilities: permit.capabilities,
            capability_names: permit.capability_names.clone(),
            executor_id: permit.executor_id.clone(),
            executor_identity_sha256: permit.executor_identity_sha256.clone(),
            deadline_ms: permit.deadline_ms,
            elapsed_ms,
            verified,
            attestation: None,
        };
        receipt.receipt_sha256 = receipt.digest();
        receipt
    }

    pub fn digest(&self) -> String {
        let body = ReceiptDigest {
            receipt_version: self.receipt_version,
            task_id: &self.task_id,
            permit_id: &self.permit_id,
            parent_task_id: &self.parent_task_id,
            operation: self.operation,
            input_sha256: &self.input_sha256,
            output_sha256: &self.output_sha256,
            policy_sha256: &self.policy_sha256,
            policy_revision: self.policy_revision,
            capabilities: self.capabilities,
            capability_names: &self.capability_names,
            executor_id: &self.executor_id,
            executor_identity_sha256: &self.executor_identity_sha256,
            deadline_ms: self.deadline_ms,
            elapsed_ms: self.elapsed_ms,
            verified: self.verified,
        };
        let bytes = serde_json::to_vec(&body).expect("receipt digest serialization is infallible");
        sha256_hex(&bytes)
    }

    pub fn attest(&mut self, identity: &Identity) {
        self.receipt_sha256 = self.digest();
        let info = identity.info();
        self.attestation = Some(ReceiptAttestation {
            algorithm: SIGNING_ALGORITHM.to_string(),
            domain: RECEIPT_SIGNATURE_DOMAIN.to_string(),
            public_key_hex: info.public_key_hex,
            key_fingerprint_sha256: info.fingerprint_sha256,
            signature_hex: identity
                .sign_domain(RECEIPT_SIGNATURE_DOMAIN, self.receipt_sha256.as_bytes()),
        });
    }

    pub fn verify_attestation(&self) -> ReceiptVerification {
        let digest_matches = self.receipt_sha256 == self.digest();
        let Some(attestation) = &self.attestation else {
            return ReceiptVerification {
                digest_matches,
                signature_present: false,
                algorithm_supported: false,
                domain_matches: false,
                fingerprint_matches: false,
                signature_valid: false,
                valid: false,
            };
        };

        let algorithm_supported = attestation.algorithm == SIGNING_ALGORITHM;
        let domain_matches = attestation.domain == RECEIPT_SIGNATURE_DOMAIN;
        let fingerprint_matches = Identity::fingerprint_public_key_hex(&attestation.public_key_hex)
            .is_some_and(|value| value == attestation.key_fingerprint_sha256);
        let signature_valid = algorithm_supported
            && domain_matches
            && Identity::verify_domain(
                &attestation.public_key_hex,
                &attestation.signature_hex,
                RECEIPT_SIGNATURE_DOMAIN,
                self.receipt_sha256.as_bytes(),
            );
        let valid = digest_matches
            && algorithm_supported
            && domain_matches
            && fingerprint_matches
            && signature_valid;

        ReceiptVerification {
            digest_matches,
            signature_present: true,
            algorithm_supported,
            domain_matches,
            fingerprint_matches,
            signature_valid,
            valid,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DivergencePoint {
    Input,
    Policy,
    ExecutorIdentity,
    Output,
    Verification,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReplayComparison {
    pub original_task_id: String,
    pub replay_task_id: String,
    pub matched: bool,
    pub input_match: bool,
    pub policy_match: bool,
    pub executor_match: bool,
    pub output_match: bool,
    pub verification_match: bool,
    pub first_divergence: Option<DivergencePoint>,
    pub receipt: ExecutionReceipt,
}

pub fn compare_receipts(
    original: &ExecutionReceipt,
    replay: &ExecutionReceipt,
) -> ReplayComparison {
    let input_match = original.input_sha256 == replay.input_sha256;
    let policy_match = original.policy_sha256 == replay.policy_sha256;
    let executor_match = original.executor_identity_sha256 == replay.executor_identity_sha256;
    let output_match = original.output_sha256 == replay.output_sha256;
    let verification_match = original.verified == replay.verified;

    let first_divergence = if !input_match {
        Some(DivergencePoint::Input)
    } else if !policy_match {
        Some(DivergencePoint::Policy)
    } else if !executor_match {
        Some(DivergencePoint::ExecutorIdentity)
    } else if !output_match {
        Some(DivergencePoint::Output)
    } else if !verification_match {
        Some(DivergencePoint::Verification)
    } else {
        None
    };

    ReplayComparison {
        original_task_id: original.task_id.clone(),
        replay_task_id: replay.task_id.clone(),
        matched: first_divergence.is_none(),
        input_match,
        policy_match,
        executor_match,
        output_match,
        verification_match,
        first_divergence,
        receipt: replay.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::{authorize_for_executor, ExecuteInput};
    use std::fs;
    use uuid::Uuid;

    fn permit(payload: &str) -> ExecutionPermit {
        authorize_for_executor(
            &ExecuteInput {
                operation: "echo".into(),
                payload: payload.into(),
                task_id: None,
                deadline_ms: 5_000,
            },
            "cpp-grpc-v1",
        )
        .unwrap()
    }

    fn identity() -> (Identity, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!("aether-receipt-id-{}.key", Uuid::new_v4()));
        (Identity::load_or_create(&path).unwrap(), path)
    }

    #[test]
    fn receipt_digest_changes_when_output_changes() {
        let p = permit("abc");
        let a = ExecutionReceipt::new(&p, None, "abc", 2, true);
        let b = ExecutionReceipt::new(&p, None, "tampered", 2, false);
        assert_ne!(a.receipt_sha256, b.receipt_sha256);
        assert_ne!(a.output_sha256, b.output_sha256);
    }

    #[test]
    fn signed_receipt_verifies_and_tampering_is_rejected() {
        let (identity, path) = identity();
        let p = permit("abc");
        let mut receipt = ExecutionReceipt::new(&p, None, "abc", 2, true);
        receipt.attest(&identity);
        assert!(receipt.verify_attestation().valid);

        let mut body_tampered = receipt.clone();
        body_tampered.output_sha256 = sha256_hex(b"evil");
        let body_result = body_tampered.verify_attestation();
        assert!(!body_result.digest_matches);
        assert!(!body_result.valid);

        let mut signature_tampered = receipt.clone();
        signature_tampered
            .attestation
            .as_mut()
            .unwrap()
            .signature_hex
            .replace_range(0..2, "00");
        assert!(!signature_tampered.verify_attestation().valid);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn unsigned_legacy_receipt_is_loaded_but_not_attested() {
        let p = permit("abc");
        let receipt = ExecutionReceipt::new(&p, None, "abc", 2, true);
        let mut value = serde_json::to_value(receipt).unwrap();
        value.as_object_mut().unwrap().remove("attestation");
        let loaded: ExecutionReceipt = serde_json::from_value(value).unwrap();
        assert!(!loaded.verify_attestation().valid);
    }

    #[test]
    fn divergence_identifies_output_boundary() {
        let p = permit("abc");
        let original = ExecutionReceipt::new(&p, None, "abc", 2, true);
        let mut replay = original.clone();
        replay.task_id = "replay".into();
        replay.output_sha256 = sha256_hex(b"different");
        let comparison = compare_receipts(&original, &replay);
        assert!(!comparison.matched);
        assert!(matches!(
            comparison.first_divergence,
            Some(DivergencePoint::Output)
        ));
    }
}
