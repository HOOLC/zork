//! Live credentials scoped to explicitly configured relay origins.
use std::{collections::HashMap, sync::Arc};
use iroh::{Endpoint, RelayConfig};
use crate::{NetOptions, error::NetError};

/// An endpoint's relay admission state. It owns no membership or account lifecycle.
/// Debug deliberately omits access tokens.
pub struct RelayAccess {
    endpoint: Endpoint,
    configs: Vec<RelayConfig>,
    offline: bool,
    current: tokio::sync::Mutex<HashMap<String, Option<String>>>,
}
impl std::fmt::Debug for RelayAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelayAccess").finish_non_exhaustive()
    }
}
impl RelayAccess {
    /// Retains public relay configurations so logout and later login are reversible.
    pub fn new(endpoint: Endpoint, options: &NetOptions) -> Result<Arc<Self>, NetError> {
        let configs = {
            options.relay_urls.iter().map(|raw| {
                let url = raw.parse().map_err(|error| NetError::Endpoint(format!("relay URL: {error}")))?;
                Ok(match options.relay_quic_port {
                    Some(port) => RelayConfig::new(url, Some(iroh_relay::RelayQuicConfig::new(port))),
                    None => RelayConfig::from(url),
                })
            }).collect::<Result<Vec<_>, NetError>>()?
        };
        Ok(Arc::new(Self { endpoint, configs, offline: options.offline, current: Default::default() }))
    }

    /// Updates only configured relays of this canonical origin. Removal closes
    /// old relay WebSockets; QAD-only servers and established direct paths remain.
    pub async fn set(&self, origin: &str, token: Option<&str>) -> Result<(), NetError> {
        let issuer = url::Url::parse(origin).map_err(|error| NetError::Endpoint(error.to_string()))?;
        let canonical = issuer.origin().ascii_serialization();
        if origin.trim_end_matches('/') != canonical || !issuer.username().is_empty() || issuer.password().is_some() {
            return Err(NetError::Endpoint("relay access requires a canonical origin".into()));
        }
        if token.is_some_and(|token| token.is_empty() || token.len() > 8192 || !token.bytes().all(|b| b.is_ascii_graphic())) {
            return Err(NetError::Endpoint("invalid relay access token".into()));
        }
        if self.endpoint.is_closed() { return Ok(()); }
        let mut current = self.current.lock().await;
        if current.get(&canonical).is_some_and(|value| value.as_deref() == token) { return Ok(()); }
        for config in self.configs.iter().filter(|config| !self.offline && config.url.origin().ascii_serialization() == canonical) {
            self.endpoint.remove_relay(&config.url).await;
            if let Some(token) = token {
                self.endpoint.insert_relay(config.url.clone(), Arc::new(config.clone().with_auth_token(token))).await;
            }
        }
        // Credentials for unrelated origins must never be retained or forwarded.
        if self.configs.iter().any(|config| config.url.origin().ascii_serialization() == canonical) {
            current.insert(canonical, token.map(str::to_owned));
        }
        Ok(())
    }

    /// Applies already observed access to a newly created endpoint. Its own
    /// configured origins still constrain every credential.
    pub async fn inherit(&self, source: &Self) -> Result<(), NetError> {
        let current = source.current.lock().await.clone();
        for (origin, token) in current { self.set(&origin, token.as_deref()).await?; }
        Ok(())
    }
}
