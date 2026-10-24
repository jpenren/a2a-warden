//! A2A Warden Crypto
//!
//! JWS over JCS verification of signed Agent Cards, plus the JWKS resolver
//! with pinned algorithms (decision ).
//!
//! The A2A specification (section 8.4) signs the Agent Card with a detached
//! JWS over the RFC 8785 canonical JSON serialization of the card, with the
//! `signatures` member removed ("proto3 field presence" semantics: a field
//! that was not set is not serialized, so it is absent from the canonical
//! bytes as well).
//!
//! This crate implements:
//!
//! * [`canonicalize_jcs`]: RFC 8785 canonicalization.
//! * [`prepare_card_for_signing`]: removes the `signatures` member.
//! * [`JwksResolver`]: configuration driven JWKS sources with caching.
//! * [`AgentCardVerifier`]: verifies every signature of a card against the
//!   pinned algorithm allowlist and the configured keys.
//!
//! Every failure is returned as a structured [`VerificationOutcome`] so the
//! service can quarantine the agent and audit the reason.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use a2a_warden_core::error::Error;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use jsonwebtoken::crypto::verify as verify_signature;
use jsonwebtoken::{Algorithm, DecodingKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::debug;

/// Algorithms that may be pinned by configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AllowedAlgorithm {
    /// ECDSA P-256 with SHA-256.
    Es256,
    /// ECDSA P-384 with SHA-384.
    Es384,
    /// ECDSA P-521 with SHA-512.
    Es512,
    /// Ed25519 (EdDSA).
    EdDsa,
    /// RSASSA-PKCS1-v1_5 with SHA-256.
    Rs256,
}

impl AllowedAlgorithm {
    /// Parses a JOSE algorithm name. Unknown algorithms are rejected.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "ES256" => Some(Self::Es256),
            "ES384" => Some(Self::Es384),
            "ES512" => Some(Self::Es512),
            "EdDSA" | "EDDSA" => Some(Self::EdDsa),
            "RS256" => Some(Self::Rs256),
            _ => None,
        }
    }

    /// Canonical JOSE name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Es256 => "ES256",
            Self::Es384 => "ES384",
            Self::Es512 => "ES512",
            Self::EdDsa => "EdDSA",
            Self::Rs256 => "RS256",
        }
    }

    /// The `jsonwebtoken` algorithm used to verify signatures.
    ///
    /// `ES512` is not supported by `jsonwebtoken`; it is verified with the
    /// pure Rust `p521` implementation instead.
    const fn jose(self) -> Option<Algorithm> {
        match self {
            Self::Es256 => Some(Algorithm::ES256),
            Self::Es384 => Some(Algorithm::ES384),
            Self::Es512 => None,
            Self::EdDsa => Some(Algorithm::EdDSA),
            Self::Rs256 => Some(Algorithm::RS256),
        }
    }
}

/// One JWKS source: either inline JSON or a URL, with its own policy.
#[derive(Debug, Clone)]
pub struct JwksSource {
    /// Issuer accepted for signatures verified with this source.
    pub issuer: Option<String>,
    /// Inline JWKS document.
    pub jwks_json: Option<Value>,
    /// Remote JWKS URL (used when `jwks_json` is absent).
    pub url: Option<String>,
    /// Pinned algorithms accepted for this source.
    pub allowed_algorithms: Vec<AllowedAlgorithm>,
}

impl JwksSource {
    /// Returns `true` when the algorithm is accepted by this source.
    #[must_use]
    pub fn allows(&self, algorithm: AllowedAlgorithm) -> bool {
        self.allowed_algorithms.is_empty() || self.allowed_algorithms.contains(&algorithm)
    }
}

/// A JSON Web Key (the subset needed for verification).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Jwk {
    /// Key type (`EC`, `RSA`, `OKP`).
    pub kty: String,
    /// Public key use (`sig`).
    #[serde(rename = "use", skip_serializing_if = "Option::is_none")]
    pub usage: Option<String>,
    /// Key operations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_ops: Option<Vec<String>>,
    /// Algorithm this key is restricted to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alg: Option<String>,
    /// Key identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kid: Option<String>,
    /// Curve for EC and OKP keys.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crv: Option<String>,
    /// EC/OKP x coordinate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<String>,
    /// EC y coordinate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<String>,
    /// RSA modulus.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<String>,
    /// RSA public exponent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub e: Option<String>,
}

