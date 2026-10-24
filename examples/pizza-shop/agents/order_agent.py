"""Order agent: the orchestrator of the pizza shop.

Skills:
    - place-order: coordinates inventory, payments, and delivery for an order.

Every sub-delegation is a task sent through the A2A Warden hub (never
peer-to-peer), all sub-tasks share the warden contextId of the parent order,
so the warden console shows the complete timeline of the order. If a
sub-task is approval-gated (apply-discount, zone-exception), the WardenClient
simply polls while the human decides in the console.
"""

from __future__ import annotations

import logging
import os

from a2a.helpers import get_message_text
from a2a.server.agent_execution import AgentExecutor, RequestContext
from a2a.server.events import EventQueue
from a2a.server.tasks import TaskUpdater
from a2a.types import TaskState

from common.a2a_client import WardenClient
from common.server import json_part, parse_json_part, publish_message, run_server, start_task

LOGGER = logging.getLogger("order-agent")

WARDEN_URL = os.environ.get("A2AWARDEN_URL", "http://127.0.0.1:8080/")


class OrderExecutor(AgentExecutor):
    """Coordinates the inventory, payments, and delivery agents."""

    def __init__(self) -> None:
        self._warden = WardenClient(WARDEN_URL)

    async def execute(self, context: RequestContext, event_queue: EventQueue) -> None:
        await start_task(context, event_queue)
        updater = TaskUpdater(event_queue, context.task_id, context.context_id)
        await updater.update_status(TaskState.TASK_STATE_WORKING)

        order = parse_json_part(get_message_text(context.message) or "{}")
        context_id = context.context_id  # correlate all sub-tasks with this order

        # 1. Inventory check.
        await publish_message(updater, event_queue, {"step": "checking ingredients"})
        availability = await self._warden.call(
            skill="check-ingredients",
            target_agent="inventory-agent",
            payload={"items": order["items"]},
            context_id=context_id,
        )
        if not availability.get("available", False):
            await updater.update_status(
                TaskState.TASK_STATE_FAILED,
                message=updater.new_agent_message(
                    [json_part({"error": "missing ingredients", "missing": availability["missing"]})]
                ),
            )
            return

        # 2. Charge the order.
        await publish_message(updater, event_queue, {"step": "charging order"})
        payment = await self._warden.call(
            skill="charge-order",
            target_agent="payments-agent",
            payload={"orderId": order["orderId"], "items": order["items"]},
            context_id=context_id,
        )

        # 3. Apply the coupon, if any. If the discount exceeds the agent's
        #    autonomy, this task is held at the warden for human approval.
        total = payment["charged"]
        coupon = order.get("coupon")
        if coupon:
            await publish_message(
                updater,
                event_queue,
                {"step": "applying coupon", "coupon": coupon},
            )
            discount = await self._warden.call(
                skill="apply-discount",
                target_agent="payments-agent",
                payload={
                    "orderId": order["orderId"],
                    "items": order["items"],
                    "coupon": coupon,
                },
                context_id=context_id,
            )
            total = discount["total"]

        # 4. Delivery. Out-of-radius addresses require a gated zone exception.
        await publish_message(updater, event_queue, {"step": "assigning delivery"})
        delivery = await self._warden.call(
            skill="assign-delivery",
            target_agent="delivery-agent",
            payload={"orderId": order["orderId"], "address": order["address"]},
            context_id=context_id,
        )
        if delivery.get("requiresZoneException"):
            await publish_message(
                updater,
                event_queue,
                {"step": "requesting zone exception"},
            )
            delivery = await self._warden.call(
                skill="zone-exception",
                target_agent="delivery-agent",
                payload={"orderId": order["orderId"], "address": order["address"]},
                context_id=context_id,
            )

        # 5. Final summary artifact for the customer-facing caller.
        summary = {
            "orderId": order["orderId"],
            "customer": order.get("customer"),
            "items": order["items"],
            "total": total,
            "currency": "EUR",
            "driver": delivery["driver"],
            "etaMinutes": delivery["etaMinutes"],
        }
        LOGGER.info("order %s completed: %s", order.get("orderId"), summary)
        await updater.add_artifact([json_part(summary)], name="order-summary")
        await updater.complete()

    async def cancel(self, context: RequestContext, event_queue: EventQueue) -> None:
        raise NotImplementedError("cancel is not supported by the order agent")


if __name__ == "__main__":
    run_server(
        host="127.0.0.1",
        port=9101,
        name="order-agent",
        description="Orchestrates pizza orders across inventory, payments, and delivery agents.",
        version="0.1.0",
        skills=[
            {
                "id": "place-order",
                "description": "Place a pizza order and coordinate the involved agents.",
                "tags": ["orders"],
            }
        ],
        executor=OrderExecutor(),
    )
