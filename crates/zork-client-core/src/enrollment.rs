//! Client invitation bootstrap; platform hosts only scan/paste and display state.
use super::*;
use crate::transport::Enrollment;
use zork_config::membership::{MeshDevice, MeshGroup};
use zork_mesh::enrollment::{Invitation, InviteKind};

#[derive(Clone, Serialize, Deserialize)]
struct Pending {
    invitation: Invitation,
    name: String,
    network: Network,
}
#[derive(Clone, Serialize, Deserialize)]
struct Input {
    id: String,
    ticket: String,
    name: String,
}
fn input_snapshot(input: &Input) -> Value {
    json!({"name":"连接设备", "status":"resolving", "id":input.id})
}
impl Client {
    pub(crate) async fn next_invitation(&mut self) -> Result<Value> {
        self.resume().await?;
        Ok(self.invitation.source.read().as_ref().clone())
    }
    pub(crate) async fn poll_invitation(&mut self) -> Result<Value> {
        self.next_invitation().await
    }
    pub(crate) fn watch_invitation(&mut self) -> Result<()> {
        if self
            .invitation_job
            .as_ref()
            .is_some_and(|task| !task.is_finished())
        {
            return Ok(());
        }
        let pending = self.pending_invitation()?;
        let input: Option<Input> = self.store.get("device", "invitation_input")?;
        if pending.is_none() && input.is_none() {
            return Ok(());
        }
        let current = self.invitation.source.read();
        if current["done"] == true
            && (pending
                .as_ref()
                .is_some_and(|pending| current["invitation"]["id"] == pending.invitation.id)
                || input
                    .as_ref()
                    .is_some_and(|input| current["invitation"]["id"] == input.id))
        {
            return Ok(());
        }
        let generation = self.invitation.replace(public_snapshot(&self.store, true)?);
        let (root, store, node, view) = (
            self.root.clone(),
            self.store.clone(),
            self.node()?,
            self.invitation.clone(),
        );
        let account = self.account.clone();
        self.invitation_job = Some(zork_notify::Task(tokio::spawn(async move {
            let pending = match pending {
                Some(pending) => pending,
                None => {
                    match resolve_input(&root, &store, input.unwrap(), &view, generation, &account)
                        .await
                    {
                        Ok(pending) => pending,
                        Err(_) => return,
                    }
                }
            };
            run_invitation(root, store, node, pending, view, generation, account).await;
        })));
        Ok(())
    }
    fn pending_invitation(&self) -> Result<Option<Pending>> {
        self.store.get("device", "invitation")
    }
    pub(crate) fn enrollment_network(&self) -> Result<Network> {
        if let Some(pending) = self.pending_invitation()? {
            return Ok(pending.network);
        }
        if self.store.nodes()?.is_empty() {
            if let Some(input) = self.store.get::<Input>("device", "invitation_input")? {
                let config =
                    zork_mesh::enrollment::ticket::Ticket::decode(&input.ticket)?.network_config();
                return Ok(Network {
                    direct_only: config.offline,
                    relay_urls: config.relay_urls,
                    discovery_url: config.discovery_url,
                });
            }
        }
        Ok(self.store.get("device", "network")?.unwrap_or_default())
    }
    pub(crate) fn invitation_snapshot(&self) -> Result<Value> {
        Ok(public_snapshot(&self.store, self.runtime.is_some())?["invitation"].clone())
    }
    pub(crate) async fn begin_invitation(&mut self, ticket: &str, name: &str) -> Result<Value> {
        let name = name.trim();
        ensure!(
            !name.is_empty() && name.len() <= 128 && !name.chars().any(char::is_control),
            "手机名称无效"
        );
        let ticket = ticket.trim();
        if zork_mesh::enrollment::ticket::Ticket::is_short(ticket) {
            let decoded = zork_mesh::enrollment::ticket::Ticket::decode(ticket)
                .context("无法识别连接邀请")?;
            ensure!(
                decoded.kind == InviteKind::Client,
                "这是执行设备邀请，请在桌面选择连接手机"
            );
            let existing: Option<Input> = self.store.get("device", "invitation_input")?;
            if let Some(existing) = existing {
                ensure!(existing.ticket == ticket, "请先取消当前连接邀请");
            } else {
                ensure!(self.pending_invitation()?.is_none(), "请先取消当前连接邀请");
                self.pause().await?;
                self.store.put(
                    "device",
                    "invitation_input",
                    &Input {
                        id: ulid::Ulid::new().to_string(),
                        ticket: ticket.into(),
                        name: name.into(),
                    },
                )?;
            }
            self.resume().await?;
            return self.snapshot();
        }
        let invitation =
            crate::transport::resolve_invitation(&self.root, ticket.trim(), InviteKind::Client)
                .await
                .context("无法识别连接邀请，请扫描 Zork 的手机连接二维码")?;
        ensure!(
            invitation.kind == InviteKind::Client,
            "这是执行设备邀请，请在桌面选择连接手机"
        );
        // The server handles expiry and same-identity completed-receipt recovery.
        if !zork_mesh::enrollment::ticket::Ticket::is_short(ticket.trim()) {
            ensure!(
                invitation.expires_at > zork_mesh::enrollment::now(),
                "邀请已过期，请在桌面重新生成"
            );
        }
        let name = name.trim();
        ensure!(
            !name.is_empty() && name.len() <= 128 && !name.chars().any(char::is_control),
            "手机名称无效"
        );
        if let Some(pending) = self.pending_invitation()? {
            ensure!(
                pending.invitation.id == invitation.id
                    && pending.invitation.secret == invitation.secret,
                "请先取消当前连接邀请"
            );
            return self.snapshot();
        }
        let existing = self.store.nodes()?;
        let network = if existing.is_empty() {
            Network {
                direct_only: invitation.offline,
                relay_urls: invitation.relay_urls.clone(),
                discovery_url: invitation.discovery_url.clone(),
            }
        } else {
            self.store.get("device", "network")?.unwrap_or_default()
        };
        self.config(&network)?;
        self.pause().await?;
        self.store.put(
            "device",
            "invitation",
            &Pending {
                invitation,
                name: name.into(),
                network,
            },
        )?;
        self.resume().await?;
        self.snapshot()
    }
    pub(crate) async fn cancel_invitation(&mut self) -> Result<Value> {
        let pending = self.pending_invitation()?;
        self.invitation_job.take();
        // Remove the expected identity before awaiting IO; an in-flight result
        // cannot commit a cancelled or replaced invitation afterwards.
        self.store.forget_invitation()?;
        self.invitation
            .replace(public_snapshot(&self.store, self.runtime.is_some())?);
        if let Some(pending) = pending {
            if !self
                .store
                .nodes()?
                .iter()
                .any(|n| n.id == pending.invitation.device.origin)
            {
                if let Ok(node) = self.node() {
                    node.untrust(&pending.invitation.device.origin).await?;
                }
            }
            self.pause().await?;
            self.resume().await?;
        }
        self.snapshot()
    }
}

