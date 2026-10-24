//! Shared harness of the end to end acceptance suite.
//!
//! The harness starts a stub remote agent (an axum server answering the A2A
//! JSON-RPC binding) and a real warden (the same wiring the `a2a-warden`
//! binary uses), both on ephemeral ports, and talks to them over HTTP.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::post;
use axum::Json;
use axum::Router;
use serde_json::{json, Value};

use a2a_warden_core::config::{Config, DatabaseBackend, DatabaseConfig};

/// A stub remote agent that records the messages it receives.
#[derive(Clone, Default)]
pub struct StubAgent {
    /// Every message the agent received, in order.
    pub received: Arc<Mutex<Vec<Value>>>,
    /// When true, the agent reports a failure instead of answering.
    pub failing: Arc<Mutex<bool>>,
    /// When true, the agent only answers the A2A v1.0 method names.
    pub v1_only: Arc<Mutex<bool>>,
    /// Name of every method the agent was called with, in order.
    pub methods: Arc<Mutex<Vec<String>>>,
}

/// State of the stub agent handlers.
#[derive(Clone)]
struct StubState {
    received: Arc<Mutex<Vec<Value>>>,
    failing: Arc<Mutex<bool>>,
    v1_only: Arc<Mutex<bool>>,
    methods: Arc<Mutex<Vec<String>>>,
}

/// Builds the stub agent router.
fn stub_router(state: StubState) -> Router {
    Router::new().route("/", post(stub_rpc)).with_state(state)
}

/// Handles one JSON-RPC request of the stub agent.
#[axum::debug_handler]
async fn stub_rpc(
    State(state): State<StubState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Json<Value> {
    assert_eq!(
        headers.get("A2A-Version").and_then(|value| value.to_str().ok()),
        Some("1.0"),
        "the warden must send the A2A-Version header"
    );

    let method = body.get("method").and_then(Value::as_str).unwrap_or_default();
    let id = body.get("id").cloned().unwrap_or(Value::Null);

    state.methods.lock().expect("methods lock").push(method.to_owned());

    if *state.failing.lock().expect("failing lock") {
        return Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": -32000, "message": "stub agent failure"}
        }));
    }

    // The stub answers the v0.3 method names, unless it is configured to
    // speak the A2A v1.0 ones only.
    let v1_only = *state.v1_only.lock().expect("v1_only lock");
    let send_method = if v1_only { "SendMessage" } else { "message/send" };
    let cancel_method = if v1_only { "CancelTask" } else { "tasks/cancel" };

    match method {
        method if method == send_method => {
            let message = body["params"]["message"].clone();
            state.received.lock().expect("received lock").push(message.clone());
            let text = message["parts"]
                .as_array()
                .map(|parts| {
                    parts
                        .iter()
                        .filter_map(|part| part.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            let context =
                message.get("contextId").cloned().unwrap_or_else(|| json!("stub-context"));
            Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "id": "stub-task-1",
                    "contextId": context,
                    "artifacts": [
                        {
                            "artifactId": "stub-artifact-1",
                            "name": "echo result",
                            "parts": [{"type": "text", "text": format!("echo: {text}")}]
                        }
                    ],
                    "status": {
                        "state": "completed",
                        "message": {
                            "messageId": "stub-message-1",
                            "role": "agent",
                            "parts": [{"type": "text", "text": format!("echo: {text}")}],
                            "contextId": context,
                        },
                        "timestamp": "2026-01-01T00:00:00.000Z"
                    }
                }
            }))
        }
        "tasks/cancel" | "CancelTask" if method == cancel_method => Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "id": body["params"]["id"].clone(),
                "contextId": "stub-context",
                "status": {"state": "canceled", "timestamp": "2026-01-01T00:00:00.000Z"}
            }
        })),
        other => Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": -32601, "message": format!("method not found: {other}")}
        })),
    }
}

/// A running stub agent.
pub struct StubAgentServer {
    /// Address the agent listens on.
    pub address: SocketAddr,
    /// Handle used to inspect and configure the agent.
    pub agent: StubAgent,
    /// The spawned server task.
    pub handle: tokio::task::JoinHandle<()>,
}

