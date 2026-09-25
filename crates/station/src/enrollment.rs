//! Station-owned invitations and personal mesh membership. No model secrets
//! leave this node. Enrollment is finalized by the joining iroh device key.
mod operations;
mod routes;
use crate::state::AppState;
use anyhow::{ensure, Context, Result};
pub use routes::start as start_routes;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use zork_client_core::transport::Enrollment;
use zork_config::{
    membership::{MeshDevice, MeshGroup},
    MeshConfig,
};
use zork_mesh::{
    enrollment::{self, Invitation},
    node::MeshNode,
};

pub struct EnrollmentService {
    pub transport: Enrollment,
    root: PathBuf,
    node: MeshNode,
    origin: String,
    transaction: tokio::sync::Mutex<()>,
    join_transaction: tokio::sync::Mutex<()>,
    join_state: std::sync::Mutex<Option<operations::JoinRecord>>,
    join_job: std::sync::Mutex<Option<zork_notify::Task<()>>>,
    pending: std::sync::Mutex<std::collections::HashMap<String, InviteRecord>>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct InviteRecord {
    id: String,
    secret_hash: String,
    expires_at: u64,
    revoked: bool,
    claim: Option<Claim>,
    used_by: Option<String>,
    committed: bool,
}
#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct Claim {
    endpoint: String,
    device: MeshDevice,
    challenge: String,
    expires_at: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BeginJoin {
    id: String,
    secret: String,
    device: MeshDevice,
}

impl EnrollmentService {
    pub async fn new(root: &Path, config: &MeshConfig, node: MeshNode) -> Result<Self> {
        Ok(Self {
            transport: Enrollment::bind(root, config).await?,
            origin: node.identity().await?,
            node,
            root: root.into(),
            transaction: Default::default(),
            join_transaction: Default::default(),
            join_state: std::sync::Mutex::new(operations::load(root)?),
            join_job: Default::default(),
            pending: Default::default(),
        })
    }
    fn path(&self, id: &str) -> Result<PathBuf> {
        ensure!(
            id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid_invite_id"
        );
        Ok(self.root.join("mesh/invites").join(format!("{id}.json")))
    }
    fn load(&self, id: &str) -> Result<InviteRecord> {
        self.path(id)?;
        if let Some(record) = self.pending.lock().expect("pending invitations").get(id) {
            return Ok(record.clone());
        }
        Ok(serde_json::from_slice(
            &std::fs::read(self.path(id)?).context("invite_not_found")?,
        )?)
    }
    fn save(&self, record: &InviteRecord) -> Result<()> {
        if record.used_by.is_none() {
            self.pending
                .lock()
                .expect("pending invitations")
                .insert(record.id.clone(), record.clone());
            return Ok(());
        }
        private_json(&self.path(&record.id)?, record)?;
        self.pending
            .lock()
            .expect("pending invitations")
            .remove(&record.id);
        Ok(())
    }
    fn records(&self) -> Result<Vec<InviteRecord>> {
        let dir = self.root.join("mesh/invites");
        std::fs::create_dir_all(&dir)?;
        let mut pending = self.pending.lock().expect("pending invitations");
        pending.retain(|_, r| r.expires_at.saturating_add(60) > enrollment::now());
        let mut result: Vec<_> = pending.values().cloned().collect();
        drop(pending);
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "json") {
                result.push(serde_json::from_slice(&std::fs::read(path)?)?);
            }
        }
        Ok(result)
    }

    /// Unused, unexpired invitations this device can still be reached for.
    fn open_invites(&self) -> Result<Vec<InviteRecord>> {
        let now = enrollment::now();
        Ok(self
            .records()?
            .into_iter()
            .filter(|r| !r.revoked && r.used_by.is_none() && r.expires_at > now)
            .collect())
    }
    /// The enrollment endpoint keeps a relay connection only while open
    /// invitations exist; returns the earliest open expiry.
    async fn sync_invite_relay(&self) -> Option<u64> {
        let open = match self.open_invites() {
            Ok(open) => open,
            Err(error) => {
                tracing::warn!(%error, "invite relay state unavailable");
                return None;
            }
        };
        self.transport.hold_relay_for_invites(!open.is_empty()).await;
        open.iter().map(|r| r.expires_at).min()
    }

    pub async fn create(&self, state: &AppState) -> Result<Value> {
        let service = state.mesh.get().context("mesh_not_ready")?;
        if let Some(group) = zork_config::load_config(&self.root)?.mesh.group {
            ensure!(group.contains(service.origin()), "device_removed_from_mesh");
            if group.authority != service.origin() {
                return service
                    .membership_call(&group.authority, "create_invite", json!({}))
                    .await;
            }
        }
        let channel = zork_config::channel::current()?;
        let test_source = if channel == zork_config::channel::Channel::Test {
            Some(zork_config::distribution::TestDistribution::load(
                &zork_config::relay_account::resolve_root(&self.root)?,
            )?)
        } else {
            None
        };
        let _guard = self.transaction.lock().await;
        let device = own_device(&self.root, service.origin(), &self.node)?;
        zork_config::update_config(&self.root, |config| {
            if config.mesh.group.is_none() {
                MeshGroup {
                    authority: device.origin.clone(),
                    revision: 1,
                    members: vec![device.clone()],
                    clients: vec![],
                }
                .apply(service.origin(), &mut config.mesh)?;
            }
            Ok(())
        })?;
        let records = self.records()?;
        ensure!(
            self.pending.lock().expect("pending invitations").len() < 64,
            "too_many_recent_invites"
        );
        ensure!(
            records
                .iter()
                .filter(|r| !r.revoked && r.used_by.is_none() && r.expires_at > enrollment::now())
                .count()
                < 8,
            "too_many_active_invites"
        );
        // Keep recent receipts for lost acknowledgements without growing forever.
        for record in records
            .iter()
            .filter(|r| r.expires_at.saturating_add(86400) < enrollment::now())
        {
            let _ = std::fs::remove_file(self.path(&record.id)?);
        }
        let config = zork_config::load_config(&self.root)?.mesh;
        // Joiners outside the LAN reach this endpoint through the relay.
        self.transport.hold_relay_for_invites(true).await;
        let short = match enrollment::ticket::Ticket::new(self.transport.address().await, &config)
        {
            Ok(short) => short,
            Err(error) => {
                self.sync_invite_relay().await;
                return Err(error);
            }
        };
        let id = short.id();
        let secret = short.secret();
        let expires_at = enrollment::now() + enrollment::INVITE_SECONDS;
        let record = InviteRecord {
            id: id.clone(),
            secret_hash: zork_mesh::content_root(secret.as_bytes()),
            expires_at,
            revoked: false,
            claim: None,
            used_by: None,
            committed: false,
        };
        self.save(&record)?;
        let ticket = short.encode()?;
        state.db.realtime.notify(crate::realtime::MESH);
        // Portable installed-CLI command: it never names the inviting host's
        // filesystem or assumes an unpublished GitHub Release asset exists.
        let command = format!("zork mesh join '{ticket}' --channel {}", channel.as_str());
        let mut network = config.clone();
        zork_config::services::ServicesConfig::load_for_data_root(
            &zork_config::relay_account::resolve_root(&self.root)?,
        )?
        .apply_defaults(&mut network)?;
        let install_url = zork_config::relay_account::control_origin(network.relay_urls.as_deref())
            .map(|origin| {
                let page = test_source
                    .as_ref()
                    .and_then(|source| source.install_page.clone())
                    .unwrap_or_else(|| format!("{origin}/install"));
                let mut link = format!("{page}#ticket={ticket}&channel={}", channel.as_str());
                if let Some(source) = test_source {
                    let fields = source.fragment_fields();
                    link.push('&');
                    link.push_str(&fields);
                }
                link
            });

        Ok(
            json!({"id":id,"expires_at":expires_at,"invitation":ticket,"command":command,"install_url":install_url,"scope":"personal_mesh","permissions":["collaborate","task_files","node_management"]}),
        )
    }

    pub async fn list(&self, state: &AppState) -> Result<Value> {
        if let Some(group) = zork_config::load_config(&self.root)?.mesh.group {
            let service = state.mesh.get().context("mesh_not_ready")?;
            if group.authority != service.origin() {
                return service
                    .membership_call(&group.authority, "list_invites", json!({}))
                    .await;
            }
        }
        let _guard = self.transaction.lock().await;
        Ok(
            json!({"items":self.records()?.into_iter().map(|r|json!({"id":r.id,"expires_at":r.expires_at,"claim_id":r.claim.as_ref().map(|c|zork_mesh::content_root(c.challenge.as_bytes())),"status":if r.revoked {"revoked"}else if r.committed{"joined"}else if r.expires_at<=enrollment::now(){"expired"}else if r.claim.is_some(){"connecting"}else{"waiting"},"device":r.claim.map(|c|c.device),"origin":r.used_by})).collect::<Vec<_>>()}),
        )
    }

    pub async fn revoke(&self, state: &AppState, id: &str) -> Result<Value> {
        if let Some(group) = zork_config::load_config(&self.root)?.mesh.group {
            let service = state.mesh.get().context("mesh_not_ready")?;
            if group.authority != service.origin() {
                return service
                    .membership_call(&group.authority, "revoke_invite", json!({"id":id}))
                    .await;
            }
        }
        let _guard = self.transaction.lock().await;
        let mut record = self.load(id)?;
        ensure!(
            record.used_by.is_none(),
            "invite_already_used_remove_device_instead"
        );
        record.revoked = true;
        self.save(&record)?;
        self.sync_invite_relay().await;
        state.db.realtime.notify(crate::realtime::MESH);
        Ok(json!({"revoked":true}))
    }

    async fn resolve_metadata(&self, state: &AppState, request: Value) -> Result<Value> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Resolve {
            op: String,
            id: String,
            secret: String,
        }
        let request: Resolve = serde_json::from_value(request)?;
        ensure!(request.op == "resolve", "invalid_invite_operation");
        let _guard = self.transaction.lock().await;
        let record = self
            .load(&request.id)
            .context("invite_expired_or_restart")?;
        ensure!(
            !record.revoked
                && zork_mesh::content_root(request.secret.as_bytes()) == record.secret_hash,
            "invalid_or_revoked_invite"
        );
        ensure!(
            record.expires_at > enrollment::now() || record.used_by.is_some(),
            "invite_expired"
        );
        let service = state.mesh.get().context("mesh_not_ready")?;
        let config = zork_config::load_config(&self.root)?.mesh;
        if let Some(origin) = &record.used_by {
            ensure!(
                config
                    .group
                    .as_ref()
                    .is_some_and(|g| !record.committed || enrolled(g, origin)),
                "device_removed_from_mesh"
            );
        }
        let invitation = Invitation {
            channel: zork_config::channel::current()?,
            relay_quic_port: config.relay_quic_port,
            quic_discovery_urls: config.quic_discovery_urls.clone(),
            version: 1,
            id: record.id,
            secret: request.secret,
            endpoint: self.transport.address().await,
            device: own_device(&self.root, service.origin(), &self.node)?,
            expires_at: record.expires_at,
            offline: config.offline,
            relay_urls: config.relay_urls,
            discovery_url: config.discovery_url,
        };
        Ok(json!({"invitation":invitation}))
    }

    pub async fn begin(&self, state: &AppState, endpoint: String, request: Value) -> Result<Value> {
        if request["op"] == "resolve" {
            return self.resolve_metadata(state, request).await;
        }
        let request: BeginJoin = serde_json::from_value(request)?;
        request.device.validate()?;
        let _guard = self.transaction.lock().await;
        let mut record = self.load(&request.id)?;
        ensure!(
            !record.revoked
                && zork_mesh::content_root(request.secret.as_bytes()) == record.secret_hash,
            "invalid_or_revoked_invite"
        );
        let service = state.mesh.get().context("mesh_not_ready")?;
        let config = zork_config::load_config(&self.root)?.mesh;
        let group = config.group.context("mesh_membership_missing")?;
        ensure!(
            group.authority == service.origin(),
            "invite_authority_changed"
        );
        ensure!(
            request.device.origin != service.origin(),
            "cannot_join_this_device_to_itself"
        );
        if let Some(origin) = &record.used_by {
            ensure!(
                origin == &request.device.origin && (!record.committed || enrolled(&group, origin)),
                "invite_already_used_or_device_removed"
            );
            // Replaying the exact receipt is safe even after the short invitation expired.
            ensure!(
                record
                    .claim
                    .as_ref()
                    .is_some_and(|c| c.endpoint == endpoint),
                "invite_already_used"
            );
        } else {
            ensure!(record.expires_at > enrollment::now(), "invite_expired");
            ensure!(
                group.members.len() < 16 || enrolled(&group, &request.device.origin),
                "mesh_device_limit"
            );
        }
        if let Some(claim) = &record.claim {
            if claim.expires_at > enrollment::now() || record.used_by.is_some() {
                ensure!(
                    claim.endpoint == endpoint && claim.device.origin == request.device.origin,
                    "invite_claimed_by_another_device"
                );
            } else {
                record.claim = None;
            }
        }
        if record.claim.is_none() {
            record.claim = Some(Claim {
                endpoint,
                device: request.device,
                challenge: enrollment::secret(),
                expires_at: (enrollment::now() + 60).min(record.expires_at),
            });
            self.save(&record)?;
        }
        let claim = record.claim.as_ref().unwrap();
        Ok(
            json!({"challenge":claim.challenge,"origin":service.origin(),"address":self.node.address()?}),
        )
    }

    pub async fn confirm(
        &self,
        state: &AppState,
        origin: &str,
        id: &str,
        secret: &str,
        challenge: &str,
        address: Option<Value>,
    ) -> Result<Value> {
        let _guard = self.transaction.lock().await;
        let mut record = self.load(id)?;
        ensure!(
            !record.revoked && zork_mesh::content_root(secret.as_bytes()) == record.secret_hash,
            "invalid_or_revoked_invite"
        );
        let claim = record.claim.clone().context("invite_not_claimed")?;
        ensure!(
            claim.device.origin == origin && claim.challenge == challenge,
            "invite_device_proof_mismatch"
        );
        // QUIC proved the claim's device identity. Only now accept its current
        // address; a bootstrap caller cannot plant routes for somebody else's key.
        if let Some(address) = address {
            self.node.remember_peer_address(origin, address).await?;
        }
        let service = state.mesh.get().context("mesh_not_ready")?;
        let mut config = zork_config::load_config(&self.root)?.mesh;
        let mut group = config.group.take().context("mesh_membership_missing")?;
        ensure!(
            group.authority == service.origin(),
            "invite_authority_changed"
        );
        if let Some(used) = &record.used_by {
            ensure!(
                used == origin && (!record.committed || enrolled(&group, origin)),
                "device_removed_from_mesh"
            );
        }
        if !record.committed {
            if record.used_by.is_none() {
                ensure!(
                    record.expires_at > enrollment::now() && claim.expires_at > enrollment::now(),
                    "invite_expired"
                );
            }
            // Persist the consumed identity before granting it. A crash resumes only this claim.
            record.used_by = Some(origin.into());
            self.save(&record)?;
            let device = claim.device.clone();
            if !enrolled(&group, origin) {
                ensure!(
                    !group.clients.iter().any(|c| c.origin == origin),
                    "device_is_a_client"
                );
                group.members.push(device);
                group.revision += 1;
            }
            group.validate()?;
            zork_config::update_config(&self.root, |config| {
                group.apply(service.origin(), &mut config.mesh)?;
                Ok(())
            })?;
            record.committed = true;
            self.save(&record)?;
            self.sync_invite_relay().await;
        }
        service.refresh(state).await?;
        state
            .db
            .realtime
            .notify(crate::realtime::MESH | crate::realtime::WORK);
        Ok(json!({"joined":true,"group":group}))
    }

    async fn join_once(
        &self,
        state: &AppState,
        ticket: &str,
        name: Option<&str>,
        switch_from: Option<&zork_config::membership::MeshVersion>,
        operation: &str,
    ) -> Result<Value> {
        let _guard = self
            .join_transaction
            .try_lock()
            .context("another_join_is_in_progress")?;
        self.join_phase(state, operation, "resolving", "正在读取邀请")?;
        let invitation = enrollment::ticket::resolve_on_node(
            &self.root.join("invite-bootstrap"),
            ticket,
            &self.node,
        )
        .await?;
        let service = state.mesh.get().context("mesh_not_ready")?;
        let before = zork_config::load_config(&self.root)?.mesh;
        ensure!(
            service.origin() != invitation.device.origin,
            "cannot_join_this_device_to_itself"
        );
        if let Some(group) = &before.group {
            if group.authority != invitation.device.origin {
                let expected = switch_from.context("already_in_another_mesh")?;
                let mut preview = before.clone();
                zork_config::membership::detach(&mut preview, service.origin(), expected)?;
            }
            if group.authority == invitation.device.origin && group.contains(service.origin()) {
                // Check the current authority rather than reporting success from stale local state.
                let value = service
                    .membership_call(&group.authority, "membership", json!({}))
                    .await?;
                let current: MeshGroup = serde_json::from_value(value["group"].clone())?;
                ensure!(
                    current.contains(service.origin()),
                    "device_removed_from_mesh"
                );
                apply_group(state, &group.authority, current.clone()).await?;
                return Ok(
                    json!({"joined":true,"already_joined":true,"group":current,"origin":service.origin()}),
                );
            }
        }
        let mut device = own_device(&self.root, service.origin(), &self.node)?;
        if let Some(name) = name {
            device.name = name.trim().into();
            device.validate()?;
        }
        self.join_phase(state, operation, "connecting", "正在连接邀请设备")?;
        let begun = self
            .transport
            .exchange(
                &invitation,
                &json!({"id":invitation.id,"secret":invitation.secret,"device":device}),
            )
            .await?;
        ensure!(
            begun["origin"] == invitation.device.origin,
            "invite_authority_mismatch"
        );
        let control = &self.node;
        control
            .trust(
                &invitation.device.origin,
                &invitation.device.name,
                invitation.device.addr.as_deref(),
            )
            .await?;
        if !begun["address"].is_null() {
            control
                .remember_peer_address(&invitation.device.origin, begun["address"].clone())
                .await?;
        }
        self.join_phase(state, operation, "confirming", "正在核实设备身份与成员关系")?;
        let result=async {
            let reply=control.exchange(&invitation.device.origin,&json!({"v":1,"request":{"kind":"confirm_join","id":invitation.id,"secret":invitation.secret,"challenge":begun["challenge"],"address":control.address()?}})).await?;
            ensure!(reply["ok"]==true,"{}",reply["error"].as_str().unwrap_or("join_confirmation_failed"));
            let group:MeshGroup=serde_json::from_value(reply["data"]["group"].clone())?;
            ensure!(group.authority==invitation.device.origin && group.contains(service.origin()),"invalid_join_membership");
            if before.group.as_ref().is_some_and(|previous| previous.authority != group.authority) { self.archive_membership(&before)?; }
            zork_config::update_config(&self.root,|config|{
                if config.mesh.group.as_ref().is_some_and(|previous| previous.authority != group.authority) {
                    zork_config::membership::detach(&mut config.mesh, service.origin(), switch_from.context("already_in_another_mesh")?)?;
                }
                group.apply(service.origin(),&mut config.mesh)?;
                config.mesh.name=device.name.clone(); Ok(())
            })?;
            service.refresh(state).await?;
            Ok::<_,anyhow::Error>(json!({"joined":true,"origin":service.origin(),"group":group}))
        }.await;
        if result.is_err()
            && !zork_config::load_config(&self.root)?
                .mesh
                .peers
                .iter()
                .any(|p| p.origin == invitation.device.origin)
        {
            let _ = control.untrust(&invitation.device.origin).await;
        }
        result
    }

    pub async fn remove_member(&self, state: &AppState, origin: &str) -> Result<Value> {
        let service = state.mesh.get().context("mesh_not_ready")?;
        let group = zork_config::load_config(&self.root)?
            .mesh
            .group
            .context("mesh_membership_missing")?;
        if group.authority != service.origin() {
            return service
                .membership_call(&group.authority, "remove_member", json!({"origin":origin}))
                .await;
        }
        ensure!(origin != group.authority, "cannot_remove_mesh_authority");
        let _guard = self.transaction.lock().await;
        for mut record in self
            .records()?
            .into_iter()
            .filter(|r| r.used_by.as_deref() == Some(origin))
        {
            record.revoked = true;
            self.save(&record)?;
        }
        let group = zork_config::update_config(&self.root, |config| {
            let mut group = config
                .mesh
                .group
                .clone()
                .context("mesh_membership_missing")?;
            if group.contains(origin) || group.clients.iter().any(|c| c.origin == origin) {
                group.members.retain(|m| m.origin != origin);
                group.clients.retain(|m| m.origin != origin);
                group.revision += 1;
                group.apply(service.origin(), &mut config.mesh)?;
            }
            Ok(group)
        })?;
        // Deliver removal while the authenticated channel still exists; all other
        // members also refresh membership on reconnect or within the polling bound.
        let _ = service
            .membership_call(origin, "apply_membership", json!({"group":group}))
            .await;
        service.refresh(state).await?;
        Ok(json!({"removed":true,"group":group}))
    }

    pub async fn register_client(&self, state: &AppState, device: MeshDevice) -> Result<Value> {
        device.validate()?;
        let service = state.mesh.get().context("mesh_not_ready")?;
        let group = zork_config::load_config(&self.root)?
            .mesh
            .group
            .context("mesh_membership_missing")?;
        if group.authority != service.origin() {
            return service
                .membership_call(
                    &group.authority,
                    "register_client",
                    json!({"device":device}),
                )
                .await;
        }
        let _guard = self.transaction.lock().await;
        let group = zork_config::update_config(&self.root, |config| {
            let mut group = config
                .mesh
                .group
                .clone()
                .context("mesh_membership_missing")?;
            ensure!(!group.contains(&device.origin), "device_is_a_station");
            if !group.clients.iter().any(|c| c == &device) {
                group.clients.retain(|c| c.origin != device.origin);
                group.clients.push(device);
                group.revision += 1;
                group.apply(service.origin(), &mut config.mesh)?;
            }
            Ok(group)
        })?;
        service.refresh(state).await?;
        self.broadcast(state, &group).await;
        Ok(json!({"registered":true,"group":group}))
    }

    pub async fn rename_device(&self, state: &AppState, origin: &str, name: &str) -> Result<Value> {
        let name = zork_config::membership::validate_device_name(name)?;
        let service = state.mesh.get().context("mesh_not_ready")?;
        let group = zork_config::load_config(&self.root)?
            .mesh
            .group
            .context("mesh_membership_missing")?;
        if group.authority != service.origin() {
            ensure!(origin == service.origin(), "can_only_rename_own_device");
            let result = service
                .membership_call(&group.authority, "rename_device", json!({"name":name}))
                .await?;
            let updated: MeshGroup = serde_json::from_value(result["group"].clone())?;
            apply_group(state, &group.authority, updated).await?;
            return Ok(result);
        }
        let _guard = self.transaction.lock().await;
        let group = zork_config::update_config(&self.root, |config| {
            let mut group = config
                .mesh
                .group
                .clone()
                .context("mesh_membership_missing")?;
            let member = group
                .members
                .iter_mut()
                .find(|m| m.origin == origin)
                .context("mesh_member_required")?;
            if member.name != name {
                member.name = name.clone();
                group.revision += 1;
                group.apply(service.origin(), &mut config.mesh)?;
            }
            Ok(group)
        })?;
        service.refresh(state).await?;
        self.broadcast(state, &group).await;
        Ok(json!({"name":name,"group":group}))
    }

    async fn broadcast(&self, state: &AppState, group: &MeshGroup) {
        let Some(service) = state.mesh.get() else {
            return;
        };
        let requests = group
            .members
            .iter()
            .filter(|m| m.origin != service.origin())
            .map(|member| async move {
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(3),
                    service.membership_call(
                        &member.origin,
                        "apply_membership",
                        json!({"group":group}),
                    ),
                )
                .await;
            });
        futures_util::future::join_all(requests).await;
    }
}

