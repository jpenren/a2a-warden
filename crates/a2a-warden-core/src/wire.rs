//! Conversion between the version independent domain types and the two wire
//! dialects spoken by A2A agents.
//!
//! A2A v1.0 is the canonical protocol of this build, but many deployed agents
//! still speak v0.3. The two dialects differ in the encoding of enums (v1.0
//! uses the protobuf JSON names such as `TASK_STATE_COMPLETED`, v0.3 uses the
//! lower case specification names), in the tagging of messages and parts
//! (v0.3 carries a `kind` discriminator, v1.0 does not tag parts at all), and
//! in the shape of the push notification configuration.
//!
//! The domain types stay version independent; the transport picks the right
//! encoder at the boundary with [`ProtocolVersion`].

use serde_json::{json, Map, Value};

use crate::protocol::{
    Artifact, ListTasksResult, Message, Part, ProtocolVersion, PushNotificationAuthentication,
    PushNotificationConfig, StreamEvent, Task, TaskPushNotificationConfig, TaskStatus,
};

/// Converts a task to its wire representation.
#[must_use]
pub fn task_value(task: &Task, version: ProtocolVersion) -> Value {
    let mut value = Map::new();
    if version.is_legacy() {
        value.insert("kind".to_owned(), json!("task"));
    }
    value.insert("id".to_owned(), json!(task.id));
    value.insert("contextId".to_owned(), json!(task.context_id));
    value.insert("status".to_owned(), status_value(&task.status, version));
    if let Some(artifacts) = &task.artifacts {
        value.insert(
            "artifacts".to_owned(),
            Value::Array(artifacts.iter().map(|a| artifact_value(a, version)).collect()),
        );
    }
    if let Some(history) = &task.history {
        value.insert(
            "history".to_owned(),
            Value::Array(history.iter().map(|m| message_value(m, version)).collect()),
        );
    }
    if let Some(metadata) = &task.metadata {
        value.insert("metadata".to_owned(), Value::Object(metadata.clone()));
    }
    Value::Object(value)
}

/// Converts a task status to its wire representation.
#[must_use]
pub fn status_value(status: &TaskStatus, version: ProtocolVersion) -> Value {
    let mut value = Map::new();
    let state = if version.is_legacy() { status.state.wire_v03() } else { status.state.wire_v1() };
    value.insert("state".to_owned(), json!(state));
    if let Some(message) = &status.message {
        value.insert("message".to_owned(), message_value(message, version));
    }
    value.insert("timestamp".to_owned(), json!(status.timestamp));
    Value::Object(value)
}

/// Converts a message to its wire representation.
#[must_use]
pub fn message_value(message: &Message, version: ProtocolVersion) -> Value {
    let mut value = Map::new();
    if version.is_legacy() {
        value.insert("kind".to_owned(), json!("message"));
    }
    value.insert("messageId".to_owned(), json!(message.message_id));
    let role = if version.is_legacy() { message.role.wire_v03() } else { message.role.wire_v1() };
    value.insert("role".to_owned(), json!(role));
    value.insert(
        "parts".to_owned(),
        Value::Array(message.parts.iter().map(|p| part_value(p, version)).collect()),
    );
    if let Some(context_id) = &message.context_id {
        value.insert("contextId".to_owned(), json!(context_id));
    }
    if let Some(task_id) = &message.task_id {
        value.insert("taskId".to_owned(), json!(task_id));
    }
    if let Some(metadata) = &message.metadata {
        value.insert("metadata".to_owned(), Value::Object(metadata.clone()));
    }
    Value::Object(value)
}

