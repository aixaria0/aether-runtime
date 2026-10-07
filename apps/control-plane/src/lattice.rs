//! A read-only semantic projection of existing execution evidence.
//! Acceptance is scoped to the current allowlisted operation and signer policy.
use serde::{Deserialize, Serialize};

use crate::{
    evidence::{ExecutionReceipt, ReceiptVerification},
    journal::{ExecutionSnapshot, JournalEvent, JournalVerification, StoredExecution},
    kernel::{authorize_for_executor, sha256_hex, ExecuteInput, Operation, MAX_OUTPUT_BYTES},
    scheduler::{CPP_EXECUTOR, RUST_EXECUTOR},
};

pub const ONTOLOGY: &str = include_str!("../../../schemas/lattice-ontology.v1.json");

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub id: String,
    pub kind: String,
    pub sha256: String,
    pub parent: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transformation {
    pub id: String,
    pub operation: Operation,
    pub input: String,
    pub output: String,
    pub evidence_ref: String,
    // A replay consumes the original input, not the original output.
    pub replay_of: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvariantEvaluation {
    pub name: String,
    pub passed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Acceptance {
    Accepted,
    Rejected,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assessment {
    pub status: Acceptance,
    pub scope: String,
    pub trusted_signer_fingerprint_sha256: String,
    pub failed_invariants: Vec<String>,
    pub not_evaluated: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub source: String,
    pub receipt: ExecutionReceipt,
    pub receipt_verification: ReceiptVerification,
    pub journal: JournalVerification,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineageCertificate {
    pub schema: String,
    pub artifacts: Vec<Artifact>,
    pub transformation: Transformation,
    pub evidence: Evidence,
    pub invariants: Vec<InvariantEvaluation>,
    pub assessment: Assessment,
}

fn record_binding(record: &StoredExecution) -> bool {
    let receipt = &record.receipt;
    uuid::Uuid::parse_str(&record.task_id).is_ok()
        && uuid::Uuid::parse_str(&receipt.permit_id).is_ok()
        && record.task_id == receipt.task_id
        && record.parent_task_id == receipt.parent_task_id
        && record.operation == receipt.operation.as_str()
        && sha256_hex(record.payload.as_bytes()) == receipt.input_sha256
        && sha256_hex(record.output.as_bytes()) == receipt.output_sha256
}

fn result_matches(record: &StoredExecution) -> bool {
    record
        .receipt
        .operation
        .expected(&record.payload)
        .is_ok_and(|expected| expected == record.output)
}

fn signer_matches(receipt: &ExecutionReceipt, trusted_fingerprint: &str) -> bool {
    receipt
        .attestation
        .as_ref()
        .is_some_and(|attestation| attestation.key_fingerprint_sha256 == trusted_fingerprint)
}

fn policy_matches(record: &StoredExecution) -> bool {
    let receipt = &record.receipt;
    if !matches!(receipt.executor_id.as_str(), CPP_EXECUTOR | RUST_EXECUTOR)
        || record.output.len() > MAX_OUTPUT_BYTES
    {
        return false;
    }
    let Ok(task_id) = uuid::Uuid::parse_str(&record.task_id) else {
        return false;
    };
    authorize_for_executor(
        &ExecuteInput {
            operation: record.operation.clone(),
            payload: record.payload.clone(),
            task_id: Some(task_id),
            deadline_ms: receipt.deadline_ms,
        },
        &receipt.executor_id,
    )
    .is_ok_and(|permit| {
        permit.policy_sha256 == receipt.policy_sha256
            && permit.policy_revision == receipt.policy_revision
            && permit.capabilities == receipt.capabilities
            && permit.capability_names == receipt.capability_names
            && permit.executor_identity_sha256 == receipt.executor_identity_sha256
    })
}

fn event_field(event: &JournalEvent, field: &str, expected: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(&event.payload_json)
        .ok()
        .and_then(|payload| payload.get(field)?.as_str().map(str::to_owned))
        .is_some_and(|value| value == expected)
}

fn journal_binds_receipt(record: &StoredExecution, events: &[JournalEvent]) -> bool {
    let receipt = &record.receipt;
    let Some(attestation) = &receipt.attestation else {
        return false;
    };
    events.iter().any(|signed| {
        signed.task_id == record.task_id
            && signed.event_type == "RECEIPT_SIGNED"
            && event_field(signed, "receipt_sha256", &receipt.receipt_sha256)
            && event_field(signed, "algorithm", &attestation.algorithm)
            && event_field(
                signed,
                "signer_fingerprint",
                &attestation.key_fingerprint_sha256,
            )
            && events.iter().any(|verified| {
                verified.task_id == record.task_id
                    && verified.event_type == "VERIFICATION_PASSED"
                    && verified.sequence > signed.sequence
                    && event_field(verified, "receipt_sha256", &receipt.receipt_sha256)
                    && event_field(verified, "executor_id", &receipt.executor_id)
            })
    })
}

fn replay_matches(snapshot: &ExecutionSnapshot, trusted_fingerprint: &str) -> bool {
    let child = &snapshot.execution;
    match (&child.receipt.parent_task_id, &snapshot.parent) {
        (None, None) => true,
        (Some(parent_id), Some(parent)) => {
            parent_id == &parent.task_id
                && parent.task_id != child.task_id
                && parent.receipt.operation == child.receipt.operation
                && parent.receipt.input_sha256 == child.receipt.input_sha256
                && parent.receipt.receipt_version == 2
                && parent.receipt.verified
                && parent.receipt.verify_attestation().valid
                && signer_matches(&parent.receipt, trusted_fingerprint)
                && record_binding(parent)
                && result_matches(parent)
                && policy_matches(parent)
        }
        _ => false,
    }
}

pub fn assess(snapshot: ExecutionSnapshot, trusted_fingerprint: &str) -> LineageCertificate {
    let record = &snapshot.execution;
    let receipt = &record.receipt;
    let receipt_verification = receipt.verify_attestation();
    let invariants: Vec<InvariantEvaluation> = [
        ("receipt_attestation", receipt_verification.valid),
        (
            "trusted_signer",
            signer_matches(receipt, trusted_fingerprint),
        ),
        ("record_binding", record_binding(record)),
        ("operation_result", result_matches(record)),
        ("current_policy", policy_matches(record)),
        (
            "verified_receipt_v2",
            receipt.receipt_version == 2 && receipt.verified,
        ),
        ("journal_integrity", snapshot.chain.valid),
        (
            "journal_receipt_binding",
            journal_binds_receipt(record, &snapshot.events),
        ),
        (
            "replay_provenance",
            replay_matches(&snapshot, trusted_fingerprint),
        ),
    ]
    .into_iter()
    .map(|(name, passed)| InvariantEvaluation {
        name: name.to_owned(),
        passed,
    })
    .collect();
    let failed_invariants: Vec<String> = invariants
        .iter()
        .filter(|invariant| !invariant.passed)
        .map(|invariant| invariant.name.clone())
        .collect();
    // Occurrence IDs keep echo(input) == input from creating a graph self-loop.
    let input_id = format!("aether:artifact:{}:input", record.task_id);
    let output_id = format!("aether:artifact:{}:output", record.task_id);
    LineageCertificate {
        schema: "lattice-ontology/v1".to_owned(),
        artifacts: vec![
            Artifact {
                id: input_id.clone(),
                kind: "execution_input".to_owned(),
                sha256: sha256_hex(record.payload.as_bytes()),
                parent: None,
            },
            Artifact {
                id: output_id.clone(),
                kind: "execution_output".to_owned(),
                sha256: sha256_hex(record.output.as_bytes()),
                parent: Some(input_id.clone()),
            },
        ],
        transformation: Transformation {
            id: format!("aether:transformation:{}", record.task_id),
            operation: receipt.operation,
            input: input_id,
            output: output_id,
            evidence_ref: receipt.receipt_sha256.clone(),
            replay_of: receipt
                .parent_task_id
                .as_ref()
                .map(|id| format!("aether:transformation:{id}")),
        },
        evidence: Evidence {
            source: format!("/evidence/{}", record.task_id),
            receipt: receipt.clone(),
            receipt_verification,
            journal: snapshot.chain,
        },
        assessment: Assessment {
            status: if failed_invariants.is_empty() {
                Acceptance::Accepted
            } else {
                Acceptance::Rejected
            },
            scope: "allowlisted_execution_lineage".to_owned(),
            trusted_signer_fingerprint_sha256: trusted_fingerprint.to_owned(),
            failed_invariants,
            not_evaluated: [
                "perturbation_stability",
                "statistical_entropy",
                "generalized_model_fidelity",
                "external_journal_anchor",
                "executor_binary_attestation",
            ]
            .map(str::to_owned)
            .to_vec(),
        },
        invariants,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{identity::Identity, journal::Journal};
    use std::fs;

    struct Fixture {
        directory: std::path::PathBuf,
        identity: Identity,
        journal: Journal,
    }

    impl Fixture {
        fn new() -> Self {
            let directory =
                std::env::temp_dir().join(format!("aether-lattice-{}", uuid::Uuid::new_v4()));
            let identity = Identity::load_or_create(directory.join("identity.key")).unwrap();
            let journal = Journal::open(directory.join("journal.sqlite3")).unwrap();
            Self {
                directory,
                identity,
                journal,
            }
        }

        fn record(&self, operation: &str, input: &str, parent: Option<String>) -> StoredExecution {
            let permit = authorize_for_executor(
                &ExecuteInput {
                    operation: operation.to_owned(),
                    payload: input.to_owned(),
                    task_id: None,
                    deadline_ms: 1_000,
                },
                RUST_EXECUTOR,
            )
            .unwrap();
            let output = permit.operation.expected(input).unwrap();
            let mut receipt = ExecutionReceipt::new(&permit, parent.clone(), &output, 1, true);
            receipt.attest(&self.identity);
            let record = StoredExecution {
                task_id: receipt.task_id.clone(),
                parent_task_id: parent,
                operation: operation.to_owned(),
                payload: input.to_owned(),
                output,
                receipt,
                created_at_ns: 1,
            };
            self.journal.save_execution(&record).unwrap();
            let attestation = record.receipt.attestation.as_ref().unwrap();
            for (kind, fields) in [
                (
                    "RECEIPT_SIGNED",
                    vec![
                        ("receipt_sha256", record.receipt.receipt_sha256.clone()),
                        ("algorithm", attestation.algorithm.clone()),
                        (
                            "signer_fingerprint",
                            attestation.key_fingerprint_sha256.clone(),
                        ),
                    ],
                ),
                (
                    "VERIFICATION_PASSED",
                    vec![
                        ("receipt_sha256", record.receipt.receipt_sha256.clone()),
                        ("executor_id", RUST_EXECUTOR.to_owned()),
                    ],
                ),
            ] {
                self.journal
                    .append_event(
                        &record.task_id,
                        kind,
                        fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect(),
                    )
                    .unwrap();
            }
            record
        }

        fn snapshot(&self, record: &StoredExecution) -> ExecutionSnapshot {
            self.journal
                .execution_snapshot(&record.task_id)
                .unwrap()
                .unwrap()
        }

        fn assess(&self, snapshot: ExecutionSnapshot) -> LineageCertificate {
            assess(snapshot, &self.identity.info().fingerprint_sha256)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    fn fails(certificate: &LineageCertificate, invariant: &str) -> bool {
        matches!(certificate.assessment.status, Acceptance::Rejected)
            && certificate
                .assessment
                .failed_invariants
                .iter()
                .any(|name| name == invariant)
    }

    #[test]
    fn signed_execution_has_distinct_artifacts_and_survives_reopen() {
        let f = Fixture::new();
        for (operation, input) in [
            ("echo", ""),
            ("echo", "same"),
            ("uppercase", "abc"),
            ("sha256", "abc"),
        ] {
            let record = f.record(operation, input, None);
            let certificate = f.assess(f.snapshot(&record));
            assert!(matches!(
                certificate.assessment.status,
                Acceptance::Accepted
            ));
            assert_ne!(certificate.artifacts[0].id, certificate.artifacts[1].id);
            assert_eq!(
                certificate.artifacts[1].parent.as_ref(),
                Some(&certificate.artifacts[0].id)
            );
            let reopened = Journal::open(f.directory.join("journal.sqlite3")).unwrap();
            let after = f.assess(
                reopened
                    .execution_snapshot(&record.task_id)
                    .unwrap()
                    .unwrap(),
            );
            assert_eq!(
                serde_json::to_value(&certificate).unwrap(),
                serde_json::to_value(after).unwrap()
            );
        }
    }

    #[test]
    fn altered_bytes_and_altered_receipt_are_rejected() {
        let f = Fixture::new();
        let r = f.record("echo", "abc", None);
        let mut snapshot = f.snapshot(&r);
        snapshot.execution.output = "changed".into();
        assert!(fails(&f.assess(snapshot), "record_binding"));
        let mut snapshot = f.snapshot(&r);
        snapshot.execution.receipt.output_sha256 = sha256_hex(b"changed");
        assert!(fails(&f.assess(snapshot), "receipt_attestation"));
    }

    #[test]
    fn valid_signature_with_wrong_operation_is_rejected() {
        let f = Fixture::new();
        let r = f.record("echo", "abc", None);
        let mut snapshot = f.snapshot(&r);
        snapshot.execution.receipt.operation = Operation::Uppercase;
        snapshot.execution.receipt.attest(&f.identity);
        assert!(snapshot.execution.receipt.verify_attestation().valid);
        let certificate = f.assess(snapshot);
        assert!(fails(&certificate, "record_binding"));
        assert!(fails(&certificate, "operation_result"));
        assert!(fails(&certificate, "journal_receipt_binding"));
    }

    #[test]
    fn self_consistent_foreign_signer_and_unsigned_receipt_are_rejected() {
        let f = Fixture::new();
        let r = f.record("echo", "abc", None);
        let other = Fixture::new();
        let mut snapshot = f.snapshot(&r);
        snapshot.execution.receipt.attest(&other.identity);
        assert!(snapshot.execution.receipt.verify_attestation().valid);
        assert!(fails(&f.assess(snapshot), "trusted_signer"));
        let mut snapshot = f.snapshot(&r);
        snapshot.execution.receipt.attestation = None;
        assert!(fails(&f.assess(snapshot), "receipt_attestation"));
    }

    #[test]
    fn current_policy_and_journal_fail_closed() {
        let f = Fixture::new();
        let r = f.record("echo", "abc", None);
        let mut snapshot = f.snapshot(&r);
        snapshot.execution.receipt.capabilities |= crate::kernel::CAP_NETWORK;
        snapshot.execution.receipt.attest(&f.identity);
        assert!(fails(&f.assess(snapshot), "current_policy"));
        let mut snapshot = f.snapshot(&r);
        snapshot.chain.valid = false;
        assert!(fails(&f.assess(snapshot), "journal_integrity"));
        let mut snapshot = f.snapshot(&r);
        snapshot.events.clear();
        assert!(fails(&f.assess(snapshot), "journal_receipt_binding"));
    }

    #[test]
    fn replay_provenance_requires_original_input_and_authentic_parent() {
        let f = Fixture::new();
        let parent = f.record("uppercase", "abc", None);
        let replay = f.record("uppercase", "abc", Some(parent.task_id.clone()));
        let certificate = f.assess(f.snapshot(&replay));
        assert!(matches!(
            certificate.assessment.status,
            Acceptance::Accepted
        ));
        assert_eq!(
            certificate.transformation.replay_of,
            Some(format!("aether:transformation:{}", parent.task_id))
        );
        assert_eq!(certificate.artifacts[0].parent, None);
        let mut snapshot = f.snapshot(&replay);
        snapshot.parent = None;
        assert!(fails(&f.assess(snapshot), "replay_provenance"));
        let wrong_input = f.record("uppercase", "ABC", Some(parent.task_id.clone()));
        assert!(fails(
            &f.assess(f.snapshot(&wrong_input)),
            "replay_provenance"
        ));
        let mut snapshot = f.snapshot(&replay);
        snapshot.parent.as_mut().unwrap().output = "changed".into();
        assert!(fails(&f.assess(snapshot), "replay_provenance"));
    }

    #[test]
    fn ontology_has_exact_counterparts_for_supported_rituals() {
        let ontology: serde_json::Value = serde_json::from_str(ONTOLOGY).unwrap();
        for operation in [Operation::Echo, Operation::Uppercase, Operation::Sha256] {
            let mapping = &ontology["x-lattice-chronicle"]["rituals"][operation.as_str()];
            assert_eq!(mapping["technical_operation"], operation.as_str());
            assert!(mapping["activation"]
                .as_str()
                .is_some_and(|value| !value.is_empty()));
            assert!(mapping["failure_boundary"]
                .as_str()
                .is_some_and(|value| !value.is_empty()));
        }
    }

    #[test]
    fn export_schema_fixtures() {
        let f = Fixture::new();
        let mut certificates = Vec::new();
        for (operation, input) in [
            ("echo", ""),
            ("echo", "Unicode: فارسی"),
            ("uppercase", "abc"),
            ("sha256", "abc"),
        ] {
            let record = f.record(operation, input, None);
            certificates.push(f.assess(f.snapshot(&record)));
        }
        let record = f.record("echo", "original", None);
        let mut snapshot = f.snapshot(&record);
        snapshot.execution.payload = "altered".to_owned();
        let rejected = f.assess(snapshot);
        assert!(fails(&rejected, "record_binding"));
        certificates.push(rejected);
        let replay = f.record("echo", "original", Some(record.task_id.clone()));
        certificates.push(f.assess(f.snapshot(&replay)));
        let mut snapshot = f.snapshot(&record);
        snapshot.execution.receipt.attestation = None;
        certificates.push(f.assess(snapshot));
        if let Ok(directory) = std::env::var("AETHER_LATTICE_FIXTURES") {
            fs::create_dir_all(&directory).unwrap();
            for (index, certificate) in certificates.iter().enumerate() {
                fs::write(
                    std::path::Path::new(&directory).join(format!("certificate-{index}.json")),
                    serde_json::to_vec_pretty(certificate).unwrap(),
                )
                .unwrap();
            }
        }
    }
}