fn enrolled(group: &MeshGroup, origin: &str) -> bool {
    group.contains(origin)
}

pub fn own_device(root: &Path, origin: &str, node: &MeshNode) -> Result<MeshDevice> {
    let config = zork_config::load_config(root)?.mesh;
    let addr = config.bind.filter(|s| {
        s.parse::<std::net::SocketAddr>()
            .is_ok_and(|a| a.port() != 0 && !a.ip().is_unspecified())
    });
    Ok(MeshDevice {
        routes: Some(node.routes()?),
        origin: origin.into(),
        name: if config.name.trim().is_empty() {
            zork_config::device_name()
        } else {
            config.name
        },
        addr,
    })
}

pub async fn apply_group(state: &AppState, sender: &str, group: MeshGroup) -> Result<Value> {
    let service = state.mesh.get().context("mesh_not_ready")?;
    let changed = zork_config::update_config(&state.config.data_root, |config| {
        ensure!(
            config
                .mesh
                .group
                .as_ref()
                .is_some_and(|g| g.authority == sender && group.authority == sender),
            "mesh_authority_required"
        );
        group.apply(service.origin(), &mut config.mesh)
    })?;
    if changed {
        service.refresh(state).await?;
    }
    Ok(json!({"applied":changed}))
}

pub fn start(service: Arc<EnrollmentService>, state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let handler = service.clone();
        let serve = service.transport.serve(move |endpoint, request| {
            let state = state.clone();
            let handler = handler.clone();
            async move { handler.begin(&state, endpoint, request).await }
        });
        // Invitations expire silently; release the relay once the last one has.
        let expiry = async {
            loop {
                let wait = match service.sync_invite_relay().await {
                    Some(expires_at) => expires_at.saturating_sub(enrollment::now()) + 1,
                    None => 60,
                };
                tokio::time::sleep(std::time::Duration::from_secs(wait.clamp(1, 60))).await;
            }
        };
        tokio::select! {
            result = serve => if let Err(error) = result {
                tracing::error!(%error,"Enrollment listener stopped");
            },
            () = expiry => {}
        }
    })
}

pub(crate) fn private_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    use std::io::Write;
    let parent = path.parent().context("state directory")?;
    std::fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    let temp = path.with_extension("tmp");
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    file.write_all(&serde_json::to_vec(value)?)?;
    file.sync_all()?;
    std::fs::rename(temp, path)?;
    Ok(())
}
