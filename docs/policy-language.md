# Cedar Policy Language

This document describes the Cedar policy language used by A2A Warden.

## Vision

The policy language defines formal Cedar policy rules that evaluate allow, deny, and escalate-to-approval verdicts based on sender, target agent, skill, tenant, and content entities.

## Policy Model

Each policy rule defines conditions on the following entities:

- **`sender`**: The identity of the sending agent
- **`targetAgent`**: The name of the receiving/remote agent
- **`skill`**: The task skill being invoked
- **`tenant`**: Optional tenant/organization identifier
- **`content`**: Optional content-based conditions (e.g., sensitive data)

## Policy Rule Structure

```yaml
rules:
  - id: "rule-1"
    description: "Require approval for refund-skill tasks"
    active: true
    effect: escalate
    condition: "targetAgent == 'echo-agent' && skill == 'refund-order'"
    version: 1
    created_at: "2024-01-01T00:00:00Z"
```

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `id` | `String` | Unique rule identifier |
| `description` | `Option<String>` | Human-readable description |
| `active` | `bool` | When `true`, the rule is active during evaluation |
| `effect` | `PolicyEffect` | `allow`, `deny`, or `escalate` |
| `condition` | `String` | Cedar-style boolean expression |
| `version` | `u32` | Policy version, for migration and rollback |
| `created_at` | `String` | RFC 3339 timestamp of rule creation |

### Effect Values

- **`allow`**: The action is permitted
- **`deny`**: The action is rejected
- **`escalate`**: The action requires human approval (triggers the approval inbox)

## Deny-by-Default

When no policy rules match the given context, the default verdict is **deny**. This is a security fundamental: no sensitive action is executed without an explicit verdict.

## Policy Evaluation

The policy engine evaluates rules in order and returns the first matching effect:

1. Evaluate each active rule's condition against the provided entities
2. Return the effect of the first matching rule
3. If no rules match, return **deny** (default)

## Simple Checks

### Check by agent
```yaml
- id: "allow-echo-agent"
  effect: allow
  condition: "targetAgent == 'echo-agent'"
```

### Check by skill
```yaml
- id: "deny-refund"
  effect: deny
  condition: "skill == 'refund-order'"
```

### Check by sender and agent
```yaml
- id: "escalate-payments-agent"
  effect: escalate
  condition: "sender == 'payments-agent' && targetAgent == 'delivery-agent'"
```

## Complex Checks by Agent and Skill

### Escalate all tasks from a specific agent to a specific skill
```yaml
- id: "escalate-payments-to-delivery"
  effect: escalate
  condition: "sender == 'payments-agent' && targetAgent == 'delivery-agent' && skill == 'deliver-order'"
```

### Allow only when agent and skill match and tenant is set
```yaml
- id: "allow-tenant-skill"
  effect: allow
  condition: "targetAgent == 'order-agent' && skill == 'create-order' && tenant == 'pizza-shop'"
```

### Deny when agent is untrusted or skill is sensitive
```yaml
- id: "deny-untrusted-refund"
  effect: deny
  condition: "targetAgent == 'untrusted-agent' || skill == 'refund-order'"
```

## Integration with A2A Warden

- Policy decisions are recorded in the audit log alongside task transitions
- The `effect_to_approval_decision()` function maps policy effects to human decisions
- The `approval_decision_to_effect()` function maps human decisions back to policy effects
- Deny-by-default ensures no action executes without explicit policy approval

## Example Configuration

```yaml
policy:
  rules:
    - id: "escalate-all-tasks"
      description: "Escalate all tasks for human review"
      active: true
      effect: escalate
      condition: "true"
      version: 1
      created_at: "{{CURRENT_TIMESTAMP}}"

    - id: "allow-echo-skill"
      description: "Allow echo skill without approval"
      active: true
      effect: allow
      condition: "skill == 'echo'"
      version: 1
      created_at: "{{CURRENT_TIMESTAMP}}"

    - id: "deny-refund"
      description: "Always deny refund orders"
      active: true
      effect: deny
      condition: "skill == 'refund-order'"
      version: 1
      created_at: "{{CURRENT_TIMESTAMP}}"
```
