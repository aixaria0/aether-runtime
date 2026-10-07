# LatticeChronicle execution lineage v1

Lattice derives a constrained transformation graph from Aether's persisted
execution records and Ed25519 receipts. It adds no execution operation, signing
format, database table, or scheduling decision.

## Run the slice

1. Start the existing trusted development stack with
   `docker compose up --build -d execution-engine control-plane`.
2. Open `http://localhost:8080`, execute a task, and choose **Inspect lineage**.
3. Or submit a bounded operation through `POST /execute`, then request
   `GET /lattice/{task_id}` using the returned task ID.
4. Retrieve `GET /lattice/ontology` for the embedded Draft 2020-12 schema and
   symbolic-to-technical mappings. It is versioned at
   `schemas/lattice-ontology.v1.json`.

A known task returns HTTP 200 with an `accepted` or `rejected` assessment. A
missing task returns 404. HTTP status indicates retrieval; the assessment
carries the verification result. Reads do not append events.

## Graph semantics

Input and output occurrences form two artifact nodes, identified by
`aether:artifact:{task_id}:input` and `aether:artifact:{task_id}:output`. Their
`sha256` fields hash actual persisted UTF-8 bytes. Occurrence identity differs
from content identity: `echo` preserves every byte without creating a graph
self-loop. The output's `parent` references this execution's input.

The transformation ID is `aether:transformation:{task_id}`. It references both
occurrences and the existing receipt digest. Inputs are imported roots; this
slice does not claim to prove how submitted inputs were created.

Replay executes the original input again. `transformation.replay_of` records
execution provenance. It never claims that replay consumed the original
output. Assessment requires an authentic stored parent with matching operation
and input, matching stored bytes, and current policy compatibility. This is a
direct-parent check, not verification of an arbitrary ancestor DAG.

## Acceptance predicate

All nine invariants must pass. The verifier selects the current runtime identity
and operation policy; the receipt cannot select its own trust anchor.

| Invariant | Required evidence |
|---|---|
| `receipt_attestation` | Recomputed digest, Ed25519 algorithm, signature domain, key fingerprint and valid signature |
| `trusted_signer` | Receipt signer equals the current persisted runtime identity |
| `record_binding` | Valid task/permit UUIDs; task, operation and parent agree; input/output bytes match commitments |
| `operation_result` | Current echo, ASCII uppercase or SHA-256 semantics recompute the exact stored result |
| `current_policy` | Current input/output/deadline admission bounds, compute capabilities, revision/digest and executor descriptor match |
| `verified_receipt_v2` | Receipt version 2 and execution verification flag |
| `journal_integrity` | Existing global journal hash chain recomputes successfully |
| `journal_receipt_binding` | Ordered signed/verified events bind task, receipt, signer, algorithm and executor |
| `replay_provenance` | Root execution, or authentic matching original execution for a replay |

The execution, direct parent, task events and global chain are read in one
SQLite read transaction. This provides a consistent snapshot while WAL writers
continue. SQLite work, signatures and recomputation run on a blocking worker.

The certificate includes the original receipt, signature-verification flags,
snapshot journal head, invariant results, failure names, and selected signer.
The certificate is a derived **unsigned** view. Only its embedded receipt is
signed. Graph fields, assessment and journal head are not covered by that receipt
signature. Consumers must recompute acceptance under their own trusted inputs.
JSON Schema validates structure, not cryptographic truth or reference equality.

## Measurement and trust boundaries

Acceptance applies to this allowlisted execution lineage under the current
signer and policy. Key rotation or policy changes can reject historical records;
v1 has no historical trust registry. Unsigned legacy receipts cannot pass.

These claims remain `not_evaluated`:

- perturbation stability;
- statistical entropy;
- generalized model fidelity;
- external journal anchoring;
- executor binary/environment attestation.

Result recomputation establishes equality with defined local semantics. It is
not a general AI evaluation, safety theorem or arbitrary-program determinism
proof. Executor identity fingerprints a declared string. The journal has no
independent external anchor. Whole-history replacement, a compromised trusted
signing key and a compromised verifier remain outside this assurance. Local
elapsed times are observations rather than externally attested timing bounds.

Like evidence inspection, lineage scans the global journal in O(event count).
The development deployment has no API authentication or per-tenant audit quota;
retain its trusted-network boundary.

## CHIMERA boundary

The provider manifest declares read-only `aether.inspect-lineage` and advisory
`aether.lattice-ontology` HTTP capabilities. The existing execute contract is
retained. This declaration does not claim that OMEGA already consumes the
certificate; a verifier-selected OMEGA adapter is a subsequent slice.

## Verification

Rust tests cover valid empty/ASCII/Unicode inputs, reopened journals, altered
bytes/receipts, a valid signature on the wrong operation, a foreign signer,
unsigned receipts, capability escalation, corrupt/missing journal evidence,
and correct/missing/mismatched replay parents. CI validates seven actual Rust
exports, including rejection reports, against the schema. Five structural
negative controls reject missing evidence, invented metrics, accepted failed
checks, hidden failures and non-allowlisted operations. Integration tests inspect
each operation and replay, require read-only journal behavior, and inspect
failover lineage before and after restart.
