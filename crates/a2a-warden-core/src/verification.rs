//! Agent Card verification contract.
//!
//! The domain crate cannot depend on the cryptography crate (the cryptography
//! crate implements the JWS/JCS details and depends on the domain), so the
//! service depends on this small trait and the binary wires the concrete
//! verifier at startup. This keeps the dependency direction acyclic while
//! making verification mandatory for every registration path.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::AgentCardVerificationConfig;

/// Result of verifying one Agent Card.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardVerification {
    /// Whether a signature verified the card.
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

impl CardVerification {
    /// A card that verification could not accept.
    #[must_use]
    pub fn failed(reason: impl Into<String>, quarantined: bool) -> Self {
        Self { verified: false, quarantined, error: Some(reason.into()), ..Self::default() }
    }
}

/// Verifies the JWS signatures of an Agent Card.
#[async_trait]
pub trait CardVerifier: Send + Sync {
    /// Verifies a card.
    ///
    /// * `require_verification`: an unsigned card is a failure.
    /// * `quarantine_on_failure`: mark a failed card as quarantined.
    async fn verify_card(
        &self,
        card: &Value,
        require_verification: bool,
        quarantine_on_failure: bool,
    ) -> CardVerification;
}

/// Effective verification policy of the running warden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationPolicy {
    /// Whether every registration must be verified.
    pub enabled: bool,
    /// Whether a failure quarantines the agent.
    pub quarantine_unverified: bool,
}

impl Default for VerificationPolicy {
    fn default() -> Self {
        Self { enabled: false, quarantine_unverified: true }
    }
}

impl VerificationPolicy {
    /// Derives the policy from configuration.
    ///
    /// Verification is implicitly enabled as soon as at least one JWKS source
    /// is configured: pinning a key set expresses the intent to verify, and a
    /// silent opt out would defeat the purpose of the pin.
    #[must_use]
    pub fn from_config(config: &AgentCardVerificationConfig) -> Self {
        Self {
            enabled: config.enabled || !config.jwks.is_empty(),
            quarantine_unverified: config.quarantine_unverified,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AgentCardVerificationConfig, JwksConfig};

    #[test]
    fn the_default_policy_does_not_verify() {
        let policy = VerificationPolicy::from_config(&AgentCardVerificationConfig::default());
        assert!(!policy.enabled);
        assert!(policy.quarantine_unverified);
    }

    #[test]
    fn configuring_a_jwks_source_enables_verification() {
        let config = AgentCardVerificationConfig {
            enabled: false,
            quarantine_unverified: true,
            jwks: vec![JwksConfig::default()],
        };
        assert!(VerificationPolicy::from_config(&config).enabled);
    }

    #[test]
    fn the_quarantine_flag_is_honored() {
        let config = AgentCardVerificationConfig {
            enabled: true,
            quarantine_unverified: false,
            jwks: Vec::new(),
        };
        let policy = VerificationPolicy::from_config(&config);
        assert!(policy.enabled);
        assert!(!policy.quarantine_unverified);
    }
}
