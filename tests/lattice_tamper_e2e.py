"""Negative controls against a dedicated native test journal and running core.

Set AETHER_TEST_JOURNAL_PATH to the fixture journal used by that core. The
controls alter only the task created here and restore its original values.
"""
import json
import os
from pathlib import Path
import sqlite3
import urllib.request

BASE = os.environ.get("AETHER_API", "http://localhost:8080")


def request(path, body=None):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(
        BASE + path,
        data=data,
        headers={"Content-Type": "application/json"} if data is not None else {},
        method="GET" if data is None else "POST",
    )
    with urllib.request.urlopen(req, timeout=20) as response:
        assert response.status == 200
        return json.load(response)


def main():
    path = Path(os.environ["AETHER_TEST_JOURNAL_PATH"]).resolve(strict=True)
    result = request("/execute", {"operation": "echo", "payload": "lattice-negative-control"})
    task = result["task_id"]
    assert request(f"/lattice/{task}")["assessment"]["status"] == "accepted"
    before = request("/journal/verify")
    with sqlite3.connect(f"file:{path}?mode=rw", uri=True) as conn:
        output = conn.execute("SELECT output FROM executions WHERE task_id = ?", (task,)).fetchone()[0]
        event = conn.execute(
            "SELECT sequence, payload_json FROM execution_events WHERE task_id = ? AND event_type = 'VERIFICATION_PASSED'",
            (task,),
        ).fetchone()
        assert output == result["output"] and event is not None
        try:
            conn.execute("UPDATE executions SET output = ? WHERE task_id = ?", ("changed", task))
            conn.commit()
            altered = request(f"/lattice/{task}")
            assert altered["evidence"]["receipt_verification"]["valid"] is True
            assert altered["evidence"]["journal"]["valid"] is True
            assert altered["assessment"]["status"] == "rejected", altered
            assert {"record_binding", "operation_result"} <= set(altered["assessment"]["failed_invariants"])
            assert request("/journal/verify") == before
            conn.execute("UPDATE executions SET output = ? WHERE task_id = ?", (output, task))
            conn.execute("UPDATE execution_events SET payload_json = ? WHERE sequence = ?", ('{"changed":"yes"}', event[0]))
            conn.commit()
            broken_chain = request(f"/lattice/{task}")
            assert broken_chain["assessment"]["status"] == "rejected", broken_chain
            assert "journal_integrity" in broken_chain["assessment"]["failed_invariants"]
        finally:
            conn.execute("UPDATE executions SET output = ? WHERE task_id = ?", (output, task))
            conn.execute("UPDATE execution_events SET payload_json = ? WHERE sequence = ?", (event[1], event[0]))
            conn.commit()
    assert request("/journal/verify") == before
    assert request(f"/lattice/{task}")["assessment"]["status"] == "accepted"
    print("Lattice stored-byte and journal tampering negative controls: PASS")


if __name__ == "__main__":
    main()