/// A JSON Web Key Set.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Jwks {
    /// The keys.
    pub keys: Vec<Jwk>,
}

/// Outcome of verifying an Agent Card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationOutcome {
    /// Whether at least one signature verified.
    pub verified: bool,
    /// Whether the card must be quarantined.
    pub quarantined: bool,
    /// Reason of the failure, when verification did not succeed.
    pub error: Option<String>,
    /// Key identifier that verified the card.
    pub kid: Option<String>,
    /// Issuer carried in the JWS protected header.
    pub issuer: Option<String>,
    /// Algorithm used to verify the card.
    pub alg: Option<String>,
}

impl VerificationOutcome {
    /// A successful verification.
    #[must_use]
    fn success(kid: Option<String>, issuer: Option<String>, alg: String) -> Self {
        Self { verified: true, quarantined: false, error: None, kid, issuer, alg: Some(alg) }
    }

    /// A failed verification, quarantined when requested.
    #[must_use]
    fn failure(reason: impl Into<String>, quarantine: bool) -> Self {
        Self {
            verified: false,
            quarantined: quarantine,
            error: Some(reason.into()),
            kid: None,
            issuer: None,
            alg: None,
        }
    }

    /// An unsigned card accepted because verification is not required.
    #[must_use]
    fn unsigned_not_required() -> Self {
        Self {
            verified: false,
            quarantined: false,
            error: None,
            kid: None,
            issuer: None,
            alg: None,
        }
    }
}

/// Canonicalizes a JSON value with RFC 8785 (JCS).
///
/// # Errors
///
/// Returns [`Error::Crypto`] when the value cannot be serialized (for example
/// a number that JCS cannot represent).
pub fn canonicalize_jcs(value: &Value) -> Result<String, Error> {
    serde_json_canonicalizer::to_string(value)
        .map_err(|err| Error::Crypto(format!("JCS canonicalization failed: {err}")))
}

/// Returns the JSON object that is signed: the card without `signatures`,
/// normalized the way the official SDKs normalize it before canonicalizing.
///
/// Both the Python and the JavaScript SDKs recursively drop empty strings,
/// empty arrays, empty objects, and `null` members before applying RFC 8785.
/// This is the documented canonicalization divergence (see
/// `docs/agent-card-verification.md`): plain JCS over the received JSON is not
/// enough to interoperate with them.
///
/// # Errors
///
/// Returns [`Error::Crypto`] when the value is not a JSON object.
pub fn prepare_card_for_signing(card: &Value) -> Result<Value, Error> {
    let mut object = card
        .as_object()
        .cloned()
        .ok_or_else(|| Error::Crypto("an Agent Card must be a JSON object".to_owned()))?;
    object.remove("signatures");
    Ok(clean_empty(Value::Object(object)).unwrap_or_else(|| Value::Object(serde_json::Map::new())))
}

/// Recursively drops empty strings, empty arrays, empty objects, and `null`,
/// mirroring the `_clean_empty`/`cleanEmpty` helpers of the official SDKs.
///
/// `false` and `0` are meaningful values and are preserved.
fn clean_empty(value: Value) -> Option<Value> {
    match value {
        Value::Null => None,
        Value::String(text) if text.is_empty() => None,
        Value::Array(items) => {
            let cleaned = items.into_iter().filter_map(clean_empty).collect::<Vec<_>>();
            (!cleaned.is_empty()).then_some(Value::Array(cleaned))
        }
        Value::Object(members) => {
            let cleaned = members
                .into_iter()
                .filter_map(|(key, value)| clean_empty(value).map(|value| (key, value)))
                .collect::<serde_json::Map<_, _>>();
            (!cleaned.is_empty()).then_some(Value::Object(cleaned))
        }
        other => Some(other),
    }
}

