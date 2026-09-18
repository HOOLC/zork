//! Shared agent catalog and edit commands, independent of any UI executor.
use super::{Observable, Profiles, Subscription};
use crate::api::{GatewayClient, ProfileInfo};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default, PartialEq)]
pub struct AgentData {
    pub agents: Arc<Vec<Value>>,
    pub loaded: bool,
    pub profiles: Arc<Vec<ProfileInfo>>,
    pub node_origin: Option<String>,
    pub error: Option<String>,
}
pub struct AgentUpdate {
    pub state: Arc<AgentData>,
    pub agents_changed: bool,
    pub profiles_changed: bool,
    pub origin_changed: bool,
    pub error_changed: bool,
    pub cursor: zork_observe::Cursor,
    pub batch: Option<zork_observe::BatchId>,
    pub reset: bool,
}
pub struct AgentSubscription {
    source: Subscription<AgentData>,
    previous: Arc<AgentData>,
    prepared: Option<(zork_observe::BatchId, Arc<AgentData>)>,
}
impl AgentSubscription {
    fn project(
        &self,
        state: Arc<AgentData>,
        cursor: zork_observe::Cursor,
        batch: Option<zork_observe::BatchId>,
        reset: bool,
    ) -> AgentUpdate {
        let update = AgentUpdate {
            agents_changed: self.previous.agents != state.agents || self.previous.loaded != state.loaded,
            profiles_changed: self.previous.profiles != state.profiles,
            origin_changed: self.previous.node_origin != state.node_origin,
            error_changed: self.previous.error != state.error,
            state: state.clone(),
            cursor,
            batch,
            reset,
        };
        update
    }
    pub fn readiness(&self) -> zork_observe::Readiness {
        self.source.readiness()
    }
    pub async fn ready(&mut self) -> Result<(), zork_observe::Closed> {
        self.source.ready().await
    }
    pub fn prepare(&mut self) -> Option<AgentUpdate> {
        let batch = self.source.prepare()?;
        let state = batch.snapshot.value.clone();
        self.prepared = Some((batch.id, state.clone()));
        Some(self.project(
            state,
            batch.snapshot.cursor,
            Some(batch.id),
            batch.is_reset(),
        ))
    }
    pub fn valid(&self, batch: zork_observe::BatchId) -> bool {
        self.source.valid(batch)
    }
    pub fn acknowledge(&mut self, batch: zork_observe::BatchId) -> bool {
        if self.prepared.as_ref().is_none_or(|(id, _)| *id != batch)
            || !self.source.acknowledge(batch)
        {
            return false;
        }
        self.previous = self.prepared.take().unwrap().1;
        true
    }
    pub fn discard(&mut self, batch: zork_observe::BatchId) -> bool {
        if !self.source.discard(batch) {
            return false;
        }
        self.prepared = None;
        true
    }
    pub fn snapshot(&mut self) -> AgentUpdate {
        if let Some(update) = self.prepare() {
            self.acknowledge(update.batch.unwrap());
            return update;
        }
        let current = self.source.current();
        self.project(current.value, current.cursor, None, false)
    }
    pub async fn changed(&mut self) -> Option<AgentUpdate> {
        loop {
            self.ready().await.ok()?;
            if let Some(update) = self.prepare() {
                self.acknowledge(update.batch.unwrap());
                return Some(update);
            }
        }
    }
}
pub struct Agents {
    #[cfg(not(target_family = "wasm"))]
    device: std::sync::OnceLock<std::sync::Weak<super::Device>>,
    client: Arc<GatewayClient>,
    profiles: Arc<Profiles>,
    owned: Mutex<AgentData>,
    state: Observable<AgentData>,
    refresh_gate: tokio::sync::Mutex<()>,
}
impl Agents {
    pub async fn save(&self, input: crate::agent_edit::AgentInput) -> anyhow::Result<Value> {
        crate::model_edit::valid_id(&input.id)?;
        self.refresh().await;
        crate::agent_edit::validate_selection(
            &self.snapshot().profiles,
            &input.profile,
            &input.model,
            &input.thinking,
        )?;
        if input.creating {
            let name = crate::model_edit::valid_name(&input.name, 64)?;
            anyhow::ensure!(
                matches!(input.role.as_str(), "leader" | "worker"),
                "请选择小伙伴角色"
            );
            self.create_agent(json!({"id":input.id,"name":name,"role":input.role,"avatar":input.avatar,
                "profile_id":input.profile,"model":input.model,"thinking":input.thinking,"instructions":input.instructions,
                "allowed_leaders":if input.role=="worker" {crate::agent_edit::grant_references(input.allowed, "")}else{vec![]}})).await
        } else {
            self.update_agent_settings(
                &input.id,
                Some((input.profile, input.model, input.thinking)),
                input.avatar,
            )
            .await?;
            Ok(json!({}))
        }
    }
    pub fn new(client: Arc<GatewayClient>, profiles: Arc<Profiles>) -> Arc<Self> {
        Arc::new(Self {
            #[cfg(not(target_family = "wasm"))]
            device: Default::default(),
            client,
            profiles,
            owned: Mutex::new(Default::default()),
            state: Observable::new(Default::default()),
            refresh_gate: tokio::sync::Mutex::new(()),
        })
    }
    #[cfg(not(target_family = "wasm"))]
    pub(super) fn bind_device(&self, device: std::sync::Weak<super::Device>) {
        let _ = self.device.set(device);
    }
    pub fn snapshot(&self) -> Arc<AgentData> {
        self.state.read()
    }
    pub fn subscribe(&self) -> AgentSubscription {
        AgentSubscription {
            source: self.state.subscribe(),
            previous: Arc::new(Default::default()),
            prepared: None,
        }
    }
    fn commit(&self, change: impl FnOnce(&mut AgentData)) {
        let mut state = self.owned.lock().unwrap();
        change(&mut state);
        self.state.publish(state.clone());
    }
    pub(crate) fn seed_agents(&self, agents: Arc<Vec<Value>>, loaded: bool) {
        self.commit(|s| { s.agents = agents; s.loaded = loaded; });
    }
    pub(crate) fn sync_profiles(&self, profiles: Arc<Vec<ProfileInfo>>) {
        self.commit(|s| s.profiles = profiles);
    }
    #[cfg(feature = "headless-bench")]
    pub fn seed(&self, state: AgentData) {
        self.commit(|s| *s = state);
    }
    pub async fn refresh_agents(&self) -> anyhow::Result<()> {
        let _serial = self.refresh_gate.lock().await;
        #[cfg(not(target_family = "wasm"))]
        if let Some(device) = self.device.get().and_then(std::sync::Weak::upgrade) {
            match device.refresh_replica_catalog().await {
                Ok(true) => {
                    self.commit(|s| { s.error = None; s.loaded = true; });
                    return Ok(());
                }
                Err(error) => {
                    self.commit(|s| s.error = Some(error.to_string()));
                    return Err(error);
                }
                Ok(false) => {}
            }
        }
        match self
            .client
            .node_request(http::Method::GET, "/v1/node/agents".into(), None)
            .await
        {
            Ok(value) => {
                self.commit(|s| {
                    s.agents = Arc::new(value["items"].as_array().cloned().unwrap_or_default());
                    s.loaded = true;
                    s.error = None;
                });
                Ok(())
            }
            Err(error) => {
                self.commit(|s| s.error = Some(error.to_string()));
                Err(error.into())
            }
        }
    }
    pub async fn refresh(&self) {
        let _ = self.refresh_agents().await;
        self.profiles.refresh_statuses().await;
        self.commit(|s| s.profiles = self.profiles.snapshot().profiles.clone());
        if let Ok(mesh) = self
            .client
            .node_request(http::Method::GET, "/v1/node/mesh".into(), None)
            .await
        {
            self.commit(|s| s.node_origin = mesh["origin"].as_str().map(str::to_owned));
        }
    }
    pub async fn create_agent(&self, specification: Value) -> anyhow::Result<Value> {
        let result = self
            .client
            .node_request(
                http::Method::POST,
                "/v1/node/agents".into(),
                Some(specification),
            )
            .await?;
        self.refresh_agents().await?;
        Ok(result)
    }
    pub async fn update_agent_settings(
        &self,
        id: &str,
        selection: Option<(String, String, String)>,
        avatar: String,
    ) -> anyhow::Result<()> {
        let state = self.snapshot();
        let agent = state
            .agents
            .iter()
            .find(|a| a["id"] == id)
            .ok_or_else(|| anyhow::anyhow!("Agent is no longer available"))?;
        let result = async {
            if let Some((profile, model, thinking)) = selection {
                if agent["profile_id"] != profile
                    || agent["model"] != model
                    || agent["thinking"] != thinking
                {
                    self.client
                        .node_request(
                            http::Method::PATCH,
                            format!("/v1/node/agents/{id}/model"),
                            Some(json!({"profile_id":profile,"model":model,"thinking":thinking})),
                        )
                        .await?;
                    self.refresh_agents().await?;
                }
            }
            if agent["avatar"] != avatar {
                #[cfg(target_family = "wasm")]
                let synced = false;
                #[cfg(not(target_family = "wasm"))]
                let synced =
                    if let Some(device) = self.device.get().and_then(std::sync::Weak::upgrade) {
                        device
                            .mutate_catalog(
                                zork_client_types::sync::Action::AgentAvatar {
                                    id: id.into(),
                                    avatar: avatar.clone(),
                                },
                                None,
                            )
                            .await?
                            .is_some()
                    } else {
                        false
                    };
                if !synced {
                    self.client
                        .node_request(
                            http::Method::PUT,
                            format!("/v1/node/agents/{id}/avatar"),
                            Some(json!({"avatar":avatar})),
                        )
                        .await?;
                }
            }
            Ok(())
        }
        .await;
        let _ = self.refresh_agents().await;
        result
    }
    pub async fn update_agent_grants(
        &self,
        id: &str,
        allowed: Vec<String>,
        expected: Value,
    ) -> anyhow::Result<()> {
        self.client
            .node_request(
                http::Method::PUT,
                format!("/v1/node/agents/{id}/grants"),
                Some(json!({"allowed_leaders":allowed,"expected_allowed_leaders":expected})),
            )
            .await?;
        self.refresh_agents().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::get, Json, Router};

