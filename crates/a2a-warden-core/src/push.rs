//! Push notification delivery policy (SSRF protection).
//!
//! A push notification makes the warden issue an HTTP request to a URL chosen
//! by a client, which is a server-side request forgery sink. The policy is
//! deny by default:
//!
//! * push notifications must be enabled;
//! * the URL must use `http` or `https`, must not carry user information, and
//!   must name a host that matches the configured allow list;
//! * unless explicitly allowed, the resolved addresses must be public (no
//!   loopback, private, link-local, multicast, or unspecified address).
//!
//! The URL is validated twice: when the configuration is registered, and
//! again immediately before delivery (DNS rebinding and long-lived
//! configurations make a single check insufficient).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use async_trait::async_trait;
use serde_json::Value;
use url::Url;

use crate::error::Error;
use crate::protocol::PushNotificationAuthentication;

/// A destination policy for push notifications.
#[derive(Debug, Clone, Default)]
pub struct PushPolicy {
    /// Whether push notifications are enabled at all.
    pub enabled: bool,
    /// Hosts that may receive notifications (`example.com`, `*.example.com`,
    /// or `*`).
    pub allowed_hosts: Vec<String>,
    /// Whether loopback and private addresses are permitted (development).
    pub allow_private_addresses: bool,
}

impl PushPolicy {
    /// Builds a policy from configuration.
    #[must_use]
    pub fn new(enabled: bool, allowed_hosts: Vec<String>, allow_private_addresses: bool) -> Self {
        Self { enabled, allowed_hosts, allow_private_addresses }
    }

    /// Validates the shape of a callback URL without resolving its host.
    ///
    /// # Errors
    /// Returns a [`Error::Forbidden`] when push notifications are disabled,
    /// the URL is malformed, or the host is not on the allow list.
    pub fn validate_shape(&self, raw: &str) -> Result<Url, Error> {
        if !self.enabled {
            return Err(Error::Forbidden("push notifications are disabled".to_owned()));
        }
        let url = Url::parse(raw)
            .map_err(|err| Error::Validation(format!("invalid push URL '{raw}': {err}")))?;
        if url.scheme() != "http" && url.scheme() != "https" {
            return Err(Error::Forbidden(format!(
                "push URL scheme '{}' is not allowed",
                url.scheme()
            )));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(Error::Forbidden("push URL must not carry user information".to_owned()));
        }
        let host =
            url.host_str().ok_or_else(|| Error::Validation("push URL has no host".to_owned()))?;
        if !self.host_is_allowed(host) {
            return Err(Error::Forbidden(format!(
                "push URL host '{host}' is not on the allow list"
            )));
        }
        Ok(url)
    }

    /// Validates a callback URL and every address its host resolves to.
    ///
    /// # Errors
    /// Returns a [`Error::Forbidden`] when the URL or a resolved address is
    /// not allowed.
    pub async fn validate(&self, raw: &str) -> Result<Url, Error> {
        let url = self.validate_shape(raw)?;
        let host = url.host_str().unwrap_or_default().to_owned();
        let port = url.port_or_known_default().unwrap_or(80);

        if let Ok(ip) = host.parse::<IpAddr>() {
            self.check_ip(ip)?;
            return Ok(url);
        }

        let addresses = tokio::net::lookup_host((host.as_str(), port))
            .await
            .map_err(|err| {
                Error::Validation(format!("cannot resolve push URL host '{host}': {err}"))
            })?
            .collect::<Vec<SocketAddr>>();
        if addresses.is_empty() {
            return Err(Error::Validation(format!(
                "push URL host '{host}' resolved to no address"
            )));
        }
        for address in addresses {
            self.check_ip(address.ip())?;
        }
        Ok(url)
    }

    /// Returns `true` when the host matches the allow list.
    ///
    /// `*` allows every host. A leading `*.` matches the domain itself and any
    /// subdomain. Matching is case insensitive and ignores a trailing dot.
    #[must_use]
    pub fn host_is_allowed(&self, host: &str) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        self.allowed_hosts.iter().any(|entry| {
            let entry = entry.trim().trim_end_matches('.').to_ascii_lowercase();
            if entry == "*" || entry == host {
                return true;
            }
            entry
                .strip_prefix("*.")
                .is_some_and(|suffix| host == suffix || host.ends_with(&format!(".{suffix}")))
        })
    }

    /// Rejects addresses that could reach the local network.
    fn check_ip(&self, ip: IpAddr) -> Result<(), Error> {
        if self.allow_private_addresses {
            return Ok(());
        }
        let denied = match ip {
            IpAddr::V4(ip) => is_denied_v4(ip),
            IpAddr::V6(ip) => is_denied_v6(ip) || ip.to_ipv4_mapped().is_some_and(is_denied_v4),
        };
        if denied {
            return Err(Error::Forbidden(format!(
                "push URL resolves to a non-public address ({ip})"
            )));
        }
        Ok(())
    }
}

