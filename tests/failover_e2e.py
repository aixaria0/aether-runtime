"""Black-box adaptive failover checks against a running Aether control plane.

The execution engine is expected to be unavailable. In normal mode the test
proves C++ is attempted first, its failure is journaled, and Rust verifies the
same task. With --post-restart it proves persisted scheduler memory avoids
retrying the known-bad C++ route after the control plane restarts.
"""
import argparse
import json
import os
import urllib.request

BASE = os.environ.get("AETHER_API", "http://localhost:8080")
CPP = "cpp-grpc-v1"
RUST = "rust-builtin-v1"


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


def status():
    code, body = request_json("/system/status")
    assert code == 200 and body["ready"] is True, body
    return body


def stats_by_id(body):
    return {item["executor_id"]: item for item in body["scheduler"]["executors"]}


def execute(payload):
    code, body = request_json(
        "/execute",
        "POST",
        {"operation": "echo", "payload": payload, "deadline_ms": 1_000},
    )
    assert code == 200, body
    assert body["success"] is True and body["verified"] is True, body
    assert body["output"] == payload, body
    assert body["receipt"]["executor_id"] == RUST, body["receipt"]
    return body


def initial_failover():
    with urllib.request.urlopen(BASE + "/health", timeout=20) as response:
        assert response.status == 200

    before = stats_by_id(status())
    assert before[CPP]["successes"] == 0 and before[CPP]["failures"] == 0, before
    assert before[RUST]["successes"] == 0 and before[RUST]["failures"] == 0, before

    result = execute("failover-proof")
    _, evidence = request_json(f"/evidence/{result['task_id']}")
    assert evidence["chain"]["valid"] is True, evidence["chain"]
    events = evidence["events"]
    event_types = [event["event_type"] for event in events]
    assert event_types == [
        "TASK_ACCEPTED",
        "POLICY_AUTHORIZED",
        "EXECUTOR_SELECTED",
        "EXECUTION_STARTED",
        "EXECUTOR_FAILED",
        "POLICY_AUTHORIZED",
        "FAILOVER_SELECTED",
        "EXECUTION_STARTED",
        "RECEIPT_SIGNED",
        "VERIFICATION_PASSED",
    ], event_types

    first = json.loads(next(e["payload_json"] for e in events if e["event_type"] == "EXECUTOR_SELECTED"))
    fallback = json.loads(next(e["payload_json"] for e in events if e["event_type"] == "FAILOVER_SELECTED"))
    assert first["executor_id"] == CPP, first
    assert fallback["executor_id"] == RUST, fallback

    after = stats_by_id(status())
    assert after[CPP]["failures"] == 1, after
    assert after[RUST]["successes"] == 1, after
    print("Aether adaptive C++ -> Rust failover: PASS")


def post_restart_memory():
    with urllib.request.urlopen(BASE + "/health", timeout=20) as response:
        assert response.status == 200

    before = stats_by_id(status())
    cpp_failures = before[CPP]["failures"]
    rust_successes = before[RUST]["successes"]
    assert cpp_failures >= 1 and rust_successes >= 1, before

    result = execute("failure-memory-proof")
    _, evidence = request_json(f"/evidence/{result['task_id']}")
    event_types = [event["event_type"] for event in evidence["events"]]
    assert "EXECUTOR_FAILED" not in event_types, event_types
    selected = json.loads(next(e["payload_json"] for e in evidence["events"] if e["event_type"] == "EXECUTOR_SELECTED"))
    assert selected["executor_id"] == RUST, selected

    after = stats_by_id(status())
    assert after[CPP]["failures"] == cpp_failures, (before, after)
    assert after[RUST]["successes"] == rust_successes + 1, (before, after)
    print("Aether durable failure memory after restart: PASS")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--post-restart", action="store_true")
    args = parser.parse_args()
    if args.post_restart:
        post_restart_memory()
    else:
        initial_failover()


if __name__ == "__main__":
    main()
