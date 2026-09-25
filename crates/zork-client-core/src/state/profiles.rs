//! Connection state and refresh policy shared by native and the in-memory Web
//! adapter. Platforms supply execution/clock only, never business comparison.
use super::{Observable, Subscription};
use crate::api::{ProfileInfo, StationClient};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone, Default, PartialEq)]
pub struct ProfileData {
    pub profiles: Arc<Vec<ProfileInfo>>,
    pub providers: Arc<Vec<Value>>,
    pub helpers: Arc<Vec<Value>>,
    pub refreshing: Arc<HashSet<String>>,
    pub failed: Arc<HashSet<String>>,
    pub loading: bool,
    pub loaded: bool,
    pub error: Option<String>,
    pub authorization: Option<Value>,
    pub authorization_busy: bool,
    pub authorization_error: Option<String>,
    pub authorization_complete: bool,
}
pub struct ProfileUpdate {
    pub state: Arc<ProfileData>,
    pub records: Vec<String>,
    pub structure_changed: bool,
    pub catalog_changed: bool,
    pub status_changed: bool,
    pub cursor: zork_observe::Cursor,
    pub batch: Option<zork_observe::BatchId>,
    pub reset: bool,
}
pub struct ProfileSubscription {
    source: Subscription<ProfileData>,
    previous: Arc<ProfileData>,
    prepared: Option<(zork_observe::BatchId, Arc<ProfileData>)>,
}
impl ProfileSubscription {
    fn project(
        &self,
        state: Arc<ProfileData>,
        cursor: zork_observe::Cursor,
        batch: Option<zork_observe::BatchId>,
        reset: bool,
    ) -> ProfileUpdate {
        let old = &self.previous;
        let previous = old
            .profiles
            .iter()
            .map(|p| (&p.profile_id, p))
            .collect::<HashMap<_, _>>();
        let catalog_changed = old.providers != state.providers || old.helpers != state.helpers;
        let mut records = state
            .profiles
            .iter()
            .filter(|p| {
                catalog_changed
                    || previous.get(&p.profile_id).is_none_or(|old| **old != **p)
                    || old.refreshing.contains(&p.profile_id)
                        != state.refreshing.contains(&p.profile_id)
                    || old.failed.contains(&p.profile_id) != state.failed.contains(&p.profile_id)
            })
            .map(|p| p.profile_id.clone())
            .collect::<Vec<_>>();
        records.extend(
            old.profiles
                .iter()
                .filter(|p| {
                    !state
                        .profiles
                        .iter()
                        .any(|new| new.profile_id == p.profile_id)
                })
                .map(|p| p.profile_id.clone()),
        );
        let structure_changed = old
            .profiles
            .iter()
            .map(|p| &p.profile_id)
            .ne(state.profiles.iter().map(|p| &p.profile_id));
        let status_changed = old.loading != state.loading
            || old.error != state.error
            || old.loaded != state.loaded
            || old.authorization != state.authorization
            || old.authorization_busy != state.authorization_busy
            || old.authorization_error != state.authorization_error
            || old.authorization_complete != state.authorization_complete;
        ProfileUpdate {
            state,
            records,
            structure_changed,
            catalog_changed,
            status_changed,
            cursor,
            batch,
            reset,
        }
    }
    pub fn readiness(&self) -> zork_observe::Readiness {
        self.source.readiness()
    }
    pub async fn ready(&mut self) -> Result<(), zork_observe::Closed> {
        self.source.ready().await
    }
    pub fn prepare(&mut self) -> Option<ProfileUpdate> {
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
    pub fn snapshot(&mut self) -> ProfileUpdate {
        if let Some(update) = self.prepare() {
            self.acknowledge(update.batch.unwrap());
            return update;
        }
        let current = self.source.current();
        self.project(current.value, current.cursor, None, false)
    }
    pub async fn changed(&mut self) -> Option<ProfileUpdate> {
        loop {
            self.ready().await.ok()?;
            if let Some(update) = self.prepare() {
                self.acknowledge(update.batch.unwrap());
                return Some(update);
            }
        }
    }
}
struct Owned {
    state: ProfileData,
    epoch: u64,
    running: bool,
    again: bool,
    authorization_epoch: u64,
}
pub struct Profiles {
    device: std::sync::OnceLock<std::sync::Weak<super::Device>>,
    client: Arc<StationClient>,
    owned: Mutex<Owned>,
    state: Observable<ProfileData>,
    authorization_task: Mutex<Option<crate::api::ClientTask>>,
}
impl Profiles {
    fn save_authorization_record(&self, record: &Value) -> anyhow::Result<()> {
        if let Some(device) = self.device.get().and_then(std::sync::Weak::upgrade) {
            if let Some((store, peer)) = &device.cache {
                store.put_authorized_settings(peer, "profile-authorization", record)?;
            }
        }
        Ok(())
    }
    /// Suspend only the local monitor when the host releases its connection.
    /// The remote authorization is resumed using its public attempt metadata.
    pub(crate) fn suspend_authorization(&self) {
        if let Some(task) = self.authorization_task.lock().unwrap().take() {
            task.abort();
        }
        let mut owned = self.owned.lock().unwrap();
        owned.authorization_epoch = owned.authorization_epoch.wrapping_add(1);
        owned.state.authorization_busy = false;
        self.state.publish(owned.state.clone());
    }
    pub(crate) fn restore_authorization(self: &Arc<Self>) -> anyhow::Result<()> {
        if self.snapshot().authorization.is_none() {
            let Some(device) = self.device.get().and_then(std::sync::Weak::upgrade) else {
                return Ok(());
            };
            let Some((store, peer)) = &device.cache else {
                return Ok(());
            };
            let Some(record) = store
                .get::<Value>(peer, "profile-authorization")?
                .filter(|v| v.is_object())
            else {
                return Ok(());
            };
            let mut owned = self.owned.lock().unwrap();
            if record["completed"] == true {
                owned.state.authorization_complete = true;
            } else if record["id"]
                .as_str()
                .is_some_and(|id| crate::model_edit::valid_id(id).is_ok())
                && record["client_expires_at"]
                    .as_i64()
                    .is_some_and(|at| at > chrono::Utc::now().timestamp())
            {
                owned.state.authorization = Some(record);
                owned.state.authorization_error = None;
            } else {
                store.put(peer, "profile-authorization", &Value::Null)?;
                owned.state.authorization_error = Some("上次授权已过期，请重新开始".into());
            }
            self.state.publish(owned.state.clone());
        }
        self.continue_authorization(String::new());
        Ok(())
    }
    pub fn continue_authorization(self: &Arc<Self>, callback: String) {
        let mut task = self.authorization_task.lock().unwrap();
        if task.as_ref().is_some_and(|task| !task.is_finished()) {
            return;
        }
        let state = self.snapshot();
        if state.authorization.is_none()
            || (state
                .authorization
                .as_ref()
                .is_some_and(|a| a["flow"] == "browser_callback")
                && callback.is_empty())
        {
            return;
        }
        let epoch = {
            let mut owned = self.owned.lock().unwrap();
            owned.state.authorization_busy = true;
            owned.state.authorization_error = None;
            self.state.publish(owned.state.clone());
            owned.authorization_epoch
        };
        let source = self.clone();
        *task = Some(self.client.spawn(async move {
            let result = source.finish_authorization(callback).await;
            let mut owned = source.owned.lock().unwrap();
            if owned.authorization_epoch != epoch {
                return;
            }
            owned.state.authorization_busy = false;
            owned.state.authorization_error = result.err().map(|e| e.to_string());
            source.state.publish(owned.state.clone());
        }));
    }
    fn accept_profile(&self, value: Value) -> anyhow::Result<()> {
        let profile: ProfileInfo = serde_json::from_value(value)?;
        let mut owned = self.owned.lock().unwrap();
        owned.epoch = owned.epoch.wrapping_add(1);
        let profiles = Arc::make_mut(&mut owned.state.profiles);
        if let Some(old) = profiles
            .iter_mut()
            .find(|p| p.profile_id == profile.profile_id)
        {
            *old = profile;
        } else {
            profiles.push(profile);
        }
        self.state.publish(owned.state.clone());
        Ok(())
    }
    async fn fetch_detail(&self, id: &str) -> anyhow::Result<Value> {
        crate::model_edit::valid_id(id)?;
        let value = self
            .client
            .node_request(http::Method::GET, format!("/v1/node/profiles/{id}"), None)
            .await?;
        anyhow::ensure!(
            value["profile_id"] == id,
            "profile response ID does not match"
        );
        self.accept_profile(value.clone())?;
        Ok(value)
    }
    pub async fn open_detail(&self, id: &str) -> anyhow::Result<()> {
        self.fetch_detail(id).await?;
        if self.snapshot().profiles.iter().any(|profile| {
            profile.profile_id == id && profile.quota_stale_at(chrono::Utc::now().timestamp())
        }) {
            self.refresh_quota(id.to_owned()).await;
        }
        Ok(())
    }
    pub fn model_errors(
        &self,
        id: &str,
        input: &crate::model_edit::ModelInput,
    ) -> Vec<crate::model_edit::FieldError> {
        let detail = self.detail(serde_json::json!({"profile_id":id}));
        input.errors(
            detail["models"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default(),
        )
    }
    async fn commit_models(
        &self,
        id: &str,
        models: Vec<Value>,
        expected: Vec<Value>,
    ) -> anyhow::Result<()> {
        let value = self
            .client
            .node_request(
                http::Method::PUT,
                format!("/v1/node/profiles/{id}/models"),
                Some(serde_json::json!({"models":models,"expected_models":expected})),
            )
            .await?;
        self.accept_profile(value)?;
        if let Some(result) = self.refresh_replica().await {
            result?;
        }
        Ok(())
    }
    pub async fn save_model(
        &self,
        id: &str,
        input: crate::model_edit::ModelInput,
    ) -> anyhow::Result<()> {
        let current = self.fetch_detail(id).await?;
        let expected = current["models"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("模型列表不可用"))?
            .clone();
        self.commit_models(id, input.apply(expected.clone())?, expected)
            .await
    }
    pub async fn remove_model(&self, id: &str, expected_model: Value) -> anyhow::Result<()> {
        let current = self.fetch_detail(id).await?;
        let expected = current["models"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("模型列表不可用"))?
            .clone();
        self.commit_models(
            id,
            crate::model_edit::remove_model(expected.clone(), &expected_model)?,
            expected,
        )
        .await
    }
    pub async fn set_model_enabled(
        &self,
        id: &str,
        model: String,
        enabled: bool,
    ) -> anyhow::Result<()> {
        crate::model_edit::valid_id(id)?;
        let value = self
            .client
            .node_request(
                http::Method::PUT,
                format!("/v1/node/profiles/{id}/models/enabled"),
                Some(serde_json::json!({"model_id":model,"enabled":enabled})),
            )
            .await?;
        self.accept_profile(value)?;
        if let Some(result) = self.refresh_replica().await {
            result?;
        }
        Ok(())
    }
    /// Ids the provider lists for this connection right now, without changing
    /// it: the model editor offers the ones not added yet. Connections without
    /// a model-list API return an empty list.
    pub async fn available_models(&self, id: &str) -> anyhow::Result<Vec<String>> {
        crate::model_edit::valid_id(id)?;
        let value = self
            .client
            .node_request(
                http::Method::GET,
                format!("/v1/node/profiles/{id}/discovered-models"),
                None,
            )
            .await?;
        Ok(value["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| item["id"].as_str())
            .map(str::to_owned)
            .collect())
    }
    /// Imports the provider's model list. New models the catalog recognizes
    /// get their preset values right away (disabled until the user turns them
    /// on); the rest stay 待配置. The result adds `preset`, `pending` and a
    /// ready-to-show `message` to the station's `added`/`truncated`.
    pub async fn discover_models(&self, id: &str) -> anyhow::Result<Value> {
        crate::model_edit::valid_id(id)?;
        let before = self.fetch_detail(id).await?["models"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut value = self
            .client
            .node_request(
                http::Method::POST,
                format!("/v1/node/profiles/{id}/models/refresh"),
                None,
            )
            .await?;
        let profile = value["profile"].take();
        let provider = profile["provider"].as_str().unwrap_or_default().to_owned();
        let expected = profile["models"].as_array().cloned().unwrap_or_default();
        self.accept_profile(profile)?;
        let mut models = expected.clone();
        let mut counts = crate::model_catalog::import_presets(&before, &mut models, &provider);
        let mut committed = false;
        if counts.preset > 0 {
            match self.commit_models(id, models, expected).await {
                Ok(()) => committed = true,
                Err(error) => {
                    tracing::warn!(%error, "applying model presets after discovery failed");
                    counts.pending += counts.preset;
                    counts.preset = 0;
                }
            }
        }
        if !committed {
            if let Some(result) = self.refresh_replica().await {
                result?;
            }
        }
        let mut message = counts.message();
        if value["truncated"] == true {
            message.push_str("（仅返回部分结果）");
        }
        value["preset"] = serde_json::json!(counts.preset);
        value["pending"] = serde_json::json!(counts.pending);
        value["message"] = serde_json::json!(message);
        Ok(value)
    }
    pub async fn save_connection(
        &self,
        input: crate::model_edit::ConnectionInput,
    ) -> anyhow::Result<()> {
        let id = input.id.trim();
        crate::model_edit::valid_id(id)?;
        self.refresh_statuses().await;
        anyhow::ensure!(
            !self.snapshot().profiles.iter().any(|p| p.profile_id == id),
            "此连接名称已存在，请使用另一个名称"
        );
        anyhow::ensure!(!input.key.trim().is_empty(), "请填写 API Key");
        let catalog = self.snapshot();
        let billing = catalog
            .providers
            .iter()
            .find(|p| p["id"] == input.provider)
            .and_then(|p| p["billing"].as_array())
            .and_then(|items| items.iter().find(|b| b["id"] == input.billing))
            .ok_or_else(|| anyhow::anyhow!("请选择有效的供应商与计费方式"))?;
        let base = if input.base_url.trim().is_empty() {
            billing["template"]["baseUrl"].as_str().unwrap_or_default()
        } else {
            input.base_url.trim()
        };
        let value = self.client.node_request(http::Method::PUT, format!("/v1/node/profiles/{id}"),
            Some(serde_json::json!({"provider":input.provider,"billing":input.billing,"base_url":base,"models":[],"auth":{"type":"api_key","key":input.key.trim()}}))).await?;
        self.accept_profile(value)?;
        if let Some(result) = self.refresh_replica().await {
            result?;
        }
        Ok(())
    }
    pub async fn start_authorization(
        self: &Arc<Self>,
        id: String,
        provider: String,
        billing: String,
    ) -> anyhow::Result<()> {
        crate::model_edit::valid_id(id.trim())?;
        let epoch = {
            let mut owned = self.owned.lock().unwrap();
            anyhow::ensure!(
                owned.state.authorization.is_none() && !owned.state.authorization_busy,
                "请先取消当前授权"
            );
            owned.authorization_epoch = owned.authorization_epoch.wrapping_add(1);
            owned.state.authorization_busy = true;
            owned.state.authorization_error = None;
            owned.state.authorization_complete = false;
            self.state.publish(owned.state.clone());
            owned.authorization_epoch
        };
        let result = self.client.node_request(http::Method::POST, "/v1/node/auth".into(),
            Some(serde_json::json!({"profile_id":id.trim(),"provider":provider,"billing":billing}))).await.map(|mut value| {
                value["profile_id"] = serde_json::json!(id.trim());
                value["provider"] = serde_json::json!(provider);
                value["billing"] = serde_json::json!(billing);
                value["client_expires_at"] = serde_json::json!(chrono::Utc::now().timestamp() + 600);
                value
            });
        let stale = {
            let mut owned = self.owned.lock().unwrap();
            if owned.authorization_epoch != epoch {
                true
            } else {
                owned.state.authorization_busy = false;
                match &result {
                    Ok(value) => owned.state.authorization = Some(value.clone()),
                    Err(error) => owned.state.authorization_error = Some(error.to_string()),
                }
                self.state.publish(owned.state.clone());
                false
            }
        };
        let value = result?;
        if stale {
            if let Some(id) = value["id"].as_str() {
                let _ = self
                    .client
                    .node_request(http::Method::DELETE, format!("/v1/node/auth/{id}"), None)
                    .await;
            }
            anyhow::bail!("授权已取消");
        }
        // Never persist a provider credential or a pasted browser callback.
        let mut record = serde_json::Map::new();
        for key in [
            "id",
            "profile_id",
            "provider",
            "billing",
            "flow",
            "verification_url",
            "user_code",
            "client_expires_at",
        ] {
            if let Some(value) = value.get(key) {
                record.insert(key.into(), value.clone());
            }
        }
        self.save_authorization_record(&Value::Object(record))?;
        self.continue_authorization(String::new());
        Ok(())
    }
    pub async fn cancel_authorization(&self) -> anyhow::Result<()> {
        if let Some(task) = self.authorization_task.lock().unwrap().take() {
            task.abort();
        }
        let attempt = {
            let mut owned = self.owned.lock().unwrap();
            owned.authorization_epoch = owned.authorization_epoch.wrapping_add(1);
            let attempt = owned.state.authorization.take();
            owned.state.authorization_busy = false;
            owned.state.authorization_error = None;
            owned.state.authorization_complete = false;
            self.state.publish(owned.state.clone());
            attempt
        };
        self.save_authorization_record(&Value::Null)?;
        if let Some(id) = attempt.as_ref().and_then(|a| a["id"].as_str()) {
            self.client
                .node_request(http::Method::DELETE, format!("/v1/node/auth/{id}"), None)
                .await?;
        }
        Ok(())
    }
    pub async fn finish_authorization(&self, callback: String) -> anyhow::Result<()> {
        let attempt = self
            .snapshot()
            .authorization
            .clone()
            .ok_or_else(|| anyhow::anyhow!("没有正在进行的授权"))?;
        let id = attempt["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("授权缺少 ID"))?;
        if attempt["flow"] == "browser_callback" && callback.is_empty() {
            return Ok(());
        }
        let mut body = if callback.is_empty() {
            serde_json::json!({})
        } else {
            serde_json::json!({"callback":callback})
        };
        let remaining = attempt["client_expires_at"]
            .as_i64()
            .map(|at| at.saturating_sub(chrono::Utc::now().timestamp()).max(0) as u64)
            .unwrap_or(600);
        let deadline = std::time::Instant::now() + Duration::from_secs(remaining);
        loop {
            anyhow::ensure!(
                std::time::Instant::now() < deadline,
                "授权已超时，请重新开始"
            );
            if self
                .snapshot()
                .authorization
                .as_ref()
                .is_none_or(|a| a["id"] != id)
            {
                return Ok(());
            }
            let value = self
                .client
                .node_request(
                    http::Method::POST,
                    format!("/v1/node/auth/{id}"),
                    Some(body),
                )
                .await?;
            body = serde_json::json!({});
            if self
                .snapshot()
                .authorization
                .as_ref()
                .is_none_or(|a| a["id"] != id)
            {
                return Ok(());
            }
            if value["status"] != "pending" {
                anyhow::ensure!(value["status"] == "completed", "授权尚未完成");
                {
                    let mut owned = self.owned.lock().unwrap();
                    if owned
                        .state
                        .authorization
                        .as_ref()
                        .is_some_and(|a| a["id"] == id)
                    {
                        self.save_authorization_record(&serde_json::json!({"id":id,"profile_id":attempt["profile_id"],"completed":true}))?;
                        owned.state.authorization = None;
                        owned.state.authorization_complete = true;
                        self.state.publish(owned.state.clone());
                    }
                }
                self.refresh().await;
                return Ok(());
            }
            self.client
                .wait(Duration::from_secs(
                    value["retry_after_seconds"]
                        .as_u64()
                        .unwrap_or(5)
                        .clamp(1, 60),
                ))
                .await;
        }
    }
    pub fn new(client: Arc<StationClient>) -> Arc<Self> {
        Arc::new(Self {
            device: Default::default(),
            client,
            owned: Mutex::new(Owned {
                state: Default::default(),
                epoch: 0,
                running: false,
                again: false,
                authorization_epoch: 0,
            }),
            state: Observable::new(Default::default()),
            authorization_task: Mutex::new(None),
        })
    }
    pub(super) fn bind_device(&self, device: std::sync::Weak<super::Device>) {
        let _ = self.device.set(device);
    }
    pub(super) fn accept_replica(
        &self,
        profiles: Arc<Vec<ProfileInfo>>,
        providers: Arc<Vec<Value>>,
        helpers: Arc<Vec<Value>>,
        ready: bool,
        error: Option<String>,
    ) {
        let mut owned = self.owned.lock().unwrap();
        owned.epoch = owned.epoch.wrapping_add(1);
        owned.state.profiles = profiles;
        owned.state.providers = providers;
        owned.state.helpers = helpers;
        owned.state.error = error;
        owned.state.loading = false;
        owned.state.loaded = ready;
        self.state.publish(owned.state.clone());
    }
    async fn refresh_replica(&self) -> Option<anyhow::Result<()>> {
        let device = self.device.get()?.upgrade()?;
        match device.refresh_replica_catalog().await {
            Ok(false) => None,
            result => Some(result.map(|_| ())),
        }
    }
    fn replica_error(&self, error: anyhow::Error) {
        let mut owned = self.owned.lock().unwrap();
        owned.state.error = Some(error.to_string());
        owned.state.loading = false;
        self.state.publish(owned.state.clone());
    }
    #[cfg(any(test, feature = "headless-bench"))]
    pub fn seed(&self, state: ProfileData) {
        let mut owned = self.owned.lock().unwrap();
        owned.state = state;
        self.state.publish(owned.state.clone());
    }
    pub fn snapshot(&self) -> Arc<ProfileData> {
        self.state.read()
    }
    pub fn subscribe(&self) -> ProfileSubscription {
        ProfileSubscription {
            source: self.state.subscribe(),
            previous: Arc::new(Default::default()),
            prepared: None,
        }
    }
    /// A wire adapter applies a complete small catalog before acknowledging it.
    pub fn subscribe_state(&self) -> Subscription<ProfileData> {
        self.state.subscribe()
    }
    /// Opening and freshness policy belong to core; ongoing updates use its subscription.
    pub async fn while_visible(&self) {
        self.while_visible_with(|p| p.quota_stale_at(chrono::Utc::now().timestamp()))
            .await;
    }
    async fn while_visible_with(&self, stale: impl Fn(&ProfileInfo) -> bool) {
        let mut initial = self.subscribe();
        self.refresh_stale_with(stale).await;
        // Ongoing changes arrive from the shared Device/catalog subscription.
        while initial.changed().await.is_some() {}
    }
    /// One opening intent, also used by platforms whose observation is independent.
    pub async fn refresh_stale(&self) {
        self.refresh_stale_with(|p| p.quota_stale_at(chrono::Utc::now().timestamp()))
            .await;
    }
    async fn refresh_stale_with(&self, stale: impl Fn(&ProfileInfo) -> bool) {
        let mut initial = self.subscribe();
        self.refresh_statuses().await;
        // A page can open while its initial list request is already running.
        // Wait for that result before deciding which snapshots need a probe.
        while self.snapshot().loading {
            if initial.changed().await.is_none() {
                return;
            }
        }
        let stale_ids = self
            .snapshot()
            .profiles
            .iter()
            .filter(|p| stale(p))
            .map(|p| p.profile_id.clone())
            .collect::<Vec<_>>();
        for id in stale_ids {
            self.refresh_quota(id).await;
        }
    }

    pub async fn rename(&self, id: String, name: String) -> anyhow::Result<()> {
        crate::model_edit::valid_id(&id)?;
        let name = name.trim();
        // An empty name clears it: the connection is titled by its account.
        anyhow::ensure!(
            name.chars().count() <= 100 && !name.chars().any(char::is_control),
            "名称最多 100 个字符"
        );
        let value = self
            .client
            .node_request(
                http::Method::PUT,
                format!("/v1/node/profiles/{id}/name"),
                Some(serde_json::json!({"name":name})),
            )
            .await?;
        if let Some(result) = self.refresh_replica().await {
            return result;
        }
        let profile: ProfileInfo = serde_json::from_value(value)?;
        anyhow::ensure!(
            profile.profile_id == id,
            "profile response ID does not match"
        );
        let mut s = self.owned.lock().unwrap();
        s.epoch = s.epoch.wrapping_add(1);
        let profiles = Arc::make_mut(&mut s.state.profiles);
        if let Some(old) = profiles.iter_mut().find(|p| p.profile_id == id) {
            *old = profile;
        } else {
            profiles.push(profile);
        }
        self.state.publish(s.state.clone());
        Ok(())
    }
    pub async fn refresh_statuses(&self) {
        if let Some(result) = self.refresh_replica().await {
            if let Err(error) = result {
                self.replica_error(error);
            }
            return;
        }
        {
            let mut owned = self.owned.lock().unwrap();
            if owned.running {
                owned.again = true;
                return;
            }
            owned.running = true;
            owned.state.loading = true;
            self.state.publish(owned.state.clone());
        }
        struct Guard<'a>(&'a Profiles, bool);
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                if !self.1 {
                    return;
                }
                let mut s = self.0.owned.lock().unwrap();
                s.running = false;
                s.state.loading = false;
                self.0.state.publish(s.state.clone());
            }
        }
        let mut guard = Guard(self, true);
        loop {
            let epoch = {
                let mut s = self.owned.lock().unwrap();
                s.again = false;
                s.epoch
            };
            let result = self.client.list_profiles().await;
            let again = {
                let mut s = self.owned.lock().unwrap();
                if s.epoch == epoch {
                    match result {
                        Ok(profiles) => {
                            s.state.profiles = Arc::new(profiles);
                            s.state.loaded = true;
                            s.state.failed = Arc::new(HashSet::new());
                            s.state.error = None;
                        }
                        Err(error) => {
                            s.state.failed = Arc::new(
                                s.state
                                    .profiles
                                    .iter()
                                    .map(|p| p.profile_id.clone())
                                    .collect(),
                            );
                            s.state.error = Some(error.to_string());
                        }
                    }
                } else {
                    s.again = true;
                }
                self.state.publish(s.state.clone());
                if s.again {
                    true
                } else {
                    s.running = false;
                    s.state.loading = false;
                    guard.1 = false;
                    self.state.publish(s.state.clone());
                    false
                }
            };
            if !again {
                break;
            }
        }
        drop(guard);
    }
    pub async fn refresh(&self) {
        if let Some(result) = self.refresh_replica().await {
            if let Err(error) = result {
                self.replica_error(error);
            }
            return;
        }
        self.refresh_statuses().await;
        let providers = self
            .client
            .node_request(http::Method::GET, "/v1/node/providers".into(), None)
            .await;
        let helpers = self
            .client
            .node_request(http::Method::GET, "/v1/node/agents".into(), None)
            .await;
        let mut state = self.owned.lock().unwrap();
        if let Ok(value) = providers {
            state.state.providers =
                Arc::new(value["providers"].as_array().cloned().unwrap_or_default());
        }
        if let Ok(value) = helpers {
            state.state.helpers = Arc::new(value["items"].as_array().cloned().unwrap_or_default());
        }
        self.state.publish(state.state.clone());
    }
    pub async fn refresh_quota(&self, id: String) {
        {
            let mut s = self.owned.lock().unwrap();
            if !Arc::make_mut(&mut s.state.refreshing).insert(id.clone()) {
                return;
            }
            s.epoch = s.epoch.wrapping_add(1);
            self.state.publish(s.state.clone());
        }
        let result = self
            .client
            .node_request(
                http::Method::POST,
                format!("/v1/node/profiles/{id}/refresh"),
                None,
            )
            .await;
        if let Some(synced) = self.refresh_replica().await {
            let mut s = self.owned.lock().unwrap();
            Arc::make_mut(&mut s.state.refreshing).remove(&id);
            if result.is_err() || synced.is_err() {
                Arc::make_mut(&mut s.state.failed).insert(id);
            } else {
                Arc::make_mut(&mut s.state.failed).remove(&id);
            }
            self.state.publish(s.state.clone());
            return;
        }
        let mut s = self.owned.lock().unwrap();
        s.epoch = s.epoch.wrapping_add(1);
        Arc::make_mut(&mut s.state.refreshing).remove(&id);
        match result
            .ok()
            .and_then(|v| serde_json::from_value::<ProfileInfo>(v).ok())
            .filter(|p| p.profile_id == id)
        {
            Some(profile) => {
                Arc::make_mut(&mut s.state.failed).remove(&id);
                let profiles = Arc::make_mut(&mut s.state.profiles);
                if let Some(old) = profiles.iter_mut().find(|p| p.profile_id == id) {
                    *old = profile;
                } else {
                    profiles.push(profile);
                }
            }
            None => {
                Arc::make_mut(&mut s.state.failed).insert(id);
            }
        }
        self.state.publish(s.state.clone());
    }
    /// Refresh an open detail's live status from the authoritative connection
    /// snapshot. Dialog rendering doesn't merge quota/account business fields.
    pub fn detail(&self, detail: Value) -> Value {
        let state = self.snapshot();
        if let Some(profile) = state
            .profiles
            .iter()
            .find(|p| detail["profile_id"].as_str() == Some(p.profile_id.as_str()))
        {
            let mut value = serde_json::to_value(profile).expect("profile projection");
            value["verified"] = Value::Bool(profile.is_verified());
            return value;
        }
        detail
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        extract::Path,
        routing::{get, post},
        Json, Router,
    };
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn authorization_restores_public_metadata_after_connection_replacement() {
        let router = Router::new().route("/v1/node/auth", post(|| async {
            Json(json!({"id":"attempt","flow":"browser_callback","verification_url":"https://example.test/login",
                "device_code":"must-not-be-persisted"}))
        })).route("/v1/node/auth/attempt", axum::routing::delete(|| async { Json(json!({})) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::store::ClientStore::open(root.path()).unwrap());
        let device = super::super::Device::open(
            Arc::new(StationClient::new(url.clone(), None)),
            Some((store.clone(), "node".into())),
            false,
        );
        let profiles = device.profiles();
        profiles
            .start_authorization("account".into(), "provider".into(), "subscription".into())
            .await
            .unwrap();
        let record = store
            .get::<Value>("node", "profile-authorization")
            .unwrap()
            .unwrap();
        assert_eq!(record["profile_id"], "account");
        assert!(!record.to_string().contains("must-not-be-persisted"));
        profiles.suspend_authorization();
        drop(profiles);
        drop(device);
        let next = super::super::Device::open(
            Arc::new(StationClient::new(url, None)),
            Some((store.clone(), "node".into())),
            false,
        );
        next.profiles().restore_authorization().unwrap();
        assert_eq!(
            next.profiles().snapshot().authorization.as_ref().unwrap()["id"],
            "attempt"
        );
        next.profiles().cancel_authorization().await.unwrap();
        assert_eq!(
            store
                .get::<Value>("node", "profile-authorization")
                .unwrap()
                .unwrap(),
            Value::Null
        );
        server.abort();
    }

    #[tokio::test]
    async fn opening_during_a_list_fetch_refreshes_only_the_stale_profile() {
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let queried = Arc::new(tokio::sync::Notify::new());
        let lists = Arc::new(AtomicUsize::new(0));
        let probes = Arc::new(Mutex::new(Vec::new()));
        let router = Router::new().route("/v1/im/profiles", get({
            let (started,release,lists)=(started.clone(),release.clone(),lists.clone());
            move || { let (started,release,lists)=(started.clone(),release.clone(),lists.clone()); async move {
                if lists.fetch_add(1,Ordering::SeqCst)==0 { started.notify_one(); release.notified().await; }
                Json(json!({"items":[{"profile_id":"old","provider":"test"},{"profile_id":"fresh","provider":"test"}]}))
            }}
        })).route("/v1/node/profiles/{id}/refresh",post({
            let (queried,probes)=(queried.clone(),probes.clone());
            move |Path(id):Path<String>| { let (queried,probes)=(queried.clone(),probes.clone()); async move {
                probes.lock().unwrap().push(id.clone());queried.notify_one();
                Json(json!({"profile_id":id,"provider":"test"}))
            }}
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let source = Profiles::new(Arc::new(StationClient::new(url, None)));
        let first = tokio::spawn({
            let source = source.clone();
            async move { source.refresh_statuses().await }
        });
        tokio::time::timeout(Duration::from_secs(5), started.notified())
            .await
            .unwrap();
        let visible = tokio::spawn({
            let source = source.clone();
            async move {
                source.while_visible_with(|p| p.profile_id == "old").await;
            }
        });
        tokio::task::yield_now().await;
        release.notify_one();
        tokio::time::timeout(Duration::from_secs(5), queried.notified())
            .await
            .unwrap();
        first.await.unwrap();
        assert_eq!(*probes.lock().unwrap(), vec!["old"]);
        visible.abort();
        server.abort();
    }
}
