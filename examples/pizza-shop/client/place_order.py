"""Simulates the customer-facing (front-of-house) caller.

Places an order through the warden and prints the final summary. With the
sample order below, two approval-gated skills fire in sequence:
    1. apply-discount (coupon SAVE25)  -> approve it in the warden console
    2. zone-exception (out-of-town)    -> approve it in the warden console
Watch the timeline at http://127.0.0.1:8080/ui while the script runs.
"""

from __future__ import annotations

import asyncio
import os

from common.a2a_client import WardenClient

WARDEN_URL = os.environ.get("A2AWARDEN_URL", "http://127.0.0.1:8080/")

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
    summary = await warden.call(
        skill="place-order",
        target_agent="order-agent",
        payload=ORDER,
    )
    print("Order completed:")
    print(summary)


if __name__ == "__main__":
    asyncio.run(main())
