#!/usr/bin/env python3
"""
A2A Warden Demo Script: complete A2A communication flow.

This script automates the end-to-end flow of two agents communicating
through A2A Warden as the intermediary:

1. Check that the warden is running.
2. Check that the remote agent is registered.
3. Send a task from the client.
4. Wait for the interception to create a pending approval.
5. Approve the task as a human supervisor.
6. Inspect the recorded timeline and audit trail.

The warden and the remote agent are expected to be running already. The
shell script `examples/scripts/demo-a2a-flow.sh` starts them.

Requirements: the warden on http://127.0.0.1:8080 and the echo agent on
http://127.0.0.1:9001. Only the Python standard library is used.
"""

from __future__ import annotations

import json
import sys
import time
import urllib.error
import urllib.request
import uuid
from typing import Any

WARDEN_URL = "http://127.0.0.1:8080"
AGENT_NAME = "echo-agent"
APPROVER = "demo-user"

A2A_HEADERS = {"A2A-Version": "1.0", "Content-Type": "application/json"}


def http_json(
    url: str,
    payload: dict[str, Any] | None = None,
    headers: dict[str, str] | None = None,
    timeout: float = 30.0,
) -> dict[str, Any]:
    """Sends a JSON request and returns the decoded response."""
    data = json.dumps(payload).encode("utf-8") if payload is not None else None
    request = urllib.request.Request(url, data=data, method="POST" if data else "GET")
    for name, value in {**A2A_HEADERS, **(headers or {})}.items():
        request.add_header(name, value)
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return json.loads(response.read().decode("utf-8"))


def rpc(url: str, method: str, params: dict[str, Any]) -> dict[str, Any]:
    """Sends a JSON-RPC request to the warden."""
    return http_json(
        url,
        {"jsonrpc": "2.0", "id": uuid.uuid4().hex, "method": method, "params": params},
    )


def print_section(title: str) -> None:
    """Prints a titled section header."""
    print("\n" + "=" * 62)
    print(f"  {title}")
    print("=" * 62)


def check_warden(warden_url: str) -> None:
    """Verifies that the warden is reachable."""
    print_section("Check the warden")
    try:
        health = http_json(f"{warden_url}/api/v1/admin/health", timeout=5)
    except urllib.error.URLError as exc:
        print(f"  ERR cannot reach the warden: {exc}")
        print("      start it with: a2a-warden --config examples/config/sqlite.yaml")
        raise SystemExit(1) from exc

    print(f"  OK  the warden is running at {warden_url}")
    print(f"  OK  version {health.get('version')}, backend {health.get('databaseBackend')}")


def check_agents(warden_url: str) -> list[dict[str, Any]]:
    """Checks the agents registered in the warden."""
    print_section("Registered agents")
    agents = http_json(f"{warden_url}/api/v1/admin/agents").get("agents", [])
    for agent in agents:
        print(
            f"  - {agent.get('name'):<16} url={agent.get('url'):<32} "
            f"requires_approval={agent.get('requiresApproval')}"
        )
    if not agents:
        print("  (no agents registered)")
    if not any(agent.get("name") == AGENT_NAME for agent in agents):
        print(f"  ERR the agent '{AGENT_NAME}' is not registered in the warden")
        print("      add it to the `agents:` section of the configuration file")
        raise SystemExit(1)
    return agents


def send_task(warden_url: str, text: str, skill: str = "echo") -> dict[str, Any]:
    """Sends a task from the client to the warden."""
    print_section(f"Send a task: {text}")

    message = {
        "messageId": f"msg-{uuid.uuid4().hex[:12]}",
        "role": "user",
        "parts": [{"type": "text", "text": text}],
        "metadata": {"skill": skill, "senderAgent": "sample-client"},
    }

    print("  method: message/send")
    print(f"  role:   {message['role']}")
    print(f"  skill:  {skill}")

    result = rpc(warden_url, "message/send", {"message": message})
    if "error" in result:
        print("  ERR the warden returned an error")
        print(json.dumps(result, indent=2))
        raise SystemExit(1)

    task = result["result"]
    print(f"  OK  task {task['id']} created in state {task['status']['state']}")
    return task


def wait_for_approval(warden_url: str, timeout: float = 30.0, poll: float = 2.0) -> dict[str, Any]:
    """Waits until a pending approval shows up."""
    print_section("Wait for the pending approval")
    deadline = time.time() + timeout

    while time.time() < deadline:
        approvals = http_json(f"{warden_url}/api/v1/admin/approvals?status=pending").get(
            "approvals", []
        )
        if approvals:
            for approval in approvals:
                print(f"  - approval {approval['id']} for task {approval['taskId']}")
            return approvals[0]
        time.sleep(poll)

    print("  WARN no approval appeared within the timeout")
    raise SystemExit(1)


def decide(warden_url: str, approval_id: str, decision: str, reason: str) -> dict[str, Any]:
    """Approves, rejects, or cancels a pending approval."""
    print_section(f"Resolve the approval: {decision}")

    result = http_json(
        f"{warden_url}/api/v1/admin/approvals/{approval_id}/decision",
        {"decision": decision, "approver": APPROVER, "reason": reason},
    )

    print(f"  OK  approval status: {result.get('approval', {}).get('status')}")
    print(f"  OK  task state: {result.get('task', {}).get('status', {}).get('state')}")
    return result


def show_timeline(warden_url: str, task_id: str) -> None:
    """Prints the recorded timeline of a task."""
    print_section(f"Timeline of the task: {task_id}")

    detail = http_json(f"{warden_url}/api/v1/admin/tasks/{task_id}")
    for message in detail.get("messages", []):
        parts = json.loads(message.get("partsJson") or "[]")
        body = "\n".join(part.get("text", "") for part in parts if part.get("type") == "text")
        if body:
            print(f"  [{message.get('role', 'unknown'):>8}] {body}")

    print("\n  Audit trail:")
    for event in detail.get("audit", []):
        print(
            f"  {event.get('action', ''):<20} by {event.get('actor', ''):<12} "
            f"at {event.get('ts', '')}"
        )


def show_tasks(warden_url: str) -> None:
    """Prints every task recorded in the warden."""
    print_section("All recorded tasks")
    for task in http_json(f"{warden_url}/api/v1/admin/tasks").get("tasks", []):
        print(
            f"  - {task.get('id')}  agent={task.get('agentId')}  "
            f"state={task.get('status')}  context={task.get('contextId')}"
        )


def main() -> None:
    print("\n" + "=" * 62)
    print("  A2A Warden Demo: two agents, one supervised intermediary")
    print("=" * 62)
    print(f"\n  warden:  {WARDEN_URL}")
    print(f"  agent:   {AGENT_NAME}")
    print(f"  console: {WARDEN_URL}/ui\n")

    check_warden(WARDEN_URL)
    check_agents(WARDEN_URL)

    task = send_task(WARDEN_URL, "Hello from the example client")
    approval = wait_for_approval(WARDEN_URL)
    decide(WARDEN_URL, approval["id"], "approve", "Automated demo approval")
    show_timeline(WARDEN_URL, task["id"])
    show_tasks(WARDEN_URL)

    print("\n" + "=" * 62)
    print("  Demo complete")
    print("=" * 62)
    print(f"\n  Open {WARDEN_URL}/ui to inspect the traffic in the web console.")
    print("  See docs/guide-a2a-communication.md for the full walkthrough.")
    print("  See docs/guide-a2a-communication.md for the full walkthrough.\n")


if __name__ == "__main__":
    main()
