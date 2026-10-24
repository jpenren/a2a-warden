//! Cross-module core tests: the full interception and approval flow exercised
//! with an in-memory store and a scripted agent client.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::json;

use crate::config::AgentConfig;
use crate::error::{Error, ProtocolError};
use crate::policy::{PolicyConfig, PolicyEffect, PolicyRule};
use crate::protocol::{
    ListTasksParams, Message, MessageSendParams, Part, PushNotificationAuthentication,
    PushNotificationConfig, Role, StreamEvent, Task, TaskState,
};
use crate::push::{PushNotifier, PushPolicy};
use crate::service::{AgentClient, TaskDetail, WardenService};
use crate::stores::{
    errors, AgentRecord, AgentStore, ApprovalRecord, ApprovalStatus, ApprovalStore, ArtifactRecord,
    AuditRecord, AuditStore, MessageRecord, PushNotificationRecord, PushNotificationStore, Store,
    TaskFilter, TaskRecord, TaskStore,
};
use crate::verification::{CardVerification, CardVerifier, VerificationPolicy};
use crate::workflow::ApprovalDecision;
use serde_json::Value;
use url::Url;

/// In-memory implementation of every store trait.
#[derive(Default)]
struct MemoryStore {
    tasks: Mutex<HashMap<String, TaskRecord>>,
    messages: Mutex<Vec<MessageRecord>>,
    artifacts: Mutex<Vec<ArtifactRecord>>,
    approvals: Mutex<HashMap<String, ApprovalRecord>>,
    audit: Mutex<Vec<AuditRecord>>,
    agents: Mutex<HashMap<String, AgentRecord>>,
    push_configs: Mutex<Vec<PushNotificationRecord>>,
}

#[async_trait]
impl TaskStore for MemoryStore {
    async fn create_task(&self, task: &TaskRecord) -> Result<(), Error> {
        let mut tasks = self.tasks.lock().expect("tasks lock");
        if tasks.contains_key(&task.id) {
            return Err(errors::duplicate("task"));
        }
        tasks.insert(task.id.clone(), task.clone());
        Ok(())
    }

    async fn get_task(&self, id: &str) -> Result<TaskRecord, Error> {
        self.tasks
            .lock()
            .expect("tasks lock")
            .get(id)
            .cloned()
            .ok_or_else(|| errors::task_not_found(id))
    }

    async fn update_task_status(
        &self,
        id: &str,
        status: TaskState,
        updated_at: &str,
    ) -> Result<(), Error> {
        let mut tasks = self.tasks.lock().expect("tasks lock");
        let task = tasks.get_mut(id).ok_or_else(|| errors::task_not_found(id))?;
        task.status = status;
        task.updated_at = updated_at.to_owned();
        Ok(())
    }

    async fn set_remote_task_id(&self, id: &str, remote_task_id: &str) -> Result<(), Error> {
        let mut tasks = self.tasks.lock().expect("tasks lock");
        let task = tasks.get_mut(id).ok_or_else(|| errors::task_not_found(id))?;
        task.remote_task_id = Some(remote_task_id.to_owned());
        Ok(())
    }

    async fn list_tasks(&self, filter: &TaskFilter) -> Result<Vec<TaskRecord>, Error> {
        let tasks = self.tasks.lock().expect("tasks lock");
        let mut result: Vec<TaskRecord> = tasks
            .values()
            .filter(|task| filter.status.is_none_or(|status| task.status == status))
            .filter(|task| filter.agent.as_ref().is_none_or(|agent| &task.agent_id == agent))
            .filter(|task| {
                filter.context_id.as_ref().is_none_or(|context| &task.context_id == context)
            })
            .cloned()
            .collect();
        result.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        if let Some(limit) = filter.limit {
            result.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        }
        Ok(result)
    }

    async fn append_message(&self, message: &MessageRecord) -> Result<(), Error> {
        self.messages.lock().expect("messages lock").push(message.clone());
        Ok(())
    }

    async fn list_messages(&self, task_id: &str) -> Result<Vec<MessageRecord>, Error> {
        Ok(self
            .messages
            .lock()
            .expect("messages lock")
            .iter()
            .filter(|message| message.task_id == task_id)
            .cloned()
            .collect())
    }

    async fn append_artifact(&self, artifact: &ArtifactRecord) -> Result<(), Error> {
        self.artifacts.lock().expect("artifacts lock").push(artifact.clone());
        Ok(())
    }

    async fn list_artifacts(&self, task_id: &str) -> Result<Vec<ArtifactRecord>, Error> {
        Ok(self
            .artifacts
            .lock()
            .expect("artifacts lock")
            .iter()
            .filter(|artifact| artifact.task_id == task_id)
            .cloned()
            .collect())
    }
}