/// Returns every canonical payload that may have been signed for this card.
///
/// The first candidate is the SDK normalization; the second is plain JCS over
/// the card without `signatures`, which is what a signer that follows the
/// specification literally produces. A signature is accepted if it verifies
/// any candidate, so the warden interoperates with both conventions without
/// weakening the signature check.
///
/// # Errors
///
/// Returns [`Error::Crypto`] when the card is not an object or cannot be
/// canonicalized.
pub fn canonical_payloads(card: &Value) -> Result<Vec<String>, Error> {
    let object = card
        .as_object()
        .ok_or_else(|| Error::Crypto("an Agent Card must be a JSON object".to_owned()))?;
    let mut without_signatures = object.clone();
    without_signatures.remove("signatures");
    let plain = Value::Object(without_signatures);

    let mut candidates = Vec::with_capacity(2);
    let normalized = prepare_card_for_signing(card)?;
    let normalized = canonicalize_jcs(&normalized)?;
    candidates.push(normalized);
    let plain = canonicalize_jcs(&plain)?;
    if !candidates.contains(&plain) {
        candidates.push(plain);
    }
    Ok(candidates)
}

/// A cached JWKS document.
#[derive(Debug, Clone)]
struct CachedJwks {
    jwks: Jwks,
    fetched_at: Instant,
}

/// Resolves JWKS documents from configuration, with an in-memory cache.
#[derive(Clone)]
pub struct JwksResolver {
    sources: Arc<Vec<JwksSource>>,
    cache: Arc<RwLock<HashMap<String, CachedJwks>>>,
    cache_ttl: Duration,
    client: reqwest::Client,
}

impl JwksResolver {
    /// Builds a resolver over the configured sources.
    ///
    /// # Panics
    ///
    /// Panics only when the TLS backend cannot be initialized, which is a
    /// programming error and cannot be recovered from.
    #[must_use]
    pub fn new(sources: Vec<JwksSource>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("the JWKS HTTP client must build");
        Self {
            sources: Arc::new(sources),
            cache: Arc::new(RwLock::new(HashMap::new())),
            cache_ttl: Duration::from_secs(300),
            client,
        }
    }

    /// Overrides the cache time to live.
    #[must_use]
    pub fn with_cache_ttl(mut self, ttl: Duration) -> Self {
        self.cache_ttl = ttl;
        self
    }

    /// Returns the configured sources.
    #[must_use]
    pub fn sources(&self) -> &[JwksSource] {
        &self.sources
    }

    /// Loads a JWKS document by source index.
    ///
    /// Inline documents are returned as is; URL documents are cached for
    /// [`Self::with_cache_ttl`] and fetched again after that, so key rotation
    /// is picked up without a restart.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Config`] for an invalid index or document and
    /// [`Error::HttpClient`] when the remote fetch fails.
    pub async fn get_jwks(&self, source_index: usize) -> Result<Jwks, Error> {
        let source = self
            .sources
            .get(source_index)
            .ok_or_else(|| Error::Validation("invalid JWKS source index".to_owned()))?;

        if let Some(inline) = &source.jwks_json {
            return serde_json::from_value(inline.clone())
                .map_err(|err| Error::Validation(format!("invalid inline JWKS: {err}")));
        }

        let url = source.url.as_deref().ok_or_else(|| {
            Error::Validation("JWKS source has neither keys nor a URL".to_owned())
        })?;

        if let Ok(cache) = self.cache.read() {
            if let Some(cached) = cache.get(url) {
                if cached.fetched_at.elapsed() < self.cache_ttl {
                    return Ok(cached.jwks.clone());
                }
            }
        }

        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|err| Error::HttpClient(format!("JWKS fetch failed: {err}")))?;
        if !response.status().is_success() {
            return Err(Error::HttpClient(format!(
                "JWKS fetch returned HTTP {}",
                response.status()
            )));
        }
        let document: Value = response
            .json()
            .await
            .map_err(|err| Error::HttpClient(format!("JWKS parse failed: {err}")))?;
        let jwks: Jwks = serde_json::from_value(document)
            .map_err(|err| Error::Validation(format!("invalid JWKS document: {err}")))?;
        if let Ok(mut cache) = self.cache.write() {
            cache.insert(
                url.to_owned(),
                CachedJwks { jwks: jwks.clone(), fetched_at: Instant::now() },
            );
        }
        Ok(jwks)
    }
}

