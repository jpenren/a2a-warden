//! A2A protocol endpoints: the JSON-RPC binding  and the published
//! Agent Card.
//!
//! The dispatcher speaks both wire dialects advertised by the Agent Card:
//!
//! * A2A v1.0 uses the canonical method names (`SendMessage`, `GetTask`,
//!   `ListTasks`, ...), `TASK_STATE_*` enums and untagged message parts.
//! * A2A v0.3 uses the legacy slash names (`message/send`, `tasks/get`, ...),
//!   lowercase states and `kind` tagged parts. A legacy method name always
//!   selects the legacy dialect, even when the client sent a v1.0 header,
//!   because deployed clients keep sending the old method names.
//!
//! `SendStreamingMessage` / `message/stream` and `SubscribeToTask` /
//! `tasks/resubscribe` reply with an SSE stream of complete JSON-RPC
//! responses; every other operation replies with one JSON document.

use std::convert::Infallible;

use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use a2a_warden_core::error::Error;
use a2a_warden_core::protocol::{
    failure, success, JsonRpcErrorObject, JsonRpcRequest, ListTasksParams, MessageSendParams,
    ProtocolVersion, PushNotificationAuthentication, PushNotificationConfig, StreamEvent,
    SubscribeToTaskParams, TasksCancelParams, TasksGetParams, A2A_VERSION_HEADER, JSONRPC_VERSION,
};
use a2a_warden_core::wire;

use crate::{rpc_error, AppState};

/// Bundle of push configuration parameters accepted by both dialects.
struct PushConfigParams {
    task_id: String,
    config: PushNotificationConfig,
}

/// Validates the `A2A-Version` header and selects the wire dialect.
///
/// A missing header is negotiated as v1.0 (the canonical version). A legacy
/// slash method forces v0.3 so that deployed clients keep working unchanged.
fn negotiate(headers: &HeaderMap, method: &str) -> Result<ProtocolVersion, Box<Response>> {
    let raw = headers.get(A2A_VERSION_HEADER).map(|value| value.to_str()).transpose();
    let raw = raw.map_err(|_| version_error("A2A-Version header is not valid ASCII"))?;
    let version = ProtocolVersion::from_header(raw).ok_or_else(|| {
        version_error(format!(
            "unsupported A2A-Version '{}', this build speaks 1.0 and 0.3",
            raw.unwrap_or_default()
        ))
    })?;
    Ok(if method.contains('/') { ProtocolVersion::V03 } else { version })
}

fn version_error(detail: impl std::fmt::Display) -> Box<Response> {
    let body = failure(None, JsonRpcErrorObject::invalid_request(detail.to_string()));
    Box::new((StatusCode::BAD_REQUEST, Json(body)).into_response())
}

fn response_of(body: impl serde::Serialize, version: ProtocolVersion) -> Response {
    let value = serde_json::to_value(body).unwrap_or(Value::Null);
    let mut response = (StatusCode::OK, Json(value)).into_response();
    if let Ok(header) = HeaderValue::from_str(version.as_str()) {
        response.headers_mut().insert(A2A_VERSION_HEADER, header);
    }
    response
}

fn result_response(id: Option<Value>, result: Value, version: ProtocolVersion) -> Response {
    response_of(success(id, result), version)
}

fn error_response(
    id: Option<Value>,
    error: JsonRpcErrorObject,
    version: ProtocolVersion,
) -> Response {
    response_of(failure(id, error), version)
}

fn invalid_params(err: impl std::fmt::Display) -> JsonRpcErrorObject {
    JsonRpcErrorObject::invalid_params(err.to_string())
}

/// Builds the SSE response of a streaming operation.
fn stream_response(
    events: mpsc::Receiver<StreamEvent>,
    id: Option<Value>,
    version: ProtocolVersion,
) -> Response {
    let stream = ReceiverStream::new(events).map(move |event| {
        let frame = match event {
            StreamEvent::Error { code, message } => {
                serde_json::to_string(&failure(id.clone(), JsonRpcErrorObject::new(code, message)))
                    .unwrap_or_default()
            }
            event => serde_json::to_string(&success(
                id.clone(),
                wire::stream_event_result(&event, version),
            ))
            .unwrap_or_default(),
        };
        Ok::<Event, Infallible>(Event::default().data(frame))
    });

    let mut response = Sse::new(stream).keep_alive(KeepAlive::default()).into_response();
    if let Ok(header) = HeaderValue::from_str(version.as_str()) {
        response.headers_mut().insert(A2A_VERSION_HEADER, header);
    }
    response
}

