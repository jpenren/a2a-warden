-- Initial schema for A2A Warden.
--
-- The SQL is intentionally written in the portable subset supported by both
-- SQLite and PostgreSQL: TEXT identifiers and timestamps (RFC 3339),
-- INTEGER booleans (0/1), no backend specific types.
--
-- Migrations are append only: never edit a file that has already been
-- applied, add a new one instead. sqlx verifies the checksum of the applied
-- migrations and refuses to start otherwise.

CREATE TABLE IF NOT EXISTS tasks (
    id            TEXT PRIMARY KEY NOT NULL,
    context_id    TEXT NOT NULL,
    agent_id      TEXT NOT NULL,
    status        TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    metadata_json TEXT,
    remote_task_id TEXT
);

CREATE INDEX IF NOT EXISTS idx_tasks_context ON tasks (context_id);
CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks (status);
CREATE INDEX IF NOT EXISTS idx_tasks_agent ON tasks (agent_id);

CREATE TABLE IF NOT EXISTS messages (
    id         TEXT PRIMARY KEY NOT NULL,
    message_id TEXT NOT NULL,
    task_id    TEXT NOT NULL,
    context_id TEXT NOT NULL,
    role       TEXT NOT NULL,
    parts_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY (task_id) REFERENCES tasks (id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_messages_task ON messages (task_id);
CREATE INDEX IF NOT EXISTS idx_messages_message ON messages (message_id);

CREATE TABLE IF NOT EXISTS approvals (
    id         TEXT PRIMARY KEY NOT NULL,
    task_id    TEXT NOT NULL,
    status     TEXT NOT NULL,
    decided_by TEXT,
    decided_at TEXT,
    expires_at TEXT,
    reason     TEXT,
    created_at TEXT NOT NULL,
    FOREIGN KEY (task_id) REFERENCES tasks (id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_approvals_task ON approvals (task_id);
CREATE INDEX IF NOT EXISTS idx_approvals_status ON approvals (status);

CREATE TABLE IF NOT EXISTS audit_log (
    id           TEXT PRIMARY KEY NOT NULL,
    ts           TEXT NOT NULL,
    actor        TEXT NOT NULL,
    action       TEXT NOT NULL,
    task_id      TEXT,
    context_id   TEXT,
    payload_json TEXT,
    previous_hash TEXT,
    hash         TEXT
);

CREATE INDEX IF NOT EXISTS idx_audit_task ON audit_log (task_id);
CREATE INDEX IF NOT EXISTS idx_audit_ts ON audit_log (ts);

CREATE TABLE IF NOT EXISTS agents (
    id                TEXT PRIMARY KEY NOT NULL,
    name              TEXT NOT NULL UNIQUE,
    url               TEXT NOT NULL,
    skills_json       TEXT NOT NULL,
    card_json         TEXT,
    verified          INTEGER NOT NULL DEFAULT 0,
    verification_error TEXT,
    jwks_kid          TEXT,
    verified_at       TEXT,
    verification_issuer TEXT,
    verification_alg  TEXT,
    quarantined       INTEGER NOT NULL DEFAULT 0,
    created_at        TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS push_notification_configs (
    id          TEXT PRIMARY KEY NOT NULL,
    task_id     TEXT NOT NULL,
    url         TEXT NOT NULL,
    token       TEXT,
    auth_json   TEXT,
    created_at  TEXT NOT NULL,
    FOREIGN KEY (task_id) REFERENCES tasks (id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_push_task ON push_notification_configs (task_id);

CREATE TABLE IF NOT EXISTS artifacts (
    id           TEXT PRIMARY KEY NOT NULL,
    artifact_id  TEXT NOT NULL,
    task_id      TEXT NOT NULL,
    name         TEXT,
    parts_json   TEXT NOT NULL,
    metadata_json TEXT,
    created_at   TEXT NOT NULL,
    FOREIGN KEY (task_id) REFERENCES tasks (id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_artifacts_task ON artifacts (task_id);
