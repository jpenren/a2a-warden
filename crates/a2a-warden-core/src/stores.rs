//! Storage traits (decision AD-2).
//!
//! The traits are defined in the domain crate and implemented once in
//! `a2a-warden-store` on top of sqlx. No domain code depends on the active
//! backend: the backend is selected exclusively by configuration.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::{Error, StorageError};
use crate::protocol::{Role, TaskState};

/// Approval lifecycle. The `cancelled` value matches the reference data model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalStatus {
    /// Waiting for a human decision.
    Pending,
    /// A supervisor approved the task.
    Approved,
    /// A supervisor rejected the task.
    Rejected,
    /// The task was cancelled before a decision.
    Cancelled,
}

impl ApprovalStatus {
    /// Canonical representation used for persistence.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Cancelled => "cancelled",
        }
    }

    /// Parses the canonical representation.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "approved" => Some(Self::Approved),
            "rejected" => Some(Self::Rejected),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

impl std::str::FromStr for ApprovalStatus {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value).ok_or_else(|| format!("unknown approval status '{value}'"))
    }
}

/// A task row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRecord {
    /// Task identifier.
    pub id: String,
    /// Conversation context identifier.
    pub context_id: String,
    /// Name of the registered remote agent targeted by the task.
    pub agent_id: String,
    /// Current state.
    pub status: TaskState,
    /// RFC 3339 creation timestamp.
    pub created_at: String,
    /// RFC 3339 last update timestamp.
    pub updated_at: String,
    /// Serialized message metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_json: Option<String>,
    /// Identifier of the task on the remote agent side, once delegated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_task_id: Option<String>,
}

/// A message row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRecord {
    /// Row identifier owned by the warden.
    pub id: String,
    /// Protocol identifier of the message, as sent by the peer.
    pub message_id: String,
    /// Owning task identifier (empty for messages before a task exists).
    pub task_id: String,
    /// Conversation context identifier.
    pub context_id: String,
    /// Author role.
    pub role: Role,
    /// Serialized A2A parts array.
    pub parts_json: String,
    /// RFC 3339 creation timestamp.
    pub created_at: String,
}

/// An artifact produced by a task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRecord {
    /// Row identifier owned by the warden.
    pub id: String,
    /// Protocol identifier of the artifact.
    pub artifact_id: String,
    /// Owning task identifier.
    pub task_id: String,
    /// Optional artifact name.
    pub name: Option<String>,
    /// Serialized A2A parts array.
    pub parts_json: String,
    /// Serialized free-form metadata.
    pub metadata_json: Option<String>,
    /// RFC 3339 creation timestamp.
    pub created_at: String,
}

/// An approval row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRecord {
    /// Approval identifier.
    pub id: String,
    /// Task waiting for the decision.
    pub task_id: String,
    /// Decision status.
    pub status: ApprovalStatus,
    /// Who decided (human supervisor identity or token name).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
    /// RFC 3339 decision timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<String>,
    /// RFC 3339 expiry timestamp (phase 4 enforces it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    /// Reason given with the decision.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// RFC 3339 creation timestamp.
    pub created_at: String,
}

/// An audit log row with hash-chaining for tamper evidence (Phase 5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditRecord {
    /// Audit identifier.
    pub id: String,
    /// RFC 3339 timestamp.
    pub ts: String,
    /// Actor: sending agent, human supervisor, or `warden`.
    pub actor: String,
    /// Action name, e.g. `task.forwarded`.
    pub action: String,
    /// Related task identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// Related context identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
    /// Serialized payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_json: Option<String>,
    /// Hash of the previous audit entry (empty for the first entry).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_hash: Option<String>,
    /// SHA-256 hash of this entry (previous_hash || ts || actor || action || payload_json).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

/// A registered agent row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRecord {
    /// Agent identifier.
    pub id: String,
    /// Unique agent name.
    pub name: String,
    /// A2A endpoint URL.
    pub url: String,
    /// Serialized skills array.
    pub skills_json: String,
    /// Serialized Agent Card (phase 3 verification).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card_json: Option<String>,
    /// Whether the Agent Card has been verified.
    #[serde(default)]
    pub verified: bool,
    /// Verification error message if verification failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_error: Option<String>,
    /// Key ID used for verification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jwks_kid: Option<String>,
    /// RFC 3339 timestamp of verification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified_at: Option<String>,
    /// Issuer used for verification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_issuer: Option<String>,
    /// Algorithm used for verification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_alg: Option<String>,
    /// Whether the agent is quarantined due to verification failure.
    #[serde(default)]
    pub quarantined: bool,
    /// RFC 3339 creation timestamp.
    pub created_at: String,
}

