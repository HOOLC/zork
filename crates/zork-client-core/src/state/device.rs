use super::{NavigationData, Observable, Subscription};
use crate::{
    api::{
        Artifact, ConversationReadMarker, MeshStatus, ProductTask, ProfileInfo, Role,
        SessionSummary, StationClient,
    },
    live::LiveEvent,
    store::ClientStore,
};
use futures_util::StreamExt;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

/// Business topics, independent of a platform's widgets or layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Domains(u16);
impl Domains {
    pub const CONNECTION: Self = Self(1);
    pub const SESSIONS: Self = Self(2);
    pub const AGENTS: Self = Self(4);
    pub const TASKS: Self = Self(8);
    pub const PROFILES: Self = Self(16);
    pub const INBOX: Self = Self(32);
    pub const ARTIFACTS: Self = Self(64);
    pub const MESH: Self = Self(128);
    pub const ALL: Self = Self(255);
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
    fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }
}
impl std::ops::BitOr for Domains {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeviceData {
    pub info: Arc<Value>,
    pub metadata_loaded: bool,
    pub online: Option<bool>,
    pub route: crate::api::ConnectionRoute,
    pub revoked: bool,
    pub connection_error: Option<String>,
    pub confirmed_at_ms: Option<u64>,
    pub sessions: Arc<Vec<SessionSummary>>,
    pub sessions_loaded: bool,
    pub agents: Arc<Vec<Value>>,
    pub agents_loaded: bool,
    pub tasks: Arc<HashMap<String, Vec<ProductTask>>>,
    pub chats: Option<Arc<Vec<zork_client_types::chat::Channel>>>,
    pub profiles: Arc<Vec<ProfileInfo>>,
    pub inbox: Arc<Vec<ProductTask>>,
    pub inbox_loaded: bool,
    pub inbox_error: Option<String>,
    pub artifacts: Arc<Vec<Artifact>>,
    pub pages: Arc<crate::pages::PageCatalog>,
    pub content_indices: Arc<crate::pages::ContentCatalog>,
    pub artifacts_loaded: bool,
    pub artifacts_error: Option<String>,
    pub mesh: Arc<MeshStatus>,
    pub read_markers: Arc<Vec<ConversationReadMarker>>,
    revisions: [u64; 8],
}

pub struct DeviceUpdate {
    pub state: Arc<DeviceData>,
    pub domains: Domains,
    pub batch: Option<zork_observe::BatchId>,
    pub cursor: zork_observe::Cursor,
    pub reset: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentAvailability {
    Loading,
    Unavailable,
    Empty,
    Available,
}
pub struct DeviceSubscription {
    source: Subscription<DeviceData>,
}
impl DeviceSubscription {
    pub fn valid(&self, batch: zork_observe::BatchId) -> bool {
        self.source.valid(batch)
    }
    pub fn reset(&mut self) {
        self.source.reset();
    }
    pub fn readiness(&self) -> zork_observe::Readiness {
        self.source.readiness()
    }
    pub async fn ready(&mut self) -> Result<(), zork_observe::Closed> {
        self.source.ready().await
    }
    pub fn prepare(&mut self) -> Option<DeviceUpdate> {
        let batch = self.source.prepare()?;
        Some(DeviceUpdate {
            state: batch.snapshot.value.clone(),
            domains: Domains(batch.topics.bits() as u16 & Domains::ALL.0),
            batch: Some(batch.id),
            cursor: batch.snapshot.cursor,
            reset: batch.is_reset(),
        })
    }
    pub fn acknowledge(&mut self, batch: zork_observe::BatchId) -> bool {
        self.source.acknowledge(batch)
    }
    pub fn discard(&mut self, batch: zork_observe::BatchId) -> bool {
        self.source.discard(batch)
    }
    pub fn snapshot(&mut self) -> DeviceUpdate {
        if let Some(update) = self.prepare() {
            self.acknowledge(update.batch.unwrap());
            return update;
        }
        let state = self.source.current();
        DeviceUpdate {
            state: state.value,
            domains: Domains::default(),
            batch: None,
            cursor: state.cursor,
            reset: false,
        }
    }
    pub async fn changed(&mut self) -> Option<DeviceUpdate> {
        loop {
            self.ready().await.ok()?;
            if let Some(update) = self.prepare() {
                self.acknowledge(update.batch.unwrap());
                return Some(update);
            }
        }
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
struct Reading {
    session: Option<String>,
    visible: bool,
    following_tail: bool,
}
struct Owned {
    data: DeviceData,
    seen: HashMap<String, String>,
    reading: Reading,
    notifications: crate::notifications::Ledger,
    notifications_dirty: bool,
}

/// One shared authority per configured device and store. Every platform view
/// observes it; no view-to-view business snapshot forwarding is necessary.
pub struct Device {
    pub(super) replication: super::replication::Replication,
    pub(super) client: Arc<StationClient>,
    profiles: std::sync::OnceLock<Arc<super::Profiles>>,
    mesh_admin: std::sync::OnceLock<Arc<super::MeshAdmin>>,
    profile_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    pub(super) cache: Option<(Arc<ClientStore>, String)>,
    pub(super) drafts: Mutex<HashMap<String, Arc<Observable<super::Draft>>>>,
    pub(crate) draft_gate: Mutex<()>,
    pub(super) conversations: Mutex<HashMap<String, std::sync::Weak<super::Conversation>>>,
    pub(super) recent_conversations: Mutex<std::collections::VecDeque<Arc<super::Conversation>>>,
    pub(super) outbox: Observable<super::Outbox>,
    pub(super) agents: std::sync::OnceLock<Arc<super::Agents>>,
    self_weak: std::sync::OnceLock<std::sync::Weak<Self>>,
    agent_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    pub(super) outbox_gate: Mutex<()>,
    pub(super) delivery_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    agents_enabled: bool,
    owned: Mutex<Owned>,
    data: Observable<DeviceData>,
    navigation: Observable<NavigationData>,
    notifications: Observable<crate::notifications::Ledger>,
    refresh_gate: tokio::sync::Mutex<()>,
    pub(super) upgrade_gate: tokio::sync::Mutex<()>,
    pub(super) operation_stopped: zork_notify::Notifier,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}
impl Drop for Device {
    fn drop(&mut self) {
        if let Some(task) = self.agent_task.get_mut().unwrap().take() {
            task.abort();
        }
        if let Some(task) = self.task.get_mut().unwrap().take() {
            task.abort();
        }
        if let Some(task) = self.delivery_task.get_mut().unwrap().take() {
            task.abort();
        }
        if let Some(task) = self.profile_task.get_mut().unwrap().take() {
            task.abort();
        }
    }
}
impl Device {
    pub fn open(
        client: Arc<StationClient>,
        cache: Option<(Arc<ClientStore>, String)>,
        agents_enabled: bool,
    ) -> Arc<Self> {
        if let Some((store, node)) = cache.as_ref() {
            let mut registry = store.1.lock().unwrap();
            if let Some(existing) = registry.get(node).and_then(std::sync::Weak::upgrade) {
                if existing.client.same_connection(&client)
                    && existing.agents_enabled == agents_enabled
                    && !existing.snapshot().revoked
                {
                    return existing;
                }
            }
            let device = Self::create(client, cache.clone(), agents_enabled);
            registry.insert(node.clone(), Arc::downgrade(&device));
            device
        } else {
            Self::create(client, None, agents_enabled)
        }
    }
    fn create(
        client: Arc<StationClient>,
        cache: Option<(Arc<ClientStore>, String)>,
        agents_enabled: bool,
    ) -> Arc<Self> {
        fn cached_value<T: DeserializeOwned>(
            cache: &Option<(Arc<ClientStore>, String)>,
            key: &str,
        ) -> Option<T> {
            cache.as_ref().and_then(|(store, node)| {
                if let Some(value) = store.get(node, key).ok().flatten() {
                    return Some(value);
                }
                let path = match key {
                    "agents" => "/v1/node/agents",
                    "sessions" => "/v1/im/sessions",
                    _ => return None,
                };
                let value: Value = store.get(node, &format!("http:{path}")).ok().flatten()?;
                serde_json::from_value(value["body"]["items"].clone()).ok()
            })
        }
        fn cached<T: DeserializeOwned + Default>(
            cache: &Option<(Arc<ClientStore>, String)>,
            key: &str,
        ) -> T {
            cached_value(cache, key).unwrap_or_default()
        }
        let agents = cached_value::<Vec<Value>>(&cache, "agents");
        let agents_loaded = agents.is_some();
        let agents = agents.unwrap_or_default();
        let tasks = agents
            .iter()
            .filter_map(|a| {
                a["id"]
                    .as_str()
                    .map(|id| (id.to_owned(), cached(&cache, &format!("leader-tasks:{id}"))))
            })
            .collect();
        let mut sessions: Vec<SessionSummary> = cached(&cache, "sessions");
        for session in &mut sessions {
            session.status = crate::api::SessionStatus::Wait;
            session.runtime_available = false;
            session.can_send = None;
        }

        let mut data = DeviceData {
            agents_loaded,
            sessions_loaded: cache.as_ref().is_some_and(|(s, n)| {
                ["sessions", "http:/v1/im/sessions"]
                    .iter()
                    .any(|key| s.get::<Value>(n, key).ok().flatten().is_some())
            }),
            sessions: Arc::new(sessions),
            agents: Arc::new(agents),
            tasks: Arc::new(tasks),
            chats: cached::<Option<Vec<zork_client_types::chat::Channel>>>(&cache, "chats")
                .map(Arc::new),
            inbox: Arc::new(cached(&cache, "inbox")),
            artifacts: Arc::new(cached(&cache, "drive")),
            pages: Arc::new(cached(&cache, "pages")),
            read_markers: Arc::new(cached(&cache, "read-markers")),
            ..Default::default()
        };
        if let Some((store, node)) = &cache {
            if let Ok(links) = store.message_links(node) {
                crate::pages::merge_message_links(Arc::make_mut(&mut data.pages), links);
            }
        }
        data.content_indices =
            Arc::new(crate::pages::content_indices(&data.artifacts, &data.pages));
        let owned = Owned {
            data: data.clone(),
            seen: cached(&cache, "navigation-seen"),
            reading: Reading::default(),
            notifications: cached(&cache, crate::notifications::KEY),
            notifications_dirty: false,
        };
        let navigation = Self::project_navigation(&owned);
        let notifications = Observable::new(owned.notifications.clone());
        let device = Arc::new(Self {
            replication: Default::default(),
            self_weak: Default::default(),
            client,
            profiles: std::sync::OnceLock::new(),
            mesh_admin: std::sync::OnceLock::new(),
            profile_task: Mutex::new(None),
            cache,
            drafts: Mutex::new(HashMap::new()),
            draft_gate: Mutex::new(()),
            conversations: Mutex::new(HashMap::new()),
            recent_conversations: Mutex::new(Default::default()),
            outbox: Observable::new(Default::default()),
            agents: std::sync::OnceLock::new(),
            agent_task: Mutex::new(None),
            outbox_gate: Mutex::new(()),
            delivery_task: Mutex::new(None),
            agents_enabled,
            owned: Mutex::new(owned),
            data: Observable::new(data),
            navigation: Observable::new(navigation),
            notifications,
            refresh_gate: tokio::sync::Mutex::new(()),
            upgrade_gate: tokio::sync::Mutex::new(()),
            operation_stopped: Default::default(),
            task: Mutex::new(None),
        });
        let _ = device.self_weak.set(Arc::downgrade(&device));
        if let Err(error) = device.load_replica_catalog() {
            device.commit(|s| s.connection_error = Some(format!("local replica: {error}")));
        }
        device.reload_outbox();
        device
    }
    pub fn profiles(&self) -> Arc<super::Profiles> {
        self.profiles
            .get_or_init(|| {
                let source = super::Profiles::new(self.client.clone());
                if let Some(owner) = self.self_weak.get() {
                    source.bind_device(owner.clone());
                }
                source
            })
            .clone()
    }
    pub fn mesh_admin(&self) -> Arc<super::MeshAdmin> {
        self.mesh_admin
            .get_or_init(|| {
                super::MeshAdmin::new(
                    self.client.clone(),
                    self.self_weak.get().cloned().unwrap_or_default(),
                )
            })
            .clone()
    }
    pub fn snapshot(&self) -> Arc<DeviceData> {
        self.data.read()
    }
    pub(crate) fn bound_peer(&self) -> Option<&str> {
        self.cache.as_ref().map(|(_, peer)| peer.as_str())
    }
    pub fn agents(&self) -> Arc<super::Agents> {
        self.agents
            .get_or_init(|| {
                let source = super::Agents::new(self.client.clone(), self.profiles());
                if let Some(owner) = self.self_weak.get() {
                    source.bind_device(owner.clone());
                }
                let state = self.snapshot();
                source.seed_agents(state.agents.clone(), state.agents_loaded);
                source
            })
            .clone()
    }
    pub fn subscribe(&self) -> DeviceSubscription {
        self.subscribe_domains(Domains::ALL)
    }
    pub fn subscribe_domains(&self, domains: Domains) -> DeviceSubscription {
        DeviceSubscription {
            source: self
                .data
                .subscribe_topics(zork_observe::Topics::new(domains.0 as u64)),
        }
    }
    pub fn has_long_term_agents(&self) -> bool {
        self.navigation
            .read()
            .agents
            .iter()
            .any(|agent| agent.can_open)
    }
    pub fn agent_availability(&self) -> AgentAvailability {
        let state = self.snapshot();
        if state.revoked {
            return AgentAvailability::Unavailable;
        }
        if !state.agents_loaded && state.agents.is_empty() {
            return if state.connection_error.is_some() || state.online == Some(false) {
                AgentAvailability::Unavailable
            } else {
                AgentAvailability::Loading
            };
        }
        if self.has_long_term_agents() {
            AgentAvailability::Available
        } else {
            AgentAvailability::Empty
        }
    }
    pub fn navigation(&self) -> Subscription<NavigationData> {
        self.navigation.subscribe()
    }
    pub(crate) fn stop_sync(&self) {
        self.operation_stopped.notify();
        for task in [
            &self.task,
            &self.profile_task,
            &self.agent_task,
            &self.delivery_task,
        ] {
            if let Some(task) = task.lock().unwrap().take() {
                task.abort();
            }
        }
        self.commit(|s| {
            s.online = None;
            s.route = Default::default();
            s.confirmed_at_ms = None;
        });
    }
    pub fn start(self: &Arc<Self>) {
        let mut task = self.task.lock().unwrap();
        if task.is_some() {
            return;
        }
        let mut profiles = self.profiles().subscribe();
        profiles.snapshot();
        let weak = Arc::downgrade(self);
        *self.profile_task.lock().unwrap() = Some(self.client.spawn(async move {
            while let Some(update) = profiles.changed().await {
                let Some(device) = weak.upgrade() else {
                    return;
                };
                if device.replica_active() {
                    continue;
                }
                device.commit(|s| s.profiles = update.state.profiles.clone());
                if let Some(agents) = device.agents.get() {
                    agents.sync_profiles(update.state.profiles.clone());
                }
            }
        }));
        let mut agents = self.agents().subscribe();
        agents.snapshot();
        let weak = Arc::downgrade(self);
        *self.agent_task.lock().unwrap() = Some(self.client.spawn(async move {
            while let Some(update) = agents.changed().await {
                let Some(device) = weak.upgrade() else {
                    return;
                };
                if device.replica_active() {
                    continue;
                }
                if update.agents_changed {
                    if update.state.loaded {
                        device.persist("agents", &update.state.agents);
                    }
                    device.commit(|s| {
                        s.agents = update.state.agents.clone();
                        s.agents_loaded = update.state.loaded;
                    });
                }
            }
        }));
        let weak = Arc::downgrade(self);
        let client = self.client.clone();
        *task = Some(self.client.spawn(async move {
            let mut feed = client.live(None, 100);
            let mut previous: Option<Value> = None;
            let mut pending = Domains::default();
            let mut retry = zork_notify::retry::Retry::default();
            let mut retry_at: Option<tokio::time::Instant> = None;
            loop {
                let event = tokio::select! {
                    event=feed.next()=>match event {Some(event)=>event,None=>return},
                    _ = async {
                        match retry_at {
                            Some(deadline) => tokio::time::sleep_until(deadline).await,
                            None => std::future::pending().await,
                        }
                    } => {
                        let Some(device) = weak.upgrade() else { return; };
                        pending = device.refresh_pending(pending).await;
                        if device.snapshot().revoked { return; }
                        retry_at = if pending.is_empty() {
                            retry.reset();
                            None
                        } else { Some(tokio::time::Instant::now() + retry.delay()) };
                        continue;
                    }
                };
                let Some(device) = weak.upgrade() else {
                    return;
                };
                match event {
                    LiveEvent::Connected => {
                        previous = None;
                        device.commit(|state| state.route = Default::default());
                    }
                    LiveEvent::Route(route) => device.commit(|state| state.route = route),
                    LiveEvent::Disconnected { error, revoked } => {
                        // The shared feed owns reconnect attempts. Retain unmet
                        // demand, but do not compete with it using business reads.
                        retry_at = None;
                        device.commit(|state| {
                            state.online = Some(false);
                            state.route = Default::default();
                            state.revoked = revoked;
                            state.connection_error = Some(error);
                        });
                        if revoked {
                            if let Err(error) = device.revoke_replica_access() {
                                device.commit(|s| s.connection_error = Some(error.to_string()));
                            }
                            return;
                        }
                    }
                    LiveEvent::Event(event) if event.name == "changed" => {
                        if let Ok(revision) = serde_json::from_str::<Value>(&event.data) {
                            let changed = |key: &str| {
                                previous
                                    .as_ref()
                                    .is_none_or(|old| old[key] != revision[key])
                            };
                            let mut domains = Domains::default();
                            if changed("profiles") {
                                domains.insert(Domains::PROFILES);
                            }
                            if changed("sessions") || changed("tasks") {
                                domains.insert(Domains::SESSIONS);
                            }
                            if changed("agents") {
                                domains.insert(Domains::AGENTS);
                            }
                            if changed("tasks") || changed("agents") {
                                domains.insert(Domains::TASKS | Domains::INBOX);
                            }
                            if changed("artifacts") {
                                domains.insert(Domains::ARTIFACTS);
                            }
                            if changed("mesh") {
                                domains.insert(Domains::MESH);
                            }
                            let target = revision.get("catalog").cloned().and_then(|value| {
                                serde_json::from_value::<zork_client_types::sync::Cursor>(value)
                                    .ok()
                            });
                            // Buffered/duplicate hints already covered by a completed
                            // pull must not queue another network synchronization.
                            if target
                                .as_ref()
                                .is_some_and(|cursor| device.replica_caught_up(cursor))
                            {
                                let snapshot = device.snapshot();
                                if snapshot.online != Some(true)
                                    || snapshot.connection_error.is_some()
                                {
                                    device.commit(|s| {
                                        s.online = Some(true);
                                        s.revoked = false;
                                        s.connection_error = None;
                                        s.confirmed_at_ms = Some(crate::store::delivery_now_ms());
                                    });
                                }
                                if domains.contains(Domains::MESH) {
                                    match device.client.mesh_status().await {
                                        Ok(mesh) => device.commit(|s| s.mesh = Arc::new(mesh)),
                                        Err(_) => pending.insert(Domains::MESH),
                                    }
                                }
                            } else {
                                if target.is_some() {
                                    domains.insert(Domains::SESSIONS);
                                }
                                domains.insert(pending);
                                pending = device.refresh_pending(domains).await;
                            }
                            if device.snapshot().revoked {
                                return;
                            }
                            retry_at = if pending.is_empty() {
                                retry.reset();
                                None
                            } else {
                                Some(tokio::time::Instant::now() + retry.delay())
                            };
                            previous = Some(revision);
                        }
                    }
                    _ => {}
                }
            }
        }));
    }
    pub(super) fn persist<T: serde::Serialize>(&self, key: &str, value: &T) {
        if let Some((store, node)) = &self.cache {
            if let Err(error) = store.put(node, key, value) {
                eprintln!("client state cache: {error}");
            }
        }
    }
    pub(super) fn commit(&self, change: impl FnOnce(&mut DeviceData)) {
        let mut owned = self.owned.lock().unwrap();
        let before = owned.data.clone();
        change(&mut owned.data);
        let after = &mut owned.data;
        let pages_changed = before.pages != after.pages;
        if !pages_changed {
            after.pages = before.pages.clone();
        }
        let changed = [
            (
                before.online,
                before.revoked,
                &before.connection_error,
                before.confirmed_at_ms,
            ) != (
                after.online,
                after.revoked,
                &after.connection_error,
                after.confirmed_at_ms,
            ) || before.route != after.route
                || before.info != after.info
                || before.metadata_loaded != after.metadata_loaded,
            before.sessions != after.sessions || before.sessions_loaded != after.sessions_loaded,
            before.agents != after.agents || before.agents_loaded != after.agents_loaded,
            before.tasks != after.tasks
                || before.chats != after.chats
                || before.read_markers != after.read_markers,
            before.profiles != after.profiles,
            before.inbox != after.inbox
                || before.inbox_loaded != after.inbox_loaded
                || before.inbox_error != after.inbox_error,
            before.artifacts != after.artifacts
                || before.artifacts_loaded != after.artifacts_loaded
                || before.artifacts_error != after.artifacts_error
                || pages_changed,
            before.mesh != after.mesh,
        ];
        if !changed.iter().any(|v| *v) {
            return;
        }
        if before.artifacts != after.artifacts
            || before.pages.references != after.pages.references
            || before.pages.files != after.pages.files
        {
            after.content_indices = Arc::new(crate::pages::content_indices(
                &after.artifacts,
                &after.pages,
            ));
        }
        for (index, changed) in changed.into_iter().enumerate() {
            if changed {
                after.revisions[index] = after.revisions[index].wrapping_add(1);
            }
        }
        if changed[0] || changed[1] || changed[3] || changed[5] {
            self.update_notifications(&mut owned, true);
        }
        self.mark_seen(&mut owned);
        if !before.revoked && owned.data.revoked {
            self.data.invalidate(owned.data.clone());
        } else {
            let bits = changed
                .iter()
                .enumerate()
                .fold(0, |bits, (index, changed)| {
                    bits | if *changed { 1 << index } else { 0 }
                });
            self.data
                .publish_changed(owned.data.clone(), zork_observe::Topics::new(bits));
        }
        // A navigation action may run as soon as this projection wakes. Its
        // device snapshot must already contain the advertised identity/state.
        self.navigation.publish(Self::project_navigation(&owned));
        let data = owned.data.clone();
        drop(owned);
        if changed[0] || changed[1] {
            let conversations = self
                .conversations
                .lock()
                .unwrap()
                .values()
                .filter_map(std::sync::Weak::upgrade)
                .collect::<Vec<_>>();
            for conversation in conversations {
                conversation.confirm_status(&data.sessions, data.online == Some(true));
            }
        }
    }
    fn project_navigation(owned: &Owned) -> NavigationData {
        let data = &owned.data;
        NavigationData::project(
            data,
            data.read_markers
                .iter()
                .filter(|m| {
                    m.role == Role::Assistant
                        && owned.seen.get(&m.session_id) != Some(&m.last_message_id)
                })
                .map(|m| m.session_id.clone())
                .collect(),
        )
    }
    pub fn notifications(&self) -> Subscription<crate::notifications::Ledger> {
        self.notifications.subscribe()
    }
    fn update_notifications(&self, owned: &mut Owned, reconcile: bool) {
        owned.notifications_dirty |= reconcile;
        let Some((store, node)) = &self.cache else {
            return;
        };
        let result = (|| -> anyhow::Result<()> {
            let prefs = crate::notifications::preferences(store)?;
            let mut next = owned.notifications.clone();
            let reading = owned
                .reading
                .visible
                .then_some(owned.reading.session.as_deref())
                .flatten();
            if owned.notifications_dirty {
                next.reconcile(
                    node,
                    &owned.data,
                    reading,
                    &prefs,
                    crate::store::delivery_now_ms(),
                );
            }
            next.filter(node, reading, &prefs, crate::store::delivery_now_ms());
            if next != owned.notifications {
                store.put(node, crate::notifications::KEY, &next)?;
                owned.notifications = next.clone();
                self.notifications.publish(next);
            }
            owned.notifications_dirty = false;
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("notification state: {error}");
        }
    }
    pub fn refresh_notifications(&self) {
        self.update_notifications(&mut self.owned.lock().unwrap(), false);
    }
    pub fn acknowledge_notification(&self, id: &str) -> anyhow::Result<()> {
        let mut owned = self.owned.lock().unwrap();
        let Some((store, node)) = &self.cache else {
            return Ok(());
        };
        let mut next = owned.notifications.clone();
        next.acknowledge(id);
        if next != owned.notifications {
            store.put(node, crate::notifications::KEY, &next)?;
            owned.notifications = next.clone();
            self.notifications.publish(next);
        }
        Ok(())
    }
    pub fn discard_notification(&self, id: &str) -> anyhow::Result<()> {
        let mut owned = self.owned.lock().unwrap();
        let Some((store, node)) = &self.cache else {
            return Ok(());
        };
        let mut next = owned.notifications.clone();
        next.pending.retain(|_, notice| notice.id != id);
        if next != owned.notifications {
            store.put(node, crate::notifications::KEY, &next)?;
            owned.notifications = next.clone();
            self.notifications.publish(next);
        }
        Ok(())
    }
    fn mark_seen(&self, owned: &mut Owned) {
        if !owned.reading.visible || !owned.reading.following_tail {
            return;
        }
        let Some(marker) = owned
            .data
            .read_markers
            .iter()
            .find(|m| Some(&m.session_id) == owned.reading.session.as_ref())
        else {
            return;
        };
        if owned.seen.get(&marker.session_id) != Some(&marker.last_message_id) {
            owned
                .seen
                .insert(marker.session_id.clone(), marker.last_message_id.clone());
            self.persist("navigation-seen", &owned.seen);
        }
    }
    /// A platform reports what the user can see, not whether a message is read.
    /// The core applies and persists the read-marker rule.
    pub fn report_view(&self, session: Option<String>, visible: bool, following_tail: bool) {
        let mut owned = self.owned.lock().unwrap();
        owned.reading = Reading {
            session,
            visible,
            following_tail,
        };
        self.update_notifications(&mut owned, false);
        self.mark_seen(&mut owned);
        self.navigation.publish(Self::project_navigation(&owned));
    }
    /// Explicit refresh is a core command; revisions, merging and publications
    /// remain here even when a platform asks to refresh after an edit.
    pub async fn refresh(&self, domains: Domains) {
        self.refresh_pending(domains).await;
    }
    /// Return only the unmet demand. The live driver retries failures and
    /// becomes completely idle once this set is empty.
    async fn refresh_pending(&self, domains: Domains) -> Domains {
        if domains.is_empty() {
            return Domains::default();
        }
        let mut failed = Domains::default();
        let _serial = self.refresh_gate.lock().await;
        match self.refresh_replica_catalog().await {
            Ok(true) => {
                if domains.contains(Domains::SESSIONS) {
                    if !self.refresh_replica_presence().await {
                        failed.insert(Domains::SESSIONS);
                    }
                }
                if domains.contains(Domains::MESH) {
                    match self.client.mesh_status().await {
                        Ok(mesh) => self.commit(|s| s.mesh = Arc::new(mesh)),
                        Err(_) => failed.insert(Domains::MESH),
                    }
                }
                return failed;
            }
            Err(error) => {
                self.commit(|s| s.connection_error = Some(format!("sync: {error}")));
                return domains;
            }
            Ok(false) => {}
        }
        if domains.contains(Domains::PROFILES) {
            let source = self.profiles();
            source.refresh_statuses().await;
            let state = source.snapshot();
            if state.error.is_some() {
                failed.insert(Domains::PROFILES);
            }
            let profiles = state.profiles.clone();
            self.commit(|s| s.profiles = profiles);
        }
        if domains.contains(Domains::SESSIONS) {
            match self.client.list_sessions().await {
                Ok(mut sessions) => {
                    crate::conversation::merge_sessions(&self.snapshot().sessions, &mut sessions);
                    self.persist("sessions", &sessions);
                    self.commit(|s| {
                        s.sessions = Arc::new(sessions);
                        s.sessions_loaded = true;
                        s.online = Some(true);
                        s.revoked = false;
                        s.connection_error = None;
                        s.confirmed_at_ms = Some(crate::store::delivery_now_ms());
                    });
                }
                Err(error) => {
                    failed.insert(Domains::SESSIONS);
                    self.commit(|s| {
                        s.online = Some(false);
                        s.connection_error = Some(format!("sessions: {error}"));
                    });
                }
            }
        }
        if self.agents_enabled && domains.contains(Domains::AGENTS) {
            let source = self.agents();
            if source.refresh_agents().await.is_ok() {
                let agents = source.snapshot().agents.clone();
                self.persist("agents", &agents);
                self.commit(|s| {
                    s.agents = agents;
                    s.agents_loaded = true;
                });
                for conversation in self
                    .conversations
                    .lock()
                    .unwrap()
                    .values()
                    .filter_map(std::sync::Weak::upgrade)
                    .collect::<Vec<_>>()
                {
                    conversation.refresh();
                }
            } else {
                failed.insert(Domains::AGENTS);
            }
        }
        if self.agents_enabled
            && domains.contains(Domains::TASKS | Domains::AGENTS | Domains::SESSIONS)
        {
            match self
                .client
                .node_request(reqwest::Method::GET, "/v1/node/chats".into(), None)
                .await
            {
                Ok(value) if value["schema_version"] == 1 => {
                    match serde_json::from_value::<Vec<zork_client_types::chat::Channel>>(
                        value["items"].clone(),
                    ) {
                        Ok(chats) => {
                            self.persist("chats", &chats);
                            self.commit(|s| s.chats = Some(Arc::new(chats)));
                        }
                        Err(_) => failed.insert(Domains::TASKS),
                    }
                }
                Err(crate::api::ApiError::Api { status: 404, .. })
                    if self.snapshot().chats.is_none() => {}
                _ => failed.insert(Domains::TASKS),
            }
        }
        if self.agents_enabled && domains.contains(Domains::TASKS | Domains::AGENTS) {
            let leaders: Vec<_> = self
                .snapshot()
                .agents
                .iter()
                .filter(|a| a["role"] == "leader")
                .filter_map(|a| a["id"].as_str().map(str::to_owned))
                .collect();
            let mut tasks = self.snapshot().tasks.as_ref().clone();
            tasks.retain(|id, _| leaders.contains(id));
            for id in leaders {
                if let Ok(value) = self
                    .client
                    .node_request(
                        reqwest::Method::GET,
                        format!("/v1/node/agents/{id}/tasks"),
                        None,
                    )
                    .await
                {
                    if let Ok(items) =
                        serde_json::from_value::<Vec<ProductTask>>(value["items"].clone())
                    {
                        self.persist(&format!("leader-tasks:{id}"), &items);
                        tasks.insert(id, items);
                    } else {
                        failed.insert(Domains::TASKS);
                    }
                } else {
                    failed.insert(Domains::TASKS);
                }
            }
            self.commit(|s| s.tasks = Arc::new(tasks));
        }
        if domains.contains(Domains::INBOX) {
            match self.client.inbox_tasks().await {
                Ok(mut tasks) => {
                    let sessions = self.snapshot().sessions.clone();
                    tasks.retain(|task| {
                        sessions
                            .iter()
                            .find_map(|s| s.task.as_ref().filter(|t| t.task_id == task.task_id))
                            .is_none_or(|current| current.revision <= task.revision)
                    });
                    self.persist("inbox", &tasks);
                    self.commit(|s| {
                        s.inbox = Arc::new(tasks);
                        s.inbox_loaded = true;
                        s.inbox_error = None;
                    });
                }
                Err(error) => {
                    failed.insert(Domains::INBOX);
                    self.commit(|s| s.inbox_error = Some(error.to_string()));
                }
            }
        }
        if self.agents_enabled
            && domains.contains(Domains::SESSIONS | Domains::TASKS | Domains::AGENTS)
        {
            if let Ok(value) = self
                .client
                .node_request(
                    reqwest::Method::GET,
                    "/v1/node/conversations/read-markers".into(),
                    None,
                )
                .await
            {
                if let Ok(markers) =
                    serde_json::from_value::<Vec<ConversationReadMarker>>(value["items"].clone())
                {
                    self.persist("read-markers", &markers);
                    self.commit(|s| s.read_markers = Arc::new(markers));
                } else {
                    failed.insert(Domains::TASKS);
                }
            } else {
                failed.insert(Domains::TASKS);
            }
        }
        if domains.contains(Domains::ARTIFACTS) {
            match self.client.page_catalog().await {
                Ok(Some(pages)) => {
                    self.persist("pages", &pages);
                    self.commit(|s| s.pages = Arc::new(pages));
                }
                Ok(None) => {}
                Err(_) => failed.insert(Domains::ARTIFACTS),
            }
            match self.client.artifacts().await {
                Ok(items) => {
                    self.persist("drive", &items);
                    self.commit(|s| {
                        s.artifacts = Arc::new(items);
                        s.artifacts_loaded = true;
                        s.artifacts_error = None;
                    });
                }
                Err(error) => {
                    failed.insert(Domains::ARTIFACTS);
                    self.commit(|s| s.artifacts_error = Some(error.to_string()));
                }
            }
        }
        if domains.contains(Domains::MESH) {
            match self.client.mesh_status().await {
                Ok(mesh) => self.commit(|s| s.mesh = Arc::new(mesh)),
                Err(_) => failed.insert(Domains::MESH),
            }
        }
        failed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;

    #[test]
    fn navigation_wakeup_can_resolve_the_advertised_agent_from_device_state() {
        use std::{
            sync::atomic::{AtomicBool, Ordering},
            task::{Context, Wake, Waker},
        };
        struct Observe {
            device: Arc<Device>,
            ready: AtomicBool,
        }
        impl Wake for Observe {
            fn wake(self: Arc<Self>) {
                let state = self.device.snapshot();
                self.ready.store(
                    state.online == Some(true)
                        && state.agents.iter().any(|a| a["id"] == "new-partner"),
                    Ordering::SeqCst,
                );
            }
        }
        let (_root, _store, device) = device();
        let mut navigation = device.navigation();
        navigation.snapshot();
        let mut readiness = navigation.readiness();
        let observed = Arc::new(Observe {
            device: device.clone(),
            ready: AtomicBool::new(false),
        });
        let waker = Waker::from(observed.clone());
        assert!(readiness
            .poll_changed(&mut Context::from_waker(&waker))
            .is_pending());
        device.commit(|state| {
            state.online = Some(true);
            state.agents = Arc::new(vec![
                serde_json::json!({"id":"new-partner", "role":"leader"}),
            ]);
        });
        assert!(
            observed.ready.load(Ordering::SeqCst),
            "navigation advertised an agent before its authoritative device snapshot"
        );
    }

    #[tokio::test]
    async fn upgrade_observes_current_operation_and_pause_does_not_repeat_the_command() {
        use axum::{routing::post, Json, Router};
        use std::{
            sync::atomic::{AtomicUsize, Ordering},
            time::Duration,
        };
        let submitted = Arc::new(tokio::sync::Notify::new());
        let count = Arc::new(AtomicUsize::new(0));
        let router = Router::new().route("/v1/node/update", post({
            let (submitted, count) = (submitted.clone(), count.clone());
            move || {
                let attempt = count.fetch_add(1, Ordering::SeqCst);
                submitted.notify_one();
                async move { Json(serde_json::json!({"update":{"status":{"operation_id":format!("attempt-{attempt}")}}})) }
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = Arc::new(StationClient::new(
            format!("http://{}", listener.local_addr().unwrap()),
            None,
        ));
        let server = zork_notify::Task(tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        }));
        let device = Device::open(client, None, false);
        // The existing feed owns this slot; drive its committed states directly
        // to isolate the upgrade monitor from unrelated catalog synchronization.
        *device.task.lock().unwrap() = Some(tokio::spawn(std::future::pending()));
        let status = |operation: &str| serde_json::json!({"update":{"status":{"version":"1.2.3","operation_id":operation,"phase":"complete"}}});
        device.commit(|state| state.info = Arc::new(status("older-attempt")));
        let operation = device.clone();
        let first = tokio::spawn(async move { operation.upgrade("1.2.3", |_| Ok(())).await });
        submitted.notified().await;
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(
            !first.is_finished(),
            "cached completion must not finish a new upgrade"
        );
        assert!(device.upgrade("1.2.3", |_| Ok(())).await.is_err());
        device.commit(|state| state.info = Arc::new(status("attempt-0")));
        tokio::time::timeout(Duration::from_secs(2), first)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);

        let operation = device.clone();
        let second = tokio::spawn(async move { operation.upgrade("1.2.3", |_| Ok(())).await });
        submitted.notified().await;
        device.stop_sync();
        assert!(tokio::time::timeout(Duration::from_secs(2), second)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        assert_eq!(count.load(Ordering::SeqCst), 2);
        drop(server);
    }

    #[test]
    fn settings_deadline_survives_starting_the_observer_task_after_expiration() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _runtime = runtime.enter();
        let (_directory, store, device) = device();
        device.commit(|s| {
            s.online = Some(true);
            s.confirmed_at_ms = Some(crate::store::delivery_now_ms() - 59_000);
        });
        let peer = device.bound_peer().unwrap().to_owned();
        let mut reader = crate::subscriptions::WireSubscription::from_device(
            crate::subscriptions::Key::Settings { peer },
            device,
            store,
        )
        .unwrap();
        let mut signals = reader.signals();
        signals.changed().now_or_never().unwrap().unwrap();
        let first = reader.prepare().unwrap().unwrap();
        assert_eq!(first["snapshot"]["online"], true);
        reader.finish(first["batch"].as_u64().unwrap(), true);
        // The Rust task has not been polled yet. Its first scheduling happens
        // after the initially published online value has expired.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        runtime.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(2), signals.changed())
                .await
                .unwrap()
                .unwrap();
            let expired = reader.prepare().unwrap().unwrap();
            assert_eq!(expired["snapshot"]["online"], false);
            reader.finish(expired["batch"].as_u64().unwrap(), true);
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(30), signals.changed())
                    .await
                    .is_err(),
                "an expired deadline installed a repeating wake"
            );
        });
    }
    fn device() -> (tempfile::TempDir, Arc<ClientStore>, Arc<Device>) {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(ClientStore::open(directory.path()).unwrap());
        let client = Arc::new(StationClient::new("http://127.0.0.1:9", None));
        let device = Device::open(client, Some((store.clone(), "node".into())), true);
        (directory, store, device)
    }

    #[test]
    fn startup_distinguishes_unknown_agent_content_from_a_cached_empty_catalog() {
        let (_root, store, initial) = device();
        assert_eq!(initial.agent_availability(), AgentAvailability::Loading);
        initial.commit(|s| s.connection_error = Some("not connected".into()));
        assert_eq!(initial.agent_availability(), AgentAvailability::Unavailable);
        drop(initial);
        let open = || {
            Device::open(
                Arc::new(StationClient::new("http://127.0.0.1:9", None)),
                Some((store.clone(), "node".into())),
                true,
            )
        };
        store.put("node", "agents", &serde_json::json!([])).unwrap();
        let cached = open();
        assert_eq!(cached.agent_availability(), AgentAvailability::Empty);
        cached.commit(|s| s.connection_error = Some("offline".into()));
        assert_eq!(cached.agent_availability(), AgentAvailability::Empty);
        drop(cached);
        store
            .put("node", "agents", &serde_json::json!({"invalid":"catalog"}))
            .unwrap();
        assert_eq!(open().agent_availability(), AgentAvailability::Loading);
    }
    #[tokio::test]
    async fn route_changes_update_navigation_without_business_refresh() {
        use crate::api::{ConnectionRoute, ConnectionScope};
        let (_directory, _store, device) = device();
        let mut navigation = device.navigation();
        navigation.snapshot();
        let mut updates = device.subscribe();
        updates.snapshot();
        for route in [
            ConnectionRoute {
                scope: ConnectionScope::Public,
                direct: false,
            },
            ConnectionRoute {
                scope: ConnectionScope::Public,
                direct: true,
            },
            ConnectionRoute {
                scope: ConnectionScope::Lan,
                direct: true,
            },
            ConnectionRoute::default(),
        ] {
            device.commit(|state| state.route = route);
            assert_eq!(navigation.changed().await.unwrap().route, route);
            let changed = updates.changed().await.unwrap();
            assert!(!changed.domains.contains(Domains::SESSIONS));
            assert!(!changed.domains.contains(Domains::MESH));
            device.commit(|state| state.route = route);
            assert!(navigation.changed().now_or_never().is_none());
            assert!(updates.changed().now_or_never().is_none());
        }
        device.commit(|state| {
            state.online = Some(true);
            state.route = ConnectionRoute {
                scope: ConnectionScope::Public,
                direct: true,
            };
        });
        device.stop_sync();
        assert_eq!(device.snapshot().route, ConnectionRoute::default());
        assert_eq!(device.snapshot().online, None);
    }

    #[tokio::test]
    async fn subscriptions_filter_business_topics_and_coalesce_without_losing_changes() {
        let (_directory, _store, device) = device();
        let mut navigation = device.navigation();
        navigation.snapshot();
        let mut all = device.subscribe();
        all.snapshot();
        device.commit(|s| {
            s.profiles = Arc::new(vec![serde_json::from_value(
                serde_json::json!({"profile_id":"p","provider":"test"}),
            )
            .unwrap()])
        });
        assert!(
            navigation.changed().now_or_never().is_none(),
            "profile update notified navigation"
        );
        device.commit(|s| s.agents = Arc::new(vec![serde_json::json!({"id":"a","role":"leader"})]));
        let update = all.changed().await.unwrap();
        assert!(update.domains.contains(Domains::PROFILES));
        assert!(update.domains.contains(Domains::AGENTS));
        assert!(!update.domains.contains(Domains::SESSIONS));
        assert_eq!(navigation.changed().await.unwrap().agents.len(), 1);
        device.commit(|s| s.agents = Arc::new(vec![serde_json::json!({"id":"a","role":"leader"})]));
        assert!(
            all.changed().now_or_never().is_none(),
            "identical state notified subscribers"
        );
        assert!(navigation.changed().now_or_never().is_none());
    }
    #[tokio::test]
    async fn navigation_wire_retries_unapplied_unread_changes_and_clears_revoked_content() {
        use crate::subscriptions::{Key, WireSubscription};
        let (_directory, store, device) = device();
        let mut wire = WireSubscription::from_device(
            Key::Conversation {
                peer: "node".into(),
                session: None,
            },
            device.clone(),
            store,
        )
        .unwrap();
        let initial = wire.prepare().unwrap().unwrap();
        assert_eq!(
            initial["state"]["navigation"]["agents"],
            serde_json::json!([])
        );
        assert!(wire.finish(initial["batch"].as_u64().unwrap(), true));
        device.commit(|s| {
            s.agents = Arc::new(vec![
                serde_json::json!({"id":"a","name":"A","role":"leader"}),
            ]);
            s.chats = Some(Arc::new(vec![serde_json::from_value(serde_json::json!({
                "chat_id":"chat","title":"Task","created_at":"now","message_count":1,
                "creator":{"id":"a","kind":"agent"}
            }))
            .unwrap()]));
            s.read_markers = Arc::new(vec![ConversationReadMarker {
                session_id: "chat".into(),
                last_message_id: "m1".into(),
                created_at: "now".into(),
                role: Role::Assistant,
            }]);
        });
        let unread = wire.prepare().unwrap().unwrap();
        assert_eq!(
            unread["state"]["navigation"]["tasks"]["a"][0]["unread"],
            true
        );
        assert!(wire.finish(unread["batch"].as_u64().unwrap(), true));
        device.report_view(Some("chat".into()), true, true);
        let read = wire.prepare().unwrap().unwrap();
        assert_eq!(
            read["state"]["navigation"]["tasks"]["a"][0]["unread"],
            false
        );
        assert!(wire.finish(read["batch"].as_u64().unwrap(), false));
        let retry = wire.prepare().unwrap().unwrap();
        assert_eq!(retry["state"]["navigation"], read["state"]["navigation"]);
        assert!(wire.finish(retry["batch"].as_u64().unwrap(), true));
        assert!(wire.prepare().unwrap().is_none());
        device.commit(|s| s.revoked = true);
        let revoked = wire.prepare().unwrap().unwrap();
        assert_eq!(
            revoked["state"]["navigation"]["tasks"],
            serde_json::json!({})
        );
        assert_eq!(
            revoked["state"]["navigation"]["agents"],
            serde_json::json!([])
        );
    }

    #[tokio::test]
    async fn core_owns_unread_and_only_marks_the_visible_tail() {
        let (_directory, store, device) = device();
        device.commit(|s| {
            s.read_markers = Arc::new(vec![ConversationReadMarker {
                session_id: "session".into(),
                last_message_id: "message".into(),
                created_at: "now".into(),
                role: Role::Assistant,
            }])
        });
        let mut navigation = device.navigation();
        assert!(navigation.snapshot().unread.contains("session"));
        device.report_view(Some("session".into()), false, true);
        assert!(navigation.changed().now_or_never().is_none());
        device.report_view(Some("session".into()), true, false);
        assert!(navigation.changed().now_or_never().is_none());
        device.report_view(Some("session".into()), true, true);
        assert!(navigation.changed().await.unwrap().unread.is_empty());
        assert_eq!(
            store
                .get::<HashMap<String, String>>("node", "navigation-seen")
                .unwrap()
                .unwrap()["session"],
            "message"
        );
        device.report_view(Some("session".into()), true, true);
        assert!(navigation.changed().now_or_never().is_none());
    }
    #[tokio::test]
    async fn notifications_are_durable_and_receipts_do_not_mark_conversations_read() {
        let (_directory, store, device) = device();
        let now = chrono::Utc::now().to_rfc3339();
        device.commit(|s| {
            s.online = Some(true);
            s.sessions_loaded = true;
            s.inbox_loaded = true;
            s.read_markers = Arc::new(vec![ConversationReadMarker {
                session_id: "session".into(),
                last_message_id: "old".into(),
                created_at: now.clone(),
                role: Role::Assistant,
            }]);
        });
        let mut notifications = device.notifications();
        assert!(notifications.snapshot().pending.is_empty());
        device.commit(|s| Arc::make_mut(&mut s.read_markers)[0].last_message_id = "new".into());
        let ledger = notifications.changed().await.unwrap();
        let notice = ledger.pending.values().next().unwrap().clone();
        assert_eq!(
            store
                .get::<crate::notifications::Ledger>("node", crate::notifications::KEY)
                .unwrap()
                .unwrap()
                .pending
                .get(&notice.tag()),
            Some(&notice)
        );
        device.acknowledge_notification(&notice.id).unwrap();
        assert!(device.navigation().snapshot().unread.contains("session"));
        notifications.snapshot();
        device.report_view(Some("session".into()), true, false);
        assert!(notifications.changed().await.unwrap().presented.is_empty());
        assert!(
            device.navigation().snapshot().unread.contains("session"),
            "a visible history viewport is not a read receipt"
        );
        device.report_view(Some("session".into()), true, false);
        assert!(
            notifications.changed().now_or_never().is_none(),
            "identical view reports must remain quiet"
        );
    }
    #[test]
    fn one_store_and_device_have_one_authority_and_credentials_replace_it() {
        let (_directory, store, device) = device();
        let again = Device::open(
            Arc::new(StationClient::new("http://127.0.0.1:9", None)),
            Some((store.clone(), "node".into())),
            true,
        );
        assert!(Arc::ptr_eq(&again, &device));
        let replacement = Device::open(
            Arc::new(StationClient::new(
                "http://127.0.0.1:9",
                Some("replacement".into()),
            )),
            Some((store, "node".into())),
            true,
        );
        assert!(!Arc::ptr_eq(&replacement, &device));
    }
    #[tokio::test]
    async fn drafts_publish_only_committed_changes_and_do_not_notify_navigation() {
        let (_directory, store, device) = device();
        let mut navigation = device.navigation();
        navigation.snapshot();
        let mut draft = device.subscribe_draft("session");
        draft.snapshot();
        device.edit_draft("session", "你好".into()).unwrap();
        assert_eq!(draft.changed().await.unwrap().text, "你好");
        device.edit_draft("session", "你好".into()).unwrap();
        assert!(draft.changed().now_or_never().is_none());
        assert!(navigation.changed().now_or_never().is_none());
        let saved: String = store.get("node", "draft:session").unwrap().unwrap();
        assert_eq!(saved, "你好");
        device.clear_draft("session").unwrap();
        assert!(draft.changed().await.unwrap().text.is_empty());
    }
}
