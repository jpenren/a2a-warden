//! Golden interoperability vectors for signed Agent Cards.
//!
//! The fixtures under `tests/fixtures` were produced by the official Python
//! (`a2a-sdk` 1.2.3) and JavaScript (`@a2a-js/sdk` 1.3.0) SDKs, plus one card
//! signed by the warden's own implementation. The generation scripts and the
//! findings are documented in `docs/agent-card-verification.md`.
//!
//! These tests are the executable part of that document: they fail if the
//! warden stops accepting an SDK signature, or starts accepting a tampered one.

use std::sync::Arc;

use serde_json::{json, Value};

use a2a_warden_core::verification::CardVerifier;
use a2a_warden_crypto::{
    canonical_payloads, AgentCardVerifier, AllowedAlgorithm, JwksResolver, JwksSource,
};

/// Every pinned algorithm, in the order the verifier admits them.
const PINNED: [AllowedAlgorithm; 5] = [
    AllowedAlgorithm::Es256,
    AllowedAlgorithm::Es384,
    AllowedAlgorithm::Es512,
    AllowedAlgorithm::EdDsa,
    AllowedAlgorithm::Rs256,
];

/// Reads a fixture as JSON.
fn fixture(name: &str) -> Value {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join(name);
    let bytes = std::fs::read(&path).unwrap_or_else(|error| {
        panic!("cannot read fixture {}: {error}", path.display());
    });
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("fixture {name} is not valid JSON: {error}"))
}

/// Builds a verifier over the committed public JWKS, pinned to every algorithm.
fn verifier() -> AgentCardVerifier {
    let source = JwksSource {
        issuer: None,
        jwks_json: Some(fixture("jwks.json")),
        url: None,
        allowed_algorithms: PINNED.to_vec(),
    };
    AgentCardVerifier::new(Arc::new(JwksResolver::new(vec![source])))
}

/// The fixtures and the key/algorithm they are expected to verify with.
fn sdk_vectors() -> [(&'static str, &'static str, &'static str); 5] {
    [
        ("python-es256.card.json", "python-es256", "ES256"),
        ("python-es384.card.json", "python-es384", "ES384"),
        ("python-es512.card.json", "python-es512", "ES512"),
        ("python-eddsa.card.json", "python-eddsa", "EdDSA"),
        ("python-rs256.card.json", "python-rs256", "RS256"),
    ]
}

#[tokio::test]
async fn cards_signed_by_the_python_sdk_verify_for_every_pinned_algorithm() {
    let verifier = verifier();
    for (name, kid, alg) in sdk_vectors() {
        let outcome = verifier.verify_card(&fixture(name), true, true).await;
        assert!(outcome.verified, "{name} was rejected: {:?}", outcome.error);
        assert!(!outcome.quarantined, "{name} must not be quarantined");
        assert_eq!(outcome.kid.as_deref(), Some(kid), "{name} key id");
        assert_eq!(outcome.alg.as_deref(), Some(alg), "{name} algorithm");
    }
}

#[tokio::test]
async fn a_card_signed_by_the_javascript_sdk_verifies() {
    let outcome = verifier().verify_card(&fixture("javascript-es256.card.json"), true, true).await;
    assert!(outcome.verified, "JavaScript vector rejected: {:?}", outcome.error);
    assert_eq!(outcome.kid.as_deref(), Some("javascript-es256"));
    assert_eq!(outcome.alg.as_deref(), Some("ES256"));
}

#[tokio::test]
async fn a_card_signed_by_the_warden_verifies() {
    let outcome = verifier().verify_card(&fixture("warden-es256.card.json"), true, true).await;
    assert!(outcome.verified, "warden vector rejected: {:?}", outcome.error);
    assert_eq!(outcome.kid.as_deref(), Some("warden-es256"));
}

#[tokio::test]
async fn a_signature_produced_through_the_core_trait_is_accepted() {
    // Guards the wiring contract: the service only ever sees `dyn CardVerifier`.
    let verifier: Arc<dyn CardVerifier> = Arc::new(verifier());
    let outcome = verifier.verify_card(&fixture("python-es256.card.json"), true, true).await;
    assert!(outcome.verified, "{:?}", outcome.error);
}

#[tokio::test]
async fn a_tampered_card_is_rejected_and_quarantined() {
    let mut card = fixture("python-es256.card.json");
    card["description"] = json!("tampered after signing");

    let outcome = verifier().verify_card(&card, true, true).await;
    assert!(!outcome.verified);
    assert!(outcome.quarantined);
    assert!(outcome.error.is_some());
}

#[tokio::test]
async fn an_unsigned_card_is_rejected_when_verification_is_mandatory() {
    let mut card = fixture("python-es256.card.json");
    card.as_object_mut().expect("object").remove("signatures");

    let outcome = verifier().verify_card(&card, true, true).await;
    assert!(!outcome.verified);
    assert!(outcome.quarantined);
    assert!(outcome.error.expect("reason").contains("unsigned"));
}

#[tokio::test]
async fn an_algorithm_outside_the_pin_is_rejected() {
    // The card is a valid ES256 vector, but this key set only admits EdDSA.
    let source = JwksSource {
        issuer: None,
        jwks_json: Some(fixture("jwks.json")),
        url: None,
        allowed_algorithms: vec![AllowedAlgorithm::EdDsa],
    };
    let verifier = AgentCardVerifier::new(Arc::new(JwksResolver::new(vec![source])));

    let outcome = verifier.verify_card(&fixture("python-es256.card.json"), true, true).await;
    assert!(!outcome.verified);
    assert!(outcome.quarantined);
}

#[tokio::test]
async fn a_valid_signature_after_an_invalid_one_is_accepted() {
    let mut card = fixture("python-es256.card.json");
    let signatures = card["signatures"].as_array().expect("signatures").clone();
    let bogus = json!({
        "protected": "eyJhbGciOiJFUzI1NiIsImtpZCI6InB5dGhvbi1lczI1NiJ9",
        "signature": "AAAA",
    });
    card["signatures"] = json!([bogus, signatures[0].clone()]);

    let outcome = verifier().verify_card(&card, true, true).await;
    assert!(outcome.verified, "{:?}", outcome.error);
}

#[tokio::test]
async fn sdk_normalization_handles_empty_members_the_sdks_do_not_sign() {
    // The Python SDK drops empty values before canonicalizing, so the published
    // card can carry empty members that are not covered by the signature. The
    // warden must accept the card through its SDK-normalization candidate, and
    // plain JCS over the received JSON must be a different payload.
    let card = fixture("python-es256-empty-fields.card.json");
    let payloads = canonical_payloads(&card).expect("canonical payloads");
    assert_eq!(payloads.len(), 2, "both canonicalization candidates are needed");
    assert_ne!(payloads[0], payloads[1]);
    assert!(!payloads[0].contains("iconUrl"), "the SDK form drops the empty field");
    assert!(payloads[1].contains("iconUrl"), "plain JCS keeps the empty field");

    let outcome = verifier().verify_card(&card, true, true).await;
    assert!(outcome.verified, "{:?}", outcome.error);
}
