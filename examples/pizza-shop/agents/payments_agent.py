"""Payments agent (mocked ledger).

Skills:
    - charge-order:   computes the gross total for the order items.
    - apply-discount: applies a coupon and returns the adjusted total.
    - refund-order:   records a refund for a completed order.

The warden policy (warden-pizzeria.yaml) gates `apply-discount` and
`refund-order`: those tasks are held in input-required at the warden until
a human approves them in the console. This file contains no approval logic
whatsoever; after approval, the warden forwards the task and the code below
runs as if nothing had happened.
"""

from __future__ import annotations

import logging
import uuid

from a2a.helpers import get_message_text
from a2a.server.agent_execution import AgentExecutor, RequestContext
from a2a.server.events import EventQueue
from a2a.server.tasks import TaskUpdater
from a2a.types import TaskState

from common import domain
from common.server import json_part, parse_json_part, run_server, skill_of, start_task

LOGGER = logging.getLogger("payments-agent")

# Mock ledger shared by all skills of this process instance.
LEDGER: dict[str, dict] = {}


class PaymentsExecutor(AgentExecutor):
    """Charges orders, applies discounts, and issues refunds."""

    async def execute(self, context: RequestContext, event_queue: EventQueue) -> None:
        await start_task(context, event_queue)
        updater = TaskUpdater(event_queue, context.task_id, context.context_id)
        await updater.update_status(TaskState.TASK_STATE_WORKING)

        request = parse_json_part(get_message_text(context.message) or "")
        skill = skill_of(context.message)
        result = self._handle(skill, request)
        LOGGER.info("skill=%s result=%s", skill, result)

        await updater.add_artifact([json_part(result)], name="payments-result")
        await updater.complete()

    def _handle(self, skill: str, request: dict) -> dict:
        if skill == "charge-order":
            total = domain.total_for(request["items"])
            record = {"orderId": request["orderId"], "charged": total, "currency": "EUR"}
            LEDGER[request["orderId"]] = dict(record)
            return record

        if skill == "apply-discount":
            order_id = request["orderId"]
            coupon = request.get("coupon", "")
            percent = domain.COUPONS.get(coupon, 0)
            if percent <= 0:
                raise ValueError(f"unknown coupon: {coupon}")
            total = domain.total_for(request["items"])
            discount = round(total * percent / 100.0, 2)
            record = LEDGER.get(order_id, {"charged": total})
            record["discount"] = discount
            record["coupon"] = coupon
            record["total"] = round(record["charged"] - discount, 2)
            LEDGER[order_id] = record
            return record

        if skill == "refund-order":
            order_id = request["orderId"]
            record = LEDGER.get(order_id)
            if not record:
                raise ValueError(f"no payment record for order {order_id}")
            refund = {
                "refundId": str(uuid.uuid4()),
                "orderId": order_id,
                "refunded": record.get("total", record.get("charged", 0.0)),
                "currency": "EUR",
            }
            return refund

        raise ValueError(f"unsupported skill: {skill}")

    async def cancel(self, context: RequestContext, event_queue: EventQueue) -> None:
        raise NotImplementedError("cancel is not supported by the payments agent")


if __name__ == "__main__":
    run_server(
        host="127.0.0.1",
        port=9103,
        name="payments-agent",
        description="Charges orders, applies discounts, and issues refunds.",
        version="0.1.0",
        skills=[
            {"id": "charge-order", "description": "Compute and record the order total.", "tags": ["payments"]},
            {"id": "apply-discount", "description": "Apply a coupon to an order.", "tags": ["payments", "approval-gated"]},
            {"id": "refund-order", "description": "Refund a completed order.", "tags": ["payments", "approval-gated"]},
        ],
        executor=PaymentsExecutor(),
    )
