# A2A Warden configuration

The warden reads a single YAML file. Every key can be overridden with an
environment variable, and the configuration is validated at startup with
explicit error messages (decision AD-6).

## 1. Loading the configuration

```bash
a2a-warden --config examples/config/sqlite.yaml
```

Without `--config`, the warden looks for the first file that exists among:

1. `config/default.yaml`
2. `./config/default.yaml`
3. `/etc/a2a-warden/config.yaml`

Other flags:

| Flag              | Effect                                                   |
| ----------------- | -------------------------------------------------------- |
| `--listen ADDR`   | Overrides `server.listen_addr`                          |
| `--check`         | Validates the configuration, prints a summary, and exits |
| `-v`, `-vv`       | Raises the log level to debug or trace                   |

The `RUST_LOG` environment variable also controls the log level
(`RUST_LOG=debug a2a-warden --config ...`).

## 2. Environment overrides

Environment variables use the `A2AWARDEN_` prefix and `__` to separate nested
keys:

```bash
export A2AWARDEN_SERVER__LISTEN_ADDR="0.0.0.0:9000"
export A2AWARDEN_SERVER__MAX_BODY_SIZE=20971520
export A2AWARDEN_DATABASE__BACKEND=postgres
export A2AWARDEN_DATABASE__POSTGRES__URL="postgresql://user:pass@host:5432/db"
export A2AWARDEN_ADMIN__TOKEN="a-long-random-token"
```

Values are parsed as YAML scalars, so numbers and booleans work as expected.

## 3. Reference

### 3.1 `server`

```yaml
server:
  listen_addr: "0.0.0.0:8080"     # host:port of the single listener
  a2a_version: "1.0"               # value of the A2A-Version header
  request_timeout_secs: 30         # timeout of the outbound calls
  max_body_size: 10485760          # maximum request body, in bytes
  cors_enabled: true
  cors_origins:                    # empty list means "allow any origin"
    - "http://localhost:5173"
  public_url: "http://127.0.0.1:8080"  # URL advertised in the Agent Card
```

`public_url` is what clients see in the Agent Card. When it is not set, the
warden advertises `http://127.0.0.1:<port>`.

### 3.2 `database`

Two interchangeable backends. Only `backend` selects which one is active.

```yaml
database:
  backend: sqlite        # sqlite | postgres

  sqlite:
    path: "./a2a-warden.db"
    wal_mode: true
    busy_timeout_ms: 5000

  postgres:
    url: "postgresql://a2a_warden:a2a_warden@127.0.0.1:5432/a2a_warden"
    max_connections: 10
    min_connections: 1
    connect_timeout_secs: 10
```

The options of the inactive backend may be omitted. The migrations are applied
automatically at startup, so a cold start needs no manual step .

### 3.3 `agents`

The registry of remote agents. The warden resolves the target of a task by
name, then by skill, then falls back to the only registered agent.

```yaml
agents:
  - name: "echo-agent"          # unique name
    url: "http://127.0.0.1:9001" # A2A endpoint of the remote agent
    skills: ["echo"]             # skills this agent provides
    # Cedar policy engine rules are configured in `policy.rules`
    card_json: null              # optional Agent Card (phase 3 verification)
    metadata: {}                 # free-form metadata
```

The Cedar policy engine replaces the legacy interception rule. Cedar policy
rules are configured in `policy.rules` with `effect: escalate` to require
human approval for matching tasks. Deny-by-default semantics  mean
that any task not matched by an `allow` or `escalate` rule is denied.

Policy rules are configured entirely in Cedar policy syntax.

```yaml
agents:
  - name: "payments-agent"
    url: "http://127.0.0.1:9103/"
    skills: ["charge-order", "apply-discount", "refund-order"]
    # Per-skill gating (optional)
```

A task that does not name its skill in `message.metadata.skill` is held for
approval when the agent has any gated skill, because the warden cannot prove
it targets a skill that is not gated (deny by default, ).

Rules enforced at startup:

* The name must not be empty and must be unique.
* The URL must use the `http` or `https` scheme.
* Every gated skill must be one of the skills the agent provides.
* At least one agent must be configured.

### 3.4 `retention`

```yaml
retention:
  audit_days: 90           # retention of the audit log
  task_days: 30            # retention of the tasks
  auto_purge: false        # scheduled purging (phase 5)
  purge_interval_hours: 24
```

### 3.5 `frontend`

```yaml
frontend:
  enabled: true
  path: "/ui"                 # prefix of the console
  spa_fallback: "/ui/index.html"
```

### 3.6 `admin`

```yaml
admin:
  enabled: true
  path: "/api/v1/admin"
  token: null                 # static token; null means "no authentication"
  token_header: "X-Admin-Token"
```

When `token` is `null`, the administration API is open and the warden logs a
warning on every request. Set a token for anything that is not a local
experiment:

```yaml
admin:
  token: "${A2AWARDEN_ADMIN__TOKEN}"   # not expanded: use the environment
```

The environment variable override is the supported way to inject a secret:

```bash
export A2AWARDEN_ADMIN__TOKEN="$(openssl rand -hex 32)"
a2a-warden --config examples/config/sqlite.yaml
```

## 4. Validation

The configuration is validated when it is loaded and again when the warden
starts. A failure aborts the start with a message that names the offending
key:

```text
a2a-warden: Configuration error: Validation failed: Duplicate agent name: echo-agent
```

Unknown keys are rejected: the schema uses `deny_unknown_fields`, so a typo
never silently disables a setting.

## 5. Checking a configuration

```bash
a2a-warden --config examples/config/sqlite.yaml --check
```

```text
configuration is valid
  listen address : 127.0.0.1:8080
  database       : sqlite
  agent          : echo-agent (1 skills, Cedar policy engine enabled)
```

## 6. Command line interface

```text
a2a-warden 0.1.0
A2A Warden: see every message, verify every agent, approve every task

Usage: a2a-warden [OPTIONS]

Options:
  -c, --config <FILE>  Path to the YAML configuration file
      --listen <ADDR>  Override the listen address (`host:port`)
      --check          Print the effective configuration and exit
  -v, --verbose...     Increase log verbosity (repeatable)
  -h, --help           Print help
  -V, --version        Print version
```
