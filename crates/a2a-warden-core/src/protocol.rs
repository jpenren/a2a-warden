//! A2A protocol types used by the warden.
//!
//! The domain types are version independent and use the specification encoding
//! (camelCase). The A2A v1.0 and v0.3 wire dialects differ in the encoding of
//! enums and message parts; the conversion happens in [`crate::wire`] at the
//! transport boundary. The types are validated against the official Python SDK
//! on the wire (decision AD-3).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// JSON object used by `metadata`, `data` parts, and artifacts.
pub type JsonObject = Map<String, Value>;

/// Header carrying the A2A protocol version on every request.
pub const A2A_VERSION_HEADER: &str = "A2A-Version";
/// Protocol version supported by this build.
pub const DEFAULT_A2A_VERSION: &str = "1.0";
/// Legacy protocol version still spoken by many deployed agents.
pub const A2A_VERSION_0_3: &str = "0.3";
/// JSON-RPC version literal.
pub const JSONRPC_VERSION: &str = "2.0";

/// Wire dialect selected for a request or a response.
///
/// The domain types are version independent. Version `1.0` and `0.3` differ
/// in the encoding of enum values, of the message parts, and of the JSON-RPC
/// method names, so the conversion happens at the transport boundary (see
/// [`crate::wire`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolVersion {
    /// A2A v1.0, the canonical protocol of this build.
    V1,
    /// A2A v0.3, kept for interoperability with deployed agents.
    V03,
}

impl ProtocolVersion {
    /// Parses the value of the `A2A-Version` header.
    ///
    /// A missing header is negotiated as v1.0, as required by the
    /// specification. Only the two advertised versions are accepted.
    #[must_use]
    pub fn from_header(value: Option<&str>) -> Option<Self> {
        let Some(raw) = value else {
            return Some(Self::V1);
        };
        let raw = raw.trim();
        if raw.is_empty() {
            return Some(Self::V1);
        }
        if raw == "1" || raw.starts_with("1.") {
            return Some(Self::V1);
        }
        if raw == "0.3" || raw.starts_with("0.3.") {
            return Some(Self::V03);
        }
        None
    }

    /// Returns `true` when this is the legacy dialect.
    #[must_use]
    pub const fn is_legacy(self) -> bool {
        matches!(self, Self::V03)
    }

    /// Stable representation used in logs.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::V1 => DEFAULT_A2A_VERSION,
            Self::V03 => A2A_VERSION_0_3,
        }
    }
}

/// Task lifecycle states as defined by the A2A specification.
///
/// The wire encoding tolerates the spellings used by the official SDKs: the
/// kebab-case form of the specification (`input-required`), the enum name of
/// the protobuf based SDKs (`TASK_STATE_INPUT_REQUIRED`) and the upper case
/// form of the v0.3 agents (`INPUT_REQUIRED`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskState {
    /// The task was accepted and has not been worked on yet.
    Submitted,
    /// The task is waiting for input (the warden's interception state).
    InputRequired,
    /// The remote agent is working on the task.
    Working,
    /// The task finished successfully.
    Completed,
    /// The task was cancelled by a participant.
    Canceled,
    /// The task failed.
    Failed,
    /// The task was rejected by a human supervisor.
    Rejected,
    /// The task requires authentication before it can continue.
    AuthRequired,
    /// The task was paused by policy.
    Paused,
}

