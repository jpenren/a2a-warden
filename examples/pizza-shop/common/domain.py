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
