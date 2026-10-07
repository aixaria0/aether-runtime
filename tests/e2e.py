"""Black-box integration checks against a running Aether evidence-carrying core."""
import hashlib
import json
import os
import urllib.error
import urllib.request

BASE = os.environ.get("AETHER_API", "http://localhost:8080")


def request_json(path, method="GET", body=None):
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(
        BASE + path,
        data=data,
        headers={"Content-Type": "application/json"} if data is not None else {},
        method=method,
    )
    with urllib.request.urlopen(request, timeout=20) as response:
        return response.status, json.load(response)


def execute(operation, payload, deadline_ms=10_000):
    return request_json(
        "/execute",
        "POST",
        {"operation": operation, "payload": payload, "deadline_ms": deadline_ms},
    )


def inspect_journal_export(task, receipt, parent=None):
    _, before = request_json('/journal/verify')
    code, export = request_json(f'/lattice/{task}/journal')
    _, after = request_json('/journal/verify')
    assert code == 200 and before == after
    assert export['schema'] == 'aether-journal-export/v1'
    assert export['journal']['coverage'] == 'genesis_to_snapshot_head'
    assert export['lineage']['certificate']['evidence']['journal'] == before
    assert export['lineage']['execution']['receipt'] == receipt
    assert set(export['lineage']['execution']) == {'task_id','parent_task_id','operation','payload','output','receipt'}
    if parent:
        assert export['lineage']['parent']['task_id'] == parent
    else:
        assert export['lineage']['parent'] is None
    previous = None
    events = export['journal']['events']
    assert len(events) == before['event_count'] <= 256
    for index, event in enumerate(events):
        assert event['sequence'] == index + 1
        assert event['previous_hash'] == previous
        timestamp = event['timestamp_ns_decimal']
        assert isinstance(timestamp,str) and str(int(timestamp)) == timestamp
        parts = (previous or '', event['event_id'], event['task_id'], event['event_type'], timestamp, event['payload_json'])
        message = b''.join(len(part.encode()).to_bytes(8,'big') + part.encode() for part in parts)
        assert event['event_hash'] == hashlib.sha256(message).hexdigest()
        previous = event['event_hash']
    assert previous == before['head_hash']


