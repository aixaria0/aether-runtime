# Aether Runtime

A Rust/C++ runtime for bounded execution with signed evidence, durable replay,
and a LatticeChronicle semantic lineage layer.

Current operations are `echo`, ASCII `uppercase`, and `sha256`. Rust admits
compute-only permits, recomputes results, signs receipts with a persisted
Ed25519 identity, and stores execution evidence in a SQLite WAL journal. A C++
gRPC route and Rust fallback share the operation contract; adaptive routing
records failure memory across restart.

Lattice derives input/output artifact occurrences and transformations from this
evidence. Acceptance requires nine current-policy and provenance checks. General
model fidelity, perturbation stability and entropy remain unevaluated. The
certificate is a derived view containing the signed execution receipt.

## Try it

```bash
docker compose up --build -d execution-engine control-plane
```

Open `http://localhost:8080`, execute a task, then choose **Inspect lineage**.
Retain this trusted local topology until authentication, transport security
and deployment isolation are configured for wider access.

| Endpoint | Purpose |
|---|---|
| `POST /execute` | Execute an admitted operation and return a signed receipt |
| `GET /evidence/{task_id}` | Inspect persisted execution and journal events |
| `GET /lattice/{task_id}` | Inspect artifact lineage and invariant checks |
| `GET /lattice/ontology` | Retrieve ontology mappings and JSON Schema |
| `POST /replay/{task_id}` | Repeat original input and compare boundaries |
| `GET /identity` | Retrieve runtime public identity |
| `GET /journal/verify` | Recompute the journal hash chain |

## Design and evidence

- [Lineage contract](docs/lattice-lineage.md)
- [Architecture decisions and next slices](docs/architecture-decisions.md)
- [Ontology foundation](docs/lattice-chronicle-ontology.md)
- [Recovered conversation and provenance](docs/context/README.md)
- [Adaptive execution](docs/adaptive-execution.md)
- [Historical v0.3 evidence kernel](docs/aether-machine-v0.3.md)

General model transformations, remote hardware attestation, distributed
consensus and arbitrary shell execution are outside the current implementation.
