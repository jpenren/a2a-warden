//! Administration REST API  and the token guard used by the console
//! and by automation.
//!
//! The API is the same one the embedded console consumes, so approvals are
//! automatable from tests and scripts.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use axum::response::sse::{Event, Sse};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use hex;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use a2a_warden_core::error::{Error, ProtocolError, StorageError};
use a2a_warden_core::protocol::TaskState;
use a2a_warden_core::stores::{ApprovalStatus, TaskFilter};
use a2a_warden_core::workflow::ApprovalDecision;
use crate::AppState;

/// `GET /health`: unauthenticated liveness and build information.
pub async fn health(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "status": "ok",
        "version": state.version,
        "a2aVersion": state.a2a_version,
        "databaseBackend": state.backend,
    }))
}

/// Filters accepted by `GET /tasks`.
#[derive(Debug, Default, Deserialize)]
pub struct TaskQuery {
    /// Filter by task state.
    pub status: Option<String>,
    /// Filter by target agent name.
    pub agent: Option<String>,
    /// Filter by conversation context.
    pub context: Option<String>,
    /// Maximum number of rows.
    pub limit: Option<u32>,
    /// Offset for pagination.
    pub offset: Option<u32>,
}

/// Filters accepted by `GET /approvals`.
#[derive(Debug, Default, Deserialize)]
pub struct ApprovalQuery {
    /// Filter by decision status (`pending` by default for the inbox).
    pub status: Option<String>,
    /// Offset for pagination.
    pub offset: Option<u32>,
    /// Maximum number of rows.
    pub limit: Option<u32>,
}

/// Body of `POST /approvals/:id/decision`.
#[derive(Debug, Deserialize)]
pub struct DecisionBody {
    /// One of `approve`, `reject`, `cancel`.
    pub decision: String,
    /// Optional reason stored with the decision.
    pub reason: Option<String>,
    /// Identity of the human supervisor (defaults to `operator`).
    pub approver: Option<String>,
}

/// Maps a domain error to the HTTP surface of the administration API.
pub fn admin_error(err: Error) -> Response {
    let (status, message) = match &err {
        Error::Storage(
            StorageError::TaskNotFound(id)
            | StorageError::ApprovalNotFound(id)
            | StorageError::AgentNotFound(id)
            | StorageError::MessageNotFound(id),
        ) => (StatusCode::NOT_FOUND, format!("not found: {id}")),
        Error::NotFound(id) => (StatusCode::NOT_FOUND, format!("not found: {id}")),
        Error::Conflict(message) => (StatusCode::CONFLICT, message.clone()),
        Error::Unauthorized(message) => (StatusCode::UNAUTHORIZED, message.clone()),
        Error::Forbidden(message) => (StatusCode::FORBIDDEN, message.clone()),
        Error::Validation(message) | Error::Protocol(ProtocolError::InvalidParams(message)) => {
            (StatusCode::BAD_REQUEST, message.clone())
        }
        other => (StatusCode::INTERNAL_SERVER_ERROR, other.to_string()),
    };
    (status, Json(json!({ "error": message }))).into_response()
}

fn ok_or_admin(result: Result<Value, Error>) -> Response {
    match result {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(err) => admin_error(err),
    }
}

/// `GET /tasks` (with `status`, `agent`, `context`, and `limit` filters).
pub async fn list_tasks(State(state): State<AppState>, Query(query): Query<TaskQuery>) -> Response {
    let status = match query.status.as_deref() {
        None => None,
        Some(raw) => match TaskState::parse(raw) {
            Some(status) => Some(status),
            None => {
                return admin_error(Error::Validation(format!("unknown task status '{raw}'")));
            }
        },
    };
    let filter = TaskFilter {
        status,
        agent: query.agent,
        context_id: query.context,
        updated_after: None,
        limit: query.limit,
        offset: query.offset,
    };
    let tasks_res = state.service.list_tasks(&filter).await;
    let total_res = state.service.count_tasks(&filter).await;
    ok_or_admin(match (tasks_res, total_res) {
        (Ok(tasks), Ok(total)) => Ok(json!({ "tasks": tasks, "total": total })),
        (Err(e), _) => Err(e),
        (_, Err(e)) => Err(e),
    })
}

