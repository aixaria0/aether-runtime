"""Black-box integration checks against a running Docker Compose core."""
import hashlib
import json
import os
import urllib.error
import urllib.request

BASE = os.environ.get("AETHER_API", "http://localhost:8080")


def post(operation, payload):
    body = json.dumps({"operation": operation, "payload": payload}).encode()
    request = urllib.request.Request(
        BASE + "/execute",
        data=body,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=20) as response:
        return response.status, json.load(response)


def main():
    with urllib.request.urlopen(BASE + "/health", timeout=20) as response:
        assert response.status == 200

    for operation, payload, expected in (
        ("echo", "Hello", "Hello"),
        ("uppercase", "hello", "HELLO"),
        ("sha256", "abc", hashlib.sha256(b"abc").hexdigest()),
        ("echo", "", ""),
    ):
        code, body = post(operation, payload)
        assert code == 200, (operation, body)
        assert body["success"] and body["output"] == expected, body
        assert body["output_sha256"] == hashlib.sha256(expected.encode()).hexdigest()
        assert body["task_id"]
        assert body["verified"] is True
        receipt = body["receipt"]
        assert receipt["task_id"] == body["task_id"]
        assert receipt["input_sha256"] == hashlib.sha256(payload.encode()).hexdigest()
        assert receipt["output_sha256"] == body["output_sha256"]
        assert receipt["policy_revision"] == 1
        assert receipt["route"] == "local"
    try:
        post("shell", "id")
        raise AssertionError("unexpected acceptance of unsupported operation")
    except urllib.error.HTTPError as error:
        assert error.code == 400

    for rejected in (
        {"operation": "echo", "payload": "abc", "deadline_ms": 0},
        {"operation": "echo", "payload": "abc", "deadline_ms": 10001},
        {"operation": "uppercase", "payload": "é"},
    ):
        request = urllib.request.Request(
            BASE + "/execute",
            data=json.dumps(rejected).encode(),
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        try:
            urllib.request.urlopen(request, timeout=20)
            raise AssertionError("policy accepted an invalid task")
        except urllib.error.HTTPError as error:
            assert error.code == 400

    with urllib.request.urlopen(BASE + "/system/status", timeout=20) as response:
        status = json.load(response)
        assert status["successes"] >= 4, status
        assert 0 <= status["delta"] <= 1, status
    print("Aether core end-to-end pipeline: PASS")


if __name__ == "__main__":
    main()
