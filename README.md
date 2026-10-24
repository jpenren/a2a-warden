# A2A Warden

<div dir="ltr">See every message. Verify every agent. Approve every task.</div>

A2A Warden is an open source Rust component that acts as an intermediary (hub-and-spoke) for communication between agents following the [Agent2Agent (A2A)](https://agent2ag.engineering/) v1.0 standard. Agents connect to the warden to exchange information; the warden records all traffic, applies policies to the tasks being executed, verifies signed Agent Cards (JWS over JCS), and enables human supervision and approval from a web interface embedded in the same executable.

## Features and capabilities

- **Agent2Agent (A2A) hub**: JSON-RPC endpoint (`POST /`, `.well-known/agent-card.json`), message replay/resend with correct `message_id`, and full traffic recording.
- **Live traffic feed**: Real-time stream of agent events with full UUID, JSON, and timestamp display; starts only on user action.
- **Audit log (Audit)**: Hash-chained audit trail (`/audit/export`, chain verification), paginated by date descending (25/page), with replay and resend helpers.
- **Task management**: Create, approve/reject/cancel tasks; server-side pagination (`offset`/`limit` 25), filter by status, debounce search (~300ms) with preserved input.
- **Approvals workflow**: Human-in-the-loop decisions (`/approvals`), status filter, pagination (25/page), total counts.
- **Policy engine**: Cedar policy rules (`deny-by-default`), skill-level gating, environment overrides (`A2AWARDEN_*`).
- **Embedded web console**: Tasks, approvals, traffic, audit, search; no separate Node server required.
- **Security**: Signed Agent Cards (JWS over JCS), token-guarded admin API, hash-chain integrity verification.
- **Deployment**: SQLite or PostgreSQL, pinned Rust toolchain, configurable listen address, example agents and pizza-shop demo.

## Why A2A Warden

Agents built with different SDKs, languages, or LLM providers often cannot talk directly: protocols differ, authentication is missing, and an untrusted agent can send arbitrary messages. A2A Warden solves this by acting as a trusted intermediary (hub-and-spoke) where every message is recorded and can be replayed or resent with the correct `message_id`.

Security policies can be enforced through a Cedar policy engine (deny-by-default, skill-level gating) so that only approved tasks reach remote agents. Human supervisors approve or reject tasks via an embedded web console; approvals are paginated and filtered by status with total counts.

Agent Cards are verified with JWS over JCS, ensuring that only signed, authentic agents connect. The platform audits every state transition in a hash-chained log (`/audit/export`) with full UUID/time/JSON, paginated by date descending (25/page) and verifiable for integrity.

Agents written in Python, Rust, or using different LLM backends communicate through a single `a2a-warden` instance published publicly, while the underlying agents remain in private networks (only the warden is exposed). Traffic is displayed live with full UUID and JSON; it only starts when the user clicks Start.

In short: A2A Warden provides interoperability across technologies, policy-controlled execution, human oversight, cryptographic verification, forensic audit, and a private-to-public deployment pattern — all in one executable.

## Quick Start

### Prerequisites

- Rust 1.80+ (the toolchain is pinned in `rust-toolchain.toml`)
- Python 3.10+ for the example agents
- PostgreSQL is optional; SQLite needs no server

### Build and run

```bash
git clone https://github.com/the-cygnusdev/a2a-warden.git
cd a2a-warden
cargo build --release
```

Start the example remote agent in one terminal:

```bash
python3 examples/sample-agents/echo-python/echo_agent.py --port 9001
```

Start the warden in another:

```bash
./target/release/a2a-warden --config examples/config/sqlite.yaml
```

The warden starts on `http://127.0.0.1:8080` with:

- **Embedded web console**: <http://127.0.0.1:8080/ui> - task list, message timeline, approve/reject/cancel
- **Administration API**: <http://127.0.0.1:8080/api/v1/admin>
- **A2A endpoints**: `POST /` (JSON-RPC), `GET /.well-known/agent-card.json`

### Send a task and approve it

```bash
python3 examples/sample-agents/client-python/client_python.py \
    --warden http://127.0.0.1:8080 --text "hello from the demo"
```

```text
task 01a155e8-acb6-76b0-973a-834d945fb9cc created in state input-required
the task is waiting for a human decision
```

The task is paused: nothing has reached the remote agent. Approve it as a supervisor:

```bash
APPROVAL=$(curl -s "http://127.0.0.1:8080/api/v1/admin/approvals?status=pending" \
  | python3 -c "import json,sys; print(json.load(sys.stdin)['approvals'][0]['id'])")

curl -s -X POST "http://127.0.0.1:8080/api/v1/admin/approvals/$APPROVAL/decision" \
  -H 'content-type: application/json' \
  -d '{"decision": "approve", "approver": "demo-user", "reason": "looks fine"}'
```

The warden forwards the task, records the response, and the client obtains the
final answer. The full walkthrough is in [docs/cedar-walkthrough.md](./docs/cedar-walkthrough.md), and the
step-by-step guide is in
[docs/guide-a2a-communication.md](./docs/guide-a2a-communication.md).

### Configuration

`examples/config/sqlite.yaml`:

```yaml
server:
  listen_addr: "0.0.0.0:8080"
database:
  backend: sqlite
  sqlite:
    path: "./a2a-warden.db"
agents:
  - name: "echo-agent"
    url: "http://127.0.0.1:9001"
    skills: ["echo"]
policy:
  rules:
    - id: "escalate-all-skills"
      description: "Escalate all tasks for human approval by default (deny-by-default)"
      active: true
      effect: escalate
      condition: "targetAgent == 'echo-agent'"
      version: 1
      created_at: "2026-01-01T00:00:00Z"
```

Every key can be overridden with an environment variable prefixed with
`A2AWARDEN_` (`A2AWARDEN_SERVER__LISTEN_ADDR=...`). The full reference is in
[docs/configuration.md](./docs/configuration.md).

The interception rule can be applied to a single skill of an agent, instead
of to all of them:

```yaml
agents:
  - name: "payments-agent"
    url: "http://127.0.0.1:9103/"
    skills: ["charge-order", "apply-discount", "refund-order"]
policy:
  rules:
    - id: "escalate-all-skills"
      description: "Escalate all tasks for human approval by default (deny-by-default)"
      active: true
      effect: escalate
      condition: "targetAgent == 'payments-agent' && skill == 'refund-order'"
      version: 1
      created_at: "2026-01-01T00:00:00Z"
```

### Pizza shop demo

A richer scenario lives in [examples/pizza-shop](./examples/pizza-shop): four
agents (order, inventory, payments, delivery) built with the official
`a2a-python` SDK, every inter-agent call routed through the warden, and two
skills held for a human decision.

```bash
cd examples/pizza-shop
python3 -m pip install -r requirements.txt

# The warden, from the root of the repository:
./target/release/a2a-warden --config examples/pizza-shop/warden-pizzeria.yaml

# The four agents, one terminal each:
python3 -m agents.inventory_agent
python3 -m agents.payments_agent
python3 -m agents.delivery_agent
python3 -m agents.order_agent

# The order, which waits for two approvals in the console:
python3 -m client.place_order
```

## Architecture

- **Core** (`a2a-warden-core`): domain types, storage traits, workflow rules, and the orchestration service
- **Store** (`a2a-warden-store`): sqlx implementations over SQLite and PostgreSQL, one set of migrations
- **Server** (`a2a-warden-server`): axum router with the A2A endpoints, the administration API, and the embedded console
- **CLI** (`a2a-warden-cli`): the single executable, configuration loading, migrations on startup
- **Frontend**: TypeScript SPA built with Vite, compiled into the binary
- **Crypto** (`a2a-warden-crypto`): JWS over JCS Agent Card verification (phase 3)

The dependency direction is one way: `cli -> server -> store -> core`. The
core crate knows nothing about axum, sqlx, or the active database backend.

The design is described in [docs/architecture.md](./docs/architecture.md).

## Testing

```bash
cargo test --workspace
```

The suite covers the domain workflow with an in-memory store, the HTTP
surface with the real router, the storage layer against both backends, and
the end to end acceptance suite in `tests/`. The PostgreSQL runs are skipped
unless `DATABASE_URL` points to a PostgreSQL server:

```bash
export DATABASE_URL="postgresql://a2a_warden:a2a_warden@127.0.0.1:5432/a2a_warden"
cargo test --workspace
```

## Documentation

| Document                    | Path                                                          |
| --------------------------- | ------------------------------------------------------------- |
| Practical guide             | [docs/guide-a2a-communication.md](./docs/guide-a2a-communication.md) |
| Architecture                | [docs/architecture.md](./docs/architecture.md)                 |
| Configuration               | [docs/configuration.md](./docs/configuration.md)               |
| Cedar policy walkthrough             | [docs/cedar-walkthrough.md](./docs/cedar-walkthrough.md)                                   |
| Data privacy                | [docs/privacy.md](./docs/privacy.md)                           |

## Examples

| Example                                | Path                                                     |
| -------------------------------------- | -------------------------------------------------------- |
| Two agents with human approval         | [examples/scripts/demo-a2a-flow.sh](./examples/scripts/demo-a2a-flow.sh) |
| The same flow, scripted in Python      | [examples/scripts/demo-a2a-flow.py](./examples/scripts/demo-a2a-flow.py) |
| Echo agent (remote A2A agent)          | [examples/sample-agents/echo-python/](./examples/sample-agents/echo-python/) |
| Sample client                         | [examples/sample-agents/client-python/](./examples/sample-agents/client-python/) |
| Configuration files                   | [examples/config/](./examples/config/)                    |

## Contributing

Please read the [CONTRIBUTING.md](./CONTRIBUTING.md) guide for details on our code of conduct, and the process for submitting pull requests.

## License

This project is licensed under the Apache License, Version 2.0 - see the [LICENSE](./LICENSE) file for details.
