//! Golden vector helper: signs an Agent Card the way the warden canonicalizes
//! it, so the result can be verified by the official SDKs.
//!
//! This example is not a public signing API. It exists so
//! `docs/agent-card-verification.md` can state that a card produced by the
//! warden's own JCS/JWS implementation is accepted by both official SDKs.
//!
//! Usage:
//!
//! ```text
//! cargo run -p a2a-warden-crypto --example sign_card -- \
//!     <ec-p256-private-key.pem> <kid> <card.json>
//! ```
//!
//! The card JSON is read from the file, the `signatures` member is removed,
//! and the signed card is printed to stdout. Only ES256 is implemented: it is
//! the algorithm used by the golden vector.

use std::env;
use std::fs;
use std::process::ExitCode;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use jsonwebtoken::{Algorithm, EncodingKey};
use serde_json::{json, Value};

use a2a_warden_crypto::{canonicalize_jcs, prepare_card_for_signing};

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let (Some(key_path), Some(kid), Some(card_path)) = (args.next(), args.next(), args.next())
    else {
        eprintln!("usage: sign_card <private-key.pem> <kid> <card.json>");
        return ExitCode::FAILURE;
    };

    match run(&key_path, &kid, &card_path) {
        Ok(card) => {
            println!("{card}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("sign_card: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(key_path: &str, kid: &str, card_path: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let pem = fs::read(key_path)?;
    let card: Value = serde_json::from_slice(&fs::read(card_path)?)?;
    let payload = canonicalize_jcs(&prepare_card_for_signing(&card)?)?;

    let header = json!({ "alg": "ES256", "kid": kid, "typ": "JOSE" });
    let protected = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header)?);
    let signing_input = format!("{protected}.{}", URL_SAFE_NO_PAD.encode(payload.as_bytes()));

    let key = EncodingKey::from_ec_pem(&pem)?;
    let signature = jsonwebtoken::crypto::sign(signing_input.as_bytes(), &key, Algorithm::ES256)?;

    let mut signed = card;
    signed["signatures"] = json!([{ "protected": protected, "signature": signature }]);
    Ok(signed)
}
