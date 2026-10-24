"""Helpers to expose an agent as an A2A server using the official SDK.

The demo targets the `a2a-sdk` v1.x line. The A2A v1.0 protocol renamed a
few things compared to v0.3: the Agent Card advertises
`supported_interfaces` instead of a single `url`, and `Part` is a flat
message with a `text` field instead of a tagged union. The helpers below
isolate those details so the agent files stay readable.

The server speaks the A2A v1.0 JSON-RPC binding (`SendMessage`, `GetTask`,
`CancelTask`), which is what the A2A Warden calls.
"""

from __future__ import annotations

import json
import logging
from typing import Any

import uvicorn
from a2a.server.agent_execution import AgentExecutor, RequestContext
from a2a.server.request_handlers import DefaultRequestHandler
from a2a.server.routes import create_agent_card_routes, create_jsonrpc_routes
from a2a.server.tasks import InMemoryTaskStore
from a2a.types import (
    AgentCapabilities,
    AgentCard,
    AgentInterface,
    AgentSkill,
    Part,
    Task,
    TaskState,
    TaskStatus,
)
from starlette.applications import Starlette
from starlette.middleware import Middleware
from starlette.requests import Request
from starlette.responses import Response
from starlette.types import ASGIApp, Receive, Scope, Send

LOGGER = logging.getLogger("pizza-agents")

#: Protocol binding advertised by the demo agents.
PROTOCOL_BINDING = "JSONRPC"

#: A2A protocol version implemented by the demo agents.
PROTOCOL_VERSION = "1.0"

#: Media types used by the demo (every payload travels as JSON in a text part).
INPUT_MODES = ["text/plain", "application/json"]
OUTPUT_MODES = ["text/plain", "application/json"]

#: Roles of the A2A v1.0 JSON encoding, and their protobuf enum spelling.
_ROLE_ALIASES = {
    "user": "ROLE_USER",
    "agent": "ROLE_AGENT",
}


def text_part(text: str) -> Part:
    """Build a text Part."""
    return Part(text=text)


def json_part(payload: dict[str, Any]) -> Part:
    """Serialize a JSON payload as a text part (the demo's message convention).

    The demo keeps the payloads in text parts so every hop is readable in the
    warden console without a schema negotiation.
    """
    return text_part(json.dumps(payload))


def parse_json_part(text: str) -> dict[str, Any]:
    """Parse a JSON payload carried by a text part."""
    if not text:
        return {}
    try:
        payload = json.loads(text)
    except json.JSONDecodeError:
        return {}
    return payload if isinstance(payload, dict) else {}


def skill_of(message: Any) -> str:
    """Read the skill of a request from its metadata.

    The warden forwards the original message, metadata included, so the
    agents know which of their skills is being invoked.
    """
    if message is None:
        return ""
    metadata = getattr(message, "metadata", None)
    if not metadata:
        return ""
    try:
        return str(metadata["skill"])
    except (KeyError, ValueError, TypeError):
        return ""


async def publish_message(
    updater: Any,
    event_queue: EventQueue,
    payload: dict[str, Any],
) -> None:
    """Publish a progress note in the timeline of the running task.

    The note travels as the message of a status update: once the task exists,
    the server only accepts status and artifact events.
    """
    await updater.update_status(
        TaskState.TASK_STATE_WORKING,
        message=updater.new_agent_message([json_part(payload)]),
    )


async def start_task(
    context: RequestContext,
    event_queue: EventQueue,
) -> None:
    """Create the task in the store before publishing any status update.

    The server rejects a status update that arrives before the task itself
    exists, so the executor has to publish the task first.
    """
    task = Task(
        id=context.task_id or "",
        context_id=context.context_id or "",
        status=TaskStatus(state=TaskState.TASK_STATE_WORKING),
    )
    await event_queue.enqueue_event(task)


