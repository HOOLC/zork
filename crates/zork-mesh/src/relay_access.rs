//! Credential-only updates shared by data and invitation endpoints.
use anyhow::{Context, Result};
use iroh::{Endpoint, RelayConfig, RelayUrl};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Debug, Default)]
pub(crate) struct RelayAccess(tokio::sync::Mutex<BTreeMap<RelayUrl, Arc<RelayConfig>>>);

impl RelayAccess {
    pub async fn apply(
        &self,
        endpoint: &Endpoint,
        configured: &[String],
        origin: &str,
        token: Option<&str>,
    ) -> Result<()> {
        let origin = zork_config::relay_account::canonical_origin(origin)?;
        let mut cached = self.0.lock().await;
        for raw in configured {
            if zork_config::relay_account::canonical_origin(raw)
                .ok()
                .as_deref()
                != Some(origin.as_str())
            {
                continue;
            }
            let url: RelayUrl = raw.parse().context("invalid relay URL")?;
            if let Some(old) = endpoint.remove_relay(&url).await {
                let mut base = old.as_ref().clone();
                base.auth_token = None;
                cached.insert(url.clone(), Arc::new(base));
            }
            if let Some(token) = token {
                if let Some(base) = cached.get(&url) {
                    endpoint
                        .insert_relay(url, Arc::new(base.as_ref().clone().with_auth_token(token)))
                        .await;
                }
            }
        }
        Ok(())
    }
}
