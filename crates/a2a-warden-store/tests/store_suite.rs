//! The same store test suite runs against every configured backend, so
//! SQLite and PostgreSQL are validated with identical expectations
//! (acceptance criterion CA-legacy-3).

use std::sync::Arc;

use a2a_warden_core::config::{PostgresConfig, SqliteConfig};
use a2a_warden_core::error::Error;
use a2a_warden_core::protocol::{Role, TaskState};
use a2a_warden_core::stores::{
    AgentRecord, ApprovalRecord, ApprovalStatus, ArtifactRecord, AuditRecord, MessageRecord,
    PushNotificationRecord, Store, TaskCursor, TaskFilter, TaskRecord,
};
use a2a_warden_store::WardenStore;
use sqlx::{Pool, Row};

const CLEANUP: [&str; 7] = [
    "DELETE FROM push_notification_configs",
    "DELETE FROM messages",
    "DELETE FROM artifacts",
    "DELETE FROM approvals",
    "DELETE FROM audit_log",
    "DELETE FROM tasks",
    "DELETE FROM agents",
];

/// Removes every row so the suite can run against a shared database.
macro_rules! define_clean {
    ($name:ident, $db:ty) => {
        async fn $name(pool: &Pool<$db>) {
            for statement in CLEANUP {
                sqlx::query(statement)
                    .execute(pool)
                    .await
                    .unwrap_or_else(|err| panic!("cleanup statement '{statement}' failed: {err}"));
            }
        }
    };
}

define_clean!(clean_sqlite, sqlx::Sqlite);
define_clean!(clean_postgres, sqlx::Postgres);