def build_agent_card(
    *,
    host: str,
    port: int,
    name: str,
    description: str,
    version: str,
    skills: list[dict[str, Any]],
) -> AgentCard:
    """Builds the Agent Card of a demo agent.

    `skills` is a plain list of dicts so each agent file stays free of SDK
    type import details; they are mapped to AgentSkill here.
    """
    agent_skills = [
        AgentSkill(
            id=skill["id"],
            name=skill["id"],
            description=skill["description"],
            tags=list(skill.get("tags", [])),
        )
        for skill in skills
    ]

    return AgentCard(
        name=name,
        description=description,
        version=version,
        capabilities=AgentCapabilities(streaming=False, push_notifications=False),
        skills=agent_skills,
        supported_interfaces=[
            AgentInterface(
                url=f"http://{host}:{port}/",
                protocol_binding=PROTOCOL_BINDING,
                protocol_version=PROTOCOL_VERSION,
            )
        ],
        default_input_modes=list(INPUT_MODES),
        default_output_modes=list(OUTPUT_MODES),
    )


def build_app(
    *,
    host: str,
    port: int,
    name: str,
    description: str,
    version: str,
    skills: list[dict[str, Any]],
    executor: AgentExecutor,
) -> Starlette:
    """Builds the Starlette application exposing an agent."""
    agent_card = build_agent_card(
        host=host,
        port=port,
        name=name,
        description=description,
        version=version,
        skills=skills,
    )

    handler = DefaultRequestHandler(
        agent_executor=executor,
        task_store=InMemoryTaskStore(),
        agent_card=agent_card,
    )

    routes = [
        *create_jsonrpc_routes(handler, rpc_url="/"),
        *create_agent_card_routes(agent_card),
    ]
    return Starlette(routes=routes, middleware=[Middleware(V1EncodingMiddleware)])


class V1EncodingMiddleware:
    """Adapts the A2A v1.0 JSON encoding to the protobuf encoding of the SDK.

    The A2A v1.0 JSON binding spells the message role in lower case
    (`"user"`), while the SDK is built on protobuf and expects the enum name
    (`"ROLE_USER"`). The middleware rewrites the role of the incoming
    messages so a spec compliant peer, such as the A2A Warden, can talk to
    these agents without the agents knowing about the difference.
    """

    def __init__(self, app: ASGIApp) -> None:
        self.app = app

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] != "http" or scope["method"] != "POST":
            await self.app(scope, receive, send)
            return

        body = await receive()
        payload = body.get("body", b"")
        rewritten = _rewrite_roles(payload)
        if rewritten is None:
            await self.app(scope, receive, send)
            return

        sent = False

        async def replay() -> dict[str, Any]:
            nonlocal sent
            if sent:
                return {"type": "http.disconnect"}
            sent = True
            return {
                "type": "http.request",
                "body": rewritten,
                "more_body": False,
            }

        await self.app(scope, replay, send)


def _rewrite_roles(payload: bytes) -> bytes | None:
    """Rewrites the role of every message of a JSON-RPC request."""
    try:
        request = json.loads(payload)
    except (json.JSONDecodeError, UnicodeDecodeError):
        return None
    if not isinstance(request, dict):
        return None

    changed = False
    message = (request.get("params") or {}).get("message")
    if isinstance(message, dict):
        role = message.get("role")
        alias = _ROLE_ALIASES.get(str(role).lower())
        if alias and alias != role:
            message["role"] = alias
            changed = True

    if not changed:
        return None
    return json.dumps(request).encode("utf-8")


def run_server(
    *,
    host: str,
    port: int,
    name: str,
    description: str,
    version: str,
    skills: list[dict[str, Any]],
    executor: AgentExecutor,
) -> None:
    """Builds the app for an executor and serves it."""
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s %(name)s %(message)s",
    )
    app = build_app(
        host=host,
        port=port,
        name=name,
        description=description,
        version=version,
        skills=skills,
        executor=executor,
    )
    LOGGER.info("%s listening on http://%s:%s", name, host, port)
    uvicorn.run(app, host=host, port=port, log_level="info")
