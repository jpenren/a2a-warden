# A2A Warden Cedar Policy Walkthrough

This walkthrough demonstrates the Cedar policy engine in A2A Warden Phase 4.
It starts the warden with Cedar policies, sends a task through the warden,
inspects the policy evaluation, and approves/rejects the task.

Prerequisites: Rust 1.80+ and Python 3.10+. No database server, no Node, and
no other external dependency is required (SQLite mode).

## 1. Build

```bash
git clone https://github.com/jpenren/a2a-warden.git
cd a2a-warden
cargo build --release
```

The binary lands in `target/release/a2a-warden`. The web console is embedded
in it, so nothing else has to be built or installed.

## 2. Start the warden

In a first terminal:

```bash
./target/release/a2a-warden --config config/cedar.yaml
```

```text
INFO a2a_warden_cli: starting a2a-warden listen="127.0.0.1:8080" backend="sqlite" agents=1
INFO a2a_warden_cli: listening on http://127.0.0.1:8080 (console: http://127.0.0.1:8080/ui, admin: http://127.0.0.1:8080/api/v1/admin)
```

The SQLite database is created on the first start and the Cedar policies
are loaded automatically from the configuration.

| Surface                 | URL                                            |
| ----------------------- | ---------------------------------------------- |
| Web console             | <http://127.0.0.1:8080/ui>                     |
| Administration API      | <http://127.0.0.1:8080/api/v1/admin>          |
| Agent Card              | <http://127.0.0.1:8080/.well-known/agent-card.json> |

The default Cedar policy requires approval for all tasks (deny-by-default,
).

## 3. Send a task

```bash
python3 examples/sample-agents/client-python/client_python.py \
    --warden http://127.0.0.1:8080 --text "hello from the demo"
```

```text
task 01a155e8-acb6-76b0-973a-834d945fb9cc created in state input-required
the task is waiting for a human decision
```

The task is paused: the Cedar policy has evaluated the task and determined
it requires approval (escalate effect).

## 5. Approve it as a human supervisor

List the pending approvals:

```bash
curl -s "http://127.0.0.1:8080/api/v1/admin/approvals?status=pending"
```

```json
{
  "approvals": [
    {
      "id": "01a155e8-acc2-73a0-92de-c10fa217d487",
      "taskId": "01a155e8-acb6-76b0-973a-834d945fb9cc",
      "status": "pending",
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
  "approval": {
    "status": "approved",
    "decidedBy": "demo-user",
    "decidedAt": "2026-01-01T10:00:06.507Z",
    "reason": "looks fine"
  },
  "task": {
    "status": {"state": "completed"},
    "status": {
      "message": {
        "parts": [{"type": "text", "text": "echo: hello from the demo"}]
      }
    }
  }
}
```

The Cedar policy engine evaluates to `escalate`, which triggers the approval
inbox. The three possible decisions are `approve`, `reject`, and `cancel`.

## 6. Inspect the recorded traffic

```bash
curl -s http://127.0.0.1:8080/api/v1/admin/tasks | python3 -m json.tool
```

Read the detail of a task to see the full timeline and the audit trail:

```bash
TASK=$(curl -s http://127.0.0.1:8080/api/v1/admin/tasks \
  | python3 -c "import json,sys; print(json.load(sys.stdin)['tasks'][0]['id'])")

curl -s "http://127.0.0.1:8080/api/v1/admin/tasks/$TASK" | python3 -m json.tool
```

The timeline contains the original message, the Cedar policy evaluation,
the human decision with the approver identity, the forwarded message, and
the response of the remote agent:

```text
user  : hello from the demo
agent : Cedar policy evaluation: escalate (require approval)
agent : Human approval granted by demo-user. Reason: looks fine
user  : hello from the demo
agent : echo: hello from the demo
```

The audit trail records every transition and its actor:

```text
message.received by sample-client
policy.evaluated by warden (escalate)
approval.requested by warden
approval.decided by demo-user
task.resumed by demo-user
task.forwarded by demo-user
response.received by warden
```

## 7. Run the acceptance suite

The same flow is automated as an end-to-end test:

```bash
cargo test --workspace
```

```text
running 4 tests
test acceptance_postgres ... ok
test agent_card_is_published ... ok
test console_is_embedded ... ok
test acceptance_sqlite ... ok
```

The PostgreSQL run is skipped unless `DATABASE_URL` points to a PostgreSQL
server:

```bash
export DATABASE_URL="postgresql://a2a_warden:a2a_warden@127.0.0.1:5432/a2a_warden"
cargo test --workspace
```

## 10. Troubleshooting

| Symptom                                  | Cause and fix                                                        |
| ---------------------------------------- | -------------------------------------------------------------------- |
| `failed to bind 127.0.0.1:8080`          | The port is busy. Change `server.listen_addr` or stop the other process. |
| Cedar policy not evaluated                | Ensure `config/cedar.yaml` exists and has valid policy rules.        |
| The task never leaves `input-required`    | No supervisor decided. Open the console or call the decision endpoint. |
