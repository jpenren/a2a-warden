# Pizza shop demo agents for A2A Warden

Example agent suite for the "pizza restaurant order management" use case. Four agents are built with the official `a2a-python` SDK as pure A2A servers, and every inter-agent call is routed through the A2A Warden hub, never peer-to-peer. Approval gating lives in the warden (configured per skill), so the agents contain no approval logic: the warden holds a task in `input-required` until a human decides in the embedded console, then forwards it.

Architecture of the demo:

```text
                 +----------------------+
   customer  --> |  order-agent :9101  |  skill: place-order (orchestrator)
                 +----------+-----------+
                            |  all calls go through the warden
                            v
                 +----------------------+
                 |  A2A WARDEN  :8080   |  records everything, gates skills by policy,
                 |  supervision hub     |  holds input-required tasks for human approval
                 +----+-------+--------+
                      |       |        \
        +-------------+       +----+    +-------------+
        v                          v                   v
+----------------+   +------------------+   +--------------------+
| inventory-agent|   | payments-agent   |   | delivery-agent     |
| :9102          |   | :9103           |   | :9104              |
| check-ingred.  |   | charge-order    |   | assign-delivery     |
|                |   | apply-discount* |   | zone-exception*    |
|                |   | refund-order*   |   |                    |
+----------------+   +------------------+   +--------------------+
     (* approval required by warden policy: task is held in input-required
      until a human approves it in the console at /ui)
```

Target layout in the repository (`examples/pizza-shop/`):

```text
examples/pizza-shop/
├── README.md
├── requirements.txt
├── warden-pizzeria.yaml
├── common/
│   ├── __init__.py
│   ├── a2a_client.py      # minimal A2A v1.0 JSON-RPC client toward the warden
│   ├── domain.py          # shared pizza-shop domain model
│   └── server.py          # helpers to expose an agent with the official SDK
├── agents/
│   ├── __init__.py
│   ├── order_agent.py
│   ├── inventory_agent.py
│   ├── payments_agent.py
│   └── delivery_agent.py
└── client/
    ├── place_order.py     # simulates the front-of-house / conversation agent
    └── refund_order.py    # triggers a refund to demo the approval inbox
```

Note on SDK versions: the server code targets the official `a2a-python` SDK v1.x. Exact names of a few SDK fields (`AgentCard.supportedInterfaces`, `Part` construction) have shifted between SDK releases; comments mark the spots to verify against the pinned version in `requirements.txt`. The wire-level JSON-RPC client is intentionally dependency-light and targets the v1.0 JSON-RPC binding, tolerating v0.3 state spellings while the ecosystem migrates.

## File: requirements.txt

```text
# Pin these to the versions you actually test against.
a2a-sdk          # official Agent2Agent (A2A) Python SDK, v1.x
httpx>=0.27
uvicorn>=0.30
pydantic>=2
```

## File: warden-pizzeria.yaml

```yaml
# A2A Warden demo configuration for the pizza shop scenario.
# The warden routes by skill, records all traffic, and holds the
# approval-gated skills in input-required until a human decides.

server:
  listen_addr: "0.0.0.0:8080"

database:
  backend: sqlite
  sqlite:
    path: "./a2a-warden-pizzeria.db"

admin:
  api_token: "change-me"   # static token for the admin API and console in the demo

agents:
  - name: order-agent
    url: "http://127.0.0.1:9101/"
    skills: ["place-order"]

  - name: inventory-agent
    url: "http://127.0.0.1:9102/"
    skills: ["check-ingredients"]

  - name: payments-agent
    url: "http://127.0.0.1:9103/"
    skills: ["charge-order", "apply-discount", "refund-order"]
    # Any task addressed to these skills is held for human approval:
    require_approval_skills: ["apply-discount", "refund-order"]

  - name: delivery-agent
    url: "http://127.0.0.1:9104/"
    skills: ["assign-delivery", "zone-exception"]
    require_approval_skills: ["zone-exception"]

retention:
  audit_days: 90
```

## File: common/a2a_client.py

