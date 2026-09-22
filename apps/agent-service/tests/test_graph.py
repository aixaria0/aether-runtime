import hashlib
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from agent import build_graph, verifier


def state(goal, plan, result):
    return {
        "goal": goal,
        "plan": plan,
        "result": result,
        "output_sha256": hashlib.sha256(result.encode()).hexdigest(),
    }


def test_graph_compiles():
    assert build_graph() is not None


def test_exact_result_verification():
    assert verifier(state("abc", "uppercase", "ABC"))["verified"]
    assert verifier(state("abc", "sha256", hashlib.sha256(b"abc").hexdigest()))["verified"]
    assert verifier(state("", "echo", ""))["verified"]
    assert not verifier(state("abc", "uppercase", "abc"))["verified"]


def test_tampered_result_is_rejected():
    item = state("abc", "echo", "abc")
    item["output_sha256"] = "0" * 64
    assert not verifier(item)["verified"]


def test_errors_are_not_verified():
    item = state("abc", "echo", "abc")
    item["error"] = "execution failed"
    assert not verifier(item)["verified"]