/// Whether an IPv4 address belongs to a range the warden must not reach.
fn is_denied_v4(ip: Ipv4Addr) -> bool {
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        // Carrier grade NAT and the "this network" range.
        || ip.octets()[0] == 0
        || (ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1]))
}

/// Whether an IPv6 address belongs to a range the warden must not reach.
fn is_denied_v6(ip: Ipv6Addr) -> bool {
    ip.is_loopback()
        || ip.is_multicast()
        || ip.is_unspecified()
        // Link-local fe80::/10.
        || (ip.segments()[0] & 0xffc0) == 0xfe80
        // Unique local fc00::/7.
        || (ip.segments()[0] & 0xfe00) == 0xfc00
}

/// Delivers a push notification to a configured endpoint.
///
/// Implementations live outside the core crate (they need an HTTP client);
/// the service depends only on this trait.
#[async_trait]
pub trait PushNotifier: Send + Sync {
    /// Posts a JSON payload to the callback, applying the token and
    /// authentication headers of the configuration.
    async fn deliver(
        &self,
        url: &Url,
        token: Option<&str>,
        authentication: Option<&PushNotificationAuthentication>,
        payload: &Value,
    ) -> Result<(), Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> PushPolicy {
        PushPolicy::new(true, vec!["example.com".to_owned(), "*.trusted.test".to_owned()], false)
    }

    #[test]
    fn disabled_push_rejects_every_url() {
        let policy = PushPolicy::default();
        assert!(policy.validate_shape("https://example.com/hook").is_err());
    }

    #[test]
    fn the_allow_list_matches_exact_hosts_and_subdomains() {
        let policy = policy();
        assert!(policy.host_is_allowed("example.com"));
        assert!(policy.host_is_allowed("EXAMPLE.com."));
        assert!(policy.host_is_allowed("a.trusted.test"));
        assert!(policy.host_is_allowed("trusted.test"));
        assert!(!policy.host_is_allowed("evil.test"));
        assert!(!policy.host_is_allowed("example.com.evil.test"));
    }

    #[test]
    fn a_wildcard_allows_every_host() {
        let policy = PushPolicy::new(true, vec!["*".to_owned()], false);
        assert!(policy.host_is_allowed("anything.test"));
    }

    #[test]
    fn the_scheme_and_user_information_are_validated() {
        let policy = policy();
        assert!(policy.validate_shape("ftp://example.com/hook").is_err());
        assert!(policy.validate_shape("https://user:secret@example.com/hook").is_err());
        assert!(policy.validate_shape("https://example.com/hook").is_ok());
    }

    #[test]
    fn a_host_outside_the_allow_list_is_rejected() {
        let policy = policy();
        let error =
            policy.validate_shape("https://evil.test/hook").expect_err("host must be denied");
        assert!(error.to_string().contains("allow list"), "{error}");
    }

    #[tokio::test]
    async fn private_and_loopback_addresses_are_rejected() {
        let policy = PushPolicy::new(true, vec!["*".to_owned()], false);
        for url in [
            "http://127.0.0.1/hook",
            "http://10.0.0.1/hook",
            "http://192.168.1.1/hook",
            "http://169.254.1.1/hook",
            "http://[::1]/hook",
            "http://[fc00::1]/hook",
        ] {
            assert!(policy.validate(url).await.is_err(), "{url} must be denied");
        }
    }

    #[tokio::test]
    async fn private_addresses_can_be_allowed_explicitly() {
        let policy = PushPolicy::new(true, vec!["*".to_owned()], true);
        assert!(policy.validate("http://127.0.0.1:9000/hook").await.is_ok());
    }

    #[tokio::test]
    async fn a_public_address_is_accepted() {
        let policy = PushPolicy::new(true, vec!["*".to_owned()], false);
        assert!(policy.validate("http://93.184.216.34/hook").await.is_ok());
    }
}
