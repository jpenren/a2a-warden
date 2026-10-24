# Practical Guide: A2A Communication with A2A Warden

This guide walks through communicating two agents through A2A Warden, and
through inspecting the recorded traffic in the web console and in the
administration API.

Everything in this guide works with the current build (Phase 4: Cedar policy engine) The
commands are copy-paste ready.

## Prerequisites

- Rust 1.80+ (the toolchain is pinned in `rust-toolchain.toml`)
- Python 3.10+ for the example agents
- `curl` and `jq` for the API examples
- SQLite (default) or PostgreSQL; SQLite needs no server

---

## Step 1: Build the project

```bash
git clone https://github.com/jpenren/a2a-warden.git
cd a2a-warden
cargo build --release
```

The binary lands in `target/release/a2a-warden`. The web console is embedded
in it, so nothing else has to be built or installed.

---

## Step 2: Start the remote agent

The echo agent is a remote A2A agent that answers every task with the text it
received, prefixed with `echo:`.

```bash
python3 examples/sample-agents/echo-python/echo_agent.py --port 9001
```

```text
2026-01-01 10:00:00,000 INFO echo-agent listening on http://127.0.0.1:9001
```

It publishes its Agent Card at
`http://127.0.0.1:9001/.well-known/agent-card.json`.

---

## Step 3: Start the warden

In a second terminal:

```bash
./target/release/a2a-warden --config examples/config/sqlite.yaml
```

```text
INFO a2a_warden_cli: starting a2a-warden listen="127.0.0.1:8080" backend="sqlite" agents=1
INFO a2a_warden_cli: listening on http://127.0.0.1:8080 (console: http://127.0.0.1:8080/ui, admin: http://127.0.0.1:8080/api/v1/admin)
```

The SQLite database is created on the first start and the migrations are
applied automatically.

| Surface | URL |
| --- | --- |
| Web console | <http://127.0.0.1:8080/ui> |
| Administration API | <http://127.0.0.1:8080/api/v1/admin> |
| Agent Card | <http://127.0.0.1:8080/.well-known/agent-card.json> |
| Health | <http://127.0.0.1:8080/api/v1/admin/health> |

### How the warden knows about the agents

The remote agents are declared in the configuration file, not through the
API. The warden seeds its registry from that file at startup:

```yaml
# examples/config/sqlite.yaml
agents:
  - name: "echo-agent"
    url: "http://127.0.0.1:9001"
    skills: ["echo"]
    # Cedar policy engine interception rule: tasks for this agent pause in
    # `input-required` until a human decides.
    # Cedar policy engine rule
```

Check what the warden registered:

```bash
curl -s "http://127.0.0.1:8080/api/v1/admin/agents" | jq '.agents'
```

```json
{
  "agents": [
    {
      "id": "9f6c...",
      "name": "echo-agent",
      "url": "http://127.0.0.1:9001",
      "skillsJson": "[\"echo\"]",
      "requiresApproval": true,
      "createdAt": "2026-01-01T10:00:00.000Z"
    }
  ]
}
```

---

## Step 4: Send a task

```bash
python3 examples/sample-agents/client-python/client_python.py \
    --warden http://127.0.0.1:8080 --text "hello from the demo"
```

```text
task 01a155e8-acb6-76b0-973a-834d945fb9cc created in state input-required
the task is waiting for a human decision
```

The task is paused: the warden has not forwarded anything to the echo agent.
This is the interception rule of the configuration (`# Cedar policy engine rule`).

You can also send it by hand:

```bash
curl -s http://127.0.0.1:8080/ \
  -H 'A2A-Version: 1.0' \
  -H 'content-type: application/json' \
  -d '{
        "jsonrpc": "2.0",
        "id": "1",
        "method": "message/send",
        "params": {
          "message": {
            "messageId": "message-1",
            "role": "user",
            "parts": [{"type": "text", "text": "hello from the demo"}],
            "metadata": {"skill": "echo", "senderAgent": "sample-client"}
          }
        }
      }'
```

The response carries the task:

```json
{
  "jsonrpc": "2.0",
  "id": "1",
  "result": {
    "id": "01a155e8-acb6-76b0-973a-834d945fb9cc",
    "contextId": "01a155e8-c0a1-7d2e-9f55-1b3c0f1a2b3c",
    "status": {
      "state": "input-required",
      "message": {
        "messageId": "msg-4f1c...",
        "role": "agent",
        "parts": [
          {
            "type": "text",
            "text": "Human approval required before this task is forwarded to 'echo-agent'."
          }
        ]
      },
      "timestamp": "2026-01-01T10:00:04.365Z"
    }
  }
}
```

### How the warden routes a task

The warden picks the target agent in this order:

1. `message.metadata.targetAgent` (or `metadata.agent`, or `metadata.target`)
2. `message.metadata.skill`, when exactly one registered agent provides it
3. the only registered agent, when there is exactly one

