//! Integration tests over the HTTP surface of the warden.
//!
//! The suite exercises the real router (axum `tower::ServiceExt`) on top of an
//! in-memory SQLite store, so it covers the A2A JSON-RPC binding, the
//! administration API, the interception flow, and the embedded console.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use a2a_warden_core::config::{
    AdminConfig, Config, DatabaseConfig, FrontendConfig, PushNotificationsConfig,
    RetentionConfig, ServerConfig, SqliteConfig, TaskExpiryConfig,
};
use a2a_warden_core::error::Error;
use a2a_warden_core::policy::{PolicyEffect, PolicyRule};
use a2a_warden_core::protocol::{AgentCard, Message, Task, TaskState};
use a2a_warden_core::service::{AgentClient, WardenService};
use a2a_warden_core::stores::Store;
use a2a_warden_server::{build_router, build_state};
use tower::ServiceExt;
use url::Url;

/// Agent client that records the delegated messages and answers with a fixed
/// reply, standing in for a remote agent.
#[derive(Default)]
struct RecordingClient {
    sent: std::sync::Mutex<Vec<Message>>,
}

impl RecordingClient {
    fn sent(&self) -> Vec<Message> {
        self.sent.lock().expect("sent lock").clone()
    }
}

#[async_trait::async_trait]
impl AgentClient for RecordingClient {
    async fn send_message(&self, _agent_url: &Url, message: &Message) -> Result<Task, Error> {
        self.sent.lock().expect("sent lock").push(message.clone());
        Ok(Task {
            id: "remote-task".to_owned(),
            context_id: message.context_id.clone().unwrap_or_default(),
            status: a2a_warden_core::protocol::TaskStatus {
                state: TaskState::Completed,
                message: Some(Message {
                    message_id: "remote-message".to_owned(),
                    role: a2a_warden_core::protocol::Role::Agent,
                    parts: vec![a2a_warden_core::protocol::Part::Text {
                        text: "echo: remote answer".to_owned(),
                    }],
                    context_id: message.context_id.clone(),
                    task_id: None,
                    metadata: None,
                }),
                timestamp: a2a_warden_core::now(),
            },
            artifacts: None,
            history: None,
            metadata: None,
        })
    }

    async fn cancel_task(&self, _agent_url: &Url, _task_id: &str) -> Result<TaskState, Error> {
        Ok(TaskState::Canceled)
    }
}

/// Builds a configuration pointing at an in-memory SQLite database.
fn test_config() -> Config {
    Config {
        server: a2a_warden_core::config::ServerConfig {
            listen_addr: "127.0.0.1:0".to_owned(),
            a2a_version: "1.0".to_owned(),
            request_timeout_secs: 5,
            max_body_size: 1024 * 1024,
            cors_enabled: false,
            cors_origins: Vec::new(),
            public_url: Some("http://127.0.0.1:8080".to_owned()),
        },
        database: DatabaseConfig {
            backend: a2a_warden_core::config::DatabaseBackend::Sqlite,
            sqlite: Some(a2a_warden_core::config::SqliteConfig {
                path: std::path::PathBuf::from(":memory:"),
                wal_mode: false,
                busy_timeout_ms: 5000,
            }),
            postgres: None,
        },
        agents: vec![a2a_warden_core::config::AgentConfig {
            name: "echo-agent".to_owned(),
            url: Url::parse("http://127.0.0.1:9001").expect("valid url"),
            skills: vec!["echo".to_owned()],
            card_json: None,
            metadata: HashMap::new(),
        }],
        retention: RetentionConfig {
            audit_days: 90,
            task_days: 30,
            auto_purge: false,
            purge_interval_hours: 24,
        },
        frontend: FrontendConfig::default(),
        admin: AdminConfig {
            enabled: true,
            path: "/api/v1/admin".to_owned(),
            token: None,
            token_header: "X-Admin-Token".to_owned(),
        },
        push_notifications: a2a_warden_core::config::PushNotificationsConfig {
            enabled: true,
            allowed_hosts: vec!["*".to_owned()],
            allow_private_addresses: true,
            timeout_secs: 1,
        },
        task_expiry: a2a_warden_core::config::TaskExpiryConfig::default(),
        agent_card_verification: a2a_warden_core::config::AgentCardVerificationConfig::default(),
        policy: a2a_warden_core::policy::PolicyConfig::default(),
    }
}

