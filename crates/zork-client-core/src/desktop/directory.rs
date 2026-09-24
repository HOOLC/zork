//! Device directory, membership and host operations. Views only observe `DirectoryData`.
use super::{node::LocalNode, transport::ClientMesh};
use crate::{
    api::StationClient,
    state::{Device, Observable, Subscription},
    store::{ClientStore, RemoteNode, SavedNode},
};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{Arc, Mutex},
};

#[derive(Clone, Default, PartialEq)]
pub struct DirectoryData {
    pub nodes: Arc<Vec<SavedNode>>,
    pub local_enabled: bool,
    pub mesh_identity: Option<String>,
    pub device_statuses: Arc<HashMap<String, zork_client_types::device::DeviceStatus>>,
    pub info: Arc<HashMap<String, Value>>,
    pub updating: HashSet<String>,
    pub error: Option<String>,
    pub local_running: bool,
    pub local_background: bool,
    pub local_at_login: bool,
    pub preferences: crate::preferences::ClientPreferences,
    pub applications: Arc<Vec<crate::pages::ApplicationEntry>>,
}

pub struct Directory {
    pub data_reset: Arc<crate::data_reset::Controller>,
    data_lease: Arc<super::data_reset::Lease>,
    pub store: Arc<ClientStore>,
    pub local: Arc<LocalNode>,
    pub transport: Arc<ClientMesh>,
    owned: Mutex<DirectoryData>,
    state: Observable<DirectoryData>,
    revisions: Mutex<HashMap<String, u64>>,
    status_publication: Mutex<()>,
    sync_gate: tokio::sync::Mutex<()>,
    devices: Mutex<HashMap<String, (Arc<Device>, tokio::task::JoinHandle<()>)>>,
    connections: Mutex<HashMap<String, Connection>>,
    next_binding: std::sync::atomic::AtomicU64,
    host_watch: Mutex<Option<zork_notify::files::FileWatch>>,
    host_observer: Mutex<Option<(u64, HostConnection)>>,
    host_generation: std::sync::atomic::AtomicU64,
    pub account: Arc<crate::relay_account::controller::Controller>,
    application_sources: Mutex<HashMap<String, crate::pages::ApplicationSource>>,
    resources: std::sync::OnceLock<Arc<crate::resources::Resources>>,
}
struct HostConnection(std::os::unix::net::UnixStream);
struct Connection {
    key: (String, Option<String>, Option<String>),
    binding: u64,
    client: Arc<StationClient>,
}
impl Drop for HostConnection {
    fn drop(&mut self) {
        let _ = self.0.shutdown(std::net::Shutdown::Both);
    }
}
impl Directory {
    pub fn open(root: &Path) -> Result<Arc<Self>> {
        Self::open_with_account(root, None)
    }

    #[cfg(test)]
    pub(crate) fn fixture(root: &Path) -> Result<Arc<Self>> {
        let account = crate::relay_account::controller::Controller::fixture(Default::default());
        Self::open_with_account(root, Some(account))
    }