async fn connect_invitation(
    root: &std::path::Path,
    store: &ClientStore,
    node: &zork_mesh::node::MeshNode,
    pending: &Pending,
) -> Result<Option<zork_mesh::feed::Watch>> {
    let invitation = &pending.invitation;
    // Still attempt the same receipt after expiry: the Station permits recovery
    // only if this exact identity already completed enrollment and remains granted.
    let identity = node.identity().await?;
    ensure!(identity != invitation.device.origin, "不能连接到自己");
    let device = MeshDevice {
        origin: identity.clone(),
        name: pending.name.clone(),
        addr: None,
    };
    device.validate()?;
    let config = MeshConfig {
        offline: invitation.offline,
        relay_urls: invitation.relay_urls.clone(),
        discovery_url: invitation.discovery_url.clone(),
        bind: Some("0.0.0.0:0".into()),
        ..Default::default()
    };
    let enrollment = Enrollment::bind(root, &config).await?;
    let begun = enrollment
        .exchange(
            invitation,
            &json!({"id":invitation.id,"secret":invitation.secret,"device":device}),
        )
        .await;
    enrollment.close().await;
    let begun = begun?;
    ensure!(
        begun["origin"] == invitation.device.origin,
        "invite_authority_mismatch"
    );
    node.trust(
        &invitation.device.origin,
        &invitation.device.name,
        invitation.device.addr.as_deref(),
    )
    .await?;
    node.remember_peer_address(&invitation.device.origin, begun["address"].clone())
        .await?;
    let reply = node.exchange(&invitation.device.origin, &json!({"v":1,"request":{"kind":"confirm_join","id":invitation.id,"secret":invitation.secret,"challenge":begun["challenge"]}})).await?;
    ensure!(
        reply["ok"] == true,
        "{}",
        reply["error"].as_str().unwrap_or("连接确认失败")
    );
    if reply["data"]["joined"] != true {
        ensure!(
            reply["data"]["status"] == "awaiting_approval",
            "invalid_invitation_response"
        );
        ensure!(begun["claim_watch"] == 1, "invite_watch_unsupported");
        return Ok(Some(zork_mesh::feed::Watch::Invitation {
            id: invitation.id.clone(),
            secret: invitation.secret.clone(),
            challenge: begun["challenge"]
                .as_str()
                .context("invite_challenge_missing")?
                .to_owned(),
        }));
    }
    let group: MeshGroup = serde_json::from_value(reply["data"]["group"].clone())?;
    group.validate()?;
    ensure!(
        group.authority == invitation.device.origin
            && group.clients.iter().any(|c| c.origin == identity),
        "invalid_client_membership"
    );
    let existing = store.nodes()?;
    ensure!(
        existing
            .iter()
            .filter(|n| !group.members.iter().any(|m| m.origin == n.id))
            .count()
            + group.members.len()
            <= 16,
        "最多添加 16 台设备"
    );
    let nodes: Vec<_> = group
        .members
        .into_iter()
        .map(|device| SavedNode {
            id: device.origin.clone(),
            name: device.name,
            url: String::new(),
            token: None,
            local: false,
            mesh: Some(RemoteNode {
                origin: device.origin,
                addr: device.addr,
            }),
            group: Some(group.authority.clone()),
        })
        .collect();
    for peer in &nodes {
        node.trust(
            &peer.id,
            &peer.name,
            peer.mesh.as_ref().and_then(|p| p.addr.as_deref()),
        )
        .await?;
    }
    store.accept_invitation_if(&invitation.id, &nodes, &pending.network)?;
    Ok(None)
}

