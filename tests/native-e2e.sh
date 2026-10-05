#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUILD_DIR="${AETHER_NATIVE_BUILD_DIR:-$ROOT/.native-build}"
ENGINE_BUILD="$BUILD_DIR/execution-engine"
ENGINE_BIN="$ENGINE_BUILD/aether-execution-engine"
CONTROL_BIN="$ROOT/apps/control-plane/target/release/aether-control-plane"
ENGINE_LOG="$BUILD_DIR/execution-engine.log"
CONTROL_LOG="$BUILD_DIR/control-plane.log"
ENGINE_PID=""
CONTROL_PID=""

cleanup() {
  if [[ -n "$CONTROL_PID" ]]; then kill "$CONTROL_PID" 2>/dev/null || true; fi
  if [[ -n "$ENGINE_PID" ]]; then kill "$ENGINE_PID" 2>/dev/null || true; fi
  if [[ -n "$CONTROL_PID" ]]; then wait "$CONTROL_PID" 2>/dev/null || true; fi
  if [[ -n "$ENGINE_PID" ]]; then wait "$ENGINE_PID" 2>/dev/null || true; fi
}
trap cleanup EXIT INT TERM

mkdir -p "$ENGINE_BUILD"
cmake -S "$ROOT/apps/execution-engine" -B "$ENGINE_BUILD" -G Ninja -DCMAKE_BUILD_TYPE=Release -DBUILD_TESTING=ON
cmake --build "$ENGINE_BUILD" -j2
ctest --test-dir "$ENGINE_BUILD" --output-on-failure

cargo +1.90.0 build --locked --release --manifest-path "$ROOT/apps/control-plane/Cargo.toml"

"$ENGINE_BIN" >"$ENGINE_LOG" 2>&1 &
ENGINE_PID=$!
ENGINE_URL=http://127.0.0.1:50051 MAX_CONCURRENT_TASKS=8 \
  "$CONTROL_BIN" >"$CONTROL_LOG" 2>&1 &
CONTROL_PID=$!

ready=0
for _ in $(seq 1 60); do
  if curl --fail --silent http://127.0.0.1:8080/health >/dev/null; then
    ready=1
    break
  fi
  if ! kill -0 "$ENGINE_PID" 2>/dev/null || ! kill -0 "$CONTROL_PID" 2>/dev/null; then
    break
  fi
  sleep 1
done

if [[ "$ready" -ne 1 ]]; then
  echo "native Aether core did not become ready" >&2
  echo "--- execution engine ---" >&2
  cat "$ENGINE_LOG" >&2 || true
  echo "--- control plane ---" >&2
  cat "$CONTROL_LOG" >&2 || true
  exit 1
fi

AETHER_API=http://127.0.0.1:8080 python3 "$ROOT/tests/e2e.py"
echo "Aether native core smoke: PASS"
