//! End to end test of the per-skill approval gate.
//!
//! The scenario mirrors the pizza shop demo: an agent provides several
//! skills and only some of them are held for a human decision. The suite
//! verifies that the other skills are forwarded without any intervention,
//! that a gated skill never reaches the agent before a decision, and that
//! the decision is recorded in the audit log.

use a2a_warden_core::policy::{PolicyEffect, PolicyRule};
use a2a_warden_e2e::{
    build_config_with_gated_skill, decide, pending_approvals, rpc, send_params, start_warden,
    task_detail, temp_database, WardenServer,
};
use chrono::Utc;
use serde_json::Value;

/// Sends a message addressed to a skill and returns the task.
async fn send_skill(warden: &WardenServer, text: &str, skill: &str) -> Value {
    let base = warden.base_url();
    let response = rpc(&base, "message/send", send_params(text, Some(skill))).await;
    assert!(response.get("error").is_none(), "unexpected error: {response}");
    response["result"].clone()
}

/// Verifies the gate of a single skill.
async fn skill_gate(warden: &WardenServer, stub: &a2a_warden_e2e::StubAgentServer) {
    let base = warden.base_url();

    // 1. A task for a skill that is not gated is forwarded right away.
    let forwarded = send_skill(warden, "plain work", "echo").await;
    assert_eq!(
        forwarded["status"]["state"], "completed",
        "a non gated skill must be forwarded without a decision: {forwarded}"
    );
    assert_eq!(stub.received_count(), 1, "the agent must have received the task");
    assert_eq!(stub.received_texts()[0], "plain work");

    // 2. A task for a gated skill is paused, and nothing reaches the agent.
    let held = send_skill(warden, "risky work", "refund").await;
    assert_eq!(
        held["status"]["state"], "input-required",
        "a gated skill must pause for a human decision: {held}"
    );
    assert_eq!(stub.received_count(), 1, "nothing may reach the agent before the decision");

    // 3. The task shows up in the approvals inbox.
    let approvals = pending_approvals(&base).await;
    assert_eq!(approvals.len(), 1, "one pending approval is expected");
    let approval_id = approvals[0]["id"].as_str().expect("approval id").to_owned();
    assert_eq!(approvals[0]["taskId"], held["id"]);

    // 4. The supervisor approves through the administration API.
    let decision = decide(&base, &approval_id, "approve", "supervisor-1").await;
    assert!(decision.get("error").is_none(), "{decision}");
    assert_eq!(decision["task"]["status"]["state"], "completed");
    assert_eq!(decision["approval"]["decidedBy"], "supervisor-1");

    // 5. The task reached the agent only after the decision.
    assert_eq!(stub.received_count(), 2, "the agent must receive the approved task");
    assert_eq!(stub.received_texts()[1], "risky work");

    // 6. The decision is recorded in the audit log of the task.
    let detail = task_detail(&base, held["id"].as_str().expect("task id")).await;
    let actions: Vec<String> = detail["audit"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry["action"].as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        actions.iter().any(|action| action == "approval.decided"),
        "the decision must be audited: {actions:?}"
    );
    assert!(
        actions.iter().any(|action| action == "task.forwarded"),
        "the delegation must be audited: {actions:?}"
    );
}

/// Runs the gate scenario against SQLite.
#[tokio::test]
async fn a_gated_skill_is_held_and_the_others_are_forwarded() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let database = temp_database(&dir);
    let stub = a2a_warden_e2e::StubAgentServer::start().await;
    let config = build_config_with_gated_skill(&stub.url(), &database);
    let warden = start_warden(&config).await;

    skill_gate(&warden, &stub).await;
}

/// The same scenario against PostgreSQL, when `DATABASE_URL` points to a
/// PostgreSQL server.
#[tokio::test]
async fn a_gated_skill_is_held_on_postgres() {
    let Some(mut config) = a2a_warden_e2e::postgres_config("http://127.0.0.1:9001") else {
        eprintln!("skipped: DATABASE_URL does not point to PostgreSQL");
        return;
    };
    let stub = a2a_warden_e2e::StubAgentServer::start().await;
    config.agents[0].url = stub.url().parse().expect("valid url");
    config.agents[0].skills = vec!["echo".to_owned(), "refund".to_owned()];
    // Replace the escalate-all policy with per-skill gating
    config.policy.rules = vec![
        PolicyRule {
            id: "escalate-refund-skill".to_owned(),
            description: Some("Escalate refund skill tasks for human approval".to_owned()),
            active: true,
            effect: PolicyEffect::Escalate,
            condition: "targetAgent == 'echo-agent' && skill == 'refund'".to_owned(),
            version: 1,
            created_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        },
        PolicyRule {
            id: "allow-echo-skill".to_owned(),
            description: Some("Allow echo skill without approval".to_owned()),
            active: true,
            effect: PolicyEffect::Allow,
            condition: "targetAgent == 'echo-agent' && skill == 'echo'".to_owned(),
            version: 1,
            created_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        },
    ];

    let warden = start_warden(&config).await;
    skill_gate(&warden, &stub).await;
}

/// The warden speaks the A2A v1.0 method names, and falls back to the
/// v0.3 ones for the agents of the older vintage.
#[tokio::test]
async fn the_warden_negotiates_the_method_name() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let database = temp_database(&dir);

    // An agent that only knows the v1.0 names is called with them.
    let modern = a2a_warden_e2e::StubAgentServer::start().await;
    *modern.agent.v1_only.lock().expect("v1_only lock") = true;
    let config = build_config_with_gated_skill(&modern.url(), &database);
    let warden = start_warden(&config).await;
    let task = send_skill(&warden, "modern agent", "echo").await;
    assert_eq!(task["status"]["state"], "completed");
    assert_eq!(
        modern.called_methods(),
        vec!["SendMessage".to_owned()],
        "the canonical method must be used first"
    );

    // An agent that only knows the v0.3 names is called with them, after the
    // canonical name was refused.
    let legacy = a2a_warden_e2e::StubAgentServer::start().await;
    let config = build_config_with_gated_skill(&legacy.url(), &database);
    let warden = start_warden(&config).await;
    let task = send_skill(&warden, "legacy agent", "echo").await;
    assert_eq!(task["status"]["state"], "completed");
    assert_eq!(
        legacy.called_methods(),
        vec!["SendMessage".to_owned(), "message/send".to_owned()],
        "the warden must fall back to the legacy method name"
    );
}