/// Verifies signed Agent Cards.
#[derive(Clone)]
pub struct AgentCardVerifier {
    resolver: Arc<JwksResolver>,
}

#[async_trait::async_trait]
impl a2a_warden_core::verification::CardVerifier for AgentCardVerifier {
    async fn verify_card(
        &self,
        card: &Value,
        require_verification: bool,
        quarantine_on_failure: bool,
    ) -> a2a_warden_core::verification::CardVerification {
        let outcome =
            AgentCardVerifier::verify_card(self, card, require_verification, quarantine_on_failure)
                .await;
        a2a_warden_core::verification::CardVerification {
            verified: outcome.verified,
            quarantined: outcome.quarantined,
            error: outcome.error,
            kid: outcome.kid,
            issuer: outcome.issuer,
            alg: outcome.alg,
        }
    }
}

impl AgentCardVerifier {
    /// Builds a verifier over a resolver.
    #[must_use]
    pub fn new(resolver: Arc<JwksResolver>) -> Self {
        Self { resolver }
    }

    /// Returns the resolver used by this verifier.
    #[must_use]
    pub fn resolver(&self) -> &JwksResolver {
        &self.resolver
    }

    /// Verifies every signature of a card.
    ///
    /// * `require_verification`: an unsigned card fails verification.
    /// * `quarantine_on_failure`: a failed verification marks the card for
    ///   quarantine.
    ///
    /// The first signature that verifies wins. A card with a `signatures`
    /// member that is present but empty is treated as unsigned.
    pub async fn verify_card(
        &self,
        card: &Value,
        require_verification: bool,
        quarantine_on_failure: bool,
    ) -> VerificationOutcome {
        let signatures = match card.get("signatures") {
            Some(Value::Array(signatures)) => signatures.clone(),
            None => {
                return if require_verification {
                    VerificationOutcome::failure(
                        "Agent Card is unsigned and verification is required",
                        quarantine_on_failure,
                    )
                } else {
                    VerificationOutcome::unsigned_not_required()
                };
            }
            Some(_) => {
                return VerificationOutcome::failure(
                    "the signatures member of the Agent Card is not an array",
                    quarantine_on_failure,
                );
            }
        };

        if signatures.is_empty() {
            return if require_verification {
                VerificationOutcome::failure(
                    "Agent Card is unsigned and verification is required",
                    quarantine_on_failure,
                )
            } else {
                VerificationOutcome::unsigned_not_required()
            };
        }

        let signed_payloads = match canonical_payloads(card) {
            Ok(payloads) => payloads,
            Err(err) => {
                return VerificationOutcome::failure(err.to_string(), quarantine_on_failure);
            }
        };

        let mut last_error = None;
        for signature in &signatures {
            for signed_payload in &signed_payloads {
                match self.verify_signature(signature, signed_payload).await {
                    Ok(outcome) if outcome.verified => return outcome,
                    Ok(outcome) => last_error = outcome.error,
                    Err(err) => last_error = Some(err.to_string()),
                }
            }
        }

        VerificationOutcome::failure(
            last_error.unwrap_or_else(|| "no signature verified the Agent Card".to_owned()),
            quarantine_on_failure,
        )
    }