impl TaskState {
    /// Returns `true` when no further transition is expected.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Canceled | Self::Failed | Self::Rejected)
    }

    /// Canonical lowercase representation used for persistence.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Submitted => "submitted",
            Self::InputRequired => "input-required",
            Self::Working => "working",
            Self::Completed => "completed",
            Self::Canceled => "canceled",
            Self::Failed => "failed",
            Self::Rejected => "rejected",
            Self::AuthRequired => "auth-required",
            Self::Paused => "paused",
        }
    }

    /// v1.0 protobuf JSON representation (`TASK_STATE_COMPLETED`).
    ///
    /// `paused` is not part of the v1.0 enum; a paused task is advertised as
    /// `input-required`, which is the closest state a v1.0 client can handle.
    #[must_use]
    pub const fn wire_v1(self) -> &'static str {
        match self {
            Self::Submitted => "TASK_STATE_SUBMITTED",
            Self::InputRequired | Self::Paused => "TASK_STATE_INPUT_REQUIRED",
            Self::Working => "TASK_STATE_WORKING",
            Self::Completed => "TASK_STATE_COMPLETED",
            Self::Canceled => "TASK_STATE_CANCELED",
            Self::Failed => "TASK_STATE_FAILED",
            Self::Rejected => "TASK_STATE_REJECTED",
            Self::AuthRequired => "TASK_STATE_AUTH_REQUIRED",
        }
    }

    /// v0.3 JSON representation (lowercase, kebab-case for `input-required`).
    #[must_use]
    pub const fn wire_v03(self) -> &'static str {
        self.as_str()
    }

    /// Parses the canonical representation.
    ///
    /// The spellings of the official SDKs are accepted as well, so a task
    /// returned by an agent built on the protobuf based SDKs is understood.
    pub fn parse(value: &str) -> Option<Self> {
        let normalized = normalize_enum_name(value);
        Some(match normalized.as_str() {
            "submitted" => Self::Submitted,
            "input-required" => Self::InputRequired,
            "working" => Self::Working,
            "completed" => Self::Completed,
            "canceled" | "cancelled" => Self::Canceled,
            "failed" => Self::Failed,
            "rejected" => Self::Rejected,
            "auth-required" => Self::AuthRequired,
            "paused" => Self::Paused,
            _ => return None,
        })
    }
}

impl<'de> Deserialize<'de> for TaskState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown task state '{raw}'")))
    }
}

/// Normalizes the spellings of an enum value coming from the wire.
///
/// The A2A v1.0 JSON binding uses kebab-case (`input-required`), the
/// protobuf based SDKs use the enum name (`TASK_STATE_INPUT_REQUIRED`) and
/// the v0.3 agents use the upper case form (`INPUT_REQUIRED`).
fn normalize_enum_name(value: &str) -> String {
    let trimmed = value.trim();
    let stripped = trimmed
        .strip_prefix("TASK_STATE_")
        .or_else(|| trimmed.strip_prefix("ROLE_"))
        .unwrap_or(trimmed);
    stripped.replace('_', "-").to_ascii_lowercase()
}

impl std::str::FromStr for TaskState {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value).ok_or_else(|| format!("unknown task state '{value}'"))
    }
}

/// Author of a [`Message`].
///
/// The wire encoding tolerates the spellings used by the official SDKs: the
/// lower case form of the specification (`user`) and the enum name of the
/// protobuf based SDKs (`ROLE_USER`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// The human or client side of the conversation.
    User,
    /// The agent side of the conversation.
    Agent,
}

impl Role {
    /// Canonical representation used for persistence.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Agent => "agent",
        }
    }

    /// v1.0 protobuf JSON representation (`ROLE_USER`).
    #[must_use]
    pub const fn wire_v1(self) -> &'static str {
        match self {
            Self::User => "ROLE_USER",
            Self::Agent => "ROLE_AGENT",
        }
    }

    /// v0.3 JSON representation (`user`).
    #[must_use]
    pub const fn wire_v03(self) -> &'static str {
        self.as_str()
    }

    /// Parses the canonical representation.
    ///
    /// The spellings of the official SDKs are accepted as well, so a message
    /// returned by an agent built on the protobuf based SDKs is understood.
    pub fn parse(value: &str) -> Option<Self> {
        match normalize_enum_name(value).as_str() {
            "user" => Some(Self::User),
            "agent" => Some(Self::Agent),
            _ => None,
        }
    }
}

impl<'de> Deserialize<'de> for Role {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).ok_or_else(|| serde::de::Error::custom(format!("unknown role '{raw}'")))
    }
}