/// Builds a configuration with an allow-all policy for tests that need direct delegation.
fn test_config_with_allow_all() -> Config {
    let mut config = test_config();
    config.policy.rules.push(PolicyRule {
        id: "allow-all".to_owned(),
        description: Some("Allow all skills for API tests".to_owned()),
        active: true,
        effect: PolicyEffect::Allow,
        condition: "true".to_owned(),
        version: 1,
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    });
    config
}

/// Builds the router over a fresh in-memory store.
async fn build_test_router() -> (axum::Router, Arc<RecordingClient>) {
    let config = test_config_with_allow_all();
    let store = a2a_warden_store::WardenStore::<sqlx::Sqlite>::connect_sqlite(
        &a2a_warden_core::config::SqliteConfig {
            path: std::path::PathBuf::from(":memory:"),
            wal_mode: false,
            busy_timeout_ms: 5000,
        },
    )
    .await
    .expect("connect sqlite");
    store.migrate().await.expect("migrate sqlite");
    let store: Arc<dyn Store> = Arc::new(store);
    let client = Arc::new(RecordingClient::default());
    let mut service = WardenService::new(store, Arc::clone(&client) as Arc<dyn AgentClient>).with_push(
        a2a_warden_core::push::PushPolicy::new(true, vec!["*".to_owned()], true),
        Arc::new(
            a2a_warden_server::push_client::HttpPushNotifier::new(std::time::Duration::from_secs(
                1,
            ))
            .expect("push notifier"),
        ),
    );
    // Apply the allow-all policy from the config
    service = service.with_policy_config(config.policy.clone());

    service.register_agents(&config.agents).await.expect("register agents");

    let state = build_state(
        &config,
        service,
        AgentCard::warden_card("A2A Warden", "http://127.0.0.1:8080", "0.1.0"),
        "sqlite",
    );
    let router = build_router(state, &config).expect("build router");
    (router, client)
}

/// Builds a configuration with a hold-all (escalate) policy for tests that need approval.
fn test_config_with_hold_all() -> Config {
    let mut config = test_config();
    config.policy.rules.push(PolicyRule {
        id: "escalate-all".to_owned(),
        description: Some("Escalate all skills for approval".to_owned()),
        active: true,
        effect: PolicyEffect::Escalate,
        condition: "true".to_owned(),
        version: 1,
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    });
    config
}

/// Builds the router with a hold-all policy for tests that need approval flow.
async fn build_test_router_with_hold_all() -> (axum::Router, Arc<RecordingClient>) {
    let config = test_config_with_hold_all();
    let store = a2a_warden_store::WardenStore::<sqlx::Sqlite>::connect_sqlite(
        &a2a_warden_core::config::SqliteConfig {
            path: std::path::PathBuf::from(":memory:"),
            wal_mode: false,
            busy_timeout_ms: 5000,
        },
    )
    .await
    .expect("connect sqlite");
    store.migrate().await.expect("migrate sqlite");
    let store: Arc<dyn Store> = Arc::new(store);
    let client = Arc::new(RecordingClient::default());
    let mut service = WardenService::new(store, Arc::clone(&client) as Arc<dyn AgentClient>).with_push(
        a2a_warden_core::push::PushPolicy::new(true, vec!["*".to_owned()], true),
        Arc::new(
            a2a_warden_server::push_client::HttpPushNotifier::new(std::time::Duration::from_secs(
                1,
            ))
            .expect("push notifier"),
        ),
    );
    // Apply the allow-all policy from the config
    service = service.with_policy_config(config.policy.clone());

    service.register_agents(&config.agents).await.expect("register agents");

    let state = build_state(
        &config,
        service,
        AgentCard::warden_card("A2A Warden", "http://127.0.0.1:8080", "0.1.0"),
        "sqlite",
    );
    let router = build_router(state, &config).expect("build router");
    (router, client)
}

/// Sends a JSON-RPC request to the warden.
async fn rpc(router: &axum::Router, method: &str, params: Value) -> Value {
    let body = json!({
        "jsonrpc": "2.0",
        "id": "test-1",
        "method": method,
        "params": params,
    });
    let request = Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/json")
        .header("A2A-Version", "1.0")
        .body(Body::from(body.to_string()))
        .expect("request");

    let response = router.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = response.into_body().collect().await.expect("body").to_bytes();
    assert!(status.is_success(), "unexpected status {status}: {}", String::from_utf8_lossy(&bytes));
    serde_json::from_slice(&bytes).expect("json body")
}

