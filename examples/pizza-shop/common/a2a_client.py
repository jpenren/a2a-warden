"""Minimal A2A v1.0 JSON-RPC client used by the pizza agents to talk to
each other THROUGH the A2A Warden hub.

Every inter-agent call goes to the warden endpoint, never peer-to-peer, so
the warden can record every message, apply policy, and hold tasks in
input-required for human approval before they reach the target agent.

Wire format targets the v1.0 JSON-RPC binding:
    methods: SendMessage / GetTask / CancelTask, header A2A-Version: 1.0.
State spellings from both v1.0 (TASK_STATE_COMPLETED) and v0.3 (completed)
are tolerated while the ecosystem migrates.
"""

from __future__ import annotations

import asyncio
import json
import os
import uuid
from typing import Any

import httpx

A2A_HEADERS = {"Content-Type": "application/json", "A2A-Version": "1.0"}

#: Default timeout of a call to the warden, in seconds. It has to cover the
#: time a human takes to decide in the console, so it is generous.
DEFAULT_TIMEOUT = 900.0

_COMPLETED = {"TASK_STATE_COMPLETED", "COMPLETED", "completed"}
_FAILED = {"TASK_STATE_FAILED", "FAILED", "failed"}
_CANCELED = {"TASK_STATE_CANCELED", "TASK_STATE_CANCELLED", "CANCELED", "CANCELLED", "canceled", "cancelled"}
_REJECTED = {"TASK_STATE_REJECTED", "REJECTED", "rejected"}
_INPUT_REQUIRED = {"TASK_STATE_INPUT_REQUIRED", "INPUT_REQUIRED", "INPUT-REQUIRED", "input-required"}


class WardenError(RuntimeError):
    """Raised when a task through the warden fails, is cancelled, or times out."""


def text_part(text: str) -> dict[str, Any]:
    """Build a text part in the A2A v1.0 JSON encoding.

    The A2A v1.0 binding tags the parts with their kind (`{"type": "text",
    "text": ...}`); the protobuf based SDKs simply ignore the tag.
    """
    return {"type": "text", "text": text}


def task_id_of(task: dict) -> str:
    """Read the task id across v1.0 (`taskId`) and v0.3 (`id`) spellings."""
    return task.get("taskId") or task.get("id") or ""


def default_timeout() -> float:
    """Read the timeout of the calls to the warden from the environment."""
    raw = os.environ.get("A2AWARDEN_TIMEOUT")
    if not raw:
        return DEFAULT_TIMEOUT
    try:
        return float(raw)
    except ValueError:
        return DEFAULT_TIMEOUT


def normalize_state(task: dict) -> str:
    """Read the task state across the v1.0 and v0.3 spellings."""
    status = task.get("status") or {}
    state = status.get("state") or ""
    if isinstance(state, dict):  # protobuf JSON: {"value": "TASK_STATE_COMPLETED"}
        state = state.get("value", "")
    return str(state)


def first_artifact_payload(task: dict) -> dict:
    """Extract the JSON payload of the first artifact of a completed task."""
    artifacts = task.get("artifacts") or []
    if not artifacts:
        return {}
    for part in artifacts[0].get("parts") or []:
        text = part.get("text")
        if text is not None:
            try:
                payload = json.loads(text)
            except json.JSONDecodeError:
                return {}
            return payload if isinstance(payload, dict) else {}
        if "data" in part:
            return part["data"]
    return {}


def last_message_text(task: dict) -> str:
    """Read the text of the last message of a task, if any."""
    for message in reversed(task.get("history") or []):
        for part in message.get("parts") or []:
            if part.get("text"):
                return part["text"]
    return ""