impl StubAgentServer {
    /// Starts a stub agent on an ephemeral port.
    pub async fn start() -> Self {
        let agent = StubAgent::default();
        let state = StubState {
            received: Arc::clone(&agent.received),
            failing: Arc::clone(&agent.failing),
            v1_only: Arc::clone(&agent.v1_only),
            methods: Arc::clone(&agent.methods),
        };
        let router = stub_router(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind stub agent");
        let address = listener.local_addr().expect("stub agent address");
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Self { address, agent, handle }
    }

    /// Name of every method the agent was called with, in order.
    pub fn called_methods(&self) -> Vec<String> {
        self.agent.methods.lock().expect("methods lock").clone()
    }

    /// Number of messages the agent received.
    pub fn received_count(&self) -> usize {
        self.agent.received.lock().expect("received lock").len()
    }

    /// Base URL of the agent.
    pub fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    /// Text of every message the agent received.
    pub fn received_texts(&self) -> Vec<String> {
        self.agent
            .received
            .lock()
            .expect("received lock")
            .iter()
            .map(|message| {
                message["parts"]
                    .as_array()
                    .map(|parts| {
                        parts
                            .iter()
                            .filter_map(|part| part.get("text").and_then(Value::as_str))
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_default()
            })
            .collect()
    }
}

/// A running warden.
pub struct WardenServer {
    /// Address the warden listens on.
    pub address: SocketAddr,
    /// The spawned server task.
    pub handle: tokio::task::JoinHandle<()>,
    /// Background tasks spawned by the warden (e.g. task expiry sweeper).
    pub background_tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl WardenServer {
    /// Base URL of the warden.
    pub fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }
}

/// Builds the configuration of the warden for the acceptance suite.
pub fn build_config(agent_url: &str, database_path: &std::path::Path) -> Config {
    let yaml = format!(
        r#"
server:
  listen_addr: "127.0.0.1:0"
  a2a_version: "1.0"
  request_timeout_secs: 10
  max_body_size: 10485760
  cors_enabled: false
  cors_origins: []
  public_url: "http://127.0.0.1:8080"
database:
  backend: sqlite
  sqlite:
    path: "{}"
    wal_mode: false
    busy_timeout_ms: 5000
agents:
  - name: "echo-agent"
    url: "{}"
    skills: ["echo"]

# Cedar policy engine configuration (Phase 4)
policy:
  rules:
    - id: "escalate-all-skills"
      description: "Escalate all tasks for human approval by default"
      active: true
      effect: escalate
      condition: "targetAgent == 'echo-agent' && skill != ''"
      version: 1
      created_at: "2026-01-01T00:00:00Z"
retention:
  audit_days: 90
  task_days: 30
  auto_purge: false
  purge_interval_hours: 24
frontend:
  enabled: true
  path: "/ui"
  spa_fallback: "/ui/index.html"
admin:
  enabled: true
  path: "/api/v1/admin"
  token: null
  token_header: "X-Admin-Token"
"#,
        database_path.display(),
        agent_url
    );
    serde_yaml::from_str(&yaml).expect("valid configuration")
}

/// Builds a configuration with a per-skill approval gate.
///
/// The agent provides two skills, and only one of them is held for a human
/// decision.
pub fn build_config_with_gated_skill(agent_url: &str, database_path: &std::path::Path) -> Config {
    let yaml = format!(
        r#"
server:
  listen_addr: "127.0.0.1:0"
  a2a_version: "1.0"
  request_timeout_secs: 10
  cors_enabled: false
  public_url: "http://127.0.0.1:8080"
database:
  backend: sqlite
  sqlite:
    path: "{}"
    wal_mode: false
    busy_timeout_ms: 5000
agents:
  - name: "echo-agent"
    url: "{}"
    skills: ["echo", "refund"]

# Cedar policy engine configuration (Phase 4)
policy:
  rules:
    - id: "escalate-refund-skill"
      description: "Escalate refund skill tasks for human approval"
      active: true
      effect: escalate
      condition: "targetAgent == 'echo-agent' && skill == 'refund'"
      version: 1
      created_at: "2026-01-01T00:00:00Z"
    - id: "allow-echo-skill"
      description: "Allow echo skill without approval"
      active: true
      effect: allow
      condition: "targetAgent == 'echo-agent' && skill == 'echo'"
      version: 1
      created_at: "2026-01-01T00:00:00Z"
retention:
  audit_days: 90
  task_days: 30
frontend:
  enabled: true
  path: "/ui"
  spa_fallback: "/ui/index.html"
admin:
  enabled: true
  path: "/api/v1/admin"
  token: null
  token_header: "X-Admin-Token"
"#,
        database_path.display(),
        agent_url
    );
    serde_yaml::from_str(&yaml).expect("valid configuration")
}

/// Builds a configuration for the PostgreSQL backend when `DATABASE_URL`
/// points to a PostgreSQL server.
pub fn postgres_config(agent_url: &str) -> Option<Config> {
    let url = std::env::var("DATABASE_URL").ok()?;
    if !url.starts_with("postgres") {
        return None;
    }
    let yaml = format!(
        r#"
server:
  listen_addr: "127.0.0.1:0"
  a2a_version: "1.0"
  request_timeout_secs: 10
  max_body_size: 10485760
  cors_enabled: false
  cors_origins: []
  public_url: "http://127.0.0.1:8080"
database:
  backend: postgres
  postgres:
    url: "{}"
    max_connections: 5
    min_connections: 1
    connect_timeout_secs: 10
agents:
  - name: "echo-agent"
    url: "{}"
    skills: ["echo"]

# Cedar policy engine configuration (Phase 4)
policy:
  rules:
    - id: "escalate-all-skills"
      description: "Escalate all tasks for human approval by default"
      active: true
      effect: escalate
      condition: "targetAgent == 'echo-agent' && skill != ''"
      version: 1
      created_at: "2026-01-01T00:00:00Z"
retention:
  audit_days: 90
  task_days: 30
  auto_purge: false
  purge_interval_hours: 24
frontend:
  enabled: true
  path: "/ui"
  spa_fallback: "/ui/index.html"
admin:
  enabled: true
  path: "/api/v1/admin"
  token: null
  token_header: "X-Admin-Token"
"#,
        url, agent_url
    );
    let mut config: Config = serde_yaml::from_str(&yaml).expect("valid configuration");
    config.database = DatabaseConfig {
        backend: DatabaseBackend::Postgres,
        sqlite: None,
        postgres: Some(a2a_warden_core::config::PostgresConfig {
            url,
            max_connections: 5,
            min_connections: 1,
            connect_timeout_secs: 10,
        }),
    };
    Some(config)
}

/// Starts the warden on an ephemeral port and returns its address.
pub async fn start_warden(config: &Config) -> WardenServer {
    let mut background_tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    let warden = a2a_warden_cli::build(config, &mut background_tasks)
        .await
        .expect("build the warden");
    let listener = tokio::net::TcpListener::bind(warden.listen_addr).await.expect("bind warden");
    let address = listener.local_addr().expect("warden address");
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, warden.router).await;
    });
    WardenServer {
        address,
        handle,
        background_tasks,
    }
}

