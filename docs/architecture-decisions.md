# Architecture decisions recovered on 2026-10-07

Sources: the shared Manifest breakdown conversation, the attached handoff, and
the repository baseline at `c5e67baf69148a2a40776465323e657a4e2b2593`.

Historical discussion and proposed code are context, not evidence of implemented
features. The inspected baseline contains the Rust control plane, C++ gRPC
executor, Python agent, policy permits, operation recomputation, SQLite WAL
journal, replay/divergence detection, adaptive failover and persisted Ed25519
identity. Its preceding commit adds the ontology foundation document; the
discussed Lattice models are not in that tree.

## Decisions

1. Keep aether-runtime canonical. Lattice is an internal semantic verification
   layer. Preserve runtime contracts and layout; avoid a new repository or rewrite.
2. Reuse identity, signed receipt serialization, journal and recomputation.
3. Deliver Artifact -> Transformation -> Evidence -> Verification as a bounded
   vertical slice before adding arbitrary workflows or model operations.
4. Keep all seven symbolic mappings explicit. Every supported Ritual needs an
   exact technical operation, activation condition and failure boundary.
5. Separate imported root occurrences, content hashes and execution provenance.
   Replay repeats original input; it does not consume original output.
6. Choose trust anchors and policy in the verifier. A valid self-declared signer
   is insufficient to establish trusted origin.
7. Preserve unknown claims. Earlier entropy/compression/fidelity/Lyapunov-like
   principles are research hypotheses or metaphors, not established invariants.
8. Change only user-owned personal repositories. Contributor-only repositories,
   especially RChain upstream/contribution work, are excluded.
9. Report implementation only with real commits and executed checks. Keep slices
   reversible and reviewable.
10. Archive recovered conversation content with message IDs, source links and
    hashes. Identify unavailable tool output and image pixels explicitly.

## Ontology normalization

| Original symbol | Neutral term | Current implementation scope |
|---|---|---|
| Agent | Executor | Existing C++ and Rust executor IDs |
| Artifact | State occurrence | Bounded input/output strings with hashes |
| Fracture | Transformation | Existing allowlisted operation |
| Realm | Execution domain | Local compute boundary |
| Ritual | Procedure | Echo, ASCII uppercase, SHA-256 |
| Trigger | Activation predicate | Existing operation/input/deadline admission |
| Shadow | Perturbation or unknown influence | Explicitly unevaluated general properties |

Invocation/transformation/stabilization correspond to admission, execution and
result/evidence checks. The original ontology is preserved in the context
archive. Compression, distillation, encryption, randomness and optimization
classes do not become capabilities merely by appearing there.

## Next vertical slices

1. An OMEGA adapter independently selects a trusted signer, versioned operation
   semantics and journal reference, then consumes this certificate contract.
2. A signer/policy registry supports explicit historical key and semantic revisions.
3. External journal anchoring binds selected heads outside the replaceable database.
4. Multi-input transformations and validated DAG composition distinguish content
   links, execution provenance and accepted-state transitions.
5. Per-transformation perturbation/fidelity suites supply actual metric definitions,
   evaluators and negative controls before claiming new measured properties.

These are future actions. Each needs a reproducible fixture, negative control,
schema version and assurance boundary. See the full context archive for retained
technical detail and the lineage contract for the implemented predicate.