/// `GET /tasks/:id`: task, timeline, approval, and audit trail.
pub async fn get_task(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    ok_or_admin(
        state
            .service
            .task_detail(&id)
            .await
            .map(|detail| serde_json::to_value(detail).unwrap_or(Value::Null)),
    )
}

/// `GET /approvals` (use `?status=pending` for the inbox).
pub async fn list_approvals(
    State(state): State<AppState>,
    Query(query): Query<ApprovalQuery>,
) -> Response {
    let status = match query.status.as_deref() {
        None => None,
        Some(raw) => match ApprovalStatus::parse(raw) {
            Some(status) => Some(status),
            None => {
                return admin_error(Error::Validation(format!("unknown approval status '{raw}'")));
            }
        },
    };
    let approvals_res = state.service.list_approvals(status, query.limit, query.offset).await;
    let total_res = state.service.count_approvals(status).await;
    ok_or_admin(match (approvals_res, total_res) {
        (Ok(approvals), Ok(total)) => Ok(json!({ "approvals": approvals, "total": total })),
        (Err(e), _) => Err(e),
        (_, Err(e)) => Err(e),
    })
}

/// `POST /approvals/:id/decision`: applies a human decision .
pub async fn decide_approval(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<DecisionBody>,
) -> Response {
    let decision = match ApprovalDecision::parse(&body.decision) {
        Ok(decision) => decision,
        Err(err) => return admin_error(err),
    };
    let approver =
        body.approver.as_deref().filter(|value| !value.trim().is_empty()).unwrap_or("operator");

    let result = async {
        let task =
            state.service.decide_approval(&id, decision, approver, body.reason.as_deref()).await?;
        let approval = state.service.get_approval(&id).await?;
        Ok::<Value, Error>(json!({ "task": task, "approval": approval }))
    }
    .await;
    ok_or_admin(result)
}

/// `GET /agents`: the registered remote agents.
pub async fn list_agents(State(state): State<AppState>) -> Response {
    ok_or_admin(state.service.list_agents().await.map(|agents| json!({ "agents": agents })))
}

/// `GET /audit/export`: all audit entries with hash chain (Phase 5).
pub async fn export_audit(State(state): State<AppState>) -> Response {
    ok_or_admin(
        state
            .service
            .list_all_audit()
            .await
            .map(|entries| json!({ "audit": entries })),
    )
}

/// `GET /audit/verify`: verifies the hash chain integrity (Phase 5).
pub async fn verify_audit(State(state): State<AppState>) -> Response {
    let result = async {
        let entries = state.service.list_all_audit().await?;
        let mut previous_hash: Option<String> = None;
        let mut broken_at: Option<usize> = None;

        for (index, entry) in entries.iter().enumerate() {
            // Verify previous_hash matches the previous entry's hash
            if entry.previous_hash.as_deref() != previous_hash.as_deref() {
                broken_at = Some(index);
                break;
            }

            // Recompute the hash for this entry
            let mut hasher = Sha256::new();
            hasher.update(entry.previous_hash.as_deref().unwrap_or(""));
            hasher.update(b"||");
            hasher.update(entry.ts.as_bytes());
            hasher.update(b"||");
            hasher.update(entry.actor.as_bytes());
            hasher.update(b"||");
            hasher.update(entry.action.as_bytes());
            hasher.update(b"||");
            hasher.update(entry.payload_json.as_deref().unwrap_or("").as_bytes());
            let expected_hash = hex::encode(hasher.finalize());

            if entry.hash.as_deref() != Some(&expected_hash).map(|s| s.as_str()) {
                broken_at = Some(index);
                break;
            }

            previous_hash = entry.hash.clone();
        }

        Ok(json!({
            "valid": broken_at.is_none(),
            "entries": entries.len(),
            "brokenAt": broken_at,
            "lastHash": previous_hash,
        }))
    }
    .await;
    ok_or_admin(result)
}