```python
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
import uuid

import httpx

A2A_HEADERS = {"Content-Type": "application/json", "A2A-Version": "1.0"}

_COMPLETED = {"TASK_STATE_COMPLETED", "COMPLETED", "completed"}
_FAILED = {"TASK_STATE_FAILED", "FAILED", "failed"}
_CANCELED = {"TASK_STATE_CANCELED", "TASK_STATE_CANCELLED", "CANCELED", "CANCELLED", "canceled", "cancelled"}
_INPUT_REQUIRED = {"TASK_STATE_INPUT_REQUIRED", "INPUT_REQUIRED", "INPUT-REQUIRED", "input-required"}


class WardenError(RuntimeError):
    """Raised when a task through the warden fails, is cancelled, or times out."""


def task_id_of(task: dict) -> str:
    """Read the task id across v1.0 (`taskId`) and v0.3 (`id`) spellings."""
    return task.get("taskId") or task.get("id") or ""


def normalize_state(task: dict) -> str:
    status = task.get("status") or {}
    return str(status.get("state") or "")


def first_artifact_payload(task: dict) -> dict:
    """Extract the JSON payload of the first artifact of a completed task."""
    artifacts = task.get("artifacts") or []
    if not artifacts:
        return {}
    for part in artifacts[0].get("parts") or []:
        text = part.get("text")
        if text is not None:
            return json.loads(text)
        if "data" in part:
            return part["data"]
    return {}


def last_message_text(task: dict) -> str:
    for message in reversed(task.get("history") or []):
        for part in message.get("parts") or []:
            if part.get("text"):
                return part["text"]
    return ""


class WardenClient:
    """JSON-RPC client for the A2A Warden hub."""

    def __init__(self, warden_url: str) -> None:
        self._base = warden_url.rstrip("/")

    async def _rpc(self, method: str, params: dict) -> dict:
        async with httpx.AsyncClient(timeout=30.0) as http:
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
        message: dict = {
            "messageId": str(uuid.uuid4()),
            "role": "user",
            "parts": [{"text": json.dumps(payload)}],
            "metadata": {"skill": skill, "targetAgent": target_agent},
        }
        if context_id:
            message["contextId"] = context_id
        result = await self._rpc("SendMessage", {"message": message})
        return result.get("task") or result

    async def get_task(self, task_id: str) -> dict:
        return await self._rpc("GetTask", {"taskId": task_id})

    async def cancel_task(self, task_id: str) -> dict:
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
            if state in _INPUT_REQUIRED and not notified:
                print(f"  [warden] task {task_id_of(task)} is WAITING FOR HUMAN APPROVAL in the warden console...")
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
        task = await self.send_message(skill=skill, target_agent=target_agent, payload=payload, context_id=context_id)
        return await self.wait_for_result(task)
```

## File: common/domain.py

```python
"""Shared pizza-shop domain model for the demo agents.

Deliberately toy-sized: prices, recipes, stock, coupons, and a two-zone
delivery model. A real deployment would query live systems; the point of
the demo is the supervision and approval flow, not the business logic.
"""

from __future__ import annotations

PIZZA_PRICES: dict[str, float] = {
    "Margherita": 9.0,
    "Pepperoni": 11.0,
    "Quattro Formaggi": 12.5,
}

# Ingredient units consumed per pizza.
RECIPE: dict[str, dict[str, int]] = {
    "Margherita": {"dough": 1, "mozzarella": 2, "tomato": 2},
    "Pepperoni": {"dough": 1, "mozzarella": 2, "tomato": 2, "pepperoni": 3},
    "Quattro Formaggi": {"dough": 1, "mozzarella": 2, "gorgonzola": 2},
}

# Current kitchen stock.
STOCK: dict[str, int] = {
    "dough": 40,
    "mozzarella": 30,
    "tomato": 25,
    "pepperoni": 18,
    "gorgonzola": 10,
}

COUPONS: dict[str, int] = {
    "SAVE25": 25,  # 25% -> approval gated by the warden policy for apply-discount
    "SAVE10": 10,
}

DRIVERS: list[str] = ["Marco", "Giulia", "Pietro"]


def zone_of(address: str) -> str:
    """Toy zone model: addresses containing 'out-of-town' are outside the usual radius."""
    return "far" if "out-of-town" in address.lower() else "near"


def total_for(items: list[dict]) -> float:
    """Gross total for a list of {'name': ..., 'qty': ...} items."""
    total = 0.0
    for item in items:
        price = PIZZA_PRICES.get(item["name"])
        if price is None:
            raise ValueError(f"unknown pizza: {item['name']}")
        total += price * int(item.get("qty", 1))
    return total


def check_availability(items: list[dict]) -> dict:
    """Aggregate ingredient needs for the order and compare against stock."""
    needed: dict[str, int] = {}
    for item in items:
        recipe = RECIPE.get(item["name"], {})
        for ingredient, per_pizza in recipe.items():
            needed[ingredient] = needed.get(ingredient, 0) + per_pizza * int(item.get("qty", 1))
    missing = {k: v for k, v in needed.items() if STOCK.get(k, 0) < v}
    return {"needed": needed, "missing": missing, "available": not missing}
```

