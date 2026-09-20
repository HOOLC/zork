//! The access client's live Mesh directory. Membership, connection ownership and
//! device discovery continue independently of which node a screen has selected.
use crate::{
    api::StationClient,
    state::{Device, Domains},
    store::{ClientStore, SavedNode},
};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use zork_mesh::node::MeshNode;

struct Connection {
    saved: SavedNode,
    client: Arc<StationClient>,
    device: Arc<Device>,
    watcher: zork_notify::Task<()>,
}
pub(crate) struct Directory {
    store: Arc<ClientStore>,
    pub(crate) source: Arc<zork_observe::ValueSource<Value>>,
    resources: Arc<crate::resources::Resources>,
    shared_files: Arc<crate::shared_files::SharedFiles>,
    chat_files: Arc<crate::chat_files::Controller>,
    adb: Arc<crate::adb::Controller>,
    node: Mutex<Option<MeshNode>>,
    connections: Mutex<HashMap<String, Connection>>,
    generation: AtomicU64,
    reconcile: tokio::sync::Mutex<()>,
    task: Mutex<Option<zork_notify::Task<()>>>,
}
impl Drop for Directory {
    fn drop(&mut self) {
        self.task.get_mut().unwrap().take();
        for (_, connection) in self.connections.get_mut().unwrap().drain() {
            connection.device.stop_sync();
        }
    }
}
impl Directory {
    pub(crate) fn new(
        store: Arc<ClientStore>,
        resources: Arc<crate::resources::Resources>,
        shared_files: Arc<crate::shared_files::SharedFiles>,
        chat_files: Arc<crate::chat_files::Controller>,
        adb: Arc<crate::adb::Controller>,
    ) -> Result<Arc<Self>> {
        let nodes: Vec<_> = store.nodes()?.iter().map(directory_node).collect();
        let selected = store
            .get::<Option<String>>("device", "last-node")?
            .flatten()
            .filter(|id| nodes.iter().any(|node| node["id"] == *id));
        let value = json!({"nodes":nodes,"running":false,"selected_peer":selected});
        Ok(Arc::new(Self {
            store,
            resources,
            shared_files,
            chat_files,
            adb,
            source: Arc::new(zork_observe::ValueSource::new(value)),
            node: Default::default(),
            connections: Default::default(),
            generation: AtomicU64::new(0),
            reconcile: Default::default(),
            task: Default::default(),
        }))
    }
    pub(crate) async fn start(self: &Arc<Self>, node: MeshNode) -> Result<()> {
        let running = self
            .node
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|active| active.same_runtime(&node));
        if !running {
            self.stop().await;
            *self.node.lock().unwrap() = Some(node);
            let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
            let weak = Arc::downgrade(self);
            let mut changes = self.store.directory_events();
            let task = tokio::spawn(async move {
                let mut retry = zork_notify::retry::Retry::default();
                loop {
                    changes.snapshot();
                    let result = match weak.upgrade() {
                        Some(directory)
                            if directory.generation.load(Ordering::Acquire) == generation =>
                        {
                            directory.refresh().await
                        }
                        _ => return,
                    };
                    if let Err(error) = result {
                        tracing::warn!(%error, "client Mesh directory will retry");
                        tokio::select! { _ = retry.wait() => {}, _ = changes.changed() => {} }
                    } else {
                        retry.reset();
                        if changes.changed().await.is_none() {
                            return;
                        }
                    }
                }
            });
            *self.task.lock().unwrap() = Some(zork_notify::Task(task));
        }
        self.refresh().await
    }
    pub(crate) async fn stop(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        let task = self.task.lock().unwrap().take();
        if let Some(mut task) = task {
            task.abort();
            let _ = (&mut task).await;
        }
        let _serial = self.reconcile.lock().await;
        self.node.lock().unwrap().take();
        let connections = std::mem::take(&mut *self.connections.lock().unwrap());
        for (_, mut connection) in connections {
            connection.watcher.abort();
            let _ = (&mut connection.watcher).await;
            connection.device.profiles().suspend_authorization();
            connection.device.stop_sync();
        }
        self.resources.replace_devices(Vec::new());
        self.shared_files.pause();
        let _ = self.publish(true);
    }
    pub(crate) fn devices(&self) -> Vec<Arc<Device>> {
        self.connections
            .lock()
            .unwrap()
            .values()
            .map(|c| c.device.clone())
            .collect()
    }
    pub(crate) fn device(&self, id: &str) -> Option<Arc<Device>> {
        self.connections
            .lock()
            .unwrap()
            .get(id)
            .map(|c| c.device.clone())
    }
    #[cfg(test)]
    pub(crate) fn insert_fixture(&self, saved: SavedNode, client: Arc<StationClient>) {
        let device = Device::open(
            client.clone(),
            Some((self.store.clone(), saved.id.clone())),
            true,
        );
        self.connections.lock().unwrap().insert(
            saved.id.clone(),
            Connection {
                saved,
                client,
                device,
                watcher: zork_notify::Task(tokio::spawn(std::future::pending())),
            },
        );
    }
    pub(crate) fn station(&self, id: &str) -> Result<Arc<StationClient>> {
        if let Some(client) = self
            .connections
            .lock()
            .unwrap()
            .get(id)
            .map(|c| c.client.clone())
        {
            return Ok(client);
        }
        let node = self
            .node
            .lock()
            .unwrap()
            .clone()
            .context("客户端连接已暂停")?;
        Ok(Arc::new(StationClient::mesh_on(
            node,
            id.into(),
            tokio::runtime::Handle::current(),
        )))
    }

    pub(crate) async fn refresh(self: &Arc<Self>) -> Result<()> {
        let _serial = self.reconcile.lock().await;
        let Some(node) = self.node.lock().unwrap().clone() else {
            return self.publish(false);
        };
        let generation = self.generation.load(Ordering::Acquire);
        let nodes = self.store.nodes()?;
        let mut changed = false;
        let removed: Vec<_> = self
            .connections
            .lock()
            .unwrap()
            .keys()
            .filter(|id| !nodes.iter().any(|node| &node.id == *id))
            .cloned()
            .collect();
        for id in removed {
            changed = true;
            let removed = self.connections.lock().unwrap().remove(&id);
            if let Some(mut connection) = removed {
                connection.watcher.abort();
                let _ = (&mut connection.watcher).await;
                connection.device.stop_sync();
                connection.device.revoke_replica_access()?;
                node.untrust(&id).await?;
                self.chat_files.revoke(&id);
                self.adb.peer_revoked(&id);
                self.resources.revoke(&id);
            }
        }
        for peer in &nodes {
            let previous = self
                .connections
                .lock()
                .unwrap()
                .get(&peer.id)
                .map(|entry| entry.saved.clone());
            if previous.as_ref() == Some(peer) {
                continue;
            }
            changed = true;
            if let Some(remote) = &peer.mesh {
                let routes_changed = previous
                    .as_ref()
                    .and_then(|saved| saved.mesh.as_ref())
                    .is_none_or(|old| old != remote);
                node.trust(
                    &remote.origin,
                    &peer.name,
                    if routes_changed {
                        remote.addr.as_deref()
                    } else {
                        None
                    },
                )
                .await?;
                if routes_changed {
                    if let Some(routes) = &remote.routes {
                        node.remember_routes(&remote.origin, routes).await?;
                    }
                }
            }
            let exists = self.connections.lock().unwrap().contains_key(&peer.id);
            if exists {
                self.connections
                    .lock()
                    .unwrap()
                    .get_mut(&peer.id)
                    .unwrap()
                    .saved = peer.clone();
                continue;
            }
            let client = Arc::new(StationClient::mesh_on(
                node.clone(),
                peer.id.clone(),
                tokio::runtime::Handle::current(),
            ));
            let device = Device::open(
                client.clone(),
                Some((self.store.clone(), peer.id.clone())),
                true,
            );
            device.start();
            device.profiles().restore_authorization()?;
            let mut changes = device.subscribe_domains(Domains::CONNECTION | Domains::MESH);
            let weak = Arc::downgrade(self);
            let id = peer.id.clone();
            let watched_client = client.clone();
            let watcher = zork_notify::Task(tokio::spawn(async move {
                loop {
                    let state = changes.snapshot().state;
                    {
                        let Some(directory) = weak.upgrade() else {
                            return;
                        };
                        if directory.generation.load(Ordering::Acquire) != generation {
                            return;
                        }
                        directory
                            .shared_files
                            .update_device(&id, &watched_client, &state);
                        if state.revoked {
                            directory.chat_files.revoke(&id);
                            directory.adb.peer_revoked(&id);
                            directory.resources.revoke(&id);
                        }
                        if let Some(group) = &state.mesh.group {
                            let identity = directory.store.get::<String>("device", "identity");
                            if let Ok(Some(identity)) = identity {
                                if let Err(error) =
                                    directory.store.apply_mesh_directory(&id, &identity, group)
                                {
                                    // A manual connection never grants access to a whole Mesh.
                                    if directory.store.nodes().ok().is_some_and(|nodes| {
                                        nodes.iter().any(|n| n.id == id && n.group.is_some())
                                    }) {
                                        tracing::debug!(%error, "client membership snapshot rejected");
                                    }
                                }
                            }
                        }
                    }
                    if changes.changed().await.is_none() {
                        return;
                    }
                }
            }));
            self.connections.lock().unwrap().insert(
                peer.id.clone(),
                Connection {
                    saved: peer.clone(),
                    client,
                    device,
                    watcher,
                },
            );
        }
        if changed {
            self.sync_resources();
        }
        self.publish(changed)
    }
    fn sync_resources(&self) {
        let clients: Vec<_> = self
            .connections
            .lock()
            .unwrap()
            .values()
            .filter(|entry| !entry.device.snapshot().revoked)
            .map(|entry| {
                (
                    entry.saved.id.clone(),
                    entry.saved.name.clone(),
                    entry.client.clone(),
                    entry.device.clone(),
                )
            })
            .collect();
        self.shared_files.replace_devices(
            clients
                .iter()
                .map(|(id, name, client, _)| (id.clone(), name.clone(), false, client.clone()))
                .collect(),
        );
        for (id, _, client, device) in &clients {
            self.shared_files
                .update_device(id, client, &device.snapshot());
        }
        self.resources.replace_devices(
            clients
                .into_iter()
                .map(|(id, name, client, _)| (id, name, client))
                .collect(),
        );
        self.adb.devices_changed();
    }
    fn publish(&self, changed: bool) -> Result<()> {
        let nodes = self.store.nodes()?;
        let running = self
            .node
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|node| node.address().is_ok());
        if running
            && nodes
                .iter()
                .any(|node| !self.connections.lock().unwrap().contains_key(&node.id))
        {
            return Ok(()); // Reconciliation publishes after creating the controllers.
        }
        let selected = self
            .store
            .get::<Option<String>>("device", "last-node")?
            .flatten()
            .filter(|id| nodes.iter().any(|n| &n.id == id));
        let previous = self.source.read();
        if !changed
            && previous["running"] == running
            && previous["selected_peer"] == json!(selected)
        {
            return Ok(());
        }
        let nodes: Vec<_> = nodes.iter().map(directory_node).collect();
        let removed = previous["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|old| !nodes.iter().any(|node| node["id"] == old["id"]));
        let value = json!({"nodes":nodes,"selected_peer":selected,"running":running});
        if removed || (!running && previous["running"] == true) {
            self.source.invalidate(value);
        } else {
            self.source
                .publish_changed(value, zork_observe::Topics::ALL);
        }
        Ok(())
    }
}

fn directory_node(node: &SavedNode) -> Value {
    json!({"id":node.id,"name":node.name,"mesh":node.mesh,"group":node.group})
}
