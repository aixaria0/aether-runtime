# Aether Machine v0.3 — Evidence + Replay Kernel

Aether v0.3 turns the existing Rust/C++ execution path into an evidence-carrying runtime.

## Implemented boundaries

- Rust policy admission validates operation, input size, deadline, and required capability bits before gRPC execution.
- Current public operations receive only the `compute` capability. File, network, process-spawn, GPU, and remote-execution capability bits exist but are not granted by any current operation.
- Every successful execution emits a versioned receipt containing task, permit, input, output, policy, capability, executor-identity, deadline, duration, and verification fields.
- Rust independently computes the expected result for `echo`, ASCII `uppercase`, and `sha256`; the C++ engine's `success=true` is not sufficient for verification.
- Execution records are stored in SQLite using WAL mode. Docker Compose mounts the database on a named volume.
- Journal events form a global hash chain. Verification recomputes every event hash and reports the first invalid sequence.
- `/evidence/{task_id}` returns the durable execution record, task events, and chain-verification result.
- `/replay/{task_id}` executes the stored input again under the current policy/executor identity and reports the first divergence across input, policy, executor identity, output, or verification.
- SQLite operations run through Tokio blocking workers so journal I/O does not directly block async request workers.
- The browser console exposes deadline selection, receipt metadata, evidence inspection, replay, and journal health.

## API

### Execute

`POST /execute`

```json
{
  "operation": "sha256",
  "payload": "hello",
  "deadline_ms": 5000
}
```

A successful response includes `verified: true` and an `ExecutionReceipt`.

### Evidence

`GET /evidence/{task_id}`

Returns the stored execution, ordered events for that task, and verification of the global event chain.

### Replay

`POST /replay/{task_id}`

Re-executes the original operation/input and compares the new receipt to the original receipt. A match means the compared boundaries are equal; it does not prove arbitrary program determinism.

### Journal verification

`GET /journal/verify`

Returns `valid`, `event_count`, `head_hash`, and `first_invalid_sequence`.

## Persistence

Default native path: `./aether-journal.sqlite3`.

Docker Compose uses:

```text
AETHER_JOURNAL_PATH=/data/aether-journal.sqlite3
```

with the named volume `aether-data`.

## What this does not claim

This release does **not** provide a cryptographic signature, remote attestation, distributed consensus, a general-purpose sandbox, arbitrary shell execution, or an AGI proof. `executor_identity_sha256` currently fingerprints the declared executor identity string (`cpp-grpc-v1`); it is not a hash of the running native binary.

The hash chain is tamper-evident relative to its stored head and recomputed history. An attacker who can replace the whole database and every external reference to its head remains outside this trust boundary.

## Next research boundaries

1. WASM component executor with fuel/memory budgets and explicit host capabilities.
2. Signed receipts with managed Ed25519 identities and key rotation.
3. External anchoring of journal heads for stronger tamper evidence.
4. Executor binary/environment fingerprints and reproducible capsule manifests.
5. Adaptive scheduling using measured verification success, latency, cost, and failure fingerprints.
