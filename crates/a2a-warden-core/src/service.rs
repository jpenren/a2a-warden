//! Orchestration of the warden workflow: capture, interception, delegation,
//! human decisions, and audit.
//!
//! The service is backend agnostic: it depends on the [`Store`] traits and on
//! an [`AgentClient`] implementation, so the whole flow can be tested with an
//! in-memory store and a scripted client.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::{broadcast, mpsc, Mutex};
use url::Url;

use crate::config::AgentConfig;
use crate::policy::PolicyConfig;
use crate::error::{Error, ProtocolError};
use crate::protocol::{
    Artifact, ListTasksParams, ListTasksResult, Message, MessageSendParams, ProtocolVersion,
    PushNotificationConfig, Role, StreamEvent, Task, TaskPushNotificationConfig, TaskState,
    TaskStatus,
};
use crate::push::{PushNotifier, PushPolicy};
use crate::stores::{
    errors, AgentRecord, ApprovalRecord, ApprovalStatus, ArtifactRecord, AuditRecord,
    MessageRecord, PushNotificationRecord, Store, TaskCursor, TaskFilter, TaskRecord,
};
use crate::verification::{CardVerification, CardVerifier, VerificationPolicy};
use crate::wire;
use crate::workflow::{evaluate, is_valid_transition, ApprovalDecision, Interception};
use crate::{new_id, now};

/// The warden acting as an A2A client toward a registered remote agent.
#[async_trait]
pub trait AgentClient: Send + Sync {
    /// Sends a message to the remote agent and returns the task it reports.
    async fn send_message(&self, agent_url: &Url, message: &Message) -> Result<Task, Error>;
    /// Requests cancellation of a remote task and returns its resulting state.
    async fn cancel_task(&self, agent_url: &Url, task_id: &str) -> Result<TaskState, Error>;
}

/// Everything the administration API shows for one task.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDetail {
    /// The task itself.
    pub task: TaskRecord,
    /// Message timeline in creation order.
    pub messages: Vec<MessageRecord>,
    /// Artifacts produced by the task, in creation order.
    pub artifacts: Vec<ArtifactRecord>,
    /// Pending or resolved approval, when one exists.
    pub approval: Option<ApprovalRecord>,
    /// Audit entries in creation order.
    pub audit: Vec<AuditRecord>,
}

/// Push delivery wired into the service.
#[derive(Clone)]
struct PushRuntime {
    policy: Arc<PushPolicy>,
    notifier: Arc<dyn PushNotifier>,
}

type TaskLocks = Arc<std::sync::Mutex<HashMap<String, Arc<Mutex<()>>>>>;

/// State machine orchestrating the whole workflow.
#[derive(Clone)]
pub struct WardenService {
    store: Arc<dyn Store>,
    client: Arc<dyn AgentClient>,
    locks: TaskLocks,
    events: broadcast::Sender<StreamEvent>,
    push: Option<PushRuntime>,
    verification: VerificationPolicy,
    verifier: Option<Arc<dyn CardVerifier>>,
    policy_config: PolicyConfig,
}

impl WardenService {
    /// Builds the service over a store and an agent client.
    #[must_use]
    pub fn new(store: Arc<dyn Store>, client: Arc<dyn AgentClient>) -> Self {
        let (events, _) = broadcast::channel(512);
        Self {
            store,
            client,
            locks: Arc::new(std::sync::Mutex::new(HashMap::new())),
            events,
            push: None,
            verification: VerificationPolicy::default(),
            verifier: None,
            policy_config: PolicyConfig::default(),
        }
    }

    /// Enables push notification delivery with an SSRF policy and a notifier.
    #[must_use]
    pub fn with_push(mut self, policy: PushPolicy, notifier: Arc<dyn PushNotifier>) -> Self {
        self.push = Some(PushRuntime { policy: Arc::new(policy), notifier });
        self
    }

    /// Sets the Cedar policy configuration for interception evaluation.
    #[must_use]
    pub fn with_policy_config(mut self, config: PolicyConfig) -> Self {
        self.policy_config = config;
        self
    }

    /// Enables Agent Card verification for every registration.
    ///
    /// When the policy is enabled, an unsigned or unverifiable card is
    /// quarantined according to [`VerificationPolicy::quarantine_unverified`].
    #[must_use]
    pub fn with_verification(
        mut self,
        verifier: Arc<dyn CardVerifier>,
        policy: VerificationPolicy,
    ) -> Self {
        self.verifier = Some(verifier);
        self.verification = policy;
        self
    }

    /// Returns the active verification policy.
    #[must_use]
    pub fn verification_policy(&self) -> VerificationPolicy {
        self.verification
    }

    /// Returns a handle to the underlying store.
    #[must_use]
    pub fn store(&self) -> Arc<dyn Store> {
        Arc::clone(&self.store)
    }

    /// Subscribes to the task event bus.
    #[must_use]
    pub fn subscribe_events(&self) -> broadcast::Receiver<StreamEvent> {
        self.events.subscribe()
    }

    /// Serializes operations on the same task (concurrent cancel/decision).
    async fn lock_task(&self, id: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = {
            let mut locks = self.locks.lock().expect("task lock map");
            Arc::clone(locks.entry(id.to_owned()).or_insert_with(|| Arc::new(Mutex::new(()))))
        };
        lock.lock_owned().await
    }

    /// Publishes an event to every subscriber. Delivery to no subscriber is
    /// not an error.
    fn publish(&self, event: StreamEvent) {
        let _ = self.events.send(event);
    }

    /// Publishes a complete task snapshot.
    fn publish_task(&self, task: &Task) {
        let event = StreamEvent::Task(task.clone());
        self.publish(event.clone());
        self.spawn_push(event);
    }

