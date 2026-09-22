import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from agent import build_graph, verifier

def test_graph_compiles():
    assert build_graph() is not None

def test_verification():
    assert verifier({"goal": "abc", "plan": "uppercase", "result": "ABC"})["verified"]
    assert not verifier({"goal": "abc", "plan": "uppercase", "result": "abc"})["verified"]