/// Builds the `message/send` params of a simple text message.
fn send_params(text: &str, skill: Option<&str>) -> Value {
    let mut metadata = serde_json::Map::new();
    metadata.insert("senderAgent".to_owned(), json!("client-1"));
    if let Some(skill) = skill {
        metadata.insert("skill".to_owned(), json!(skill));
    }
    json!({
        "message": {
            "messageId": "message-1",
            "role": "user",
            "parts": [{"type": "text", "text": text}],
            "metadata": metadata,
        }
    })
}

/// Performs a GET request against the router.
async fn get(router: &axum::Router, uri: &str) -> (StatusCode, String) {
    let request = Request::builder().method("GET").uri(uri).body(Body::empty()).expect("request");
    let response = router.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = response.into_body().collect().await.expect("body").to_bytes();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

#[tokio::test]
async fn message_send_pauses_when_approval_is_required() {
    let (router, client) = build_test_router_with_hold_all().await;

    let response = rpc(&router, "message/send", send_params("hello", Some("echo"))).await;
    assert_eq!(response["result"]["status"]["state"], "input-required");
    assert!(client.sent().is_empty(), "nothing must be delegated yet");

    let task_id = response["result"]["id"].as_str().expect("task id").to_owned();

    // The task is visible in the administration API.
    let (status, body) = get(&router, &format!("/api/v1/admin/tasks/{task_id}")).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let detail: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(detail["task"]["status"], "input-required");
    assert_eq!(detail["approval"]["status"], "pending");
    assert_eq!(detail["messages"].as_array().expect("messages").len(), 2);
}

#[tokio::test]
async fn approval_forwards_the_task_and_completes_it() {
    let (router, client) = build_test_router_with_hold_all().await;

    let response = rpc(&router, "message/send", send_params("hello", Some("echo"))).await;
    let task_id = response["result"]["id"].as_str().expect("task id").to_owned();

    let (_, body) = get(&router, "/api/v1/admin/approvals?status=pending").await;
    let approvals: Value = serde_json::from_str(&body).expect("json");
    let approval_id = approvals["approvals"][0]["id"].as_str().expect("approval id").to_owned();

    let decision = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/admin/approvals/{approval_id}/decision"))
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"decision": "approve", "approver": "alice", "reason": "looks good"}).to_string(),
        ))
        .expect("request");
    let response = router.clone().oneshot(decision).await.expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.expect("body").to_bytes();
    let decision_body: Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(decision_body["task"]["status"]["state"], "completed");
    assert_eq!(decision_body["approval"]["status"], "approved");
    assert_eq!(decision_body["approval"]["decidedBy"], "alice");

    assert_eq!(client.sent().len(), 1, "the remote agent received the task");
    assert_eq!(client.sent()[0].text(), "hello");

    // The client can now read the final result.
    let response = rpc(&router, "tasks/get", json!({"id": task_id, "historyLength": 10})).await;
    assert_eq!(response["result"]["status"]["state"], "completed");
    let history = response["result"]["history"].as_array().expect("history");
    assert!(history.iter().any(|message| message["parts"][0]["text"] == "echo: remote answer"));
}

#[tokio::test]
async fn rejection_never_reaches_the_remote_agent() {
    let (router, client) = build_test_router_with_hold_all().await;

    let response = rpc(&router, "message/send", send_params("hello", None)).await;
    let task_id = response["result"]["id"].as_str().expect("task id").to_owned();

    let (_, body) = get(&router, "/api/v1/admin/approvals?status=pending").await;
    let approvals: Value = serde_json::from_str(&body).expect("json");
    let approval_id = approvals["approvals"][0]["id"].as_str().expect("approval id").to_owned();

    let decision = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/admin/approvals/{approval_id}/decision"))
        .header("content-type", "application/json")
        .body(Body::from(json!({"decision": "reject", "approver": "bob"}).to_string()))
        .expect("request");
    let response = router.clone().oneshot(decision).await.expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.expect("body").to_bytes();
    let body: Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(body["task"]["status"]["state"], "rejected");
    assert_eq!(body["task"]["id"], task_id);
    assert!(client.sent().is_empty(), "a rejected task is never delegated");
}