    /// Delivers an event to the push notification endpoints of its task.
    ///
    /// Delivery is best effort and runs in the background: a slow or broken
    /// callback must never delay the task lifecycle. The initial `submitted`
    /// snapshot of a request is not pushed; the state reached by the request
    /// is.
    fn spawn_push(&self, event: StreamEvent) {
        let Some(runtime) = self.push.clone() else {
            return;
        };
        if matches!(&event, StreamEvent::Task(task) if task.status.state == TaskState::Submitted) {
            return;
        }
        let Some(task_id) = event.task_id().map(str::to_owned) else {
            return;
        };
        let store = Arc::clone(&self.store);
        let payload = wire::stream_event_result(&event, ProtocolVersion::V1);
        tokio::spawn(async move {
            let records = match store.list_push_configs(&task_id).await {
                Ok(records) => records,
                Err(err) => {
                    tracing::warn!(task_id = %task_id, error = %err, "cannot read push configurations");
                    return;
                }
            };
            for record in records {
                // Revalidate at delivery time: DNS rebinding and long lived
                // configurations make the registration-time check insufficient.
                let url = match runtime.policy.validate(&record.url).await {
                    Ok(url) => url,
                    Err(err) => {
                        tracing::warn!(task_id = %task_id, url = %record.url, error = %err, "push URL rejected at delivery");
                        continue;
                    }
                };
                let authentication =
                    record.auth_json.as_deref().and_then(|raw| serde_json::from_str(raw).ok());
                if let Err(err) = runtime
                    .notifier
                    .deliver(&url, record.token.as_deref(), authentication.as_ref(), &payload)
                    .await
                {
                    tracing::warn!(task_id = %task_id, url = %record.url, error = %err, "push delivery failed");
                }
            }
        });
    }

    /// Seeds the agent registry from configuration (idempotent).
    ///
    /// When verification is enabled, every configured card is verified before
    /// the agent is stored. An unsigned or unverifiable card is marked as not
    /// verified and, depending on the policy, quarantined. Every outcome is
    /// audited.
    pub async fn register_agents(&self, agents: &[AgentConfig]) -> Result<(), Error> {
        for agent in agents {
            let mut record = AgentRecord {
                id: new_id(),
                name: agent.name.clone(),
                url: agent.url.to_string(),
                skills_json: serde_json::to_string(&agent.skills)?,
                card_json: agent.card_json.as_ref().map(Value::to_string),
                verified: false,
                verification_error: None,
                jwks_kid: None,
                verified_at: None,
                verification_issuer: None,
                verification_alg: None,
                quarantined: false,
                created_at: now(),
            };

            if self.verification.enabled {
                let outcome = self.verify_agent_card(agent).await;
                record.verified = outcome.verified;
                record.verification_error.clone_from(&outcome.error);
                record.jwks_kid.clone_from(&outcome.kid);
                record.verified_at = outcome.verified.then(now);
                record.verification_issuer.clone_from(&outcome.issuer);
                record.verification_alg.clone_from(&outcome.alg);
                record.quarantined = outcome.quarantined;

                let action = if outcome.verified {
                    "agent.verified"
                } else if outcome.quarantined {
                    "agent.quarantined"
                } else {
                    "agent.verification_failed"
                };
                self.audit_agent(
                    &record,
                    action,
                    json!({
                        "agent": record.name,
                        "verified": outcome.verified,
                        "quarantined": outcome.quarantined,
                        "kid": outcome.kid,
                        "issuer": outcome.issuer,
                        "alg": outcome.alg,
                        "error": outcome.error,
                    }),
                )
                .await?;

                if outcome.quarantined {
                    tracing::warn!(
                        agent = %record.name,
                        error = outcome.error.as_deref().unwrap_or("unknown"),
                        "Agent Card verification failed: agent quarantined"
                    );
                }
            }

            self.store.upsert_agent(&record).await?;
        }
        Ok(())
    }

    /// Verifies the configured card of an agent, if any.
    async fn verify_agent_card(&self, agent: &AgentConfig) -> CardVerification {
        let Some(card) = agent.card_json.as_ref() else {
            return CardVerification::failed(
                "no Agent Card is configured and verification is required",
                self.verification.quarantine_unverified,
            );
        };
        let Some(verifier) = &self.verifier else {
            return CardVerification::failed(
                "verification is enabled but no verifier is configured",
                self.verification.quarantine_unverified,
            );
        };
        verifier.verify_card(card, true, self.verification.quarantine_unverified).await
    }

