//! End to end acceptance suite of the A2A Warden legacy release.
//!
//! The suite starts a real warden over HTTP and verifies the whole supervised
//! flow: interception, human approval, delegation, capture, and the decision
//! log. It runs against SQLite, and against PostgreSQL when `DATABASE_URL`
//! points to a PostgreSQL server (acceptance criterion CA-legacy-3).

use a2a_warden_e2e::{
    build_config, decide, pending_approvals, rpc, rpc_with_version, send_params, start_warden,
    task_detail, timeline_texts, wait_for, WardenServer,
};
use serde_json::Value;
use std::time::Duration;

/// Runs the acceptance flow against the given warden.
async fn acceptance_flow(warden: &WardenServer, stub: &a2a_warden_e2e::StubAgentServer) {
    let base = warden.base_url();

    // 1. The client sends a task to the warden.
    let response = rpc(&base, "message/send", send_params("hello warden", Some("echo"))).await;
    assert!(response.get("error").is_none(), "unexpected error: {response}");
    let task = &response["result"];
    let task_id = task["id"]
        .as_str()
        .unwrap_or_else(|| panic!("the warden must return a task: {response}"))
        .to_owned();

    // 2. The task is paused and nothing reached the remote agent.
    assert_eq!(
        task["status"]["state"], "input-required",
        "the task must pause for a human decision: {task}"
    );
    assert_eq!(stub.received_count(), 0, "nothing may reach the remote agent before the decision");

    // 3. The task is visible in the approvals inbox.
    let approvals = pending_approvals(&base).await;
    assert_eq!(approvals.len(), 1, "one pending approval is expected");
    let approval_id = approvals[0]["id"].as_str().expect("approval id").to_owned();
    assert_eq!(approvals[0]["taskId"], task_id);

    // 4. The supervisor approves through the administration API.
    let decision = decide(&base, &approval_id, "approve", "supervisor-1").await;
    assert!(decision.get("error").is_none(), "{decision}");
    assert_eq!(decision["task"]["status"]["state"], "completed");
    assert_eq!(decision["approval"]["status"], "approved");
    assert_eq!(decision["approval"]["decidedBy"], "supervisor-1");

    // 5. The remote agent received the task and the answer was recorded.
    assert_eq!(stub.received_count(), 1, "the remote agent must be called once");
    assert_eq!(stub.received_texts()[0], "hello warden");

    // 6. The client obtains the final result.
    let result =
        rpc(&base, "tasks/get", serde_json::json!({"id": task_id, "historyLength": 20})).await;
    assert_eq!(result["result"]["status"]["state"], "completed");
    let answer =
        result["result"]["status"]["message"]["parts"][0]["text"].as_str().expect("answer text");
    assert_eq!(answer, "echo: hello warden");

    let artifacts = result["result"]["artifacts"]
        .as_array()
        .expect("the task must expose the artifacts of the remote agent");
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0]["artifactId"], "stub-artifact-1");
    assert_eq!(artifacts[0]["parts"][0]["text"], "echo: hello warden");

    // 7. The timeline contains the original message, the human decision, the
    //    forwarded message, and the response.
    let detail = task_detail(&base, &task_id).await;
    let texts = timeline_texts(&detail);
    assert!(texts.iter().any(|text| text == "hello warden"), "original message: {texts:?}");
    assert!(
        texts.iter().any(|text| text.contains("Human approval granted by supervisor-1")),
        "human decision: {texts:?}"
    );
    assert_eq!(
        texts.iter().filter(|text| *text == "hello warden").count(),
        2,
        "the message appears twice: original and forwarded: {texts:?}"
    );
    assert!(
        texts.iter().any(|text| text.contains("echo: hello warden")),
        "remote response: {texts:?}"
    );

    // 8. The audit trail records every transition with its actor.
    let actions: Vec<String> = detail["audit"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry["action"].as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default();
    for expected in [
        "message.received",
        "approval.requested",
        "approval.decided",
        "task.forwarded",
        "response.received",
    ] {
        assert!(
            actions.iter().any(|action| action == expected),
            "missing audit action '{expected}': {actions:?}"
        );
    }
}

/// Runs the rejection flow: the task never reaches the remote agent.
async fn rejection_flow(warden: &WardenServer, stub: &a2a_warden_e2e::StubAgentServer) {
    let base = warden.base_url();
    let before = stub.received_count();

    let response = rpc(&base, "message/send", send_params("please reject", Some("echo"))).await;
    let task_id = response["result"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("the warden must return a task: {response}"))
        .to_owned();
    assert_eq!(response["result"]["status"]["state"], "input-required");

    let approvals = pending_approvals(&base).await;
    let approval_id = approvals
        .iter()
        .find(|approval| approval["taskId"] == task_id)
        .and_then(|approval| approval["id"].as_str())
        .unwrap_or_else(|| panic!("no pending approval for task {task_id}: {approvals:?}"))
        .to_owned();

    let decision = decide(&base, &approval_id, "reject", "supervisor-2").await;
    assert_eq!(decision["task"]["status"]["state"], "rejected");
    assert_eq!(decision["approval"]["status"], "rejected");
    assert_eq!(stub.received_count(), before, "a rejected task is never delegated");

    // The timeline records the rejection.
    let detail = task_detail(&base, &task_id).await;
    let texts = timeline_texts(&detail);
    assert!(
        texts.iter().any(|text| text.contains("rejected by supervisor-2")),
        "rejection in the timeline: {texts:?}"
    );
}

