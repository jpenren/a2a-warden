//! Domain logic for interception, human decisions, and state transitions.
//!
//! Phase 4 replaces the legacy interception rule with Cedar policies.
//! The `evaluate` function uses the Cedar policy engine exclusively.
//! The policy evaluation is synchronous for simplicity.

use crate::error::{Error, ProtocolError};
use crate::protocol::TaskState;
use crate::policy::{evaluate_policies, PolicyEffect, PolicyConfig};

/// Outcome of evaluating the interception rules for an incoming message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interception {
    /// The task may be forwarded to the remote agent immediately.
    Forward,
    /// The task must pause in `input-required` until a human decides.
    HoldForApproval,
}

/// Evaluates the interception rules for a target agent using Cedar policies.
///
/// Returns `Interception::HoldForApproval` when any active policy evaluates
/// to `escalate` or `deny`, otherwise returns `Interception::Forward`.
/// When no policies match, the default is `deny` (deny-by-default, ),
/// so the task is held for approval.
///
/// # Arguments
///
/// * `policy_config` - The Cedar policy configuration.
/// * `sender` - The identity of the sending agent.
/// * `target_agent` - The name of the target/remote agent.
/// * `skill` - The task skill from message metadata.
/// * `message` - The original A2A message (not used in Cedar evaluation).
pub fn evaluate(
    policy_config: &PolicyConfig,
    sender: &str,
    target_agent: &str,
    skill: &str,
) -> Interception {
    let effect = evaluate_policies(
        policy_config,
        sender,
        target_agent,
        skill,
        None, // tenant
        None, // content
    );
    interception_from_effect(effect)
}

/// Evaluates whether a task context would be held for approval by Cedar policies.
///
/// Returns `true` when any active Cedar policy would hold the task for
/// approval (effect is `deny` or `escalate`).
///
/// This is a convenience function useful for migrations and quick checks.
///
/// # Arguments
///
/// * `policy_config` - The Cedar policy configuration.
/// * `sender` - The identity of the sending agent.
/// * `target_agent` - The name of the target/remote agent.
/// * `skill` - The task skill from message metadata.
pub fn would_hold_for_approval(
    policy_config: Option<&PolicyConfig>,
    sender: &str,
    target_agent: &str,
    skill: &str,
) -> bool {
    match policy_config {
        Some(config) => {
            let effect = evaluate_policies(
                config,
                sender,
                target_agent,
                skill,
                None,
                None,
            );
            matches!(effect, PolicyEffect::Deny | PolicyEffect::Escalate)
        }
        None => {
            // Fallback: without policies, we cannot determine, so assume
            // the task should be held (safe default).
            true
        }
    }
}

/// Adapts a [`PolicyEffect`] to the [`Interception`] enum.
///
/// - `allow` -> `Forward`
/// - `deny` or `escalate` -> `HoldForApproval`
#[must_use]
pub fn interception_from_effect(effect: PolicyEffect) -> Interception {
    match effect {
        PolicyEffect::Allow => Interception::Forward,
        _ => Interception::HoldForApproval,
    }
}

/// A human decision on a pending approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// Forward the task to the remote agent.
    Approve,
    /// Refuse the task; it terminates as `rejected`.
    Reject,
    /// Withdraw the task; it terminates as `canceled`.
    Cancel,
}

impl ApprovalDecision {
    /// Parses an administration API decision (`approve`, `reject`, `cancel`).
    pub fn parse(value: &str) -> Result<Self, Error> {
        match value.trim().to_ascii_lowercase().as_str() {
            "approve" | "approved" => Ok(Self::Approve),
            "reject" | "rejected" => Ok(Self::Reject),
            "cancel" | "cancelled" | "canceled" => Ok(Self::Cancel),
            other => Err(ProtocolError::InvalidParams(format!(
                "unknown decision '{other}', expected one of: approve, reject, cancel"
            ))
            .into()),
        }
    }

    /// State the task takes when the decision is applied without forwarding.
    #[must_use]
    pub const fn resulting_state(self) -> TaskState {
        match self {
            Self::Approve => TaskState::Working,
            Self::Reject => TaskState::Rejected,
            Self::Cancel => TaskState::Canceled,
        }
    }
}

