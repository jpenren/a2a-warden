#!/usr/bin/env python3
"""Generates the Agent Card golden vectors produced by the official Python SDK.

The script is reproducible: it creates fresh keys on every run, signs one Agent
Card per pinned algorithm with the official ``a2a-sdk`` signer, and writes the
public JWKS together with the signed cards. Re-running it produces different
signatures over the same canonical payloads, which is exactly what the Rust
suite needs to prove interoperability.

Usage (from the repository root):

    python3 crates/a2a-warden-crypto/tests/golden/generate.py

The Python SDK version is pinned in ``requirements.txt`` next to this file. The
private keys stay in ``tests/fixtures/.keys`` (git ignored); only the public
JWKS and the signed cards are meant to be committed.
"""

from __future__ import annotations

import base64
import json
import sys
from pathlib import Path

from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric import ec, ed25519, rsa
from google.protobuf.json_format import MessageToDict

from a2a.types import (
    AgentCapabilities,
    AgentCard,
    AgentInterface,
    AgentProvider,
    AgentSkill,
)
from a2a.utils.signing import create_agent_card_signer


HERE = Path(__file__).resolve().parent
FIXTURES = HERE.parent / "fixtures"
KEYS = FIXTURES / ".keys"


def b64url(data: bytes) -> str:
    """Encodes bytes as unpadded base64url."""
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode("ascii")


def int_b64url(value: int, size: int) -> str:
    """Encodes an integer as a fixed size, unpadded base64url big endian."""
    return b64url(value.to_bytes(size, "big"))


def ec_key(name: str, curve, kid: str, alg: str, size: int) -> dict:
    """Creates an EC key pair and returns its JWK."""
    private = ec.generate_private_key(curve, backend=None)
    pem = private.private_bytes(
        serialization.Encoding.PEM,
        serialization.PrivateFormat.PKCS8,
        serialization.NoEncryption(),
    )
    (KEYS / f"{name}-private.pem").write_bytes(pem)
    numbers = private.public_key().public_numbers()
    crv = {32: "P-256", 48: "P-384", 66: "P-521"}[size]
    return {
        "kty": "EC",
        "crv": crv,
        "x": int_b64url(numbers.x, size),
        "y": int_b64url(numbers.y, size),
        "use": "sig",
        "kid": kid,
        "alg": alg,
    }


def ed25519_key(name: str, kid: str) -> dict:
    """Creates an Ed25519 key pair and returns its JWK."""
    private = ed25519.Ed25519PrivateKey.generate()
    pem = private.private_bytes(
        serialization.Encoding.PEM,
        serialization.PrivateFormat.PKCS8,
        serialization.NoEncryption(),
    )
    (KEYS / f"{name}-private.pem").write_bytes(pem)
    raw = private.public_key().public_bytes(
        serialization.Encoding.Raw, serialization.PublicFormat.Raw
    )
    return {"kty": "OKP", "crv": "Ed25519", "x": b64url(raw), "use": "sig", "kid": kid, "alg": "EdDSA"}


def rsa_key(name: str, kid: str) -> dict:
    """Creates an RSA 2048 key pair and returns its JWK."""
    private = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    pem = private.private_bytes(
        serialization.Encoding.PEM,
        serialization.PrivateFormat.PKCS8,
        serialization.NoEncryption(),
    )
    (KEYS / f"{name}-private.pem").write_bytes(pem)
    numbers = private.public_key().public_numbers()
    return {
        "kty": "RSA",
        "n": int_b64url(numbers.n, (numbers.n.bit_length() + 7) // 8),
        "e": int_b64url(numbers.e, (numbers.e.bit_length() + 7) // 8),
        "use": "sig",
        "kid": kid,
        "alg": "RS256",
    }


def agent_card() -> AgentCard:
    """Builds the card used by every vector."""
    return AgentCard(
        name="Golden Agent",
        description="Agent Card used by the A2A Warden golden vector suite",
        version="1.0.0",
        supported_interfaces=[
            AgentInterface(
                url="https://agent.example/a2a",
                protocol_binding="JSONRPC",
                protocol_version="1.0",
            )
        ],
        provider=AgentProvider(
            url="https://example.com", organization="Example Organization"
        ),
        capabilities=AgentCapabilities(streaming=True, push_notifications=True),
        default_input_modes=["text/plain"],
        default_output_modes=["text/plain"],
        skills=[
            AgentSkill(
                id="echo",
                name="Echo",
                description="Repeats the received text",
                tags=["test", "echo"],
                examples=["hello"],
                input_modes=["text/plain"],
                output_modes=["text/plain"],
            )
        ],
    )


def sign(private_pem: Path, kid: str, alg: str) -> dict:
    """Signs the shared card with the official SDK signer."""
    signer = create_agent_card_signer(
        private_pem.read_bytes(),
        {"kid": kid, "alg": alg, "typ": "JOSE"},
    )
    return MessageToDict(signer(agent_card()))


def write(name: str, value: dict) -> None:
    """Writes a JSON fixture with a stable formatting."""
    path = FIXTURES / name
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {path.relative_to(FIXTURES.parent.parent.parent.parent)}")


def main() -> int:
    FIXTURES.mkdir(parents=True, exist_ok=True)
    KEYS.mkdir(parents=True, exist_ok=True)

    keys = [
        ec_key("es256", ec.SECP256R1(), "python-es256", "ES256", 32),
        ec_key("es384", ec.SECP384R1(), "python-es384", "ES384", 48),
        ec_key("es512", ec.SECP521R1(), "python-es512", "ES512", 66),
        ed25519_key("eddsa", "python-eddsa"),
        rsa_key("rs256", "python-rs256"),
    ]
    # The warden-signed and JavaScript-signed vectors use the same EC P-256 key
    # under their own key identifiers, modelling a key that is published for
    # every SDK to consume.
    for kid in ("warden-es256", "javascript-es256"):
        alias = dict(keys[0])
        alias["kid"] = kid
        keys.append(alias)
    write("jwks.json", {"keys": keys})

    vectors = [
        ("es256", "python-es256", "ES256", "python-es256.card.json"),
        ("es384", "python-es384", "ES384", "python-es384.card.json"),
        ("es512", "python-es512", "ES512", "python-es512.card.json"),
        ("eddsa", "python-eddsa", "EdDSA", "python-eddsa.card.json"),
        ("rs256", "python-rs256", "RS256", "python-rs256.card.json"),
    ]
    signed = {}
    for name, kid, alg, fixture in vectors:
        card = sign(KEYS / f"{name}-private.pem", kid, alg)
        signed[fixture] = card
        write(fixture, card)

    # The SDK canonicalization drops empty values before JCS. Publishing the
    # card with empty members therefore produces a card that the SDK still
    # verifies (the empty members are not covered by the signature) while plain
    # JCS over the received JSON would fail. The Rust suite asserts that the
    # warden accepts this vector through its SDK-normalization path.
    with_empty = dict(signed["python-es256.card.json"])
    with_empty["iconUrl"] = ""
    with_empty["securitySchemes"] = {}
    with_empty["securityRequirements"] = []
    write("python-es256-empty-fields.card.json", with_empty)

    return 0


if __name__ == "__main__":
    sys.exit(main())