## File: common/server.py

```python
"""Helpers to expose an agent as an A2A server using the official a2a-python SDK.

Verify the marked spots against the pinned SDK version: the AgentCard shape
and the Part union construction have both evolved between SDK releases.
"""

from __future__ import annotations

import json

import uvicorn

from a2a.server.apps import A2AStarletteApplication
from a2a.server.request_handlers import DefaultRequestHandler
from a2a.server.tasks import InMemoryTaskStore
from a2a.types import AgentCapabilities, AgentCard, AgentSkill, Part, TextPart


def text_part(text: str) -> Part:
    """Build a text Part, compatible with older and newer a2a-python versions."""
    try:
        return Part(root=TextPart(text=text))
    except Exception:  # pragma: no cover - SDK version dependent
        return Part(TextPart(text=text))


def json_part(payload: dict) -> Part:
    """Serialize a JSON payload as a text part (the demo's message convention)."""
    return text_part(json.dumps(payload))


def run_server(
    *,
    host: str,
    port: int,
    name: str,
    description: str,
    version: str,
    skills: list[dict],
    executor,
) -> None:
    """Build the Starlette app for an executor and serve it.

    `skills` is a plain list of dicts so each agent file stays free of SDK
    type import details; they are mapped to AgentSkill here.
    """
    agent_skills = [
        AgentSkill(id=s["id"], name=s["id"], description=s["description"], tags=s.get("tags", []))
        for s in skills
    ]

    agent_card = AgentCard(
        name=name,
        description=description,
        url=f"http://{host}:{port}/",
        version=version,
        capabilities=AgentCapabilities(streaming=False, push_notifications=False),
        skills=agent_skills,
        # NOTE: with a2a-python v1.x, add supportedInterfaces if the pinned
        # version enforces it, e.g.:
        #   supportedInterfaces=[AgentInterface(
        #       url=f"http://{host}:{port}/", protocolBinding="JSONRPC",
        #       protocolVersion="1.0")]
    )

    handler = DefaultRequestHandler(agent_executor=executor, task_store=InMemoryTaskStore())
    app = A2AStarletteApplication(agent_card=agent_card, http_handler=handler).build()
    uvicorn.run(app, host=host, port=port)
```

## File: agents/inventory_agent.py

```python
"""Inventory agent.

Skills:
    - check-ingredients: reports whether the kitchen can prepare the order.

This agent knows nothing about approvals: if the warden policy gates a
skill, the task is held at the warden before it ever reaches this code.
"""

from __future__ import annotations

import json

from a2a.server.agent_execution import AgentExecutor, RequestContext
from a2a.server.events import EventQueue
from a2a.server.tasks import TaskUpdater
from a2a.types import TaskState
from a2a.utils.message import get_message_text

from common import domain
from common.server import json_part, run_server


class InventoryExecutor(AgentExecutor):
    async def execute(self, context: RequestContext, event_queue: EventQueue) -> None:
        updater = TaskUpdater(event_queue, context.task_id, context.context_id)
        await updater.update_status(TaskState.WORKING)

        request = json.loads(get_message_text(context.message) or "{}")
        items = request.get("items", [])
        result = domain.check_availability(items)

        await updater.add_artifact([json_part(result)], name="availability")
        await updater.submit()

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
```

## File: agents/payments_agent.py

