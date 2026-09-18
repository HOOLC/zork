//! Device directory, membership and host operations. Views only observe `DirectoryData`.
use super::{
    account::{AccountFlow, AccountIdentity},
    node::LocalNode,
    transport::ClientMesh,
};
use crate::{
    api::GatewayClient,
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
    pub account: Option<AccountIdentity>,
    pub account_available: bool,
    pub account_busy: bool,
    pub account_url: Option<String>,
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
    sync_gate: tokio::sync::Mutex<()>,
    devices: Mutex<HashMap<String, (Arc<Device>, tokio::task::JoinHandle<()>)>>,
    connections: Mutex<HashMap<String, Connection>>,
    next_binding: std::sync::atomic::AtomicU64,
    host_watch: Mutex<Option<zork_notify::files::FileWatch>>,
    host_observer: Mutex<Option<(u64, HostConnection)>>,
    host_generation: std::sync::atomic::AtomicU64,
    account_attempt: Mutex<Option<Arc<zork_notify::io::Cancellation>>>,
    application_sources: Mutex<HashMap<String, crate::pages::ApplicationSource>>,
    resources: std::sync::OnceLock<Arc<crate::resources::Resources>>,
    shared_files: std::sync::OnceLock<Arc<crate::shared_files::SharedFiles>>,
}
struct HostConnection(std::os::unix::net::UnixStream);
struct Connection {
    key: (String, Option<String>, Option<String>),
    binding: u64,
    client: Arc<GatewayClient>,
}
impl Drop for HostConnection {
    fn drop(&mut self) {
        let _ = self.0.shutdown(std::net::Shutdown::Both);
    }
}
impl Directory {
    pub fn open(root: &Path) -> Result<Arc<Self>> {
        let data_lease = super::data_reset::acquire(root)?;
        let store = Arc::new(ClientStore::open(root)?);
        let (local_enabled, error) = match store.local_node_enabled() {
            Ok(value) => (value, None),
            Err(e) => (false, Some(e.to_string())),
        };
        let account: Option<AccountIdentity> = store.get("account", "identity")?;
        let account = account.filter(|a| {
            super::load_services()
                .ok()
                .and_then(|s| s.cue)
                .is_some_and(|s| s.issuer == a.issuer && s.client_id == a.client_id)
        });
        let state = DirectoryData {
            nodes: Arc::new(store.nodes()?),
            local_enabled,
            account,
            account_available: super::load_services().ok().and_then(|s| s.cue).is_some(),
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
            sync_gate: tokio::sync::Mutex::new(()),
            devices: Mutex::new(HashMap::new()),
            host_watch: Default::default(),
            host_observer: Default::default(),
            host_generation: Default::default(),
            account_attempt: Default::default(),
            connections: Mutex::new(HashMap::new()),
            next_binding: std::sync::atomic::AtomicU64::new(1),
            application_sources: Default::default(),
            resources: Default::default(),
            shared_files: Default::default(),
        });
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
                source.cancel_account();
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
    fn publish_nodes(&self) -> Result<()> {
        let nodes = self.store.nodes()?;
        let changed = self.snapshot().nodes.as_ref() != &nodes;
        self.commit(|s| s.nodes = Arc::new(nodes));
        if changed {
            self.publish_applications();
            if let Some(resources) = self.resources.get() {
                resources.replace_devices(self.resource_clients());
            }
            if let Some(source) = self.shared_files.get() {
                self.refresh_shared_files(source);
            }
        }
        Ok(())
    }
    fn resource_clients(&self) -> Vec<(String, String, Arc<GatewayClient>)> {
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
    fn shared_file_clients(&self) -> Vec<(String,String,bool,Arc<GatewayClient>)> {
        self.snapshot().nodes.iter().filter(|node| !self.store.replica_revoked(&node.id).unwrap_or(true)).filter_map(|node| {
            self.connection(&node.id).ok().map(|(_,client)|(node.id.clone(),node.name.clone(),node.local,client))
        }).collect()
    }
    pub fn shared_files(&self) -> Arc<crate::shared_files::SharedFiles> {
        self.shared_files.get_or_init(|| {
            let source=crate::shared_files::SharedFiles::new(self.store.clone());
            self.refresh_shared_files(&source);
            source
        }).clone()
    }
    fn refresh_shared_files(&self, source: &Arc<crate::shared_files::SharedFiles>) {
        source.replace_devices(self.shared_file_clients());
        let states = self.devices.lock().unwrap().iter()
            .map(|(id, (device, _))| (id.clone(), device.snapshot())).collect::<Vec<_>>();
        for (id, state) in states {
            if let Ok((_, client)) = self.connection(&id) {
                source.update_device(&id, &client, &state);
            }
        }
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
            .get_or_init(|| crate::resources::Resources::new(self.resource_clients()))
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
                    && s.origin == origin
            }) {
                return;
            }
            sources.insert(
                id.into(),
                crate::pages::ApplicationSource {
                    applications: Arc::new(data.pages.applications.clone()),
                    online: data.online,
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
    pub fn node(&self, id: &str) -> Option<SavedNode> {
        self.snapshot()
            .nodes
            .iter()
            .find(|node| node.id == id)
            .cloned()
    }
    pub fn connection(&self, id: &str) -> Result<(u64, Arc<GatewayClient>)> {
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
            Some(remote) => GatewayClient::new_mesh(self.transport.control(), remote.origin),
            None => GatewayClient::new(node.url.clone(), node.token.clone())
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
    pub fn bind(self: &Arc<Self>, id: String, device: Arc<Device>, client: Arc<GatewayClient>) {
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
        let mut updates = device.subscribe();
        let weak = Arc::downgrade(self);
        let anchor = id.clone();
        let observation_client = client.clone();
        let task = client.spawn(async move {
            let mut retry = zork_notify::retry::Retry::default();
            let mut previous_online = None;
            loop {
                let mut failed = false;
                let update = updates.snapshot();
                if let Some(directory) = weak.upgrade() {
                    if binding.is_some_and(|binding|directory.ensure_connection(&anchor,binding).is_err()){return;}
                    directory.accept_applications(&anchor,&update.state);
                    if let Some(source) = directory.shared_files.get() {
                        source.update_device(&anchor, &observation_client, &update.state);
                    }
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
    pub fn login_account(self: &Arc<Self>) -> Result<()> {
        let mut attempt = self.account_attempt.lock().unwrap();
        anyhow::ensure!(attempt.is_none(), "登录仍在进行");
        let cancel = Arc::new(zork_notify::io::Cancellation::new()?);
        *attempt = Some(cancel.clone());
        self.commit(|s| {
            s.account_busy = true;
            s.account_url = None;
            s.error = None;
        });
        let source = self.clone();
        std::thread::spawn(move || {
            let result = (|| {
                let config = super::load_services()?
                    .cue
                    .context("尚未配置 Cue 账号服务地址和 client_id")?;
                let flow = AccountFlow::prepare(config, cancel.clone())?;
                {
                    let attempt = source.account_attempt.lock().unwrap();
                    anyhow::ensure!(
                        attempt.as_ref().is_some_and(|a| Arc::ptr_eq(a, &cancel)),
                        "登录已取消"
                    );
                    source.commit(|s| s.account_url = Some(flow.url.clone()));
                }
                flow.finish()
            })();
            source.complete_account(&cancel, result);
        });
        Ok(())
    }
    fn complete_account(
        &self,
        cancel: &Arc<zork_notify::io::Cancellation>,
        result: Result<AccountIdentity>,
    ) {
        let mut attempt = self.account_attempt.lock().unwrap();
        if !attempt.as_ref().is_some_and(|a| Arc::ptr_eq(a, cancel)) {
            return;
        }
        attempt.take();
        let result = result.and_then(|identity| {
            self.store.put("account", "identity", &identity)?;
            Ok(identity)
        });
        self.commit(|s| {
            s.account_busy = false;
            s.account_url = None;
            match result {
                Ok(identity) => s.account = Some(identity),
                Err(e) => s.error = Some(e.to_string()),
            }
        });
    }
    pub fn cancel_account(&self) {
        let mut attempt = self.account_attempt.lock().unwrap();
        if let Some(cancel) = attempt.take() {
            cancel.cancel();
        }
        self.commit(|s| {
            s.account_busy = false;
            s.account_url = None;
        });
    }
    pub fn logout(&self) -> Result<()> {
        self.cancel_account();
        self.store
            .put("account", "identity", &Option::<AccountIdentity>::None)?;
        self.commit(|s| s.account = None);
        Ok(())
    }
    pub fn select(&self, id: &str) -> Result<()> {
        self.store.put("device", "last-node", &id)
    }
    pub fn selected(&self) -> Option<String> {
        self.store.get("device", "last-node").ok().flatten()
    }
    pub fn save_message_preview_height(&self, height: u32) -> Result<()> {
        let preferences = crate::preferences::save_message_preview_height(&self.store, height)?;
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
                saved.clone()
            } else {
                let node = SavedNode {
                    id: member.origin.clone(),
                    name: member.name.clone(),
                    url: String::new(),
                    token: None,
                    local: false,
                    mesh: Some(RemoteNode {
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
        if let Some(source) = self.shared_files.get() {
            self.refresh_shared_files(source);
        }
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

    #[test]
    fn cancelled_account_result_cannot_persist_identity_or_end_the_next_attempt() {
        let root = tempfile::tempdir().unwrap();
        let directory = Directory::open(root.path()).unwrap();
        let old = Arc::new(zork_notify::io::Cancellation::new().unwrap());
        *directory.account_attempt.lock().unwrap() = Some(old.clone());
        directory.cancel_account();
        assert!(old.is_cancelled());
        let next = Arc::new(zork_notify::io::Cancellation::new().unwrap());
        *directory.account_attempt.lock().unwrap() = Some(next.clone());
        directory.commit(|s| s.account_busy = true);
        directory.complete_account(
            &old,
            Ok(AccountIdentity {
                issuer: "https://issuer.example.test".into(),
                client_id: "fixture".into(),
                subject: "old-user".into(),
                name: "Old User".into(),
                email: None,
                expires_at: 1,
            }),
        );
        assert!(directory.snapshot().account.is_none());
        assert!(directory.snapshot().account_busy);
        assert!(Arc::ptr_eq(
            directory.account_attempt.lock().unwrap().as_ref().unwrap(),
            &next
        ));
        assert!(directory
            .store
            .get::<AccountIdentity>("account", "identity")
            .unwrap()
            .is_none());
    }

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
