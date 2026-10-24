#!/usr/bin/env node
// Generates the Agent Card golden vector of the official JavaScript SDK and
// verifies the vectors produced by the warden and by the Python SDK.
//
// Run `python3 generate.py` first: it creates the shared ES256 key and the
// JWKS. Usage (from this directory):
//
//     npm install
//     cargo run -p a2a-warden-crypto --example sign_card -- \
//         ../fixtures/.keys/es256-private.pem warden-es256 \
//         ../fixtures/python-es256.card.json > ../fixtures/warden-es256.card.json
//     node generate.mjs
//
// Only the public JWKS and the resulting cards are meant to be committed.

import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  generateAgentCardSignature,
  verifyAgentCardSignature,
} from "@a2a-js/sdk";
import * as jose from "jose";

const here = dirname(fileURLToPath(import.meta.url));
const fixtures = resolve(here, "..", "fixtures");
const keys = join(fixtures, ".keys");
const jwks = JSON.parse(readFileSync(join(fixtures, "jwks.json"), "utf8"));

const card = {
  name: "Golden Agent",
  description: "Agent Card used by the A2A Warden golden vector suite",
  version: "1.0.0",
  supportedInterfaces: [
    {
      url: "https://agent.example/a2a",
      protocolBinding: "JSONRPC",
      protocolVersion: "1.0",
    },
  ],
  provider: {
    url: "https://example.com",
    organization: "Example Organization",
  },
  capabilities: { streaming: true, pushNotifications: true },
  defaultInputModes: ["text/plain"],
  defaultOutputModes: ["text/plain"],
  skills: [
    {
      id: "echo",
      name: "Echo",
      description: "Repeats the received text",
      tags: ["test", "echo"],
      examples: ["hello"],
      inputModes: ["text/plain"],
      outputModes: ["text/plain"],
    },
  ],
};

const privateKey = await jose.importPKCS8(
  readFileSync(join(keys, "es256-private.pem"), "utf8"),
  "ES256",
);
const signer = generateAgentCardSignature(privateKey, {
  alg: "ES256",
  kid: "javascript-es256",
  typ: "JOSE",
});
const signed = await signer(card);
const output = join(fixtures, "javascript-es256.card.json");
writeFileSync(output, `${JSON.stringify(signed, null, 2)}\n`, "utf8");
console.log(`wrote ${output}`);

/** Resolves a verification key from the public JWKS fixture. */
async function retrievePublicKey(kid) {
  const jwk = jwks.keys.find((key) => key.kid === kid);
  if (!jwk) {
    throw new Error(`no fixture key for kid '${kid}'`);
  }
  return jose.importJWK(jwk, jwk.alg);
}

const verify = verifyAgentCardSignature(retrievePublicKey);

for (const name of [
  "warden-es256.card.json",
  "python-es256-empty-fields.card.json",
]) {
  const path = join(fixtures, name);
  if (!existsSync(path)) {
    console.log(`skipped ${name}: not generated`);
    continue;
  }
  await verify(JSON.parse(readFileSync(path, "utf8")));
  console.log(`${name} verified by the JavaScript SDK`);
}