```python
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

import json
import uuid

from a2a.server.agent_execution import AgentExecutor, RequestContext
from a2a.server.events import EventQueue
from a2a.server.tasks import TaskUpdater
from a2a.types import TaskState
from a2a.utils.message import get_message_text

from common import domain
from common.server import json_part, run_server

# Mock ledger shared by all skills of this process instance.
LEDGER: dict[str, dict] = {}


class PaymentsExecutor(AgentExecutor):
    async def execute(self, context: RequestContext, event_queue: EventQueue) -> None:
        updater = TaskUpdater(event_queue, context.task_id, context.context_id)
        await updater.update_status(TaskState.WORKING)

        request = json.loads(get_message_text(context.message) or "{}")
        skill = (context.message.metadata or {}).get("skill", "")
        result = self._handle(skill, request)

        await updater.add_artifact([json_part(result)], name="payments-result")
        await updater.submit()

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
```

## File: agents/delivery_agent.py

```python
"""Delivery agent.

Skills:
    - assign-delivery: assigns a driver when the address is inside the radius.
    - zone-exception:   assigns a driver for an out-of-radius address.

`zone-exception` is approval-gated by the warden policy: an out-of-radius
order pauses at the warden until a human approves the exception. The agent
itself is unaware of the gate.
"""

from __future__ import annotations

import json

from a2a.server.agent_execution import AgentExecutor, RequestContext
from a2a.server.events import EventQueue
from a2a.server.tasks import TaskUpdater
from a2a.types import TaskState
from a2a.utils.message import get_message_text

from common import domain
from common.server import json_part, run_server

_ASSIGNED: list[str] = []  # toy rotation of drivers


class DeliveryExecutor(AgentExecutor):
    async def execute(self, context: RequestContext, event_queue: EventQueue) -> None:
        updater = TaskUpdater(event_queue, context.task_id, context.context_id)
        await updater.update_status(TaskState.WORKING)

        request = json.loads(get_message_text(context.message) or "{}")
        skill = (context.message.metadata or {}).get("skill", "")
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

        await updater.add_artifact([json_part(result)], name="delivery-result")
        await updater.submit()

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
```

## File: agents/order_agent.py

```python
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

import json
import os

from a2a.server.agent_execution import AgentExecutor, RequestContext
from a2a.server.events import EventQueue
from a2a.server.tasks import TaskUpdater
from a2a.types import TaskState
from a2a.utils.message import get_message_text

from common.a2a_client import WardenClient
from common.server import json_part, run_server

WARDEN_URL = os.environ.get("A2AWARDEN_URL", "http://127.0.0.1:8080/")


class OrderExecutor(AgentExecutor):
    def __init__(self) -> None:
        self._warden = WardenClient(WARDEN_URL)

    async def execute(self, context: RequestContext, event_queue: EventQueue) -> None:
        updater = TaskUpdater(event_queue, context.task_id, context.context_id)
        await updater.update_status(TaskState.WORKING)

        order = json.loads(get_message_text(context.message) or "{}")
        context_id = context.context_id  # correlate all sub-tasks with this order

        # 1. Inventory check.
        await updater.add_message([json_part({"step": "checking ingredients"})])
        availability = await self._warden.call(
            skill="check-ingredients",
            target_agent="inventory-agent",
            payload={"items": order["items"]},
            context_id=context_id,
        )
        if not availability.get("available", False):
            await updater.update_status(
                TaskState.FAILED,
                message_parts=[json_part({"error": "missing ingredients", "missing": availability["missing"]})],
            )
            return

        # 2. Charge the order.
        await updater.add_message([json_part({"step": "charging order"})])
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
            await updater.add_message([json_part({"step": "applying coupon", "coupon": coupon})])
            discount = await self._warden.call(
                skill="apply-discount",
                target_agent="payments-agent",
                payload={"orderId": order["orderId"], "items": order["items"], "coupon": coupon},
                context_id=context_id,
            )
            total = discount["total"]

        # 4. Delivery. Out-of-radius addresses require a gated zone exception.
        await updater.add_message([json_part({"step": "assigning delivery"})])
        delivery = await self._warden.call(
            skill="assign-delivery",
            target_agent="delivery-agent",
            payload={"orderId": order["orderId"], "address": order["address"]},
            context_id=context_id,
        )
        if delivery.get("requiresZoneException"):
            await updater.add_message([json_part({"step": "requesting zone exception"})])
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
        await updater.add_artifact([json_part(summary)], name="order-summary")
        await updater.submit()

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
            {"id": "place-order", "description": "Place a pizza order and coordinate the involved agents.", "tags": ["orders"]},
        ],
        executor=OrderExecutor(),
    )
```

