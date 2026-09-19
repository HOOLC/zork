//! Invitation transport with the same account lifecycle as the data endpoint.
use crate::relay_account::{Access, Account, RelayAccountTask};
use anyhow::{ensure, Context, Result};
use std::{ops::Deref, path::Path, sync::Arc, time::Duration};
use zork_config::MeshConfig;
use zork_mesh::enrollment::{ticket::Ticket, Invitation, InviteKind};

pub struct Enrollment {
    _account: Option<RelayAccountTask>,
    transport: Arc<zork_mesh::enrollment::Enrollment>,
}
impl Deref for Enrollment {
    type Target = zork_mesh::enrollment::Enrollment;
    fn deref(&self) -> &Self::Target {
        &self.transport
    }
}
impl Enrollment {
    pub async fn bind(root: &Path, config: &MeshConfig) -> Result<Self> {
        Self::bind_at(root, root, config).await
    }
    async fn bind_at(root: &Path, key_root: &Path, config: &MeshConfig) -> Result<Self> {
        let mut effective = config.clone();
        if !effective.offline {
            zork_config::services::ServicesConfig::load_for_data_root(
                &zork_config::relay_account::resolve_root(root)?,
            )?
            .apply_defaults(&mut effective)?;
        }
        let config = &effective;
        let transport = Arc::new(zork_mesh::enrollment::Enrollment::bind(key_root, config).await?);
        let account = if config.offline {
            None
        } else {
            zork_config::relay_account::control_origin(config.relay_urls.as_deref())
                .map(|origin| Account::new(root, &origin))
                .transpose()?
        };
        let task = if let Some(account) = account {
            let origin = account.origin().to_owned();
            let access = account.cached_access()?;
            transport
                .set_relay_access(&origin, access.as_ref().map(Access::token))
                .await?;
            let endpoint = transport.clone();
            Some(account.maintain(move |access| {
                let endpoint = endpoint.clone();
                let origin = origin.clone();
                async move {
                    endpoint
                        .set_relay_access(&origin, access.as_ref().map(Access::token))
                        .await
                }
            })?)
        } else {
            None
        };
        Ok(Self {
            _account: task,
            transport,
        })
    }
}

/// Account storage stays in the profile root, separate from bootstrap identity.
pub async fn resolve_invitation(
    root: &Path,
    value: &str,
    expected: InviteKind,
) -> Result<Invitation> {
    let invite = if Ticket::is_short(value) {
        let ticket = Ticket::decode(value)?;
        ensure!(ticket.kind == expected, "invite_kind_mismatch");
        let mut config = ticket.network_config()?;
        if !config.offline {
            let services = zork_config::services::ServicesConfig::load_for_data_root(
                &zork_config::relay_account::resolve_root(root)?,
            )?;
            config.relay_urls = services.relay_urls;
            if config.discovery_url.is_none() {
                config.discovery_url = services.discovery_url;
            }
        }
        let transport = tokio::time::timeout(
            Duration::from_secs(15),
            Enrollment::bind_at(root, &root.join("invite-bootstrap"), &config),
        )
        .await
        .context("bootstrap_bind_timeout")??;
        let result = ticket.resolve_using(&transport).await;
        let _ = tokio::time::timeout(Duration::from_secs(3), transport.close()).await;
        result?
    } else {
        Invitation::decode(value)?
    };
    ensure!(invite.kind == expected, "invite_kind_mismatch");
    ensure!(
        invite.channel == zork_config::channel::current()?,
        "邀请属于另一环境，请使用对应的 Zork 或 Zork Dev"
    );
    Ok(invite)
}
