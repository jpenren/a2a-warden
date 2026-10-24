#!/usr/bin/env python3
"""Echo agent speaking the A2A JSON-RPC binding.

This is the remote agent used by the A2A Warden legacy demo. It implements the
subset of the A2A v1.0 interface that the warden needs: `message/send`,
`tasks/get`, and `tasks/cancel`, plus the Agent Card publication endpoint.

It deliberately has no third party dependency so the demo runs anywhere with a
Python 3.10+ interpreter.
"""

from __future__ import annotations

import argparse
import json
import logging
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

LOGGER = logging.getLogger("echo-agent")

A2A_VERSION = "1.0"
JSONRPC_VERSION = "2.0"

# In-memory task store: task id -> task.
TASKS: dict[str, dict[str, Any]] = {}


def now() -> str:
    """Returns the current UTC timestamp in RFC 3339."""
    from datetime import datetime, timezone

    return datetime.now(timezone.utc).isoformat(timespec="milliseconds").replace(
        "+00:00", "Z"
    )


def new_id() -> str:
    """Returns a new task identifier."""
    return str(uuid.uuid4())


def text_of(message: dict[str, Any]) -> str:
    """Extracts the plain text of a message."""
    parts = message.get("parts") or []
    return "\n".join(part.get("text", "") for part in parts if part.get("type") == "text")


def make_task(text: str, context_id: str | None) -> dict[str, Any]:
    """Builds a completed task echoing the received text."""
    task_id = new_id()
    context = context_id or new_id()
    reply = f"echo: {text}"
    task = {
        "id": task_id,
        "contextId": context,
        "status": {
            "state": "completed",
            "message": {
                "messageId": new_id(),
                "role": "agent",
                "parts": [{"type": "text", "text": reply}],
                "contextId": context,
                "taskId": task_id,
            },
            "timestamp": now(),
        },
    }
    TASKS[task_id] = task
    LOGGER.info("task %s completed: %s", task_id, reply)
    return task


def handle_rpc(request: dict[str, Any]) -> dict[str, Any]:
    """Handles one JSON-RPC request and returns the JSON-RPC response."""
    request_id = request.get("id")
    method = request.get("method", "")
    params = request.get("params") or {}

    if method in ("message/send", "SendMessage"):
        message = params.get("message") or {}
        task = make_task(text_of(message), message.get("contextId"))
        return {"jsonrpc": JSONRPC_VERSION, "id": request_id, "result": task}

    if method in ("tasks/get", "GetTask"):
        task = TASKS.get(params.get("id", ""))
        if task is None:
            return {
                "jsonrpc": JSONRPC_VERSION,
                "id": request_id,
                "error": {"code": -32602, "message": "task not found"},
            }
        return {"jsonrpc": JSONRPC_VERSION, "id": request_id, "result": task}

    if method in ("tasks/cancel", "CancelTask"):
        task = TASKS.get(params.get("id", ""))
        if task is None:
            return {
                "jsonrpc": JSONRPC_VERSION,
                "id": request_id,
                "error": {"code": -32602, "message": "task not found"},
            }
        task["status"]["state"] = "canceled"
        task["status"]["timestamp"] = now()
        return {"jsonrpc": JSONRPC_VERSION, "id": request_id, "result": task}

    return {
        "jsonrpc": JSONRPC_VERSION,
        "id": request_id,
        "error": {"code": -32601, "message": f"method not found: {method}"},
    }


AGENT_CARD = {
    "name": "echo-agent",
    "description": "Echoes back the text it receives. Used by the A2A Warden demo.",
    "url": "http://127.0.0.1:9001",
    "version": "0.1.0",
    "protocolVersion": A2A_VERSION,
    "capabilities": {
        "streaming": False,
        "pushNotifications": False,
        "stateTransitionHistory": False,
    },
    "skills": [
        {
            "id": "echo",
            "name": "Echo",
            "description": "Returns the received text unchanged, prefixed with 'echo:'.",
            "tags": ["demo", "echo"],
            "examples": ["Send 'hello' to get 'echo: hello'"],
        }
    ],
    "defaultInputModes": ["text/plain"],
    "defaultOutputModes": ["text/plain"],
}


class Handler(BaseHTTPRequestHandler):
    """HTTP handler of the echo agent."""

    server_version = "echo-agent/0.1.0"

    def _send_json(self, payload: Any, status: int = 200) -> None:
        """Writes a JSON response."""
        body = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("A2A-Version", A2A_VERSION)
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:  # noqa: N802 - required by BaseHTTPRequestHandler
        """Serves the Agent Card."""
        if self.path in ("/.well-known/agent-card.json", "/.well-known/agent.json"):
            self._send_json(AGENT_CARD)
            return
        self._send_json({"error": "not found"}, status=404)

    def do_POST(self) -> None:  # noqa: N802 - required by BaseHTTPRequestHandler
        """Handles a JSON-RPC request."""
        length = int(self.headers.get("Content-Length", "0"))
        raw = self.rfile.read(length) if length else b"{}"
        try:
            request = json.loads(raw.decode("utf-8"))
        except json.JSONDecodeError as error:
            self._send_json(
                {
                    "jsonrpc": JSONRPC_VERSION,
                    "id": None,
                    "error": {"code": -32700, "message": f"parse error: {error}"},
                },
                status=400,
            )
            return
        LOGGER.info("received %s", request.get("method"))
        self._send_json(handle_rpc(request))

    def log_message(self, fmt: str, *args: Any) -> None:
        """Routes the HTTP access log to the logging module."""
        LOGGER.debug(fmt, *args)


def main() -> None:
    """Starts the echo agent."""
    parser = argparse.ArgumentParser(description="A2A echo agent")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=9001)
    args = parser.parse_args()

    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
    AGENT_CARD["url"] = f"http://{args.host}:{args.port}"

    server = ThreadingHTTPServer((args.host, args.port), Handler)
    LOGGER.info("echo-agent listening on http://%s:%s", args.host, args.port)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        LOGGER.info("shutting down")
        server.server_close()


if __name__ == "__main__":
    main()
