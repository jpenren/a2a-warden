"""Inventory agent.

Skills:
    - check-ingredients: reports whether the kitchen can prepare the order.

This agent knows nothing about approvals: if the warden policy gates a
skill, the task is held at the warden before it ever reaches this code.
"""

from __future__ import annotations

import json
import logging

from a2a.helpers import get_message_text
from a2a.server.agent_execution import AgentExecutor, RequestContext
from a2a.server.events import EventQueue
from a2a.server.tasks import TaskUpdater
from a2a.types import TaskState

from common import domain
from common.server import json_part, parse_json_part, run_server, start_task

LOGGER = logging.getLogger("inventory-agent")


class InventoryExecutor(AgentExecutor):
    """Answers whether the kitchen has the ingredients of an order."""

    async def execute(self, context: RequestContext, event_queue: EventQueue) -> None:
        await start_task(context, event_queue)
        updater = TaskUpdater(event_queue, context.task_id, context.context_id)
        await updater.update_status(TaskState.TASK_STATE_WORKING)

        request = parse_json_part(get_message_text(context.message) or "")
        items = request.get("items", [])
        result = domain.check_availability(items)
        LOGGER.info("availability checked: %s", result)

        await updater.add_artifact([json_part(result)], name="availability")
        await updater.complete()

    async def cancel(self, context: RequestContext, event_queue: EventQueue) -> None:
        raise NotImplementedError("cancel is not supported by the inventory agent")


if __name__ == "__main__":
    run_server(
        host="127.0.0.1",
        port=9102,
        name="inventory-agent",
        description="Checks ingredient availability for pizza orders.",
        version="0.1.0",
        skills=[
            {
                "id": "check-ingredients",
                "description": "Returns needed ingredients, missing ingredients, and availability.",
                "tags": ["inventory"],
            }
        ],
        executor=InventoryExecutor(),
    )
