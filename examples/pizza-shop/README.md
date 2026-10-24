# Pizza shop demo agents for A2A Warden

Four agents (order, inventory, payments, delivery) that place pizza orders
while A2A Warden supervises every message and holds approval-gated tasks
for human decision.

```text
                 +----------------------+
   customer  --> |  order-agent :9101   |  skill: place-order (orchestrator)
                 +----------+-----------+
                            |  all calls go through the warden
                            v
                 +----------------------+
                 |  A2A WARDEN  :8080   |  records everything, gates skills by policy,
                 |  supervision hub     |  holds input-required tasks for human approval
                 +----+-------+--------+
                      |       |        \
        +-------------+       +----+    +-------------+
        v                          v                   v
+----------------+   +------------------+   +--------------------+
| inventory-agent|   | payments-agent   |   | delivery-agent     |
| :9102          |   | :9103            |   | :9104              |
| check-ingred.  |   | charge-order     |   | assign-delivery    |
|                |   | apply-discount*  |   | zone-exception*    |
|                |   | refund-order*    |   |                    |
+----------------+   +------------------+   +--------------------+
     (* approval required by warden policy: task is held in input-required
      until a human approves it in the console at /ui)
```

The agents are built with the official [`a2a-python`](https://github.com/a2aproject/a2a-python)
SDK and speak the A2A v1.0 JSON-RPC binding. Every inter-agent call goes
through the warden, never peer-to-peer, so the warden records all the
traffic and applies the policy before anything is forwarded.

## Requirements

- Python 3.10+
- A built `a2a-warden` binary (see the [README](../../README.md) of the
  repository)

## Run

```bash
cd examples/pizza-shop
python3 -m pip install -r requirements.txt
```

1. Start the warden with the pizza configuration (from the root of the
   repository):

   ```bash
   cargo build --release
   ./target/release/a2a-warden --config examples/pizza-shop/warden-pizzeria.yaml
   ```

2. Start the four agents (one terminal each, from `examples/pizza-shop`):

   ```bash
   python3 -m agents.inventory_agent
   python3 -m agents.payments_agent
   python3 -m agents.delivery_agent
   python3 -m agents.order_agent
   ```

3. Open the warden console: <http://127.0.0.1:8080/ui>

4. Place an order (it needs two approvals in the console while it runs):

   ```bash
   python3 -m client.place_order
   ```

5. In a second terminal, trigger a refund and approve it in the console:

   ```bash
   python3 -m client.refund_order
   ```

The order of the sample client exercises both gates: the `SAVE25` coupon
gates `apply-discount`, and the `out-of-town` address gates
`zone-exception`. The client waits (and says so) while the task sits in
`input-required`.

## What to observe in the warden console

- The order timeline groups every sub-task (inventory, payments, delivery)
  under the order context, with the full content of each message.
- Tasks addressed to `apply-discount`, `refund-order`, and `zone-exception`
  appear in the approvals inbox in input-required state; nothing has been
  forwarded to the target agent until a human approves.
- Every decision (who approved, when, over which content) is recorded in the
  audit log alongside the messages.
- The agents view lists the gated skills of every agent.

## Layout

```text
examples/pizza-shop/
├── README.md
├── requirements.txt
├── warden-pizzeria.yaml      # the warden configuration of the scenario
├── common/
│   ├── __init__.py
│   ├── a2a_client.py         # minimal A2A v1.0 JSON-RPC client toward the warden
│   ├── domain.py             # shared pizza-shop domain model
│   └── server.py             # helpers to expose an agent with the official SDK
├── agents/
│   ├── __init__.py
│   ├── order_agent.py        # orchestrator
│   ├── inventory_agent.py
│   ├── payments_agent.py
│   └── delivery_agent.py
└── client/
    ├── __init__.py
    ├── place_order.py        # simulates the front-of-house / conversation agent
    └── refund_order.py       # triggers a refund to demo the approval inbox
```

## Configuration

The policy lives in the warden configuration, not in the agents:

```yaml
agents:
  - name: payments-agent
    url: "http://127.0.0.1:9103/"
    skills: ["charge-order", "apply-discount", "refund-order"]
    require_approval_skills: ["apply-discount", "refund-order"]
```

`require_approval_skills` gates the listed skills of the agent; the other
skills of the same agent are forwarded without a human decision. The
boolean `require_approval` still gates every skill of an agent.

A task that does not name its skill is held for approval, because the warden
cannot prove it targets a skill that is not gated (deny by default).

## Notes on the wire format

- The agents answer the A2A v1.0 method names (`SendMessage`, `GetTask`,
  `CancelTask`). The warden prefers those and falls back to the v0.3 names
  (`message/send`, …) for agents of the older vintage.
- The SDK is built on protobuf, so it spells the enums with their name
  (`ROLE_USER`, `TASK_STATE_COMPLETED`) where the A2A v1.0 JSON binding uses
  the lower case form (`user`, `completed`). The warden accepts both, and
  the small middleware in `common/server.py` adapts the role of the
  incoming messages for the agents.
- The parts of a message are tagged with their kind in the A2A v1.0 JSON
  binding (`{"type": "text", "text": ...}`); the SDK does not tag them. The
  warden accepts both forms.
- The demo keeps every payload in a text part holding JSON, so the whole
  conversation is readable in the console without a schema negotiation.
- The warden holds the request open until the whole workflow is over, which
  includes the time a human takes to decide. The clients therefore use a
  generous timeout (`A2AWARDEN_TIMEOUT`, 900 seconds by default).

## Integration notes for the a2a-warden repository

- The demo doubles as a richer acceptance scenario beyond the initial demo
  agent: the E2E suite can drive `place_order.py` and assert that (a) the
  discount task is held in input-required with no traffic to
  `payments-agent`, (b) after admin API approval the order completes, and
  (c) the warden timeline contains every sub-task under one context.
- The per-skill gating in `warden-pizzeria.yaml` (`require_approval_skills`)
  is a minimal extension of the production-level `require_approval` boolean in the
  project plan; the Cedar policy engine in phase 4 replaces both.
- Routing metadata (`skill`, `targetAgent`) is the demo's convention; the
  warden can alternatively resolve agents purely by skill from its registry.
