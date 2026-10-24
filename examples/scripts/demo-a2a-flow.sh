#!/usr/bin/env bash
#
# A2A Warden Demo Script: complete A2A communication flow.
#
# This script automates the end-to-end flow of two agents communicating
# through A2A Warden as the intermediary:
#
#   1. Start the example echo agent.
#   2. Start the warden.
#   3. Register the remote agent in the warden.
#   4. Send a task from the client.
#   5. Approve the task as a human supervisor.
#   6. Inspect the recorded timeline and audit trail.
#
# Requirements: a built warden (target/release/a2a-warden) and Python 3.10+.

set -euo pipefail

WARDEN_HOST=${WARDEN_HOST:-127.0.0.1}
WARDEN_PORT=${WARDEN_PORT:-8080}
AGENT_HOST=${AGENT_HOST:-127.0.0.1}
AGENT_PORT=${AGENT_PORT:-9001}
WARDEN_URL="http://${WARDEN_HOST}:${WARDEN_PORT}"
AGENT_URL="http://${AGENT_HOST}:${AGENT_PORT}"
CONFIG=${CONFIG:-examples/config/sqlite.yaml}
BINARY=${BINARY:-target/release/a2a-warden}

AGENT_PID=""
WARDEN_PID=""

cleanup() {
    echo ""
    echo "Stopping the processes..."
    [ -n "$AGENT_PID" ] && kill "$AGENT_PID" 2>/dev/null || true
    [ -n "$WARDEN_PID" ] && kill "$WARDEN_PID" 2>/dev/null || true
    wait 2>/dev/null || true
}
trap cleanup EXIT

if [ ! -x "$BINARY" ]; then
    echo "The warden binary was not found at ${BINARY}."
    echo "Build it first with: cargo build --release"
    exit 1
fi

echo "============================================================"
echo "  A2A Warden Demo: two agents, one supervised intermediary"
echo "============================================================"
echo ""
echo "  warden:  ${WARDEN_URL}"
echo "  agent:   ${AGENT_URL} (echo-agent)"
echo "  console: ${WARDEN_URL}/ui"
echo ""

# --- Step 1: start the remote agent -----------------------------------------
echo "[1/6] Starting the remote echo agent..."
python3 examples/sample-agents/echo-python/echo_agent.py --port "$AGENT_PORT" &
AGENT_PID=$!
sleep 2

if ! kill -0 "$AGENT_PID" 2>/dev/null; then
    echo "  ERROR: the remote agent exited. Check the port ${AGENT_PORT}."
    exit 1
fi

if ! curl -sf "${AGENT_URL}/.well-known/agent-card.json" >/dev/null; then
    echo "  ERROR: the remote agent is not answering."
    exit 1
fi
echo "  OK  the agent is listening on ${AGENT_URL}"
echo ""

# --- Step 2: start the warden ------------------------------------------------
echo "[2/6] Starting the warden..."
"$BINARY" --config "$CONFIG" &
WARDEN_PID=$!
sleep 3

if ! kill -0 "$WARDEN_PID" 2>/dev/null; then
    echo "  ERROR: the warden exited. Check the configuration and the port."
    exit 1
fi

if ! curl -sf "${WARDEN_URL}/api/v1/admin/health" >/dev/null; then
    echo "  ERROR: the warden is not answering on ${WARDEN_URL}."
    exit 1
fi
echo "  OK  the warden is listening on ${WARDEN_URL}"
echo ""

# --- Step 3: check the registered agents ------------------------------------
echo "[3/6] Checking the agents registered in the warden..."
curl -sf "${WARDEN_URL}/api/v1/admin/agents" | python3 -c "
import json, sys
for agent in json.load(sys.stdin).get('agents', []):
    print('  - %-16s url=%-32s requires_approval=%s' % (
        agent.get('name'), agent.get('url'), agent.get('requiresApproval')))
"
echo ""

# --- Step 4: send a task ----------------------------------------------------
echo "[4/6] Sending a task from the client..."
TASK_RESPONSE=$(curl -sf "${WARDEN_URL}/" \
    -H 'A2A-Version: 1.0' \
    -H 'content-type: application/json' \
    -d "{
          \"jsonrpc\": \"2.0\",
          \"id\": \"1\",
          \"method\": \"message/send\",
          \"params\": {
            \"message\": {
              \"messageId\": \"msg-$(date +%s)\",
              \"role\": \"user\",
              \"parts\": [{\"type\": \"text\", \"text\": \"Hello from the example client\"}],
              \"metadata\": {\"skill\": \"echo\"}
            }
          }
        }")

TASK_ID=$(printf '%s' "$TASK_RESPONSE" | python3 -c "import json,sys; print(json.load(sys.stdin)['result']['id'])")
echo "  OK  task ${TASK_ID} created"
echo ""

# --- Step 5: approve the task ----------------------------------------------
echo "[5/6] Approving the task as the human supervisor..."
APPROVAL_ID=$(curl -sf "${WARDEN_URL}/api/v1/admin/approvals?status=pending" \
    | python3 -c "import json,sys; print(json.load(sys.stdin)['approvals'][0]['id'])")

curl -sf -X POST "${WARDEN_URL}/api/v1/admin/approvals/${APPROVAL_ID}/decision" \
    -H 'content-type: application/json' \
    -d '{"decision": "approve", "approver": "demo-user", "reason": "Automated demo approval"}' >/dev/null
echo "  OK  the task was approved and forwarded"
echo ""

# --- Step 6: inspect the recorded traffic -----------------------------------
echo "[6/6] Inspecting the recorded traffic..."
echo ""
echo "  Timeline:"
curl -sf "${WARDEN_URL}/api/v1/admin/tasks/${TASK_ID}" \
    | python3 -c "
import json, sys
detail = json.load(sys.stdin)
for message in detail.get('messages', []):
    parts = json.loads(message.get('partsJson') or '[]')
    body = '\n'.join(p.get('text', '') for p in parts if p.get('type') == 'text')
    if body:
        print('    [%-8s] %s' % (message.get('role'), body))
print('')
print('  Audit trail:')
for event in detail.get('audit', []):
    print('    %-20s by %-12s at %s' % (event.get('action'), event.get('actor'), event.get('ts')))
"
echo ""

echo "============================================================"
echo "  Demo complete"
echo "============================================================"
echo ""
echo "  Open ${WARDEN_URL}/ui to inspect the traffic in the web console."
echo "  See docs/guide-a2a-communication.md for the full walkthrough."
echo ""