pub(crate) struct InvitationState {
    pub(crate) source: Arc<zork_observe::ValueSource<Value>>,
    generation: Mutex<u64>,
}
impl InvitationState {
    pub(crate) fn new(value: Value) -> Self {
        Self {
            source: Arc::new(zork_observe::ValueSource::new(value)),
            generation: Mutex::new(0),
        }
    }
    pub(crate) fn replace(&self, value: Value) -> u64 {
        let mut generation = self.generation.lock().unwrap();
        *generation += 1;
        self.source.invalidate(value);
        *generation
    }
    fn publish(&self, generation: u64, value: Value) {
        let current = self.generation.lock().unwrap();
        if *current == generation {
            self.source.publish(value);
        }
    }
}

pub(crate) fn public_snapshot(store: &ClientStore, running: bool) -> Result<Value> {
    let pending: Option<Pending> = store.get("device", "invitation")?;
    let input: Option<Input> = store.get("device", "invitation_input")?;
    Ok(json!({
        "invitation": pending.as_ref().map(|p| json!({"name":p.invitation.device.name,"expires_at":p.invitation.expires_at,"status":"waiting","id":p.invitation.id})).or_else(|| input.as_ref().map(input_snapshot)),
        "done":pending.is_none() && input.is_none(),
        "identity":store.get::<String>("device","identity")?, "running":running,
        "nodes":store.nodes()?, "selected_peer":store.get::<Option<String>>("device","last-node")?.flatten(),
        "network":store.get::<Network>("device","network")?.unwrap_or_default(),
    }))
}

