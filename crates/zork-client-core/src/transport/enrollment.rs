//! Bootstrap uses profile account storage while keeping its identity separate.
use crate::relay_account::{Access, Account};
use anyhow::{ensure, Context, Result};
use std::{path::Path, sync::Arc, time::Duration};
use zork_mesh::{
    enrollment::{ticket::Ticket, Enrollment, Invitation, InviteKind},
    node::MeshNode,
};

pub async fn resolve_invitation(
    root: &Path,
    value: &str,
    expected: InviteKind,
    owner: Option<&MeshNode>,
) -> Result<Invitation> {
    if let Some(node) = owner {
        return zork_mesh::enrollment::ticket::resolve_on_node(
            &root.join("invite-bootstrap"),
            value,
            expected,
            node,
        )
        .await;
    }
    let invite = if Ticket::is_short(value) {
        let ticket = Ticket::decode(value)?;
        ensure!(ticket.kind == expected, "invite_kind_mismatch");
        let mut config = ticket.network_config()?;
        zork_config::services::ServicesConfig::load_from_install()?.apply_defaults(&mut config)?;
        let transport = Arc::new(
            tokio::time::timeout(
                Duration::from_secs(15),
                Enrollment::bind(&root.join("invite-bootstrap"), &config),
            )
            .await
            .context("bootstrap_bind_timeout")??,
        );
        let account = if config.offline {
            None
        } else {
            zork_config::relay_account::control_origin(config.relay_urls.as_deref())
                .map(|origin| Account::new(root, &origin))
                .transpose()?
        };
        let _account = if let Some(account) = account {
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
