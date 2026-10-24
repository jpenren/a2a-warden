//! HTTP delivery of A2A push notifications.
//!
//! The service validates the destination against its [`PushPolicy`] before
//! calling the notifier and again immediately before delivery, so this client
//! only has to apply the configured credentials and post the payload. HTTP
//! redirects are disabled on purpose: following one would bypass the address
//! checks performed for the original URL.
//!
//! [`PushPolicy`]: a2a_warden_core::push::PushPolicy

use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde_json::Value;
use url::Url;

use a2a_warden_core::error::Error;
use a2a_warden_core::protocol::PushNotificationAuthentication;
use a2a_warden_core::push::PushNotifier;

/// Header carrying the validation token of a push configuration.
pub const NOTIFICATION_TOKEN_HEADER: &str = "X-A2A-Notification-Token";

/// [`PushNotifier`] implementation backed by `reqwest` with rustls.
#[derive(Clone)]
pub struct HttpPushNotifier {
    http: reqwest::Client,
}

impl HttpPushNotifier {
    /// Builds a client with the configured delivery timeout.
    ///
    /// Redirects are disabled so a callback cannot bounce the warden to a
    /// destination that the SSRF policy already rejected.
    pub fn new(timeout: Duration) -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("a2a-warden/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|err| Error::HttpClient(err.to_string()))?;
        Ok(Self { http })
    }
}

#[async_trait]
impl PushNotifier for HttpPushNotifier {
    async fn deliver(
        &self,
        url: &Url,
        token: Option<&str>,
        authentication: Option<&PushNotificationAuthentication>,
        payload: &Value,
    ) -> Result<(), Error> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if let Some(token) = token {
            let value = HeaderValue::from_str(token).map_err(|err| {
                Error::Validation(format!("invalid push notification token: {err}"))
            })?;
            headers.insert(NOTIFICATION_TOKEN_HEADER, value);
        }
        if let Some(authentication) = authentication {
            if let (Some(scheme), Some(credentials)) =
                (authentication.schemes.first(), authentication.credentials.as_deref())
            {
                let value =
                    HeaderValue::from_str(&format!("{scheme} {credentials}")).map_err(|err| {
                        Error::Validation(format!("invalid push authorization: {err}"))
                    })?;
                headers.insert(AUTHORIZATION, value);
            }
        }

        let response =
            self.http.post(url.clone()).headers(headers).json(payload).send().await.map_err(
                |err| Error::HttpClient(format!("push delivery to {url} failed: {err}")),
            )?;

        if !response.status().is_success() {
            return Err(Error::HttpClient(format!(
                "push delivery to {url} returned HTTP {}",
                response.status()
            )));
        }
        Ok(())
    }
}
