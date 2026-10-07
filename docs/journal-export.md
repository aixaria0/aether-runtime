# Bounded journal-lineage export v1

GET `/lattice/{task_id}/journal` returns one read-only SQLite snapshot containing
the existing unsigned lineage certificate, six raw execution fields, the direct
parent when present, and every journal event from genesis through that snapshot's
head. HTTP 404 means the task is absent. HTTP 413 means bounded export cannot be
provided; it never means that a partial history passed verification.

The existing execution, receipt-signing format, journal tables and HTTP inspection
contracts are unchanged. This endpoint does not paginate, anchor history externally,
attest a binary or promote producer assessment flags to consumer authority.

## Wire contract

The outer object has exactly `schema: aether-journal-export/v1`, `lineage`, and
`journal`. `lineage` has `schema: chimera-aether-lineage-evidence/v1`, `certificate`,
`execution`, and `parent` (null for a root). Each raw execution has exactly
`task_id`, `parent_task_id`, `operation`, `payload`, `output`, and `receipt`.
Nanosecond creation timestamps are excluded from raw execution records.

`journal` has `schema: aether-journal-events/v1`,
`coverage: genesis_to_snapshot_head`, and `events` in increasing sequence order.
Each event has `sequence`, `event_id`, `task_id`, `event_type`,
`timestamp_ns_decimal`, `payload_json`, `previous_hash`, and `event_hash`.
`timestamp_ns_decimal` preserves the exact positive decimal integer as a string;
it is neither rounded to a JSON number nor converted to a floating-point time.
`payload_json` preserves the original compact, sorted string-map JSON bytes.

The existing event hash is SHA-256 of six length-prefixed UTF-8 parts, in order:
previous hash (empty for genesis), event ID, task ID, event type, decimal
timestamp, and raw payload JSON. Each length is an eight-byte big-endian byte
count. Sequence is not hashed in the existing format; a consumer must separately
require contiguous sequences beginning at 1, unique event IDs, exact count and
the externally selected head. Missing events and arbitrary suffixes are invalid.

## Limits and snapshot boundary

The reader requests at most 257 rows and rejects a 257th event. Event string
fields and raw execution task/parent/operation/input/output fields are checked
against 4096 UTF-8 bytes in SQLite before materialization. Encoded receipt JSON
is limited to 16384 bytes. The final compact response is limited to 1 MiB.
These are export limits, not changes to Aether's existing 64 KiB execution policy.
A consumer still applies its own complete JSON profile and semantic checks.

The execution lookup, direct parent, complete bounded events and chain assessment
share one SQLite read transaction; the certificate is derived from that snapshot.
Crypto, hashing and JSON encoding run on a blocking worker. Reads append no events.
Histories beyond 256 events require a future version with an explicitly trusted
starting checkpoint and coverage contract; v1 refuses them.

## Consumer procedure

1. Retrieve this envelope without mixing separate HTTP reads.
2. Independently select signer, reviewed source semantics, exact certificate
   digest and journal head/count; pin the complete export/context through a
   trusted review channel.
3. Verify the signed raw operation and graph/direct-parent references.
4. Recompute every event hash, links, sequence coverage and selected head/count.
5. Bind the selected receipt to exactly one RECEIPT_SIGNED event (algorithm,
   signer fingerprint, receipt digest) and a later VERIFICATION_PASSED event
   (executor ID, receipt digest). Repeat for the direct replay parent.
6. Keep external anchoring, independent observation, binary attestation and
   freshness unestablished. A hash-consistent caller-selected history is not an
   independently observed or globally canonical history.

Unit controls exercise exact timestamp preservation, read-only snapshots, missing
tasks, maximum/overflow counts, oversized database fields and corrupt history.
The cross-repository consumer conformance slice adds independent recomputation
and receipt-event adversarial controls under an externally pinned context.
