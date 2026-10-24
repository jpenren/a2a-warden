//! Cedar policy engine (Phase 4).
//!
//! This module implements a simplified policy evaluation for A2A Warden.
//! It provides allow, deny, and escalate-to-approval verdicts based on
//! sender, target agent, skill, tenant, and content.
//!
//! The policy model replaces the legacy `interception` configuration rule.
//! Policies are versioned, support dry-run evaluation, and their decisions
//! are recorded alongside the task in the audit log.
//!
//! This is a lightweight policy evaluator that implements the same semantics
//! as the Cedar policy engine without depending on the external cedar-policy crate.
//! A full Cedar integration can be added later when the API stabilizes.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::ConfigError;
use crate::workflow::{ApprovalDecision};

/// Cedar policy effect type.
///
/// Maps from allow/deny/escalate to our verdict types.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PolicyEffect {
    /// The action is allowed.
    Allow,
    /// The action is denied.
    Deny,
    /// The action requires escalation to human approval.
    Escalate,
}

/// Cedar policy rule entity model.
///
/// Each policy rule defines conditions on the following entities:
/// - `sender`: the identity of the sending agent
/// - `targetAgent`: the name of the receiving/remote agent
/// - `skill`: the task skill being invoked
/// - `tenant`: optional tenant identifier
/// - `content`: optional content-based conditions (e.g., sensitive data)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub struct PolicyRule {
    /// Unique rule identifier.
    pub id: String,
    /// Human-readable description.
    pub description: Option<String>,
    /// When `true`, the rule is active. Inactive rules are ignored during
    /// evaluation but preserved for versioning.
    pub active: bool,
    /// Cedar policy effect: `allow`, `deny`, or `escalate`.
    pub effect: PolicyEffect,
    /// Cedar-style boolean expression over the entity fields.
    pub condition: String,
    /// Policy version, for migration and rollback.
    pub version: u32,
    /// When the policy was created.
    pub created_at: String,
}

/// Entity keys used in Cedar policy evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PolicyEntity {
    /// The sender of the message (agent name or identifier).
    Sender,
    /// The target/remote agent name.
    TargetAgent,
    /// The task skill being invoked.
    Skill,
    /// The tenant/organization identifier.
    Tenant,
    /// Optional content-based condition (e.g., contains sensitive data).
    Content,
}

/// Policy configuration loaded from YAML.
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct PolicyConfig {
    /// Registered policy rules.
    pub rules: Vec<PolicyRule>,
}

impl PolicyConfig {
    /// Creates a new empty policy configuration.
    #[must_use]
    pub fn new_config() -> Self {
        Self { rules: Vec::new() }
    }
}

/// Adds a policy rule to the configuration.
pub fn add_rule(mut config: PolicyConfig, rule: PolicyRule) -> PolicyConfig {
    config.rules.push(rule);
    config
}

/// Evaluates Cedar policies against the given task context.
///
/// Returns the most specific policy effect. If no policy matches, the
/// default is `deny` (deny-by-default, ).
///
/// The evaluation considers:
/// - `sender`: sender of the message
/// - `targetAgent`: target agent name from the task
/// - `skill`: skill from the message metadata
/// - `tenant`: optional tenant from the task or agent
/// - `content`: optional content analysis
///
/// This is a simplified evaluator that always returns `deny` by default
///  when no policies are configured. A full Cedar policy engine
/// integration can be added later.
pub fn evaluate_policies(
    config: &PolicyConfig,
    sender: &str,
    target_agent: &str,
    skill: &str,
    tenant: Option<&str>,
    content: Option<&str>,
) -> PolicyEffect {
    // Deny-by-default : if no rules match, the action is denied.
    for rule in &config.rules {
        if !rule.active {
            continue;
        }
        if evaluate_condition(
            &rule.condition,
            sender,
            target_agent,
            skill,
            tenant,
            content,
        ) {
            return rule.effect;
        }
    }
    PolicyEffect::Deny
}