/// Converts a content part to its wire representation.
#[must_use]
pub fn part_value(part: &Part, version: ProtocolVersion) -> Value {
    match part {
        Part::Text { text } => {
            if version.is_legacy() {
                json!({ "kind": "text", "text": text })
            } else {
                json!({ "text": text })
            }
        }
        Part::Data { data } => {
            if version.is_legacy() {
                json!({ "kind": "data", "data": data })
            } else {
                json!({ "data": data })
            }
        }
        Part::File { file } => {
            if version.is_legacy() {
                let mut payload = Map::new();
                if let Some(name) = &file.name {
                    payload.insert("name".to_owned(), json!(name));
                }
                if let Some(bytes) = &file.bytes {
                    payload.insert("bytes".to_owned(), json!(bytes));
                }
                if let Some(uri) = &file.uri {
                    payload.insert("uri".to_owned(), json!(uri));
                }
                if let Some(mime_type) = &file.mime_type {
                    payload.insert("mimeType".to_owned(), json!(mime_type));
                }
                json!({ "kind": "file", "file": Value::Object(payload) })
            } else {
                let mut payload = Map::new();
                if let Some(bytes) = &file.bytes {
                    payload.insert("raw".to_owned(), json!(bytes));
                }
                if let Some(uri) = &file.uri {
                    payload.insert("url".to_owned(), json!(uri));
                }
                if let Some(name) = &file.name {
                    payload.insert("filename".to_owned(), json!(name));
                }
                if let Some(mime_type) = &file.mime_type {
                    payload.insert("mediaType".to_owned(), json!(mime_type));
                }
                Value::Object(payload)
            }
        }
    }
}

/// Converts an artifact to its wire representation.
#[must_use]
pub fn artifact_value(artifact: &Artifact, version: ProtocolVersion) -> Value {
    let mut value = Map::new();
    value.insert("artifactId".to_owned(), json!(artifact.artifact_id));
    if let Some(name) = &artifact.name {
        value.insert("name".to_owned(), json!(name));
    }
    value.insert(
        "parts".to_owned(),
        Value::Array(artifact.parts.iter().map(|p| part_value(p, version)).collect()),
    );
    if let Some(metadata) = &artifact.metadata {
        value.insert("metadata".to_owned(), Value::Object(metadata.clone()));
    }
    Value::Object(value)
}

/// Encodes the result member of a `SendMessage` / `SendStreamingMessage`
/// response.
///
/// v1.0 wraps the task in a `SendMessageResponse` (`{"task": ...}`); v0.3
/// returns the task itself.
#[must_use]
pub fn send_result(task: &Task, version: ProtocolVersion) -> Value {
    if version.is_legacy() {
        task_value(task, version)
    } else {
        json!({ "task": task_value(task, version) })
    }
}

/// Encodes a `tasks/list` result.
#[must_use]
pub fn list_tasks_result(result: &ListTasksResult, version: ProtocolVersion) -> Value {
    let mut value = Map::new();
    value.insert(
        "tasks".to_owned(),
        Value::Array(result.tasks.iter().map(|task| task_value(task, version)).collect()),
    );
    if let Some(token) = &result.next_page_token {
        value.insert("nextPageToken".to_owned(), json!(token));
    }
    if let Some(page_size) = result.page_size {
        value.insert("pageSize".to_owned(), json!(page_size));
    }
    if let Some(total_size) = result.total_size {
        value.insert("totalSize".to_owned(), json!(total_size));
    }
    Value::Object(value)
}

/// Encodes the result member of a streaming frame.
///
/// v1.0 uses the `StreamResponse` oneof wrappers (`task`, `message`,
/// `statusUpdate`, `artifactUpdate`) and no `final` flag; v0.3 uses the
/// `kind`-tagged bare events and a mandatory `final` flag on status updates.
#[must_use]
pub fn stream_event_result(event: &StreamEvent, version: ProtocolVersion) -> Value {
    match event {
        StreamEvent::Task(task) => {
            let task = task_value(task, version);
            if version.is_legacy() {
                task
            } else {
                json!({ "task": task })
            }
        }
        StreamEvent::Message(message) => {
            let message = message_value(message, version);
            if version.is_legacy() {
                message
            } else {
                json!({ "message": message })
            }
        }
        StreamEvent::StatusUpdate(event) => {
            let mut value = Map::new();
            if version.is_legacy() {
                value.insert("kind".to_owned(), json!("status-update"));
            }
            value.insert("taskId".to_owned(), json!(event.task_id));
            value.insert("contextId".to_owned(), json!(event.context_id));
            value.insert("status".to_owned(), status_value(&event.status, version));
            if version.is_legacy() {
                value.insert("final".to_owned(), json!(event.final_event));
            }
            let value = Value::Object(value);
            if version.is_legacy() {
                value
            } else {
                json!({ "statusUpdate": value })
            }
        }
        StreamEvent::ArtifactUpdate(event) => {
            let mut value = Map::new();
            if version.is_legacy() {
                value.insert("kind".to_owned(), json!("artifact-update"));
            }
            value.insert("taskId".to_owned(), json!(event.task_id));
            value.insert("contextId".to_owned(), json!(event.context_id));
            value.insert("artifact".to_owned(), artifact_value(&event.artifact, version));
            if event.append {
                value.insert("append".to_owned(), json!(true));
            }
            if event.last_chunk {
                value.insert("lastChunk".to_owned(), json!(true));
            }
            let value = Value::Object(value);
            if version.is_legacy() {
                value
            } else {
                json!({ "artifactUpdate": value })
            }
        }
        StreamEvent::Error { code, message } => {
            json!({ "code": code, "message": message })
        }
    }
}