class WardenClient:
    """JSON-RPC client for the A2A Warden hub.

    The timeout is generous by default: a task may sit in `input-required`
    while a human decides in the console, and the warden holds the request
    open until the whole workflow is over.
    """

    def __init__(self, warden_url: str, timeout: float | None = None) -> None:
        self._base = warden_url.rstrip("/")
        self._timeout = timeout if timeout is not None else default_timeout()

    async def _rpc(self, method: str, params: dict) -> dict:
        async with httpx.AsyncClient(timeout=self._timeout) as http:
            response = await http.post(
                self._base,
                headers=A2A_HEADERS,
                json={
                    "jsonrpc": "2.0",
                    "id": str(uuid.uuid4()),
                    "method": method,
                    "params": params,
                },
            )
        body = response.json()
        if "error" in body:
            raise WardenError(f"{method} failed: {body['error']}")
        return body.get("result") or {}

    async def send_message(
        self,
        *,
        skill: str,
        target_agent: str,
        payload: dict,
        context_id: str | None = None,
    ) -> dict:
        """Send a task addressed to `target_agent` / `skill` through the warden.

        The routing convention for this demo carries the intended skill and
        target agent in the message metadata; the warden resolves the remote
        agent from its registry, records the task, and applies policy before
        forwarding. Passing the same `context_id` for the whole order keeps
        every sub-task grouped in the warden console timeline.
        """
        message: dict[str, Any] = {
            "messageId": str(uuid.uuid4()),
            "role": "user",
            "parts": [text_part(json.dumps(payload))],
            "metadata": {"skill": skill, "targetAgent": target_agent},
        }
        if context_id:
            message["contextId"] = context_id
        result = await self._rpc("SendMessage", {"message": message})
        return result.get("task") or result

    async def get_task(self, task_id: str) -> dict:
        """Read a task from the warden.

        The warden names the parameter `id`; `taskId` is accepted as well so
        the client works against either spelling of the binding.
        """
        try:
            return await self._rpc("GetTask", {"id": task_id})
        except WardenError:
            return await self._rpc("GetTask", {"taskId": task_id})

    async def cancel_task(self, task_id: str) -> dict:
        """Cancel a task through the warden."""
        try:
            return await self._rpc("CancelTask", {"id": task_id})
        except WardenError:
            return await self._rpc("CancelTask", {"taskId": task_id})

    async def wait_for_result(
        self,
        task: dict,
        *,
        poll_seconds: float = 2.0,
        max_wait_seconds: float = 300.0,
    ) -> dict:
        """Poll a task until it completes and return its artifact payload.

        If the warden holds the task for human approval, the state is
        input-required: keep polling while a human decides in the console.
        This is the only place where agents "see" the approval flow.
        """
        waited = 0.0
        notified = False
        while True:
            state = normalize_state(task)
            if state in _COMPLETED:
                return first_artifact_payload(task)
            if state in _FAILED:
                raise WardenError(f"task {task_id_of(task)} failed: {last_message_text(task)}")
            if state in _CANCELED:
                raise WardenError(f"task {task_id_of(task)} was cancelled: {last_message_text(task)}")
            if state in _REJECTED:
                raise WardenError(
                    f"task {task_id_of(task)} was rejected by a supervisor: "
                    f"{last_message_text(task)}"
                )
            if state in _INPUT_REQUIRED and not notified:
                print(
                    f"  [warden] task {task_id_of(task)} is WAITING FOR HUMAN APPROVAL "
                    "in the warden console..."
                )
                notified = True
            task = await self.get_task(task_id_of(task))
            await asyncio.sleep(poll_seconds)
            waited += poll_seconds
            if waited > max_wait_seconds:
                raise WardenError(f"timed out waiting for task {task_id_of(task)}")

    async def call(
        self,
        *,
        skill: str,
        target_agent: str,
        payload: dict,
        context_id: str | None = None,
    ) -> dict:
        """Send a task through the warden and wait for its result."""
        task = await self.send_message(
            skill=skill,
            target_agent=target_agent,
            payload=payload,
            context_id=context_id,
        )
        return await self.wait_for_result(task)