fn sample_task(id: &str, created_at: &str) -> TaskRecord {
    TaskRecord {
        id: id.to_owned(),
        context_id: "ctx-1".to_owned(),
        agent_id: "echo-agent".to_owned(),
        status: TaskState::Submitted,
        created_at: created_at.to_owned(),
        updated_at: created_at.to_owned(),
        metadata_json: Some(r#"{"senderAgent":"client-1"}"#.to_owned()),
        remote_task_id: Some("remote-1".to_owned()),
    }
}

fn sample_message(id: &str, task_id: &str, created_at: &str) -> MessageRecord {
    MessageRecord {
        id: id.to_owned(),
        message_id: format!("protocol-{id}"),
        task_id: task_id.to_owned(),
        context_id: "ctx-1".to_owned(),
        role: Role::User,
        parts_json: r#"[{"type":"text","text":"hello"}]"#.to_owned(),
        created_at: created_at.to_owned(),
    }
}

fn sample_agent(name: &str) -> AgentRecord {
    AgentRecord {
        id: format!("id-{name}"),
        name: name.to_owned(),
        url: "http://127.0.0.1:9001".to_owned(),
        skills_json: r#"["echo"]"#.to_owned(),
        card_json: None,
        verified: false,
        verification_error: None,
        jwks_kid: None,
        verified_at: None,
        verification_issuer: None,
        verification_alg: None,
        quarantined: false,
        created_at: "2026-01-01T00:00:00.000Z".to_owned(),
    }
}

async fn run_suite(store: Arc<dyn Store>) {
    task_roundtrip(&store).await;
    task_errors(&store).await;
    task_filters(&store).await;
    messages_are_ordered(&store).await;
    artifacts_are_listed(&store).await;
    approval_lifecycle(&store).await;
    audit_is_ordered(&store).await;
    agent_upsert_is_idempotent(&store).await;
    tasks_are_paginated(&store).await;
    expired_tasks_are_listed(&store).await;
    push_configs_roundtrip(&store).await;
}

async fn task_roundtrip(store: &Arc<dyn Store>) {
    let task = sample_task("task-1", "2026-01-01T00:00:00.000Z");
    store.create_task(&task).await.expect("create task");

    let loaded = store.get_task("task-1").await.expect("get task");
    assert_eq!(loaded, task);

    store
        .update_task_status("task-1", TaskState::InputRequired, "2026-01-01T00:00:01.000Z")
        .await
        .expect("update status");
    let loaded = store.get_task("task-1").await.expect("get task");
    assert_eq!(loaded.status, TaskState::InputRequired);
    assert_eq!(loaded.updated_at, "2026-01-01T00:00:01.000Z");
}

async fn task_errors(store: &Arc<dyn Store>) {
    let duplicate = store
        .create_task(&sample_task("task-1", "2026-01-01T00:00:00.000Z"))
        .await
        .expect_err("duplicate task id must be rejected");
    assert!(duplicate.to_string().contains("Duplicate"), "{duplicate}");

    let missing = store.get_task("nope").await.expect_err("missing task");
    assert!(missing.to_string().contains("not found"), "{missing}");

    let missing_update = store
        .update_task_status("nope", TaskState::Working, "2026-01-01T00:00:00.000Z")
        .await
        .expect_err("missing task update");
    assert!(missing_update.to_string().contains("not found"));
}

async fn task_filters(store: &Arc<dyn Store>) {
    let mut other = sample_task("task-2", "2026-01-02T00:00:00.000Z");
    other.agent_id = "other-agent".to_owned();
    other.context_id = "ctx-2".to_owned();
    store.create_task(&other).await.expect("create task");

    let everything = store.list_tasks(&TaskFilter::default()).await.expect("list");
    assert_eq!(everything.len(), 2, "no filter returns every task");
    assert_eq!(everything[0].id, "task-2", "newest first");

    let by_status = store
        .list_tasks(&TaskFilter { status: Some(TaskState::InputRequired), ..TaskFilter::default() })
        .await
        .expect("list");
    assert_eq!(by_status.len(), 1);
    assert_eq!(by_status[0].id, "task-1");

    let by_agent = store
        .list_tasks(&TaskFilter { agent: Some("other-agent".to_owned()), ..TaskFilter::default() })
        .await
        .expect("list");
    assert_eq!(by_agent.len(), 1);
    assert_eq!(by_agent[0].id, "task-2");

    let by_context = store
        .list_tasks(&TaskFilter { context_id: Some("ctx-2".to_owned()), ..TaskFilter::default() })
        .await
        .expect("list");
    assert_eq!(by_context.len(), 1);

    let limited = store
        .list_tasks(&TaskFilter { limit: Some(1), ..TaskFilter::default() })
        .await
        .expect("list");
    assert_eq!(limited.len(), 1);

    let by_state = store
        .list_tasks(&TaskFilter { status: Some(TaskState::Working), ..TaskFilter::default() })
        .await
        .expect("list");
    assert!(by_state.is_empty(), "unknown state returns no rows");
}

async fn messages_are_ordered(store: &Arc<dyn Store>) {
    store
        .append_message(&sample_message("m1", "task-1", "2026-01-01T00:00:00.000Z"))
        .await
        .expect("append m1");
    store
        .append_message(&sample_message("m2", "task-1", "2026-01-01T00:00:01.000Z"))
        .await
        .expect("append m2");

    let messages = store.list_messages("task-1").await.expect("list messages");
    let ids: Vec<&str> = messages.iter().map(|message| message.id.as_str()).collect();
    assert_eq!(ids, ["m1", "m2"]);
    assert_eq!(messages[0].message_id, "protocol-m1");

    let none = store.list_messages("other").await.expect("list messages");
    assert!(none.is_empty());
}

async fn artifacts_are_listed(store: &Arc<dyn Store>) {
    store
        .append_artifact(&ArtifactRecord {
            id: "artifact-row-1".to_owned(),
            artifact_id: "artifact-1".to_owned(),
            task_id: "task-1".to_owned(),
            name: Some("result".to_owned()),
            parts_json: r#"[{"type":"text","text":"the answer"}]"#.to_owned(),
            metadata_json: Some(r#"{"kind":"final"}"#.to_owned()),
            created_at: "2026-01-01T00:00:05.000Z".to_owned(),
        })
        .await
        .expect("append artifact");

    let artifacts = store.list_artifacts("task-1").await.expect("list artifacts");
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].artifact_id, "artifact-1");
    assert_eq!(artifacts[0].name.as_deref(), Some("result"));
    assert!(artifacts[0].parts_json.contains("the answer"));

    let none = store.list_artifacts("other").await.expect("list artifacts");
    assert!(none.is_empty());
}