    // -------------------------------------------------------------- entry point
    /// Handles an incoming `message/send` request.
    pub async fn send_message(&self, params: MessageSendParams) -> Result<Task, Error> {
        let mut message = params.message;
        if message.role != Role::User {
            return Err(
                ProtocolError::InvalidParams("message.role must be \"user\"".to_owned()).into()
            );
        }
        if message.parts.is_empty() {
            return Err(
                ProtocolError::InvalidParams("message.parts must not be empty".to_owned()).into()
            );
        }

        let agent = self.resolve_target(&message).await?;
        let context_id = match message.context_id.as_deref() {
            Some(value) if !value.is_empty() => value.to_owned(),
            _ => new_id(),
        };
        message.context_id = Some(context_id.clone());
        let sender = sender_of(&message);

        let timestamp = now();
        let task = TaskRecord {
            id: new_id(),
            context_id,
            agent_id: agent.name.clone(),
            status: TaskState::Submitted,
            created_at: timestamp.clone(),
            updated_at: timestamp,
            metadata_json: message
                .metadata
                .as_ref()
                .map(|meta| Value::Object(meta.clone()).to_string()),
            remote_task_id: None,
        };
        self.store.create_task(&task).await?;
        self.append_message(&task, &message).await?;
        self.audit(
            &task,
            &sender,
            "message.received",
            json!({ "messageId": message.message_id, "agent": agent.name }),
        )
        .await?;

        // A subscriber that connected before this call sees the task as soon
        // as it exists, which is what the streaming endpoint needs.
        self.publish_task(&task_snapshot(&task));

        // Use Cedar policy evaluation if configured, otherwise fall back to safe default
        let skill = message
            .metadata
            .as_ref()
            .and_then(|m| m.get("skill"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if evaluate(&self.policy_config, &agent.name, &agent.name, skill)
            == Interception::HoldForApproval
        {
            let held = self.hold_for_approval(&task, &sender).await?;
            let task =
                self.to_task(&held, params.configuration.and_then(|c| c.history_length)).await?;
            self.publish_task(&task);
            return Ok(task);
        }

        let task = self.forward(&task, &message, &sender).await?;
        self.publish_task(&task);
        Ok(task)
    }

    /// Handles a `tasks/get` request.
    pub async fn get_task(&self, id: &str, history_length: Option<u64>) -> Result<Task, Error> {
        let record = self.store.get_task(id).await?;
        self.to_task(&record, history_length).await
    }

    /// Handles a `tasks/cancel` request.
    pub async fn cancel_task(&self, id: &str, actor: &str) -> Result<Task, Error> {
        let _guard = self.lock_task(id).await;
        let task = self.cancel_task_locked(id, actor).await?;
        self.publish_task(&task);
        Ok(task)
    }

    /// Cancels a task while the caller holds its per-task lock.
    async fn cancel_task_locked(&self, id: &str, actor: &str) -> Result<Task, Error> {
        let task = self.store.get_task(id).await?;
        if task.status.is_terminal() {
            return Err(Error::Conflict(format!(
                "task {} is already {}",
                id,
                task.status.as_str()
            )));
        }

        // A pending approval owns the task while it is paused: cancelling resolves it.
        if let Some(approval) = self.store.pending_approval_for_task(id).await? {
            return self
                .decide_approval_locked(
                    &approval.id,
                    ApprovalDecision::Cancel,
                    actor,
                    Some("cancelled through tasks/cancel"),
                )
                .await;
        }

        // The task was already forwarded: request cancellation on the remote agent
        // on a best-effort basis and record the outcome (see docs/architecture.md).
        if let Ok(agent) = self.store.get_agent(&task.agent_id).await {
            if let Ok(url) = Url::parse(&agent.url) {
                // The remote agent knows the task by its own identifier.
                let remote_id = task.remote_task_id.as_deref().unwrap_or(id);
                match self.client.cancel_task(&url, remote_id).await {
                    Ok(state) => {
                        self.audit(
                            &task,
                            actor,
                            "remote.cancel_requested",
                            json!({ "agent": agent.name, "state": state.as_str() }),
                        )
                        .await?;
                    }
                    Err(err) => {
                        self.audit(
                            &task,
                            actor,
                            "remote.cancel_unreachable",
                            json!({ "agent": agent.name, "error": err.to_string() }),
                        )
                        .await?;
                    }
                }
            }
        }

        let message = Message::agent_text(format!("Task cancelled by {actor}."));
        self.append_message(&task, &message).await?;
        let updated = self
            .set_state(
                &task,
                TaskState::Canceled,
                actor,
                "task.canceled",
                json!({ "actor": actor }),
            )
            .await?;
        self.to_task(&updated, None).await
    }

    /// Applies a human decision to a pending approval.
    pub async fn decide_approval(
        &self,
        approval_id: &str,
        decision: ApprovalDecision,
        actor: &str,
        reason: Option<&str>,
    ) -> Result<Task, Error> {
        let approval = self.store.get_approval(approval_id).await?;
        let _guard = self.lock_task(&approval.task_id).await;
        let task = self.decide_approval_locked(approval_id, decision, actor, reason).await?;
        self.publish_task(&task);
        Ok(task)
    }

    /// Applies a human decision while the caller holds the task lock.
    async fn decide_approval_locked(
        &self,
        approval_id: &str,
        decision: ApprovalDecision,
        actor: &str,
        reason: Option<&str>,
    ) -> Result<Task, Error> {
        let approval = self.store.get_approval(approval_id).await?;
        if approval.status != ApprovalStatus::Pending {
            return Err(Error::Conflict(format!(
                "approval {approval_id} is already {}",
                approval.status.as_str()
            )));
        }
        let task = self.store.get_task(&approval.task_id).await?;
        let status = match decision {
            ApprovalDecision::Approve => ApprovalStatus::Approved,
            ApprovalDecision::Reject => ApprovalStatus::Rejected,
            ApprovalDecision::Cancel => ApprovalStatus::Cancelled,
        };
        let decided_at = now();
        self.store.decide_approval(approval_id, status, actor, &decided_at, reason).await?;

        let payload = json!({
            "approver": actor,
            "decision": decision_verb(decision),
            "reason": reason,
            "approvalId": approval_id,
        });
        self.audit(&task, actor, "approval.decided", payload.clone()).await?;

        let suffix = reason.map(|r| format!(" Reason: {r}")).unwrap_or_default();
        match decision {
            ApprovalDecision::Approve => {
                let message =
                    Message::agent_text(format!("Human approval granted by {actor}.{suffix}"));
                self.append_message(&task, &message).await?;
                let resumed = self
                    .set_state(&task, TaskState::Working, actor, "task.resumed", payload)
                    .await?;
                let original = self.original_message(&resumed).await?;
                self.forward(&resumed, &original, actor).await
            }
            ApprovalDecision::Reject | ApprovalDecision::Cancel => {
                let label = decision_label(decision);
                let message = Message::agent_text(format!("Task {} by {actor}.{suffix}", label));
                self.append_message(&task, &message).await?;
                let updated = self
                    .set_state(
                        &task,
                        decision.resulting_state(),
                        actor,
                        match decision {
                            ApprovalDecision::Reject => "task.rejected",
                            ApprovalDecision::Cancel => "task.canceled",
                            ApprovalDecision::Approve => "task.resumed",
                        },
                        payload,
                    )
                    .await?;
                self.to_task(&updated, None).await
            }
        }
    }

    // ------------------------------------------------------------- streaming
    /// Handles a `message/stream` request, forwarding the task events to the
    /// sink. The stream ends after the terminal event of the task.
    pub async fn send_message_streaming(
        &self,
        params: MessageSendParams,
        sink: mpsc::Sender<StreamEvent>,
    ) -> Result<(), Error> {
        let mut events = self.subscribe_events();
        let task = match self.send_message(params).await {
            Ok(task) => task,
            Err(err) => {
                let _ = sink
                    .send(StreamEvent::Error { code: error_code(&err), message: err.to_string() })
                    .await;
                return Ok(());
            }
        };

        // Events published while the task was being processed are buffered in
        // the subscription: replay the ones that belong to this task.
        let mut sent = 0_usize;
        loop {
            match events.try_recv() {
                Ok(event) if event.task_id() == Some(task.id.as_str()) => {
                    sent += 1;
                    if sink.send(event).await.is_err() {
                        return Ok(());
                    }
                }
                Ok(_) => {}
                Err(broadcast::error::TryRecvError::Lagged(_)) => {}
                Err(broadcast::error::TryRecvError::Empty)
                | Err(broadcast::error::TryRecvError::Closed) => break,
            }
        }
        if sent == 0 {
            let _ = sink.send(StreamEvent::Task(task)).await;
        }
        Ok(())
    }

    /// Handles a `tasks/resubscribe` request: emits the current snapshot and
    /// then live events until the task reaches a terminal state.
    pub async fn subscribe_task(
        &self,
        task_id: &str,
        history_length: Option<u64>,
        sink: mpsc::Sender<StreamEvent>,
    ) -> Result<(), Error> {
        let mut events = self.subscribe_events();
        let task = self.get_task(task_id, history_length).await?;
        if sink.send(StreamEvent::Task(task.clone())).await.is_err() {
            return Ok(());
        }
        if task.status.state.is_terminal() {
            return Ok(());
        }
        loop {
            match events.recv().await {
                Ok(event) if event.task_id() == Some(task_id) => {
                    let terminal = event_is_terminal(&event);
                    if sink.send(event).await.is_err() {
                        return Ok(());
                    }
                    if terminal {
                        return Ok(());
                    }
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return Ok(()),
            }
        }
    }

    // ------------------------------------------------------------- listing
    /// Handles a `tasks/list` request with cursor pagination.
    pub async fn list_tasks_page(
        &self,
        params: &ListTasksParams,
    ) -> Result<ListTasksResult, Error> {
        let page_size = params.page_size.unwrap_or(50).clamp(1, 200);
        let cursor = match params.page_token.as_deref() {
            Some(token) => Some(
                TaskCursor::decode(token)
                    .ok_or_else(|| ProtocolError::InvalidParams("invalid pageToken".to_owned()))?,
            ),
            None => None,
        };
        let filter = TaskFilter {
            status: params.status,
            agent: None,
            context_id: params.context_id.clone(),
            updated_after: params.status_timestamp_after.clone(),
            limit: None,
            offset: None,
        };

        let mut records = self
            .store
            .list_tasks_page(&filter, cursor.as_ref(), page_size.saturating_add(1))
            .await?;
        let has_more = records.len() > usize::try_from(page_size).unwrap_or(usize::MAX);
        if has_more {
            records.truncate(usize::try_from(page_size).unwrap_or(usize::MAX));
        }
        let next_page_token = has_more
            .then(|| records.last().map(|record| TaskCursor::from_task(record).encode()))
            .flatten();

        let include_artifacts = params.include_artifacts.unwrap_or(true);
        let mut tasks = Vec::with_capacity(records.len());
        for record in &records {
            let mut task = self.to_task(record, params.history_length).await?;
            if !include_artifacts {
                task.artifacts = None;
            }
            tasks.push(task);
        }
        let total_size = self.store.count_tasks(&filter).await?;

        Ok(ListTasksResult {
            tasks,
            next_page_token,
            page_size: Some(page_size),
            total_size: Some(total_size),
        })
    }

    // ------------------------------------------------------------- push configs
    /// Registers a push notification configuration for a task.
    pub async fn create_push_config(
        &self,
        task_id: &str,
        mut config: PushNotificationConfig,
    ) -> Result<TaskPushNotificationConfig, Error> {
        let runtime = self
            .push
            .as_ref()
            .ok_or_else(|| Error::Forbidden("push notifications are disabled".to_owned()))?;
        // The task must exist; a callback for an unknown task is a mistake.
        self.store.get_task(task_id).await?;
        let url = runtime.policy.validate(&config.url).await?;
        config.url = url.to_string();
        let id = config.id.clone().unwrap_or_else(new_id);
        config.id = Some(id.clone());
        let record = PushNotificationRecord {
            id: id.clone(),
            task_id: task_id.to_owned(),
            url: config.url.clone(),
            token: config.token.clone(),
            auth_json: config.authentication.as_ref().map(serde_json::to_string).transpose()?,
            created_at: now(),
        };
        self.store.upsert_push_config(&record).await?;
        Ok(TaskPushNotificationConfig {
            task_id: task_id.to_owned(),
            push_notification_config: config,
        })
    }

    /// Loads a push notification configuration.
    pub async fn get_push_config(
        &self,
        task_id: &str,
        id: &str,
    ) -> Result<TaskPushNotificationConfig, Error> {
        let record = self.store.get_push_config(task_id, id).await?;
        Ok(push_config_from_record(record))
    }

    /// Lists the push notification configurations of a task.
    pub async fn list_push_configs(
        &self,
        task_id: &str,
    ) -> Result<Vec<TaskPushNotificationConfig>, Error> {
        Ok(self
            .store
            .list_push_configs(task_id)
            .await?
            .into_iter()
            .map(push_config_from_record)
            .collect())
    }

    /// Deletes a push notification configuration.
    pub async fn delete_push_config(&self, task_id: &str, id: &str) -> Result<(), Error> {
        self.store.delete_push_config(task_id, id).await
    }

    // ------------------------------------------------------------- expiry
    /// Expires non-terminal tasks that have been inactive for `ttl`.
    ///
    /// The returned count is the number of tasks that transitioned to
    /// `canceled`. Pending approvals are cancelled and audited as well, so a
    /// stale approval can never be approved later.
    pub async fn expire_stale_tasks(&self, ttl: Duration) -> Result<usize, Error> {
        let ttl = chrono::Duration::from_std(ttl)
            .map_err(|err| Error::Validation(format!("invalid expiry ttl: {err}")))?;
        let cutoff = (Utc::now() - ttl).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let mut expired = 0_usize;

        for record in self.store.list_expired_tasks(&cutoff).await? {
            let _guard = self.lock_task(&record.id).await;
            // Another worker may have handled it while we waited for the lock.
            let task = self.store.get_task(&record.id).await?;
            if task.status.is_terminal() {
                continue;
            }

            if let Some(approval) = self.store.pending_approval_for_task(&task.id).await? {
                let decided = self
                    .decide_approval_locked(
                        &approval.id,
                        ApprovalDecision::Cancel,
                        "warden",
                        Some("task expired"),
                    )
                    .await?;
                self.audit(
                    &task,
                    "warden",
                    "task.expired",
                    json!({ "previousState": task.status.as_str(), "reason": "ttl" }),
                )
                .await?;
                self.publish_task(&decided);
                expired += 1;
                continue;
            }

            let message = Message::agent_text("Task expired because it was inactive for too long.");
            self.append_message(&task, &message).await?;
            let updated = self
                .set_state(
                    &task,
                    TaskState::Canceled,
                    "warden",
                    "task.expired",
                    json!({ "previousState": task.status.as_str(), "reason": "ttl" }),
                )
                .await?;
            let snapshot = self.to_task(&updated, None).await?;
            self.publish_task(&snapshot);
            expired += 1;
        }

        Ok(expired)
    }

    // ------------------------------------------------------------- read side
    /// Lists tasks for the administration API.
    pub async fn list_tasks(&self, filter: &TaskFilter) -> Result<Vec<TaskRecord>, Error> {
        self.store.list_tasks(filter).await
    }

    pub async fn count_tasks(&self, filter: &TaskFilter) -> Result<u64, Error> {
        self.store.count_tasks(filter).await
    }

    /// Full detail of a task for the administration API and the console.
    pub async fn task_detail(&self, id: &str) -> Result<TaskDetail, Error> {
        let task = self.store.get_task(id).await?;
        let messages = self.store.list_messages(id).await?;
        let artifacts = self.store.list_artifacts(id).await?;
        let approval = self
            .store
            .pending_approval_for_task(id)
            .await?
            .or(self.first_approval_for_task(id).await?);
        let audit = self.store.list_audit_for_task(id).await?;
        Ok(TaskDetail { task, messages, artifacts, approval, audit })
    }

    /// Lists approvals for the administration inbox.
    pub async fn count_approvals(&self, status: Option<ApprovalStatus>) -> Result<u64, Error> {
        self.store.count_approvals(status).await
    }
    pub async fn list_approvals(
        &self,
        status: Option<ApprovalStatus>,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<ApprovalRecord>, Error> {
        self.store.list_approvals(status, limit, offset).await
    }

    /// Loads a single approval.
    pub async fn get_approval(&self, id: &str) -> Result<ApprovalRecord, Error> {
        self.store.get_approval(id).await
    }

    /// Lists all audit entries for export/verify (Phase 5).
    pub async fn list_all_audit(&self) -> Result<Vec<AuditRecord>, Error> {
        self.store.list_all_audit().await
    }

    /// Masks personally identifiable information in a text value (Phase 5).
    #[must_use]
    pub fn mask_pii(&self, value: &str) -> String {
        crate::pii::mask_pii(value)
    }

    /// Forensic replay: re-sends the original user message of a task to its
    /// remote agent, creating a new delegation attempt while preserving the
    /// original task record (Phase 5).
    pub async fn replay_task(&self, task_id: &str, actor: &str) -> Result<Task, Error> {
        let task = self.store.get_task(task_id).await?;
        let original = self.original_message(&task).await?;

        let sender = sender_of(&original);

        // Audit the replay attempt before forwarding.
        self.audit(
            &task,
            actor,
            "task.replay_started",
            json!({ "originalMessageId": original.message_id }),
        )
        .await?;

        let result = self.forward(&task, &original, &sender).await;

        // Audit the outcome.
        match &result {
            Ok(replayed) => {
                self.audit(
                    &task,
                    actor,
                    "task.replay_completed",
                    json!({ "newStatus": replayed.status.state.as_str(), "remoteTaskId": replayed.id }),
                )
                .await?;
            }
            Err(err) => {
                self.audit(
                    &task,
                    actor,
                    "task.replay_failed",
                    json!({ "error": err.to_string() }),
                )
                .await?;
            }
        }

        result
    }

    /// Selective re-send: finds a stored message by its protocol message ID
    /// and re-sends it to the remote agent of its task (Phase 5).
    pub async fn resend_message(
        &self,
        message_id: &str,
        actor: &str,
    ) -> Result<Task, Error> {
        // Find the message by scanning all tasks (MVP: simple linear scan).
        let all_tasks = self.store.list_tasks(&TaskFilter::default()).await?;
        let mut found = None;
        for task in &all_tasks {
            for message in self.store.list_messages(&task.id).await? {
                if message.id == message_id {
                    found = Some((task.clone(), message));
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }

        let (task, message) = found.ok_or_else(|| {
            Error::NotFound(format!("message '{message_id}' not found in any task"))
        })?;

        let protocol_message = message_from_record(&message);
        let sender = sender_of(&protocol_message);

        self.audit(
            &task,
            actor,
            "message.resend_started",
            json!({ "messageId": message_id, "taskId": task.id }),
        )
        .await?;

        let result = self.forward(&task, &protocol_message, &sender).await;

        match &result {
            Ok(_) => {
                self.audit(
                    &task,
                    actor,
                    "message.resend_completed",
                    json!({ "messageId": message_id }),
                )
                .await?;
            }
            Err(err) => {
                self.audit(
                    &task,
                    actor,
                    "message.resend_failed",
                    json!({ "messageId": message_id, "error": err.to_string() }),
                )
                .await?;
            }
        }

        result
    }

    /// Global search across tasks, audit entries, and approvals (Phase 5).
    pub async fn search(
        &self,
        query: &str,
    ) -> Result<serde_json::Value, Error> {
        let q = query.to_lowercase();
        let mut tasks = Vec::new();
        let mut audit = Vec::new();
        let mut approvals = Vec::new();

        // Search tasks by id, agent, or context
        let all_tasks = self.store.list_tasks(&TaskFilter::default()).await?;
        for task in all_tasks {
            if task.id.to_lowercase().contains(&q)
                || task.agent_id.to_lowercase().contains(&q)
                || task.context_id.to_lowercase().contains(&q)
            {
                tasks.push(task);
            }
        }

        // Search audit entries by actor, action, or task/context id
        let all_audit = self.store.list_all_audit().await?;
        for entry in all_audit {
            if entry.actor.to_lowercase().contains(&q)
                || entry.action.to_lowercase().contains(&q)
                || entry.task_id.as_deref().map_or(false, |t| t.to_lowercase().contains(&q))
                || entry.context_id.as_deref().map_or(false, |c| c.to_lowercase().contains(&q))
            {
                audit.push(entry);
            }
        }

        // Search approvals by id or task_id
        let all_approvals = self.store.list_approvals(None, None, None).await?;
        for approval in all_approvals {
            if approval.id.to_lowercase().contains(&q)
                || approval.task_id.to_lowercase().contains(&q)
            {
                approvals.push(approval);
            }
        }

        Ok(json!({
            "query": query,
            "tasks": tasks,
            "audit": audit,
            "approvals": approvals,
        }))
    }

    /// Lists the registered agents.
    pub async fn list_agents(&self) -> Result<Vec<AgentRecord>, Error> {
        self.store.list_agents().await
    }

    // -------------------------------------------------------------- internals
    async fn first_approval_for_task(
        &self,
        task_id: &str,
    ) -> Result<Option<ApprovalRecord>, Error> {
        Ok(self
            .store
            .list_approvals(None, None, None)
            .await?
            .into_iter()
            .find(|approval| approval.task_id == task_id))
    }

    async fn resolve_target(&self, message: &Message) -> Result<AgentRecord, Error> {
        if let Some(name) = message
            .metadata_str("targetAgent")
            .or_else(|| message.metadata_str("agent"))
            .or_else(|| message.metadata_str("target"))
        {
            let agent =
                self.store.get_agent(&name).await.map_err(|_| errors::agent_not_found(&name))?;
            return self.ensure_delegable(agent);
        }

        if let Some(skill) = message.metadata_str("skill") {
            let mut matches = Vec::new();
            for agent in self.store.list_agents().await? {
                if parse_skills(&agent.skills_json).contains(&skill) && !agent.quarantined {
                    matches.push(agent);
                }
            }
            return match matches.len() {
                0 => Err(ProtocolError::SkillNotFound(skill).into()),
                1 => Ok(matches.remove(0)),
                _ => Err(ProtocolError::InvalidParams(format!(
                    "skill '{skill}' is provided by more than one agent; set message.metadata.targetAgent"
                ))
                .into()),
            };
        }

        let mut agents = self.store.list_agents().await?;
        agents.retain(|agent| !agent.quarantined);
        match agents.len() {
            1 => Ok(agents.remove(0)),
            0 => Err(ProtocolError::AgentNotFound(
                "no agent is registered or every registered agent is quarantined".to_owned(),
            )
            .into()),
            _ => Err(ProtocolError::AgentNotFound(
                "several agents are registered; set message.metadata.targetAgent or message.metadata.skill"
                    .to_owned(),
            )
            .into()),
        }
    }

    /// Rejects an agent whose Agent Card is quarantined.
    fn ensure_delegable(&self, agent: AgentRecord) -> Result<AgentRecord, Error> {
        if agent.quarantined {
            return Err(ProtocolError::CardVerificationFailed(format!(
                "agent '{}' is quarantined: {}",
                agent.name,
                agent
                    .verification_error
                    .as_deref()
                    .unwrap_or("the Agent Card could not be verified")
            ))
            .into());
        }
        Ok(agent)
    }

    async fn hold_for_approval(
        &self,
        task: &TaskRecord,
        sender: &str,
    ) -> Result<TaskRecord, Error> {
        let timestamp = now();
        let prompt = Message::agent_text(format!(
            "Human approval required before this task is forwarded to '{}'.",
            task.agent_id
        ));
        self.append_message(task, &prompt).await?;

        self.store.update_task_status(&task.id, TaskState::InputRequired, &timestamp).await?;
        let approval = ApprovalRecord {
            id: new_id(),
            task_id: task.id.clone(),
            status: ApprovalStatus::Pending,
            decided_by: None,
            decided_at: None,
            expires_at: None,
            reason: None,
            created_at: timestamp.clone(),
        };
        self.store.create_approval(&approval).await?;
        self.audit(
            task,
            "warden",
            "approval.requested",
            json!({ "approvalId": approval.id, "sender": sender, "agent": task.agent_id }),
        )
        .await?;

        Ok(TaskRecord { status: TaskState::InputRequired, updated_at: timestamp, ..task.clone() })
    }

    async fn forward(
        &self,
        task: &TaskRecord,
        message: &Message,
        sender: &str,
    ) -> Result<Task, Error> {
        let agent = self.store.get_agent(&task.agent_id).await?;
        if agent.quarantined {
            let message = Message::agent_text(format!(
                "Delegation to '{}' refused: the Agent Card is quarantined.",
                agent.name
            ));
            self.append_message(task, &message).await?;
            self.set_state(
                task,
                TaskState::Failed,
                "warden",
                "forward.refused",
                json!({
                    "agent": agent.name,
                    "reason": "agent card quarantined",
                    "error": agent.verification_error,
                }),
            )
            .await?;
            let failed = self.store.get_task(&task.id).await?;
            return self.to_task(&failed, None).await;
        }
        let url = Url::parse(&agent.url).map_err(|err| {
            Error::Internal(format!("invalid url for agent '{}': {err}", agent.name))
        })?;

        let mut forwarded = message.clone();
        forwarded.message_id = new_id();
        forwarded.context_id = Some(task.context_id.clone());
        // The task identifier is not forwarded: it is the warden's own
        // identifier, and the remote agent would look for a task it does not
        // know. The remote identifier is recorded on the task instead.
        forwarded.task_id = None;
        self.append_message(task, &forwarded).await?;
        self.audit(
            task,
            sender,
            "task.forwarded",
            json!({ "agent": agent.name, "messageId": forwarded.message_id }),
        )
        .await?;

        match self.client.send_message(&url, &forwarded).await {
            Ok(remote) => {
                if let Some(response) = remote.status.message {
                    self.append_message(task, &response).await?;
                }
                self.record_artifacts(task, &remote.artifacts).await?;
                self.store.set_remote_task_id(&task.id, &remote.id).await?;
                let state = match remote.status.state {
                    TaskState::Submitted => TaskState::Working,
                    other => other,
                };
                let updated = self
                    .set_state(
                        task,
                        state,
                        "warden",
                        "response.received",
                        json!({ "agent": agent.name, "remoteTaskId": remote.id }),
                    )
                    .await?;
                self.to_task(&updated, None).await
            }
            Err(err) => {
                let message =
                    Message::agent_text(format!("Delegation to '{}' failed: {err}", agent.name));
                self.append_message(task, &message).await?;
                self.set_state(
                    task,
                    TaskState::Failed,
                    "warden",
                    "forward.failed",
                    json!({ "agent": agent.name, "error": err.to_string() }),
                )
                .await?;
                let failed = self.store.get_task(&task.id).await?;
                self.to_task(&failed, None).await
            }
        }
    }

    async fn record_artifacts(
        &self,
        task: &TaskRecord,
        artifacts: &Option<Vec<Artifact>>,
    ) -> Result<(), Error> {
        let Some(artifacts) = artifacts else {
            return Ok(());
        };
        for artifact in artifacts {
            self.store
                .append_artifact(&ArtifactRecord {
                    id: new_id(),
                    artifact_id: artifact.artifact_id.clone(),
                    task_id: task.id.clone(),
                    name: artifact.name.clone(),
                    parts_json: serde_json::to_string(&artifact.parts)?,
                    metadata_json: artifact
                        .metadata
                        .as_ref()
                        .map(|meta| Value::Object(meta.clone()).to_string()),
                    created_at: now(),
                })
                .await?;
        }
        Ok(())
    }

    async fn set_state(
        &self,
        task: &TaskRecord,
        next: TaskState,
        actor: &str,
        action: &str,
        payload: Value,
    ) -> Result<TaskRecord, Error> {
        if !is_valid_transition(task.status, next) {
            tracing::warn!(
                from = task.status.as_str(),
                to = next.as_str(),
                task_id = %task.id,
                "ignoring invalid task state transition"
            );
            self.audit(
                task,
                actor,
                "state.transition_ignored",
                json!({ "from": task.status.as_str(), "to": next.as_str() }),
            )
            .await?;
            return Ok(task.clone());
        }

        let timestamp = now();
        self.store.update_task_status(&task.id, next, &timestamp).await?;
        self.audit(task, actor, action, payload).await?;
        Ok(TaskRecord { status: next, updated_at: timestamp, ..task.clone() })
    }

    async fn original_message(&self, task: &TaskRecord) -> Result<Message, Error> {
        let record = self
            .store
            .list_messages(&task.id)
            .await?
            .into_iter()
            .find(|message| message.role == Role::User)
            .ok_or_else(|| {
                Error::Internal(format!("task {} has no user message to forward", task.id))
            })?;
        let mut message = message_from_record(&record);
        // The message row keeps the content of the request but not its
        // metadata. The task row holds the metadata of the original request,
        // so it is restored here: a task that was held for approval must be
        // forwarded exactly as it was received, otherwise the target agent
        // loses the skill, the sender, and the routing information.
        message.metadata =
            task.metadata_json.as_deref().and_then(|raw| serde_json::from_str(raw).ok());
        Ok(message)
    }

    async fn append_message(&self, task: &TaskRecord, message: &Message) -> Result<(), Error> {
        let record = MessageRecord {
            // The row identity belongs to the warden: two peers may reuse the
            // same protocol message identifier in different conversations.
            id: new_id(),
            message_id: message.message_id.clone(),
            task_id: task.id.clone(),
            context_id: task.context_id.clone(),
            role: message.role,
            parts_json: serde_json::to_string(&message.parts)?,
            created_at: now(),
        };
        self.store.append_message(&record).await
    }

    async fn audit(
        &self,
        task: &TaskRecord,
        actor: &str,
        action: &str,
        payload: Value,
    ) -> Result<(), Error> {
        // Hash-chained audit log (Phase 5): every entry includes the hash of
        // the previous entry, forming a tamper-evident chain.
        let previous_hash = self.store.last_audit_hash().await?;
        let payload_str = payload.to_string();
        let mut hasher = Sha256::new();
        hasher.update(previous_hash.as_deref().unwrap_or(""));
        hasher.update(b"||");
        hasher.update(now().as_bytes());
        hasher.update(b"||");
        hasher.update(actor.as_bytes());
        hasher.update(b"||");
        hasher.update(action.as_bytes());
        hasher.update(b"||");
        hasher.update(payload_str.as_bytes());
        let hash = hex::encode(hasher.finalize());

        self.store
            .append_audit(&AuditRecord {
                id: new_id(),
                ts: now(),
                actor: actor.to_owned(),
                action: action.to_owned(),
                task_id: Some(task.id.clone()),
                context_id: Some(task.context_id.clone()),
                payload_json: Some(payload_str),
                previous_hash,
                hash: Some(hash),
            })
            .await
    }

    /// Appends an audit entry about an agent, outside of any task.
    async fn audit_agent(
        &self,
        agent: &AgentRecord,
        action: &str,
        payload: Value,
    ) -> Result<(), Error> {
        let previous_hash = self.store.last_audit_hash().await?;
        let payload_str = payload.to_string();
        let mut hasher = Sha256::new();
        hasher.update(previous_hash.as_deref().unwrap_or(""));
        hasher.update(b"||");
        hasher.update(now().as_bytes());
        hasher.update(b"||");
        hasher.update(b"warden");
        hasher.update(b"||");
        hasher.update(action.as_bytes());
        hasher.update(b"||");
        hasher.update(payload_str.as_bytes());
        let hash = hex::encode(hasher.finalize());

        self.store
            .append_audit(&AuditRecord {
                id: new_id(),
                ts: now(),
                actor: "warden".to_owned(),
                action: action.to_owned(),
                task_id: None,
                context_id: None,
                payload_json: Some(payload_str),
                previous_hash,
                hash: Some(hash),
            })
            .await
            .map_err(|err| {
                Error::Internal(format!(
                    "cannot audit the verification of agent '{}': {err}",
                    agent.name
                ))
            })
    }

    async fn to_task(
        &self,
        record: &TaskRecord,
        history_length: Option<u64>,
    ) -> Result<Task, Error> {
        let messages = self.store.list_messages(&record.id).await?;
        let status_message = messages
            .iter()
            .rev()
            .find(|message| message.role == Role::Agent)
            .map(message_from_record);

        // Without an explicit limit the whole timeline is returned; a limit
        // keeps the most recent messages.
        let history = messages.iter().map(message_from_record).collect::<Vec<_>>();
        let history = match history_length {
            Some(length) => {
                let length = usize::try_from(length).unwrap_or(usize::MAX);
                let start = history.len().saturating_sub(length);
                history[start..].to_vec()
            }
            None => history,
        };

        let artifacts = self
            .store
            .list_artifacts(&record.id)
            .await?
            .into_iter()
            .map(|artifact| Artifact {
                artifact_id: artifact.artifact_id,
                name: artifact.name,
                parts: serde_json::from_str(&artifact.parts_json).unwrap_or_default(),
                metadata: artifact
                    .metadata_json
                    .as_deref()
                    .and_then(|raw| serde_json::from_str(raw).ok()),
            })
            .collect::<Vec<_>>();

        Ok(Task {
            id: record.id.clone(),
            context_id: record.context_id.clone(),
            status: TaskStatus {
                state: record.status,
                message: status_message,
                timestamp: record.updated_at.clone(),
            },
            artifacts: (!artifacts.is_empty()).then_some(artifacts),
            history: (!history.is_empty()).then_some(history),
            metadata: record
                .metadata_json
                .as_deref()
                .and_then(|raw| serde_json::from_str(raw).ok()),
        })
    }
}

/// Rebuilds a protocol message from its stored row.
#[must_use]
pub fn message_from_record(record: &MessageRecord) -> Message {
    Message {
        message_id: record.message_id.clone(),
        role: record.role,
        parts: serde_json::from_str(&record.parts_json).unwrap_or_default(),
        context_id: Some(record.context_id.clone()),
        task_id: (!record.task_id.is_empty()).then(|| record.task_id.clone()),
        metadata: None,
    }
}

/// Extracts the sending agent identity from message metadata.
#[must_use]
pub fn sender_of(message: &Message) -> String {
    message
        .metadata_str("senderAgent")
        .or_else(|| message.metadata_str("sender"))
        .unwrap_or_else(|| "client".to_owned())
}

fn parse_skills(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

/// Past tense label of a decision, used in the human readable timeline.
const fn decision_label(decision: ApprovalDecision) -> &'static str {
    match decision {
        ApprovalDecision::Approve => "approved",
        ApprovalDecision::Reject => "rejected",
        ApprovalDecision::Cancel => "cancelled",
    }
}

/// Imperative verb of a decision, used in the audit trail.
const fn decision_verb(decision: ApprovalDecision) -> &'static str {
    match decision {
        ApprovalDecision::Approve => "approve",
        ApprovalDecision::Reject => "reject",
        ApprovalDecision::Cancel => "cancel",
    }
}

/// Builds a task snapshot from a stored row, without history or artifacts.
fn task_snapshot(record: &TaskRecord) -> Task {
    Task {
        id: record.id.clone(),
        context_id: record.context_id.clone(),
        status: TaskStatus {
            state: record.status,
            message: None,
            timestamp: record.updated_at.clone(),
        },
        artifacts: None,
        history: None,
        metadata: record.metadata_json.as_deref().and_then(|raw| serde_json::from_str(raw).ok()),
    }
}

/// Rebuilds a protocol push configuration from its stored row.
fn push_config_from_record(record: PushNotificationRecord) -> TaskPushNotificationConfig {
    TaskPushNotificationConfig {
        task_id: record.task_id,
        push_notification_config: PushNotificationConfig {
            id: Some(record.id),
            url: record.url,
            token: record.token,
            authentication: record
                .auth_json
                .as_deref()
                .and_then(|raw| serde_json::from_str(raw).ok()),
        },
    }
}

/// Maps a service error to a JSON-RPC error code.
#[must_use]
pub fn error_code(error: &Error) -> i64 {
    match error {
        Error::Protocol(ProtocolError::MethodNotFound(_)) => -32601,
        Error::Protocol(ProtocolError::CardVerificationFailed(_)) => -32004,
        Error::Protocol(ProtocolError::InvalidParams(_))
        | Error::Protocol(ProtocolError::InvalidStateTransition(_))
        | Error::Protocol(ProtocolError::AgentNotFound(_))
        | Error::Protocol(ProtocolError::SkillNotFound(_))
        | Error::Protocol(ProtocolError::PushNotification(_)) => -32602,
        Error::Protocol(_) => -32600,
        Error::Storage(crate::error::StorageError::TaskNotFound(_))
        | Error::Storage(crate::error::StorageError::ApprovalNotFound(_))
        | Error::Storage(crate::error::StorageError::AgentNotFound(_))
        | Error::Storage(crate::error::StorageError::PushNotificationNotFound(_))
        | Error::NotFound(_) => -32001,
        Error::Conflict(_) | Error::InvalidState(_) => -32002,
        Error::Unauthorized(_) => -32003,
        Error::Forbidden(_) => -32004,
        Error::Validation(_) | Error::Config(_) => -32602,
        _ => -32603,
    }
}

/// Whether an event ends the stream of its task.
#[must_use]
pub fn event_is_terminal(event: &StreamEvent) -> bool {
    match event {
        StreamEvent::Task(task) => task.status.state.is_terminal(),
        StreamEvent::StatusUpdate(event) => event.final_event || event.status.state.is_terminal(),
        StreamEvent::Error { .. } => true,
        StreamEvent::Message(_) | StreamEvent::ArtifactUpdate(_) => false,
    }
}
