//! One projection of the client's Synch tree. Published metadata is already
//! merged by Synch; this controller never builds a second per-Station catalog.
mod content;
mod model;
mod projection;
#[cfg(test)]
mod tests;
use crate::{
    api::StationClient,
    state::{Observable, Subscription},
    store::ClientStore,
};
use anyhow::{ensure, Context, Result};
pub use model::*;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use zork_mesh::node::{MeshNode, TreeCatalog, TreePage, TreeQuery};

const MAX_DIRECTORY_ENTRIES: usize = 8192;
struct Binding {
    id: String,
    name: String,
    generation: u64,
    client: Arc<StationClient>,
    origin: Option<String>,
    online: Option<bool>,
}
struct Owned {
    data: SharedFilesData,
    bindings: Vec<Binding>,
    names: HashMap<String, String>,
    node: Option<MeshNode>,
    ready: bool,
    feed: Option<zork_notify::Task<()>>,
    catalog: TreeCatalog,
    page: Option<TreePage>,
    window: usize,
    loading_more: bool,
    epoch: u64,
    next_binding: u64,
    preview_epoch: u64,
    export: Option<content::Export>,
    cache: HashMap<String, Arc<Vec<u8>>>,
    suspended: bool,
}
pub struct SharedFiles {
    store: Arc<ClientStore>,
    owned: Mutex<Owned>,
    changes: Observable<SharedFilesData>,
    preview_cancel: zork_notify::Notifier,
}
impl SharedFiles {
    pub fn new(store: Arc<ClientStore>) -> Arc<Self> {
        let data = SharedFilesData {
            location: Some(Location {
                space: zork_config::tree::SHARED_FILES_SPACE.into(),
                path: String::new(),
            }),
            ..Default::default()
        };
        Arc::new(Self {
            store,
            owned: Mutex::new(Owned {
                data: data.clone(),
                bindings: vec![],
                names: HashMap::new(),
                node: None,
                ready: false,
                feed: None,
                catalog: TreeCatalog::default(),
                page: None,
                window: 128,
                loading_more: false,
                epoch: 0,
                next_binding: 0,
                preview_epoch: 0,
                export: None,
                cache: HashMap::new(),
                suspended: false,
            }),
            changes: Observable::new(data),
            preview_cancel: Default::default(),
        })
    }
    #[cfg(feature = "headless-bench")]
    pub fn rendering_fixture(store: Arc<ClientStore>, data: SharedFilesData) -> Arc<Self> {
        let source = Self::new(store);
        source.owned.lock().unwrap().data = data.clone();
        source.changes.publish(data);
        source
    }
    pub fn snapshot(&self) -> Arc<SharedFilesData> {
        self.changes.read()
    }
    pub fn subscribe(&self) -> Subscription<SharedFilesData> {
        self.changes.subscribe()
    }
    pub(crate) fn access_peers(&self) -> Vec<String> {
        self.owned
            .lock()
            .unwrap()
            .bindings
            .iter()
            .map(|b| b.id.clone())
            .collect()
    }
    fn cancel_preview(&self, s: &mut Owned) {
        s.preview_epoch = s.preview_epoch.wrapping_add(1);
        self.preview_cancel.notify();
    }
    fn publish(&self, s: &mut Owned) {
        self.rebuild(s);
        self.changes.publish(s.data.clone());
    }
    pub fn replace_devices(
        self: &Arc<Self>,
        devices: Vec<(String, String, bool, Arc<StationClient>)>,
    ) {
        let mut s = self.owned.lock().unwrap();
        let old = std::mem::take(&mut s.bindings);
        let mut names = HashMap::new();
        let mut node = None;
        for (id, name, _, client) in devices {
            if self.store.replica_revoked(&id).unwrap_or(true) {
                continue;
            }
            let same = old
                .iter()
                .find(|b| b.id == id && b.client.same_connection(&client));
            let generation = match same {
                Some(b) => b.generation,
                None => {
                    s.next_binding += 1;
                    s.next_binding
                }
            };
            let origin = client
                .authenticated_mesh_origin()
                .map(str::to_owned)
                .or_else(|| same.and_then(|b| b.origin.clone()))
                .or_else(|| self.store.get::<String>(&id, "mesh-origin").ok().flatten());
            if let Some(origin) = &origin {
                names.insert(origin.clone(), name.clone());
            }
            if node.is_none() {
                node = client.file_tree();
            }
            s.bindings.push(Binding {
                id,
                name,
                generation,
                client,
                origin,
                online: same.and_then(|b| b.online),
            });
        }
        let changed = old.len() != s.bindings.len()
            || old.iter().any(|b| {
                !s.bindings
                    .iter()
                    .any(|n| n.id == b.id && n.generation == b.generation)
            });
        let ready = node.as_ref().is_some_and(MeshNode::is_running);
        let restart = changed
            || s.ready != ready
            || s.suspended
            || s.node.as_ref().map(MeshNode::data_dir) != node.as_ref().map(MeshNode::data_dir);
        s.names = names;
        s.node = node;
        s.ready = ready;
        s.suspended = false;
        if changed {
            self.cancel_preview(&mut s);
            s.data.preview = None;
            s.export = None;
            s.data.save = SaveState::default();
            s.cache.clear();
        }
        self.publish(&mut s);
        drop(s);
        if restart {
            self.restart();
        }
    }
    /// Consume the existing Device controller's connection state. This does
    /// not create a new connection or treat local Synch readiness as a peer ping.
    pub(crate) fn update_device(
        self: &Arc<Self>,
        id: &str,
        client: &StationClient,
        data: &crate::state::DeviceData,
    ) {
        if data.revoked {
            self.revoke_connection(id, Some(client));
            return;
        }
        let mut s = self.owned.lock().unwrap();
        let Some(binding) = s
            .bindings
            .iter_mut()
            .find(|b| b.id == id && b.client.same_connection(client))
        else {
            return;
        };
        let origin = data.mesh.origin.clone().or_else(|| binding.origin.clone());
        if binding.origin == origin && binding.online == data.online {
            return;
        }
        let name = binding.name.clone();
        binding.origin = origin.clone();
        binding.online = data.online;
        if let Some(origin) = origin {
            s.names.insert(origin, name);
        }
        self.publish(&mut s);
    }
    pub fn revoke(self: &Arc<Self>, peer: &str) {
        self.revoke_connection(peer, None);
    }
    fn revoke_connection(self: &Arc<Self>, peer: &str, client: Option<&StationClient>) {
        let mut s = self.owned.lock().unwrap();
        if !s
            .bindings
            .iter()
            .any(|b| b.id == peer && client.is_none_or(|client| b.client.same_connection(client)))
        {
            return;
        }
        s.bindings.retain(|b| b.id != peer);
        s.data.preview = None;
        s.export = None;
        s.data.save = SaveState::default();
        s.cache.clear();
        self.cancel_preview(&mut s);
        if s.bindings.is_empty() {
            s.catalog = TreeCatalog::default();
            s.page = None;
            s.node = None;
        }
        self.rebuild(&mut s);
        self.changes.invalidate(s.data.clone());
        drop(s);
        self.restart();
    }
    pub fn pause(&self) {
        let mut s = self.owned.lock().unwrap();
        s.suspended = true;
        s.epoch += 1;
        s.feed = None;
        s.loading_more = false;
        s.data.loading = false;
        s.data.offline = true;
        self.cancel_preview(&mut s);
        if s.data.preview.as_ref().is_some_and(|p| p.loading) {
            s.data.preview = None;
        }
        self.publish(&mut s);
    }
    fn query(s: &Owned) -> Option<TreeQuery> {
        s.data.location.as_ref().map(|l| TreeQuery {
            space: l.space.clone(),
            path: l.path.clone(),
            origin: s.data.source.clone(),
            search: s.data.search.clone(),
            descending: s.data.sort == Sort::NameDescending,
            after: None,
            limit: 128,
        })
    }
    fn restart(self: &Arc<Self>) {
        let mut s = self.owned.lock().unwrap();
        s.feed = None;
        s.epoch += 1;
        s.loading_more = false;
        let epoch = s.epoch;
        if !s.data.active || s.suspended {
            s.data.loading = false;
            self.publish(&mut s);
            return;
        }
        let Some(node) = s.node.clone().filter(MeshNode::is_running) else {
            s.data.loading = false;
            s.data.offline = true;
            s.data.error = Some("文件网络尚未就绪".into());
            self.publish(&mut s);
            return;
        };
        let Some(client) = s.bindings.first().map(|b| b.client.clone()) else {
            s.data.loading = false;
            self.publish(&mut s);
            return;
        };
        s.data.loading = true;
        s.data.error = None;
        s.data.offline = false;
        let query = Self::query(&s);
        let weak = Arc::downgrade(self);
        s.feed = Some(zork_notify::Task(client.spawn(async move {
            let source = match node.source_changes().await {
                Ok(source) => source,
                Err(error) => {
                    if let Some(this) = weak.upgrade() {
                        let window = this.owned.lock().unwrap().window;
                        this.accept(epoch, window, Err(error));
                    }
                    return;
                }
            };
            let mut changes = source.subscribe();
            loop {
                changes.checkpoint();
                let Some(this) = weak.upgrade() else {
                    return;
                };
                let window = this.owned.lock().unwrap().window;
                drop(this);
                let result = async {
                    let catalog = node
                        .tree_space(zork_config::tree::SHARED_FILES_SPACE)
                        .await?;
                    let page = match &query {
                        Some(q) => node
                            .tree_directory_window(q.clone(), window)
                            .await
                            .map(Some),
                        None => Ok(None),
                    };
                    Ok((catalog, page))
                }
                .await;
                let Some(this) = weak.upgrade() else {
                    return;
                };
                this.accept(epoch, window, result);
                drop(this);
                if changes.changed().await.is_err() {
                    return;
                }
            }
        })));
        self.publish(&mut s);
    }
    fn accept(
        self: &Arc<Self>,
        epoch: u64,
        window: usize,
        result: Result<(TreeCatalog, Result<Option<TreePage>>)>,
    ) {
        let mut s = self.owned.lock().unwrap();
        if s.epoch != epoch {
            return;
        }
        s.data.loading = s.loading_more;
        let mut withdrawn = false;
        let mut restart = false;
        match result {
            Ok((catalog, page)) => {
                let visible = catalog
                    .spaces
                    .iter()
                    .flat_map(|space| space.origins.iter())
                    .collect::<std::collections::BTreeSet<_>>();
                withdrawn = s
                    .catalog
                    .spaces
                    .iter()
                    .flat_map(|space| space.origins.iter())
                    .any(|origin| !visible.contains(origin));
                if withdrawn {
                    s.epoch = s.epoch.wrapping_add(1);
                    restart = true;
                    self.cancel_preview(&mut s);
                    s.data.preview = None;
                    s.export = None;
                    s.data.save = SaveState::default();
                    s.cache.clear();
                }
                if s.data
                    .source
                    .as_ref()
                    .is_some_and(|origin| !visible.contains(origin))
                {
                    s.data.source = None;
                    restart = true;
                }
                let page = match page {
                    Ok(page) => {
                        s.data.error = None;
                        page
                    }
                    Err(error) => {
                        s.data.error = Some(error.to_string());
                        if withdrawn {
                            None
                        } else {
                            s.page.clone()
                        }
                    }
                };
                let unchanged = s.catalog == catalog
                    && s.page
                        .as_ref()
                        .is_some_and(|p| page.as_ref().is_some_and(|n| p.revision == n.revision));
                s.catalog = catalog;
                if !unchanged && (window == s.window || withdrawn) {
                    s.page = page;
                }
                s.data.offline = false;
            }
            Err(error) => s.data.error = Some(error.to_string()),
        }
        if withdrawn {
            self.rebuild(&mut s);
            self.changes.invalidate(s.data.clone());
        } else {
            self.publish(&mut s);
        }
        drop(s);
        if restart {
            self.restart();
        }
    }
    async fn more(self: &Arc<Self>) -> Result<()> {
        let (node, query, epoch, window) = {
            let mut s = self.owned.lock().unwrap();
            if s.data.loading || s.loading_more || !s.data.more {
                return Ok(());
            }
            let query = Self::query(&s).context("请先打开空间")?;
            let node = s.node.clone().context("文件网络尚未就绪")?;
            s.data.loading = true;
            s.loading_more = true;
            s.window = (s.window + 128).min(MAX_DIRECTORY_ENTRIES);
            self.publish(&mut s);
            (node, query, s.epoch, s.window)
        };
        let result = node.tree_directory_window(query, window).await;
        let mut s = self.owned.lock().unwrap();
        if s.epoch != epoch {
            return Ok(());
        }
        s.data.loading = false;
        s.loading_more = false;
        match result {
            Ok(next) => {
                s.page = Some(next);
            }
            Err(error) => {
                s.data.error = Some(error.to_string());
                drop(s);
                self.restart();
                return Ok(());
            }
        }
        self.publish(&mut s);
        Ok(())
    }
    pub async fn dispatch(self: &Arc<Self>, action: Action) -> Result<()> {
        let result = self.apply(action).await;
        if let Err(error) = &result {
            let mut s = self.owned.lock().unwrap();
            s.data.error = Some(error.to_string());
            self.publish(&mut s);
        }
        result
    }
    async fn apply(self: &Arc<Self>, action: Action) -> Result<()> {
        match action {
            Action::More => return self.more().await,
            Action::PrepareSave => return self.prepare_save().await,
            Action::SelectVersion { root } => return self.select_version(root).await,
            Action::CancelSave { ticket } => {
                self.cancel_save(&ticket, None);
                return Ok(());
            }
            Action::SaveFailed { ticket, error } => {
                self.cancel_save(&ticket, Some(error));
                return Ok(());
            }
            _ => {}
        }
        let mut open = None;
        let mut restart = false;
        {
            let mut s = self.owned.lock().unwrap();
            s.data.error = None;
            match action {
                Action::Activate { active } => {
                    s.data.active = active;
                    restart = true;
                }
                Action::Refresh => restart = true,
                Action::OpenSpace { space } => {
                    ensure!(
                        space == zork_config::tree::SHARED_FILES_SPACE,
                        "共享空间不可用"
                    );
                    ensure!(
                        s.data.spaces.iter().any(|v| v.id == space),
                        "共享空间不可用"
                    );
                    s.data.location = Some(Location {
                        space,
                        path: String::new(),
                    });
                    s.data.search.clear();
                    restart = true;
                }
                Action::OpenEntry { id } => {
                    let entry = s
                        .data
                        .entries
                        .iter()
                        .find(|e| e.id == id)
                        .cloned()
                        .context("文件已不在当前目录")?;
                    if entry.kind == EntryKind::Directory {
                        s.data.location.as_mut().context("请先打开空间")?.path = entry.path;
                        s.data.search.clear();
                        restart = true;
                    } else {
                        open = Some(entry);
                    }
                }
                Action::Back => {
                    if s.data.preview.take().is_some() {
                        self.cancel_preview(&mut s);
                    } else if let Some(location) = &mut s.data.location {
                        if location.path.is_empty() {
                            s.data.active = false;
                        } else {
                            location.path =
                                location.path.rsplit_once('/').map_or("", |(p, _)| p).into();
                        }
                        s.data.search.clear();
                        restart = true;
                    } else {
                        s.data.active = false;
                        restart = true;
                    }
                }
                Action::ClosePreview => {
                    s.data.preview = None;
                    self.cancel_preview(&mut s);
                }
                Action::Source { peer } => {
                    ensure!(
                        peer.as_ref()
                            .is_none_or(|id| s.data.devices.iter().any(|d| &d.id == id)),
                        "来源不可用"
                    );
                    s.data.source = peer;
                    restart = true;
                }
                Action::Search { query } => {
                    ensure!(query.len() <= 255, "搜索内容过长");
                    s.data.search = query;
                    restart = true;
                }
                Action::Layout { layout } => s.data.layout = layout,
                Action::Sort { sort } => {
                    s.data.sort = sort;
                    restart = true;
                }
                _ => {}
            }
            if restart {
                s.page = None;
                s.window = 128;
                s.loading_more = false;
                s.epoch = s.epoch.wrapping_add(1);
                s.data.preview = None;
                self.cancel_preview(&mut s);
            }
            self.publish(&mut s);
        }
        if restart {
            self.restart();
        }
        if let Some(entry) = open {
            self.open_preview(entry).await?;
        }
        Ok(())
    }
}
