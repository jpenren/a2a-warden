#!/usr/bin/env python3
"""Sample A2A client driving a task through the A2A Warden with Cedar policies.

The client sends a message to the warden, polls the task with `tasks/get`
until it leaves the input-required state, and prints the final answer.
It supports both Cedar policy-based approval and auto-approve modes.

Usage:

    python3 client-python.py --warden http://127.0.0.1:8080 \
        --text "hello" [--skill echo] [--auto-approve] [--admin-token TOKEN]

Cedar policies mode (default):
    - No --admin-token needed
    - Cedar policies evaluate whether approval is required
    - If policy requires approval, task enters input-required state
    - Use --admin-token to resolve approvals

Auto-approve mode:
    - Use --admin-token with --auto-approve
    - Bypasses Cedar policy evaluation
    - Task is automatically approved
"""

import argparse
import json
import sys
import time
import urllib.request
import urllib.error


def jsonrpc_post(warden_url: str, method: str, params: dict, timeout: int = 30) -> dict:
    """Send a JSON-RPC 2.0 request to the warden and return the parsed result."""
    payload = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    }
    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        warden_url.rstrip("/") + "/message/send",
        data=data,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            body = resp.read().decode("utf-8")
    except urllib.error.HTTPError as exc:
        body = exc.read().decode("utf-8")
        raise RuntimeError(f"HTTP {exc.code}: {body}") from exc

    envelope = json.loads(body)
    if "error" in envelope:
        err = envelope["error"]
        raise RuntimeError(f"JSON-RPC error {err.get('code')}: {err.get('message')}")
    return envelope.get("result", {})


def jsonrpc_get(warden_url: str, method: str, params: dict, timeout: int = 30) -> dict:
    """Send a JSON-RPC 2.0 POST (tasks/get uses POST in this binding)."""
    payload = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    }
    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        warden_url.rstrip("/") + "/" + method.replace(".", "/"),
        data=data,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            body = resp.read().decode("utf-8")
    except urllib.error.HTTPError as exc:
        body = exc.read().decode("utf-8")
        raise RuntimeError(f"HTTP {exc.code}: {body}") from exc

    envelope = json.loads(body)
    if "error" in envelope:
        err = envelope["error"]
        raise RuntimeError(f"JSON-RPC error {err.get('code')}: {err.get('message')}")
    return envelope.get("result", {})


def admin_post(warden_url: str, path: str, body: dict, admin_token: str | None,
               timeout: int = 30) -> dict:
    """Send a POST to the administration API."""
    data = json.dumps(body).encode("utf-8")
    url = warden_url.rstrip("/") + path
    req = urllib.request.Request(
        url,
        data=data,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    if admin_token:
        req.add_header("X-Admin-Token", admin_token)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            raw = resp.read().decode("utf-8")
    except urllib.error.HTTPError as exc:
        raw = exc.read().decode("utf-8")
        raise RuntimeError(f"HTTP {exc.code}: {raw}") from exc
    return json.loads(raw) if raw.strip() else {}


def poll_task(warden_url: str, task_id: str, admin_token: str | None,
              auto_approve: bool, timeout_secs: int, poll_interval: float = 2.0) -> None:
    """Poll tasks/get until the task reaches a terminal state."""
    deadline = time.time() + timeout_secs
    last_state = None

    while time.time() < deadline:
        result = jsonrpc_get(warden_url, "tasks/get", {"id": task_id})
        state = result.get("status", {}).get("state", "unknown")

        if state != last_state:
            print(f"  task {task_id}: state -> {state}")
            last_state = state

        if state in ("completed", "failed", "canceled", "rejected"):
            msg = result.get("status", {}).get("message")
            print(f"msg: {msg}")
            if msg and msg.get("parts"):
                text = "".join(p.get("text", "") for p in msg["parts"] if p.get("kind") == "text")
                print(f"\nFinal response: '{text}'")
            return

        if state == "input-required":
            if auto_approve and admin_token:
                # Find the pending approval and approve it
                approvals = admin_post(
                    warden_url,
                    "/api/v1/admin/approvals",
                    {"status": "pending"},
                    admin_token,
                )
                pending = [a for a in approvals.get("approvals", [])
                           if a.get("taskId") == task_id]
                if pending:
                    approval_id = pending[0]["id"]
                    print(f"  auto-approving approval {approval_id}...")
                    admin_post(
                        warden_url,
                        f"/api/v1/admin/approvals/{approval_id}/decision",
                        {"decision": "approve", "approver": "client-python",
                         "reason": "auto-approve from client"},
                        admin_token,
                    )

        time.sleep(poll_interval)

    print(f"\nTimeout after {timeout_secs}s: task {task_id} still in state {last_state}")


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Sample A2A client driving a task through A2A Warden"
    )
    parser.add_argument("--warden", required=True, help="Warden base URL (e.g. http://127.0.0.1:8080)")
    parser.add_argument("--text", required=True, help="Message text to send")
    parser.add_argument("--skill", default="echo", help="Skill name (default: echo)")
    parser.add_argument("--auto-approve", action="store_true",
                        help="Automatically approve pending approvals (requires --admin-token)")
    parser.add_argument("--admin-token", default=None, help="Admin token for the warden API")
    parser.add_argument("--timeout", type=int, default=60,
                        help="Maximum seconds to wait for task completion (default: 60)")
    args = parser.parse_args()

    message = {
        "messageId": "client-msg-" + str(int(time.time() * 1000)),
        "role": "user",
        "parts": [{"type": "text", "text": args.text}],
        "metadata": {
            "skill": args.skill,
            "senderAgent": "client-python",
            "targetAgent": "echo-agent",
        },
    }

    print(f"Sending task to {args.warden} (skill={args.skill}, text='{args.text}')")
    try:
        task = jsonrpc_post(args.warden, "message/send", {"message": message}, timeout=30)
    except RuntimeError as exc:
        print(f"Failed to send message: {exc}", file=sys.stderr)
        sys.exit(1)

    task_id = task.get("id")
    state = task.get("status", {}).get("state", "unknown")
    print(f"Task {task_id} created in state {state}")

    if state == "input-required":
        print("Task requires human approval before forwarding.")
    else:
        print(f"Task {task_id} forwarded immediately.")

    poll_task(args.warden, task_id, args.admin_token, args.auto_approve, args.timeout)


if __name__ == "__main__":
    main()