#[tokio::test]
async fn cancel_task_resolves_the_pending_approval() {
    let (router, client) = build_test_router_with_hold_all().await;

    let response = rpc(&router, "message/send", send_params("hello", None)).await;
    let _task_id = response["result"]["id"].as_str().expect("task id").to_owned();

    let response = rpc(&router, "tasks/cancel", json!({"id": _task_id})).await;
    assert_eq!(response["result"]["status"]["state"], "canceled");
    assert!(client.sent().is_empty());

    let (_, body) = get(&router, "/api/v1/admin/approvals").await;
    let approvals: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(approvals["approvals"][0]["status"], "cancelled");
}

#[tokio::test]
async fn direct_delegation_without_interception() {
    let (router, client) = build_test_router().await;

    let response = rpc(&router, "message/send", send_params("hello", Some("echo"))).await;
    assert_eq!(response["result"]["status"]["state"], "completed");
    assert_eq!(client.sent().len(), 1);
}

#[tokio::test]
async fn agent_card_is_published() {
    let (router, _) = build_test_router().await;

    let (status, body) = get(&router, "/.well-known/agent-card.json").await;
    assert_eq!(status, StatusCode::OK);
    let card: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(card["name"], "A2A Warden");
    assert_eq!(card["url"], "http://127.0.0.1:8080");
    assert_eq!(card["protocolVersion"], "1.0");
}

#[tokio::test]
async fn missing_version_header_is_negotiated_as_v1() {
    let (router, _) = build_test_router().await;

    let request = Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"jsonrpc": "2.0", "id": 1, "method": "GetTask", "params": {"id": "x"}})
                .to_string(),
        ))
        .expect("request");
    let response = router.clone().oneshot(request).await.expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("A2A-Version").and_then(|value| value.to_str().ok()),
        Some("1.0")
    );
}

#[tokio::test]
async fn an_unsupported_version_header_is_rejected() {
    let (router, _) = build_test_router().await;

    let request = Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/json")
        .header("A2A-Version", "9.9")
        .body(Body::from(
            json!({"jsonrpc": "2.0", "id": 1, "method": "GetTask", "params": {"id": "x"}})
                .to_string(),
        ))
        .expect("request");
    let response = router.clone().oneshot(request).await.expect("response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn unknown_method_returns_a_json_rpc_error() {
    let (router, _) = build_test_router().await;

    let response = rpc(&router, "tasks/unknown", json!({})).await;
    assert_eq!(response["error"]["code"], -32601);
}

#[tokio::test]
async fn missing_task_returns_an_error() {
    let (router, _) = build_test_router().await;

    let response = rpc(&router, "tasks/get", json!({"id": "does-not-exist"})).await;
    assert!(response.get("error").is_some(), "{response}");
}

#[tokio::test]
async fn admin_api_lists_tasks_and_agents() {
    let (router, _) = build_test_router().await;
    rpc(&router, "message/send", send_params("hello", Some("echo"))).await;

    let (status, body) = get(&router, "/api/v1/admin/tasks").await;
    assert_eq!(status, StatusCode::OK);
    let tasks: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(tasks["tasks"].as_array().expect("tasks").len(), 1);

    let (status, body) = get(&router, "/api/v1/admin/agents").await;
    assert_eq!(status, StatusCode::OK);
    let agents: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(agents["agents"][0]["name"], "echo-agent");
}

#[tokio::test]
async fn admin_api_rejects_unknown_task_states() {
    let (router, _) = build_test_router().await;
    let (status, _) = get(&router, "/api/v1/admin/tasks?status=nonsense").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn console_is_served_with_the_correct_base() {
    let (router, _) = build_test_router().await;

    let (status, body) = get(&router, "/ui").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<base href=\"/ui/\">"), "{body}");
    assert!(body.contains("A2A Warden"), "{body}");

    // Client side routes fall back to the SPA shell.
    let (status, body) = get(&router, "/ui/tasks/123").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<base href=\"/ui/\">"), "{body}");
}

#[tokio::test]
async fn root_redirects_to_the_console() {
    let (router, _) = build_test_router().await;
    let request = Request::builder().method("GET").uri("/").body(Body::empty()).expect("request");
    let response = router.clone().oneshot(request).await.expect("response");
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(
        response.headers().get("location").and_then(|value| value.to_str().ok()),
        Some("/ui")
    );
}

/// Keeps the unused import warning away while documenting the address type.
#[allow(dead_code)]
fn _address_type(_: SocketAddr) {}