/// Encodes the authentication block of a push configuration.
#[must_use]
fn authentication_value(
    authentication: &PushNotificationAuthentication,
    version: ProtocolVersion,
) -> Value {
    if version.is_legacy() {
        json!({
            "schemes": authentication.schemes,
            "credentials": authentication.credentials,
        })
    } else {
        json!({
            "scheme": authentication.schemes.first(),
            "credentials": authentication.credentials,
        })
    }
}

/// Encodes a push notification configuration in the v1.0 flat shape.
#[must_use]
pub fn push_config_value(
    task_id: &str,
    config: &PushNotificationConfig,
    version: ProtocolVersion,
) -> Value {
    if version.is_legacy() {
        json!({
            "taskId": task_id,
            "pushNotificationConfig": {
                "id": config.id,
                "url": config.url,
                "token": config.token,
                "authentication": config.authentication.as_ref()
                    .map(|auth| authentication_value(auth, version)),
            },
        })
    } else {
        json!({
            "taskId": task_id,
            "id": config.id,
            "url": config.url,
            "token": config.token,
            "authentication": config.authentication.as_ref()
                .map(|auth| authentication_value(auth, version)),
        })
    }
}

/// Encodes a task push notification configuration.
#[must_use]
pub fn task_push_config_value(
    config: &TaskPushNotificationConfig,
    version: ProtocolVersion,
) -> Value {
    push_config_value(&config.task_id, &config.push_notification_config, version)
}

