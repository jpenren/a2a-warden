# A2A Warden architecture

This document describes how A2A Warden is put together, the decisions that
shaped it, and the flow of a supervised task.

## 1. Overview

A2A Warden is a hub-and-spoke intermediary. Agents do not talk to each other:
they talk to the warden, and the warden talks to the registered remote agents
on their behalf. Everything that crosses the boundary is recorded, and the
tasks that require supervision pause until a human decides.

```text
                        +---------------------------+
   A2A client  --->     |                           |  --->  remote agent
   (JSON-RPC)           |        A2A Warden         |  (JSON-RPC, outbound)
                        |                           |
   browser     --->     |  console / admin API     |
                        +---------------------------+
                                  |
                            +-----------+
                            |  database |
                            +-----------+
```

Three families of endpoints coexist in a single axum router (decision AD-4):

| Family              | Prefix                | Purpose                                    |
| ------------------- | --------------------- | ------------------------------------------ |
| A2A protocol        | `/`, `/a2a`, `/rpc`   | JSON-RPC binding                  |
| Administration API  | `/api/v1/admin`       | Supervision API consumed by the console    |
| Web console         | `/ui`                 | Embedded SPA                      |

The Agent Card is published at `/.well-known/agent-card.json` (and at the
legacy `/.well-known/agent.json`).

## 2. Crates

The workspace is split by responsibility (decision AD-1):

| Crate                 | Responsibility                                                        |
| --------------------- | --------------------------------------------------------------------- |
| `a2a-warden-core`     | Domain types, storage traits, workflow rules, orchestration service   |
| `a2a-warden-store`    | sqlx implementations of the storage traits, SQLite and PostgreSQL     |
| `a2a-warden-server`   | axum router: A2A endpoints, administration API, embedded console      |
| `a2a-warden-cli`      | The single executable: configuration, wiring, migrations, serving     |
| `a2a-warden-crypto`   | JWS over JCS verification of signed Agent Cards (phase 3)             |
| `a2a-warden-e2e`      | Cross-crate acceptance suite (lives in `tests/`)                       |

The dependency direction is strictly one way: `cli -> server -> store ->
core`. The core crate knows nothing about axum, sqlx, or the concrete
database backend.

## 3. Layering

```text
+-------------------+     +--------------------+     +------------------+
| a2a-warden-core   |     | a2a-warden-store   |     | a2a-warden-server|
+-------------------+     +--------------------+     +------------------+
| protocol          |     | TaskStore          |     | a2a (JSON-RPC)   |
| stores (traits)   |<----| ApprovalStore      |---->| admin (REST)     |
| workflow          |     | AuditStore         |     | ui (embedded)    |
| service           |     | AgentStore         |     | agent_client     |
| config, error     |     +--------------------+     +------------------+
+-------------------+
```

### 3.1 Domain types (`core::protocol`)

The A2A v1.0 wire types used by the Cedar policy engine: `Message`, `Part`, `Task`,
`TaskStatus`, `TaskState`, `Artifact`, and the JSON-RPC envelope. The types
use the specification encoding (camelCase) so the official SDKs can talk to
the warden without a translation layer. Phase 2 validates them against the
official SDKs (decision AD-3).

### 3.2 Storage traits (`core::stores`)

`TaskStore`, `ApprovalStore`, `AuditStore`, and `AgentStore` are defined in
the domain crate and implemented exactly once, in `a2a-warden-store`, on top
of sqlx. No domain code branches on the active backend.

### 3.3 Orchestration (`core::service`)

`WardenService` owns the whole flow. It depends on the `Store` traits and on
an `AgentClient` trait, so the same code runs against SQLite, against
PostgreSQL, and against an in-memory store in the unit tests.

### 3.4 HTTP surface (`a2a-warden-server`)

* `a2a::jsonrpc` dispatches the JSON-RPC methods.
* `admin::*` exposes the supervision API and the token guard.
* `ui::*` serves the embedded SPA with a single route fallback.
* `agent_client::ReqwestAgentClient` is the outbound leg: the warden speaks
  JSON-RPC to the remote agents.

## 4. The supervised flow

```text
client                warden                         remote agent
  |                      |                                 |
  |-- message/send ----->|                                 |
  |                      |-- create task, record message   |
  |                      |-- interception rule?            |
  |                      |   no: forward ---------------->|
  |                      |   yes: pause (input-required)  |
  |<-- task (paused) ----|                                 |
  |                      |   supervisor approves          |
  |                      |-- forward -------------------->|
  |                      |<-- response -------------------|
  |<-- task (completed) -|                                 |
```