    fn open_with_account(
        root: &Path,
        account: Option<Arc<crate::relay_account::controller::Controller>>,
    ) -> Result<Arc<Self>> {
        let data_lease = super::data_reset::acquire(root)?;
        let store = Arc::new(ClientStore::open(root)?);
        let (local_enabled, error) = match store.local_node_enabled() {
            Ok(value) => (value, None),
            Err(e) => (false, Some(e.to_string())),
        };
        zork_config::relay_account::bind_profile(root)?;
        let account = match account {
            Some(account) => account,
            None => crate::relay_account::controller::Controller::open(root)?,
        };
        let state = DirectoryData {
            nodes: Arc::new(store.nodes()?),
            local_enabled,
            error,
            preferences: crate::preferences::read(&store),
            ..Default::default()
        };
        let directory = Arc::new(Self {
            data_reset: Arc::new(crate::data_reset::Controller::default()),
            data_lease,
            transport: Arc::new(ClientMesh::with_store(
                root.join("transport"),
                store.clone(),
            )),
            local: Arc::new(LocalNode::new(root.join("node"))),
            store,
            owned: Mutex::new(state.clone()),
            state: Observable::new(state),
            revisions: Mutex::new(HashMap::new()),
            status_publication: Default::default(),
            sync_gate: tokio::sync::Mutex::new(()),
            devices: Mutex::new(HashMap::new()),
            host_watch: Default::default(),
            host_observer: Default::default(),
            host_generation: Default::default(),
            account,
            connections: Mutex::new(HashMap::new()),
            next_binding: std::sync::atomic::AtomicU64::new(1),
            application_sources: Default::default(),
            resources: Default::default(),
        });
        let weak = Arc::downgrade(&directory);
        directory.transport.observe(move |readiness| {
            if let Some(directory) = weak.upgrade() {
                let devices = directory
                    .devices
                    .lock()
                    .unwrap()
                    .values()
                    .map(|(device, _)| device.clone())
                    .collect::<Vec<_>>();
                for device in devices {
                    device.set_mesh_readiness(readiness.clone());
                }
                directory.publish_device_statuses();
            }
        });
        directory.publish_device_statuses();
        let node_root = directory.local.root().to_owned();
        let socket = zork_config::zork_sock_path(&node_root);
        let mut roots = vec![(node_root.clone(), false), (node_root.join("run"), false)];
        if !socket.starts_with(&node_root) {
            roots.push((
                socket
                    .parent()
                    .context("supervisor socket directory")?
                    .to_owned(),
                false,
            ));
        }
        let weak = Arc::downgrade(&directory);
        let watch = zork_notify::files::watch_paths(
            roots,
            move |path| {
                path == socket
                    || path == node_root.join("service.json")
                    || path == node_root.join("zork.pid")
            },
            move |_| {
                if let Some(directory) = weak.upgrade() {
                    directory.refresh_host_status();
                }
            },
        )?;
        *directory.host_watch.lock().unwrap() = Some(watch);
        let weak = Arc::downgrade(&directory);
        std::thread::spawn(move || {
            if let Some(directory) = weak.upgrade() {
                directory.refresh_host_status();
            }
        });
        Ok(directory)
    }
    /// File changes announce settings/startup; an observation socket announces
    /// supervisor exit, including a crash that leaves its PID/socket files behind.
    fn refresh_host_status(self: &Arc<Self>) {
        let mut observer = self.host_observer.lock().unwrap();
        if observer.is_some() {
            self.local.refresh_settings();
        } else {
            self.local.refresh_status();
        }
        self.commit(|s| {
            s.local_running = self.local.running();
            s.local_background = self.local.background();
            s.local_at_login = self.local.start_at_login();
        });
        if !self.local.running() {
            observer.take();
            return;
        }
        if observer.is_some() {
            return;
        }
        let Ok((mut stream, reply)) = zork_config::service::connect(self.local.root(), "observe")
        else {
            return;
        };
        if !serde_json::from_str::<Value>(&reply).is_ok_and(|v| v["protocol"] == 1) {
            return;
        }
        let Ok(handle) = stream.try_clone() else {
            return;
        };
        if stream.set_read_timeout(None).is_err() {
            return;
        }
        let generation = self
            .host_generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        *observer = Some((generation, HostConnection(handle)));
        let weak = Arc::downgrade(self);
        std::thread::spawn(move || {
            use std::io::Read;
            let mut byte = [0u8; 1];
            while matches!(stream.read(&mut byte), Ok(1..)) {}
            let Some(directory) = weak.upgrade() else {
                return;
            };
            let mut observer = directory.host_observer.lock().unwrap();
            if observer.as_ref().is_none_or(|(id, _)| *id != generation) {
                return;
            }
            observer.take();
            drop(observer);
            directory.refresh_host_status();
        });
    }
    pub fn snapshot(&self) -> Arc<DirectoryData> {
        self.state.read()
    }
    pub fn clear_data(self: &Arc<Self>, confirmed: bool) {
        let source = self.clone();
        std::thread::spawn(move || {
            let _ = source.data_reset.clear(confirmed, || {
                let _ = source.cancel_account();
                source.local.stop()?;
                super::data_reset::restart(&source.data_lease)
            });
        });
    }
    pub fn subscribe(&self) -> Subscription<DirectoryData> {
        self.state.subscribe()
    }
    fn commit(&self, change: impl FnOnce(&mut DirectoryData)) {
        let mut state = self.owned.lock().unwrap();
        change(&mut state);
        self.state.publish(state.clone());
    }
    fn publish_device_statuses(&self) {
        let _publication = self.status_publication.lock().unwrap();
        let snapshot = self.snapshot();
        let readiness = self.transport.readiness();
        let devices = self.devices.lock().unwrap();
        let statuses = snapshot
            .nodes
            .iter()
            .map(|node| {
                let data = devices.get(&node.id).map(|(device, _)| device.snapshot());
                let status = crate::device_status::project(
                    Some(&readiness),
                    data.as_ref().and_then(|data| data.online),
                    &data.as_ref().map(|data| data.route).unwrap_or_default(),
                    data.as_ref().is_some_and(|data| data.revoked),
                );
                (node.id.clone(), status)
            })
            .collect::<HashMap<_, _>>();
        drop(devices);
        if *snapshot.device_statuses != statuses {
            if let Some(resources) = self.resources.get() {
                resources.set_device_statuses(&statuses);
            }
            self.commit(|s| s.device_statuses = Arc::new(statuses));
        }
    }
    fn publish_nodes(&self) -> Result<()> {
        let nodes = self.store.nodes()?;
        let changed = self.snapshot().nodes.as_ref() != &nodes;
        self.commit(|s| s.nodes = Arc::new(nodes));
        if changed {
            self.publish_device_statuses();
            self.publish_applications();
            if let Some(resources) = self.resources.get() {
                resources.replace_devices(self.resource_clients());
                resources.set_device_statuses(&self.snapshot().device_statuses);
            }
        }
        Ok(())
    }
    fn resource_clients(&self) -> Vec<(String, String, Arc<StationClient>)> {
        self.snapshot()
            .nodes
            .iter()
            .filter_map(|node| {
                self.connection(&node.id)
                    .ok()
                    .map(|(_, client)| (node.id.clone(), node.name.clone(), client))
            })
            .collect()
    }
    pub fn inspection_node(
        &self,
        current: &str,
        target: &crate::resources::InspectionTarget,
    ) -> Result<String> {
        let nodes = self.snapshot().nodes.clone();
        match &target.origin {
            None => {
                anyhow::ensure!(nodes.iter().any(|node| node.id == current), "设备已移除");
                Ok(current.into())
            }
            Some(origin) if origin == "local" => {
                anyhow::ensure!(nodes.iter().any(|node| node.id == current), "设备已移除");
                Ok(current.into())
            }
            Some(origin) => {
                let sources = self.application_sources.lock().unwrap();
                nodes
                    .iter()
                    .find(|node| {
                        node.mesh
                            .as_ref()
                            .is_some_and(|mesh| &mesh.origin == origin)
                            || sources
                                .get(&node.id)
                                .is_some_and(|source| source.origin.as_ref() == Some(origin))
                    })
                    .map(|node| node.id.clone())
                    .context("运行设备尚未连接")
            }
        }
    }
    pub fn resources(&self) -> Arc<crate::resources::Resources> {
        self.resources
            .get_or_init(|| {
                let source = crate::resources::Resources::new(self.resource_clients());
                source.set_device_statuses(&self.snapshot().device_statuses);
                source
            })
            .clone()
    }
    fn accept_applications(&self, id: &str, data: &crate::state::DeviceData) {
        let mut sources = self.application_sources.lock().unwrap();
        if data.revoked {
            sources.remove(id);
        } else {
            let origin = data.mesh.origin.clone().or_else(|| {
                data.info
                    .pointer("/sync/owner")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
            if sources.get(id).is_some_and(|s| {
                s.applications.as_ref() == &data.pages.applications
                    && s.online == data.online
                    && s.status == data.status
                    && s.origin == origin
            }) {
                return;
            }
            sources.insert(
                id.into(),
                crate::pages::ApplicationSource {
                    applications: Arc::new(data.pages.applications.clone()),
                    online: data.online,
                    status: data.status.clone(),
                    origin,
                },
            );
        }
        drop(sources);
        self.publish_applications();
    }
    fn publish_applications(&self) {
        let nodes = self
            .snapshot()
            .nodes
            .iter()
            .map(|n| (n.id.clone(), n.name.clone()))
            .collect::<Vec<_>>();
        let mut sources = self.application_sources.lock().unwrap();
        sources.retain(|id, _| nodes.iter().any(|(node, _)| node == id));
        let applications = crate::pages::applications(&nodes, &sources);
        drop(sources);
        self.commit(|s| s.applications = Arc::new(applications));
    }
    pub fn device_status(&self, id: &str) -> zork_client_types::device::DeviceStatus {
        self.snapshot()
            .device_statuses
            .get(id)
            .cloned()
            .unwrap_or_default()
    }
    pub fn node(&self, id: &str) -> Option<SavedNode> {
        self.snapshot()
            .nodes
            .iter()
            .find(|node| node.id == id)
            .cloned()
    }
    pub fn connection(&self, id: &str) -> Result<(u64, Arc<StationClient>)> {
        let node = self.node(id).context("设备已移除")?;
        let key = (
            node.url.clone(),
            node.token.clone(),
            node.mesh.as_ref().map(|m| m.origin.clone()),
        );
        let mut connections = self.connections.lock().unwrap();
        if let Some(connection) = connections.get(id).filter(|c| c.key == key) {
            return Ok((connection.binding, connection.client.clone()));
        }
        let client = Arc::new(match node.mesh.clone() {
            Some(remote) => StationClient::new_mesh(self.transport.control(), remote.origin),
            None => StationClient::new(node.url.clone(), node.token.clone())
                .with_service_mesh(self.transport.control()),
        });
        let binding = self
            .next_binding
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        connections.insert(
            id.to_owned(),
            Connection {
                key,
                binding,
                client: client.clone(),
            },
        );
        Ok((binding, client))
    }
    fn ensure_connection(&self, id: &str, binding: u64) -> Result<()> {
        anyhow::ensure!(self.connection(id)?.0 == binding, "设备连接已变化，请重试");
        Ok(())
    }
    pub fn bind(self: &Arc<Self>, id: String, device: Arc<Device>, client: Arc<StationClient>) {
        let binding = self.connection(&id).ok().map(|(binding, _)| binding);
        let mut devices = self.devices.lock().unwrap();
        if devices
            .get(&id)
            .is_some_and(|(old, _)| Arc::ptr_eq(old, &device))
        {
            return;
        }
        if let Some((_, task)) = devices.remove(&id) {
            task.abort();
        }
        device.set_mesh_readiness(self.transport.readiness());
        let mut updates = device.subscribe();
        let weak = Arc::downgrade(self);
        let anchor = id.clone();
        let observed_device = device.clone();
        let task = client.spawn(async move {
            let mut retry = zork_notify::retry::Retry::default();
            let mut previous_online = None;
            loop {
                let mut failed = false;
                let update = updates.snapshot();
                if let Some(directory) = weak.upgrade() {
                    if binding.is_some_and(|binding|directory.ensure_connection(&anchor,binding).is_err()){return;}
                    if update.domains.contains(crate::state::Domains::CONNECTION) {
                        observed_device.set_mesh_readiness(directory.transport.readiness());
                        directory.publish_device_statuses();
                    }
                    directory.accept_applications(&anchor,&update.state);

                    if update.state.online != previous_online {
                        previous_online = update.state.online;
                        if directory.snapshot().nodes.iter().any(|node| node.id == anchor && node.local) {
                            let host = directory.clone();
                            tokio::task::spawn_blocking(move || host.refresh_host_status());
                        }
                    }
                    if let (Some(origin),Some(group)) = (&update.state.mesh.origin,&update.state.mesh.group) {
                        if let Err(error) = directory.sync_members(&anchor,origin,group.clone()).await {
                            failed = true;
                            directory.commit(|s|s.error=Some(format!("设备列表暂未同步：{error}")));
                        }
                    }
                    let _ = directory.publish_nodes();
                } else { return; }
                if failed {
                    tokio::select! { change=updates.changed()=>if change.is_none(){return;}, _=retry.wait()=>{} }
                } else {
                    retry.reset();
                    if updates.changed().await.is_none() { return; }
                }
            }
        });
        devices.insert(id, (device, task));
    }
    pub fn remote_input(origin: String, name: String, address: String) -> Result<SavedNode> {
        let name = zork_config::membership::validate_device_name(&name)?;
        let origin = origin.trim().to_owned();
        anyhow::ensure!(origin.starts_with("key:"), "请填写有效的设备身份");
        let addr = address.trim();
        Ok(SavedNode {
            id: origin.clone(),
            name,
            url: String::new(),
            token: None,
            local: false,
            group: None,
            mesh: Some(RemoteNode {
                routes: None,
                origin,
                addr: (!addr.is_empty()).then(|| addr.into()),
            }),
        })
    }
    pub fn pair(&self, node: Option<SavedNode>) -> Result<Option<SavedNode>> {
        let mut nodes = self.store.nodes()?;
        if let Some(node) = &node {
            nodes.retain(|n| n.id != node.id);
            nodes.push(node.clone());
        }
        let identity = self.transport.start(&nodes)?;
        if let Some(node) = &node {
            self.store.save_node(node)?;
        }
        self.publish_nodes()?;
        self.commit(|s| s.mesh_identity = Some(identity));
        Ok(node)
    }
    pub fn start_local(&self) -> Result<SavedNode> {
        self.store.set_local_node_enabled(true)?;
        self.commit(|s| s.local_enabled = true);
        let node = self.local.start()?;
        self.store.save_node(&node)?;
        self.publish_nodes()?;
        Ok(node)
    }
    pub fn stop_local(&self) -> Result<()> {
        self.store.set_local_node_enabled(false)?;
        self.commit(|s| s.local_enabled = false);
        self.local.stop()
    }
    pub fn login_account(&self) -> Result<()> {
        self.account
            .submit(crate::relay_account::controller::Action::Login)
    }
    pub fn cancel_account(&self) -> Result<()> {
        self.account
            .submit(crate::relay_account::controller::Action::Cancel)
    }
    pub fn logout(&self) -> Result<()> {
        self.account
            .submit(crate::relay_account::controller::Action::Logout)
    }
    pub fn select(&self, id: &str) -> Result<()> {
        self.store.put("device", "last-node", &id)
    }
    pub fn new_chat_devices(&self, current: &str) -> zork_client_types::new_chat::Choice {
        let snapshot = self.snapshot();
        zork_client_types::new_chat::Choice {
            value: current.into(),
            options: snapshot
                .nodes
                .iter()
                .map(|node| zork_client_types::new_chat::OptionItem {
                    value: node.id.clone(),
                    label: node.name.clone(),
                    status: Some(
                        snapshot
                            .device_statuses
                            .get(&node.id)
                            .cloned()
                            .unwrap_or_default(),
                    ),
                })
                .collect(),
        }
    }
    /// Resolve a creation destination without replacing either device's persistent draft.
    pub fn new_chat_destination(&self, current: &str, target: &str) -> Result<SavedNode> {
        let devices = self.devices.lock().unwrap();
        let source = devices.get(current).context("当前设备不可用")?;
        let draft = source.0.new_chat().snapshot();
        anyhow::ensure!(
            !draft.busy && draft.pending.is_none(),
            "请先处理未确认的创建操作"
        );
        anyhow::ensure!(!source.0.snapshot().revoked, "设备访问权限已撤销");
        self.node(target).context("所选设备已移除")
    }
    pub fn selected(&self) -> Option<String> {
        self.store.get("device", "last-node").ok().flatten()
    }
    pub fn save_theme(&self, theme: crate::preferences::Theme) -> Result<()> {
        let preferences = crate::preferences::save_theme(&self.store, theme)?;
        self.commit(|s| s.preferences = preferences);
        Ok(())
    }
    pub async fn refresh_info(&self, node: &SavedNode) -> Result<()> {
        let (binding, client) = self.connection(&node.id)?;
        let result = client
            .node_request(http::Method::GET, "/v1/node/info".into(), None)
            .await;
        self.ensure_connection(&node.id, binding)?;
        match result {
            Ok(value) => {
                crate::device_metadata::record(&self.store, &node.id, "/v1/node/info", &value)?;
                self.commit(|s| {
                    Arc::make_mut(&mut s.info).insert(node.id.clone(), value);
                });
                self.publish_nodes()
            }
            Err(error) => {
                self.commit(|s| {
                    Arc::make_mut(&mut s.info)
                        .entry(node.id.clone())
                        .or_insert_with(|| json!({}))["error"] = json!(error.to_string());
                });
                Err(error.into())
            }
        }
    }
    pub async fn rename(&self, node: &SavedNode, name: String) -> Result<()> {
        let name = zork_config::membership::validate_device_name(&name)?;
        let (binding, client) = self.connection(&node.id)?;
        let value = client
            .node_request(
                http::Method::PUT,
                "/v1/node/name".into(),
                Some(json!({"name":name})),
            )
            .await?;
        self.ensure_connection(&node.id, binding)?;
        crate::device_metadata::record(&self.store, &node.id, "/v1/node/name", &value)?;
        self.publish_nodes()
    }
    pub async fn update_release(self: &Arc<Self>, node: &SavedNode, install: bool) -> Result<()> {
        let directory = self.clone();
        let node = node.clone();
        self.connection(&node.id)?
            .1
            .spawn(async move { directory.update_release_inner(&node, install).await })
            .await?
    }
    async fn update_release_inner(&self, node: &SavedNode, install: bool) -> Result<()> {
        let (binding, client) = self.connection(&node.id)?;
        if !install {
            let value = client
                .node_request(http::Method::GET, "/v1/node/update".into(), None)
                .await?;
            self.ensure_connection(&node.id, binding)?;
            self.commit(|s| {
                let info = Arc::make_mut(&mut s.info)
                    .entry(node.id.clone())
                    .or_insert_with(|| json!({}));
                info["latest_version"] = value["latest_version"].clone();
                info["update"] = value["update"].clone();
            });
            return Ok(());
        }
        let version = self
            .snapshot()
            .info
            .get(&node.id)
            .and_then(|v| v["latest_version"].as_str())
            .map(str::to_owned)
            .context("请先检查可用版本")?;
        {
            let mut state = self.owned.lock().unwrap();
            anyhow::ensure!(state.updating.insert(node.id.clone()), "设备升级仍在进行");
            self.state.publish(state.clone());
        }
        let result: Result<()> = async {
            let device = Device::open(
                client.clone(),
                Some((self.store.clone(), node.id.clone())),
                true,
            );
            device
                .upgrade(&version, |state| {
                    self.ensure_connection(&node.id, binding)?;
                    if state.metadata_loaded {
                        self.commit(|s| {
                            Arc::make_mut(&mut s.info)
                                .insert(node.id.clone(), state.info.as_ref().clone());
                        });
                    }
                    Ok(())
                })
                .await
        }
        .await;
        self.commit(|s| {
            s.updating.remove(&node.id);
            if let Err(error) = &result {
                s.error = Some(error.to_string());
            }
        });
        result
    }
    async fn sync_members(
        &self,
        anchor_id: &str,
        own_origin: &str,
        group: zork_config::membership::MeshGroup,
    ) -> Result<()> {
        let _serial = self.sync_gate.lock().await;
        if self
            .revisions
            .lock()
            .unwrap()
            .get(&group.authority)
            .is_some_and(|r| *r >= group.revision)
        {
            return Ok(());
        }
        group.validate()?;
        if !group.contains(own_origin) {
            return Ok(());
        }
        let nodes = self.store.nodes()?;
        let anchor = nodes
            .iter()
            .find(|n| n.id == anchor_id)
            .context("设备已移除")?;
        let (binding, client) = self.connection(&anchor.id)?;
        let mut origins = HashMap::new();
        for node in &nodes {
            if let Some(origin) = node
                .mesh
                .as_ref()
                .map(|m| m.origin.clone())
                .or(self.store.get(&node.id, "mesh-origin")?)
            {
                origins.insert(origin, node.id.clone());
            }
        }
        origins.insert(own_origin.to_owned(), anchor_id.to_owned());
        let network = client
            .node_request(http::Method::GET, "/v1/node/mesh".into(), None)
            .await?;
        self.ensure_connection(anchor_id, binding)?;
        self.store.put(anchor_id, "mesh-origin", &own_origin)?;
        let network: zork_config::MeshConfig = serde_json::from_value(network["config"].clone())?;
        let mut nodes = self.store.nodes()?;
        let removed = nodes
            .iter()
            .filter(|n| {
                n.group.as_deref() == Some(&group.authority)
                    && n.mesh.as_ref().is_some_and(|m| !group.contains(&m.origin))
            })
            .map(|n| n.id.clone())
            .collect::<Vec<_>>();
        for id in &removed {
            self.store.remove_node(id)?;
        }
        nodes.retain(|n| !removed.contains(&n.id));
        for member in &group.members {
            if let Some(saved) = origins
                .get(&member.origin)
                .and_then(|id| nodes.iter_mut().find(|n| n.id == *id))
            {
                saved.name = member.name.clone();
                self.store.save_node(saved)?;
            }
        }
        for member in group.members.iter().filter(|m| m.origin != own_origin) {
            if origins
                .get(&member.origin)
                .is_some_and(|id| nodes.iter().any(|n| n.id == *id && n.mesh.is_none()))
            {
                continue;
            }
            let node = if let Some(saved) = nodes
                .iter_mut()
                .find(|n| n.mesh.as_ref().is_some_and(|m| m.origin == member.origin))
            {
                saved.name = member.name.clone();
                saved.mesh.as_mut().unwrap().addr = member.addr.clone();
                saved.mesh.as_mut().unwrap().routes = member.routes.clone();
                saved.clone()
            } else {
                let node = SavedNode {
                    id: member.origin.clone(),
                    name: member.name.clone(),
                    url: String::new(),
                    token: None,
                    local: false,
                    mesh: Some(RemoteNode {
                        routes: member.routes.clone(),
                        origin: member.origin.clone(),
                        addr: member.addr.clone(),
                    }),
                    group: Some(group.authority.clone()),
                };
                nodes.push(node.clone());
                node
            };
            self.store.save_node(&node)?;
        }
        let transport = self.transport.clone();
        let started = nodes.clone();
        let identity =
            tokio::task::spawn_blocking(move || transport.start_on(&started, Some(&network)))
                .await??;
        if !group.clients.iter().any(|c| c.origin == identity) {
            anyhow::ensure!(
                self.store
                    .get::<String>("device", &format!("mesh-client:{}", group.authority))?
                    .as_deref()
                    != Some(&identity),
                "此客户端已从 mesh 移除，请在已授权设备上重新允许访问"
            );
            client.node_request(http::Method::POST,"/v1/node/mesh/clients".into(),Some(json!({"origin":identity,"name":format!("{} 客户端",zork_config::device_name()),"addr":null}))).await?;
        }
        self.store.put(
            "device",
            &format!("mesh-client:{}", group.authority),
            &identity,
        )?;
        self.revisions
            .lock()
            .unwrap()
            .insert(group.authority, group.revision);
        self.commit(|s| {
            s.mesh_identity = Some(identity);
            s.error = None;
        });
        self.publish_nodes()?;
        // The same saved connection can become usable when its embedded
        // transport starts. Wake the file reader even without a node-list edit.

        Ok(())
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        for (_, task) in self.devices.get_mut().unwrap().values() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn replacing_connection_rejects_late_metadata_and_reuses_unchanged_binding() {
        use axum::{routing::get, Json, Router};
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let router = Router::new().route(
            "/v1/node/info",
            get({
                let (started, release) = (started.clone(), release.clone());
                move || {
                    let (started, release) = (started.clone(), release.clone());
                    async move {
                        started.notify_one();
                        release.notified().await;
                        Json(json!({"name":"obsolete name"}))
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let root = tempfile::tempdir().unwrap();
        let directory = Directory::open(root.path()).unwrap();
        let mut node = SavedNode {
            id: "fixture".into(),
            name: "current name".into(),
            url: format!("http://{}", listener.local_addr().unwrap()),
            token: Some("old".into()),
            local: false,
            mesh: None,
            group: None,
        };
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        directory.store.save_node(&node).unwrap();
        directory.publish_nodes().unwrap();
        let initial = directory.connection(&node.id).unwrap();
        assert!(Arc::ptr_eq(
            &initial.1,
            &directory.connection(&node.id).unwrap().1
        ));
        let request = tokio::spawn({
            let directory = directory.clone();
            let node = node.clone();
            async move { directory.refresh_info(&node).await }
        });
        tokio::time::timeout(Duration::from_secs(5), started.notified())
            .await
            .unwrap();
        node.token = Some("new".into());
        directory.store.save_node(&node).unwrap();
        directory.publish_nodes().unwrap();
        release.notify_one();
        assert!(request.await.unwrap().is_err());
        assert!(directory.snapshot().info.is_empty());
        assert_eq!(directory.node(&node.id).unwrap().name, "current name");
        let current = directory.connection(&node.id).unwrap();
        assert_ne!(initial.0, current.0);
        node.name = "renamed".into();
        directory.store.save_node(&node).unwrap();
        directory.publish_nodes().unwrap();
        assert_eq!(directory.connection(&node.id).unwrap().0, current.0);
        directory.store.remove_node(&node.id).unwrap();
        directory.publish_nodes().unwrap();
        assert!(directory.connection(&node.id).is_err());
        server.abort();
    }
}