/// `POST /` and the alias routes: a JSON-RPC 2.0 dispatcher.
pub async fn jsonrpc(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let method = body.get("method").and_then(Value::as_str).unwrap_or_default();
    let version = match negotiate(&headers, method) {
        Ok(version) => version,
        Err(response) => return *response,
    };

    let request: JsonRpcRequest = match serde_json::from_value::<JsonRpcRequest>(body) {
        Ok(request) if request.jsonrpc == JSONRPC_VERSION => request,
        Ok(request) => {
            return error_response(
                request.id,
                JsonRpcErrorObject::invalid_request(format!(
                    "unsupported jsonrpc version '{}'",
                    request.jsonrpc
                )),
                version,
            );
        }
        Err(err) => {
            return error_response(
                None,
                JsonRpcErrorObject::parse_error(format!("invalid JSON-RPC request: {err}")),
                version,
            );
        }
    };

    let id = request.id.clone();
    let params = request.params.unwrap_or(Value::Null);
    let actor = request.method.clone();

    match request.method.as_str() {
        // ---------------------------------------------------------- messaging
        "message/send" | "SendMessage" => {
            match serde_json::from_value::<MessageSendParams>(params) {
                Err(err) => error_response(id, invalid_params(err), version),
                Ok(parsed) => match state.service.send_message(parsed).await {
                    Ok(task) => result_response(id, wire::send_result(&task, version), version),
                    Err(err) => error_response(id, rpc_error(err), version),
                },
            }
        }
        "message/stream" | "SendStreamingMessage" => {
            match serde_json::from_value::<MessageSendParams>(params) {
                Err(err) => error_response(id, invalid_params(err), version),
                Ok(parsed) => {
                    let (sender, receiver) = mpsc::channel(64);
                    let service = state.service.clone();
                    tokio::spawn(async move {
                        let _ = service.send_message_streaming(parsed, sender).await;
                    });
                    stream_response(receiver, id, version)
                }
            }
        }
        "tasks/resubscribe" | "SubscribeToTask" => {
            let history_length = params.get("historyLength").and_then(Value::as_u64);
            match serde_json::from_value::<SubscribeToTaskParams>(params) {
                Err(err) => error_response(id, invalid_params(err), version),
                Ok(parsed) => {
                    let (sender, receiver) = mpsc::channel(64);
                    let service = state.service.clone();
                    tokio::spawn(async move {
                        let _ = service.subscribe_task(&parsed.id, history_length, sender).await;
                    });
                    stream_response(receiver, id, version)
                }
            }
        }

        // -------------------------------------------------------------- tasks
        "tasks/get" | "GetTask" => match serde_json::from_value::<TasksGetParams>(params) {
            Err(err) => error_response(id, invalid_params(err), version),
            Ok(parsed) => match state.service.get_task(&parsed.id, parsed.history_length).await {
                Ok(task) => result_response(id, wire::task_value(&task, version), version),
                Err(err) => error_response(id, rpc_error(err), version),
            },
        },
        "tasks/cancel" | "CancelTask" => {
            match serde_json::from_value::<TasksCancelParams>(params) {
                Err(err) => error_response(id, invalid_params(err), version),
                Ok(parsed) => match state.service.cancel_task(&parsed.id, &actor).await {
                    Ok(task) => result_response(id, wire::task_value(&task, version), version),
                    Err(err) => error_response(id, rpc_error(err), version),
                },
            }
        }
        "tasks/list" | "ListTasks" => match serde_json::from_value::<ListTasksParams>(params) {
            Err(err) => error_response(id, invalid_params(err), version),
            Ok(parsed) => match state.service.list_tasks_page(&parsed).await {
                Ok(result) => {
                    result_response(id, wire::list_tasks_result(&result, version), version)
                }
                Err(err) => error_response(id, rpc_error(err), version),
            },
        },

        // ------------------------------------------------- push notifications
        "tasks/pushNotificationConfig/set" | "CreateTaskPushNotificationConfig" => {
            match parse_push_config(params, version) {
                Err(err) => error_response(id, err, version),
                Ok(parsed) => {
                    match state.service.create_push_config(&parsed.task_id, parsed.config).await {
                        Ok(config) => result_response(
                            id,
                            wire::task_push_config_value(&config, version),
                            version,
                        ),
                        Err(err) => error_response(id, rpc_error(err), version),
                    }
                }
            }
        }
        "tasks/pushNotificationConfig/get" | "GetTaskPushNotificationConfig" => {
            match parse_push_ref(&params, version, "pushNotificationConfigId") {
                Err(err) => error_response(id, err, version),
                Ok((task_id, config_id)) => {
                    match resolve_push_config(&state, &task_id, config_id.as_deref()).await {
                        Err(err) => error_response(id, rpc_error(err), version),
                        Ok(config_id) => {
                            match state.service.get_push_config(&task_id, &config_id).await {
                                Ok(config) => result_response(
                                    id,
                                    wire::task_push_config_value(&config, version),
                                    version,
                                ),
                                Err(err) => error_response(id, rpc_error(err), version),
                            }
                        }
                    }
                }
            }
        }
        "tasks/pushNotificationConfig/list" | "ListTaskPushNotificationConfigs" => {
            match parse_task_reference(&params, version) {
                Err(err) => error_response(id, err, version),
                Ok(task_id) => match state.service.list_push_configs(&task_id).await {
                    Err(err) => error_response(id, rpc_error(err), version),
                    Ok(configs) => {
                        let page_size = params
                            .get("pageSize")
                            .and_then(Value::as_u64)
                            .unwrap_or(50)
                            .clamp(1, 200);
                        let offset = params
                            .get("pageToken")
                            .and_then(Value::as_str)
                            .and_then(|token| token.parse::<usize>().ok())
                            .unwrap_or(0)
                            .min(configs.len());
                        let end = offset
                            .saturating_add(usize::try_from(page_size).unwrap_or(usize::MAX))
                            .min(configs.len());
                        let page = &configs[offset..end];
                        let next = (end < configs.len()).then(|| end.to_string());
                        result_response(
                            id,
                            wire::push_config_list_value(page, next.as_deref(), version),
                            version,
                        )
                    }
                },
            }
        }
        "tasks/pushNotificationConfig/delete" | "DeleteTaskPushNotificationConfig" => {
            match parse_push_ref(&params, version, "pushNotificationConfigId") {
                Err(err) => error_response(id, err, version),
                Ok((task_id, config_id)) => {
                    match resolve_push_config(&state, &task_id, config_id.as_deref()).await {
                        Err(err) => error_response(id, rpc_error(err), version),
                        Ok(config_id) => {
                            match state.service.delete_push_config(&task_id, &config_id).await {
                                Ok(()) => result_response(id, Value::Null, version),
                                Err(err) => error_response(id, rpc_error(err), version),
                            }
                        }
                    }
                }
            }
        }

        other => {
            error_response(id, JsonRpcErrorObject::method_not_found(other.to_owned()), version)
        }
    }
}