async fn resolve_input(
    root: &std::path::Path,
    store: &ClientStore,
    input: Input,
    view: &InvitationState,
    generation: u64,
    account: &crate::relay_account::controller::Controller,
) -> Result<Pending> {
    let mut retry = zork_notify::retry::Retry::default();
    let mut changes = account.subscribe();
    let public_relay = !zork_mesh::enrollment::ticket::Ticket::decode(&input.ticket)?
        .network_config()
        .offline;
    loop {
        match crate::transport::resolve_invitation(root, &input.ticket, InviteKind::Client).await {
            Ok(invitation) => {
                let network = if store.nodes()?.is_empty() {
                    Network {
                        direct_only: invitation.offline,
                        relay_urls: invitation.relay_urls.clone(),
                        discovery_url: invitation.discovery_url.clone(),
                    }
                } else {
                    store.get("device", "network")?.unwrap_or_default()
                };
                let pending = Pending {
                    invitation,
                    name: input.name,
                    network,
                };
                store.resolve_invitation_if(&input.id, &pending)?;
                return Ok(pending);
            }
            Err(error) => {
                let reason = error.to_string();
                let terminal = [
                    "expired",
                    "revoked",
                    "mismatch",
                    "invalid",
                    "unsupported",
                    "not_claimed",
                ]
                .iter()
                .any(|part| reason.contains(part));
                let mut snapshot = public_snapshot(store, true)?;
                if snapshot["invitation"]["id"] != input.id {
                    anyhow::bail!("invitation_cancelled");
                }
                let login = public_relay && !account.snapshot().authenticated;
                snapshot["done"] = json!(terminal);
                snapshot["invitation"]["status"] = json!(if terminal {
                    "failed"
                } else if login {
                    "login_required"
                } else {
                    "resolving"
                });
                snapshot["notice"] = json!(if terminal {
                    "邀请无法使用，请在电脑重新生成。"
                } else if login {
                    "尚未找到局域网设备。跨网络连接请先登录 Zork，登录后会继续此邀请。"
                } else {
                    "暂时无法读取邀请，正在重试…"
                });
                view.publish(generation, snapshot);
                if terminal {
                    return Err(error);
                }
                tokio::select! { _ = retry.wait() => {}, _ = changes.changed() => {} }
            }
        }
    }
}