#[async_trait]
impl ApprovalStore for MemoryStore {
    async fn create_approval(&self, approval: &ApprovalRecord) -> Result<(), Error> {
        self.approvals
            .lock()
            .expect("approvals lock")
            .insert(approval.id.clone(), approval.clone());
        Ok(())
    }

    async fn get_approval(&self, id: &str) -> Result<ApprovalRecord, Error> {
        self.approvals
            .lock()
            .expect("approvals lock")
            .get(id)
            .cloned()
            .ok_or_else(|| errors::approval_not_found(id))
    }

    async fn pending_approval_for_task(
        &self,
        task_id: &str,
    ) -> Result<Option<ApprovalRecord>, Error> {
        Ok(self
            .approvals
            .lock()
            .expect("approvals lock")
            .values()
            .find(|approval| {
                approval.task_id == task_id && approval.status == ApprovalStatus::Pending
            })
            .cloned())
    }

    async fn list_approvals(
        &self,
        status: Option<ApprovalStatus>,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<ApprovalRecord>, Error> {
        let mut approvals: Vec<ApprovalRecord> = self
            .approvals
            .lock()
            .expect("approvals lock")
            .values()
            .filter(|approval| status.is_none_or(|expected| approval.status == expected))
            .cloned()
            .collect();
        approvals.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        let start = offset.map(|o| o as usize).unwrap_or(0);
        let end = limit.map(|l| start + l as usize).unwrap_or(approvals.len());
        Ok(approvals.into_iter().skip(start).take(end - start).collect())
    }

    async fn decide_approval(
        &self,
        id: &str,
        status: ApprovalStatus,
        decided_by: &str,
        decided_at: &str,
        reason: Option<&str>,
    ) -> Result<ApprovalRecord, Error> {
        let mut approvals = self.approvals.lock().expect("approvals lock");
        let approval = approvals.get_mut(id).ok_or_else(|| errors::approval_not_found(id))?;
        if approval.status != ApprovalStatus::Pending {
            return Err(Error::Conflict(format!("approval {id} already decided")));
        }
        approval.status = status;
        approval.decided_by = Some(decided_by.to_owned());
        approval.decided_at = Some(decided_at.to_owned());
        approval.reason = reason.map(str::to_owned);
        Ok(approval.clone())
    }
}

#[async_trait]
impl AuditStore for MemoryStore {
    async fn append_audit(&self, entry: &AuditRecord) -> Result<(), Error> {
        self.audit.lock().expect("audit lock").push(entry.clone());
        Ok(())
    }

    async fn list_audit_for_task(&self, task_id: &str) -> Result<Vec<AuditRecord>, Error> {
        Ok(self
            .audit
            .lock()
            .expect("audit lock")
            .iter()
            .filter(|entry| entry.task_id.as_deref() == Some(task_id))
            .cloned()
            .collect())
    }

    async fn last_audit_hash(&self) -> Result<Option<String>, Error> {
        Ok(self
            .audit
            .lock()
            .expect("audit lock")
            .last()
            .and_then(|entry| entry.hash.clone()))
    }

    async fn list_all_audit(&self) -> Result<Vec<AuditRecord>, Error> {
        Ok(self.audit.lock().expect("audit lock").clone())
    }
}

#[async_trait]
impl AgentStore for MemoryStore {
    async fn upsert_agent(&self, agent: &AgentRecord) -> Result<(), Error> {
        self.agents.lock().expect("agents lock").insert(agent.name.clone(), agent.clone());
        Ok(())
    }

    async fn get_agent(&self, name: &str) -> Result<AgentRecord, Error> {
        self.agents
            .lock()
            .expect("agents lock")
            .get(name)
            .cloned()
            .ok_or_else(|| errors::agent_not_found(name))
    }

    async fn list_agents(&self) -> Result<Vec<AgentRecord>, Error> {
        Ok(self.agents.lock().expect("agents lock").values().cloned().collect())
    }
}

#[async_trait]
impl PushNotificationStore for MemoryStore {
    async fn upsert_push_config(&self, record: &PushNotificationRecord) -> Result<(), Error> {
        let mut configs = self.push_configs.lock().expect("push configs lock");
        configs.retain(|config| !(config.task_id == record.task_id && config.id == record.id));
        configs.push(record.clone());
        Ok(())
    }

    async fn get_push_config(
        &self,
        task_id: &str,
        id: &str,
    ) -> Result<PushNotificationRecord, Error> {
        self.push_configs
            .lock()
            .expect("push configs lock")
            .iter()
            .find(|config| config.task_id == task_id && config.id == id)
            .cloned()
            .ok_or_else(|| errors::push_config_not_found(&format!("{task_id}/{id}")))
    }