async fn approval_lifecycle(store: &Arc<dyn Store>) {
    let approval = ApprovalRecord {
        id: "appr-1".to_owned(),
        task_id: "task-1".to_owned(),
        status: ApprovalStatus::Pending,
        decided_by: None,
        decided_at: None,
        expires_at: None,
        reason: None,
        created_at: "2026-01-01T00:00:02.000Z".to_owned(),
    };
    store.create_approval(&approval).await.expect("create");

    let pending = store.pending_approval_for_task("task-1").await.expect("pending lookup");
    assert_eq!(pending.map(|approval| approval.id), Some("appr-1".to_owned()));

    let decided = store
        .decide_approval(
            "appr-1",
            ApprovalStatus::Approved,
            "alice",
            "2026-01-01T00:00:03.000Z",
            Some("looks good"),
        )
        .await
        .expect("decide");
    assert_eq!(decided.status, ApprovalStatus::Approved);
    assert_eq!(decided.decided_by.as_deref(), Some("alice"));
    assert_eq!(decided.reason.as_deref(), Some("looks good"));

    let second = store
        .decide_approval(
            "appr-1",
            ApprovalStatus::Rejected,
            "bob",
            "2026-01-01T00:00:04.000Z",
            None,
        )
        .await
        .expect_err("second decision must be rejected");
    assert!(second.to_string().contains("already"), "{second}");

    let missing = store.get_approval("nope").await.expect_err("missing approval");
    assert!(missing.to_string().contains("not found"));

    let pending_left = store.pending_approval_for_task("task-1").await.expect("pending lookup");
    assert!(pending_left.is_none(), "decided approval is no longer pending");

    let approved = store.list_approvals(Some(ApprovalStatus::Approved)).await.expect("list");
    assert_eq!(approved.len(), 1);
    let all = store.list_approvals(None).await.expect("list");
    assert_eq!(all.len(), 1);
}

async fn audit_is_ordered(store: &Arc<dyn Store>) {
    for (index, ts) in
        ["2026-01-01T00:00:00.000Z", "2026-01-01T00:00:01.000Z"].into_iter().enumerate()
    {
        store
            .append_audit(&AuditRecord {
                id: format!("audit-{index}"),
                ts: ts.to_owned(),
                actor: "warden".to_owned(),
                action: "message.received".to_owned(),
                task_id: Some("task-1".to_owned()),
                context_id: Some("ctx-1".to_owned()),
                payload_json: Some(r#"{"n":1}"#.to_owned()),
                previous_hash: None,
                hash: None,
            })
            .await
            .expect("append audit");
    }

    let entries = store.list_audit_for_task("task-1").await.expect("list audit");
    let ids: Vec<&str> = entries.iter().map(|entry| entry.id.as_str()).collect();
    assert_eq!(ids, ["audit-0", "audit-1"]);
    assert_eq!(entries[0].payload_json.as_deref(), Some(r#"{"n":1}"#));

    let none = store.list_audit_for_task("other").await.expect("list audit");
    assert!(none.is_empty());
}

async fn agent_upsert_is_idempotent(store: &Arc<dyn Store>) {
    store.upsert_agent(&sample_agent("echo-agent")).await.expect("insert");
    let first = store.get_agent("echo-agent").await.expect("get agent");
    assert!(!first.verified);
    assert!(!first.quarantined);
    assert!(first.verification_error.is_none());

    let mut updated = sample_agent("echo-agent");
    updated.url = "http://127.0.0.1:9002".to_owned();
    updated.verified = true;
    updated.verification_error = None;
    updated.jwks_kid = Some("key-1".to_owned());
    updated.verified_at = Some("2026-01-01T00:00:05.000Z".to_owned());
    updated.verification_issuer = Some("https://issuer.example".to_owned());
    updated.verification_alg = Some("ES256".to_owned());
    updated.quarantined = false;
    store.upsert_agent(&updated).await.expect("update");
    let second = store.get_agent("echo-agent").await.expect("get agent");

    assert_eq!(second.id, first.id, "upsert keeps the original identifier");
    assert_eq!(second.url, "http://127.0.0.1:9002");
    assert!(second.verified);
    assert_eq!(second.jwks_kid.as_deref(), Some("key-1"));
    assert_eq!(second.verified_at.as_deref(), Some("2026-01-01T00:00:05.000Z"));
    assert_eq!(second.verification_issuer.as_deref(), Some("https://issuer.example"));
    assert_eq!(second.verification_alg.as_deref(), Some("ES256"));

    let mut quarantined = sample_agent("other-agent");
    quarantined.verification_error = Some("signature does not verify".to_owned());
    quarantined.quarantined = true;
    store.upsert_agent(&quarantined).await.expect("insert quarantined");
    let loaded = store.get_agent("other-agent").await.expect("quarantined agent");
    assert!(!loaded.verified);
    assert!(loaded.quarantined);
    assert_eq!(loaded.verification_error.as_deref(), Some("signature does not verify"));

    let agents = store.list_agents().await.expect("list agents");
    assert_eq!(agents.len(), 2);

    let missing = store.get_agent("nope").await.expect_err("missing agent");
    assert!(missing.to_string().contains("not found"));
}

async fn tasks_are_paginated(store: &Arc<dyn Store>) {
    for (id, created_at) in [
        ("page-1", "2026-02-01T00:00:00.000Z"),
        ("page-2", "2026-02-02T00:00:00.000Z"),
        ("page-3", "2026-02-03T00:00:00.000Z"),
    ] {
        let mut task = sample_task(id, created_at);
        task.context_id = "page-ctx".to_owned();
        store.create_task(&task).await.expect("create page task");
    }

    let filter = TaskFilter { context_id: Some("page-ctx".to_owned()), ..TaskFilter::default() };
    let first = store.list_tasks_page(&filter, None, 2).await.expect("first page");
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].id, "page-3", "newest first");
    assert_eq!(first[1].id, "page-2");

    let cursor = TaskCursor::from_task(&first[1]);
    let second = store.list_tasks_page(&filter, Some(&cursor), 2).await.expect("second page");
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].id, "page-1");

    assert_eq!(store.count_tasks(&filter).await.expect("count"), 3);

    let recent = store
        .list_tasks(&TaskFilter {
            context_id: Some("page-ctx".to_owned()),
            updated_after: Some("2026-02-02T12:00:00.000Z".to_owned()),
            ..TaskFilter::default()
        })
        .await
        .expect("updated after");
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].id, "page-3");
}