When several agents are registered, always set `targetAgent` or `skill`.

---

## Step 5: Approve the task as a human supervisor

### From the web console

Open <http://127.0.0.1:8080/ui>:

- the **Tasks** tab lists the task in state `input-required`
- the **Approvals** tab lists the pending decision
- the task detail shows the message timeline and the approve, reject, and
  cancel buttons

### From the API

```bash
curl -s "http://127.0.0.1:8080/api/v1/admin/approvals?status=pending" | jq
```

```json
{
  "approvals": [
    {
      "id": "01a155e8-acc2-73a0-92de-c10fa217d487",
      "taskId": "01a155e8-acb6-76b0-973a-834d945fb9cc",
      "status": "pending",
      "decidedBy": null,
      "decidedAt": null,
      "expiresAt": null,
      "reason": null,
      "createdAt": "2026-01-01T10:00:04.365Z"
    }
  ]
}
```

Resolve it:

```bash
curl -s -X POST \
  "http://127.0.0.1:8080/api/v1/admin/approvals/01a155e8-acc2-73a0-92de-c10fa217d487/decision" \
  -H 'content-type: application/json' \
  -d '{"decision": "approve", "approver": "demo-user", "reason": "looks fine"}'
```

The response carries the resolved approval and the final task:

```json
{
  "task": {
    "id": "01a155e8-acb6-76b0-973a-834d945fb9cc",
    "status": {"state": "completed", "message": {"parts": [{"type": "text", "text": "echo: hello from the demo"}]}}
  },
  "approval": {
    "id": "01a155e8-acc2-73a0-92de-c10fa217d487",
    "taskId": "01a155e8-acb6-76b0-973a-834d945fb9cc",
    "status": "approved",
    "decidedBy": "demo-user",
    "decidedAt": "2026-01-01T10:00:06.507Z",
    "reason": "looks fine"
  }
}
```

The three possible decisions are `approve`, `reject`, and `cancel`. A
rejection terminates the task as `rejected` and the remote agent is never
contacted. A cancellation terminates it as `canceled`.

If the administration API is protected with a token
(`admin.token` in the configuration), add the header to every call:

```bash
curl -s -H 'X-Admin-Token: secret' \
  "http://127.0.0.1:8080/api/v1/admin/approvals?status=pending"
```

---

## Step 6: Inspect the recorded traffic

### All the tasks

```bash
curl -s "http://127.0.0.1:8080/api/v1/admin/tasks" \
  | jq '.tasks[] | {id, agentId, status, contextId, createdAt}'
```

```json
{
  "id": "01a155e8-acb6-76b0-973a-834d945fb9cc",
  "agentId": "echo-agent",
  "status": "completed",
  "contextId": "01a155e8-c0a1-7d2e-9f55-1b3c0f1a2b3c",
  "createdAt": "2026-01-01T10:00:04.365Z"
}
```

The list accepts the filters `status`, `agent`, `context`, and `limit`:

```bash
curl -s "http://127.0.0.1:8080/api/v1/admin/tasks?agent=echo-agent&status=completed"
```

### The detail of a task: timeline, approval, and audit trail

```bash
TASK=$(curl -s "http://127.0.0.1:8080/api/v1/admin/tasks" \
  | jq -r '.tasks[0].id')

curl -s "http://127.0.0.1:8080/api/v1/admin/tasks/$TASK" | jq '.task, .approval'
```

The detail response has four sections: `task`, `messages`, `artifacts`, and
`audit`.

The messages are the timeline. Their content is stored as a JSON string in
`partsJson`:

```bash
curl -s "http://127.0.0.1:8080/api/v1/admin/tasks/$TASK" \
  | jq -r '.messages[] | "\(.role): " + ((.partsJson | fromjson)
      | map(select(.type == "text") | .text) | join(" "))'
```

```text
user: hello from the demo
agent: Human approval required before this task is forwarded to 'echo-agent'.
agent: Human approval granted by demo-user. Reason: looks fine
user: hello from the demo
agent: echo: hello from the demo
```

The audit trail records every transition and its actor:

```bash
curl -s "http://127.0.0.1:8080/api/v1/admin/tasks/$TASK" \
  | jq -r '.audit[] | "\(.action) by \(.actor)"'
```

```text
message.received by sample-client
approval.requested by warden
approval.decided by demo-user
task.resumed by demo-user
task.forwarded by demo-user
response.received by warden
```

---

## Step 7: The flow inside the warden

