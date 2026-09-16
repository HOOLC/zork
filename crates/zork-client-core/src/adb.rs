//! Owns phone readiness and independent, phone-initiated Station ADB leases.
mod presentation;
mod probe;
#[cfg(test)]
mod tests;

use crate::store::ClientStore;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{net::TcpStream, sync::watch};
use zork_client_types::adb::{Advertisement, Facts, HostState, LocalState};
use zork_mesh::node::MeshNode;
use zork_observe::ValueSource;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    enabled: bool,
    port: u16,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            port: 5555,
        }
    }
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    SetEnabled { enabled: bool },
    SetPort { port: String },
    Facts { facts: Facts },
    Refresh,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Station {
    id: String,
    origin: String,
    name: String,
}
#[derive(Clone)]
struct Lease {
    station: Station,
    generation: String,
    state: HostState,
    serial: Option<String>,
    cancel: watch::Sender<bool>,
}
#[derive(Clone)]
struct State {
    settings: Settings,
    facts: Facts,
    local: LocalState,
    stations: BTreeMap<String, Lease>,
    active: bool,
    revision: u64,
    configuration: u64,
}
impl State {
    fn clear_connections(&mut self) {
        for lease in self.stations.values() {
            lease.cancel.send_replace(true);
        }
        self.stations.clear();
    }
    fn reconfigure(&mut self) {
        self.configuration += 1;
        self.revision += 1;
        self.local = LocalState::Checking;
        self.clear_connections();
    }
}
pub struct Controller {
    pub(crate) source: ValueSource<Value>,
    store: Arc<ClientStore>,
    state: Mutex<State>,
    publishing: Mutex<()>,
    wake: watch::Sender<u64>,
    task: Mutex<Option<zork_notify::Task<()>>>,
}
impl Controller {
    pub fn new(store: Arc<ClientStore>) -> Result<Arc<Self>> {
        // Older persisted settings may contain a selected peer. It no longer
        // constrains discovery; enabled and the activation port are preserved.
        let settings = store.get("", "adb-settings")?.unwrap_or_default();
        let controller = Arc::new(Self {
            source: ValueSource::new(Value::Null),
            publishing: Mutex::new(()),
            store,
            state: Mutex::new(State {
                settings,
                facts: Facts::default(),
                local: LocalState::Checking,
                stations: BTreeMap::new(),
                active: false,
                revision: 0,
                configuration: 0,
            }),
            wake: watch::channel(0).0,
            task: Mutex::new(None),
        });
        controller.publish();
        Ok(controller)
    }
    fn stations(&self) -> Result<BTreeMap<String, Station>> {
        let mut stations = BTreeMap::new();
        for saved in self.store.nodes()? {
            if self.store.replica_revoked(&saved.id)? {
                continue;
            }
            if let Some(mesh) = saved.mesh {
                stations.insert(
                    saved.id.clone(),
                    Station {
                        id: saved.id,
                        origin: mesh.origin,
                        name: if saved.name.trim().is_empty() {
                            "Station".into()
                        } else {
                            saved.name
                        },
                    },
                );
            }
        }
        Ok(stations)
    }
    pub fn requested(&self) -> bool {
        let state = self.state.lock().expect("ADB state");
        state.settings.enabled && state.facts.supported
    }
    pub fn snapshot(&self) -> Value {
        let mut state = self.state.lock().expect("ADB state").clone();
        // Fence prepared UI frames as soon as the authoritative membership
        // changes, even before the controller processes its cancellation wake.
        let stations = self.stations().unwrap_or_default();
        state.stations.retain(|id, lease| {
            stations
                .get(id)
                .is_some_and(|station| station.origin == lease.station.origin)
        });
        presentation::snapshot(&state)
    }
    pub(crate) fn publish(&self) {
        let _publishing = self.publishing.lock().expect("ADB publication");
        self.source.publish(self.snapshot());
    }
    pub(crate) fn devices_changed(&self) {
        self.wake.send_modify(|version| *version += 1);
    }
    pub(crate) fn peer_revoked(&self, peer: &str) {
        let mut state = self.state.lock().expect("ADB state");
        if let Some(lease) = state.stations.remove(peer) {
            lease.cancel.send_replace(true);
            state.revision += 1;
        }
        drop(state);
        self.publish();
        self.devices_changed();
    }
    pub fn execute(&self, action: Action) -> Result<Value> {
        {
            let mut state = self.state.lock().expect("ADB state");
            match action {
                Action::SetEnabled { enabled } => {
                    ensure!(
                        !enabled || state.facts.supported,
                        "当前设备不支持 Android 调试"
                    );
                    if enabled != state.settings.enabled {
                        let settings = Settings {
                            enabled,
                            ..state.settings.clone()
                        };
                        self.store.put("", "adb-settings", &settings)?;
                        state.settings = settings;
                        state.reconfigure();
                    }
                }
                Action::SetPort { port } => {
                    let port = port
                        .trim()
                        .parse::<u16>()
                        .ok()
                        .filter(|port| *port >= 1024)
                        .context("ADB 端口需在 1024–65535 之间")?;
                    if port != state.settings.port {
                        let settings = Settings {
                            port,
                            ..state.settings.clone()
                        };
                        self.store.put("", "adb-settings", &settings)?;
                        state.settings = settings;
                        state.reconfigure();
                    }
                }
                Action::Facts { facts } => {
                    ensure!(
                        facts.application_id.len() <= 255
                            && facts
                                .application_id
                                .bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_'),
                        "invalid Android application ID"
                    );
                    ensure!(
                        facts.name.len() <= 128 && !facts.name.chars().any(char::is_control),
                        "invalid ADB device name"
                    );
                    if facts == state.facts {
                        drop(state);
                        return Ok(self.snapshot());
                    }
                    if facts.application_id != state.facts.application_id
                        || facts.name != state.facts.name
                        || facts.supported != state.facts.supported
                    {
                        state.reconfigure();
                    } else {
                        state.revision += 1;
                    }
                    state.facts = facts;
                }
                Action::Refresh => {}
            }
        }
        self.devices_changed();
        self.publish();
        Ok(self.snapshot())
    }
    pub fn start(self: &Arc<Self>, node: MeshNode) {
        let mut task = self.task.lock().expect("ADB task");
        if task.as_ref().is_some_and(|task| !task.is_finished()) {
            return;
        }
        {
            let mut state = self.state.lock().expect("ADB state");
            state.active = true;
            state.reconfigure();
        }
        let owner = self.clone();
        *task = Some(zork_notify::Task(tokio::spawn(async move {
            owner.run(node).await;
        })));
        self.publish();
    }
    pub fn pause(&self) {
        self.task.lock().expect("ADB task").take();
        let mut state = self.state.lock().expect("ADB state");
        if !state.active {
            return;
        }
        state.active = false;
        state.reconfigure();
        drop(state);
        self.publish();
    }
    fn reconcile(
        &self,
        configuration: u64,
        facts: &Facts,
        local: LocalState,
        stations: BTreeMap<String, Station>,
    ) -> Option<Vec<Lease>> {
        let mut state = self.state.lock().expect("ADB state");
        if state.configuration != configuration || &state.facts != facts || !state.active {
            return None;
        }
        let mut changed = false;
        if state.local != local {
            state.local = local;
            state.clear_connections();
            changed = true;
        }
        let stations = if state.settings.enabled && facts.supported {
            stations
        } else {
            BTreeMap::new()
        };
        state.stations.retain(|id, lease| {
            let keep = stations
                .get(id)
                .is_some_and(|station| station.origin == lease.station.origin);
            if !keep {
                lease.cancel.send_replace(true);
                changed = true;
            }
            keep
        });
        for (id, station) in stations {
            match state.stations.get_mut(&id) {
                Some(lease) => {
                    if lease.station.name != station.name {
                        lease.station.name = station.name;
                        changed = true;
                    }
                }
                None => {
                    state.stations.insert(
                        id,
                        Lease {
                            station,
                            generation: ulid::Ulid::new().to_string(),
                            state: HostState::Connecting,
                            serial: None,
                            cancel: watch::channel(false).0,
                        },
                    );
                    changed = true;
                }
            }
        }
        if changed {
            state.revision += 1;
        }
        let leases = state.stations.values().cloned().collect();
        drop(state);
        if changed {
            self.publish();
        }
        Some(leases)
    }
    fn update_station(
        &self,
        peer: &str,
        generation: &str,
        remote: HostState,
        serial: Option<String>,
    ) {
        let mut state = self.state.lock().expect("ADB state");
        if !state.active || !state.settings.enabled {
            return;
        }
        let Some(lease) = state
            .stations
            .get_mut(peer)
            .filter(|lease| lease.generation == generation)
        else {
            return;
        };
        if lease.state == remote && lease.serial == serial {
            return;
        }
        lease.state = remote;
        lease.serial = serial;
        state.revision += 1;
        drop(state);
        self.publish();
    }
    async fn run(self: Arc<Self>, node: MeshNode) {
        let mut wake = self.wake.subscribe();
        let mut workers: BTreeMap<String, (String, zork_notify::Task<()>)> = BTreeMap::new();
        let mut misses = 0;
        loop {
            wake.borrow_and_update();
            let (configuration, settings, facts, previous) = {
                let state = self.state.lock().expect("ADB state");
                if !state.active {
                    return;
                }
                (
                    state.configuration,
                    state.settings.clone(),
                    state.facts.clone(),
                    state.local,
                )
            };
            let detecting = settings.enabled && facts.supported;
            let mut local = if detecting {
                tokio::select! {
                    _ = wake.changed() => continue,
                    local = probe::probe(settings.port, &facts) => local,
                }
            } else {
                LocalState::Checking
            };
            if detecting
                && previous == LocalState::Ready
                && local != LocalState::Ready
                && facts.developer_enabled != Some(false)
                && facts.usb_enabled != Some(false)
            {
                misses += 1;
                if misses < 2 {
                    local = previous;
                }
            } else {
                misses = 0;
            }
            let stations = self.stations().unwrap_or_default();
            let Some(leases) = self.reconcile(configuration, &facts, local, stations) else {
                continue;
            };
            workers.retain(|id, (generation, task)| {
                !task.is_finished()
                    && leases
                        .iter()
                        .any(|lease| lease.station.id == *id && lease.generation == *generation)
            });
            for lease in leases {
                if workers.contains_key(&lease.station.id) {
                    continue;
                }
                let owner = self.clone();
                let node = node.clone();
                let settings = settings.clone();
                let facts = facts.clone();
                let station = lease.station;
                let id = station.id.clone();
                let generation = lease.generation;
                let token = generation.clone();
                let cancelled = lease.cancel.subscribe();
                workers.insert(
                    id,
                    (
                        generation,
                        zork_notify::Task(tokio::spawn(async move {
                            owner
                                .run_station(
                                    node, station, token, settings, facts, local, cancelled,
                                )
                                .await;
                        })),
                    ),
                );
            }
            if detecting {
                tokio::select! {
                    _ = wake.changed() => {},
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {},
                }
            } else if wake.changed().await.is_err() {
                return;
            }
        }
    }
    async fn run_station(
        self: Arc<Self>,
        node: MeshNode,
        station: Station,
        generation: String,
        settings: Settings,
        facts: Facts,
        local: LocalState,
        mut cancelled: watch::Receiver<bool>,
    ) {
        let mut failures = 0_u32;
        loop {
            if *cancelled.borrow() {
                return;
            }
            let failure = tokio::select! {
                biased;
                _ = cancelled.changed() => return,
                result = self.connect(&node, &station, &generation, &settings, &facts, local) => result.ok().flatten().unwrap_or(HostState::Offline),
            };
            self.update_station(&station.id, &generation, failure, None);
            failures = (failures + 1).min(5);
            tokio::select! {
                _ = cancelled.changed() => return,
                _ = tokio::time::sleep(Duration::from_secs((1 << failures).min(30))) => {},
            }
        }
    }
    async fn connect(
        &self,
        node: &MeshNode,
        station: &Station,
        lease: &str,
        settings: &Settings,
        facts: &Facts,
        local: LocalState,
    ) -> Result<Option<HostState>> {
        let generation = ulid::Ulid::new().to_string();
        let body = Advertisement {
            generation: generation.clone(),
            name: facts.name.clone(),
            activation_port: settings.port,
            bridge_application_id: facts.application_id.clone(),
            local,
        };
        let mut control = node
            .subscribe(
                &station.origin,
                &json!({"v":1,"request":{"kind":"adb_register","body":body}}),
            )
            .await?;
        let mut streams = tokio::task::JoinSet::new();
        loop {
            let frame = tokio::select! {
                frame = tokio::time::timeout(Duration::from_secs(45), control.next()) => frame??.context("ADB bridge disconnected")?,
                Some(_) = streams.join_next(), if !streams.is_empty() => continue,
            };
            if frame["ok"] == false {
                return Ok(Some(
                    if frame["error"].as_str() == Some("mesh_client_not_granted") {
                        HostState::AccessDenied
                    } else {
                        HostState::BridgeUnavailable
                    },
                ));
            }
            if frame["generation"].as_str() != Some(&generation) {
                return Ok(Some(HostState::BridgeUnavailable));
            }
            match frame["event"].as_str() {
                Some("state") => {
                    let state = serde_json::from_value::<HostState>(frame["state"].clone())?;
                    let serial = frame["serial"].as_str().map(str::to_owned);
                    self.update_station(&station.id, lease, state, serial);
                }
                Some("open") => {
                    ensure!(local == LocalState::Ready, "ADB unavailable");
                    ensure!(streams.len() < 16, "ADB stream limit");
                    let id = frame["id"]
                        .as_str()
                        .context("missing ADB stream ID")?
                        .to_owned();
                    ensure!(id.parse::<ulid::Ulid>().is_ok(), "invalid ADB stream ID");
                    let node = node.clone();
                    let origin = station.origin.clone();
                    let generation = generation.clone();
                    let port = settings.port;
                    streams.spawn(async move {
                        let mut socket = tokio::time::timeout(Duration::from_secs(3),
                            TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))).await??;
                        let tunnel = node.connect_tunnel(&origin, &json!({"v":1,"request":{"kind":"adb_stream","generation":generation,"id":id}})).await?;
                        tunnel.forward(&mut socket, &[]).await
                    });
                }
                _ => anyhow::bail!("invalid ADB bridge event"),
            }
        }
    }
}