async fn expired_tasks_are_listed(store: &Arc<dyn Store>) {
    let mut stale = sample_task("stale-1", "2020-01-01T00:00:00.000Z");
    stale.updated_at = "2020-01-01T00:00:00.000Z".to_owned();
    store.create_task(&stale).await.expect("create stale");

    let mut done = sample_task("done-1", "2020-01-01T00:00:00.000Z");
    done.updated_at = "2020-01-01T00:00:00.000Z".to_owned();
    done.status = TaskState::Completed;
    store.create_task(&done).await.expect("create done");

    let expired = store.list_expired_tasks("2021-01-01T00:00:00.000Z").await.expect("list expired");
    let ids: Vec<&str> = expired.iter().map(|task| task.id.as_str()).collect();
    assert!(ids.contains(&"stale-1"));
    assert!(!ids.contains(&"done-1"), "terminal tasks never expire");
}

async fn push_configs_roundtrip(store: &Arc<dyn Store>) {
    let record = PushNotificationRecord {
        id: "push-1".to_owned(),
        task_id: "task-1".to_owned(),
        url: "https://example.com/hook".to_owned(),
        token: Some("secret".to_owned()),
        auth_json: Some(r#"{"schemes":["Bearer"],"credentials":"token"}"#.to_owned()),
        created_at: "2026-01-01T00:00:00.000Z".to_owned(),
    };
    store.upsert_push_config(&record).await.expect("create push config");

    let loaded = store.get_push_config("task-1", "push-1").await.expect("get push config");
    assert_eq!(loaded, record);

    let mut updated = record.clone();
    updated.url = "https://example.com/other".to_owned();
    store.upsert_push_config(&updated).await.expect("update push config");
    let listed = store.list_push_configs("task-1").await.expect("list push configs");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].url, "https://example.com/other");

    let missing = store.get_push_config("task-1", "nope").await.expect_err("missing push config");
    assert!(missing.to_string().contains("not found"), "{missing}");

    store.delete_push_config("task-1", "push-1").await.expect("delete push config");
    assert!(store.list_push_configs("task-1").await.expect("list").is_empty());
    assert!(store.delete_push_config("task-1", "push-1").await.is_err());
}

async fn sqlite_store(dir: &tempfile::TempDir) -> WardenStore<sqlx::Sqlite> {
    let config =
        SqliteConfig { path: dir.path().join("store-suite.db"), ..SqliteConfig::default() };
    let store = WardenStore::<sqlx::Sqlite>::connect_sqlite(&config).await.expect("connect sqlite");
    store.migrate().await.expect("migrate sqlite");
    clean_sqlite(store.pool()).await;
    store
}