## File: client/place_order.py

```python
"""Simulates the customer-facing (front-of-house) caller.

Places an order through the warden and prints the final summary. With the
sample order below, two approval-gated skills fire in sequence:
    1. apply-discount (coupon SAVE25)  -> approve it in the warden console
    2. zone-exception (out-of-town)    -> approve it in the warden console
Watch the timeline at http://127.0.0.1:8080/ui while the script runs.
"""

from __future__ import annotations

import asyncio

from common.a2a_client import WardenClient

WARDEN_URL = "http://127.0.0.1:8080/"

ORDER = {
    "orderId": "ORD-1001",
    "customer": "Alice",
    "address": "Calle Mayor 5, out-of-town",
    "items": [
        {"name": "Margherita", "qty": 2},
        {"name": "Pepperoni", "qty": 1},
    ],
    "coupon": "SAVE25",
}


async def main() -> None:
    warden = WardenClient(WARDEN_URL)
    print(f"Placing order {ORDER['orderId']} through A2A Warden...")
    summary = await warden.call(skill="place-order", target_agent="order-agent", payload=ORDER)
    print("Order completed:")
    print(summary)


if __name__ == "__main__":
    asyncio.run(main())
```

## File: client/refund_order.py

```python
"""Triggers a refund to demo the approval inbox.

The refund task is held at the warden in input-required; open the console at
http://127.0.0.1:8080/ui, approve or reject it in the approvals inbox, and
the script prints the result (or the rejection).
"""

from __future__ import annotations

import asyncio

from common.a2a_client import WardenClient

WARDEN_URL = "http://127.0.0.1:8080/"


async def main() -> None:
    warden = WardenClient(WARDEN_URL)
    print("Requesting refund for ORD-1001 (waiting for human approval)...")
    refund = await warden.call(
        skill="refund-order",
        target_agent="payments-agent",
        payload={"orderId": "ORD-1001"},
    )
    print("Refund completed:")
    print(refund)


if __name__ == "__main__":
    asyncio.run(main())
```

## File: README.md

```markdown
# Pizza shop demo agents for A2A Warden

Four agents (order, inventory, payments, delivery) that place pizza orders
while A2A Warden supervises every message and holds approval-gated tasks
for human decision.

## Run

```bash
pip install -r requirements.txt

# 1. Start the warden with the pizza configuration (from the a2a-warden repo):
a2a-warden --config examples/pizza/warden-pizzeria.yaml

# 2. Start the four agents (one terminal each, from examples/pizza):
python -m agents.inventory_agent
python -m agents.payments_agent
python -m agents.delivery_agent
python -m agents.order_agent

# 3. Open the warden console: http://127.0.0.1:8080/ui

# 4. Place an order (needs two approvals in the console while it runs):
python -m client.place_order

# 5. In a second terminal, trigger a refund and approve it in the console:
python -m client.refund_order
```

## What to observe in the warden console

- The order timeline groups every sub-task (inventory, payments, delivery)
  under the order context, with the full content of each message.
- Tasks addressed to `apply-discount`, `refund-order`, and `zone-exception`
  appear in the approvals inbox in input-required state; nothing has been
  forwarded to the target agent until a human approves.
- Every decision (who approved, when, over which content) is recorded in the
  audit log alongside the messages.
```

## Integration notes for the a2a-warden repository

- The demo doubles as a richer acceptance scenario beyond the initial demo
  agent: the E2E suite can drive `place_order.py` and assert that (a) the
  discount task is held in input-required with no traffic to
  `payments-agent`, (b) after admin API approval the order completes, and
  (c) the warden timeline contains every sub-task under one context.
- The per-skill gating in `warden-pizzeria.yaml` (`require_approval_skills`)
  is a minimal extension of the production-level `require_approval` boolean in the
  project plan; the Cedar policy engine in phase 4 replaces both.
- Routing metadata (`skill`, `targetAgent`) is the demo's convention; the
  warden can alternatively resolve agents purely by skill from its registry.
```