/// Returns `true` when the transition is allowed by the protocol state machine.
#[must_use]
pub fn is_valid_transition(from: TaskState, to: TaskState) -> bool {
    if from == to {
        return true;
    }
    match from {
        TaskState::Submitted => !matches!(to, TaskState::AuthRequired | TaskState::Paused),
        TaskState::InputRequired => matches!(
            to,
            TaskState::Working | TaskState::Rejected | TaskState::Canceled | TaskState::Failed
        ),
        TaskState::Working => matches!(
            to,
            TaskState::Completed
                | TaskState::Failed
                | TaskState::Canceled
                | TaskState::InputRequired
                | TaskState::Paused
        ),
        TaskState::Paused => {
            matches!(to, TaskState::Working | TaskState::Canceled | TaskState::Failed)
        }
        TaskState::AuthRequired => {
            matches!(to, TaskState::Working | TaskState::Canceled | TaskState::Failed)
        }
        TaskState::Completed | TaskState::Canceled | TaskState::Failed | TaskState::Rejected => {
            false
        }
    }
}

/// Validates a transition, returning a protocol error when it is not allowed.
pub fn ensure_transition(from: TaskState, to: TaskState) -> Result<(), Error> {
    if is_valid_transition(from, to) {
        Ok(())
    } else {
        Err(ProtocolError::InvalidStateTransition(format!(
            "cannot move task from '{}' to '{}'",
            from.as_str(),
            to.as_str()
        ))
        .into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::PolicyRule;
    use crate::protocol::{Message, Part, Role};

    fn message() -> Message {
        Message {
            message_id: "m1".to_owned(),
            role: Role::User,
            parts: vec![Part::Text { text: "hello".to_owned() }],
            context_id: None,
            task_id: None,
            metadata: None,
        }
    }

    fn message_with_skill(skill: &str) -> Message {
        let mut msg = message();
        let mut meta = serde_json::Map::new();
        meta.insert("skill".to_owned(), serde_json::Value::String(skill.to_owned()));
        msg.metadata = Some(meta);
        msg
    }

    #[test]
    fn legacy_interception_holds_the_task_without_policy() {
        // With no policy rules, deny-by-default holds the task
        assert_eq!(
            evaluate(&PolicyConfig::default(), "sender-1", "echo-agent", "echo"),
            Interception::HoldForApproval
        );
    }

    #[test]
    fn gated_skill_holds_only_that_skill_with_policy() {
        let mut config = PolicyConfig::new_config();
        config.rules.push(PolicyRule {
            id: "rule-1".to_owned(),
            description: Some("Escalate refund-skill".to_owned()),
            active: true,
            effect: PolicyEffect::Escalate,
            condition: "targetAgent == 'echo-agent' && skill == 'refund-order'".to_owned(),
            version: 1,
            created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        });

        assert_eq!(
            evaluate(&config, "sender-1", "echo-agent", "refund-order"),
            Interception::HoldForApproval
        );
    }

    #[test]
    fn forwarded_skill_with_policy() {
        let mut config = PolicyConfig::new_config();
        config.rules.push(PolicyRule {
            id: "rule-1".to_owned(),
            description: Some("Allow echo-skill".to_owned()),
            active: true,
            effect: PolicyEffect::Allow,
            condition: "targetAgent == 'echo-agent' && skill == 'echo'".to_owned(),
            version: 1,
            created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        });

        assert_eq!(
            evaluate(&config, "sender-1", "echo-agent", "echo"),
            Interception::Forward
        );
    }

    #[test]
    fn would_hold_for_approval_default() {
        let config = PolicyConfig::new_config(); // No rules = deny by default
        assert!(would_hold_for_approval(Some(&config), "sender-1", "echo-agent", "echo"));
    }

    #[test]
    fn would_hold_for_approval_with_allow() {
        let mut config = PolicyConfig::new_config();
        config.rules.push(PolicyRule {
            id: "rule-1".to_owned(),
            description: Some("Allow".to_owned()),
            active: true,
            effect: PolicyEffect::Allow,
            condition: "true".to_owned(),
            version: 1,
            created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        });
        // With an allow rule matching "true", the task is NOT held
        let result = would_hold_for_approval(Some(&config), "sender-1", "echo-agent", "echo");
        assert!(!result); // Allow means forward, not hold
    }
}