impl std::str::FromStr for Role {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value).ok_or_else(|| format!("unknown role '{value}'"))
    }
}

/// Reference to a file carried inline (base64) or by URI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileContent {
    /// Optional file name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Base64 encoded content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<String>,
    /// Remote URI of the content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    /// MIME type of the content.
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// A single content unit inside a [`Message`] or [`Artifact`].
///
/// The domain representation tags the parts with their kind (`{"type":
/// "text", "text": ...}`) for persistence. A2A v0.3 uses the same tagged form
/// with a `kind` discriminator, while the v1.0 protobuf JSON binding does not
/// tag them at all. The deserialization accepts every spelling and deduces the
/// kind from the field that is present; the transport encodes the right one
/// for the negotiated version (see [`crate::wire`]).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Part {
    /// Plain text content.
    Text {
        /// The text.
        text: String,
    },
    /// File content.
    File {
        /// The file payload.
        file: FileContent,
    },
    /// Structured data content.
    Data {
        /// The JSON payload.
        data: Value,
    },
}

impl<'de> Deserialize<'de> for Part {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        parse_part(&value).map_err(serde::de::Error::custom)
    }
}

/// Reads a part from its JSON representation.
///
/// The discriminator may be `type` (domain and v0.3 encodings) or `kind`
/// (v0.3), or absent (v1.0 protobuf JSON). The v1.0 file parts carry `raw`,
/// `url`, `filename`, and `mediaType` at the same level as the discriminator;
/// the v0.3 file parts nest the payload under `file`.
fn parse_part(value: &Value) -> Result<Part, String> {
    let object = value.as_object().ok_or_else(|| "a part must be a JSON object".to_owned())?;

    let discriminator = object.get("type").or_else(|| object.get("kind")).and_then(Value::as_str);

    match discriminator {
        Some("text") => match object.get("text") {
            Some(Value::String(text)) => Ok(Part::Text { text: text.clone() }),
            _ => Err("a text part must carry a text field".to_owned()),
        },
        Some("file") => {
            let file = object
                .get("file")
                .ok_or_else(|| "a file part must carry a file field".to_owned())?;
            Ok(Part::File { file: parse_file(file)? })
        }
        Some("data") => {
            let data = object
                .get("data")
                .ok_or_else(|| "a data part must carry a data field".to_owned())?;
            Ok(Part::Data { data: data.clone() })
        }
        Some(other) => Err(format!("unknown part type '{other}'")),
        // Untagged part: the kind follows from the field that is present.
        None => {
            if let Some(Value::String(text)) = object.get("text") {
                Ok(Part::Text { text: text.clone() })
            } else if let Some(file) = object.get("file") {
                Ok(Part::File { file: parse_file(file)? })
            } else if let Some(data) = object.get("data") {
                Ok(Part::Data { data: data.clone() })
            } else if object.contains_key("raw") || object.contains_key("url") {
                Ok(Part::File { file: parse_flat_file(object)? })
            } else {
                Err("a part must carry a text, file or data field".to_owned())
            }
        }
    }
}

/// Reads the payload of a file part.
fn parse_file(value: &Value) -> Result<FileContent, String> {
    let object =
        value.as_object().ok_or_else(|| "a file payload must be a JSON object".to_owned())?;
    let mut file: FileContent =
        serde_json::from_value(value.clone()).map_err(|err| format!("invalid file part: {err}"))?;
    // The v0.3 file payload uses `mimeType`; the v1.0 flat form uses
    // `mediaType`. Both are normalized to the domain `mime_type`.
    if file.mime_type.is_none() {
        file.mime_type = object
            .get("mediaType")
            .or_else(|| object.get("mimeType"))
            .and_then(Value::as_str)
            .map(str::to_owned);
    }
    Ok(file)
}

