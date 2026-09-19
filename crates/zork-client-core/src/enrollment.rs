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
    #[serde(default)]
    switch_from: Option<zork_config::membership::MeshVersion>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Input {
    id: String,
    ticket: String,
    name: String,
    #[serde(default)]
    resolved: Option<Invitation>,
}
fn input_snapshot(input: &Input) -> Value {
    json!({"name":input.resolved.as_ref().map_or("连接设备", |invite| invite.device.name.as_str()),
        "status":if input.resolved.is_some() {"switch_confirmation"} else {"resolving"}, "id":input.id})
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
        if input.as_ref().is_some_and(|input| input.resolved.is_some()) {
            self.invitation
                .replace(public_snapshot(&self.store, self.runtime.is_some())?);
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
        self.invitation_job = Some(zork_notify::Task(tokio::spawn(async move {
            let pending = match pending {
                Some(pending) => pending,
                None => {
                    match resolve_input(&root, &store, input.unwrap(), &view, generation).await {
                        Ok(pending) => pending,
                        Err(_) => return,
                    }
                }
            };
            run_invitation(root, store, node, pending, view, generation).await;
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
                if input.resolved.is_some() {
                    return Ok(self.store.get("device", "network")?.unwrap_or_default());
                }
                let config = zork_mesh::enrollment::ticket::Ticket::decode(&input.ticket)?
                    .network_config()?;
                return Ok(Network {
                    direct_only: config.offline,
                    relay_urls: config.relay_urls,
                    discovery_url: config.discovery_url,
                    relay_quic_port: config.relay_quic_port,
                    quic_discovery_urls: config.quic_discovery_urls,
                    channel: config.channel,
                });
            }
        }
        Ok(self.store.get("device", "network")?.unwrap_or_default())
    }
    pub(crate) async fn begin_invitation(
        &mut self,
        ticket: &str,
        name: &str,
        switch_from: Option<zork_config::membership::MeshVersion>,
    ) -> Result<Value> {
        let name = name.trim();
        ensure!(
            !name.is_empty() && name.len() <= 128 && !name.chars().any(char::is_control),
            "手机名称无效"
        );
        let ticket = ticket.trim();
        if zork_mesh::enrollment::ticket::Ticket::is_short(ticket) && switch_from.is_none() {
            let decoded = zork_mesh::enrollment::ticket::Ticket::decode(ticket)
                .context("无法识别连接邀请")?;
            ensure!(
                decoded.kind == InviteKind::Client,
                "这是执行设备邀请，请在桌面选择连接手机"
            );
            decoded.network_config()?;
            let existing: Option<Input> = self.store.get("device", "invitation_input")?;
            if let Some(existing) = existing {
                ensure!(existing.ticket == ticket, "请先取消当前连接邀请");
            } else {
                ensure!(self.pending_invitation()?.is_none(), "请先取消当前连接邀请");
                if self.store.nodes()?.is_empty() {
                    self.pause().await?;
                }
                self.store.put(
                    "device",
                    "invitation_input",
                    &Input {
                        id: ulid::Ulid::new().to_string(),
                        ticket: ticket.into(),
                        name: name.into(),
                        resolved: None,
                    },
                )?;
            }
            self.resume().await?;
            return self.snapshot();
        }
        let input: Option<Input> = self.store.get("device", "invitation_input")?;
        if let Some(input) = &input {
            ensure!(input.ticket == ticket, "请先取消当前连接邀请");
        }
        let invitation = match input.as_ref().and_then(|input| input.resolved.clone()) {
            Some(invitation) => invitation,
            None => crate::transport::resolve_invitation(&self.root, ticket, InviteKind::Client)
                .await
                .context("无法识别连接邀请，请扫描 Zork 的手机连接二维码")?,
        };
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
        if let Some(pending) = self.pending_invitation()? {
            ensure!(
                pending.invitation.id == invitation.id
                    && pending.invitation.secret == invitation.secret,
                "请先取消当前连接邀请"
            );
            return self.snapshot();
        }
        let current = self.store.current_mesh()?;
        let switching = current
            .as_ref()
            .is_some_and(|group| group.authority != invitation.device.origin);
        if switching {
            let group = current.as_ref().unwrap();
            if let Some(expected) = &switch_from {
                ensure!(expected.matches(group), "Mesh 成员已变化，请重新确认切换");
            } else {
                self.invitation_job.take();
                let input = Input {
                    id: input
                        .as_ref()
                        .map_or_else(|| ulid::Ulid::new().to_string(), |input| input.id.clone()),
                    ticket: ticket.into(),
                    name: name.into(),
                    resolved: Some(invitation.clone()),
                };
                self.store.put("device", "invitation_input", &input)?;
                self.invitation
                    .replace(public_snapshot(&self.store, self.runtime.is_some())?);
                return self.snapshot();
            }
        } else {
            ensure!(switch_from.is_none(), "Mesh 已变化，请重新读取邀请");
        }
        let existing = self.store.nodes()?;
        let network = if existing.is_empty() || switch_from.is_some() {
            Network {
                direct_only: invitation.offline,
                relay_urls: invitation.relay_urls.clone(),
                discovery_url: invitation.discovery_url.clone(),
                relay_quic_port: invitation.relay_quic_port,
                quic_discovery_urls: invitation.quic_discovery_urls.clone(),
                channel: Some(invitation.channel),
            }
        } else {
            self.store.get("device", "network")?.unwrap_or_default()
        };
        self.config(&network)?;
        self.pause().await?;
        let pending = Pending {
            invitation,
            name: name.into(),
            network,
            switch_from,
        };
        if let Some(input) = input {
            self.store.resolve_invitation_if(&input.id, &pending)?;
        } else {
            self.store.put("device", "invitation", &pending)?;
        }
        self.resume().await?;
        self.snapshot()
    }
    pub(crate) async fn confirm_invitation_switch(
        &mut self,
        input_id: &str,
        expected: zork_config::membership::MeshVersion,
    ) -> Result<Value> {
        let input = self
            .store
            .get::<Input>("device", "invitation_input")?
            .context("请重新读取连接邀请")?;
        ensure!(input.id == input_id, "连接邀请已变化，请重新确认");
        ensure!(input.resolved.is_some(), "请等待连接邀请读取完成");
        self.begin_invitation(&input.ticket, &input.name, Some(expected))
            .await
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
        routes: Some(node.routes()?),
        origin: identity.clone(),
        name: pending.name.clone(),
        addr: None,
    };
    device.validate()?;
    let config = MeshConfig {
        offline: invitation.offline,
        relay_urls: invitation.relay_urls.clone(),
        discovery_url: invitation.discovery_url.clone(),
        relay_quic_port: invitation.relay_quic_port,
        quic_discovery_urls: invitation.quic_discovery_urls.clone(),
        channel: Some(invitation.channel),
        bind: None,
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
    let reply = node.exchange(&invitation.device.origin, &json!({"v":1,"request":{"kind":"confirm_join","id":invitation.id,"secret":invitation.secret,"challenge":begun["challenge"],"address":node.address()?}})).await?;
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
        .iter()
        .cloned()
        .map(|device| SavedNode {
            id: device.origin.clone(),
            name: device.name,
            url: String::new(),
            token: None,
            local: false,
            mesh: Some(RemoteNode {
                routes: device.routes,
                origin: device.origin,
                addr: device.addr,
            }),
            group: Some(group.authority.clone()),
        })
        .collect();
    store.accept_invitation_if(&invitation.id, &nodes, &pending.network, &group)?;
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
    let confirmation = if let Some(input) = input.as_ref() {
        match &input.resolved {
            Some(invitation) => store
                .current_mesh()?
                .map(|group| switch_confirmation(&input.id, &group, invitation)),
            None => None,
        }
    } else {
        None
    };
    Ok(json!({
        "switch_confirmation": confirmation,
        "invitation": pending.as_ref().map(|p| json!({"name":p.invitation.device.name,"expires_at":p.invitation.expires_at,"status":"waiting","id":p.invitation.id})).or_else(|| input.as_ref().map(input_snapshot)),
        "done":pending.is_none() && input.is_none(),
        "identity":store.get::<String>("device","identity")?, "running":running,
        "nodes":store.nodes()?, "selected_peer":store.get::<Option<String>>("device","last-node")?.flatten(),
        "network":store.get::<Network>("device","network")?.unwrap_or_default(),
    }))
}

fn switch_confirmation(input_id: &str, group: &MeshGroup, invitation: &Invitation) -> Value {
    json!({"input_id": input_id, "expected": zork_config::membership::MeshVersion::of(group),
        "target_name": invitation.device.name,
        "message": format!("切换到「{}」所在的 Mesh？本机保留聊天记录与草稿，原 Mesh 的其它成员不变。", invitation.device.name)})
}

async fn resolve_input(
    root: &std::path::Path,
    store: &ClientStore,
    input: Input,
    view: &InvitationState,
    generation: u64,
) -> Result<Pending> {
    let mut retry = zork_notify::retry::Retry::default();
    loop {
        match crate::transport::resolve_invitation(root, &input.ticket, InviteKind::Client).await {
            Ok(invitation) => {
                if store
                    .current_mesh()?
                    .as_ref()
                    .is_some_and(|group| group.authority != invitation.device.origin)
                {
                    let input = Input {
                        resolved: Some(invitation),
                        ..input
                    };
                    store.update_invitation_input_if(&input.id, &input)?;
                    view.publish(generation, public_snapshot(store, true)?);
                    anyhow::bail!("switch_confirmation_required");
                }
                let network = if store.nodes()?.is_empty() {
                    Network {
                        direct_only: invitation.offline,
                        relay_urls: invitation.relay_urls.clone(),
                        discovery_url: invitation.discovery_url.clone(),
                        relay_quic_port: invitation.relay_quic_port,
                        quic_discovery_urls: invitation.quic_discovery_urls.clone(),
                        channel: Some(invitation.channel),
                    }
                } else {
                    store.get("device", "network")?.unwrap_or_default()
                };
                let pending = Pending {
                    invitation,
                    name: input.name,
                    network,
                    switch_from: None,
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
                snapshot["done"] = json!(terminal);
                snapshot["invitation"]["status"] =
                    json!(if terminal { "failed" } else { "resolving" });
                snapshot["notice"] = json!(if terminal {
                    "邀请无法使用，请在电脑重新生成。"
                } else {
                    "暂时无法读取邀请，正在重试…"
                });
                view.publish(generation, snapshot);
                if terminal {
                    return Err(error);
                }
                retry.wait().await;
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
) {
    let mut retry = zork_mesh::retry::DiscoveryBackoff::default();
    let mut attempt = 0u64;
    loop {
        attempt += 1;
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
                } else {
                    ("waiting", "暂时无法连接设备，正在重试…")
                };
                snapshot["notice"] = json!(notice);
                snapshot["invitation"]["status"] = json!(status);
                let delay = retry.next_delay();
                snapshot["attempt"] = json!(attempt);
                snapshot["retry_in_seconds"] = if terminal {
                    Value::Null
                } else {
                    json!(delay.as_secs())
                };
                if !terminal {
                    snapshot["notice"] = json!(format!(
                        "暂时无法连接设备，{} 秒后重试（第 {} 次）",
                        delay.as_secs(),
                        attempt + 1
                    ));
                }
                view.publish(generation, snapshot);
                if terminal {
                    return;
                }
                tokio::time::sleep(delay).await;
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
            .update_invitation_input_if("first", &json!({"id":"first", "resolved":true}))
            .is_err());
        assert!(store
            .resolve_invitation_if("first", &json!({"invitation":{"id":"resolved"}}))
            .is_err());
        store
            .put("device", "invitation_input", &json!({"id":"second"}))
            .unwrap();
        assert!(store
            .resolve_invitation_if("first", &json!({"invitation":{"id":"resolved"}}))
            .is_err());
        assert!(store
            .update_invitation_input_if("first", &json!({"id":"first", "resolved":true}))
            .is_err());
        assert_eq!(
            store
                .get::<Value>("device", "invitation_input")
                .unwrap()
                .unwrap()["id"],
            "second"
        );
        store
            .update_invitation_input_if("second", &json!({"id":"second", "resolved":true}))
            .unwrap();
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
        let authority = format!("key:{}", "y".repeat(52));
        let group = MeshGroup {
            authority: authority.clone(),
            revision: 1,
            members: vec![MeshDevice {
                routes: None,
                origin: authority,
                name: "Station".into(),
                addr: None,
            }],
            clients: vec![],
        };
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
            .accept_invitation_if("first", &[], &Network::default(), &group)
            .is_err());
        assert_eq!(
            store.get::<Value>("device", "invitation").unwrap().unwrap()["invitation"]["id"],
            "second"
        );
        store.forget_invitation().unwrap();
        assert!(store
            .accept_invitation_if("second", &[], &Network::default(), &group)
            .is_err());
    }
}