/// Sends a JSON-RPC request to the warden.
pub async fn rpc(base_url: &str, method: &str, params: Value) -> Value {
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{base_url}/"))
        .header("A2A-Version", "1.0")
        .header("content-type", "application/json")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": "e2e-1",
            "method": method,
            "params": params,
        }))
        .send()
        .await
        .expect("send rpc");
    response.json().await.expect("rpc response")
}

/// Sends a JSON-RPC request with an explicit A2A-Version header.
pub async fn rpc_with_version(
    base_url: &str,
    method: &str,
    params: Value,
    version: Option<&str>,
) -> reqwest::Response {
    let client = reqwest::Client::new();
    let mut request = client
        .post(format!("{base_url}/"))
        .header("content-type", "application/json")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": "e2e-1",
            "method": method,
            "params": params,
        }));
    if let Some(version) = version {
        request = request.header("A2A-Version", version);
    }
    request.send().await.expect("send rpc")
}

/// Builds the `message/send` parameters of a text message.
pub fn send_params(text: &str, skill: Option<&str>) -> Value {
    let mut metadata = serde_json::Map::new();
    metadata.insert("senderAgent".to_owned(), json!("e2e-client"));
    if let Some(skill) = skill {
        metadata.insert("skill".to_owned(), json!(skill));
    }
    json!({
        "message": {
            "messageId": "e2e-message-1",
            "role": "user",
            "parts": [{"type": "text", "text": text}],
            "metadata": metadata,
        }
    })
}

/// Lists the pending approvals of the warden.
pub async fn pending_approvals(base_url: &str) -> Vec<Value> {
    let client = reqwest::Client::new();
    let body: Value = client
        .get(format!("{base_url}/api/v1/admin/approvals?status=pending"))
        .send()
        .await
        .expect("list approvals")
        .json()
        .await
        .expect("approvals body");
    body["approvals"].as_array().cloned().unwrap_or_default()
}

/// Applies a decision to an approval through the administration API.
pub async fn decide(base_url: &str, approval_id: &str, decision: &str, approver: &str) -> Value {
    let client = reqwest::Client::new();
    client
        .post(format!("{base_url}/api/v1/admin/approvals/{approval_id}/decision"))
        .header("content-type", "application/json")
        .json(&json!({
            "decision": decision,
            "approver": approver,
            "reason": "e2e decision",
        }))
        .send()
        .await
        .expect("decide")
        .json()
        .await
        .expect("decision body")
}

/// Reads the detail of a task from the administration API.
pub async fn task_detail(base_url: &str, task_id: &str) -> Value {
    let client = reqwest::Client::new();
    client
        .get(format!("{base_url}/api/v1/admin/tasks/{task_id}"))
        .send()
        .await
        .expect("task detail")
        .json()
        .await
        .expect("detail body")
}

/// Extracts the text of every message of a task detail.
pub fn timeline_texts(detail: &Value) -> Vec<String> {
    detail["messages"]
        .as_array()
        .map(|messages| {
            messages
                .iter()
                .map(|message| {
                    message["partsJson"]
                        .as_str()
                        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                        .and_then(|parts| {
                            parts.as_array().map(|parts| {
                                parts
                                    .iter()
                                    .filter_map(|part| part.get("text").and_then(Value::as_str))
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            })
                        })
                        .unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Waits until the asynchronous predicate holds or the timeout expires.
pub async fn wait_for<F, Fut>(timeout: std::time::Duration, mut predicate: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if predicate().await {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    predicate().await
}

/// Creates a temporary SQLite path for the acceptance suite.
pub fn temp_database(dir: &tempfile::TempDir) -> std::path::PathBuf {
    dir.path().join("a2a-warden-e2e.db")
}