/// Reads a v1.0 flat file part (`raw`/`url` + `filename` + `mediaType`).
fn parse_flat_file(object: &Map<String, Value>) -> Result<FileContent, String> {
    Ok(FileContent {
        name: object.get("filename").and_then(Value::as_str).map(str::to_owned),
        bytes: object.get("raw").and_then(Value::as_str).map(str::to_owned),
        uri: object.get("url").and_then(Value::as_str).map(str::to_owned),
        mime_type: object
            .get("mediaType")
            .or_else(|| object.get("mimeType"))
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

impl Part {
    /// Returns the text carried by this part, if any.
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            _ => None,
        }
    }
}

/// An A2A message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    /// Unique message identifier.
    pub message_id: String,
    /// Author of the message.
    pub role: Role,
    /// Content parts.
    pub parts: Vec<Part>,
    /// Conversation context identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
    /// Associated task identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// Free-form metadata (target agent, skill, sender, ...).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonObject>,
}

impl Message {
    /// Builds a user message with a single text part.
    #[must_use]
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            message_id: crate::new_id(),
            role: Role::User,
            parts: vec![Part::Text { text: text.into() }],
            context_id: None,
            task_id: None,
            metadata: None,
        }
    }

    /// Builds an agent message with a single text part.
    #[must_use]
    pub fn agent_text(text: impl Into<String>) -> Self {
        Self {
            message_id: crate::new_id(),
            role: Role::Agent,
            parts: vec![Part::Text { text: text.into() }],
            context_id: None,
            task_id: None,
            metadata: None,
        }
    }

    /// Concatenates every text part of the message.
    #[must_use]
    pub fn text(&self) -> String {
        self.parts.iter().filter_map(Part::as_text).collect::<Vec<_>>().join("\n")
    }

    /// Returns a metadata string value.
    #[must_use]
    pub fn metadata_str(&self, key: &str) -> Option<String> {
        self.metadata.as_ref()?.get(key)?.as_str().map(str::to_owned)
    }
}

/// Status of a task at a point in time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatus {
    /// Current state.
    pub state: TaskState,
    /// Message explaining the state, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<Message>,
    /// RFC 3339 timestamp of the transition.
    pub timestamp: String,
}

/// A unit of work tracked by the warden.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    /// Task identifier.
    pub id: String,
    /// Conversation context identifier.
    pub context_id: String,
    /// Current status.
    pub status: TaskStatus,
    /// Produced artifacts, once completed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<Vec<Artifact>>,
    /// Request message history (populated by `tasks/get`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history: Option<Vec<Message>>,
    /// Free-form metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonObject>,
}

/// An output produced by a completed task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    /// Artifact identifier.
    pub artifact_id: String,
    /// Artifact name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Artifact content.
    pub parts: Vec<Part>,
    /// Free-form metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonObject>,
}

/// Options accepted by `message/send`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageSendConfiguration {
    /// Modes the client accepts in the response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accepted_output_modes: Option<Vec<String>>,
    /// Whether the client wants to block until the task reaches a final state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocking: Option<bool>,
    /// Number of history messages to return.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_length: Option<u64>,
}

/// Parameters of the `message/send` operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageSendParams {
    /// The message to process.
    pub message: Message,
    /// Optional send configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub configuration: Option<MessageSendConfiguration>,
}

/// Parameters of the `tasks/get` operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasksGetParams {
    /// Task identifier.
    pub id: String,
    /// Number of history messages to return.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_length: Option<u64>,
}

/// Parameters of the `tasks/cancel` operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasksCancelParams {
    /// Task identifier.
    pub id: String,
}

/// Parameters of the `tasks/list` operation (A2A v1.0).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListTasksParams {
    /// Only return tasks of this conversation context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
    /// Only return tasks in this state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TaskState>,
    /// Maximum number of tasks in the page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_size: Option<u32>,
    /// Opaque cursor returned by a previous call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_token: Option<String>,
    /// Number of history messages to include in each task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_length: Option<u64>,
    /// Only return tasks updated after this RFC 3339 timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_timestamp_after: Option<String>,
    /// Whether to include the artifacts of each task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_artifacts: Option<bool>,
}

