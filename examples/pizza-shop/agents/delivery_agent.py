"""Delivery agent.

Skills:
    - assign-delivery: assigns a driver when the address is inside the radius.
    - zone-exception:   assigns a driver for an out-of-radius address.

`zone-exception` is approval-gated by the warden policy: an out-of-radius
order pauses at the warden until a human approves the exception. The agent
itself is unaware of the gate.
"""

from __future__ import annotations

import logging

from a2a.helpers import get_message_text
from a2a.server.agent_execution import AgentExecutor, RequestContext
from a2a.server.events import EventQueue
from a2a.server.tasks import TaskUpdater
from a2a.types import TaskState

from common import domain
from common.server import json_part, parse_json_part, run_server, skill_of, start_task

LOGGER = logging.getLogger("delivery-agent")

_ASSIGNED: list[str] = []  # toy rotation of drivers


class DeliveryExecutor(AgentExecutor):
    """Assigns drivers and handles zone exceptions."""

    async def execute(self, context: RequestContext, event_queue: EventQueue) -> None:
        await start_task(context, event_queue)
        updater = TaskUpdater(event_queue, context.task_id, context.context_id)
        await updater.update_status(TaskState.TASK_STATE_WORKING)

        request = parse_json_part(get_message_text(context.message) or "")
        skill = skill_of(context.message)
        address = request.get("address", "")
        zone = domain.zone_of(address)

        if skill == "assign-delivery":
            if zone == "far":
                # Signal the orchestrator that a zone exception is required;
                # the orchestrator will invoke the gated zone-exception skill.
                result = {"zone": zone, "driver": None, "requiresZoneException": True}
            else:
                driver = self._pick_driver()
                result = {"zone": zone, "driver": driver, "etaMinutes": 25}

        elif skill == "zone-exception":
            driver = self._pick_driver()
            result = {
                "zone": zone,
                "driver": driver,
                "etaMinutes": 45,
                "exceptionApprovedFor": address,
            }

        else:
            raise ValueError(f"unsupported skill: {skill}")

        LOGGER.info("skill=%s result=%s", skill, result)

        await updater.add_artifact([json_part(result)], name="delivery-result")
        await updater.complete()

    def _pick_driver(self) -> str:
        driver = domain.DRIVERS[len(_ASSIGNED) % len(domain.DRIVERS)]
        _ASSIGNED.append(driver)
        return driver

    async def cancel(self, context: RequestContext, event_queue: EventQueue) -> None:
        raise NotImplementedError("cancel is not supported by the delivery agent")


if __name__ == "__main__":
    run_server(
        host="127.0.0.1",
        port=9104,
        name="delivery-agent",
        description="Assigns delivery drivers and handles zone exceptions.",
        version="0.1.0",
        skills=[
            {"id": "assign-delivery", "description": "Assign a driver for an in-radius address.", "tags": ["delivery"]},
            {"id": "zone-exception", "description": "Assign a driver for an out-of-radius address (approval required).", "tags": ["delivery", "approval-gated"]},
        ],
        executor=DeliveryExecutor(),
    )
