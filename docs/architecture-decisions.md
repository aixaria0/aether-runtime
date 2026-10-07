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

## Completed continuation: OMEGA lineage consumer

The first proposed consumer slice is implemented in CHIMERA-OMEGA feature commit
`8cbd6ddc169d0c641c109e562c3947538c39b057`, merged in
[PR #11](https://github.com/aixaria0/CHIMERA-OMEGA/pull/11) at
`91c5e28a0713c50c0b9921a9dfd2c0c98026b412`. It consumes Aether's existing
certificate and separately exported raw execution bytes, with an external
context digest selecting trust and source semantics at Aether revision
`04d785f7beee5ff96d78e092a1d73b79987c9e0e`.

The adapter independently verifies receipt signatures, operation results,
current compute policy, graph references and authenticated direct replay parents.
It confirms only signed operation and pinned lineage bindings. The journal
head/count is a selected reference; full event-chain integrity, receipt-event
inclusion, independent observation and producer binary attestation remain
unestablished. Existing OMEGA core verdict rules and the distinct Aether Runtime
OS importer are preserved. See [the lineage contract](lattice-lineage.md#chimera-boundary)
and the pinned consumer documentation linked there for exact technical details.

Validation on the feature commit passed all 83 Rust tests, six Node tests,
Clippy with warnings denied, five live signed exports, eight live rejection
controls and all six GitHub push/PR checks. The
[live interop CI run](https://github.com/aixaria0/CHIMERA-OMEGA/actions/runs/37559982734)
published `aether-lineage-evidence` (artifact ID `11456396986`), archive digest
`sha256:2673d6671536d16df9ece62257f6a7d24a2606df10c0fc4d5e5aaeb05d7cb0db`.
Recorded public fixtures and per-file/source hashes also live in the consumer
commit; private signing material is excluded.

## Next vertical slices

1. Complete the consumer side of the [bounded journal export](journal-export.md):
   independently recompute existing chain hashes and ordered receipt-event
   bindings under a caller-selected checkpoint. Keep the first-stage consumer
   unchanged and require explicit, fail-closed genesis-to-head coverage.
2. A signer/policy registry supports explicit historical key and semantic revisions.
3. External journal anchoring binds selected heads outside the replaceable database.
4. Multi-input transformations and validated DAG composition distinguish content
   links, execution provenance and accepted-state transitions.
5. Per-transformation perturbation/fidelity suites supply actual metric definitions,
   evaluators and negative controls before claiming new measured properties.

These are future actions. Each needs a reproducible fixture, negative control,
schema version and assurance boundary. See the full context archive for retained
technical detail and the lineage contract for the implemented predicate.
