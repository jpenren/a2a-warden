//! A2A Warden Server
//!
//! axum-based A2A endpoints, administration API, and embedded UI router.
//!
//! The three families coexist in a single [`axum::Router`] (decision AD-4):
//! the A2A endpoints under the specification routes, the administration API
//! under `/api/v1/admin`, and the web console under `/ui`.

pub mod a2a;
pub mod admin;
pub mod agent_client;
pub mod push_client;
pub mod ui;

use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderValue, StatusCode};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde_json::{json, Value};
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

use a2a_warden_core::config::{AdminConfig, Config, ServerConfig};
use a2a_warden_core::error::Error;
use a2a_warden_core::protocol::AgentCard;
use a2a_warden_core::service::WardenService;

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    /// Domain service orchestrating the warden workflow.
    pub service: WardenService,
    /// Agent Card published by the warden.
    pub agent_card: Arc<AgentCard>,
    /// Administration API authentication settings.
    pub admin: AdminConfig,
    /// Declared A2A protocol version.
    pub a2a_version: String,
    /// Active database backend name.
    pub backend: String,
    /// Warden version.
    pub version: String,
}

/// Maps a domain error onto the JSON-RPC error space.
pub fn rpc_error(err: Error) -> a2a_warden_core::protocol::JsonRpcErrorObject {
    use a2a_warden_core::protocol::JsonRpcErrorObject;

    match err {
        Error::Protocol(a2a_warden_core::error::ProtocolError::MethodNotFound(method)) => {
            JsonRpcErrorObject::method_not_found(method)
        }
        Error::Protocol(a2a_warden_core::error::ProtocolError::InvalidParams(detail)) => {
            JsonRpcErrorObject::invalid_params(detail)
        }
        Error::Protocol(a2a_warden_core::error::ProtocolError::AgentNotFound(detail)) => {
            JsonRpcErrorObject::invalid_params(detail)
        }
        Error::Protocol(a2a_warden_core::error::ProtocolError::SkillNotFound(detail)) => {
            JsonRpcErrorObject::invalid_params(detail)
        }
        Error::Storage(a2a_warden_core::error::StorageError::TaskNotFound(id)) => {
            JsonRpcErrorObject::invalid_params(format!("task not found: {id}"))
        }
        Error::Storage(a2a_warden_core::error::StorageError::ApprovalNotFound(id)) => {
            JsonRpcErrorObject::invalid_params(format!("approval not found: {id}"))
        }
        Error::Conflict(message) => JsonRpcErrorObject::new(-32001, message),
        Error::Validation(message) => JsonRpcErrorObject::invalid_params(message),
        other => JsonRpcErrorObject::server_error(other.to_string()),
    }
}

/// Builds the complete application router.
///
/// # Errors
/// Returns an error when the administration API path is not usable as a route
/// prefix.
pub fn build_router(state: AppState, config: &Config) -> Result<Router, Error> {
    let admin_path = normalize_prefix(&config.admin.path);
    let ui_path = normalize_prefix(&config.frontend.path);

    let mut router = Router::new();

    // A2A JSON-RPC binding .
    router = router
        .route("/", post(a2a::jsonrpc).get(a2a::redirect_to_console))
        .route("/a2a", post(a2a::jsonrpc))
        .route("/rpc", post(a2a::jsonrpc))
        .route("/message/send", post(a2a::jsonrpc))
        .route("/message/stream", post(a2a::jsonrpc))
        .route("/tasks/get", post(a2a::jsonrpc))
        .route("/tasks/cancel", post(a2a::jsonrpc))
        .route("/tasks/list", post(a2a::jsonrpc))
        .route("/tasks/resubscribe", post(a2a::jsonrpc))
        .route("/tasks/pushNotificationConfig/set", post(a2a::jsonrpc))
        .route("/tasks/pushNotificationConfig/get", post(a2a::jsonrpc))
        .route("/tasks/pushNotificationConfig/list", post(a2a::jsonrpc))
        .route("/tasks/pushNotificationConfig/delete", post(a2a::jsonrpc))
        .route("/.well-known/agent-card.json", get(a2a::agent_card))
        .route("/.well-known/agent.json", get(a2a::agent_card));

    // Administration API .
    if config.admin.enabled {
        let admin_router = Router::new()
            .route("/health", get(admin::health))
            .route("/tasks", get(admin::list_tasks))
            .route("/tasks/:id", get(admin::get_task))
            .route("/approvals", get(admin::list_approvals))
            .route("/approvals/:id/decision", post(admin::decide_approval))
            .route("/agents", get(admin::list_agents))
            .route("/audit/export", get(admin::export_audit))
            .route("/audit/verify", get(admin::verify_audit))
            .route("/traffic/stream", get(admin::traffic_stream))
            .route("/search", get(admin::search))
            .route("/tasks/:id/replay", post(admin::replay_task))
            .route("/messages/:id/resend", post(admin::resend_message))
            .route("/pii/mask", post(admin::mask_pii))
            .layer(middleware::from_fn_with_state(state.clone(), admin::require_admin_token));

        router = router.nest(&admin_path, admin_router);
    }

    // Embedded web console .
    if config.frontend.enabled {
        router = router.merge(ui::router(&ui_path, config.frontend.spa_fallback.as_str()));
    }

    let router = router
        .fallback(not_found)
        .layer(DefaultBodyLimit::max(config.server.max_body_size))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    Ok(if config.server.cors_enabled {
        let origins = if config.server.cors_origins.is_empty() {
            CorsLayer::permissive()
        } else {
            let parsed: Vec<HeaderValue> = config
                .server
                .cors_origins
                .iter()
                .filter_map(|origin| HeaderValue::from_str(origin).ok())
                .collect();
            CorsLayer::new().allow_origin(parsed).allow_methods(Any).allow_headers(Any)
        };
        router.layer(origins)
    } else {
        router
    })
}

/// Normalizes a configured path into a router prefix (no trailing slash).
fn normalize_prefix(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "/" {
        String::new()
    } else {
        format!("/{}", trimmed.trim_matches('/'))
    }
}

/// Fallback handler returning a JSON-RPC shaped error for unknown routes.
async fn not_found(uri: axum::http::Uri) -> Response {
    (
        StatusCode::NOT_FOUND,
        axum::Json(json!({
            "jsonrpc": "2.0",
            "id": Value::Null,
            "error": {
                "code": -32601,
                "message": format!("no route for {}", uri.path()),
            }
        })),
    )
        .into_response()
}

/// Builds the application state from a validated configuration.
#[must_use]
pub fn build_state(
    config: &Config,
    service: WardenService,
    agent_card: AgentCard,
    backend: &str,
) -> AppState {
    AppState {
        service,
        agent_card: Arc::new(agent_card),
        admin: config.admin.clone(),
        a2a_version: config.server.a2a_version.clone(),
        backend: backend.to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

/// Convenience accessor used by the CLI to log the effective configuration.
#[must_use]
pub fn describe(config: &ServerConfig) -> String {
    format!(
        "listen={} a2a_version={} cors={}",
        config.listen_addr, config.a2a_version, config.cors_enabled
    )
}
