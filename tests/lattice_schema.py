"""Validate real Rust exports, including rejection reports, against the ontology.

Run cargo test with AETHER_LATTICE_FIXTURES set, then pass that directory here.
The schema validates structure. Runtime evidence checks determine acceptance.
"""
import copy
import json
from pathlib import Path
import sys

from jsonschema import Draft202012Validator


def main():
    root = Path(__file__).resolve().parents[1]
    schema = json.loads((root / "schemas/lattice-ontology.v1.json").read_text())
    Draft202012Validator.check_schema(schema)
    validator = Draft202012Validator(schema)
    paths = sorted(Path(sys.argv[1]).glob("certificate-*.json"))
    assert len(paths) == 7, f"expected seven real Rust exports, got {len(paths)}"
    certificates = [json.loads(path.read_text()) for path in paths]
    for certificate in certificates:
        validator.validate(certificate)

    accepted = next(c for c in certificates if c["assessment"]["status"] == "accepted")
    rejected = next(c for c in certificates if c["assessment"]["status"] == "rejected")
    mutations = []
    missing_evidence = copy.deepcopy(accepted)
    del missing_evidence["evidence"]
    mutations.append(missing_evidence)
    invented_metric = copy.deepcopy(accepted)
    invented_metric["assessment"]["stability"] = 1.0
    mutations.append(invented_metric)
    failed_but_accepted = copy.deepcopy(accepted)
    failed_but_accepted["invariants"][0]["passed"] = False
    mutations.append(failed_but_accepted)
    failure_hidden = copy.deepcopy(rejected)
    failure_hidden["assessment"]["failed_invariants"] = []
    mutations.append(failure_hidden)
    non_allowlisted = copy.deepcopy(accepted)
    non_allowlisted["transformation"]["operation"] = "shell"
    mutations.append(non_allowlisted)
    for mutation in mutations:
        assert list(validator.iter_errors(mutation)), "invalid certificate passed schema"
    assert {c["assessment"]["status"] for c in certificates} == {"accepted", "rejected"}
    assert any(c["transformation"]["replay_of"] for c in certificates)
    print(f"Lattice ontology: {len(paths)} Rust exports + {len(mutations)} negative controls PASS")


if __name__ == "__main__":
    main()