/// Filter accepted by `TaskStore::list_tasks`.
#[derive(Debug, Clone, Default)]
pub struct TaskFilter {
    /// Filter by state.
    pub status: Option<TaskState>,
    /// Filter by target agent name.
    pub agent: Option<String>,
    /// Filter by conversation context.
    pub context_id: Option<String>,
    /// Only tasks updated strictly after this RFC 3339 timestamp.
    pub updated_after: Option<String>,
    /// Maximum number of rows (applied after filtering).
    pub limit: Option<u32>,
    /// Offset for pagination (applied after filtering/ordering).
    pub offset: Option<u32>,
}

/// Keyset cursor of the `(created_at, id)` task ordering.
///
/// Tasks are listed newest first, so the cursor points at the last task of the
/// previous page and the next page contains the rows that sort after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCursor {
    /// Creation timestamp of the last row of the previous page.
    pub created_at: String,
    /// Identifier of the last row of the previous page.
    pub id: String,
}

impl TaskCursor {
    /// Builds a cursor from the last task of a page.
    #[must_use]
    pub fn from_task(task: &TaskRecord) -> Self {
        Self { created_at: task.created_at.clone(), id: task.id.clone() }
    }

    /// Encodes the cursor as the opaque `pageToken` of `tasks/list`.
    #[must_use]
    pub fn encode(&self) -> String {
        format!("{}|{}", self.created_at, self.id)
    }

    /// Decodes a `pageToken`.
    pub fn decode(token: &str) -> Option<Self> {
        let (created_at, id) = token.split_once('|')?;
        if created_at.is_empty() || id.is_empty() {
            return None;
        }
        Some(Self { created_at: created_at.to_owned(), id: id.to_owned() })
    }
}

/// A persisted push notification configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushNotificationRecord {
    /// Configuration identifier.
    pub id: String,
    /// Owning task identifier.
    pub task_id: String,
    /// Callback URL.
    pub url: String,
    /// Validation token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// Serialized [`crate::protocol::PushNotificationAuthentication`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_json: Option<String>,
    /// RFC 3339 creation timestamp.
    pub created_at: String,
}

/// Persistence of tasks and messages.
#[async_trait]
pub trait TaskStore: Send + Sync {
    /// Inserts a new task. Fails with [`StorageError::Duplicate`] on collision.
    async fn create_task(&self, task: &TaskRecord) -> Result<(), Error>;
    /// Loads a task by identifier.
    async fn get_task(&self, id: &str) -> Result<TaskRecord, Error>;
    /// Updates the state and the `updated_at` timestamp of a task.
    async fn update_task_status(
        &self,
        id: &str,
        status: TaskState,
        updated_at: &str,
    ) -> Result<(), Error>;
    /// Records the identifier the remote agent gave to the task.
    async fn set_remote_task_id(&self, id: &str, remote_task_id: &str) -> Result<(), Error>;
    /// Lists tasks matching the filter, newest first.
    async fn list_tasks(&self, filter: &TaskFilter) -> Result<Vec<TaskRecord>, Error>;
    /// Lists one page of tasks matching the filter, newest first.
    ///
    /// The default implementation filters [`TaskStore::list_tasks`] in memory;
    /// the SQL backends override it with a keyset query.
    async fn list_tasks_page(
        &self,
        filter: &TaskFilter,
        cursor: Option<&TaskCursor>,
        limit: u32,
    ) -> Result<Vec<TaskRecord>, Error> {
        let mut tasks = self.list_tasks(filter).await?;
        tasks.sort_by(|a, b| b.created_at.cmp(&a.created_at).then_with(|| b.id.cmp(&a.id)));
        if let Some(cursor) = cursor {
            tasks.retain(|task| {
                task.created_at < cursor.created_at
                    || (task.created_at == cursor.created_at && task.id < cursor.id)
            });
        }
        tasks.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(tasks)
    }
    /// Counts the tasks matching the filter.
    async fn count_tasks(&self, filter: &TaskFilter) -> Result<u64, Error> {
        Ok(self.list_tasks(filter).await?.len() as u64)
    }
    /// Lists non-terminal tasks that were not updated after `cutoff`.
    async fn list_expired_tasks(&self, cutoff: &str) -> Result<Vec<TaskRecord>, Error> {
        Ok(self
            .list_tasks(&TaskFilter::default())
            .await?
            .into_iter()
            .filter(|task| !task.status.is_terminal() && task.updated_at.as_str() < cutoff)
            .collect())
    }
    /// Appends a message to the timeline.
    async fn append_message(&self, message: &MessageRecord) -> Result<(), Error>;
    /// Lists the messages of a task in creation order.
    async fn list_messages(&self, task_id: &str) -> Result<Vec<MessageRecord>, Error>;
    /// Appends an artifact produced by a task.
    async fn append_artifact(&self, artifact: &ArtifactRecord) -> Result<(), Error>;
    /// Lists the artifacts of a task in creation order.
    async fn list_artifacts(&self, task_id: &str) -> Result<Vec<ArtifactRecord>, Error>;
}