/// Parses a push notification configuration in either dialect.
fn parse_push_config(
    params: Value,
    version: ProtocolVersion,
) -> Result<PushConfigParams, JsonRpcErrorObject> {
    let task_id = params
        .get("taskId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid_params("missing taskId"))?
        .to_owned();

    let raw = if version.is_legacy() {
        params
            .get("pushNotificationConfig")
            .cloned()
            .ok_or_else(|| invalid_params("missing pushNotificationConfig"))?
    } else {
        params.clone()
    };

    let url = raw
        .get("url")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid_params("missing push notification url"))?
        .to_owned();
    let id = raw.get("id").and_then(Value::as_str).map(str::to_owned);
    let token = raw.get("token").and_then(Value::as_str).map(str::to_owned);
    let authentication = parse_authentication(raw.get("authentication"), version);

    Ok(PushConfigParams {
        task_id,
        config: PushNotificationConfig { id, url, token, authentication },
    })
}

/// Parses the authentication block of a push configuration.
fn parse_authentication(
    value: Option<&Value>,
    version: ProtocolVersion,
) -> Option<PushNotificationAuthentication> {
    let value = value?;
    let schemes = if version.is_legacy() {
        value
            .get("schemes")
            .and_then(Value::as_array)
            .map(|schemes| schemes.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default()
    } else {
        value
            .get("scheme")
            .and_then(Value::as_str)
            .map(|scheme| vec![scheme.to_owned()])
            .unwrap_or_default()
    };
    let credentials = value.get("credentials").and_then(Value::as_str).map(str::to_owned);
    Some(PushNotificationAuthentication { schemes, credentials })
}

/// Extracts the task identifier of a push operation.
///
/// v1.0 uses `taskId`; v0.3 uses `id` for the task and a separate
/// `pushNotificationConfigId` for the configuration.
fn parse_task_reference(
    params: &Value,
    version: ProtocolVersion,
) -> Result<String, JsonRpcErrorObject> {
    let key = if version.is_legacy() { "id" } else { "taskId" };
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| invalid_params(format!("missing {key}")))
}

/// Extracts the task and configuration identifiers of a get/delete call.
fn parse_push_ref(
    params: &Value,
    version: ProtocolVersion,
    config_key: &str,
) -> Result<(String, Option<String>), JsonRpcErrorObject> {
    let task_id = parse_task_reference(params, version)?;
    let config_id = if version.is_legacy() {
        params.get(config_key).and_then(Value::as_str).map(str::to_owned)
    } else {
        params.get("id").and_then(Value::as_str).map(str::to_owned)
    };
    Ok((task_id, config_id))
}

/// Resolves a missing configuration identifier to the only/first config.
async fn resolve_push_config(
    state: &AppState,
    task_id: &str,
    config_id: Option<&str>,
) -> Result<String, Error> {
    if let Some(id) = config_id {
        return Ok(id.to_owned());
    }
    state
        .service
        .list_push_configs(task_id)
        .await?
        .into_iter()
        .next()
        .and_then(|config| config.push_notification_config.id)
        .ok_or_else(|| {
            Error::NotFound(format!("no push notification configuration for task {task_id}"))
        })
}

/// `GET /` sends humans to the console; machines must `POST` JSON-RPC.
pub async fn redirect_to_console() -> Response {
    Redirect::temporary("/ui").into_response()
}

/// `GET /.well-known/agent-card.json` (and the legacy alias).
pub async fn agent_card(State(state): State<AppState>) -> Response {
    let value = serde_json::to_value(&*state.agent_card).unwrap_or(Value::Null);
    (StatusCode::OK, Json(value)).into_response()
}
