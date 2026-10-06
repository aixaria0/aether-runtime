# Adaptive execution and durable failure memory

Aether currently exposes two deterministic execution routes for the same allowlisted operation contract:

- `cpp-grpc-v1`: the native C++ gRPC execution engine.
- `rust-builtin-v1`: a Rust in-process fallback implementing the same `echo`, ASCII `uppercase`, and `sha256` semantics.

The control plane owns routing. An executor never authorizes itself and a successful transport response is not enough: the Rust verifier recomputes the expected result before the scheduler records success.

## Scheduler state

For each executor Aether persists:

- successful verified executions;
- failures;
- consecutive failures;
- EWMA latency;
- quarantine deadline.

The state is stored in the existing SQLite WAL database and is reloaded after process restart. It is operational evidence, not a learned model or a proof of optimal scheduling.

A candidate score combines a smoothed reliability prior, bounded latency penalty, and consecutive-failure penalty. Cold start deterministically prefers `cpp-grpc-v1`. Three consecutive failures quarantine an executor for 30 seconds. The fallback route can still serve requests while the native route is unavailable.

## Evidence boundary

Every routing attempt is represented in the hash-chained journal. Relevant events include:

`TASK_ACCEPTED -> POLICY_AUTHORIZED -> EXECUTOR_SELECTED -> EXECUTION_STARTED`

A failed primary route continues as:

`EXECUTOR_FAILED -> POLICY_AUTHORIZED -> FAILOVER_SELECTED -> EXECUTION_STARTED -> VERIFICATION_PASSED`

The final execution receipt contains the identity hash and ID of the executor that actually produced the verified output.

## Readiness semantics

`GET /health` reports Aether runtime readiness: at least one scheduler route is available and the durable journal verifies. It does not mean every executor is healthy.

`GET /system/status` exposes the selected route and per-executor scheduler statistics so an operator can distinguish a healthy degraded runtime from full native availability.

## Verified failure scenario

The integration gate intentionally removes the C++ executor from a fresh runtime and requires Aether to:

1. select C++ first;
2. observe and persist its transport failure;
3. issue a separate permit for the Rust route;
4. execute and independently verify the same task;
5. preserve the resulting failure memory across control-plane restart;
6. route the next equivalent task directly to Rust without repeating the known failure.

## Claim boundary

This feature is adaptive deterministic routing with durable operational memory. It is **not** reinforcement learning, autonomous AGI, Byzantine consensus, remote attestation, or a general-purpose sandbox. Scheduler scores are local heuristics and may be replaced by stronger decision policies later without changing the execution/evidence contract.