/// `GET /traffic/stream`: Server-Sent Events with live task stream events (Phase 5).
pub async fn traffic_stream(
    State(state): State<AppState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, axum::Error>>> {
    let rx = state.service.subscribe_events();
    let stream = BroadcastStream::new(rx);
    let sse_stream = stream.map(|result| {
        result
            .map(|event| {
                let json = serde_json::to_string(&event).unwrap_or_default();
                Event::default().data(json)
            })
            .map_err(|err| {
                axum::Error::new(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    err.to_string(),
                ))
            })
    });
    Sse::new(sse_stream)
}

/// `GET /search?q=...`: global search across tasks, audit, and approvals (Phase 5).
pub async fn search(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Response {
    let q = match query.q {
        Some(ref q) if !q.is_empty() => q.clone(),
        _ => {
            return admin_error(Error::Validation("missing search query parameter 'q'".to_owned()))
        }
    };
    ok_or_admin(state.service.search(&q).await)
}

/// `POST /tasks/:id/replay`: forensic replay of a task's original message (Phase 5).
pub async fn replay_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ReplayBody>,
) -> Response {
    let actor = body.actor.unwrap_or_else(|| "operator".to_owned());
    ok_or_admin(state.service.replay_task(&id, &actor).await.map(|task| json!({ "task": task })))
}

/// `POST /messages/:id/resend`: selective re-send of a stored message (Phase 5).
pub async fn resend_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ResendBody>,
) -> Response {
    let actor = body.actor.unwrap_or_else(|| "operator".to_owned());
    ok_or_admin(
        state.service.resend_message(&id, &actor).await.map(|task| json!({ "task": task })),
    )
}

/// `POST /pii/mask`: masks PII in arbitrary text (Phase 5).
pub async fn mask_pii(
    State(_state): State<AppState>,
    Json(body): Json<MaskPiiBody>,
) -> Response {
    let masked = a2a_warden_core::pii::mask_pii(&body.text);
    ok_or_admin(Ok(json!({ "original": body.text, "masked": masked })))
}

/// Body of `POST /tasks/:id/replay`.
#[derive(Debug, Deserialize)]
pub struct ReplayBody {
    /// Identity of the human supervisor triggering the replay.
    pub actor: Option<String>,
}

/// Body of `POST /messages/:id/resend`.
#[derive(Debug, Deserialize)]
pub struct ResendBody {
    /// Identity of the human supervisor triggering the re-send.
    pub actor: Option<String>,
}

/// Body of `POST /pii/mask`.
#[derive(Debug, Deserialize)]
pub struct MaskPiiBody {
    /// Text to mask PII from.
    pub text: String,
}

/// Filters accepted by `GET /search`.
#[derive(Debug, Default, Deserialize)]
pub struct SearchQuery {
    /// Free-text search query.
    pub q: Option<String>,
}

/// Static-token guard for the administration API (legacy authentication; OIDC
/// arrives in phase 6).
pub async fn require_admin_token(
    State(state): State<AppState>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let Some(expected) = state.admin.token.as_deref() else {
        // No token configured: the API is open. Loud enough to show up in logs.
        tracing::warn!(
            "administration API is unauthenticated: set server token `admin.token` to protect it"
        );
        return next.run(request).await;
    };

    let provided = request
        .headers()
        .get(state.admin.token_header.as_str())
        .and_then(|value| value.to_str().ok());

    match provided {
        Some(actual) if actual == expected => next.run(request).await,
        _ => (StatusCode::UNAUTHORIZED, Json(json!({ "error": "missing or invalid admin token" })))
            .into_response(),
    }
}