def main():
    with urllib.request.urlopen(BASE + "/health", timeout=20) as response:
        assert response.status == 200

    code, identity = request_json("/identity")
    assert code == 200
    assert identity["algorithm"] == "ed25519"
    assert len(identity["public_key_hex"]) == 64
    assert len(identity["fingerprint_sha256"]) == 64

    code, ontology = request_json("/lattice/ontology")
    assert code == 200
    assert ontology["$schema"] == "https://json-schema.org/draft/2020-12/schema"
    assert set(ontology["x-lattice-chronicle"]["entities"]) == {
        "Agent", "Artifact", "Fracture", "Realm", "Ritual", "Trigger", "Shadow"
    }

    first_task = None
    first_receipt = None
    for operation, input_payload, expected in (
        ("echo", "Hello", "Hello"),
        ("uppercase", "hello", "HELLO"),
        ("sha256", "abc", hashlib.sha256(b"abc").hexdigest()),
        ("echo", "", ""),
    ):
        code, body = execute(operation, input_payload)
        assert code == 200, (operation, body)
        assert body["success"] and body["verified"], body
        assert body["output"] == expected, body
        assert body["output_sha256"] == hashlib.sha256(expected.encode()).hexdigest()
        assert body["task_id"]

        receipt = body["receipt"]
        assert receipt["task_id"] == body["task_id"]
        assert receipt["input_sha256"] == hashlib.sha256(input_payload.encode()).hexdigest()
        assert receipt["output_sha256"] == body["output_sha256"]
        assert receipt["receipt_sha256"]
        assert receipt["verified"] is True
        assert receipt["capability_names"] == ["compute"]
        assert receipt["executor_id"] in {"cpp-grpc-v1", "rust-builtin-v1"}
        attestation = receipt["attestation"]
        assert attestation["algorithm"] == "ed25519"
        assert attestation["domain"] == "aether.execution-receipt.v1"
        assert attestation["public_key_hex"] == identity["public_key_hex"]
        assert attestation["key_fingerprint_sha256"] == identity["fingerprint_sha256"]
        assert len(attestation["signature_hex"]) == 128
        code, verification = request_json("/verify/receipt", "POST", receipt)
        assert code == 200
        assert verification["valid"] is True, verification
        _, journal_before = request_json("/journal/verify")
        code, lineage = request_json(f"/lattice/{body['task_id']}")
        assert code == 200
        assert lineage["schema"] == "lattice-ontology/v1"
        assert lineage["assessment"]["status"] == "accepted", lineage
        assert lineage["assessment"]["failed_invariants"] == []
        assert len(lineage["invariants"]) == 9
        assert all(item["passed"] for item in lineage["invariants"])
        assert lineage["assessment"]["trusted_signer_fingerprint_sha256"] == identity["fingerprint_sha256"]
        assert lineage["evidence"]["receipt"] == receipt
        assert lineage["artifacts"][0]["sha256"] == receipt["input_sha256"]
        assert lineage["artifacts"][1]["sha256"] == receipt["output_sha256"]
        assert lineage["artifacts"][0]["id"] != lineage["artifacts"][1]["id"]
        assert lineage["artifacts"][1]["parent"] == lineage["artifacts"][0]["id"]
        assert lineage["transformation"]["replay_of"] is None
        inspect_journal_export(body['task_id'], receipt)
        assert ontology["x-lattice-chronicle"]["rituals"][operation]["technical_operation"] == operation
        _, journal_after = request_json("/journal/verify")
        assert journal_before == journal_after, "lineage inspection must not append events"
        if first_task is None:
            # Prove the native route works at cold start. Subsequent successful
            # calls may legitimately route to Rust after measuring latency.
            assert receipt["executor_id"] == "cpp-grpc-v1", receipt
            first_task = body["task_id"]
            first_receipt = receipt

    assert first_task
    assert first_receipt

    tampered = dict(first_receipt)
    tampered["output_sha256"] = "00" * 32
    code, verification = request_json("/verify/receipt", "POST", tampered)
    assert code == 200
    assert verification["valid"] is False
    assert verification["digest_matches"] is False
    code, evidence = request_json(f"/evidence/{first_task}")
    assert code == 200
    assert evidence["execution"]["task_id"] == first_task
    assert evidence["execution"]["payload"] == "Hello"
    assert evidence["chain"]["valid"] is True
    assert evidence["receipt_verification"]["valid"] is True
    event_types = [event["event_type"] for event in evidence["events"]]
    assert event_types == [
        "TASK_ACCEPTED",
        "POLICY_AUTHORIZED",
        "EXECUTOR_SELECTED",
        "EXECUTION_STARTED",
        "RECEIPT_SIGNED",
        "VERIFICATION_PASSED",
    ], event_types

    code, replay = request_json(f"/replay/{first_task}", "POST", {})
    assert code == 200
    policy_match = first_receipt["policy_sha256"] == replay["receipt"]["policy_sha256"]
    executor_match = first_receipt["executor_identity_sha256"] == replay["receipt"]["executor_identity_sha256"]
    assert replay["input_match"] and replay["output_match"]
    assert replay["policy_match"] == policy_match
    assert replay["executor_match"] == executor_match
    assert replay["verification_match"]
    expected_divergence = "policy" if not policy_match else "executor_identity" if not executor_match else None
    assert replay["first_divergence"] == expected_divergence, replay
    assert replay["matched"] == (expected_divergence is None), replay
    assert replay["replay_task_id"] != first_task
    assert replay["receipt"]["parent_task_id"] == first_task
    code, replay_lineage = request_json(f"/lattice/{replay['replay_task_id']}")
    assert code == 200
    assert replay_lineage["assessment"]["status"] == "accepted", replay_lineage
    assert replay_lineage["transformation"]["replay_of"] == f"aether:transformation:{first_task}"
    assert replay_lineage["artifacts"][0]["parent"] is None
    assert replay_lineage["artifacts"][0]["sha256"] == first_receipt["input_sha256"]
    inspect_journal_export(replay['replay_task_id'], replay['receipt'], first_task)

    try:
        request_json("/lattice/00000000-0000-0000-0000-000000000000")
        raise AssertionError("missing lineage must return 404")
    except urllib.error.HTTPError as error:
        assert error.code == 404
    try:
        request_json('/lattice/00000000-0000-0000-0000-000000000000/journal')
        raise AssertionError('missing journal export must return 404')
    except urllib.error.HTTPError as error:
        assert error.code == 404

    code, journal = request_json("/journal/verify")
    assert code == 200
    assert journal["valid"] is True
    assert journal["event_count"] >= 21, journal
    assert journal["head_hash"]

    for invalid in (
        {"operation": "shell", "payload": "id", "deadline_ms": 1_000},
        {"operation": "echo", "payload": "ok", "deadline_ms": 0},
        {"operation": "echo", "payload": "ok", "deadline_ms": 10_001},
        {"operation": "uppercase", "payload": "é", "deadline_ms": 1_000},
    ):
        try:
            request_json("/execute", "POST", invalid)
            raise AssertionError(f"unexpected policy acceptance: {invalid}")
        except urllib.error.HTTPError as error:
            assert error.code == 400, (invalid, error.code)

    with urllib.request.urlopen(BASE + "/system/status", timeout=20) as response:
        status = json.load(response)
        assert status["successes"] >= 5, status
        assert 0 <= status["delta"] <= 1, status
        assert status["journal_chain_valid"] is True
        assert status["journal_events"] >= 21
        assert status["journal_head"]
        assert status["ready"] is True
        assert status["identity"]["fingerprint_sha256"] == identity["fingerprint_sha256"]
        assert status["scheduler"]["selected"] in {"cpp-grpc-v1", "rust-builtin-v1"}
        executors = {item["executor_id"] for item in status["scheduler"]["executors"]}
        assert executors == {"cpp-grpc-v1", "rust-builtin-v1"}, executors

    print("Aether evidence + replay end-to-end pipeline: PASS")


if __name__ == "__main__":
    main()