/// Runs the cancellation flow.
async fn cancellation_flow(warden: &WardenServer, stub: &a2a_warden_e2e::StubAgentServer) {
    let base = warden.base_url();
    let before = stub.received_count();

    let response = rpc(&base, "message/send", send_params("please cancel", Some("echo"))).await;
    let task_id = response["result"]["id"].as_str().expect("task id").to_owned();

    let cancelled = rpc(&base, "tasks/cancel", serde_json::json!({"id": task_id})).await;
    assert_eq!(cancelled["result"]["status"]["state"], "canceled");
    assert_eq!(stub.received_count(), before, "a cancelled task is never delegated");

    let detail = task_detail(&base, &task_id).await;
    assert_eq!(detail["approval"]["status"], "cancelled");
}

/// The SQLite acceptance run.
#[tokio::test]
async fn acceptance_sqlite() {
    let stub = a2a_warden_e2e::StubAgentServer::start().await;
    let agent_url = format!("http://{}", stub.address);

    let dir = tempfile::tempdir().expect("temp dir");
    let database_path = a2a_warden_e2e::temp_database(&dir);
    let config = build_config(&agent_url, &database_path);

    let warden = start_warden(&config).await;
    let base = warden.base_url();

    // The warden answers immediately after the listener is up.
    let healthy = wait_for(Duration::from_secs(5), || {
        let base = base.clone();
        async move { reqwest::get(format!("{base}/api/v1/admin/health")).await.is_ok() }
    })
    .await;
    assert!(healthy, "the warden must answer the health endpoint");

    acceptance_flow(&warden, &stub).await;
    rejection_flow(&warden, &stub).await;
    cancellation_flow(&warden, &stub).await;

    // A missing version header is negotiated as v1.0 instead of rejected.
    let response =
        rpc_with_version(&base, "tasks/get", serde_json::json!({"id": "unknown"}), None).await;
    assert_eq!(response.status(), 200, "a missing A2A-Version header is negotiated");
    let body: serde_json::Value = response.json().await.expect("negotiated response");
    assert!(body.get("error").is_some(), "the unknown task must be reported");
}

/// The PostgreSQL acceptance run, skipped when no server is configured.
#[tokio::test]
async fn acceptance_postgres() {
    let stub = a2a_warden_e2e::StubAgentServer::start().await;
    let agent_url = format!("http://{}", stub.address);

    let Some(config) = a2a_warden_e2e::postgres_config(&agent_url) else {
        eprintln!("DATABASE_URL does not point to PostgreSQL, acceptance run skipped");
        return;
    };

    let warden = start_warden(&config).await;
    acceptance_flow(&warden, &stub).await;
    rejection_flow(&warden, &stub).await;
    cancellation_flow(&warden, &stub).await;
}

/// The console is served by the same binary, with no external dependency.
#[tokio::test]
async fn console_is_embedded() {
    let stub = a2a_warden_e2e::StubAgentServer::start().await;
    let agent_url = format!("http://{}", stub.address);
    let dir = tempfile::tempdir().expect("temp dir");
    let database_path = a2a_warden_e2e::temp_database(&dir);
    let config = build_config(&agent_url, &database_path);

    let warden = start_warden(&config).await;
    let base = warden.base_url();

    let response = reqwest::get(format!("{base}/ui")).await.expect("console");
    assert!(response.status().is_success());
    let html = response.text().await.expect("console body");
    assert!(html.contains("<base href=\"/ui/\">"), "{html}");
    assert!(html.contains("A2A Warden"), "{html}");
}

/// The Agent Card is published on the well known path.
#[tokio::test]
async fn agent_card_is_published() {
    let stub = a2a_warden_e2e::StubAgentServer::start().await;
    let agent_url = format!("http://{}", stub.address);
    let dir = tempfile::tempdir().expect("temp dir");
    let database_path = a2a_warden_e2e::temp_database(&dir);
    let config = build_config(&agent_url, &database_path);

    let warden = start_warden(&config).await;
    let base = warden.base_url();

    let card: Value = reqwest::get(format!("{base}/.well-known/agent-card.json"))
        .await
        .expect("agent card")
        .json()
        .await
        .expect("card body");
    assert_eq!(card["name"], "A2A Warden");
    assert_eq!(card["protocolVersion"], "1.0");
}
