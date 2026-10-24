#!/usr/bin/env python3
"""Runs the pizza shop scenario end to end, with an automated supervisor.

The script places an order through the warden, approves the gated skills as
a supervisor would do in the console, and prints the final summary. It is
the automated version of the manual walkthrough of the README, and the
scenario the CI runs.

    python3 scripts/run_demo.py
"""

from __future__ import annotations

import asyncio
import json
import os
import sys
import time
from typing import Any

import httpx

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from client.place_order import ORDER  # noqa: E402
from common.a2a_client import WardenClient  # noqa: E402
from common.domain import DRIVERS  # noqa: E402

WARDEN_URL = os.environ.get("A2AWARDEN_URL", "http://127.0.0.1:8080/")
ADMIN_URL = f"{WARDEN_URL.rstrip('/')}/api/v1/admin"

#: How long to wait for a pending approval to show up, in seconds.
APPROVAL_TIMEOUT = 60.0


async def pending_approvals(client: httpx.AsyncClient) -> list[dict[str, Any]]:
    """Lists the approvals waiting for a decision."""
    response = await client.get(f"{ADMIN_URL}/approvals", params={"status": "pending"})
    response.raise_for_status()
    return response.json().get("approvals", [])


async def approve(client: httpx.AsyncClient, approval_id: str, reason: str) -> None:
    """Approves a task as a supervisor would do from the console."""
    response = await client.post(
        f"{ADMIN_URL}/approvals/{approval_id}/decision",
        json={"decision": "approve", "approver": "run-demo", "reason": reason},
    )
    response.raise_for_status()
    print(f"  [supervisor] approved {approval_id} ({reason})")


async def supervise(client: httpx.AsyncClient) -> None:
    """Approves the gated tasks as they show up.

    The order needs more than one approval, so the supervisor keeps watching
    the inbox until the order is over.
    """
    deadline = time.monotonic() + APPROVAL_TIMEOUT
    seen: set[str] = set()
    while time.monotonic() < deadline:
        try:
            approvals = await pending_approvals(client)
        except httpx.HTTPError as error:
            print(f"  [supervisor] cannot reach the admin API: {error}", file=sys.stderr)
            await asyncio.sleep(1.0)
            continue
        for approval in approvals:
            if approval["id"] in seen:
                continue
            seen.add(approval["id"])
            await approve(client, approval["id"], "automated demo approval")
        await asyncio.sleep(0.5)
    print("  [supervisor] gave up waiting for an approval", file=sys.stderr)


async def main() -> int:
    """Places the order and returns the exit code of the demo."""
    warden = WardenClient(WARDEN_URL)
    async with httpx.AsyncClient(timeout=30.0) as client:
        print(f"Placing order {ORDER['orderId']} through A2A Warden...", flush=True)
        supervisor = asyncio.create_task(supervise(client))
        try:
            summary = await warden.call(
                skill="place-order",
                target_agent="order-agent",
                payload=ORDER,
            )
        finally:
            supervisor.cancel()

    print("Order completed:")
    print(json.dumps(summary, indent=2))

    expected = {"orderId": ORDER["orderId"], "currency": "EUR"}
    for key, value in expected.items():
        if summary.get(key) != value:
            print(f"unexpected {key}: {summary.get(key)!r} != {value!r}", file=sys.stderr)
            return 1
    if summary.get("driver") not in DRIVERS:
        print(f"unexpected driver: {summary.get('driver')!r}", file=sys.stderr)
        return 1
    if summary.get("etaMinutes") != 45:
        # The out-of-town address needs the zone exception, with its own ETA.
        print(f"unexpected eta: {summary.get('etaMinutes')!r}", file=sys.stderr)
        return 1
    # 2 Margherita (9) + 1 Pepperoni (11) = 29, with the 25% coupon: 21.75.
    if abs(float(summary.get("total", 0)) - 21.75) > 0.001:
        print(f"unexpected total: {summary.get('total')!r}", file=sys.stderr)
        return 1
    print("The order matches the expected summary.")
    return 0


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
