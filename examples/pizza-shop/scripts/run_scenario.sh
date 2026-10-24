#!/usr/bin/env bash
# Runs the whole pizza shop scenario: the warden, the four agents, and one
# order with the two gated skills approved by an automated supervisor.
#
#   scripts/run_scenario.sh
#
# The script expects the warden binary to be built (cargo build) and the
# dependencies of the demo to be installed (pip install -r requirements.txt).

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
DEMO="$ROOT/examples/pizza-shop"
PYTHON="${PYTHON:-python3}"

cd "$ROOT"

echo "==> building the warden"
cargo build --bin a2a-warden

echo "==> starting the warden"
rm -f "$ROOT/a2a-warden-pizzeria.db"*
"$ROOT/target/debug/a2a-warden" \
    --config "$DEMO/warden-pizzeria.yaml" &
WARDEN_PID=$!

cleanup() {
    kill "$WARDEN_PID" 2>/dev/null || true
    for pid in "${AGENT_PIDS[@]:-}"; do
        kill "$pid" 2>/dev/null || true
    done
}
trap cleanup EXIT

echo "==> waiting for the warden"
for _ in $(seq 1 30); do
    if curl -fsS http://127.0.0.1:8080/api/v1/admin/health > /dev/null 2>&1; then
        break
    fi
    sleep 1
done

echo "==> starting the agents"
AGENT_PIDS=()
cd "$DEMO"
for agent in inventory_agent payments_agent delivery_agent order_agent; do
    "$PYTHON" -m "agents.$agent" &
    AGENT_PIDS+=("$!")
done
sleep 3

echo "==> running the scenario"
cd "$DEMO"
"$PYTHON" scripts/run_demo.py