Concretely, `WardenService::send_message`:

1. Validates the message (role `user`, at least one part).
2. Resolves the target agent from `message.metadata.targetAgent`, then
   `message.metadata.skill`, then the single registered agent.
3. Creates the task, records the incoming message, and writes an audit entry.
4. Evaluates the interception rule. The Cedar policy engine evaluates the
   configured rules against sender, targetAgent, skill, tenant, and content
   entities. The decision comes from configuration only: a sender
   can never opt out of an interception rule by crafting metadata (deny by
   default, ).
5. When the rule fires, the task moves to `input-required`, a pending approval
   is created, and nothing is forwarded.
6. Otherwise the message is forwarded to the remote agent and the response is
   recorded.

`WardenService::decide_approval` applies the human decision:

* **approve**: the task resumes as `working`, the original message is
  forwarded, and the remote response is recorded. The forwarded message keeps
  the metadata of the original request (the skill, the sender, and the
  routing information), because the message row stores the content of the
  request but not its metadata: the metadata is restored from the task row.
  A task that was held for approval is therefore forwarded exactly as it was
  received.
* **reject**: the task terminates as `rejected`.
* **cancel**: the task terminates as `canceled`.

Every transition is written to `audit_log` with its actor: the sending agent,
the human supervisor, or `warden`.

## 5. Data model

The tables follow the reference architecture.

```text
tasks      (id, context_id, agent_id, status, created_at, updated_at, metadata_json)
messages   (id, message_id, task_id, context_id, role, parts_json, created_at)
artifacts  (id, artifact_id, task_id, name, parts_json, metadata_json, created_at)
approvals  (id, task_id, status, decided_by, decided_at, expires_at, reason, created_at)
audit_log  (id, ts, actor, action, task_id, context_id, payload_json)
agents     (id, name, url, skills_json, requires_approval, card_json, created_at)
```

Two deliberate choices:

* `messages.id` is owned by the warden and `messages.message_id` keeps the
  protocol identifier sent by the peer. Two peers may reuse a message
  identifier in different conversations; the row identity must not collide.
* Every column is `TEXT` and every timestamp is RFC 3339. The same migration
  files run unchanged on SQLite and PostgreSQL, and a test asserts that both
  backends end up with the same schema.

Migrations are append only: a file that has already been applied is never
edited, because sqlx verifies its checksum and refuses to start otherwise
(see ADR 0002).

## 6. Backend interchangeability

The backend is selected exclusively by configuration:

```yaml
database:
  backend: sqlite      # or postgres
```

`a2a_warden_store::connect` returns an `Arc<dyn Store>`: the caller never
knows which backend is behind it. The store test suite runs the same
assertions against both backends, and the acceptance suite in `tests/` has one
run per backend (acceptance criterion CA-Cedar-3: PostgreSQL compatibility).

## 7. Embedded console

The console is a TypeScript SPA built with Vite into `frontend/dist` and
embedded in the binary with `rust-embed`. The execution machine needs no
Node . Because the console is mounted under a configurable prefix,
the served shell carries a `<base href="/ui/">` element so the relative asset
URLs resolve inside the prefix. Unknown paths under the prefix fall back to
the shell, which is what client side routing needs.

## 8. Security posture of the Cedar policy engine

* The administration API is protected by a static token (`admin.token`). When
  no token is configured the API is open and the warden logs a warning on
  every request. Phase 6 replaces this with OIDC.
* The A2A-Version header is mandatory and validated.
* Request bodies are bounded by `server.max_body_size`.
* The default CORS policy allows the origins listed in the configuration; an
  empty list means "allow any" for local experiments.
* Every policy decision defaults to deny: a task is never forwarded without an
  explicit verdict.

## 9. Phase map

| Phase | What lands                                                    |
| ----- | ------------------------------------------------------------- |
| 0     | Repository, toolchain, CI, configuration schema                |
| 1     | Cedar policy engine: interception, human approval, console, admin API (this code) |
| 2     | Streaming, subscriptions, task listing, push notifications     |
| 3     | Signed Agent Card verification (`a2a-warden-crypto`)          |
| 4     | Cedar policy engine replacing the ` Cedar policy engine rule` rule      |
| 5     | Live traffic view, search, forensic replay, hash-chained audit |
| 6     | Multi-tenancy, gRPC and REST bindings, hardening              |
