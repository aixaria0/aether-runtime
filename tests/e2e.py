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


def main():
    with urllib.request.urlopen(BASE + "/health", timeout=20) as response:
        assert response.status == 200

    code, identity = request_json("/identity")
    assert code == 200
    assert identity["algorithm"] == "ed25519"
    assert len(identity["public_key_hex"]) == 64
    assert len(identity["fingerprint_sha256"]) == 64

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
        assert receipt["executor_id"] == "cpp-grpc-v1"
        attestation = receipt["attestation"]
        assert attestation["algorithm"] == "ed25519"
        assert attestation["domain"] == "aether.execution-receipt.v1"
        assert attestation["public_key_hex"] == identity["public_key_hex"]
        assert attestation["key_fingerprint_sha256"] == identity["fingerprint_sha256"]
        assert len(attestation["signature_hex"]) == 128
        code, verification = request_json("/verify/receipt", "POST", receipt)
        assert code == 200
        assert verification["valid"] is True, verification
        if first_task is None:
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
        "VERIFICATION_PASSED",
    ], event_types

    code, replay = request_json(f"/replay/{first_task}", "POST", {})
    assert code == 200
    assert replay["matched"] is True, replay
    assert replay["first_divergence"] is None
    assert replay["input_match"] and replay["policy_match"]
    assert replay["executor_match"] and replay["output_match"]
    assert replay["verification_match"]
    assert replay["replay_task_id"] != first_task
    assert replay["receipt"]["parent_task_id"] == first_task

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
