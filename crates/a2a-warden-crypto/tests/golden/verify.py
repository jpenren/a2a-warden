#!/usr/bin/env python3
"""Verifies the cross-SDK golden vectors with the official Python SDK.

Checks that the card signed by the warden implementation and the card signed by
the JavaScript SDK both pass the Python SDK verifier. Run it after
``generate.py`` and ``generate.mjs``.

Usage (from the repository root):

    python3 crates/a2a-warden-crypto/tests/golden/verify.py
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import jwt
from google.protobuf.json_format import ParseDict

from a2a.types import AgentCard
from a2a.utils.signing import create_signature_verifier


HERE = Path(__file__).resolve().parent
FIXTURES = HERE.parent / "fixtures"

VECTORS = [
    "warden-es256.card.json",
    "javascript-es256.card.json",
    "python-es512.card.json",
    "python-eddsa.card.json",
    "python-rs256.card.json",
    "python-es256-empty-fields.card.json",
]

ALGORITHMS = ["ES256", "ES384", "ES512", "EdDSA", "RS256"]


def main() -> int:
    jwks = json.loads((FIXTURES / "jwks.json").read_text(encoding="utf-8"))
    by_kid = {key["kid"]: key for key in jwks["keys"]}

    def key_provider(kid: str | None, _jku: str | None) -> jwt.PyJWK:
        if kid not in by_kid:
            raise KeyError(f"no fixture key for kid {kid!r}")
        return jwt.PyJWK(by_kid[kid])

    verifier = create_signature_verifier(key_provider, ALGORITHMS)
    failures = 0
    for name in VECTORS:
        path = FIXTURES / name
        if not path.exists():
            print(f"skipped {name}: not generated")
            continue
        card = ParseDict(json.loads(path.read_text(encoding="utf-8")), AgentCard())
        try:
            verifier(card)
        except Exception as error:  # noqa: BLE001 - report every SDK failure
            failures += 1
            print(f"FAILED {name}: {error}")
        else:
            print(f"{name} verified by the Python SDK")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
