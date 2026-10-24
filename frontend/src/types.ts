/** Types mirroring the administration API payloads. */

export interface TaskRecord {
  id: string;
  contextId: string;
  agentId: string;
  status: string;
  createdAt: string;
  updatedAt: string;
  metadataJson?: string;
}

export interface MessageRecord {
  id: string;
  taskId: string;
  contextId: string;
  role: "user" | "agent";
  partsJson: string;
  createdAt: string;
}

export interface ApprovalRecord {
  id: string;
  taskId: string;
  status: "pending" | "approved" | "rejected" | "cancelled";
  decidedBy?: string;
  decidedAt?: string;
  expiresAt?: string;
  reason?: string;
  createdAt: string;
}

export interface AuditRecord {
  id: string;
  ts: string;
  actor: string;
  action: string;
  taskId?: string;
  contextId?: string;
  payloadJson?: string;
  previousHash?: string;
  hash?: string;
}

export interface ArtifactRecord {
  id: string;
  artifactId: string;
  taskId: string;
  name?: string;
  partsJson: string;
  metadataJson?: string;
  createdAt: string;
}

export interface AgentRecord {
  id: string;
  name: string;
  url: string;
  skillsJson: string;
  requiresApproval: boolean;
  requireApprovalSkills?: string[];
  cardJson?: string;
  createdAt: string;
}

export interface TaskDetail {
  task: TaskRecord;
  messages: MessageRecord[];
  artifacts: ArtifactRecord[];
  approval?: ApprovalRecord;
  audit: AuditRecord[];
}

export type Decision = "approve" | "reject" | "cancel";

/** Phase 5: live traffic event coming over the SSE stream. */
export interface StreamEventTask {
  Task: TaskRecord;
}

export interface StreamEventMessage {
  Message: MessageRecord;
}

export interface StreamEventStatusUpdate {
  StatusUpdate: {
    taskId: string;
    contextId: string;
    status: string;
    finalEvent?: boolean;
  };
}

export interface StreamEventArtifactUpdate {
  ArtifactUpdate: {
    taskId: string;
    contextId: string;
    artifactId: string;
    append?: boolean;
    lastChunk?: boolean;
  };
}

export interface StreamEventError {
  Error: { code: number; message: string };
}

export type StreamEvent =
  | StreamEventTask
  | StreamEventMessage
  | StreamEventStatusUpdate
  | StreamEventArtifactUpdate
  | StreamEventError;

/** Phase 5: global search result. */
export interface SearchResult {
  query: string;
  tasks: TaskRecord[];
  audit: AuditRecord[];
  approvals: ApprovalRecord[];
}

/** Phase 4: Cedar policy rule (informational, shown in the console). */
export interface PolicyRule {
  id: string;
  description?: string;
  active: boolean;
  effect: "allow" | "deny" | "escalate";
  condition: string;
  version: number;
  createdAt: string;
}

/** Phase 4: policy evaluation verdict attached to a task. */
export interface PolicyVerdict {
  ruleId: string;
  effect: "allow" | "deny" | "escalate";
  condition: string;
  recordedAt: string;
}
