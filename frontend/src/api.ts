/** Thin client over the warden administration API. */

import type {
  AgentRecord,
  ApprovalRecord,
  AuditRecord,
  Decision,
  SearchResult,
  StreamEvent,
  TaskDetail,
  TaskRecord,
} from "./types";

const BASE = "/api/v1/admin";

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${BASE}${path}`, {
    headers: { "content-type": "application/json" },
    ...init,
  });
  if (!response.ok) {
    let detail = `${response.status} ${response.statusText}`;
    try {
      const body = (await response.json()) as { error?: string };
      if (body.error) {
        detail = body.error;
      }
    } catch {
      // Keep the status line when the body is not JSON.
    }
    throw new Error(detail);
  }
  return (await response.json()) as T;
}

export async function health(): Promise<{ status: string; version: string }> {
  return request<{ status: string; version: string }>("/health");
}

export async function listTasks(params?: {
  status?: string;
  agent?: string;
  limit?: number;
  offset?: number;
}): Promise<{ tasks: TaskRecord[]; total: number }> {
  const query = new URLSearchParams();
  if (params?.status) query.set("status", params.status);
  if (params?.agent) query.set("agent", params.agent);
  if (params?.limit !== undefined) query.set("limit", String(params.limit));
  if (params?.offset !== undefined) query.set("offset", String(params.offset));
  const suffix = query.toString();
  const body = await request<{ tasks: TaskRecord[]; total: number }>(
    `/tasks${suffix ? `?${suffix}` : ""}`,
  );
  return body;
}

export async function getTask(id: string): Promise<TaskDetail> {
  return request<TaskDetail>(`/tasks/${encodeURIComponent(id)}`);
}

export async function listApprovals(
  params?: { status?: string; limit?: number; offset?: number },
): Promise<{ approvals: ApprovalRecord[]; total: number }> {
  const q = new URLSearchParams();
  if (params?.status) q.set("status", params.status);
  if (params?.limit !== undefined) q.set("limit", String(params.limit));
  if (params?.offset !== undefined) q.set("offset", String(params.offset));
  const suffix = q.toString() ? `?${q}` : "";
  const body = await request<{ approvals: ApprovalRecord[]; total: number }>(
    `/approvals${suffix}`,
  );
  return body;
}

export async function decide(
  approvalId: string,
  decision: Decision,
  reason?: string,
  approver?: string,
): Promise<TaskDetail> {
  return request<TaskDetail>(
    `/approvals/${encodeURIComponent(approvalId)}/decision`,
    {
      method: "POST",
      body: JSON.stringify({ decision, reason, approver }),
    },
  );
}

export async function listAgents(): Promise<AgentRecord[]> {
  const body = await request<{ agents: AgentRecord[] }>("/agents");
  return body.agents;
}

/** Phase 5: export the full hash-chained audit trail. */
export async function exportAudit(): Promise<AuditRecord[]> {
  const body = await request<{ audit: AuditRecord[] }>("/audit/export");
  return body.audit;
}

/** Phase 5: verify the integrity of the hash chain. */
export async function verifyAudit(): Promise<{
  valid: boolean;
  entries: number;
  brokenAt?: number;
  lastHash?: string;
}> {
  return request<{
    valid: boolean;
    entries: number;
    brokenAt?: number;
    lastHash?: string;
  }>("/audit/verify");
}

/** Phase 5: global search across tasks, audit entries, and approvals. */
export async function search(q: string): Promise<SearchResult> {
  const query = new URLSearchParams({ q });
  return request<SearchResult>(`/search?${query}`);
}

/** Phase 5: forensic replay of a task's original message. */
export async function replayTask(
  id: string,
  actor?: string,
): Promise<{ task: unknown }> {
  return request<{ task: unknown }>(`/tasks/${encodeURIComponent(id)}/replay`, {
    method: "POST",
    body: JSON.stringify({ actor }),
  });
}

/** Phase 5: selective re-send of a stored message. */
export async function resendMessage(
  id: string,
  actor?: string,
): Promise<{ task: unknown }> {
  return request<{ task: unknown }>(`/messages/${encodeURIComponent(id)}/resend`, {
    method: "POST",
    body: JSON.stringify({ actor }),
  });
}

/** Phase 5: mask personally identifiable information in arbitrary text. */
export async function maskPii(text: string): Promise<{
  original: string;
  masked: string;
}> {
  return request<{ original: string; masked: string }>("/pii/mask", {
    method: "POST",
    body: JSON.stringify({ text }),
  });
}

/**
 * Phase 5: subscribe to the live traffic SSE stream.
 *
 * Returns an unsubscribe function. The listener is called for every event
 * received from the warden's event bus.
 */
export function trafficStream(listener: (event: StreamEvent) => void): () => void {
  const eventSource = new EventSource(`${BASE}/traffic/stream`);
  eventSource.onmessage = (messageEvent) => {
    try {
      listener(JSON.parse(messageEvent.data) as StreamEvent);
    } catch {
      // Malformed frames are ignored: the feed must keep flowing.
    }
  };
  eventSource.onerror = () => {
    eventSource.close();
  };
  return () => {
    eventSource.close();
  };
}