    async fn list_push_configs(&self, task_id: &str) -> Result<Vec<PushNotificationRecord>, Error> {
        Ok(self
            .push_configs
            .lock()
            .expect("push configs lock")
            .iter()
            .filter(|config| config.task_id == task_id)
            .cloned()
            .collect())
    }

    async fn delete_push_config(&self, task_id: &str, id: &str) -> Result<(), Error> {
        let mut configs = self.push_configs.lock().expect("push configs lock");
        let before = configs.len();
        configs.retain(|config| !(config.task_id == task_id && config.id == id));
        if configs.len() == before {
            return Err(errors::push_config_not_found(&format!("{task_id}/{id}")));
        }
        Ok(())
    }
}

/// Scripted agent client recording every call.
#[derive(Default)]
struct FakeClient {
    sent: AtomicUsize,
    cancelled: AtomicUsize,
    fail: Mutex<bool>,
    reply: Mutex<String>,
    forwarded: Mutex<Vec<Option<serde_json::Map<String, serde_json::Value>>>>,
}

impl FakeClient {
    fn new() -> Self {
        Self {
            sent: AtomicUsize::new(0),
            cancelled: AtomicUsize::new(0),
            fail: Mutex::new(false),
            reply: Mutex::new("echo: hello".to_owned()),
            forwarded: Mutex::new(Vec::new()),
        }
    }

    fn sent(&self) -> usize {
        self.sent.load(Ordering::SeqCst)
    }

    /// Metadata of every message forwarded to the remote agent, in order.
    fn forwarded_metadata(&self) -> Vec<Option<serde_json::Map<String, serde_json::Value>>> {
        self.forwarded.lock().expect("forwarded lock").clone()
    }
}

#[async_trait]
impl AgentClient for FakeClient {
    async fn send_message(&self, _agent_url: &Url, message: &Message) -> Result<Task, Error> {
        self.sent.fetch_add(1, Ordering::SeqCst);
        self.forwarded.lock().expect("forwarded lock").push(message.metadata.clone());
        if *self.fail.lock().expect("fail lock") {
            return Err(Error::HttpClient("connection refused".to_owned()));
        }
        let reply = self.reply.lock().expect("reply lock").clone();
        Ok(Task {
            id: format!("remote-{}", self.sent()),
            context_id: message.context_id.clone().unwrap_or_default(),
            status: crate::protocol::TaskStatus {
                state: TaskState::Completed,
                message: Some(Message {
                    message_id: format!("remote-msg-{}", self.sent()),
                    role: Role::Agent,
                    parts: vec![Part::Text { text: reply }],
                    context_id: message.context_id.clone(),
                    task_id: None,
                    metadata: None,
                }),
                timestamp: crate::now(),
            },
            artifacts: None,
            history: None,
            metadata: None,
        })
    }

