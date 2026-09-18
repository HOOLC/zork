//! The normalized replica feeds the existing Device/Profiles/Agents observables.
//! Legacy endpoints are used only when the owner does not advertise protocol 1.
use super::Device;
use crate::{
    api::{ApiError, SessionStatus},
    sync::Coordinator,
};
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use zork_client_types::sync::{Cursor, Scope, PROTOCOL};

#[derive(Default)]
pub(super) struct Replication {
    coordinator: tokio::sync::Mutex<Option<Arc<Coordinator>>>,
    unsupported_until: Mutex<Option<Instant>>,
    applied: Mutex<Option<Cursor>>,
    load_gate: Mutex<()>,
    active: std::sync::atomic::AtomicBool,
    catalog: Mutex<Option<Arc<crate::catalog::Catalog>>>,
}
impl Device {
    pub(crate) fn revoke_replica_access(&self) -> Result<()> {
        if let Some((store, peer)) = &self.cache {
            store.revoke_replica(peer)?;
        }
        if let Some(task) = self.delivery_task.lock().unwrap().take() {
            task.abort();
        }
        for conversation in self
            .conversations
            .lock()
            .unwrap()
            .values()
            .filter_map(std::sync::Weak::upgrade)
        {
            conversation.revoke_content();
        }
        *self.replication.applied.lock().unwrap() = None;
        *self.replication.catalog.lock().unwrap() = None;
        self.replication
            .active
            .store(false, std::sync::atomic::Ordering::Release);
        self.commit(|s| {
            s.info = Default::default();
            s.metadata_loaded = false;
            s.agents = Default::default();
            s.agents_loaded = false;
            s.sessions = Default::default();
            s.sessions_loaded = false;
            s.tasks = Default::default();
            s.chats = None;
            s.profiles = Default::default();
            s.inbox = Default::default();
            s.inbox_loaded = false;
            s.artifacts = Default::default();
            s.pages = Default::default();
            s.artifacts_loaded = false;
            s.read_markers = Default::default();
            s.online = Some(false);
            s.revoked = true;
        });
        self.profiles().accept_replica(
            Default::default(),
            Default::default(),
            Default::default(),
            false,
            Some("设备访问权限已撤销".into()),
        );
        if let Some(agents) = self.agents.get() {
            agents.seed_agents(Default::default(), false);
            agents.sync_profiles(Default::default());
        }
        Ok(())
    }
    pub(crate) fn replica_response(&self, path: &str) -> Result<Option<Value>> {
        let catalog = self.replication.catalog.lock().unwrap().clone();
        let Some(catalog) = catalog else {
            return Ok(None);
        };
        if path == "/v1/im/sessions" {
            return Ok(Some(serde_json::json!({"items":self.snapshot().sessions})));
        }
        catalog.response(path)
    }
    pub(super) fn replica_active(&self) -> bool {
        self.replication
            .active
            .load(std::sync::atomic::Ordering::Acquire)
    }
    pub(super) fn replica_caught_up(&self, target: &Cursor) -> bool {
        self.replica_active()
            && self
                .replication
                .applied
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|cursor| {
                    cursor.same_stream(target) && cursor.sequence >= target.sequence
                })
    }

    pub(crate) async fn refresh_replica_catalog(&self) -> Result<bool> {
        let Some((store, peer)) = &self.cache else {
            return Ok(false);
        };
        if self
            .replication
            .unsupported_until
            .lock()
            .unwrap()
            .is_some_and(|until| until > Instant::now())
        {
            return Ok(false);
        }
        let mut coordinator = self.replication.coordinator.lock().await;
        if coordinator.is_none() {
            let info = match self
                .client
                .node_request(reqwest::Method::GET, "/v1/node/info".into(), None)
                .await
            {
                Ok(value) => value,
                Err(ApiError::Api { status: 404, .. }) => Value::Null,
                Err(error) => {
                    if error.access_revoked() {
                        self.revoke_replica_access()?;
                    }
                    return Err(error.into());
                }
            };
            if info["sync"]["protocol"].as_u64() != Some(PROTOCOL as u64) {
                ensure!(
                    !self.replica_active(),
                    "Station no longer advertises the stored replication protocol"
                );
                *self.replication.unsupported_until.lock().unwrap() =
                    Some(Instant::now() + Duration::from_secs(60));
                return Ok(false);
            }
            let owner = info["sync"]["owner"]
                .as_str()
                .context("sync owner missing")?;
            if let Some(authenticated) = self.client.authenticated_mesh_origin() {
                ensure!(owner == authenticated, "sync_owner_mismatch");
            } else if store
                .replica_state(peer, &Scope::Catalog {})?
                .cursor
                .as_ref()
                .is_some_and(|cursor| cursor.owner != owner)
            {
                // A local authenticated Station can acquire its Mesh identity.
                // Rebind that local connection explicitly; never do this for a
                // remote key origin or reuse its prior cursor/uncertain writes.
                store.revoke_replica(peer)?;
            }
            *coordinator = Some(Coordinator::new(
                self.client.clone(),
                store.clone(),
                peer.clone(),
                owner.into(),
            )?);
        }
        let ready = coordinator.as_ref().unwrap().clone();
        // Release initialization gate; Coordinator serializes the entire batch.
        drop(coordinator);
        if let Err(error) = ready.refresh(Scope::Catalog {}).await {
            if error
                .downcast_ref::<ApiError>()
                .is_some_and(ApiError::access_revoked)
            {
                self.revoke_replica_access()?;
            }
            if !self.client.is_mesh() && error.to_string().contains("sync_owner_mismatch") {
                *self.replication.coordinator.lock().await = None;
            }
            return Err(error);
        }
        self.load_replica_catalog()?;
        self.commit(|s| {
            s.online = Some(true);
            s.revoked = false;
            s.connection_error = None;
            s.confirmed_at_ms = Some(crate::store::delivery_now_ms());
        });
        Ok(true)
    }

    pub(crate) async fn mutate_catalog(
        &self,
        action: zork_client_types::sync::Action,
        expected: Option<u64>,
    ) -> Result<Option<Value>> {
        let Some((store, peer)) = &self.cache else {
            return Ok(None);
        };
        let (kind, id) = action.entity();
        let prior = store.replica_record(peer, &Scope::Catalog {}, kind, id)?;
        if !self.refresh_replica_catalog().await? {
            return Ok(None);
        }
        let cursor = store
            .replica_state(peer, &Scope::Catalog {})?
            .cursor
            .context("catalog unavailable")?;
        let revision = expected
            .or_else(|| prior.as_ref().map(|r| r.revision))
            .or(store
                .replica_record(peer, &Scope::Catalog {}, kind, id)?
                .map(|r| r.revision))
            .context("object no longer exists")?;
        let intent = zork_client_types::sync::Mutation {
            request_id: ulid::Ulid::new().to_string(),
            owner: Some(cursor.owner),
            epoch: cursor.epoch,
            expected_revision: revision,
            action,
        };
        let coordinator = self
            .replication
            .coordinator
            .lock()
            .await
            .as_ref()
            .context("sync coordinator unavailable")?
            .clone();
        let receipt = coordinator.mutate(&intent).await?;
        *self.replication.applied.lock().unwrap() = None;
        self.load_replica_catalog()?;
        ensure!(
            receipt.outcome == zork_client_types::sync::Outcome::Applied,
            "{}",
            receipt
                .reason
                .as_deref()
                .unwrap_or("修改发生冲突，请检查最新内容后重试")
        );
        Ok(Some(
            receipt.entity.and_then(|r| r.value).unwrap_or(Value::Null),
        ))
    }

    pub(crate) async fn mutate_request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Option<Value>> {
        use zork_client_types::sync::Action;
        let parts = path.split('/').collect::<Vec<_>>();
        let body = body.cloned().unwrap_or(Value::Null);
        let action = match (method, parts.as_slice()) {
            ("PUT", ["", "v1", "node", "agents", id, "avatar"]) => Action::AgentAvatar {
                id: (*id).into(),
                avatar: body["avatar"].as_str().context("avatar required")?.into(),
            },
            ("POST", ["", "v1", "tasks", id, "transitions"]) => Action::TaskDecision {
                id: (*id).into(),
                expected_task_revision: body["expected_revision"]
                    .as_i64()
                    .context("task revision required")?,
                decision: serde_json::from_value(body["action"].clone())?,
            },
            _ => return Ok(None),
        };
        self.mutate_catalog(action, body["expected_sync_revision"].as_u64())
            .await
    }

    pub(super) fn load_replica_catalog(&self) -> Result<()> {
        let _load = self.replication.load_gate.lock().unwrap();
        let Some((store, peer)) = &self.cache else {
            return Ok(());
        };
        let current = store.replica_state(peer, &Scope::Catalog {})?.cursor;
        if current.is_none() || *self.replication.applied.lock().unwrap() == current {
            return Ok(());
        }
        let Some(catalog) = crate::catalog::Catalog::read(store, peer)? else {
            return Ok(());
        };
        let catalog = Arc::new(catalog);
        let cursor = catalog.cursor.clone();
        let mut sessions = catalog.sessions.as_ref().clone();
        let profiles = catalog.profiles.clone();
        let agents = catalog.agents.clone();
        self.replication
            .active
            .store(true, std::sync::atomic::Ordering::Release);
        self.commit(|s| {
            let presence: HashMap<_, _> = s
                .sessions
                .iter()
                .map(|old| {
                    (
                        old.session_id.as_str(),
                        (old.status, old.runtime_available, old.can_send),
                    )
                })
                .collect();
            // Presence is an expiring observation, never a restored online flag.
            for session in &mut sessions {
                if let Some((status, available, can_send)) =
                    presence.get(session.session_id.as_str())
                {
                    session.status = *status;
                    session.runtime_available = *available;
                    session.can_send = *can_send;
                }
            }
            s.info = Arc::new(catalog.info.clone());
            s.metadata_loaded = true;
            s.sessions = Arc::new(sessions);
            s.sessions_loaded = true;
            s.agents = agents.clone();
            s.agents_loaded = true;
            s.profiles = profiles.clone();
            s.tasks = catalog.by_leader.clone();
            s.chats = catalog.chats.clone();
            s.inbox = catalog.inbox.clone();
            s.inbox_loaded = true;
            s.inbox_error = None;
            s.artifacts = catalog.artifacts.clone();
            s.pages = catalog.pages.clone();
            s.artifacts_loaded = true;
            s.artifacts_error = None;
            s.read_markers = catalog.markers.clone();
        });
        self.profiles().accept_replica(
            profiles.clone(),
            catalog.providers.clone(),
            agents.clone(),
            catalog.profile_state.ready,
            catalog.profile_state.error.clone(),
        );
        if let Some(source) = self.agents.get() {
            source.seed_agents(agents, true);
            source.sync_profiles(profiles);
        }
        *self.replication.catalog.lock().unwrap() = Some(catalog);
        *self.replication.applied.lock().unwrap() = Some(cursor);
        Ok(())
    }

    pub(super) async fn refresh_replica_presence(&self) -> bool {
        if let Ok(live) = self.client.list_sessions().await {
            let by_id: HashMap<_, _> = live.iter().map(|s| (s.session_id.as_str(), s)).collect();
            self.commit(|state| {
                let sessions = Arc::make_mut(&mut state.sessions);
                for session in sessions {
                    let observed = by_id.get(session.session_id.as_str());
                    session.status = observed.map_or(SessionStatus::Wait, |v| v.status);
                    session.runtime_available = observed.is_some_and(|v| v.runtime_available);
                    session.can_send = observed.and_then(|v| v.can_send);
                }
            });
            true
        } else {
            false
        }
    }
}