/// Result of the `tasks/list` operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListTasksResult {
    /// Tasks in the page, newest first.
    pub tasks: Vec<Task>,
    /// Cursor of the next page, absent on the last page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_page_token: Option<String>,
    /// Effective page size.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_size: Option<u32>,
    /// Total number of tasks matching the filter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_size: Option<u64>,
}

/// Parameters of the `tasks/resubscribe` operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeToTaskParams {
    /// Task identifier.
    pub id: String,
}

/// Authentication details for a push notification endpoint.
///
/// v1.0 carries a single `scheme`, v0.3 a `schemes` list; the domain keeps the
/// list and the transport flattens it for v1.0 (see [`crate::wire`]).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushNotificationAuthentication {
    /// Supported authentication schemes (`Bearer`, `Basic`, ...).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schemes: Vec<String>,
    /// Credentials sent with the notification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<String>,
}

/// Configuration of a push notification endpoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushNotificationConfig {
    /// Client supplied configuration identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Callback URL.
    pub url: String,
    /// Validation token echoed in the `X-A2A-Notification-Token` header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// Optional authentication details.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authentication: Option<PushNotificationAuthentication>,
}

/// A push notification configuration associated with a task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskPushNotificationConfig {
    /// Owning task identifier.
    pub task_id: String,
    /// The configuration itself.
    pub push_notification_config: PushNotificationConfig,
}

/// A status change event emitted while streaming or resubscribing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatusUpdateEvent {
    /// Owning task identifier.
    pub task_id: String,
    /// Conversation context identifier.
    pub context_id: String,
    /// New status.
    pub status: TaskStatus,
    /// v0.3 only: whether this is the last event of the stream. v1.0 infers it
    /// from the terminal state, so the transport never serializes it for v1.0.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub final_event: bool,
}

/// An artifact produced or updated while streaming.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskArtifactUpdateEvent {
    /// Owning task identifier.
    pub task_id: String,
    /// Conversation context identifier.
    pub context_id: String,
    /// The artifact payload.
    pub artifact: Artifact,
    /// Whether the artifact content should be appended to a previous chunk.
    #[serde(default)]
    pub append: bool,
    /// Whether this is the last chunk of the artifact.
    #[serde(default)]
    pub last_chunk: bool,
}

/// Event flowing through the warden's task event bus and the SSE endpoints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StreamEvent {
    /// A complete task snapshot.
    Task(Task),
    /// A standalone agent or user message.
    Message(Message),
    /// A task status change.
    StatusUpdate(TaskStatusUpdateEvent),
    /// An artifact update.
    ArtifactUpdate(TaskArtifactUpdateEvent),
    /// A JSON-RPC error frame.
    Error {
        /// JSON-RPC error code.
        code: i64,
        /// Human readable error message.
        message: String,
    },
}

impl StreamEvent {
    /// Identifier of the task this event belongs to, when it has one.
    #[must_use]
    pub fn task_id(&self) -> Option<&str> {
        match self {
            Self::Task(task) => Some(&task.id),
            Self::Message(message) => message.task_id.as_deref(),
            Self::StatusUpdate(event) => Some(&event.task_id),
            Self::ArtifactUpdate(event) => Some(&event.task_id),
            Self::Error { .. } => None,
        }
    }
}

/// A protocol binding advertised in the Agent Card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInterface {
    /// Endpoint URL for this binding.
    pub url: String,
    /// Binding name (`JSONRPC`, `GRPC`, `HTTP+JSON`).
    pub protocol_binding: String,
    /// Protocol version spoken on this endpoint.
    pub protocol_version: String,
    /// Optional tenant identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
}

impl AgentInterface {
    /// Builds an A2A JSON-RPC interface entry.
    #[must_use]
    pub fn jsonrpc(url: &str, version: &str) -> Self {
        Self {
            url: url.to_owned(),
            protocol_binding: "JSONRPC".to_owned(),
            protocol_version: version.to_owned(),
            tenant: None,
        }
    }
}