    /// Verifies a single signature entry.
    async fn verify_signature(
        &self,
        signature: &Value,
        signed_payload: &str,
    ) -> Result<VerificationOutcome, Error> {
        let object = signature.as_object().ok_or_else(|| {
            Error::Crypto("an Agent Card signature must be a JSON object".to_owned())
        })?;

        // A2A signatures use the flattened JWS JSON serialization with a
        // detached payload: `protected` and `signature`, with `header`
        // carrying unprotected parameters.
        let protected = object
            .get("protected")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Crypto("signature has no protected header".to_owned()))?;
        let signature_b64 = object
            .get("signature")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Crypto("signature has no signature member".to_owned()))?;

        let header_bytes = URL_SAFE_NO_PAD
            .decode(protected)
            .map_err(|err| Error::Crypto(format!("invalid protected header encoding: {err}")))?;
        let header: Value = serde_json::from_slice(&header_bytes)
            .map_err(|err| Error::Crypto(format!("invalid protected header: {err}")))?;

        let alg_name = header
            .get("alg")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Crypto("protected header has no alg".to_owned()))?;
        let algorithm = AllowedAlgorithm::parse(alg_name).ok_or_else(|| {
            Error::Crypto(format!("algorithm '{alg_name}' is not in the pinned allowlist"))
        })?;

        let kid = header.get("kid").and_then(Value::as_str).map(str::to_owned);
        let issuer = header.get("iss").and_then(Value::as_str).map(str::to_owned);

        // The signing input is the JWS protected header followed by the
        // base64url encoding of the canonical card (detached payload).
        let signing_input = format!("{protected}.{}", URL_SAFE_NO_PAD.encode(signed_payload));

        for (index, source) in self.resolver.sources().iter().enumerate() {
            if !source.allows(algorithm) {
                debug!(
                    source = index,
                    alg = algorithm.as_str(),
                    "skipping JWKS source that does not pin this algorithm"
                );
                continue;
            }
            if let (Some(expected), Some(actual)) = (&source.issuer, &issuer) {
                if expected != actual {
                    continue;
                }
            }

            let jwks = match self.resolver.get_jwks(index).await {
                Ok(jwks) => jwks,
                Err(err) => {
                    debug!(source = index, error = %err, "JWKS source unavailable");
                    continue;
                }
            };

            for key in &jwks.keys {
                // Only signature keys are usable.
                if key.usage.as_deref().is_some_and(|usage| usage != "sig") {
                    continue;
                }
                if let Some(operations) = &key.key_ops {
                    if !operations.iter().any(|operation| operation == "verify") {
                        continue;
                    }
                }
                if let (Some(expected), Some(actual)) = (&kid, &key.kid) {
                    if expected != actual {
                        continue;
                    }
                }
                if let Some(key_alg) = &key.alg {
                    if key_alg != algorithm.as_str() {
                        continue;
                    }
                }

                match verify_jwk(key, algorithm, signing_input.as_bytes(), signature_b64) {
                    Ok(true) => {
                        return Ok(VerificationOutcome::success(
                            key.kid.clone().or(kid),
                            issuer,
                            algorithm.as_str().to_owned(),
                        ));
                    }
                    Ok(false) => {}
                    Err(err) => {
                        debug!(kid = key.kid.as_deref(), error = %err, "key rejected the signature");
                    }
                }
            }
        }

        Err(Error::Crypto("no configured key verified the signature".to_owned()))
    }
}

/// Verifies a JWS signature with one JWK.
///
/// # Errors
///
/// Returns [`Error::Crypto`] when the key is malformed or its type does not
/// match the algorithm.
fn verify_jwk(
    key: &Jwk,
    algorithm: AllowedAlgorithm,
    message: &[u8],
    signature_b64: &str,
) -> Result<bool, Error> {
    if algorithm == AllowedAlgorithm::Es512 {
        return verify_es512(key, message, signature_b64);
    }
    let decoding_key = decoding_key(key, algorithm)?;
    let jose = algorithm
        .jose()
        .ok_or_else(|| Error::Crypto(format!("no backend for {}", algorithm.as_str())))?;
    verify_signature(signature_b64, message, &decoding_key, jose)
        .map_err(|err| Error::Crypto(format!("signature verification failed: {err}")))
}