fn postgres_url() -> Option<String> {
    let url = std::env::var("DATABASE_URL").ok()?;
    url.starts_with("postgres").then_some(url)
}

async fn postgres_store() -> Option<WardenStore<sqlx::Postgres>> {
    let url = postgres_url()?;
    let store = WardenStore::<sqlx::Postgres>::connect_postgres(&PostgresConfig {
        url,
        ..PostgresConfig::default()
    })
    .await
    .expect("connect postgres");
    store.migrate().await.expect("migrate postgres");
    clean_postgres(store.pool()).await;
    Some(store)
}

#[tokio::test]
async fn suite_sqlite() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = sqlite_store(&dir).await;
    run_suite(Arc::new(store.clone())).await;

    let columns = |table: &'static str| {
        let store = store.clone();
        async move {
            let rows = sqlx::query(&format!("PRAGMA table_info({table})"))
                .fetch_all(store.pool())
                .await
                .expect("pragma");
            let mut columns: Vec<String> = rows
                .iter()
                .map(|row| row.try_get::<String, _>("name").expect("column name"))
                .collect();
            columns.sort();
            columns
        }
    };
    assert_schema(columns).await;
}

#[tokio::test]
async fn suite_postgres() {
    let Some(store) = postgres_store().await else {
        eprintln!("DATABASE_URL does not point to PostgreSQL, suite skipped");
        return;
    };
    run_suite(Arc::new(store.clone())).await;

    let columns = |table: &'static str| {
        let store = store.clone();
        async move {
            let rows = sqlx::query(
                "SELECT column_name FROM information_schema.columns WHERE table_name = $1",
            )
            .bind(table)
            .fetch_all(store.pool())
            .await
            .expect("information_schema");
            let mut columns: Vec<String> = rows
                .iter()
                .map(|row| row.try_get::<String, _>("column_name").expect("column name"))
                .collect();
            columns.sort();
            columns
        }
    };
    assert_schema(columns).await;
}

/// Asserts that both backends end up with the columns of the reference data
/// model (reference architecture).
async fn assert_schema<F, Fut>(columns_for: F)
where
    F: Fn(&'static str) -> Fut,
    Fut: std::future::Future<Output = Vec<String>>,
{
    let mut tasks = columns_for("tasks").await;
    tasks.dedup();
    assert_eq!(
        tasks,
        [
            "agent_id",
            "context_id",
            "created_at",
            "id",
            "metadata_json",
            "remote_task_id",
            "status",
            "updated_at",
        ],
        "tasks schema"
    );

    let mut messages = columns_for("messages").await;
    messages.dedup();
    assert_eq!(
        messages,
        ["context_id", "created_at", "id", "message_id", "parts_json", "role", "task_id",],
        "messages schema"
    );

    let mut approvals = columns_for("approvals").await;
    approvals.dedup();
    assert_eq!(
        approvals,
        [
            "created_at",
            "decided_at",
            "decided_by",
            "expires_at",
            "id",
            "reason",
            "status",
            "task_id",
        ],
        "approvals schema"
    );

    let mut audit = columns_for("audit_log").await;
    audit.dedup();
    assert_eq!(
        audit,
        ["action", "actor", "context_id", "hash", "id", "payload_json", "previous_hash", "task_id", "ts",],
        "audit_log schema"
    );

    let mut agents = columns_for("agents").await;
    agents.dedup();
    assert_eq!(
        agents,
        [
            "card_json",
            "created_at",
            "id",
            "jwks_kid",
            "name",
            "quarantined",
            "skills_json",
            "url",
            "verification_alg",
            "verification_error",
            "verification_issuer",
            "verified",
            "verified_at",
        ],
        "agents schema"
    );

    let mut artifacts = columns_for("artifacts").await;
    artifacts.dedup();
    assert_eq!(
        artifacts,
        ["artifact_id", "created_at", "id", "metadata_json", "name", "parts_json", "task_id",],
        "artifacts schema"
    );
}

/// Keeps the compiler honest about the error type used across the suite.
#[allow(dead_code)]
fn assert_error(err: Error) -> String {
    err.to_string()
}