/// A JSON-RPC 2.0 request as received by the warden.
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcRequest {
    /// JSON-RPC version, always `2.0`.
    pub jsonrpc: String,
    /// Request identifier (echoed back in the response).
    #[serde(default)]
    pub id: Option<Value>,
    /// Operation name.
    pub method: String,
    /// Operation parameters.
    #[serde(default)]
    pub params: Option<Value>,
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcErrorObject {
    /// JSON-RPC error code.
    pub code: i64,
    /// Human readable message.
    pub message: String,
    /// Optional structured detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcErrorObject {
    /// Builds an error object.
    #[must_use]
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), data: None }
    }

    /// `Invalid Request` (-32600).
    #[must_use]
    pub fn invalid_request(detail: impl Into<String>) -> Self {
        Self::new(-32600, detail)
    }

    /// `Method not found` (-32601).
    #[must_use]
    pub fn method_not_found(method: impl Into<String>) -> Self {
        Self::new(-32601, format!("method not found: {}", method.into()))
    }

    /// `Invalid params` (-32602).
    #[must_use]
    pub fn invalid_params(detail: impl Into<String>) -> Self {
        Self::new(-32602, detail)
    }

    /// `Parse error` (-32700).
    #[must_use]
    pub fn parse_error(detail: impl Into<String>) -> Self {
        Self::new(-32700, detail)
    }

    /// Application defined server error (-32000).
    #[must_use]
    pub fn server_error(detail: impl Into<String>) -> Self {
        Self::new(-32000, detail)
    }
}

/// JSON-RPC 2.0 success response.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcSuccess {
    /// JSON-RPC version.
    pub jsonrpc: &'static str,
    /// Echoed request identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    /// Operation result.
    pub result: Value,
}

/// JSON-RPC 2.0 error response.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcFailure {
    /// JSON-RPC version.
    pub jsonrpc: &'static str,
    /// Echoed request identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    /// Error detail.
    pub error: JsonRpcErrorObject,
}

/// Builds a JSON-RPC success response.
#[must_use]
pub fn success(id: Option<Value>, result: impl Serialize) -> JsonRpcSuccess {
    JsonRpcSuccess {
        jsonrpc: JSONRPC_VERSION,
        id,
        result: serde_json::to_value(result).unwrap_or(Value::Null),
    }
}

/// Builds a JSON-RPC error response.
#[must_use]
pub fn failure(id: Option<Value>, error: JsonRpcErrorObject) -> JsonRpcFailure {
    JsonRpcFailure { jsonrpc: JSONRPC_VERSION, id, error }
}

/// Declared capabilities of an agent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilities {
    /// Whether the agent supports streaming.
    #[serde(default)]
    pub streaming: bool,
    /// Whether the agent supports push notifications.
    #[serde(default)]
    pub push_notifications: bool,
    /// Whether the agent exposes state transition history.
    #[serde(default)]
    pub state_transition_history: bool,
    /// Whether the agent exposes an authenticated extended card.
    #[serde(default)]
    pub extended_agent_card: bool,
}

/// A skill advertised in an Agent Card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    /// Stable skill identifier.
    pub id: String,
    /// Human readable name.
    pub name: String,
    /// Human readable description.
    pub description: String,
    /// Search tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Example invocations.
    #[serde(default)]
    pub examples: Vec<String>,
}

/// Signed Agent Card (verification is implemented in phase 3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCard {
    /// Agent name.
    pub name: String,
    /// Human readable description.
    pub description: String,
    /// Endpoint the clients must call.
    pub url: String,
    /// Card version.
    pub version: String,
    /// Declared protocol version of the interface (legacy v0.3 field).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<String>,
    /// Interfaces advertised by the agent (A2A v1.0).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_interfaces: Vec<AgentInterface>,
    /// Declared capabilities.
    pub capabilities: AgentCapabilities,
    /// Advertised skills.
    #[serde(default)]
    pub skills: Vec<Skill>,
    /// Input modes accepted by the agent.
    #[serde(default)]
    pub default_input_modes: Vec<String>,
    /// Output modes produced by the agent.
    #[serde(default)]
    pub default_output_modes: Vec<String>,
    /// JWS signatures over the card (phase 3).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signatures: Option<Vec<Value>>,
}