/// Persistence of human decisions.
#[async_trait]
pub trait ApprovalStore: Send + Sync {
    /// Inserts a pending approval.
    async fn create_approval(&self, approval: &ApprovalRecord) -> Result<(), Error>;
    /// Loads an approval by identifier.
    async fn get_approval(&self, id: &str) -> Result<ApprovalRecord, Error>;
    /// Loads the pending approval of a task, if any.
    async fn pending_approval_for_task(
        &self,
        task_id: &str,
    ) -> Result<Option<ApprovalRecord>, Error>;
    /// Lists approvals, optionally filtered by status, newest first.
    async fn count_approvals(&self, status: Option<ApprovalStatus>) -> Result<u64, Error> {
        Ok(self.list_approvals(status, None, None).await?.len() as u64)
    }
    async fn list_approvals(
        &self,
        status: Option<ApprovalStatus>,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<ApprovalRecord>, Error>;
    /// Records a decision. Fails with [`StorageError::ApprovalNotFound`] when the
    /// approval does not exist and with [`Error::Conflict`] when it is already decided.
    async fn decide_approval(
        &self,
        id: &str,
        status: ApprovalStatus,
        decided_by: &str,
        decided_at: &str,
        reason: Option<&str>,
    ) -> Result<ApprovalRecord, Error>;
}

/// Persistence of the audit trail.
#[async_trait]
pub trait AuditStore: Send + Sync {
    /// Appends an audit entry.
    async fn append_audit(&self, entry: &AuditRecord) -> Result<(), Error>;
    /// Lists the audit entries of a task in creation order.
    async fn list_audit_for_task(&self, task_id: &str) -> Result<Vec<AuditRecord>, Error>;
    /// Returns the hash of the most recent audit entry, if any.
    async fn last_audit_hash(&self) -> Result<Option<String>, Error>;
    /// Lists all audit entries in creation order (for export/verify).
    async fn list_all_audit(&self) -> Result<Vec<AuditRecord>, Error>;
}

/// Persistence of the registered agent registry.
#[async_trait]
pub trait AgentStore: Send + Sync {
    /// Inserts or updates a registered agent.
    async fn upsert_agent(&self, agent: &AgentRecord) -> Result<(), Error>;
    /// Loads an agent by name.
    async fn get_agent(&self, name: &str) -> Result<AgentRecord, Error>;
    /// Lists every registered agent.
    async fn list_agents(&self) -> Result<Vec<AgentRecord>, Error>;
}

/// Persistence of push notification configurations.
///
/// The default methods implement an empty store, so test stores that do not
/// care about push notifications need no extra code; the configured push
/// endpoints reject a registration when the backend returns the unsupported
/// error.
#[async_trait]
pub trait PushNotificationStore: Send + Sync {
    /// Inserts or replaces a push notification configuration.
    async fn upsert_push_config(&self, _record: &PushNotificationRecord) -> Result<(), Error> {
        Err(Error::Forbidden("push notifications are not supported by this store".to_owned()))
    }
    /// Loads a configuration by task and configuration identifier.
    async fn get_push_config(
        &self,
        task_id: &str,
        id: &str,
    ) -> Result<PushNotificationRecord, Error> {
        Err(errors::push_config_not_found(&format!("{task_id}/{id}")))
    }
    /// Lists the configurations of a task in creation order.
    async fn list_push_configs(
        &self,
        _task_id: &str,
    ) -> Result<Vec<PushNotificationRecord>, Error> {
        Ok(Vec::new())
    }
    /// Deletes a configuration.
    async fn delete_push_config(&self, task_id: &str, id: &str) -> Result<(), Error> {
        Err(errors::push_config_not_found(&format!("{task_id}/{id}")))
    }
}

/// Bundle of every storage capability required by the warden.
pub trait Store:
    TaskStore + ApprovalStore + AuditStore + AgentStore + PushNotificationStore
{
}

impl<T> Store for T where
    T: TaskStore + ApprovalStore + AuditStore + AgentStore + PushNotificationStore
{
}

/// Convenience constructors for storage errors.
pub mod errors {
    use super::*;

    /// Task missing.
    #[must_use]
    pub fn task_not_found(id: &str) -> Error {
        StorageError::TaskNotFound(id.to_owned()).into()
    }

    /// Approval missing.
    #[must_use]
    pub fn approval_not_found(id: &str) -> Error {
        StorageError::ApprovalNotFound(id.to_owned()).into()
    }

    /// Agent missing.
    #[must_use]
    pub fn agent_not_found(name: &str) -> Error {
        StorageError::AgentNotFound(name.to_owned()).into()
    }

    /// Push notification configuration missing.
    #[must_use]
    pub fn push_config_not_found(id: &str) -> Error {
        StorageError::PushNotificationNotFound(id.to_owned()).into()
    }

    /// Duplicate primary key.
    #[must_use]
    pub fn duplicate(what: &str) -> Error {
        StorageError::Duplicate(what.to_owned()).into()
    }

    /// Backend query failure.
    #[must_use]
    pub fn query(err: impl std::fmt::Display) -> Error {
        StorageError::Query(err.to_string()).into()
    }
}