```
┌─────────────────────────────────────────────────────────────┐
│ 1. The client sends a message to the warden                │
│    - method: message/send                                  │
│    - role: user                                            │
│    - metadata.skill / metadata.targetAgent                 │
├─────────────────────────────────────────────────────────────┤
│ 2. The warden records the task                             │
│    - creates the task row                                  │
│    - appends the client message to the timeline            │
│    - writes the message.received audit entry               │
├─────────────────────────────────────────────────────────────┤
│ 3. The interception rule applies                           │
│    - Cedar policy engine rule -> input-required + approval │
│    - # Cedar policy engine: false -> straight to step 4         │
├─────────────────────────────────────────────────────────────┤
│ 4. A human supervisor decides                              │
│    - POST /api/v1/admin/approvals/{id}/decision            │
│    - approve: forward the message                          │
│    - reject:  terminate as rejected, the agent is skipped  │
│    - cancel:  terminate as canceled                        │
├─────────────────────────────────────────────────────────────┤
│ 5. The warden forwards the message to the remote agent     │
│    - POST {agent.url} with the A2A-Version header          │
│    - the warden acts as an A2A client                      │
├─────────────────────────────────────────────────────────────┤
│ 6. The warden records the answer                           │
│    - appends the agent message to the timeline             │
│    - writes the response.received audit entry              │
│    - moves the task to completed / failed                  │
└─────────────────────────────────────────────────────────────┘
```

```
┌─────────────┐     ┌─────────────────┐     ┌─────────────┐
│   Client    │────▶│     Warden      │────▶│ Echo agent  │
│  (A2A)      │     │  (intermediary) │     │   (A2A)     │
└─────────────┘     └─────────────────┘     └─────────────┘
      │                    │                       │
      │ 1. message/send    │                       │
      │───────────────────▶│                       │
      │                    │ 2. record the task   │
      │                    │ 3. hold for approval │
      │                    │                       │
      │                    │ 4. human decision    │
      │                    │ 5. forward           │
      │                    │──────────────────────▶│
      │                    │                       │ 6. answer
      │                    │◀──────────────────────│
      │                    │ 7. record the answer │
      ▼                    ▼                       ▼
   [User]           [Supervisor]              [Assistant]
```

---

## Step 8: Run it with the helper scripts

The same flow is automated by two scripts:

```bash
# Starts the agent and the warden, sends a task, approves it, and prints
# the recorded timeline and audit trail.
bash examples/scripts/demo-a2a-flow.sh
```

```bash
# The same flow from Python, for the cases where you want to script it.
python3 examples/scripts/demo-a2a-flow.py
```

Both scripts expect the warden binary in `target/release/a2a-warden`.

---

## Step 9: Run the acceptance suite

```bash
cargo test --workspace
```

The suite runs the same flow over SQLite and over PostgreSQL, plus the unit
tests of every crate.

---

## Useful commands

```bash
# Health and build information
curl -s "http://127.0.0.1:8080/api/v1/admin/health" | jq

# Registered agents
curl -s "http://127.0.0.1:8080/api/v1/admin/agents" \
  | jq '.agents[] | {name, url, requiresApproval}'

# Tasks, with the state filter
curl -s "http://127.0.0.1:8080/api/v1/admin/tasks?status=input-required" | jq

# Pending approvals
curl -s "http://127.0.0.1:8080/api/v1/admin/approvals?status=pending" | jq

# Read a task over the A2A endpoint
curl -s http://127.0.0.1:8080/ \
  -H 'A2A-Version: 1.0' -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":"1","method":"tasks/get","params":{"id":"'"$TASK"'"}}' | jq

# Cancel a task
curl -s -X POST http://127.0.0.1:8080/ \
  -H 'A2A-Version: 1.0' -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":"2","method":"tasks/cancel","params":{"id":"'"$TASK"'"}}' | jq
```

---

## Troubleshooting

### The warden refuses to start with a configuration error

The configuration schema is strict: an unknown key is an error, not a
warning. Run the warden with the example configuration to compare, and see
[docs/configuration.md](./configuration.md) for the full reference.

### The task stays in `input-required`

Nobody approved it yet. List the pending approvals and decide:

```bash
curl -s "http://127.0.0.1:8080/api/v1/admin/approvals?status=pending" | jq
```

### The warden reports "several agents are registered"

Set `message.metadata.targetAgent` or `message.metadata.skill` in the
message, or use the `--target-agent` and `--skill` flags of the sample
client.

### "Skill 'x' is provided by more than one agent"

Two agents advertise the same skill. Disambiguate with
`message.metadata.targetAgent`.

### The database is locked

```bash
pkill -f a2a-warden
rm -f a2a-warden.db-shm a2a-warden.db-wal
./target/release/a2a-warden --config examples/config/sqlite.yaml
```

---

## Next steps

- [Cedar policy walkthrough](/cedar-walkthrough.md): the shortest path to the demo
- [Configuration reference](./configuration.md)
- [Architecture](./architecture.md)

## Resources

- [README](../README.md)
- [Examples](../examples/)
- [Tests](../tests/)
