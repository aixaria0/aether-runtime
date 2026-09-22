#!/bin/sh
set -eu
curl -fsS http://localhost:8080/health
response=$(curl -fsS -H 'Content-Type: application/json' -d '{"operation":"uppercase","payload":"hello"}' http://localhost:8080/execute)
echo "$response" | grep -q '"output":"HELLO"'
echo "end-to-end smoke test passed"
