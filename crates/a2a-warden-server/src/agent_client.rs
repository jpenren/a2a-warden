//! HTTP client used by the warden when it delegates a task to a registered
//! remote agent . The warden speaks JSON-RPC to the remote agent, so
//! it validates its own wire format on the way out.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use a2a_warden_core::error::Error;
use a2a_warden_core::protocol::{
    Message, Task, TaskState, TasksCancelParams, A2A_VERSION_HEADER, DEFAULT_A2A_VERSION,
    JSONRPC_VERSION,
};
use a2a_warden_core::service::AgentClient;
use url::Url;

/// JSON-RPC error code for an unknown method.
const METHOD_NOT_FOUND: i64 = -32601;

/// A2A v1.0 method names, as implemented by the official SDKs.
const METHOD_SEND_MESSAGE: &str = "SendMessage";
const METHOD_CANCEL_TASK: &str = "CancelTask";

/// A2A v0.3 method names, kept as a fallback for agents of that vintage.
const METHOD_SEND_MESSAGE_V03: &str = "message/send";
const METHOD_CANCEL_TASK_V03: &str = "tasks/cancel";

/// [`AgentClient`] implementation backed by `reqwest` with rustls.
#[derive(Clone)]
pub struct ReqwestAgentClient {
    http: reqwest::Client,
}

impl ReqwestAgentClient {
    /// Builds a client with the timeout taken from configuration.
    pub fn new(timeout: Duration) -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(concat!("a2a-warden/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|err| Error::HttpClient(err.to_string()))?;
        Ok(Self { http })
    }

    /// Calls a method, falling back to its v0.3 spelling when the agent does
    /// not know the v1.0 one.
    ///
    /// The A2A v1.0 JSON-RPC binding renamed the operations (`SendMessage`
    /// instead of `message/send`); the official SDKs implement the v1.0
    /// names, while agents written against v0.3 only answer to the old ones.
    /// The warden therefore tries the canonical name first and retries with
    /// the legacy one when the agent reports "method not found".
    async fn rpc(
        &self,
        url: &Url,
        method: &str,
        legacy: &str,
        params: Value,
    ) -> Result<Value, Error> {
        match self.rpc_raw(url, method, params.clone()).await {
            Ok(result) => Ok(unwrap_result(result)),
            Err(failure) if failure.is_method_not_found() => {
                let result = self.rpc_raw(url, legacy, params).await.map_err(|err| err.error)?;
                Ok(unwrap_result(result))
            }
            Err(failure) => Err(failure.error),
        }
    }

    /// Sends a JSON-RPC call and returns the raw `result` member.
    ///
    /// The JSON-RPC error code is preserved so the caller can tell "this
    /// agent does not know the method" apart from a genuine failure.
    async fn rpc_raw(&self, url: &Url, method: &str, params: Value) -> Result<Value, RpcFailure> {
        let request_id = a2a_warden_core::new_id();
        let body = json!({
            "jsonrpc": JSONRPC_VERSION,
            "id": request_id,
            "method": method,
            "params": params,
        });
        let payload = serde_json::to_vec(&body)
            .map_err(|err| RpcFailure { code: None, error: Error::Serialization(err) })?;

        let response = self
            .http
            .post(url.clone())
            .header("content-type", "application/json")
            .header(A2A_VERSION_HEADER, DEFAULT_A2A_VERSION)
            .body(payload)
            .send()
            .await
            .map_err(|err| RpcFailure { code: None, error: Error::HttpClient(err.to_string()) })?;

        let status = response.status();
        let body: Value = response.json().await.map_err(|err| RpcFailure {
            code: None,
            error: Error::HttpClient(format!("invalid response ({status}): {err}")),
        })?;

        if let Some(rpc_error) = body.get("error") {
            let message = rpc_error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("remote agent reported an error");
            let code = rpc_error.get("code").and_then(Value::as_i64);
            let rendered = code.map_or_else(
                || format!("{method} failed on {url}: {message}"),
                |code| format!("{method} failed on {url} (code {code}): {message}"),
            );
            return Err(RpcFailure { code, error: Error::HttpClient(rendered) });
        }

        if !status.is_success() {
            return Err(RpcFailure {
                code: None,
                error: Error::HttpClient(format!("{method} failed on {url}: HTTP {status}")),
            });
        }

        body.get("result").cloned().ok_or_else(|| RpcFailure {
            code: None,
            error: Error::HttpClient(format!("{method} response has no result field")),
        })
    }
}

/// A failed JSON-RPC call, keeping the numeric error code.
struct RpcFailure {
    /// JSON-RPC error code, when the agent reported one.
    code: Option<i64>,
    /// Rendered error.
    error: Error,
}

impl RpcFailure {
    /// Returns `true` when the agent does not implement the method.
    fn is_method_not_found(&self) -> bool {
        self.code == Some(METHOD_NOT_FOUND)
    }
}

/// Unwraps the payload of a JSON-RPC result.
///
/// The A2A v1.0 binding wraps the task in a `SendMessageResponse` (`{ "task":
/// ... }`), the v0.3 binding wraps it in `{ "id": ..., "result": ... }`, and
/// some agents answer with the task itself. All three shapes are accepted so
/// the warden interoperates with agents of either vintage.
fn unwrap_result(result: Value) -> Value {
    if let Some(task) = result.get("task") {
        return task.clone();
    }
    let Some(inner) = result.get("result") else {
        return result;
    };
    if result.get("status").is_some() {
        return result;
    }
    inner.clone()
}

#[async_trait]
impl AgentClient for ReqwestAgentClient {
    async fn send_message(&self, agent_url: &Url, message: &Message) -> Result<Task, Error> {
        let params = json!({ "message": message });
        let result =
            self.rpc(agent_url, METHOD_SEND_MESSAGE, METHOD_SEND_MESSAGE_V03, params).await?;
        serde_json::from_value::<Task>(result).map_err(|err| {
            Error::HttpClient(format!("invalid task returned by {agent_url}: {err}"))
        })
    }

    async fn cancel_task(&self, agent_url: &Url, task_id: &str) -> Result<TaskState, Error> {
        let params = TasksCancelParams { id: task_id.to_owned() };
        let result = self
            .rpc(
                agent_url,
                METHOD_CANCEL_TASK,
                METHOD_CANCEL_TASK_V03,
                serde_json::to_value(params)?,
            )
            .await?;
        serde_json::from_value::<Task>(result).map(|task| task.status.state).map_err(|err| {
            Error::HttpClient(format!("invalid task returned by {agent_url}: {err}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_v1_result_is_returned_untouched() {
        let result = json!({ "id": "t1", "status": { "state": "completed" } });
        assert_eq!(unwrap_result(result.clone()), result);
    }

    #[test]
    fn a_v03_result_is_unwrapped() {
        let result = json!({
            "id": "rpc-1",
            "result": { "id": "t1", "status": { "state": "completed" } },
        });
        let unwrapped = unwrap_result(result);
        assert_eq!(unwrapped["id"], "t1");
        assert_eq!(unwrapped["status"]["state"], "completed");
    }

    #[test]
    fn a_send_message_response_is_unwrapped() {
        let result = json!({
            "task": { "id": "t1", "status": { "state": "TASK_STATE_COMPLETED" } },
        });
        let unwrapped = unwrap_result(result);
        assert_eq!(unwrapped["id"], "t1");
        assert_eq!(unwrapped["status"]["state"], "TASK_STATE_COMPLETED");
    }
}