impl AgentCard {
    /// Builds the Agent Card published by the warden itself.
    #[must_use]
    pub fn warden_card(name: &str, url: &str, version: &str) -> Self {
        Self {
            name: name.to_owned(),
            description: "See every message. Verify every agent. Approve every task.".to_owned(),
            url: url.to_owned(),
            version: version.to_owned(),
            protocol_version: Some(DEFAULT_A2A_VERSION.to_owned()),
            supported_interfaces: vec![
                AgentInterface::jsonrpc(url, DEFAULT_A2A_VERSION),
                AgentInterface::jsonrpc(url, A2A_VERSION_0_3),
            ],
            capabilities: AgentCapabilities {
                streaming: true,
                push_notifications: true,
                state_transition_history: true,
                extended_agent_card: false,
            },
            skills: Vec::new(),
            default_input_modes: vec!["text/plain".to_owned(), "application/json".to_owned()],
            default_output_modes: vec!["text/plain".to_owned(), "application/json".to_owned()],
            signatures: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tagged_part_is_read() {
        let part: Part =
            serde_json::from_str(r#"{"type":"text","text":"hello"}"#).expect("tagged part");
        assert_eq!(part, Part::Text { text: "hello".to_owned() });
    }

    #[test]
    fn an_untagged_part_is_read() {
        let part: Part = serde_json::from_str(r#"{"text":"hello"}"#).expect("untagged part");
        assert_eq!(part, Part::Text { text: "hello".to_owned() });
    }

    #[test]
    fn an_untagged_data_part_is_read() {
        let part: Part = serde_json::from_str(r#"{"data":{"a":1}}"#).expect("data part");
        assert_eq!(part, Part::Data { data: serde_json::json!({"a": 1}) });
    }

    #[test]
    fn a_part_without_content_is_rejected() {
        assert!(serde_json::from_str::<Part>(r#"{"foo":1}"#).is_err());
        assert!(serde_json::from_str::<Part>(r#"{"type":"text"}"#).is_err());
    }

    #[test]
    fn the_task_state_accepts_the_sdk_spellings() {
        assert_eq!(TaskState::parse("completed"), Some(TaskState::Completed));
        assert_eq!(TaskState::parse("TASK_STATE_COMPLETED"), Some(TaskState::Completed));
        assert_eq!(TaskState::parse("COMPLETED"), Some(TaskState::Completed));
        assert_eq!(TaskState::parse("TASK_STATE_INPUT_REQUIRED"), Some(TaskState::InputRequired));
        assert_eq!(TaskState::parse("TASK_STATE_CANCELED"), Some(TaskState::Canceled));
        assert_eq!(TaskState::parse("TASK_STATE_CANCELLED"), Some(TaskState::Canceled));
        assert_eq!(TaskState::parse("nonsense"), None);
    }

    #[test]
    fn the_role_accepts_the_sdk_spellings() {
        assert_eq!(Role::parse("user"), Some(Role::User));
        assert_eq!(Role::parse("ROLE_USER"), Some(Role::User));
        assert_eq!(Role::parse("agent"), Some(Role::Agent));
        assert_eq!(Role::parse("ROLE_AGENT"), Some(Role::Agent));
        assert_eq!(Role::parse("nonsense"), None);
    }

    #[test]
    fn a_task_state_is_read_from_the_sdk_spellings() {
        let state: TaskState =
            serde_json::from_str(r#""TASK_STATE_COMPLETED""#).expect("sdk state");
        assert_eq!(state, TaskState::Completed);
    }
}
