//! Mesh configuration and invitation lifecycles, independent of the settings view.
use super::{Observable, Subscription};
use crate::api::{MeshPeer, StationClient};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone, Default, PartialEq)]
pub struct MeshAdminData {
    pub config: Option<zork_config::MeshConfig>,
    pub origin: Option<String>,
    pub peers: Vec<MeshPeer>,
    pub invitations: [Option<Value>; 2],
    pub busy: bool,
    pub message: Option<String>,
    pub saved: u64,
}
pub enum MeshAction {
    Refresh,
    Enable(bool),
    AddPeer {
        origin: String,
        name: String,
        address: String,
        client: bool,
    },
    RemovePeer(String),
    CreateInvite(bool),
    ApproveInvite(bool),
    RevokeInvite(bool),
}
pub struct MeshAdmin {
    client: Arc<StationClient>,
    device: std::sync::Weak<super::Device>,
    owned: Mutex<MeshAdminData>,
    state: Observable<MeshAdminData>,
    watcher: Mutex<Option<zork_notify::Task<()>>>,
}
impl MeshAdmin {
    pub(super) fn new(
        client: Arc<StationClient>,
        device: std::sync::Weak<super::Device>,
    ) -> Arc<Self> {
        let invitations = device
            .upgrade()
            .and_then(|d| d.cache.clone())
            .and_then(|(store, peer)| {
                store
                    .get::<[Option<Value>; 2]>(&peer, "mesh-admin-invitations")
                    .ok()
                    .flatten()
            })
            .unwrap_or_default();
        let initial = MeshAdminData {
            invitations,
            ..Default::default()
        };
        let source = Arc::new(Self {
            client,
            device,
            owned: Mutex::new(initial.clone()),
            state: Observable::new(initial),
            watcher: Mutex::new(None),
        });
        if source
            .snapshot()
            .invitations
            .iter()
            .flatten()
            .any(|invite| !finished(invite))
        {
            source.watch();
        }
        source
    }
    pub fn peer_status(&self, origin: &str) -> zork_client_types::device::DeviceStatus {
        let device = self.device.upgrade().map(|device| device.snapshot());
        crate::device_status::project(
            device
                .as_ref()
                .and_then(|device| device.mesh_readiness.as_ref()),
            self.snapshot()
                .peers
                .iter()
                .find(|peer| peer.origin == origin)
                .map(|peer| peer.online),
            &Default::default(),
            false,
        )
    }
    pub fn subscribe(&self) -> Subscription<MeshAdminData> {
        self.state.subscribe()
    }
    pub fn snapshot(&self) -> Arc<MeshAdminData> {
        self.state.read()
    }
    pub fn invitation_view(&self) -> Value {
        let state = self.snapshot();
        let available = state.config.is_some();
        let project = |invite: &Option<Value>| {
            let mut value = invite.clone().unwrap_or_else(|| json!({}));
            let active = invite.as_ref().is_some_and(|i| !finished(i));
            value["can_create"] = json!(available && !state.busy && !active);
            value["can_revoke"] = json!(available && !state.busy && active);
            value["can_approve"] =
                json!(available && !state.busy && value["status"] == "awaiting_approval");
            value
        };
        json!({"available":available,"busy":state.busy,"error":state.message,
            "node":project(&state.invitations[0]),"phone":project(&state.invitations[1])})
    }
    fn commit(&self, change: impl FnOnce(&mut MeshAdminData)) {
        let mut s = self.owned.lock().unwrap();
        let previous = s.invitations.clone();
        change(&mut s);
        for invitation in s.invitations.iter_mut().flatten() {
            if finished(invitation) {
                if let Some(fields) = invitation.as_object_mut() {
                    fields.remove("invitation");
                    fields.remove("command");
                }
            }
        }
        if previous != s.invitations {
            if let Some((store, peer)) = self.device.upgrade().and_then(|d| d.cache.clone()) {
                if let Err(error) =
                    store.put_authorized_settings(&peer, "mesh-admin-invitations", &s.invitations)
                {
                    s.message = Some(format!("邀请状态未能保存：{error}"));
                }
            }
        }
        self.state.publish(s.clone());
    }
    pub fn dispatch(self: &Arc<Self>, action: MeshAction) {
        {
            let mut s = self.owned.lock().unwrap();
            if s.busy {
                return;
            }
            s.busy = true;
            s.message = None;
            self.state.publish(s.clone());
        }
        let source = self.clone();
        self.client.spawn(async move {
            let result = source.apply(action).await;
            source.commit(|s| {
                s.busy = false;
                if let Err(error) = result {
                    s.message = Some(error.to_string());
                }
            });
        });
    }
    async fn current(&self) -> Result<zork_config::MeshConfig> {
        let value = self
            .client
            .node_request(http::Method::GET, "/v1/node/mesh".into(), None)
            .await?;
        let config: zork_config::MeshConfig = serde_json::from_value(value["config"].clone())?;
        self.commit(|s| {
            s.config = Some(config.clone());
            s.origin = value["origin"].as_str().map(str::to_owned);
        });
        Ok(config)
    }
    async fn refresh(&self) -> Result<()> {
        self.current().await?;
        if let Ok(status) = self.client.mesh_status().await {
            self.commit(|s| s.peers = status.peers);
        }
        Ok(())
    }
    async fn save(&self, config: zork_config::MeshConfig) -> Result<()> {
        self.client
            .node_request(
                http::Method::PUT,
                "/v1/node/mesh".into(),
                Some(json!(config)),
            )
            .await?;
        self.refresh().await?;
        self.commit(|s| {
            s.saved = s.saved.wrapping_add(1);
            s.message = Some("设备连接已更新，运行中的任务继续执行。".into());
        });
        Ok(())
    }
    async fn apply(self: &Arc<Self>, action: MeshAction) -> Result<()> {
        match action {
            MeshAction::Refresh => self.refresh().await,
            MeshAction::Enable(enabled) => {
                let mut config = self.current().await?;
                config.enabled = enabled;
                self.save(config).await
            }
            MeshAction::AddPeer {
                origin,
                name,
                address,
                client,
            } => {
                let input = crate::device_edit::validate_peer(&name, &origin, &address)
                    .map_err(anyhow::Error::msg)?;
                let mut config = self.current().await?;
                config.peers.retain(|p| p.origin != input.origin);
                config.peers.push(zork_config::MeshPeer {
                    routes: None,
                    origin: input.origin,
                    name: input.name,
                    addr: input.address,
                    execute: vec![],
                    client,
                    collaborate: false,
                });
                config.enabled = true;
                self.save(config).await
            }
            MeshAction::RemovePeer(origin) => {
                let mut config = self.current().await?;
                if config.group.as_ref().is_some_and(|g| {
                    g.members
                        .iter()
                        .chain(&g.clients)
                        .any(|m| m.origin == origin)
                }) {
                    self.client
                        .node_request(
                            http::Method::POST,
                            "/v1/node/mesh/members/remove".into(),
                            Some(json!({"origin":origin})),
                        )
                        .await?;
                    self.refresh().await
                } else {
                    config.peers.retain(|p| p.origin != origin);
                    self.save(config).await
                }
            }
            MeshAction::CreateInvite(phone) => {
                let mut config = self.current().await?;
                if !config.enabled {
                    config.enabled = true;
                    self.save(config).await?;
                }
                let device = self.device.upgrade().context("设备连接已关闭")?;
                let mut updates = device.subscribe();
                device.start();
                tokio::time::timeout(Duration::from_secs(30), async {
                    loop {
                        if updates.snapshot().state.mesh.origin.is_some() {
                            return Ok::<_, anyhow::Error>(());
                        }
                        updates.changed().await.context("设备连接已关闭")?;
                    }
                })
                .await
                .context("Mesh 启动超时")??;
                self.current().await?;
                let path = if phone {
                    "/v1/node/mesh/client-invites"
                } else {
                    "/v1/node/mesh/invites"
                };
                let mut invitation = self
                    .client
                    .node_request(http::Method::POST, path.into(), None)
                    .await?;
                invitation["status"] = json!("waiting");
                self.commit(|s| {
                    s.invitations[usize::from(phone)] = Some(invitation);
                    s.message = None;
                });
                self.watch();
                self.refresh().await
            }
            MeshAction::ApproveInvite(phone) => {
                let snapshot = self.snapshot();
                let invite = snapshot.invitations[usize::from(phone)]
                    .as_ref()
                    .context("没有正在进行的邀请")?;
                anyhow::ensure!(
                    invite["status"] == "awaiting_approval",
                    "邀请已变化，请刷新后重试"
                );
                let id = invite["id"].as_str().context("邀请缺少 ID")?;
                self.client.node_request(http::Method::POST,format!("/v1/node/mesh/invites/{id}/approve"),Some(json!({"origin":invite["device"]["origin"],"claim_id":invite["claim_id"]}))).await?;
                self.commit(|s| {
                    if let Some(invite) = &mut s.invitations[usize::from(phone)] {
                        invite["status"] = json!("connecting");
                    }
                });
                Ok(())
            }
            MeshAction::RevokeInvite(phone) => {
                let snapshot = self.snapshot();
                let invite = snapshot.invitations[usize::from(phone)]
                    .as_ref()
                    .context("没有正在进行的邀请")?;
                let id = invite["id"].as_str().context("邀请缺少 ID")?;
                self.client
                    .node_request(
                        http::Method::DELETE,
                        format!("/v1/node/mesh/invites/{id}"),
                        None,
                    )
                    .await?;
                self.commit(|s| {
                    if let Some(invite) = &mut s.invitations[usize::from(phone)] {
                        invite["status"] = json!("revoked");
                    }
                });
                self.watch();
                Ok(())
            }
        }
    }
    fn watch(self: &Arc<Self>) {
        let mut watcher = self.watcher.lock().unwrap();
        watcher.take();
        let Some(device) = self.device.upgrade() else {
            return;
        };
        let mut changes = device.subscribe();
        let weak = Arc::downgrade(self);
        let client = self.client.clone();
        *watcher = Some(zork_notify::Task(self.client.spawn(async move {
            let mut retry = zork_notify::retry::Retry::default();
            loop {
                let online = changes.snapshot().state.online;
                let Some(source) = weak.upgrade() else { return; };
                let old = source.snapshot();
                let active: Vec<_> = old.invitations.iter().enumerate()
                    .filter_map(|(index, invite)| invite.as_ref().filter(|value| !finished(value)).map(|value| (index, value.clone())))
                    .collect();
                if active.is_empty() { return; }
                // Both invitation kinds share one authority/list request.
                let response = client.node_request(http::Method::GET, "/v1/node/mesh/invites".into(), None).await;
                let failed = response.is_err();
                source.commit(|state| {
                    for (index, mut next) in active {
                        let id = next["id"].clone();
                        if let Ok(value) = &response {
                            if let Some(status) = value["items"].as_array().and_then(|items| items.iter().find(|item| item["id"] == id)) {
                                for field in ["status", "device", "claim_id"] { next[field] = status[field].clone(); }
                            } else if value["items"].is_array() { next["status"] = json!("expired"); }
                        }
                        if next["expires_at"].as_u64().is_some_and(|at| at <= zork_mesh::enrollment::now())
                            && !matches!(next["status"].as_str(), Some("joined" | "revoked")) {
                            next["status"] = json!("expired");
                        }
                        if state.invitations[index].as_ref().is_some_and(|current| current["id"] == id && !finished(current)) {
                            state.invitations[index] = Some(next);
                        }
                    }
                });
                let current = source.snapshot();
                if current.invitations.iter().flatten().all(finished) {
                    let _ = source.refresh().await;
                    return;
                }
                let remaining = current.invitations.iter().flatten().filter(|value| !finished(value))
                    .filter_map(|value| value["expires_at"].as_u64())
                    .map(|at| at.saturating_sub(zork_mesh::enrollment::now())).min();
                drop(source);
                if !failed { retry.reset(); }
                let mesh_changed = async {
                    loop {
                        let update = changes.changed().await?;
                        if update.domains.contains(super::Domains::MESH) || update.state.online != online { return Some(()); }
                    }
                };
                tokio::select! {
                    changed = mesh_changed => if changed.is_none() { return; },
                    _ = async { if let Some(seconds) = remaining { tokio::time::sleep(Duration::from_secs(seconds)).await } else { std::future::pending().await } } => {},
                    _ = retry.wait(), if failed => {},
                }
            }
        })));
    }
    pub fn remaining(&self, phone: bool) -> Option<u64> {
        self.snapshot().invitations[usize::from(phone)]
            .as_ref()?
            .get("expires_at")?
            .as_u64()
            .map(|at| at.saturating_sub(zork_mesh::enrollment::now()))
    }
}
fn finished(invite: &Value) -> bool {
    matches!(
        invite["status"].as_str(),
        Some("joined" | "revoked" | "expired")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn invitations_survive_controller_replacement_and_finished_tickets_are_removed() {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::store::ClientStore::open(root.path()).unwrap());
        let client = Arc::new(StationClient::new("http://127.0.0.1:9", None));
        let device =
            super::super::Device::open(client.clone(), Some((store.clone(), "node".into())), false);
        let source = device.mesh_admin();
        source.commit(|s| s.invitations[1] = Some(json!({"id":"phone","status":"waiting","invitation":"private-ticket","expires_at":zork_mesh::enrollment::now()+300})));
        drop(source);
        drop(device);
        let next = super::super::Device::open(client, Some((store.clone(), "node".into())), false);
        let source = next.mesh_admin();
        assert_eq!(
            source.snapshot().invitations[1].as_ref().unwrap()["id"],
            "phone"
        );
        source.commit(|s| {
            s.config = Some(Default::default());
            s.invitations[1].as_mut().unwrap()["status"] = json!("revoked");
        });
        assert_eq!(source.invitation_view()["phone"]["can_create"], true);
        assert_eq!(source.invitation_view()["phone"]["can_revoke"], false);
        assert!(!store
            .get::<Value>("node", "mesh-admin-invitations")
            .unwrap()
            .unwrap()
            .to_string()
            .contains("private-ticket"));
        source.watcher.lock().unwrap().take();
    }

    #[tokio::test]
    async fn invitation_kinds_share_reads_and_ignore_unrelated_device_changes() {
        let reads = Arc::new(AtomicUsize::new(0));
        let count = reads.clone();
        let router = axum::Router::new().route("/v1/node/mesh/invites", axum::routing::get(move || {
            count.fetch_add(1, Ordering::SeqCst);
            async { axum::Json(json!({"items":[{"id":"node","status":"waiting"},{"id":"phone","status":"waiting"}]})) }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = Arc::new(StationClient::new(
            format!("http://{}", listener.local_addr().unwrap()),
            None,
        ));
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let device = super::super::Device::open(client, None, false);
        device.commit(|state| state.online = Some(true));
        let source = device.mesh_admin();
        source.commit(|state| {
            state.invitations = ["node", "phone"].map(|id| Some(json!({"id":id,"status":"waiting","expires_at":zork_mesh::enrollment::now()+600})));
        });
        source.watch();
        let wait_for = |expected| {
            let reads = reads.clone();
            async move {
                tokio::time::timeout(Duration::from_secs(3), async {
                    while reads.load(Ordering::SeqCst) < expected {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .unwrap();
                tokio::time::sleep(Duration::from_millis(40)).await;
                assert_eq!(reads.load(Ordering::SeqCst), expected);
            }
        };
        wait_for(1).await;
        device.commit(|state| {
            Arc::make_mut(&mut state.mesh).change_token = Some("new-authority-hint".into())
        });
        wait_for(2).await;
        device.commit(|state| state.info = Arc::new(json!({"unrelated":"metadata"})));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(reads.load(Ordering::SeqCst), 2);
        source.watcher.lock().unwrap().take();
        server.abort();
    }
}