async fn run_invitation(
    root: PathBuf,
    store: Arc<ClientStore>,
    node: zork_mesh::node::MeshNode,
    pending: Pending,
    view: Arc<InvitationState>,
    generation: u64,
    account: Arc<crate::relay_account::controller::Controller>,
) {
    let mut retry = zork_notify::retry::Retry::default();
    let mut account_changes = account.subscribe();
    loop {
        let result: Result<()> = async {
            let request = connect_invitation(&root, &store, &node, &pending).await?;
            let Some(request) = request else {
                return Ok(());
            };
            retry.reset();
            let mut snapshot = public_snapshot(&store, true)?;
            snapshot["invitation"]["status"] = json!("awaiting_approval");
            view.publish(generation, snapshot);
            let mut feed = node.follow(pending.invitation.device.origin.clone(), request);
            while let Some(event) = feed.next().await {
                match event {
                    zork_mesh::feed::Event::Data(value) => {
                        ensure!(
                            value["id"] == pending.invitation.id,
                            "invite_reply_scope_mismatch"
                        );
                        if matches!(value["status"].as_str(), Some("approved" | "joined")) {
                            ensure!(
                                connect_invitation(&root, &store, &node, &pending)
                                    .await?
                                    .is_none(),
                                "invite_approval_changed"
                            );
                            return Ok(());
                        }
                    }
                    zork_mesh::feed::Event::Disconnected {
                        error,
                        terminal: true,
                    } => anyhow::bail!("{error}"),
                    zork_mesh::feed::Event::Disconnected {
                        error: _,
                        terminal: false,
                    } => {
                        let mut snapshot = public_snapshot(&store, true)?;
                        snapshot["notice"] = json!("暂时无法连接设备，正在恢复订阅…");
                        view.publish(generation, snapshot);
                    }
                }
            }
            anyhow::bail!("invite_subscription_closed")
        }
        .await;
        let Ok(mut snapshot) = public_snapshot(&store, true) else {
            return;
        };
        match result {
            Ok(()) => {
                snapshot["joined_peer"] = json!(pending.invitation.device.origin);
                snapshot["notice"] = json!("设备已连接");
                view.publish(generation, snapshot);
                return;
            }
            Err(error) => {
                let reason = error.to_string();
                let terminal = [
                    "expired",
                    "revoked",
                    "removed",
                    "another_device",
                    "already_used",
                    "mismatch",
                    "unsupported",
                    "cancelled",
                    "authority_changed",
                    "not_claimed",
                ]
                .iter()
                .any(|part| reason.contains(part));
                snapshot["done"] = json!(terminal);
                let (status, notice) = if reason.contains("unsupported") {
                    ("failed", "邀请设备需要更新后才能实时确认审批")
                } else if reason.contains("expired") || reason.contains("not_claimed") {
                    ("expired", "邀请已过期或邀请设备已重启，请重新生成")
                } else if reason.contains("revoked")
                    || reason.contains("removed")
                    || reason.contains("cancelled")
                {
                    ("revoked", "连接邀请已取消或访问权限已撤销")
                } else if terminal {
                    ("conflict", "邀请身份或来源已变化，请重新生成")
                } else if !account.snapshot().authenticated && !pending.invitation.offline {
                    (
                        "login_required",
                        "跨网络连接需要登录 Zork；登录后会自动继续此邀请。",
                    )
                } else {
                    ("waiting", "暂时无法连接设备，正在重试…")
                };
                snapshot["notice"] = json!(notice);
                snapshot["invitation"]["status"] = json!(status);
                view.publish(generation, snapshot);
                if terminal {
                    return;
                }
                tokio::select! { _ = retry.wait() => {}, _ = account_changes.changed() => {} }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_fences_unresolved_invitation_and_late_login_resume() {
        let root = tempfile::tempdir().unwrap();
        let store = ClientStore::open(root.path()).unwrap();
        store
            .put("device", "invitation_input", &json!({"id":"first"}))
            .unwrap();
        store.forget_invitation().unwrap();
        assert!(store
            .resolve_invitation_if("first", &json!({"invitation":{"id":"resolved"}}))
            .is_err());
        store
            .put("device", "invitation_input", &json!({"id":"second"}))
            .unwrap();
        assert!(store
            .resolve_invitation_if("first", &json!({"invitation":{"id":"resolved"}}))
            .is_err());
        store
            .resolve_invitation_if("second", &json!({"invitation":{"id":"resolved"}}))
            .unwrap();
        assert!(store
            .get::<Value>("device", "invitation_input")
            .unwrap()
            .is_none());
    }
    #[test]
    fn a_cancelled_claim_cannot_commit_peers_after_the_network_reply_arrives() {
        let root = tempfile::tempdir().unwrap();
        let store = ClientStore::open(root.path()).unwrap();
        store
            .put(
                "device",
                "invitation",
                &json!({"invitation":{"id":"first"}}),
            )
            .unwrap();
        store
            .put(
                "device",
                "invitation",
                &json!({"invitation":{"id":"second"}}),
            )
            .unwrap();
        assert!(store
            .accept_invitation_if("first", &[], &Network::default())
            .is_err());
        assert_eq!(
            store.get::<Value>("device", "invitation").unwrap().unwrap()["invitation"]["id"],
            "second"
        );
        store.forget_invitation().unwrap();
        assert!(store
            .accept_invitation_if("second", &[], &Network::default())
            .is_err());
    }
}
