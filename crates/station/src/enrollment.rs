//! Station-owned invitations and personal mesh membership. No model secrets
//! leave this node. Enrollment is finalized by the joining Synch device key.
use crate::state::AppState;
use anyhow::{ensure, Context, Result};
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
    enrollment::{self, Invitation, InviteKind},
    node::MeshNode,
};

pub struct EnrollmentService {
    pub transport: Enrollment,
    root: PathBuf,
    node: MeshNode,
    origin: String,
    transaction: tokio::sync::Mutex<()>,
    join_transaction: tokio::sync::Mutex<()>,
    pending: std::sync::Mutex<std::collections::HashMap<String, InviteRecord>>,
}

#[derive(Serialize, Deserialize, Clone)]
struct InviteRecord {
    #[serde(default)]
    ephemeral: bool,
    #[serde(default)]
    kind: InviteKind,
    id: String,
    secret_hash: String,
    expires_at: u64,
    revoked: bool,
    claim: Option<Claim>,
    used_by: Option<String>,
    #[serde(default)]
    committed: bool,
}
#[derive(Serialize, Deserialize, Clone)]
struct Claim {
    #[serde(default)]
    verified: bool,
    #[serde(default)]
    approved: bool,
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
            pending: Default::default(),
        })
    }
    fn path(&self, id: &str) -> Result<PathBuf> {
        ensure!(
            ulid::Ulid::from_string(id).is_ok_and(|value| value.to_string() == id)
                || (id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())),
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
        if record.ephemeral && record.used_by.is_none() {
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

    pub async fn create(&self, state: &AppState) -> Result<Value> {
        self.create_kind(state, InviteKind::Station).await
    }

    pub async fn create_kind(&self, state: &AppState, kind: InviteKind) -> Result<Value> {
        let service = state.mesh.get().context("mesh_not_ready")?;
        if let Some(group) = zork_config::load_config(&self.root)?.mesh.group {
            ensure!(group.contains(service.origin()), "device_removed_from_mesh");
            if group.authority != service.origin() {
                return service
                    .membership_call(&group.authority, "create_invite", json!({"kind":kind}))
                    .await;
            }
        }
        let _guard = self.transaction.lock().await;
        let device = own_device(&self.root, service.origin())?;
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
        let short = enrollment::ticket::Ticket::new(kind, self.transport.address().await, &config)?;
        let id = short.id();
        let secret = short.secret();
        let expires_at = enrollment::now() + enrollment::INVITE_SECONDS;
        let record = InviteRecord {
            ephemeral: true,
            kind,
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
        if kind == InviteKind::Client {
            state.db.realtime.notify(crate::realtime::MESH);
            return Ok(
                json!({"id":id,"expires_at":expires_at,"invitation":ticket,"scope":"client","permissions":["node_management"]}),
            );
        }
        state.db.realtime.notify(crate::realtime::MESH);
        // Pin bootstrap and binaries to the release that generated the invitation.
        let package: Value = serde_json::from_str(include_str!("../../../package.json"))?;
        let version = package["version"].as_str().context("release version")?;
        let command = format!(
            "curl -fsSL {}/download/v{version}/install.sh | sh -s -- --version {version} -- mesh join '{}'",
            zork_config::update::RELEASE_BASE,
            ticket
        );
        Ok(
            json!({"id":id,"expires_at":expires_at,"invitation":ticket,"command":command,"scope":"personal_mesh","permissions":["collaborate","task_files","node_management"]}),
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
            json!({"items":self.records()?.into_iter().map(|r|json!({"id":r.id,"kind":r.kind,"expires_at":r.expires_at,"claim_id":r.claim.as_ref().map(|c|zork_mesh::content_root(c.challenge.as_bytes())),"status":if r.revoked {"revoked"}else if r.committed{"joined"}else if r.expires_at<=enrollment::now(){"expired"}else if r.kind==InviteKind::Client && r.claim.as_ref().is_some_and(|c|c.verified && !c.approved){"awaiting_approval"}else if r.claim.is_some(){"connecting"}else{"waiting"},"device":r.claim.map(|c|c.device),"origin":r.used_by})).collect::<Vec<_>>()}),
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
        if let Some(claim) = record.claim {
            self.clear_provisional(&claim.device.origin).await?;
        }
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
            kind: InviteKind,
        }
        let request: Resolve = serde_json::from_value(request)?;
        ensure!(request.op == "resolve", "invalid_invite_operation");
        let _guard = self.transaction.lock().await;
        let record = self
            .load(&request.id)
            .context("invite_expired_or_restart")?;
        ensure!(
            !record.revoked
                && record.kind == request.kind
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
                    .is_some_and(|g| !record.committed || enrolled(g, record.kind, origin)),
                "device_removed_from_mesh"
            );
        }
        let invitation = Invitation {
            kind: record.kind,
            version: 1,
            id: record.id,
            secret: request.secret,
            endpoint: self.transport.address().await,
            device: own_device(&self.root, service.origin())?,
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
                origin == &request.device.origin
                    && (!record.committed || enrolled(&group, record.kind, origin)),
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
                (if record.kind == InviteKind::Client {
                    group.clients.len()
                } else {
                    group.members.len()
                }) < 16
                    || enrolled(&group, record.kind, &request.device.origin),
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
                self.clear_provisional(&claim.device.origin).await?;
                record.claim = None;
            }
        }
        if record.claim.is_none() {
            record.claim = Some(Claim {
                verified: false,
                approved: false,
                endpoint,
                device: request.device,
                challenge: enrollment::secret(),
                expires_at: if record.kind == InviteKind::Client {
                    record.expires_at
                } else {
                    (enrollment::now() + 60).min(record.expires_at)
                },
            });
            self.save(&record)?;
        }
        let claim = record.claim.as_ref().unwrap();
        if !record.committed {
            // Only the fixed enrollment-capable bridge is reachable before confirmation.
            self.node
                .grant_delegation(
                    &claim.device.origin[4..],
                    vec!["zork-control".into()],
                    std::time::Duration::from_secs(
                        claim.expires_at.saturating_sub(enrollment::now()).max(1),
                    ),
                    "Zork invitation handshake",
                )
                .await?;
        }
        Ok(
            json!({"challenge":claim.challenge,"origin":service.origin(),"address":self.node.address()?,"claim_watch":1}),
        )
    }

    /// Observe only the claim already proved by this Synch identity. This does
    /// not grant membership or execute confirm_join during a reconnect.
    fn claim_status(
        &self,
        origin: &str,
        id: &str,
        secret: &str,
        challenge: &str,
    ) -> Result<(Value, Option<u64>)> {
        let record = self.load(id).context("invite_expired_or_restart")?;
        ensure!(
            !record.revoked && zork_mesh::content_root(secret.as_bytes()) == record.secret_hash,
            "invalid_or_revoked_invite"
        );
        ensure!(record.kind == InviteKind::Client, "invalid_invitation_kind");
        let claim = record.claim.as_ref().context("invite_not_claimed")?;
        ensure!(
            claim.verified && claim.device.origin == origin && claim.challenge == challenge,
            "invite_device_proof_mismatch"
        );
        let config = zork_config::load_config(&self.root)?.mesh;
        let group = config.group.context("mesh_membership_missing")?;
        ensure!(group.authority == self.origin, "invite_authority_changed");
        if record.committed {
            ensure!(
                enrolled(&group, record.kind, origin),
                "device_removed_from_mesh"
            );
        }
        let expires = record
            .used_by
            .is_none()
            .then_some(record.expires_at.min(claim.expires_at));
        ensure!(
            expires.is_none_or(|at| at > enrollment::now()),
            "invite_expired"
        );
        Ok((
            json!({"id":id,"status":if record.committed {"joined"} else if claim.approved {"approved"} else {"awaiting_approval"},"expires_at":expires}),
            expires,
        ))
    }

    /// Called only after Synch authenticated `origin`; no caller-supplied identity.
    pub async fn confirm(
        &self,
        state: &AppState,
        origin: &str,
        id: &str,
        secret: &str,
        challenge: &str,
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
        let service = state.mesh.get().context("mesh_not_ready")?;
        let mut config = zork_config::load_config(&self.root)?.mesh;
        let mut group = config.group.take().context("mesh_membership_missing")?;
        ensure!(
            group.authority == service.origin(),
            "invite_authority_changed"
        );
        if let Some(used) = &record.used_by {
            ensure!(
                used == origin && (!record.committed || enrolled(&group, record.kind, origin)),
                "device_removed_from_mesh"
            );
        }
        if !record.committed {
            if record.kind == InviteKind::Client {
                ensure!(
                    record.used_by.is_some()
                        || (record.expires_at > enrollment::now()
                            && claim.expires_at > enrollment::now()),
                    "invite_expired"
                );
                if !claim.verified {
                    record.claim.as_mut().unwrap().verified = true;
                    self.save(&record)?;
                    state.db.realtime.notify(crate::realtime::MESH);
                }
                if !claim.approved {
                    return Ok(json!({"joined":false,"status":"awaiting_approval"}));
                }
            }
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
            if !enrolled(&group, record.kind, origin) {
                if record.kind == InviteKind::Client {
                    ensure!(!group.contains(origin), "device_is_a_station");
                    group.clients.push(device);
                } else {
                    ensure!(
                        !group.clients.iter().any(|c| c.origin == origin),
                        "device_is_a_client"
                    );
                    group.members.push(device);
                }
                group.revision += 1;
            }
            group.validate()?;
            zork_config::update_config(&self.root, |config| {
                group.apply(service.origin(), &mut config.mesh)?;
                Ok(())
            })?;
            record.committed = true;
            self.save(&record)?;
        }
        service.refresh(state).await?;
        self.clear_provisional(origin).await?;
        state
            .db
            .realtime
            .notify(crate::realtime::MESH | crate::realtime::WORK);
        Ok(json!({"joined":true,"group":group}))
    }

    async fn clear_provisional(&self, origin: &str) -> Result<()> {
        self.node.revoke_delegation(&origin[4..]).await?;
        Ok(())
    }

    pub async fn join(&self, state: &AppState, ticket: &str, name: Option<&str>) -> Result<Value> {
        let _guard = self
            .join_transaction
            .try_lock()
            .context("another_join_is_in_progress")?;
        let invitation = zork_client_core::transport::resolve_invitation(
            &self.root,
            ticket,
            InviteKind::Station,
        )
        .await?;
        ensure!(
            invitation.kind == InviteKind::Station,
            "client_invite_requires_client"
        );
        let service = state.mesh.get().context("mesh_not_ready")?;
        let before = zork_config::load_config(&self.root)?.mesh;
        if let Some(group) = &before.group {
            ensure!(
                group.authority == invitation.device.origin,
                "already_in_another_mesh"
            );
            if group.contains(service.origin()) {
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
        ensure!(
            service.origin() != invitation.device.origin,
            "cannot_join_this_device_to_itself"
        );
        let mut device = own_device(&self.root, service.origin())?;
        if let Some(name) = name {
            device.name = name.trim().into();
            device.validate()?;
        }
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
        let result=async {
            let reply=control.exchange(&invitation.device.origin,&json!({"v":1,"request":{"kind":"confirm_join","id":invitation.id,"secret":invitation.secret,"challenge":begun["challenge"]}})).await?;
            ensure!(reply["ok"]==true,"{}",reply["error"].as_str().unwrap_or("join_confirmation_failed"));
            let group:MeshGroup=serde_json::from_value(reply["data"]["group"].clone())?;
            ensure!(group.authority==invitation.device.origin && group.contains(service.origin()),"invalid_join_membership");
            zork_config::update_config(&self.root,|config|{group.apply(service.origin(),&mut config.mesh)?;config.mesh.name=device.name.clone();Ok(())})?;
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

    pub async fn approve_client(
        &self,
        state: &AppState,
        id: &str,
        origin: &str,
        claim_id: &str,
    ) -> Result<Value> {
        let service = state.mesh.get().context("mesh_not_ready")?;
        let group = zork_config::load_config(&self.root)?
            .mesh
            .group
            .context("mesh_membership_missing")?;
        if group.authority != service.origin() {
            return service
                .membership_call(
                    &group.authority,
                    "approve_client_invite",
                    json!({"id":id,"origin":origin,"claim_id":claim_id}),
                )
                .await;
        }
        let _guard = self.transaction.lock().await;
        let mut record = self.load(id)?;
        ensure!(
            record.kind == InviteKind::Client
                && !record.revoked
                && record.expires_at > enrollment::now()
                && record.used_by.is_none(),
            "client_invite_not_active"
        );
        let claim = record.claim.as_mut().context("invite_not_claimed")?;
        ensure!(
            claim.verified
                && claim.device.origin == origin
                && zork_mesh::content_root(claim.challenge.as_bytes()) == claim_id
                && claim.expires_at > enrollment::now(),
            "invite_claim_changed"
        );
        claim.approved = true;
        self.save(&record)?;
        state.db.realtime.notify(crate::realtime::MESH);
        Ok(json!({"approved":true}))
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

fn enrolled(group: &MeshGroup, kind: InviteKind, origin: &str) -> bool {
    match kind {
        InviteKind::Station => group.contains(origin),
        InviteKind::Client => group.clients.iter().any(|c| c.origin == origin),
    }
}

pub fn own_device(root: &Path, origin: &str) -> Result<MeshDevice> {
    let config = zork_config::load_config(root)?.mesh;
    let addr = config.bind.filter(|s| {
        s.parse::<std::net::SocketAddr>()
            .is_ok_and(|a| a.port() != 0 && !a.ip().is_unspecified())
    });
    Ok(MeshDevice {
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
        if let Err(error) = service
            .transport
            .serve(move |endpoint, request| {
                let state = state.clone();
                let handler = handler.clone();
                async move { handler.begin(&state, endpoint, request).await }
            })
            .await
        {
            tracing::error!(%error,"Enrollment listener stopped");
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

#[cfg(test)]
mod invitation_storage_tests {
    use super::*;
    #[tokio::test]
    async fn claim_observation_is_bound_to_proof_authority_and_membership() {
        let root = tempfile::tempdir().unwrap();
        zork_config::ensure_layout(root.path()).unwrap();
        let mesh = MeshConfig {
            offline: true,
            bind: Some("127.0.0.1:0".into()),
            ..Default::default()
        };
        let mut runtime = zork_mesh::managed::start(root.path(), &mesh).await.unwrap();
        let service = EnrollmentService::new(root.path(), &mesh, runtime.node())
            .await
            .unwrap();
        let device = MeshDevice {
            origin: format!("key:{}", "y".repeat(52)),
            name: "Phone".into(),
            addr: None,
        };
        zork_config::update_config(root.path(), |config| {
            config.mesh.group = Some(MeshGroup {
                authority: service.origin.clone(),
                revision: 1,
                members: vec![MeshDevice {
                    origin: service.origin.clone(),
                    name: "Authority".into(),
                    addr: None,
                }],
                clients: vec![],
            });
            Ok(())
        })
        .unwrap();
        let secret = "a".repeat(64);
        let mut record = InviteRecord {
            ephemeral: true,
            kind: InviteKind::Client,
            id: "a".repeat(32),
            secret_hash: zork_mesh::content_root(secret.as_bytes()),
            expires_at: enrollment::now() + 900,
            revoked: false,
            used_by: None,
            committed: false,
            claim: Some(Claim {
                verified: true,
                approved: false,
                endpoint: "127.0.0.1:1".into(),
                device: device.clone(),
                challenge: "proof".into(),
                expires_at: enrollment::now() + 900,
            }),
        };
        service.save(&record).unwrap();
        let read = || service.claim_status(&device.origin, &record.id, &secret, "proof");
        assert_eq!(read().unwrap().0["status"], "awaiting_approval");
        assert!(service
            .claim_status(&service.origin, &record.id, &secret, "proof")
            .is_err());
        assert!(service
            .claim_status(&device.origin, &record.id, "wrong", "proof")
            .is_err());
        assert!(service
            .claim_status(&device.origin, &record.id, &secret, "wrong")
            .is_err());
        record.claim.as_mut().unwrap().approved = true;
        service.save(&record).unwrap();
        assert_eq!(read().unwrap().0["status"], "approved");
        record.expires_at = enrollment::now() - 1;
        service.save(&record).unwrap();
        assert_eq!(read().unwrap_err().to_string(), "invite_expired");
        record.used_by = Some(device.origin.clone());
        record.committed = true;
        service.save(&record).unwrap();
        assert_eq!(read().unwrap_err().to_string(), "device_removed_from_mesh");
        zork_config::update_config(root.path(), |config| {
            config
                .mesh
                .group
                .as_mut()
                .unwrap()
                .clients
                .push(device.clone());
            Ok(())
        })
        .unwrap();
        let joined = read().unwrap();
        assert_eq!(joined.0["status"], "joined");
        assert_eq!(joined.1, None);
        record.revoked = true;
        service.save(&record).unwrap();
        assert_eq!(read().unwrap_err().to_string(), "invalid_or_revoked_invite");
        record.revoked = false;
        service.save(&record).unwrap();
        zork_config::update_config(root.path(), |config| {
            let group = config.mesh.group.as_mut().unwrap();
            group.authority = device.origin.clone();
            group.clients.clear();
            group.members.push(device.clone());
            Ok(())
        })
        .unwrap();
        assert_eq!(read().unwrap_err().to_string(), "invite_authority_changed");
        service.transport.close().await;
        runtime.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn pending_is_memory_only_but_committed_receipts_and_legacy_survive_restart() {
        let root = std::env::temp_dir().join(format!("zork-invite-storage-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&root).unwrap();
        let config = MeshConfig {
            offline: true,
            bind: Some("127.0.0.1:0".into()),
            ..Default::default()
        };
        let mut runtime = zork_mesh::managed::start(&root, &config).await.unwrap();
        let make = || runtime.node();
        let service = EnrollmentService::new(&root, &config, make())
            .await
            .unwrap();
        let mut pending = InviteRecord {
            ephemeral: true,
            kind: InviteKind::Client,
            id: "a".repeat(32),
            secret_hash: "test".into(),
            expires_at: enrollment::now() + 900,
            revoked: false,
            claim: None,
            used_by: None,
            committed: false,
        };
        service.save(&pending).unwrap();
        assert!(!service.path(&pending.id).unwrap().exists());
        assert!(service.load(&pending.id).is_ok());
        let missing_id = pending.id.clone();
        pending.id = "b".repeat(32);
        pending.used_by = Some(format!("key:{}", "y".repeat(52)));
        pending.committed = true;
        service.save(&pending).unwrap();
        let mut legacy = pending.clone();
        legacy.id = "c".repeat(32);
        legacy.ephemeral = false;
        legacy.used_by = None;
        legacy.committed = false;
        service.save(&legacy).unwrap();
        service.transport.close().await;
        drop(service);
        let restarted = EnrollmentService::new(&root, &config, make())
            .await
            .unwrap();
        assert!(restarted.load(&missing_id).is_err());
        assert!(restarted.load(&pending.id).unwrap().committed);
        assert!(!restarted.load(&legacy.id).unwrap().ephemeral);
        restarted.transport.close().await;
        runtime.shutdown().await.unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }
}

pub(crate) fn subscribe_claim(
    state: AppState,
    origin: String,
    id: String,
    secret: String,
    challenge: String,
) -> Result<tokio::sync::mpsc::Receiver<Value>> {
    let changes = state.db.realtime.listen(crate::realtime::MESH);
    let enrollment = state
        .mesh
        .get()
        .context("mesh_not_ready")?
        .enrollment
        .clone();
    let (_, expires) = enrollment.claim_status(&origin, &id, &secret, &challenge)?;
    let source = ClaimSource {
        enrollment,
        origin,
        id,
        secret,
        challenge,
        expires: expires.map(|at| {
            tokio::time::Instant::now()
                + std::time::Duration::from_secs(at.saturating_sub(enrollment::now()))
        }),
    };
    Ok(zork_notify::stream::spawn(
        source,
        changes,
        4,
        Some(zork_mesh::feed::HEARTBEAT),
        |event| {
            use zork_notify::stream::Event;
            Some(match event {
                Event::Data(data) => json!({"v":1,"ok":true,"data":data}),
                Event::Heartbeat => json!({"v":1,"ok":true,"heartbeat":true}),
                Event::Error(error) => json!({"v":1,"ok":false,"error":error.to_string()}),
            })
        },
    ))
}
struct ClaimSource {
    enrollment: Arc<EnrollmentService>,
    origin: String,
    id: String,
    secret: String,
    challenge: String,
    expires: Option<tokio::time::Instant>,
}
impl zork_notify::stream::Source for ClaimSource {
    type Item = Value;
    type Error = anyhow::Error;
    fn check_access(&self) -> Result<()> {
        self.enrollment
            .claim_status(&self.origin, &self.id, &self.secret, &self.challenge)
            .map(|_| ())
    }
    fn expiry(&self) -> Option<(tokio::time::Instant, Self::Error)> {
        self.expires
            .map(|at| (at, anyhow::anyhow!("invite_expired")))
    }
    async fn read(&mut self) -> Result<zork_notify::stream::Page<Value>> {
        let (status, _) =
            self.enrollment
                .claim_status(&self.origin, &self.id, &self.secret, &self.challenge)?;
        let done = status["status"] != "awaiting_approval";
        let mut page = zork_notify::stream::Page::snapshot(status);
        page.done = done;
        Ok(page)
    }
}