/// Verifies an ES512 (ECDSA P-521 with SHA-512) signature.
///
/// `jsonwebtoken` does not expose ES512, so the verification is implemented
/// on the RustCrypto `p521` crate. JWS uses the fixed-size `r || s` encoding,
/// which is exactly what [`p521::ecdsa::Signature::from_slice`] expects.
fn verify_es512(key: &Jwk, message: &[u8], signature_b64: &str) -> Result<bool, Error> {
    use p521::ecdsa::signature::Verifier;

    if key.kty != "EC" || key.crv.as_deref() != Some("P-521") {
        return Err(Error::Crypto("algorithm ES512 requires an EC P-521 key".to_owned()));
    }
    let (x, y) = match (&key.x, &key.y) {
        (Some(x), Some(y)) => (x, y),
        _ => return Err(Error::Crypto("EC key is missing its coordinates".to_owned())),
    };
    let x = decode_b64_url(x)?;
    let y = decode_b64_url(y)?;
    let x = p521::FieldBytes::try_from(x.as_slice())
        .map_err(|_| Error::Crypto("invalid P-521 x coordinate length".to_owned()))?;
    let y = p521::FieldBytes::try_from(y.as_slice())
        .map_err(|_| Error::Crypto("invalid P-521 y coordinate length".to_owned()))?;
    let mut sec1 = Vec::with_capacity(1 + 2 * 66);
    sec1.push(0x04);
    sec1.extend_from_slice(&x);
    sec1.extend_from_slice(&y);
    let verifying_key = p521::ecdsa::VerifyingKey::from_sec1_bytes(&sec1)
        .map_err(|err| Error::Crypto(format!("invalid P-521 key: {err}")))?;
    let signature = decode_b64_url(signature_b64)?;
    let signature = p521::ecdsa::Signature::from_slice(&signature)
        .map_err(|err| Error::Crypto(format!("invalid ES512 signature: {err}")))?;
    verifying_key
        .verify(message, &signature)
        .map(|()| true)
        .map_err(|err| Error::Crypto(format!("signature verification failed: {err}")))
}

/// Builds a `DecodingKey` from the JWK members.
fn decoding_key(key: &Jwk, algorithm: AllowedAlgorithm) -> Result<DecodingKey, Error> {
    match algorithm {
        AllowedAlgorithm::Es256 | AllowedAlgorithm::Es384 => {
            if key.kty != "EC" {
                return Err(Error::Crypto(format!(
                    "algorithm {} requires an EC key, got '{}'",
                    algorithm.as_str(),
                    key.kty
                )));
            }
            let (x, y) = match (&key.x, &key.y) {
                (Some(x), Some(y)) => (x, y),
                _ => return Err(Error::Crypto("EC key is missing its coordinates".to_owned())),
            };
            DecodingKey::from_ec_components(x, y)
                .map_err(|err| Error::Crypto(format!("invalid EC key: {err}")))
        }
        AllowedAlgorithm::EdDsa => {
            if key.kty != "OKP" || key.crv.as_deref() != Some("Ed25519") {
                return Err(Error::Crypto(
                    "algorithm EdDSA requires an Ed25519 OKP key".to_owned(),
                ));
            }
            let x = key
                .x
                .as_deref()
                .ok_or_else(|| Error::Crypto("Ed25519 key is missing its public key".to_owned()))?;
            DecodingKey::from_ed_components(x)
                .map_err(|err| Error::Crypto(format!("invalid Ed25519 key: {err}")))
        }
        AllowedAlgorithm::Rs256 => {
            if key.kty != "RSA" {
                return Err(Error::Crypto(format!(
                    "algorithm RS256 requires an RSA key, got '{}'",
                    key.kty
                )));
            }
            let (n, e) = match (&key.n, &key.e) {
                (Some(n), Some(e)) => (n, e),
                _ => {
                    return Err(Error::Crypto(
                        "RSA key is missing its modulus or exponent".to_owned(),
                    ));
                }
            };
            DecodingKey::from_rsa_components(n, e)
                .map_err(|err| Error::Crypto(format!("invalid RSA key: {err}")))
        }
        AllowedAlgorithm::Es512 => {
            Err(Error::Crypto("ES512 is verified with the dedicated P-521 backend".to_owned()))
        }
    }
}

