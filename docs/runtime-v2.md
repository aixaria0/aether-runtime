# Aether Runtime v0.2 design notes

The execution plane supports three allowlisted operations. No arbitrary command execution occurs.

## Concurrency and readiness

The Rust API uses a bounded semaphore; when the configured limit is reached, requests return HTTP 429. The `MAX_CONCURRENT_TASKS` environment variable accepts values from 1 through 4096 and defaults to 32. Readiness probes the C++ gRPC health endpoint.

## Operational health

`delta` is an exponentially weighted signal over reported execution success and transport failures. The initial value is 1.0, but this is a starting assumption and NOT a proof of consistency, reliability, or mathematical convergence. All counters and the signal are currently process-local and reset on restart.

## Evidence and verification

Each response includes an output SHA-256 digest, an operation name, a task ID and local elapsed time. SHA-256 is an integrity aid, not a signature or a proof of correct execution. The Python verifier recomputes outputs for the allowlisted operations instead of trusting a response's success flag.

## Deliberate constraints

- HTTP API authentication, per-tenant quotas, persistent event storage and distributed consensus are not implemented.
- Temporal activity wiring remains an optional integration: the worker must be explicitly started.
- The AI planner proposes only one of three allowlisted operations, and the Rust API independently validates that choice.
- CPU- and locale-specific text uppercase conversion differs between Python Unicode and C++ byte-based `std::toupper`; use ASCII for `uppercase` until a documented Unicode semantics layer is added.
- Only run the compose stack in a trusted local development environment; add TLS, authentication and network isolation before exposing it externally.
