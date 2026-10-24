"""Triggers a refund to demo the approval inbox.

The refund task is held at the warden in input-required; open the console at
http://127.0.0.1:8080/ui, approve or reject it in the approvals inbox, and
the script prints the result (or the rejection).
"""

from __future__ import annotations

import asyncio
import os

from common.a2a_client import WardenClient

WARDEN_URL = os.environ.get("A2AWARDEN_URL", "http://127.0.0.1:8080/")


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