/// Decodes a URL-safe base64 value without padding.
fn decode_b64_url(value: &str) -> Result<Vec<u8>, Error> {
    URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|err| Error::Crypto(format!("base64url decode failed: {err}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn jcs_orders_object_members_and_normalizes_numbers() {
        let value = json!({
            "b": 1,
            "a": 2,
            "c": { "z": true, "y": null },
            "n": 1.0,
        });
        let canonical = canonicalize_jcs(&value).expect("canonical");
        assert_eq!(canonical, r#"{"a":2,"b":1,"c":{"y":null,"z":true},"n":1}"#);
    }

    #[test]
    fn jcs_escapes_control_characters_but_not_line_separators() {
        let value = json!({ "text": "line\u{2028}break\u{0007}" });
        let canonical = canonicalize_jcs(&value).expect("canonical");
        // RFC 8785 escapes the C0 control range only; U+2028 is valid JSON
        // text and is emitted as literal UTF-8 (the SDKs do the same).
        assert!(canonical.contains("\\u0007"));
        assert!(canonical.contains('\u{2028}'));
    }

    #[test]
    fn the_signatures_member_is_removed_before_signing() {
        let card = json!({ "name": "test", "signatures": [{ "protected": "x" }] });
        let prepared = prepare_card_for_signing(&card).expect("card");
        assert!(prepared.get("signatures").is_none());
        assert_eq!(prepared["name"], "test");
    }

    #[test]
    fn canonicalization_preserves_proto3_field_presence() {
        // A field that the producer did not set is absent from the JSON and
        // therefore from the canonical bytes; an explicitly null field stays.
        let absent = json!({ "name": "agent" });
        let present = json!({ "name": "agent", "description": null });
        assert_ne!(
            canonicalize_jcs(&absent).expect("absent"),
            canonicalize_jcs(&present).expect("present")
        );
    }

    #[test]
    fn algorithms_are_pinned_to_the_documented_set() {
        assert_eq!(AllowedAlgorithm::parse("ES256"), Some(AllowedAlgorithm::Es256));
        assert_eq!(AllowedAlgorithm::parse("ES384"), Some(AllowedAlgorithm::Es384));
        assert_eq!(AllowedAlgorithm::parse("ES512"), Some(AllowedAlgorithm::Es512));
        assert_eq!(AllowedAlgorithm::parse("EdDSA"), Some(AllowedAlgorithm::EdDsa));
        assert_eq!(AllowedAlgorithm::parse("RS256"), Some(AllowedAlgorithm::Rs256));
        assert_eq!(AllowedAlgorithm::parse("HS256"), None);
        assert_eq!(AllowedAlgorithm::parse("none"), None);
    }

    #[test]
    fn a_source_without_algorithms_accepts_the_default_set() {
        let source =
            JwksSource { issuer: None, jwks_json: None, url: None, allowed_algorithms: Vec::new() };
        assert!(source.allows(AllowedAlgorithm::Es256));
        assert!(source.allows(AllowedAlgorithm::Rs256));
    }

    #[test]
    fn a_source_with_pinned_algorithms_rejects_the_others() {
        let source = JwksSource {
            issuer: None,
            jwks_json: None,
            url: None,
            allowed_algorithms: vec![AllowedAlgorithm::EdDsa],
        };
        assert!(source.allows(AllowedAlgorithm::EdDsa));
        assert!(!source.allows(AllowedAlgorithm::Es256));
    }

    #[tokio::test]
    async fn an_unsigned_card_is_rejected_when_verification_is_required() {
        let resolver = Arc::new(JwksResolver::new(Vec::new()));
        let verifier = AgentCardVerifier::new(resolver);
        let outcome = verifier.verify_card(&json!({ "name": "agent" }), true, true).await;
        assert!(!outcome.verified);
        assert!(outcome.quarantined);
        assert!(outcome.error.expect("reason").contains("unsigned"));
    }

    #[tokio::test]
    async fn an_unsigned_card_is_accepted_when_verification_is_not_required() {
        let resolver = Arc::new(JwksResolver::new(Vec::new()));
        let verifier = AgentCardVerifier::new(resolver);
        let outcome = verifier.verify_card(&json!({ "name": "agent" }), false, true).await;
        assert!(!outcome.verified);
        assert!(!outcome.quarantined);
        assert!(outcome.error.is_none());
    }

    #[tokio::test]
    async fn a_signature_with_an_unknown_algorithm_is_rejected() {
        let resolver = Arc::new(JwksResolver::new(Vec::new()));
        let verifier = AgentCardVerifier::new(resolver);
        let protected = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","kid":"k1"}"#);
        let card = json!({
            "name": "agent",
            "signatures": [{ "protected": protected, "signature": "AAAA" }],
        });
        let outcome = verifier.verify_card(&card, true, true).await;
        assert!(!outcome.verified);
        assert!(outcome.quarantined);
        assert!(outcome.error.expect("reason").contains("pinned allowlist"));
    }
}