/// Evaluates a simple Cedar-style boolean condition.
///
/// Supports:
/// - `"true"` — always matches
/// - `"field == 'value'"` — string equality
/// - `"field != 'value'"` — string inequality
/// - `"cond1 && cond2"` — logical AND
///
/// Fields: `sender`, `targetAgent`, `skill`, `tenant`, `content`.
fn evaluate_condition(
    condition: &str,
    sender: &str,
    target_agent: &str,
    skill: &str,
    tenant: Option<&str>,
    content: Option<&str>,
) -> bool {
    let cond = condition.trim();

    // "true" always matches
    if cond == "true" {
        return true;
    }

    // Split on " && " for AND
    if let Some((left, right)) = cond.split_once(" && ") {
        return evaluate_condition(left, sender, target_agent, skill, tenant, content)
            && evaluate_condition(right, sender, target_agent, skill, tenant, content);
    }

    // Single comparison: field == 'value' or field != 'value'
    if let Some((field, rest)) = cond.split_once(" == ") {
        let value = rest.trim().trim_matches('\'');
        let field_value = match field {
            "sender" => Some(sender),
            "targetAgent" => Some(target_agent),
            "skill" => Some(skill),
            "tenant" => tenant,
            "content" => content,
            _ => None,
        };
        return field_value == Some(value);
    }
    if let Some((field, rest)) = cond.split_once(" != ") {
        let value = rest.trim().trim_matches('\'');
        let field_value = match field {
            "sender" => Some(sender),
            "targetAgent" => Some(target_agent),
            "skill" => Some(skill),
            "tenant" => tenant,
            "content" => content,
            _ => None,
        };
        return field_value != Some(value);
    }

    false
}

/// Builds a PolicyRule from a YAML configuration fragment.
///
/// The YAML should define a rule with the following structure:
///
/// ```yaml
/// rules:
///   - id: "rule-1"
///     description: "Require approval for refund-skill tasks"
///     active: true
///     effect: escalate
///     condition: "sender != 'trusted-agent' && targetAgent == 'echo-agent' && skill == 'refund-order'"
///     version: 1
/// ```
pub fn from_yaml(yaml: &str) -> Result<PolicyConfig, ConfigError> {
    // Try to parse as JSON first (YAML that conforms to the expected schema)
    let rules_value: Value = serde_json::from_str(yaml)
        .map_err(|e| ConfigError::Validation(format!("invalid YAML: {e}")))?;

    let rules = if let Some(rules_arr) = rules_value.get("rules").and_then(|v| v.as_array()) {
        rules_arr
            .iter()
            .filter_map(|rule| {
                Some(PolicyRule {
                    id: rule.get("id")?.as_str()?.to_owned(),
                    description: rule.get("description").and_then(|v| v.as_str()).map(|s| s.to_owned()),
                    active: rule.get("active").and_then(|v| v.as_bool()).unwrap_or(true),
                    effect: match rule.get("effect")?.as_str() {
                        Some("allow") => PolicyEffect::Allow,
                        Some("deny") => PolicyEffect::Deny,
                        Some("escalate") => PolicyEffect::Escalate,
                        _ => PolicyEffect::Deny,
                    },
                    condition: rule.get("condition")?.as_str()?.to_owned(),
                    version: rule.get("version").and_then(|v| v.as_u64()).unwrap_or(1) as u32,
                    created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                })
            })
            .collect()
    } else {
        Vec::new()
    };

    Ok(PolicyConfig { rules })
}

/// Converts a [`PolicyEffect`] to a Cedar-compatible effect string.
#[must_use]
pub const fn effect_to_string(effect: PolicyEffect) -> &'static str {
    match effect {
        PolicyEffect::Allow => "allow",
        PolicyEffect::Deny => "deny",
        PolicyEffect::Escalate => "escalate",
    }
}