/// Encodes a list of push configurations (`configs` + `nextPageToken` for
/// v1.0, a bare array for v0.3).
#[must_use]
pub fn push_config_list_value(
    configs: &[TaskPushNotificationConfig],
    next_page_token: Option<&str>,
    version: ProtocolVersion,
) -> Value {
    if version.is_legacy() {
        Value::Array(configs.iter().map(|config| task_push_config_value(config, version)).collect())
    } else {
        let mut value = Map::new();
        value.insert(
            "configs".to_owned(),
            Value::Array(
                configs.iter().map(|config| task_push_config_value(config, version)).collect(),
            ),
        );
        if let Some(token) = next_page_token {
            value.insert("nextPageToken".to_owned(), json!(token));
        }
        Value::Object(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{FileContent, Role, TaskState};

    fn sample_task() -> Task {
        Task {
            id: "task-1".to_owned(),
            context_id: "ctx-1".to_owned(),
            status: TaskStatus {
                state: TaskState::InputRequired,
                message: Some(Message {
                    message_id: "status-1".to_owned(),
                    role: Role::Agent,
                    parts: vec![Part::Text { text: "approval required".to_owned() }],
                    context_id: Some("ctx-1".to_owned()),
                    task_id: Some("task-1".to_owned()),
                    metadata: None,
                }),
                timestamp: "2026-01-01T00:00:00.000Z".to_owned(),
            },
            artifacts: Some(vec![Artifact {
                artifact_id: "art-1".to_owned(),
                name: Some("result".to_owned()),
                parts: vec![Part::Data { data: json!({"answer": 42}) }],
                metadata: None,
            }]),
            history: None,
            metadata: None,
        }
    }

    #[test]
    fn v1_tasks_use_protobuf_enums_and_untagged_parts() {
        let value = task_value(&sample_task(), ProtocolVersion::V1);
        assert_eq!(value["status"]["state"], "TASK_STATE_INPUT_REQUIRED");
        assert_eq!(value["status"]["message"]["role"], "ROLE_AGENT");
        assert_eq!(value["status"]["message"]["parts"][0], json!({"text": "approval required"}));
        assert_eq!(value["artifacts"][0]["parts"][0], json!({"data": {"answer": 42}}));
        assert!(value.get("kind").is_none());
    }

    #[test]
    fn v03_tasks_are_kind_tagged() {
        let value = task_value(&sample_task(), ProtocolVersion::V03);
        assert_eq!(value["kind"], "task");
        assert_eq!(value["status"]["state"], "input-required");
        assert_eq!(value["status"]["message"]["kind"], "message");
        assert_eq!(
            value["status"]["message"]["parts"][0],
            json!({"kind": "text", "text": "approval required"})
        );
        assert_eq!(value["artifacts"][0]["parts"][0]["kind"], "data");
    }

    #[test]
    fn files_follow_each_dialect() {
        let part = Part::File {
            file: FileContent {
                name: Some("report.txt".to_owned()),
                bytes: Some("aGVsbG8=".to_owned()),
                uri: None,
                mime_type: Some("text/plain".to_owned()),
            },
        };
        assert_eq!(
            part_value(&part, ProtocolVersion::V1),
            json!({"raw": "aGVsbG8=", "filename": "report.txt", "mediaType": "text/plain"})
        );
        assert_eq!(
            part_value(&part, ProtocolVersion::V03),
            json!({
                "kind": "file",
                "file": {"name": "report.txt", "bytes": "aGVsbG8=", "mimeType": "text/plain"}
            })
        );
    }

    #[test]
    fn streaming_frames_wrap_v1_events_and_tag_v03_events() {
        let event = StreamEvent::StatusUpdate(crate::protocol::TaskStatusUpdateEvent {
            task_id: "task-1".to_owned(),
            context_id: "ctx-1".to_owned(),
            status: TaskStatus {
                state: TaskState::Completed,
                message: None,
                timestamp: "2026-01-01T00:00:00.000Z".to_owned(),
            },
            final_event: true,
        });
        let v1 = stream_event_result(&event, ProtocolVersion::V1);
        assert_eq!(v1["statusUpdate"]["status"]["state"], "TASK_STATE_COMPLETED");
        assert!(v1["statusUpdate"].get("final").is_none());

        let v03 = stream_event_result(&event, ProtocolVersion::V03);
        assert_eq!(v03["kind"], "status-update");
        assert_eq!(v03["final"], true);
    }

    #[test]
    fn send_message_v1_wraps_the_task() {
        let task = sample_task();
        let v1 = send_result(&task, ProtocolVersion::V1);
        assert_eq!(v1["task"]["id"], "task-1");
        let v03 = send_result(&task, ProtocolVersion::V03);
        assert_eq!(v03["id"], "task-1");
    }

    #[test]
    fn push_configurations_follow_each_dialect() {
        let config = TaskPushNotificationConfig {
            task_id: "task-1".to_owned(),
            push_notification_config: PushNotificationConfig {
                id: Some("push-1".to_owned()),
                url: "https://example.com/hook".to_owned(),
                token: Some("secret".to_owned()),
                authentication: Some(PushNotificationAuthentication {
                    schemes: vec!["Bearer".to_owned()],
                    credentials: Some("abc".to_owned()),
                }),
            },
        };
        let v1 = task_push_config_value(&config, ProtocolVersion::V1);
        assert_eq!(v1["taskId"], "task-1");
        assert_eq!(v1["authentication"]["scheme"], "Bearer");

        let v03 = task_push_config_value(&config, ProtocolVersion::V03);
        assert_eq!(v03["taskId"], "task-1");
        assert_eq!(v03["pushNotificationConfig"]["url"], "https://example.com/hook");
        assert_eq!(v03["pushNotificationConfig"]["authentication"]["schemes"][0], "Bearer");
    }
}
