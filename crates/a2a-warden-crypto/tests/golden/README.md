# Golden vector generation

These scripts reproduce the interoperability vectors used by
`crates/a2a-warden-crypto/tests/golden_vectors.rs`. See
[`docs/agent-card-verification.md`](../../../../docs/agent-card-verification.md)
for the findings.

Python (official SDK 1.2.3):

```sh
python3 -m venv /tmp/a2a-py-venv
/tmp/a2a-py-venv/bin/pip install -r requirements.txt
/tmp/a2a-py-venv/bin/python generate.py
```

The warden direction (the example signs with the same JCS payload the verifier
recomputes):

```sh
cargo run -p a2a-warden-crypto --example sign_card -- \
  ../fixtures/.keys/es256-private.pem warden-es256 \
  ../fixtures/python-es256.card.json > ../fixtures/warden-es256.card.json
```

JavaScript (official SDK 1.3.0):

```sh
npm install
node generate.mjs
```

Cross-check the Python SDK against the warden and JavaScript cards:

```sh
/tmp/a2a-py-venv/bin/python verify.py
```

`tests/fixtures/.keys` holds the ephemeral private keys and is git ignored. Only
the public JWKS and the cards are committed; re-running the pipeline produces
different signatures over the same canonical payloads, which still verify.