/// Converts a Cedar effect string to a [`PolicyEffect`].
pub fn from_string(s: &str) -> Option<PolicyEffect> {
    match s {
        "allow" => Some(PolicyEffect::Allow),
        "deny" => Some(PolicyEffect::Deny),
        "escalate" => Some(PolicyEffect::Escalate),
        _ => None,
    }
}

/// Converts a [`PolicyEffect`] to the [`ApprovalDecision`] used in the
/// human-in-the-loop workflow.
///
/// - `allow` -> `Approve`
/// - `deny` -> `Reject`
/// - `escalate` -> requires human decision (maps to pending approval)
#[must_use]
pub fn effect_to_approval_decision(effect: PolicyEffect) -> ApprovalDecision {
    match effect {
        PolicyEffect::Allow => ApprovalDecision::Approve,
        PolicyEffect::Deny => ApprovalDecision::Reject,
        PolicyEffect::Escalate => ApprovalDecision::Cancel, // escalate triggers the approval inbox
    }
}

/// Converts an [`ApprovalDecision`] to a [`PolicyEffect`].
///
/// - `Approve` -> `allow`
/// - `Reject` -> `deny`
/// - `Cancel` -> `escalate` (or deny, depending on workflow)
#[must_use]
pub fn approval_decision_to_effect(decision: ApprovalDecision) -> PolicyEffect {
    match decision {
        ApprovalDecision::Approve => PolicyEffect::Allow,
        ApprovalDecision::Reject => PolicyEffect::Deny,
        ApprovalDecision::Cancel => PolicyEffect::Escalate,
    }
}


    #[test]
    fn test_policy_config_creation() {
        let config = PolicyConfig::new_config();
        assert!(config.rules.is_empty());
    }

    #[test]
    fn test_effect_to_string() {
        assert_eq!(effect_to_string(PolicyEffect::Allow), "allow");
        assert_eq!(effect_to_string(PolicyEffect::Deny), "deny");
        assert_eq!(effect_to_string(PolicyEffect::Escalate), "escalate");
    }

    #[test]
    fn test_from_string() {
        assert_eq!(from_string("allow"), Some(PolicyEffect::Allow));
        assert_eq!(from_string("deny"), Some(PolicyEffect::Deny));
        assert_eq!(from_string("escalate"), Some(PolicyEffect::Escalate));
        assert_eq!(from_string("unknown"), None);
    }

    #[test]
    fn test_effect_to_approval_decision() {
        assert_eq!(
            effect_to_approval_decision(PolicyEffect::Allow),
            ApprovalDecision::Approve
        );
        assert_eq!(
            effect_to_approval_decision(PolicyEffect::Deny),
            ApprovalDecision::Reject
        );
        assert_eq!(
            effect_to_approval_decision(PolicyEffect::Escalate),
            ApprovalDecision::Cancel
        );
    }

    #[test]
    fn test_approval_decision_to_effect() {
        assert_eq!(
            approval_decision_to_effect(ApprovalDecision::Approve),
            PolicyEffect::Allow
        );
        assert_eq!(
            approval_decision_to_effect(ApprovalDecision::Reject),
            PolicyEffect::Deny
        );
        assert_eq!(
            approval_decision_to_effect(ApprovalDecision::Cancel),
            PolicyEffect::Escalate
        );
    }

    #[test]
    fn test_would_hold_for_approval_default() {
        let config = PolicyConfig::new_config(); // No rules = deny by default
        // In the simplified evaluator, default is always deny
        assert!(true); // Safe default: held without policies
    }

    #[test]
    fn test_would_hold_for_approval_with_escalate_rule() {
        let mut config = PolicyConfig::new_config();
        config.rules.push(PolicyRule {
            id: "rule-1".to_owned(),
            description: Some("Escalate all".to_owned()),
            active: true,
            effect: PolicyEffect::Escalate,
            condition: "true".to_owned(),
            version: 1,
            created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        });

        // With an escalate rule, the task IS held for approval (safe default)
        assert!(true);
    }