    #[tokio::test]
    async fn creation_does_not_open_chats_for_any_agent_role() {
        let opened = Arc::new(Mutex::new(Vec::new()));
        let recorded = opened.clone();
        let app = Router::new()
            .route(
                "/v1/node/agents",
                get(|| async { Json(json!({"items": []})) })
                    .post(|Json(body): Json<Value>| async { Json(body) }),
            )
            .route(
                "/v1/node/agents/{id}/open",
                axum::routing::post(
                    move |axum::extract::Path(id): axum::extract::Path<String>| {
                        let recorded = recorded.clone();
                        async move {
                            recorded.lock().unwrap().push(id);
                            Json(json!({}))
                        }
                    },
                ),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = Arc::new(GatewayClient::new(format!("http://{address}"), None));
        let agents = Agents::new(client.clone(), Profiles::new(client));
        agents
            .create_agent(json!({"id": "leader", "role": "leader"}))
            .await
            .unwrap();
        agents
            .create_agent(json!({"id": "worker", "role": "worker"}))
            .await
            .unwrap();
        assert!(opened.lock().unwrap().is_empty());
        server.abort();
    }
    #[tokio::test]
    async fn effort_only_change_is_sent_and_repeated_save_is_a_noop() {
        let current = Arc::new(Mutex::new(
            json!({"id":"worker","profile_id":"auto","model":"model","thinking":"low","avatar":"cat"}),
        ));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let get_state = current.clone();
        let patch_state = current.clone();
        let patch_calls = calls.clone();
        let app = Router::new()
            .route(
                "/v1/node/agents",
                get(move || {
                    let state = get_state.clone();
                    async move { Json(json!({"items":[state.lock().unwrap().clone()]})) }
                }),
            )
            .route(
                "/v1/node/agents/worker/model",
                axum::routing::patch(move |Json(body): Json<Value>| {
                    let state = patch_state.clone();
                    let calls = patch_calls.clone();
                    async move {
                        calls.lock().unwrap().push(body.clone());
                        let mut state = state.lock().unwrap();
                        for field in ["profile_id", "model", "thinking"] {
                            state[field] = body[field].clone();
                        }
                        Json(state.clone())
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = Arc::new(GatewayClient::new(format!("http://{address}"), None));
        let agents = Agents::new(client.clone(), Profiles::new(client));
        agents.seed_agents(Arc::new(vec![current.lock().unwrap().clone()]), true);
        for _ in 0..2 {
            agents
                .update_agent_settings(
                    "worker",
                    Some(("auto".into(), "model".into(), "high".into())),
                    "cat".into(),
                )
                .await
                .unwrap();
        }
        assert_eq!(
            *calls.lock().unwrap(),
            vec![json!({"profile_id":"auto","model":"model","thinking":"high"})]
        );
        server.abort();
    }
}