    async fn cancel_task(&self, _agent_url: &Url, _task_id: &str) -> Result<TaskState, Error> {
        self.cancelled.fetch_add(1, Ordering::SeqCst);
        Ok(TaskState::Canceled)
    }
}

async fn setup(policy_config: Option<PolicyConfig>) -> (WardenService, Arc<FakeClient>) {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let fake = Arc::new(FakeClient::new());
    let client: Arc<dyn AgentClient> = fake.clone();
    let mut service = WardenService::new(store, client);
    if let Some(config) = policy_config {
        service = service.with_policy_config(config);
    }
    service.register_agents(&[agent_config()]).await.expect("register agents");
    (service, fake)
}

fn agent_config() -> AgentConfig {
    AgentConfig {
        name: "echo-agent".to_owned(),
        url: Url::parse("http://127.0.0.1:9001").expect("valid url"),
        skills: vec!["echo".to_owned()],
        card_json: None,
        metadata: HashMap::new(),
    }
}

fn send_params(text: &str, skill: Option<&str>) -> MessageSendParams {
    let mut message = Message::user_text(text);
    let mut metadata = serde_json::Map::new();
    metadata.insert("senderAgent".to_owned(), json!("client-1"));
    if let Some(skill) = skill {
        metadata.insert("skill".to_owned(), json!(skill));
    }
    message.metadata = Some(metadata);
    MessageSendParams { message, configuration: None }
}

#[tokio::test]
async fn interception_holds_then_approval_forwards_and_completes() {
    let (service, client) = setup(None).await;

    let task = service.send_message(send_params("hello", Some("echo"))).await.expect("send");
    assert_eq!(task.status.state, TaskState::InputRequired);
    assert_eq!(client.sent(), 0, "nothing must reach the remote agent");

    let approvals = service.list_approvals(Some(ApprovalStatus::Pending), None, None).await.expect("approvals");
    assert_eq!(approvals.len(), 1);

    let task = service
        .decide_approval(&approvals[0].id, ApprovalDecision::Approve, "alice", Some("looks good"))
        .await
        .expect("approve");
    assert_eq!(task.status.state, TaskState::Completed);
    assert_eq!(client.sent(), 1);
    assert_eq!(task.status.message.as_ref().map(Message::text), Some("echo: hello".to_owned()));

    let detail: TaskDetail = service.task_detail(&task.id).await.expect("detail");
    let timeline: Vec<String> = detail
        .messages
        .iter()
        .map(|message| {
            serde_json::from_str::<Vec<Part>>(&message.parts_json)
                .expect("parts")
                .iter()
                .filter_map(Part::as_text)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    assert_eq!(timeline.len(), 5, "timeline: {timeline:?}");
    assert_eq!(timeline[0], "hello");
    assert!(timeline[1].contains("Human approval required"));
    assert!(timeline[2].contains("Human approval granted by alice"));
    assert_eq!(timeline[3], "hello");
    assert!(timeline[4].contains("echo: hello"));

    let actions: Vec<&str> = detail.audit.iter().map(|entry| entry.action.as_str()).collect();
    assert!(actions.contains(&"message.received"));
    assert!(actions.contains(&"approval.requested"));
    assert!(actions.contains(&"approval.decided"));
    assert!(actions.contains(&"task.forwarded"));
    assert!(actions.contains(&"response.received"));
    assert_eq!(detail.approval.map(|a| a.status), Some(ApprovalStatus::Approved));
}

#[tokio::test]
async fn rejection_never_reaches_the_remote_agent() {
    let (service, client) = setup(None).await;
    service.send_message(send_params("hello", None)).await.expect("send");
    let approval =
        service.list_approvals(Some(ApprovalStatus::Pending), None, None).await.expect("approvals").remove(0);

    let task = service
        .decide_approval(&approval.id, ApprovalDecision::Reject, "bob", Some("no"))
        .await
        .expect("reject");
    assert_eq!(task.status.state, TaskState::Rejected);
    assert_eq!(client.sent(), 0);

    // A decision can only be applied once.
    assert!(service
        .decide_approval(&approval.id, ApprovalDecision::Approve, "bob", None)
        .await
        .is_err());
}

#[tokio::test]
async fn cancellation_resolves_the_pending_approval() {
    let (service, client) = setup(None).await;
    let task = service.send_message(send_params("hello", None)).await.expect("send");

    let cancelled = service.cancel_task(&task.id, "carol").await.expect("cancel");
    assert_eq!(cancelled.status.state, TaskState::Canceled);
    assert_eq!(client.sent(), 0);

    let approval = service
        .get_approval(&service.list_approvals(None, None, None).await.expect("approvals").remove(0).id)
        .await
        .expect("approval");
    assert_eq!(approval.status, ApprovalStatus::Cancelled);

    // Cancelling twice is a conflict.
    assert!(service.cancel_task(&task.id, "carol").await.is_err());
}

#[tokio::test]
async fn approval_preserves_the_metadata_of_the_forwarded_message() {
    // The message is rebuilt from the stored row when the task is resumed,
    // and the row does not keep the metadata. The task row does, so the
    // forwarding must restore it: the remote agent needs the skill, the
    // sender, and the routing information.
    let (service, client) = setup(None).await;

    service.send_message(send_params("hello", Some("echo"))).await.expect("send");
    let approvals = service.list_approvals(Some(ApprovalStatus::Pending), None, None).await.expect("approvals");
    service
        .decide_approval(&approvals[0].id, ApprovalDecision::Approve, "alice", None)
        .await
        .expect("approve");

    let forwarded = client.forwarded_metadata();
    assert_eq!(forwarded.len(), 1, "the message must be forwarded once");
    let metadata = forwarded[0].as_ref().expect("the metadata must be preserved");
    assert_eq!(metadata.get("skill").and_then(|value| value.as_str()), Some("echo"));
    assert_eq!(metadata.get("senderAgent").and_then(|value| value.as_str()), Some("client-1"));
}

#[tokio::test]
async fn direct_forwarding_preserves_the_metadata_of_the_message() {
    let mut config = PolicyConfig::new_config();
    config.rules.push(PolicyRule {
        id: "allow-all".to_owned(),
        description: Some("Allow all skills".to_owned()),
        active: true,
        effect: PolicyEffect::Allow,
        condition: "true".to_owned(),
        version: 1,
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    });
    let (service, client) = setup(Some(config)).await;

    service.send_message(send_params("hello", Some("echo"))).await.expect("send");

    let forwarded = client.forwarded_metadata();
    assert_eq!(forwarded.len(), 1);
    let metadata = forwarded[0].as_ref().expect("the metadata must be preserved");
    assert_eq!(metadata.get("skill").and_then(|value| value.as_str()), Some("echo"));
}

#[tokio::test]
async fn direct_forward_without_interception() {
    let mut config = PolicyConfig::new_config();
    config.rules.push(PolicyRule {
        id: "allow-all".to_owned(),
        description: Some("Allow all skills".to_owned()),
        active: true,
        effect: PolicyEffect::Allow,
        condition: "true".to_owned(),
        version: 1,
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    });
    let (service, client) = setup(Some(config)).await;
    let task = service.send_message(send_params("hello", Some("echo"))).await.expect("send");
    assert_eq!(task.status.state, TaskState::Completed);
    assert_eq!(client.sent(), 1);
    assert!(service.list_approvals(None, None, None).await.expect("approvals").is_empty());
}

#[tokio::test]
async fn failing_delegation_marks_the_task_failed() {
    let mut config = PolicyConfig::new_config();
    config.rules.push(PolicyRule {
        id: "allow-all".to_owned(),
        description: Some("Allow all skills".to_owned()),
        active: true,
        effect: PolicyEffect::Allow,
        condition: "true".to_owned(),
        version: 1,
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    });
    let (service, client) = setup(Some(config)).await;
    *client.fail.lock().expect("fail lock") = true;
    let task = service.send_message(send_params("hello", None)).await.expect("send");
    assert_eq!(task.status.state, TaskState::Failed);
}

#[tokio::test]
async fn unknown_target_is_rejected() {
    let mut config = PolicyConfig::new_config();
    config.rules.push(PolicyRule {
        id: "allow-all".to_owned(),
        description: Some("Allow all skills".to_owned()),
        active: true,
        effect: PolicyEffect::Allow,
        condition: "true".to_owned(),
        version: 1,
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    });
    let (service, _) = setup(Some(config)).await;
    assert!(service.send_message(send_params("hello", Some("unknown-skill"))).await.is_err());
}

/// Recorded push notification delivery.
#[derive(Clone)]
struct DeliveryRecord {
    url: String,
    token: Option<String>,
    authentication: Option<PushNotificationAuthentication>,
    payload: serde_json::Value,
}

/// Push notifier that records every delivery instead of using the network.
#[derive(Default)]
struct RecordingNotifier {
    deliveries: Mutex<Vec<DeliveryRecord>>,
}

impl RecordingNotifier {
    fn deliveries(&self) -> usize {
        self.deliveries.lock().expect("deliveries lock").len()
    }
}

#[async_trait]
impl PushNotifier for RecordingNotifier {
    async fn deliver(
        &self,
        url: &Url,
        token: Option<&str>,
        authentication: Option<&PushNotificationAuthentication>,
        payload: &serde_json::Value,
    ) -> Result<(), Error> {
        self.deliveries.lock().expect("deliveries lock").push(DeliveryRecord {
            url: url.to_string(),
            token: token.map(str::to_owned),
            authentication: authentication.cloned(),
            payload: payload.clone(),
        });
        Ok(())
    }
}

async fn setup_with_push(allow_private_addresses: bool) -> (WardenService, Arc<RecordingNotifier>) {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let client: Arc<dyn AgentClient> = Arc::new(FakeClient::new());
    let notifier = Arc::new(RecordingNotifier::default());
    let policy = PushPolicy::new(true, vec!["*".to_owned()], allow_private_addresses);
    let service = WardenService::new(store, client)
        .with_push(policy, notifier.clone() as Arc<dyn PushNotifier>);
    service
        .register_agents(&[agent_config()])
        .await
        .expect("register agents");
    (service, notifier)
}

#[tokio::test]
async fn streaming_sends_a_task_snapshot_and_ends_on_the_terminal_state() {
    let mut config = PolicyConfig::new_config();
    config.rules.push(PolicyRule {
        id: "allow-all".to_owned(),
        description: Some("Allow all skills".to_owned()),
        active: true,
        effect: PolicyEffect::Allow,
        condition: "true".to_owned(),
        version: 1,
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    });
    let (service, _) = setup(Some(config)).await;
    let (sender, mut receiver) = tokio::sync::mpsc::channel(32);
    service
        .send_message_streaming(send_params("hello", Some("echo")), sender)
        .await
        .expect("stream");

    let mut events = Vec::new();
    while let Some(event) = receiver.recv().await {
        events.push(event);
    }
    assert!(!events.is_empty(), "the stream must carry at least one event");
    let last = events.last().expect("last event");
    match last {
        StreamEvent::Task(task) => assert!(task.status.state.is_terminal()),
        other => panic!("unexpected final event: {other:?}"),
    }
    assert!(crate::service::event_is_terminal(last));
}

#[tokio::test]
async fn subscribing_to_a_finished_task_replays_one_terminal_snapshot() {
    let mut config = PolicyConfig::new_config();
    config.rules.push(PolicyRule {
        id: "allow-all".to_owned(),
        description: Some("Allow all skills".to_owned()),
        active: true,
        effect: PolicyEffect::Allow,
        condition: "true".to_owned(),
        version: 1,
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    });
    let (service, _) = setup(Some(config)).await;
    let task = service.send_message(send_params("hello", Some("echo"))).await.expect("send");

    let (sender, mut receiver) = tokio::sync::mpsc::channel(8);
    service.subscribe_task(&task.id, None, sender).await.expect("subscribe");
    let event = receiver.recv().await.expect("snapshot");
    match event {
        StreamEvent::Task(snapshot) => assert_eq!(snapshot.status.state, TaskState::Completed),
        other => panic!("unexpected event: {other:?}"),
    }
    assert!(receiver.recv().await.is_none(), "a terminal task ends the subscription");
}

#[tokio::test]
async fn tasks_are_listed_with_a_keyset_cursor() {
    let mut config = PolicyConfig::new_config();
    config.rules.push(PolicyRule {
        id: "allow-all".to_owned(),
        description: Some("Allow all skills".to_owned()),
        active: true,
        effect: PolicyEffect::Allow,
        condition: "true".to_owned(),
        version: 1,
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    });
    let (service, _) = setup(Some(config)).await;
    for text in ["one", "two", "three"] {
        service.send_message(send_params(text, Some("echo"))).await.expect("send");
    }

    let first = service
        .list_tasks_page(&ListTasksParams { page_size: Some(2), ..ListTasksParams::default() })
        .await
        .expect("first page");
    assert_eq!(first.tasks.len(), 2);
    assert_eq!(first.total_size, Some(3));
    let token = first.next_page_token.expect("next page token");

    let second = service
        .list_tasks_page(&ListTasksParams {
            page_size: Some(2),
            page_token: Some(token),
            ..ListTasksParams::default()
        })
        .await
        .expect("second page");
    assert_eq!(second.tasks.len(), 1);
    assert!(second.next_page_token.is_none());

    let all: Vec<String> =
        first.tasks.into_iter().chain(second.tasks).map(|task| task.id).collect();
    let mut unique = all.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 3, "the pages cover every task exactly once: {all:?}");
}

#[tokio::test]
async fn an_invalid_page_token_is_rejected() {
    let mut config = PolicyConfig::new_config();
    config.rules.push(PolicyRule {
        id: "allow-all".to_owned(),
        description: Some("Allow all skills".to_owned()),
        active: true,
        effect: PolicyEffect::Allow,
        condition: "true".to_owned(),
        version: 1,
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    });
    let (service, _) = setup(Some(config)).await;
    let result = service
        .list_tasks_page(&ListTasksParams {
            page_token: Some("not-a-cursor".to_owned()),
            ..ListTasksParams::default()
        })
        .await;
    assert!(matches!(result, Err(Error::Protocol(_))));
}

#[tokio::test]
async fn push_configurations_are_validated_and_delivered() {
    let (service, notifier) = setup_with_push(true).await;
    let task = service.send_message(send_params("hello", Some("echo"))).await.expect("send");

    let config = service
        .create_push_config(
            &task.id,
            PushNotificationConfig {
                id: Some("push-1".to_owned()),
                url: "http://127.0.0.1:9999/hook".to_owned(),
                token: Some("secret".to_owned()),
                authentication: Some(PushNotificationAuthentication {
                    schemes: vec!["Bearer".to_owned()],
                    credentials: Some("credential".to_owned()),
                }),
            },
        )
        .await
        .expect("create push config");
    assert_eq!(config.push_notification_config.id.as_deref(), Some("push-1"));

    // Trigger a new event on the task with the configured callback in place.
    service.cancel_task(&task.id, "alice").await.expect("cancel");

    for _ in 0..50 {
        if notifier.deliveries() > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let deliveries = notifier.deliveries.lock().expect("deliveries lock").clone();
    let delivery = deliveries.last().expect("delivery");
    assert_eq!(delivery.url, "http://127.0.0.1:9999/hook");
    assert_eq!(delivery.token.as_deref(), Some("secret"));
    assert_eq!(
        delivery.authentication.as_ref().and_then(|auth| auth.schemes.first()).map(String::as_str),
        Some("Bearer")
    );
    assert!(
        delivery.payload.get("task").is_some(),
        "v1.0 push payload wraps the task: {}",
        delivery.payload
    );
}

#[tokio::test]
async fn push_rejects_a_private_address_when_the_policy_forbids_it() {
    let (service, _) = setup_with_push(false).await;
    let task = service.send_message(send_params("hello", Some("echo"))).await.expect("send");
    let error = service
        .create_push_config(
            &task.id,
            PushNotificationConfig {
                id: None,
                url: "http://127.0.0.1:9999/hook".to_owned(),
                token: None,
                authentication: None,
            },
        )
        .await
        .expect_err("loopback must be rejected");
    assert!(matches!(error, Error::Forbidden(_)), "{error}");
}

#[tokio::test]
async fn push_rejects_a_host_outside_the_allow_list() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let client: Arc<dyn AgentClient> = Arc::new(FakeClient::new());
    let policy = PushPolicy::new(true, vec!["example.com".to_owned()], false);
    let service = WardenService::new(store, client)
        .with_push(policy, Arc::new(RecordingNotifier::default()) as Arc<dyn PushNotifier>);
    service
        .register_agents(&[agent_config()])
        .await
        .expect("register agents");
    let task = service.send_message(send_params("hello", None)).await.expect("send");
    assert!(service
        .create_push_config(
            &task.id,
            PushNotificationConfig {
                id: None,
                url: "https://evil.test/hook".to_owned(),
                token: None,
                authentication: None,
            },
        )
        .await
        .is_err());
}

#[tokio::test]
async fn expired_tasks_are_cancelled_with_their_pending_approval() {
    let (service, _) = setup(None).await;
    let task = service.send_message(send_params("hello", Some("echo"))).await.expect("send");
    assert_eq!(task.status.state, TaskState::InputRequired);

    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    let expired =
        service.expire_stale_tasks(std::time::Duration::from_millis(1)).await.expect("expire");
    assert_eq!(expired, 1);

    let snapshot = service.get_task(&task.id, None).await.expect("task");
    assert_eq!(snapshot.status.state, TaskState::Canceled);
    let approval = service.list_approvals(None, None, None).await.expect("approvals").remove(0);
    assert_eq!(approval.status, ApprovalStatus::Cancelled);
    let actions: Vec<String> = service
        .task_detail(&task.id)
        .await
        .expect("detail")
        .audit
        .into_iter()
        .map(|entry| entry.action)
        .collect();
    assert!(actions.iter().any(|action| action == "task.expired"), "{actions:?}");
}

#[tokio::test]
async fn concurrent_cancellations_resolve_exactly_once() {
    let (service, _) = setup(None).await;
    let task = service.send_message(send_params("hello", Some("echo"))).await.expect("send");

    let mut handles = Vec::new();
    for index in 0..8 {
        let service = service.clone();
        let task_id = task.id.clone();
        handles.push(tokio::spawn(async move {
            service.cancel_task(&task_id, &format!("actor-{index}")).await
        }));
    }
    let mut successes = 0;
    let mut conflicts = 0;
    for handle in handles {
        match handle.await.expect("join") {
            Ok(_) => successes += 1,
            Err(Error::Conflict(_)) => conflicts += 1,
            Err(other) => panic!("unexpected error: {other}"),
        }
    }
    assert_eq!(successes, 1, "only one cancellation may win");
    assert_eq!(conflicts, 7);
    assert_eq!(
        service.get_task(&task.id, None).await.expect("task").status.state,
        TaskState::Canceled
    );
}

/// Verifier double: returns a scripted outcome without touching cryptography.
struct ScriptedVerifier {
    outcome: CardVerification,
}

#[async_trait]
impl CardVerifier for ScriptedVerifier {
    async fn verify_card(
        &self,
        _card: &Value,
        require_verification: bool,
        quarantine_on_failure: bool,
    ) -> CardVerification {
        assert!(require_verification, "registration must demand verification");
        let mut outcome = self.outcome.clone();
        outcome.quarantined &= quarantine_on_failure;
        outcome
    }
}

fn agent_with_card(name: &str, _skill: &str) -> AgentConfig {
    let mut agent = agent_config();
    agent.card_json = Some(json!({ "name": name, "version": "1.0.0" }));
    agent
}

fn verified_service(
    store: Arc<MemoryStore>,
    outcome: CardVerification,
    quarantine_unverified: bool,
) -> WardenService {
    let client: Arc<dyn AgentClient> = Arc::new(FakeClient::new());
    WardenService::new(store, client).with_verification(
        Arc::new(ScriptedVerifier { outcome }),
        VerificationPolicy { enabled: true, quarantine_unverified },
    )
}

#[tokio::test]
async fn a_verified_card_is_persisted_with_its_metadata_and_audited() {
    let store = Arc::new(MemoryStore::default());
    let service = verified_service(
        store.clone(),
        CardVerification {
            verified: true,
            quarantined: false,
            error: None,
            kid: Some("key-1".to_owned()),
            issuer: Some("https://issuer.example".to_owned()),
            alg: Some("ES256".to_owned()),
        },
        true,
    );

    service.register_agents(&[agent_with_card("echo-agent", "echo")]).await.expect("register");

    let agent = store.get_agent("echo-agent").await.expect("agent");
    assert!(agent.verified);
    assert!(!agent.quarantined);
    assert_eq!(agent.jwks_kid.as_deref(), Some("key-1"));
    assert_eq!(agent.verification_issuer.as_deref(), Some("https://issuer.example"));
    assert_eq!(agent.verification_alg.as_deref(), Some("ES256"));
    assert!(agent.verified_at.is_some());
    assert!(agent.verification_error.is_none());

    let audit = store.audit.lock().expect("audit lock");
    let audit_len = audit.len();
    let first_action = audit[0].action.clone();
    let first_task_id = audit[0].task_id.clone();
    let first_payload = audit[0].payload_json.clone();
    drop(audit);
    assert_eq!(audit_len, 1);
    assert_eq!(first_action, "agent.verified");
    assert!(first_task_id.is_none(), "registration audits are not tied to a task");
    let payload: Value =
        serde_json::from_str(first_payload.as_deref().expect("payload")).expect("json");
    assert_eq!(payload["kid"], "key-1");
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn an_unverifiable_card_is_quarantined_audited_and_never_delegated_to() {
    let store = Arc::new(MemoryStore::default());
    let fake = Arc::new(FakeClient::new());
    let client: Arc<dyn AgentClient> = fake.clone();
    let service = WardenService::new(store.clone(), client).with_verification(
        Arc::new(ScriptedVerifier {
            outcome: CardVerification::failed("signature does not verify", true),
        }),
        VerificationPolicy { enabled: true, quarantine_unverified: true },
    );

    service.register_agents(&[agent_with_card("echo-agent", "echo")]).await.expect("register");

    let agent = store.get_agent("echo-agent").await.expect("agent");
    assert!(!agent.verified);
    assert!(agent.quarantined);
    assert_eq!(agent.verification_error.as_deref(), Some("signature does not verify"));
    assert!(agent.verified_at.is_none());

    let audit = store.audit.lock().expect("audit lock");
    let audit_len = audit.len();
    let first_action = audit[0].action.clone();
    drop(audit);
    assert_eq!(audit_len, 1);
    assert_eq!(first_action, "agent.quarantined");

    let mut params = send_params("hello", None);
    params
        .message
        .metadata
        .as_mut()
        .expect("metadata")
        .insert("targetAgent".to_owned(), json!("echo-agent"));
    let error = service.send_message(params).await.expect_err("must refuse delegation");
    assert!(
        matches!(error, Error::Protocol(ProtocolError::CardVerificationFailed(_))),
        "unexpected error: {error}"
    );
    assert_eq!(fake.sent(), 0, "a quarantined agent must never receive traffic");

    let by_skill = service.send_message(send_params("hello", Some("echo"))).await;
    assert!(matches!(
        by_skill.expect_err("must not resolve"),
        Error::Protocol(ProtocolError::SkillNotFound(_))
    ));
    assert_eq!(fake.sent(), 0);
}

#[tokio::test]
async fn quarantine_can_be_disabled_while_verification_stays_mandatory() {
    let store = Arc::new(MemoryStore::default());
    let service =
        verified_service(store.clone(), CardVerification::failed("bad signature", true), false);

    service.register_agents(&[agent_with_card("echo-agent", "echo")]).await.expect("register");

    let agent = store.get_agent("echo-agent").await.expect("agent");
    assert!(!agent.verified);
    assert!(!agent.quarantined);
    assert_eq!(agent.verification_error.as_deref(), Some("bad signature"));
    let audit = store.audit.lock().expect("audit lock");
    let audit_len = audit.len();
    let first_action = audit[0].action.clone();
    drop(audit);
    assert_eq!(audit_len, 1);
    assert_eq!(first_action, "agent.verification_failed");
}

#[tokio::test]
async fn a_missing_card_is_quarantined_when_verification_is_enabled() {
    let store = Arc::new(MemoryStore::default());
    let service = verified_service(store.clone(), CardVerification::default(), true);

    service.register_agents(&[agent_config()]).await.expect("register");

    let agent = store.get_agent("echo-agent").await.expect("agent");
    assert!(agent.quarantined);
    assert!(agent.verification_error.as_deref().expect("error").contains("no Agent Card"));
}

#[tokio::test]
async fn verification_is_opt_in_for_backwards_compatibility() {
    let store = Arc::new(MemoryStore::default());
    let client: Arc<dyn AgentClient> = Arc::new(FakeClient::new());
    let service = WardenService::new(store.clone(), client);

    service.register_agents(&[agent_config()]).await.expect("register");

    let agent = store.get_agent("echo-agent").await.expect("agent");
    assert!(!agent.verified);
    assert!(!agent.quarantined);
    assert!(agent.verification_error.is_none());
    assert!(store.audit.lock().expect("audit lock").is_empty());
}
