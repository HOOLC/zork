use crate::{
    db::{
        mesh::{Assignment, EventBody, Link, MeshEvent},
        EnsureSession, SessionBindingRow,
    },
    im_entry::{LOCAL_GUI_ENTRY_ID, LOCAL_GUI_PLATFORM},
    state::AppState,
};
use anyhow::{ensure, Context, Result};
use axum::{
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{path::Path, sync::Arc, time::Duration};
use zork_mesh::{
    control::{Peer, Reply},
    managed,
    node::MeshNode,
};

pub struct MeshService {
    pub adb: Arc<crate::adb::Registry>,
    pub services: Arc<crate::shared_services::Registry>,
    node: MeshNode,
    runtime: tokio::sync::Mutex<Option<zork_client_core::transport::Runtime>>,
    tasks: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
    origin: String,
    root: std::path::PathBuf,
    applied: std::sync::RwLock<zork_config::MeshConfig>,
    refresh_lock: tokio::sync::Mutex<()>,
    pub enrollment: Arc<crate::enrollment::EnrollmentService>,
    peer_cache: std::path::PathBuf,
    peers: std::sync::Mutex<std::collections::BTreeMap<String, PeerStatus>>,
    owner_subscriptions:
        tokio::sync::Mutex<std::collections::HashMap<String, zork_notify::Task<()>>>,
}
#[derive(Clone, Serialize)]
struct PeerStatus {
    origin: String,
    name: String,
    online: bool,
    last_seen_at: Option<String>,
    execute_workspaces: Vec<String>,
}
pub struct Prepared {
    pub service: Arc<MeshService>,
    state: Arc<std::sync::OnceLock<AppState>>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    v: u8,
    request: RpcRequest,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum RpcRequest {
    Watch {
        topic: WatchTopic,
    },
    NodeTool {
        body: crate::node_tools::Rpc,
    },
    ChannelTool {
        body: crate::channels::Rpc,
    },
    InteractionRegistration {
        body: crate::interaction_registry::Remote,
    },
    BusinessCard {
        body: crate::business_cards::Remote,
    },
    ChannelFile {
        body: crate::channels::FileRequest,
    },
    ClientBrowser {
        session_id: String,
        client_id: String,
        command: zork_browser::Command,
    },
    Hello,
    ConfirmJoin {
        id: String,
        secret: String,
        challenge: String,
        #[serde(default)]
        address: Option<Value>,
    },
    Membership {
        action: String,
        body: Value,
    },
    Subscribe {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<Value>,
    },

    Client {
        method: String,
        path: String,
        body: Option<Value>,
    },
    ClientFile {
        artifact_id: String,
        offset: usize,
    },
    Workers {
        leader_id: String,
    },
    InputFile {
        assignment_id: String,
        file_id: String,
    },
    ExecutionHistory {
        assignment_id: String,
        query: zork_agent_api::HistoryQuery,
    },
    Rework {
        assignment_id: String,
        request_id: String,
        goal: String,
    },
    Delegate {
        assignment: Assignment,
    },
    Cancel {
        assignment_id: String,
    },
    Decision {
        assignment_id: String,
        decision: Value,
    },
}

type WatchTopic = zork_mesh::feed::Watch;

fn station_peers(config: &zork_config::MeshConfig) -> impl Iterator<Item = &zork_config::MeshPeer> {
    // Access clients share trust for reads, but do not host a Station control
    // service. Legacy permission flags do not describe that endpoint role.
    config.peers.iter().filter(|peer| {
        !config.group.as_ref().is_some_and(|group| {
            group
                .clients
                .iter()
                .any(|client| client.origin == peer.origin)
        })
    })
}

impl MeshService {
    pub async fn execution_history(
        &self,
        link: &Link,
        query: &zork_agent_api::HistoryQuery,
    ) -> Result<Value> {
        ensure!(link.role == "owner", "mesh_history_requires_owner");
        let response = self
            .call(
                &link.assignment.executor_origin,
                RpcRequest::ExecutionHistory {
                    assignment_id: link.assignment.assignment_id.clone(),
                    query: query.clone(),
                },
            )
            .await?;
        if response["object"].is_object() {
            let object: zork_mesh::node::ObjectRef =
                serde_json::from_value(response["object"].clone())?;
            ensure!(
                object.origin == link.assignment.executor_origin && object.space == "zork-client",
                "history_wrong_source"
            );
            Ok(serde_json::from_slice(&self.node.read(&object).await?)?)
        } else {
            ensure!(response["page"].is_object(), "invalid_history_response");
            Ok(response["page"].clone())
        }
    }

    pub async fn channel_call(&self, origin: &str, body: crate::channels::Rpc) -> Result<Value> {
        if zork_agent_station_tools::channels::participating(&body.tool, &body.arguments) {
            self.peer(origin)?;
            let reply = self
                .node
                .subscribe(
                    origin,
                    &json!({"v":1,"request":{"kind":"channel_tool","body":body}}),
                )
                .await?
                .next()
                .await?
                .context("mesh_channel_transport_closed")?;
            ensure!(
                reply["v"] == 1 && reply["ok"] == true,
                "{}",
                reply["error"].as_str().unwrap_or("mesh_channel_failed")
            );
            return Ok(reply["data"].clone());
        }
        self.call(origin, RpcRequest::ChannelTool { body }).await
    }
    pub(crate) async fn business_card_call(
        &self,
        origin: &str,
        body: crate::business_cards::Remote,
    ) -> Result<Value> {
        self.participation_call(origin, RpcRequest::BusinessCard { body })
            .await
    }
    pub(crate) async fn registration_call(
        &self,
        origin: &str,
        body: crate::interaction_registry::Remote,
    ) -> Result<Value> {
        self.participation_call(origin, RpcRequest::InteractionRegistration { body })
            .await
    }
    async fn participation_call(&self, origin: &str, request: RpcRequest) -> Result<Value> {
        self.peer(origin)?;
        let payload = json!({"v":1,"request":request});
        // Connection setup is bounded; waiting for user participation is not.
        let reply = self.node.exchange(origin, &payload).await?;
        ensure!(
            reply["error"] != "invalid_mesh_request",
            "interaction_unsupported"
        );
        ensure!(reply["v"] == 1, "mesh_protocol_version");
        ensure!(
            reply["ok"] == true,
            "{}",
            reply["error"].as_str().unwrap_or("mesh_interaction_failed")
        );
        Ok(reply["data"].clone())
    }
    pub async fn channel_file(
        &self,
        origin: &str,
        body: crate::channels::FileRequest,
    ) -> Result<Value> {
        self.call(origin, RpcRequest::ChannelFile { body }).await
    }
    pub fn follow_channel_messages(
        &self,
        origin: &str,
        epoch: Option<String>,
        after: i64,
    ) -> zork_mesh::feed::Feed {
        self.node
            .follow(origin.into(), WatchTopic::AgentMessages { epoch, after })
    }
    fn config(&self) -> Result<zork_config::MeshConfig> {
        Ok(zork_config::load_config(&self.root)?.mesh)
    }

    pub async fn refresh(&self, state: &AppState) -> Result<()> {
        let _guard = self.refresh_lock.lock().await;
        let config = self.config()?;
        managed::validate(&config)?;
        let previous = self.applied.read().expect("mesh configuration").clone();
        if previous == config {
            return Ok(());
        }
        managed::configure_changed(&self.root, &config, &self.node, Some(&previous)).await?;
        {
            let mut peers = self.peers.lock().expect("mesh peer cache");
            peers.retain(|origin, _| station_peers(&config).any(|p| p.origin == *origin));
            for peer in station_peers(&config) {
                let status = peers
                    .entry(peer.origin.clone())
                    .or_insert_with(|| PeerStatus {
                        origin: peer.origin.clone(),
                        name: peer.name.clone(),
                        online: false,
                        last_seen_at: None,
                        execute_workspaces: vec![],
                    });
                status.name = peer.name.clone();
            }
        }
        *self.applied.write().expect("mesh configuration") = config;
        state
            .db
            .realtime
            .notify(crate::realtime::MESH | crate::realtime::WORK);
        Ok(())
    }

    pub(crate) async fn watch_tool(
        &self,
        origin: &str,
        body: crate::tool_stream::Watch,
    ) -> Result<tokio::sync::mpsc::Receiver<Value>> {
        let mut stream = self
            .node
            .subscribe(
                origin,
                &json!({"v":1,"request":{"kind":"watch_tool","body":body}}),
            )
            .await?;
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tx.closed() => break,
                    next = stream.next() => match next {
                        Ok(Some(value)) => if tx.send(value).await.is_err() { break; },
                        Ok(None) => break,
                        Err(error) => { let _ = tx.send(json!({"error":error.to_string()})).await; break; }
                    }
                }
            }
        });
        Ok(rx)
    }
    pub async fn node_tool_call(
        &self,
        origin: &str,
        body: crate::node_tools::Rpc,
    ) -> Result<Value> {
        let reply = self
            .node
            .subscribe(
                origin,
                &json!({"v":1,"request":{"kind":"node_tool","body":body}}),
            )
            .await?
            .next()
            .await?
            .context("tool_transport_closed")?;
        ensure!(
            reply["error"] != "invalid_mesh_request",
            "device_unsupported"
        );
        ensure!(
            reply["ok"] == true,
            "{}",
            reply["error"]
                .as_str()
                .unwrap_or("device_remote_unavailable")
        );
        Ok(reply["data"].clone())
    }
    pub async fn membership_call(&self, origin: &str, action: &str, body: Value) -> Result<Value> {
        let reply = self
            .node
            .exchange(
                origin,
                &json!({"v":1,"request":{"kind":"membership","action":action,"body":body}}),
            )
            .await?;
        ensure!(
            reply["ok"] == true,
            "{}",
            reply["error"]
                .as_str()
                .unwrap_or("mesh_membership_unavailable")
        );
        Ok(reply["data"].clone())
    }
    pub async fn wait(&self) -> Result<()> {
        let mut runtime = self.runtime.lock().await;
        match runtime.as_mut() {
            Some(runtime) => runtime.wait().await,
            None => std::future::pending().await,
        }
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.enrollment.stop_join().await;
        self.adb.shutdown();
        self.services.disconnect().await;
        let tasks = std::mem::take(&mut *self.tasks.lock().expect("Mesh tasks"));
        for task in &tasks {
            task.abort();
        }
        for task in tasks {
            let _ = task.await;
        }
        let subscriptions = std::mem::take(&mut *self.owner_subscriptions.lock().await);
        for (_, task) in subscriptions {
            task.abort();
            let _ = task.await;
        }
        // These are independent endpoints. Waiting for enrollment's external
        // address probes first can delay transport close beyond the desktop peer's
        // quit window, turning a local close into a timeout.
        let runtime = self.runtime.lock().await.take();
        let ((), stopped) = tokio::join!(self.enrollment.transport.close(), async {
            if let Some(mut runtime) = runtime {
                runtime.shutdown().await?;
            }
            Ok::<_, anyhow::Error>(())
        });
        stopped?;
        Ok(())
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }
    /// What this node currently publishes: the relays it can be reached
    /// through and the direct sockets it holds.
    ///
    /// An empty direct list is the honest answer for a node that has not
    /// learned a reachable address of its own — the state a network that drops
    /// address discovery leaves it in, and the state in which every peer
    /// connection falls back to a relay.
    pub fn address(&self) -> Value {
        match self.node.address() {
            Ok(address) => json!({
                "relays": address
                    .relay_urls()
                    .map(|relay| relay.as_str().to_string())
                    .collect::<Vec<_>>(),
                "direct": address
                    .ip_addrs()
                    .map(|addr| addr.to_string())
                    .collect::<Vec<_>>(),
            }),
            Err(_) => Value::Null,
        }
    }

    pub async fn prepare(root: &Path) -> Result<Option<Prepared>> {
        let mut config = zork_config::load_config(root)?.mesh;
        if !config.enabled {
            return Ok(None);
        }
        zork_config::services::ServicesConfig::load_for_data_root(
            &zork_config::relay_account::resolve_root(root)?,
        )?
        .apply_defaults(&mut config)?;
        managed::validate(&config)?;
        let control_state = Arc::new(std::sync::OnceLock::new());
        let runtime = managed::start_with_control(
            root,
            &config,
            Arc::new(ControlIngress {
                state: control_state.clone(),
            }),
        )
        .await?;
        let node = runtime.node();
        let origin = node.identity().await?;
        ensure!(
            origin.starts_with("key:"),
            "Zork Mesh currently requires a key origin"
        );
        managed::configure(root, &config, &node).await?;
        let files = zork_config::files_root(root);
        std::fs::create_dir_all(&files)?;
        let peer_cache = root.join("mesh/peer-capabilities.json");
        let cached: std::collections::BTreeMap<String, Vec<String>> = std::fs::read(&peer_cache)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let peers = station_peers(&config)
            .map(|p| {
                (
                    p.origin.clone(),
                    PeerStatus {
                        origin: p.origin.clone(),
                        name: p.name.clone(),
                        online: false,
                        last_seen_at: None,
                        execute_workspaces: cached.get(&p.origin).cloned().unwrap_or_default(),
                    },
                )
            })
            .collect();
        let enrollment =
            Arc::new(crate::enrollment::EnrollmentService::new(root, &config, node.clone()).await?);
        Ok(Some(Prepared {
            service: Arc::new(Self {
                services: Arc::new(crate::shared_services::Registry::open(root)?),
                adb: Arc::new(crate::adb::Registry::default()),
                node,
                runtime: tokio::sync::Mutex::new(Some(runtime)),
                tasks: Default::default(),
                origin,
                root: root.into(),
                applied: std::sync::RwLock::new(config),
                refresh_lock: Default::default(),
                enrollment,
                peer_cache,
                peers: std::sync::Mutex::new(peers),
                owner_subscriptions: Default::default(),
            }),
            state: control_state,
        }))
    }

    fn peer(&self, origin: &str) -> Result<zork_config::MeshPeer> {
        self.config()?
            .peers
            .iter()
            .find(|peer| peer.origin == origin)
            .cloned()
            .context("mesh_peer_not_paired")
    }
    fn execution_workspace(&self, origin: &str, id: &str) -> Result<zork_config::MeshWorkspace> {
        self.peer(origin)?;
        self.config()?
            .workspaces
            .iter()
            .find(|workspace| workspace.id == id)
            .cloned()
            .context("mesh_workspace_unavailable")
    }
    fn execution_worker(
        &self,
        state: &AppState,
        assignment: &Assignment,
    ) -> Result<crate::db::agents::NodeAgent> {
        let target = assignment.worker.as_ref().context("mesh_worker_required")?;
        let config = zork_config::load_config(&state.config.data_root)?.mesh;
        config
            .peers
            .iter()
            .find(|p| p.origin == assignment.owner_origin)
            .context("mesh_peer_not_paired")?;
        let worker = state
            .db
            .node_agent(&target.worker_id)?
            .context("mesh_worker_not_found")?;
        Ok(worker)
    }
    pub async fn remote_workers(&self, leader_id: &str) -> Vec<Value> {
        let config = self.config().unwrap_or_default();
        let requests=station_peers(&config).map(|peer|async move {
            let result=tokio::time::timeout(Duration::from_secs(3),self.call(&peer.origin,RpcRequest::Workers{leader_id:leader_id.into()})).await;
            match result {Ok(Ok(value))=>value["items"].as_array().into_iter().flatten().filter_map(|item|Some(json!({"id":format!("{}/{}",peer.origin,item["id"].as_str()?),"name":item["name"],"node":peer.name,"origin":peer.origin}))).collect::<Vec<_>>(),_=>vec![]}
        });
        futures_util::future::join_all(requests)
            .await
            .into_iter()
            .flatten()
            .collect()
    }
    pub async fn remote_worker_available(&self, leader: &str, worker: &str) -> Option<bool> {
        let (origin, id) = worker.split_once('/')?;
        match tokio::time::timeout(
            Duration::from_secs(3),
            self.call(
                origin,
                RpcRequest::Workers {
                    leader_id: leader.into(),
                },
            ),
        )
        .await
        {
            Ok(Ok(value)) => Some(
                value["items"]
                    .as_array()
                    .is_some_and(|items| items.iter().any(|w| w["id"] == id)),
            ),
            _ => None,
        }
    }
    pub fn assign_worker(
        &self,
        state: &AppState,
        leader: &crate::db::agents::NodeAgent,
        worker_id: &str,
        request_id: &str,
        goal: &str,
    ) -> Result<Value> {
        let (origin, id) = worker_id
            .split_once('/')
            .context("invalid remote Worker ID")?;
        self.peer(origin)?;
        ensure!(
            !id.is_empty()
                && id.len() <= 96
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                && goal.len() <= 32 * 1024,
            "invalid remote Worker or goal"
        );
        ensure!(
            zork_config::load_config(&state.config.data_root)?
                .mesh
                .peers
                .iter()
                .any(|p| p.origin == origin),
            "mesh_peer_not_paired"
        );
        let (key, runtime_id, _) = state
            .db
            .worker_task_allocation(&leader.id, request_id, worker_id, goal)?;
        let parts = key.split(':').collect::<Vec<_>>();
        ensure!(parts.len() == 3, "invalid allocation");
        let session = state.db.ensure_session(EnsureSession {
            connection_id: LOCAL_GUI_ENTRY_ID,
            platform: LOCAL_GUI_PLATFORM,
            channel_id: parts[1],
            root_thread_ts: parts[2],
            channel_type: Some("worker_task"),
            initiator_user_id: Some(&leader.id),
            initiator_message_ts: None,
        })?;
        if session.id.is_none() {
            state
                .db
                .set_agent_session(&key, &runtime_id, &session.workspace_path, "", "", "")?;
        }
        let session = state.db.get_session(&key)?.context("owner task missing")?;
        state.db.ensure_product_task(&key)?;
        let task = state
            .db
            .product_task_for_session(&key)?
            .context("owner task missing")?;
        let goal = state.db.copy_message_files(
            leader
                .session_key
                .as_deref()
                .context("leader session missing")?,
            &session,
            goal,
        )?;
        state.db.mesh_delegate(
            &Assignment {
                assignment_id: format!("worker-{runtime_id}"),
                task_id: task.task_id.clone(),
                owner_origin: self.origin.clone(),
                executor_origin: origin.into(),
                workspace_id: format!("worker-{id}"),
                goal: goal.into(),
                worker: Some(crate::db::mesh::WorkerTarget {
                    leader_id: leader.id.clone(),
                    worker_id: id.into(),
                }),
            },
            &session,
            task.revision,
        )?;
        state.db.set_worker_task_state(&key, "sent")?;
        Ok(
            json!({"task":state.db.product_task_for_session(&key)?,"worker_id":worker_id,"session_id":runtime_id}),
        )
    }
    async fn call(&self, origin: &str, request: RpcRequest) -> Result<Value> {
        self.peer(origin)?;
        let payload = json!({"v":1,"request":request});
        let reply = self.node.exchange(origin, &payload).await?;
        ensure!(reply["v"] == 1, "mesh_protocol_version");
        ensure!(
            reply["ok"] == true,
            "{}",
            reply["error"].as_str().unwrap_or("mesh_request_failed")
        );
        Ok(reply["data"].clone())
    }
}

async fn membership_request(
    state: &AppState,
    origin: &str,
    action: &str,
    body: Value,
) -> Result<Value> {
    let service = state.mesh.get().context("mesh_not_ready")?;
    let group = zork_config::load_config(&state.config.data_root)?
        .mesh
        .group
        .context("mesh_membership_missing")?;
    if action == "apply_membership" {
        return crate::enrollment::apply_group(
            state,
            origin,
            serde_json::from_value(body["group"].clone())?,
        )
        .await;
    }
    ensure!(
        group.authority == service.origin() && group.contains(origin),
        "mesh_member_required"
    );
    match action {
        "membership" => Ok(json!({"group":group})),
        "rename_device" => {
            service
                .enrollment
                .rename_device(
                    state,
                    origin,
                    body["name"].as_str().context("device_name_required")?,
                )
                .await
        }
        "update_routes" => {
            service
                .enrollment
                .update_routes(
                    state,
                    origin,
                    serde_json::from_value(body["routes"].clone())?,
                )
                .await
        }
        "create_invite" => service.enrollment.create(state).await,
        "list_invites" => service.enrollment.list(state).await,
        "revoke_invite" => {
            service
                .enrollment
                .revoke(state, body["id"].as_str().context("invite_id_required")?)
                .await
        }
        "remove_member" => {
            service
                .enrollment
                .remove_member(
                    state,
                    body["origin"]
                        .as_str()
                        .context("device_identity_required")?,
                )
                .await
        }
        "register_client" => {
            service
                .enrollment
                .register_client(state, serde_json::from_value(body["device"].clone())?)
                .await
        }
        _ => anyhow::bail!("unknown_membership_action"),
    }
}

async fn maintain_membership(service: Arc<MeshService>, state: AppState) {
    let mut changes = state.db.realtime.listen(crate::realtime::MESH);
    let mut watchers =
        std::collections::HashMap::<String, (zork_config::MeshPeer, zork_notify::Task<()>)>::new();
    let mut membership: Option<(String, zork_notify::Task<()>)> = None;
    let mut retry = zork_notify::retry::Retry::default();
    loop {
        changes.checkpoint();
        let config = match service.refresh(&state).await.and_then(|_| service.config()) {
            Ok(config) => config,
            Err(error) => {
                tracing::warn!(%error, "Mesh configuration will retry");
                tokio::select! {
                    _ = retry.wait() => {},
                    changed = changes.changed() => if changed.is_err() { return; },
                }
                continue;
            }
        };
        retry.reset();
        watchers.retain(|origin, (previous, task)| {
            !task.is_finished()
                && station_peers(&config).any(|p| p.origin == *origin && p == previous)
        });
        for peer in station_peers(&config)
            .filter(|p| !watchers.contains_key(&p.origin))
            .cloned()
            .collect::<Vec<_>>()
        {
            let task = zork_notify::Task(tokio::spawn(watch_peer(
                service.clone(),
                state.clone(),
                peer.clone(),
            )));
            watchers.insert(peer.origin.clone(), (peer, task));
        }
        let authority = config
            .group
            .as_ref()
            .filter(|g| g.authority != service.origin() && g.contains(service.origin()))
            .map(|g| g.authority.clone());
        if membership.as_ref().map(|(id, _)| id) != authority.as_ref() {
            membership = authority.map(|authority| {
                let task = zork_notify::Task(tokio::spawn(watch_membership(
                    service.clone(),
                    state.clone(),
                    authority.clone(),
                )));
                (authority, task)
            });
        }
        if changes.changed().await.is_err() {
            return;
        }
    }
}

async fn watch_membership(service: Arc<MeshService>, state: AppState, authority: String) {
    let request = WatchTopic::Membership;
    let mut retry = zork_notify::retry::Retry::default();
    loop {
        let mut feed = service.node.follow(authority.clone(), request.clone());
        while let Some(event) = feed.next().await {
            match event {
                zork_mesh::feed::Event::Data(value) => {
                    let result = async {
                        let group = serde_json::from_value(value["group"].clone())?;
                        let applied =
                            crate::enrollment::apply_group(&state, &authority, group).await?;
                        if applied["applied"] != true {
                            // Authority-owned invitations can change without a
                            // membership revision. Forward their invalidation.
                            state.db.realtime.notify(crate::realtime::MESH);
                        }
                        Ok::<_, anyhow::Error>(())
                    }
                    .await;
                    if let Err(error) = result {
                        tracing::warn!(%error, "Mesh membership application will retry");
                        break;
                    }
                    retry.reset();
                }
                zork_mesh::feed::Event::Disconnected { terminal: true, .. } => return,
                zork_mesh::feed::Event::Disconnected { .. } => {}
            }
        }
        drop(feed);
        retry.wait().await;
    }
}

async fn watch_peer(service: Arc<MeshService>, state: AppState, peer: zork_config::MeshPeer) {
    let request = WatchTopic::Peer;
    let mut feed = service.node.follow(peer.origin.clone(), request);
    while let Some(event) = feed.next().await {
        let workspaces = match event {
            zork_mesh::feed::Event::Data(value) => {
                let Ok(workspaces) =
                    serde_json::from_value::<Vec<String>>(value["execute_workspaces"].clone())
                else {
                    continue;
                };
                Some(workspaces)
            }
            zork_mesh::feed::Event::Disconnected { .. } => None,
        };
        let mut peers = service.peers.lock().expect("mesh peer cache");
        if let Some(status) = peers.get_mut(&peer.origin) {
            let changed = status.online != workspaces.is_some()
                || workspaces
                    .as_ref()
                    .is_some_and(|next| &status.execute_workspaces != next);
            status.online = workspaces.is_some();
            if let Some(workspaces) = workspaces {
                status.last_seen_at = Some(crate::config::now_rfc3339());
                status.execute_workspaces = workspaces;
            }
            if changed {
                state
                    .db
                    .realtime
                    .notify(crate::realtime::MESH | crate::realtime::WORK);
                let cached: std::collections::BTreeMap<_, _> = peers
                    .iter()
                    .map(|(id, p)| (id, &p.execute_workspaces))
                    .collect();
                if let Ok(bytes) = serde_json::to_vec(&cached) {
                    let temp = service.peer_cache.with_extension("tmp");
                    if std::fs::write(&temp, bytes).is_ok() {
                        let _ = std::fs::rename(temp, &service.peer_cache);
                    }
                }
            }
        }
    }
}

/// Serves the native control ALPN: peer requests straight into the station's
/// request dispatcher, with no socket program and no loopback hop.
struct ControlIngress {
    state: Arc<std::sync::OnceLock<AppState>>,
}

impl std::fmt::Debug for ControlIngress {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ControlIngress")
            .finish_non_exhaustive()
    }
}

impl zork_mesh::control::ControlHandler for ControlIngress {
    fn serve(
        &self,
        peer: Peer,
        request: Value,
        mut stream: zork_mesh::control::ControlStream,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send>> {
        let state = self.state.clone();
        Box::pin(async move {
            let Some(state) = state.get().cloned() else {
                stream
                    .write(&json!({"v":1,"ok":false,"status":503,"error":"mesh_not_ready"}))
                    .await?;
                return Ok(());
            };
            match dispatch(state, peer, request).await {
                Ok(Reply::Once(value)) => stream.write(&value).await,
                Ok(Reply::Subscription(mut rx)) => {
                    while let Some(frame) = rx.recv().await {
                        stream.write(&frame).await?;
                    }
                    Ok(())
                }
                Ok(Reply::Tunnel {
                    mut upstream,
                    mut cancelled,
                    guard,
                }) => {
                    let _guard = guard;
                    ensure!(!*cancelled.borrow(), "tunnel_cancelled");
                    stream.write(&json!({"v":1,"ok":true})).await?;
                    tokio::select! {
                        result = stream.splice(&mut upstream) => result,
                        _ = async { while !*cancelled.borrow_and_update() { if cancelled.changed().await.is_err() { break; } } } => Ok(()),
                    }
                }
                Err(error) => {
                    stream
                        .write(&json!({"v":1,"ok":false,"error":error.to_string()}))
                        .await
                }
            }
        })
    }
}

/// Dispatches one inbound request authenticated by the native control ALPN.
pub async fn dispatch(state: AppState, peer: Peer, request: Value) -> Result<Reply> {
    if matches!(
        request.pointer("/request/kind").and_then(Value::as_str),
        Some("adb_register" | "adb_stream")
    ) {
        return Ok(match crate::adb::handle(state, peer, request).await {
            Ok(reply) => reply,
            Err(error) => Reply::Once(json!({"v":1,"ok":false,"error":error.to_string()})),
        });
    }
    if request.pointer("/request/kind").and_then(Value::as_str) == Some("watch_tool") {
        let result = async {
            ensure!(request["v"] == 1, "mesh_protocol_version");
            let body = serde_json::from_value(request["request"]["body"].clone())?;
            crate::tool_stream::remote(state, &peer.origin, body).await
        }
        .await;
        return Ok(match result {
            Ok(rx) => Reply::Subscription(rx),
            Err(error) => Reply::Once(json!({"error":error.to_string()})),
        });
    }
    if request.pointer("/request/kind").and_then(Value::as_str) == Some("service") {
        return match crate::shared_services::tunnel(state, peer, request).await {
            Ok(reply) => Ok(reply),
            Err(error) => Ok(Reply::Once(
                json!({"v":1,"ok":false,"error":error.to_string()}),
            )),
        };
    }
    if matches!(
        request.pointer("/request/kind").and_then(Value::as_str),
        Some("watch" | "subscribe")
    ) {
        return match mesh_subscription(&state, peer, request).await {
            Ok(rx) => Ok(Reply::Subscription(rx)),
            Err(e) => Ok(Reply::Once(json!({"v":1,"ok":false,"error":e.to_string()}))),
        };
    }
    if matches!(
        request.pointer("/request/kind").and_then(Value::as_str),
        Some("node_tool" | "business_card" | "interaction_registration" | "client_browser")
    ) || (request.pointer("/request/kind").and_then(Value::as_str) == Some("channel_tool")
        && request
            .pointer("/request/body/tool")
            .and_then(Value::as_str)
            .is_some_and(|tool| {
                zork_agent_station_tools::channels::participating(
                    tool,
                    &request["request"]["body"]["arguments"],
                )
            }))
    {
        // These asynchronous operations may wait for resources
        // or user participation. Complete the stream handshake
        // before waiting; setup deadlines do not bound execution.
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        tokio::spawn(async move {
            let result = tokio::select! {
                _ = tx.closed() => return,
                result = handle(&state,peer,request) => result,
            };
            let reply = match result {
                Ok(data) => json!({"v":1,"ok":true,"data":data}),
                Err(error) => json!({"v":1,"ok":false,"error":error.to_string()}),
            };
            let _ = tx.send(reply).await;
        });
        return Ok(Reply::Subscription(rx));
    }
    let response = handle(&state, peer, request).await;
    Ok(Reply::Once(match response {
        Ok(data) => json!({"v":1,"ok":true,"data":data}),
        Err(error) => json!({"v":1,"ok":false,"error":error.to_string()}),
    }))
}

pub fn start(prepared: Prepared, state: AppState) {
    let _ = prepared.state.set(state.clone());
    let service = prepared.service;
    service.enrollment.resume_join(state.clone());
    let owner = service.clone();
    let services = service.services.clone();
    let service_task = tokio::spawn(services.run());
    let mut tasks = vec![crate::enrollment::start(
        service.enrollment.clone(),
        state.clone(),
    )];
    {
        let service = service.clone();
        let state = state.clone();
        tasks.push(tokio::spawn(async move {
            let root = service.root.clone();
            let account_task = zork_client_core::relay_account::devices::start(
                &root,
                service.node.clone(),
                true,
                move |devices| {
                    let devices: Vec<_> = devices
                        .into_iter()
                        .filter(|device| !device.station)
                        .collect();
                    let service = service.clone();
                    let state = state.clone();
                    async move {
                        let root = service.root.clone();
                        let origin = service.origin.clone();
                        tokio::task::spawn_blocking(move || -> Result<()> {
                            let current = zork_config::load_config(&root)?.mesh;
                            let project = |config: &mut zork_config::MeshConfig| -> Result<()> {
                                let previous = config.account_peers.clone();
                                config.peers.retain(|peer| {
                                    !previous.contains(&peer.origin)
                                        || devices.iter().any(|device| device.origin == peer.origin)
                                });
                                let mut owned = Vec::new();
                                for device in &devices {
                                    if config.peers.iter().any(|peer| peer.origin == device.origin)
                                        && !previous.contains(&device.origin)
                                    {
                                        continue;
                                    }
                                    let peer = zork_config::MeshPeer {
                                        origin: device.origin.clone(),
                                        name: device.name.clone(),
                                        addr: None,
                                        routes: None,
                                        execute: vec![],
                                        client: !device.station,
                                        collaborate: device.station,
                                    };
                                    if let Some(existing) = config
                                        .peers
                                        .iter_mut()
                                        .find(|peer| peer.origin == device.origin)
                                    {
                                        *existing = peer;
                                    } else {
                                        config.peers.push(peer);
                                    }
                                    owned.push(device.origin.clone());
                                }
                                if let Some(mut group) = config.group.clone() {
                                    let original = group.clone();
                                    group.clients.retain(|client| {
                                        !previous.contains(&client.origin)
                                            || devices
                                                .iter()
                                                .any(|device| device.origin == client.origin)
                                    });
                                    for device in &devices {
                                        if owned.contains(&device.origin)
                                            && !group
                                                .clients
                                                .iter()
                                                .any(|client| client.origin == device.origin)
                                        {
                                            group.clients.push(
                                                zork_config::membership::MeshDevice {
                                                    origin: device.origin.clone(),
                                                    name: device.name.clone(),
                                                    addr: None,
                                                    routes: None,
                                                },
                                            );
                                        }
                                    }
                                    if group != original && group.authority == origin {
                                        group.revision += 1;
                                        group.apply(&origin, config)?;
                                    }
                                }
                                config.account_peers = owned;
                                managed::validate(config)
                            };
                            let mut next = current.clone();
                            project(&mut next)?;
                            if next != current {
                                zork_config::update_config(&root, |config| {
                                    project(&mut config.mesh)
                                })?;
                            }
                            Ok(())
                        })
                        .await??;
                        service.refresh(&state).await?;
                        // Pinned public addresses are transport hints, not a second trust source.
                        Ok(())
                    }
                },
            );
            match account_task {
                Ok(task) => {
                    let _ = task.await;
                }
                Err(error) => tracing::warn!(%error,"account device discovery unavailable"),
            }
        }));
    }
    tasks.push(service_task);
    tasks.push(tokio::spawn(maintain_membership(
        service.clone(),
        state.clone(),
    )));
    tasks.push(crate::enrollment::start_routes(
        service.enrollment.clone(),
        state.clone(),
    ));
    tasks.push(tokio::spawn(async move {
        if let Err(error) = state.db.mesh_recover_dispatch() {
            tracing::error!(%error,"Mesh recovery failed");
            return;
        }
        let mut changes = state
            .db
            .realtime
            .listen(crate::realtime::WORK | crate::realtime::MESH);
        let mut retry =
            zork_notify::retry::Retry::new(Duration::from_secs(2), Duration::from_secs(2));
        loop {
            changes.checkpoint();
            let failed = match work(&service, &state).await {
                Ok(failed) => failed,
                Err(error) => {
                    tracing::warn!(%error,"Mesh work will retry");
                    true
                }
            };
            if failed {
                retry.wait().await;
            } else if changes.changed().await.is_err() {
                break;
            }
        }
    }));
    *owner.tasks.lock().expect("Mesh tasks") = tasks;
}

async fn handle(state: &AppState, peer: Peer, request: Value) -> Result<Value> {
    let service = state.mesh.get().context("mesh_disabled")?;
    let envelope: Envelope = serde_json::from_value(request).context("invalid_mesh_request")?;
    ensure!(envelope.v == 1, "mesh_protocol_version");
    if let RpcRequest::ConfirmJoin {
        id,
        secret,
        challenge,
        address,
    } = &envelope.request
    {
        return service
            .enrollment
            .confirm(state, &peer.origin, id, secret, challenge, address.clone())
            .await;
    }
    if let RpcRequest::Membership { action, body } = &envelope.request {
        return membership_request(state, &peer.origin, action, body.clone()).await;
    }
    let live_config = zork_config::load_config(&state.config.data_root)?.mesh;
    let _paired = live_config
        .peers
        .iter()
        .find(|p| p.origin == peer.origin)
        .context("mesh_peer_not_paired")?;
    match envelope.request {
        RpcRequest::NodeTool { body } => crate::node_tools::remote(state, &peer.origin, body).await,
        RpcRequest::ChannelTool { body } => {
            crate::channels::remote(state, &peer.origin, body).await
        }
        RpcRequest::InteractionRegistration { body } => {
            crate::interaction_registry::remote(state, &peer.origin, body).await
        }
        RpcRequest::BusinessCard { body } => {
            crate::business_cards::remote(state, &peer.origin, body).await
        }
        RpcRequest::ChannelFile { body } => {
            crate::channels::remote_file(state, &peer.origin, body).await
        }
        RpcRequest::ConfirmJoin { .. } | RpcRequest::Membership { .. } => unreachable!(),
        RpcRequest::Watch { .. } | RpcRequest::Subscribe { .. } => {
            anyhow::bail!("subscription_requires_stream")
        }
        RpcRequest::Client { method, path, body } => {
            client_request(state, &method, &path, body).await
        }
        RpcRequest::ClientFile {
            artifact_id,
            offset,
        } => {
            ensure!(artifact_id.len() <= 200, "invalid_artifact_id");
            let db = state.db.clone();
            Ok(
                match tokio::task::spawn_blocking(move || db.artifact_chunk(&artifact_id, offset))
                    .await?
                {
                    Ok(Some(chunk)) => json!({"status":200,"body":chunk}),
                    Ok(None) => json!({"status":404,"body":{"error":"artifact_not_found"}}),
                    Err(error) => json!({"status":400,"body":{"error":error.to_string()}}),
                },
            )
        }
        RpcRequest::ClientBrowser {
            session_id,
            client_id,
            command,
        } => {
            crate::browser::command_for_client(
                state,
                &peer.origin,
                &session_id,
                &client_id,
                command,
            )
            .await
        }
        RpcRequest::InputFile {
            assignment_id,
            file_id,
        } => {
            let link = state
                .db
                .mesh_link(&assignment_id)?
                .context("unknown_mesh_assignment")?;
            ensure!(
                link.role == "owner" && link.assignment.executor_origin == peer.origin,
                "attachment_assignment_unauthorized"
            );
            let (_, files) = zork_client_types::files::decode(&link.assignment.goal)
                .context("assignment_has_no_files")?;
            let file = files
                .iter()
                .find(|f| f.id == file_id)
                .context("attachment_not_in_assignment")?;
            let bytes = state.db.conversation_file_bytes(
                link.session_key
                    .as_deref()
                    .context("assignment_not_bound")?,
                file,
            )?;
            let space = format!("zork-ws-{}", link.assignment.workspace_id);
            let object = service
                .node
                .put(
                    &space,
                    &format!("inputs/{}/{}", file.id, file.content_root),
                    &bytes,
                )
                .await?;
            Ok(json!({"object":object}))
        }
        RpcRequest::ExecutionHistory {
            assignment_id,
            query,
        } => {
            let link = authorized_assignment(state, &peer.origin, &assignment_id)?;
            let session = state
                .db
                .get_session(
                    link.session_key
                        .as_deref()
                        .context("assignment_not_bound")?,
                )?
                .context("assignment_not_bound")?;
            let runtime = session.id.as_deref().context("assignment_not_bound")?;
            // Reading history never allocates or prepares an execution context.
            let page = crate::agent::session_history(&state.agent, runtime, &query).await?;
            let bytes = serde_json::to_vec(&page)?;
            ensure!(
                bytes.len() <= zork_mesh::MAX_ARTIFACT,
                "history_response_too_large"
            );
            if bytes.len() > 128 * 1024 {
                let object = service
                    .node
                    .put(
                        "zork-client",
                        &format!("history/{}", zork_mesh::content_root(&bytes)),
                        &bytes,
                    )
                    .await?;
                Ok(json!({"object":object}))
            } else {
                Ok(json!({"page":page}))
            }
        }
        RpcRequest::Rework {
            assignment_id,
            request_id,
            goal,
        } => {
            ensure!(
                !request_id.is_empty()
                    && request_id.len() <= 96
                    && request_id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                    && !goal.trim().is_empty()
                    && goal.len() <= 32 * 1024,
                "invalid rework request"
            );
            let link = authorized_assignment(state, &peer.origin, &assignment_id)?;
            service.execution_worker(state, &link.assignment)?;
            let _guard = state
                .entries
                .lock_local_task(&format!("mesh-exec:{assignment_id}"))
                .await;
            let key = link
                .session_key
                .as_deref()
                .context("Worker session unavailable")?;
            let session = state
                .db
                .get_session(key)?
                .context("Worker session unavailable")?;
            let id = session
                .id
                .as_deref()
                .context("Worker runtime unavailable")?;
            let receipt = format!("rework-{assignment_id}-{request_id}");
            if !state.db.has_message_receipt(&receipt, key, &goal)? {
                state
                    .entries
                    .accept_local_user_message(&session, &receipt, &goal)?;
            }
            crate::agent::append_mailbox_id(&state.agent, id, &receipt, &goal).await?;
            Ok(json!({"durable":true}))
        }
        RpcRequest::Workers { .. } => {
            let items = state
                .db
                .node_agents()?
                .into_iter()
                .map(|a| json!({"id":a.id,"name":a.name}))
                .collect::<Vec<_>>();
            Ok(json!({"items":items}))
        }
        RpcRequest::Hello => Ok(
            json!({"origin":service.origin,"authenticated_peer":peer.origin,"execute_workspaces":live_config.workspaces.iter().map(|w|w.id.clone()).collect::<Vec<_>>(),"protocol":1}),
        ),
        RpcRequest::Delegate { assignment } => {
            ensure!(
                assignment.owner_origin == peer.origin
                    && assignment.executor_origin == service.origin,
                "mesh_assignment_identity_mismatch"
            );
            if let Some(existing) = state.db.mesh_link(&assignment.assignment_id)? {
                ensure!(
                    existing.role == "executor" && existing.assignment == assignment,
                    "mesh_command_conflict"
                );
                return Ok(
                    json!({"assignment_id":assignment.assignment_id,"durable":true,"state":existing.state}),
                );
            }
            if assignment.worker.is_some() {
                service.execution_worker(state, &assignment)?;
            } else {
                service.execution_workspace(&peer.origin, &assignment.workspace_id)?;
            }
            let link = state.db.mesh_receive_assignment(&assignment)?;
            Ok(json!({"assignment_id":assignment.assignment_id,"durable":true,"state":link.state}))
        }
        RpcRequest::Cancel { assignment_id } => {
            let link = authorized_assignment(state, &peer.origin, &assignment_id)?;
            state.db.mesh_cancel_request(&assignment_id)?;
            let cancelled = cancel_executor(state, &link).await?;
            Ok(json!({"cancelled":cancelled}))
        }
        RpcRequest::Decision {
            assignment_id,
            decision,
        } => {
            authorized_assignment(state, &peer.origin, &assignment_id)?;
            state.db.mesh_receive_decision(&assignment_id, &decision)?;
            Ok(json!({"durable":true}))
        }
    }
}

fn client_route(method: &str, path: &str) -> bool {
    let raw = path.split('?').next().unwrap_or_default();
    if !raw.starts_with('/')
        || raw.contains("//")
        || raw
            .bytes()
            .any(|b| !(b.is_ascii_alphanumeric() || b"/-_.".contains(&b)))
    {
        return false;
    }
    let parts = raw.trim_start_matches('/').split('/').collect::<Vec<_>>();
    if parts.iter().any(|part| matches!(*part, "." | "..")) {
        return false;
    }
    match (method, parts.as_slice()) {
        (
            "GET" | "POST" | "DELETE",
            ["v1", "node", "chats", _, "messages", _, "provider-login"],
        ) => true,
        (
            "GET",
            ["readyz"]
            | ["v1", "im", "profiles"]
            | ["v1", "im", "sessions"]
            | ["v1", "tasks"]
            | ["v1", "inbox"]
            | ["v1", "artifacts"]
            | ["v1", "mesh"],
        ) => true,
        (
            "GET",
            ["v1", "tasks", _]
            | ["v1", "artifacts", _, "content"]
            | ["v1", "im", "sessions", _, "messages"]
            | ["v1", "im", "sessions", _, "context"]
            | ["v1", "im", "sessions", _, "history"]
            | ["v1", "im", "sessions", _, "status"],
        ) => true,
        (
            "POST",
            ["v1", "im", "chats"]
            | ["v1", "im", "sessions", _, "messages"]
            | ["v1", "im", "sessions", _, "files"]
            | ["v1", "client", "browser", "receipts"]
            | ["v1", "im", "sessions", _, "cancel"]
            | ["v1", "tasks", _, "transitions"]
            | ["v1", "tasks", _, "artifacts"],
        ) => true,
        ("PUT", ["v1", "im", "sessions", _, "context"]) => true,
        (
            "GET",
            ["v1", "node", "providers"]
            | ["v1", "node", "agents"]
            | ["v1", "node", "chats"]
            | ["v1", "node", "mesh"]
            | ["v1", "node", "status"]
            | ["v1", "node", "info"]
            | ["v1", "node", "resources"]
            | ["v1", "node", "pages"]
            | ["v1", "node", "resources", "service", _]
            | ["v1", "node", "update"]
            | ["v1", "node", "conversations", "read-markers"]
            | ["v1", "node", "profiles", _]
            | ["v1", "node", "profiles", _, "discovered-models"]
            | ["v1", "node", "mesh", "invites"]
            | ["v1", "node", "agents", _, "tasks"],
        ) => true,
        (
            "POST",
            ["v1", "node", "agents"]
            | ["v1", "node", "sync"]
            | ["v1", "node", "sync", "commands"]
            | ["v1", "node", "sync", "receipt"]
            | ["v1", "node", "update"]
            | ["v1", "node", "agents", _, "open"]
            | ["v1", "node", "chats", _, "archive"]
            | ["v1", "node", "chats", _, "messages", _, "agent-configuration"]
            | ["v1", "node", "auth"]
            | ["v1", "node", "profiles", _, "refresh"]
            | ["v1", "node", "profiles", _, "models", "refresh"]
            | ["v1", "node", "mesh", "invites"]
            | ["v1", "node", "mesh", "invites", _, "approve"]
            | ["v1", "node", "mesh", "members", "remove"]
            | ["v1", "node", "mesh", "clients"]
            | ["v1", "node", "auth", _],
        ) => true,
        (
            "PUT",
            ["v1", "node", "profiles", _]
            | ["v1", "node", "name"]
            | ["v1", "node", "profiles", _, "models"]
            | ["v1", "node", "profiles", _, "models", "enabled"]
            | ["v1", "node", "profiles", _, "name"]
            | ["v1", "node", "agents", _, "grants"]
            | ["v1", "node", "agents", _, "avatar"]
            | ["v1", "node", "mesh"],
        ) => true,
        ("PATCH", ["v1", "node", "agents", _, "model"]) => true,
        ("DELETE", ["v1", "node", "auth", _]) => true,
        ("DELETE", ["v1", "node", "mesh", "invites", _]) => true,
        _ => false,
    }
}
async fn client_request(
    state: &AppState,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Result<Value> {
    ensure!(client_route(method, path), "mesh_client_route_not_allowed");
    let config = zork_config::load_config(&state.config.data_root)?;
    let base = zork_config::loopback_base_url(&config.bind.runtime);
    let http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()?;
    let mut request = http
        .request(method.parse()?, format!("{base}{path}"))
        .bearer_auth(crate::node::local_token(&state.config.data_root)?);
    if let Some(body) = body {
        request = request.json(&body)
    }
    let mut response = request.send().await?;
    let status = response.status().as_u16();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= zork_mesh::MAX_ARTIFACT,
            "client_response_too_large"
        );
        bytes.extend_from_slice(&chunk);
    }
    let binary = path
        .split('?')
        .next()
        .is_some_and(|p| p.ends_with("/content"));
    if (200..300).contains(&status) && (binary || bytes.len() > 128 * 1024) {
        let service = state.mesh.get().context("mesh_disabled")?;
        let object = service
            .node
            .put(
                "zork-client",
                &format!("responses/{}", zork_mesh::content_root(&bytes)),
                &bytes,
            )
            .await?;
        Ok(json!({"status":status,"object":object}))
    } else {
        Ok(
            json!({"status":status,"body":serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null)}),
        )
    }
}

pub(crate) async fn browser_request(
    state: &AppState,
    origin: &str,
    session_id: &str,
    client_id: &str,
    command: zork_browser::Command,
) -> Result<Value> {
    let service = state.mesh.get().context("mesh_disabled")?;
    service
        .participation_call(
            origin,
            RpcRequest::ClientBrowser {
                session_id: session_id.into(),
                client_id: client_id.into(),
                command,
            },
        )
        .await
}

fn authorized_assignment(state: &AppState, peer: &str, id: &str) -> Result<Link> {
    let link = state.db.mesh_link(id)?.context("unknown_mesh_assignment")?;
    ensure!(
        link.role == "executor" && link.assignment.owner_origin == peer,
        "mesh_assignment_unauthorized"
    );
    Ok(link)
}

async fn prepare_executor(service: &MeshService, state: &AppState, link: &Link) -> Result<()> {
    let assignment = &link.assignment;
    if assignment.worker.is_some() {
        let worker = service.execution_worker(state, assignment)?;
        let conversation = format!("mesh-{}", assignment.assignment_id);
        let session = state.db.ensure_session(EnsureSession {
            connection_id: LOCAL_GUI_ENTRY_ID,
            platform: LOCAL_GUI_PLATFORM,
            channel_id: &conversation,
            root_thread_ts: &conversation,
            channel_type: Some("worker_task"),
            initiator_user_id: Some(&assignment.owner_origin),
            initiator_message_ts: None,
        })?;
        state.db.ensure_product_task(&session.key)?;
        state
            .db
            .mesh_bind_executor(&assignment.assignment_id, &session)?;
        let runtime_id = state.db.mesh_runtime_id(&assignment.assignment_id)?;
        let selection = crate::agent::SessionSelection {
            profile_id: worker.profile_id.clone(),
            model: worker.model.clone(),
            thinking: worker.thinking.clone(),
        };
        crate::agent::ensure_allocated_session(
            &state.agent,
            &state.db,
            &SessionBindingRow::Normal(session.clone()),
            &runtime_id,
            &selection,
            &crate::node::agent_prompt(&worker),
        )
        .await?;
        state
            .status_projection
            .ensure(
                &session.key,
                &runtime_id,
                &session.connection_id,
                &session.channel_id,
                &session.root_thread_ts,
            )
            .await;
        if let Some((_, files)) = zork_client_types::files::decode(&assignment.goal) {
            ensure!(
                zork_client_types::files::valid(&files),
                "invalid_attachments"
            );
            for file in files {
                if state
                    .db
                    .conversation_file_bytes(&session.key, &file)
                    .is_ok()
                {
                    continue;
                }
                let response = service
                    .call(
                        &assignment.owner_origin,
                        RpcRequest::InputFile {
                            assignment_id: assignment.assignment_id.clone(),
                            file_id: file.id.clone(),
                        },
                    )
                    .await?;
                let object: zork_mesh::node::ObjectRef =
                    serde_json::from_value(response["object"].clone())?;
                ensure!(
                    object.origin == assignment.owner_origin,
                    "attachment_wrong_origin"
                );
                let bytes = service.node.read(&object).await?;
                state.db.receive_complete_file(&session, &file, &bytes)?;
                state
                    .db
                    .record_file_source(&file.id, &json!({"object":object,"file":file}))?;
            }
        }
        let agent_input =
            crate::http::conversation_files::agent_content(state, &session.key, &assignment.goal)
                .await?;
        let agent_input = if let Some(chat) = assignment.assignment_id.strip_prefix("worker-") {
            state
                .db
                .record_chat_source(&worker.id, &assignment.owner_origin)?;
            json!({"source":"assignment","target":assignment.owner_origin,"chat_id":chat,"text":agent_input}).to_string()
        } else {
            agent_input
        };
        state
            .db
            .mesh_state(&assignment.assignment_id, "dispatching", None)?;
        match crate::agent::append_mailbox_id(
            &state.agent,
            &runtime_id,
            &format!("mesh-{}", assignment.assignment_id),
            &agent_input,
        )
        .await
        {
            Ok(()) => state
                .db
                .mesh_state(&assignment.assignment_id, "running", None)?,
            Err(error) => state.db.mesh_state(
                &assignment.assignment_id,
                "preparing",
                Some(&error.to_string()),
            )?,
        }
        return Ok(());
    }
    let workspace =
        service.execution_workspace(&assignment.owner_origin, &assignment.workspace_id)?;
    let workspace_path = std::fs::canonicalize(&workspace.path)?;
    let conversation = format!("mesh-{}", assignment.assignment_id);
    let key = format!("{LOCAL_GUI_ENTRY_ID}:{conversation}:{conversation}");
    // The local conversation key is deterministic; a retry cannot create a
    // second product task. A session with uncertain mailbox delivery is not retried.
    let session = match state.db.get_session(&key)? {
        Some(session) => session,
        None => state.db.create_session_at_workspace(
            EnsureSession {
                connection_id: LOCAL_GUI_ENTRY_ID,
                platform: LOCAL_GUI_PLATFORM,
                channel_id: &conversation,
                root_thread_ts: &conversation,
                channel_type: Some("mesh"),
                initiator_user_id: Some(&assignment.owner_origin),
                initiator_message_ts: None,
            },
            &workspace_path,
        )?,
    };
    state
        .db
        .mesh_bind_executor(&assignment.assignment_id, &session)?;
    let session_id = if let Some(id) = session.id.as_ref() {
        id.clone()
    } else {
        let selection = crate::agent::SessionSelection {
            profile_id: workspace.profile_id.clone(),
            model: workspace.model.clone(),
            thinking: workspace.thinking.clone(),
        };
        let created = crate::agent::create_binding_session(
            &state.agent,
            &state.db,
            &SessionBindingRow::Normal(session.clone()),
            &selection,
        )
        .await
        .map_err(|e| anyhow::anyhow!(e.message))?;
        created.session_id
    };
    state
        .status_projection
        .ensure(
            &session.key,
            &session_id,
            &session.connection_id,
            &session.channel_id,
            &session.root_thread_ts,
        )
        .await;
    state
        .db
        .mesh_state(&assignment.assignment_id, "dispatching", None)?;
    match crate::agent::append_mailbox(&state.agent, &session_id, &assignment.goal).await {
        Ok(()) => state
            .db
            .mesh_state(&assignment.assignment_id, "running", None)?,
        Err(_) => state
            .db
            .mesh_attention(&assignment.assignment_id, "runtime_delivery_uncertain")?,
    }
    Ok(())
}

async fn cancel_executor(state: &AppState, link: &Link) -> Result<bool> {
    // Serialize cancellation with preparation, so no run can start after a
    // cancellation has already been acknowledged.
    let _guard = state
        .entries
        .lock_local_task(&format!("mesh-exec:{}", link.assignment.assignment_id))
        .await;
    let link = state
        .db
        .mesh_link(&link.assignment.assignment_id)?
        .context("mesh link missing")?;
    if matches!(link.state.as_str(), "queued" | "preparing") {
        state
            .db
            .mesh_state(&link.assignment.assignment_id, "cancelled", None)?;
        return Ok(true);
    }
    let Some(key) = link.session_key.as_ref() else {
        return Ok(link.state == "cancelled");
    };
    let session = state.db.get_session(key)?.context("mesh session missing")?;
    let Some(id) = session.id.as_ref() else {
        return Ok(link.state == "cancelled");
    };
    crate::agent::cancel_session(&state.agent, id).await?;
    let statuses = crate::agent::session_statuses(&state.agent).await?;
    Ok(statuses.get(id).is_some_and(|status| {
        matches!(
            status.as_str(),
            "wait" | "finished" | "failed" | "cancelled"
        )
    }))
}

async fn work(service: &Arc<MeshService>, state: &AppState) -> Result<bool> {
    let mut failed = false;
    for link in state.db.mesh_links()? {
        if link.role == "executor" {
            if state
                .db
                .mesh_cancel_pending(&link.assignment.assignment_id)?
                && cancel_executor(state, &link).await.unwrap_or(false)
            {
                state.db.mesh_cancel_sent(&link.assignment.assignment_id)?;
            }
            let _guard = state
                .entries
                .lock_local_task(&format!("mesh-exec:{}", link.assignment.assignment_id))
                .await;
            let link = state
                .db
                .mesh_link(&link.assignment.assignment_id)?
                .context("mesh link missing")?;
            if matches!(link.state.as_str(), "queued" | "preparing") {
                if let Err(error) = prepare_executor(service, state, &link).await {
                    failed = true;
                    state.db.mesh_state(
                        &link.assignment.assignment_id,
                        &link.state,
                        Some(&error.to_string()),
                    )?;
                }
            }
            state.db.mesh_capture(&link.assignment.assignment_id)?;
        }
    }
    // Publishing files and immutable event exports is recoverable and occurs
    // outside the local product transaction. Do not expose an unready event.
    for mut event in state.db.mesh_pending_exports()? {
        let result = publish_event(service, state, &mut event).await;
        if let Err(error) = result {
            failed = true;
            tracing::warn!(%error,"Mesh export will retry");
        }
    }
    for link in state
        .db
        .mesh_links()?
        .into_iter()
        .filter(|link| link.role == "owner")
    {
        let result = work_owner(service, state, &link).await;
        if let Err(error) = result {
            failed = true;
            state.db.mesh_state(
                &link.assignment.assignment_id,
                &link.state,
                Some(&error.to_string()),
            )?;
        } else {
            ensure_owner_subscription(service, state, &link).await;
        }
    }
    Ok(failed)
}

async fn publish_event(
    service: &MeshService,
    state: &AppState,
    event: &mut MeshEvent,
) -> Result<()> {
    let link = state
        .db
        .mesh_link(&event.assignment_id)?
        .context("mesh link missing")?;
    let space = format!("zork-ws-{}", link.assignment.workspace_id);
    if let EventBody::Artifact {
        artifact_id,
        object,
        ..
    } = &mut event.body
    {
        let bytes = state
            .db
            .artifact_content(artifact_id)?
            .context("mesh artifact missing")?;
        let path = format!("artifacts/{artifact_id}/snapshot");
        *object = Some(service.node.put(&space, &path, &bytes).await?);
    }
    let bytes = serde_json::to_vec(event)?;
    service
        .node
        .put(
            &space,
            &format!("events/{:020}.json", event.sequence),
            &bytes,
        )
        .await?;
    state.db.mesh_export_ready(event)?;
    Ok(())
}

async fn work_owner(service: &MeshService, state: &AppState, link: &Link) -> Result<()> {
    let id = &link.assignment.assignment_id;
    let executor = &link.assignment.executor_origin;
    if link.state == "queued" {
        service
            .call(
                executor,
                RpcRequest::Delegate {
                    assignment: link.assignment.clone(),
                },
            )
            .await?;
        state.db.mesh_state(id, "received", None)?;
    }
    if state.db.mesh_cancel_pending(id)? {
        let reply = service
            .call(
                executor,
                RpcRequest::Cancel {
                    assignment_id: id.clone(),
                },
            )
            .await?;
        if reply["cancelled"] == true {
            state.db.mesh_cancel_sent(id)?;
            state.db.mesh_remote_cancelled(id)?;
        }
    }
    if let Some(decision) = state.db.mesh_pending_decision(id)? {
        service
            .call(
                executor,
                RpcRequest::Decision {
                    assignment_id: id.clone(),
                    decision,
                },
            )
            .await?;
        state.db.mesh_decision_sent(id)?;
    }
    for (request_id, goal) in state.db.mesh_pending_reworks(id)? {
        service
            .call(
                executor,
                RpcRequest::Rework {
                    assignment_id: id.clone(),
                    request_id: request_id.clone(),
                    goal,
                },
            )
            .await?;
        state.db.mesh_rework_sent(id, &request_id)?;
    }

    Ok(())
}

async fn apply_remote_batch(
    service: &MeshService,
    state: &AppState,
    link: &Link,
    reply: Value,
) -> Result<()> {
    let id = &link.assignment.assignment_id;
    let executor = &link.assignment.executor_origin;
    if let Some(key) = link.session_key.as_deref() {
        if reply["activity"].is_object() {
            state
                .entries
                .import_remote_activity(key, reply["activity"].clone());
        }
    }
    if reply["execution"].is_object() {
        if let Some(key) = link.session_key.as_deref() {
            if let Some(session) = state.db.get_session(key)? {
                let mut snapshot: zork_agent_api::SessionSnapshot =
                    serde_json::from_value(reply["execution"].clone())?;
                snapshot.session_id = session.id.context("owner_session_unavailable")?;
                state.entries.store_execution_snapshot(key, &snapshot)?;
            }
        }
    }
    let events: Vec<MeshEvent> = serde_json::from_value(reply["events"].clone())?;
    let mut previous = link.cursor;
    for event in events {
        ensure!(
            event.assignment_id == *id && event.sequence > previous,
            "invalid_mesh_event_order"
        );
        let content = if let EventBody::Artifact { object, .. } = &event.body {
            let object = object.as_ref().context("mesh artifact has no object")?;
            ensure!(
                object.origin == *executor
                    && object.space == format!("zork-ws-{}", link.assignment.workspace_id),
                "mesh_artifact_wrong_source"
            );
            Some(service.node.read(object).await?)
        } else {
            None
        };
        let applied = state.db.mesh_import(executor, &event, content.as_deref())?;
        if applied {
            if let EventBody::Message { text, .. } = &event.body {
                if let Some(key) = link.session_key.as_deref() {
                    state
                        .entries
                        .publish_imported_message(key, "assistant", text);
                }
            }
        }
        previous = event.sequence;
    }

    Ok(())
}

async fn ensure_owner_subscription(service: &Arc<MeshService>, state: &AppState, link: &Link) {
    let id = link.assignment.assignment_id.clone();
    let mut subscriptions = service.owner_subscriptions.lock().await;
    if matches!(link.state.as_str(), "settled" | "cancelled") {
        subscriptions.remove(&id);
        return;
    }
    if subscriptions
        .get(&id)
        .is_some_and(|task| !task.is_finished())
    {
        return;
    }
    subscriptions.remove(&id);
    let state = state.clone();
    let service = service.clone();
    let initial = link.clone();
    let task = tokio::spawn(async move {
        let request = |link: &Link| WatchTopic::Assignment {
            assignment_id: link.assignment.assignment_id.clone(),
            after: link.cursor,
        };
        let mut retry = zork_notify::retry::Retry::default();
        loop {
            let link = match state.db.mesh_link(&initial.assignment.assignment_id) {
                Ok(Some(link)) => link,
                _ => return,
            };
            let mut feed = service
                .node
                .follow(link.assignment.executor_origin.clone(), request(&link));
            while let Some(event) = feed.next().await {
                match event {
                    zork_mesh::feed::Event::Data(data) => {
                        let result = async {
                            let link = state
                                .db
                                .mesh_link(&initial.assignment.assignment_id)?
                                .context("mesh assignment disappeared")?;
                            apply_remote_batch(&service, &state, &link, data).await?;
                            let committed = state
                                .db
                                .mesh_link(&initial.assignment.assignment_id)?
                                .context("mesh assignment disappeared")?;
                            feed.resume_with(request(&committed))?;
                            Ok::<_, anyhow::Error>(())
                        }
                        .await;
                        if let Err(error) = result {
                            tracing::warn!(%error, "Mesh assignment application will retry");
                            break;
                        }
                        retry.reset();
                    }
                    zork_mesh::feed::Event::Disconnected { terminal: true, .. } => return,
                    zork_mesh::feed::Event::Disconnected { .. } => {}
                }
            }
            drop(feed);
            retry.wait().await;
        }
    });
    subscriptions.insert(id, zork_notify::Task(task));
}

pub async fn status(State(state): State<AppState>) -> Response {
    let Some(service) = state.mesh.get() else {
        let enabled = zork_config::load_config(&state.config.data_root)
            .is_ok_and(|config| config.mesh.enabled);
        return Json(json!({"enabled":enabled,"ready":false,"state":if enabled {"starting"} else {"disabled"}})).into_response();
    };
    let links = state.db.mesh_links().unwrap_or_default();
    let peers = service
        .peers
        .lock()
        .expect("mesh peers")
        .values()
        .cloned()
        .collect::<Vec<_>>();
    Json(json!({"enabled":true,"change_token":state.db.realtime.token(state.db.realtime.current().mesh),"origin":service.origin,"group":service.config().ok().and_then(|c|c.group),"peers":peers,"workspaces":service.config().unwrap_or_default().workspaces.iter().map(|w|&w.id).collect::<Vec<_>>(),"assignments":links})).into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DelegateRequest {
    command_id: String,
    expected_revision: i64,
    executor_origin: String,
    workspace_id: String,
    goal: String,
}

pub async fn delegate(
    State(state): State<AppState>,
    AxumPath(task_id): AxumPath<String>,
    Json(request): Json<DelegateRequest>,
) -> Response {
    let result = async {
        let service = state.mesh.get().context("mesh_disabled")?;
        service.peer(&request.executor_origin)?;
        let task = state.db.product_task(&task_id)?.context("task_not_found")?;
        let session_id = task.session_id.as_deref().context("task_has_no_runtime")?;
        let _guard = state.entries.lock_local_task(session_id).await;
        // Retry of an existing command is safe even if the local Agent is offline.
        if state.db.mesh_link(&request.command_id)?.is_none() {
            let statuses = crate::agent::session_statuses(&state.agent).await?;
            ensure!(
                statuses.get(session_id).is_some_and(|s| matches!(
                    s.as_str(),
                    "wait" | "finished" | "failed" | "cancelled"
                )),
                "task_run_active"
            );
        }
        let session = state
            .db
            .get_session_by_id(session_id)?
            .context("task session missing")?;
        let link = state.db.mesh_delegate(
            &Assignment {
                assignment_id: request.command_id,
                task_id,
                owner_origin: service.origin.clone(),
                executor_origin: request.executor_origin,
                workspace_id: request.workspace_id,
                goal: request.goal,
                worker: None,
            },
            &session,
            request.expected_revision,
        )?;
        // Repeated HTTP responses do not repeat the visible message. SSE is a
        // wakeup for the first local submit; reconnect reads the durable history.
        if task.mesh.is_none() {
            state
                .entries
                .publish_imported_message(&session.key, "user", &link.assignment.goal);
        }
        Ok::<_, anyhow::Error>(link)
    }
    .await;
    match result {
        Ok(link) => (StatusCode::ACCEPTED, Json(json!({"assignment":link}))).into_response(),
        Err(error) => (
            StatusCode::CONFLICT,
            Json(json!({"error":error.to_string()})),
        )
            .into_response(),
    }
}

async fn client_subscription(
    state: &AppState,
    peer: Peer,
    request: Value,
) -> Result<tokio::sync::mpsc::Receiver<Value>> {
    let envelope: Envelope = serde_json::from_value(request)?;
    ensure!(envelope.v == 1, "mesh_protocol_version");
    let RpcRequest::Subscribe { path, body } = envelope.request else {
        anyhow::bail!("invalid subscription")
    };
    let session = match path.split('/').collect::<Vec<_>>().as_slice() {
        ["", "v1", "im", "events"] => None,
        ["", "v1", "im", "sessions", id, "events"] if !id.is_empty() => Some((*id).to_owned()),
        ["", "v1", "client", "browser", "events"] => None,
        _ => anyhow::bail!("mesh_subscription_not_allowed"),
    };
    let granted = |state: &AppState| -> bool {
        zork_config::load_config(&state.config.data_root)
            .ok()
            .is_some_and(|c| c.mesh.peers.iter().any(|p| p.origin == peer.origin))
    };
    ensure!(granted(state), "mesh_client_not_granted");
    let policy = state.db.realtime.listen(crate::realtime::MESH);
    let source = if path.ends_with("/browser/events") {
        crate::browser::subscribe(
            state.clone(),
            serde_json::from_value(body.context("browser_registration_required")?)?,
        )?
    } else {
        ensure!(body.is_none(), "subscription_body_not_allowed");
        crate::desktop_events::subscribe(state.clone(), session).await?
    };
    let state = state.clone();
    let source = zork_notify::stream::guard(
        source,
        policy,
        move || {
            zork_config::load_config(&state.config.data_root)
                .ok()
                .is_some_and(|config| config.mesh.peers.iter().any(|p| p.origin == peer.origin))
        },
        16,
    );
    let (tx, rx) = tokio::sync::mpsc::channel(16);
    tokio::spawn(async move {
        let mut source = source;
        loop {
            let value = tokio::select! {
                _ = tx.closed() => return,
                value = source.recv() => match value { Some(value) => value, None => return },
            };
            if tx
                .send(json!({"v":1,"ok":true,"data":value}))
                .await
                .is_err()
            {
                return;
            }
        }
    });
    Ok(rx)
}

async fn mesh_subscription(
    state: &AppState,
    peer: Peer,
    request: Value,
) -> Result<tokio::sync::mpsc::Receiver<Value>> {
    let envelope: Envelope = serde_json::from_value(request.clone())?;
    ensure!(envelope.v == 1, "mesh_protocol_version");
    let topic = match envelope.request {
        RpcRequest::Subscribe { .. } => return client_subscription(state, peer, request).await,
        RpcRequest::Watch { topic } => topic,
        _ => anyhow::bail!("invalid subscription"),
    };
    if let WatchTopic::AgentMessages { epoch, after } = topic {
        return crate::channels::subscribe(state.clone(), peer.origin, epoch, after);
    }
    let flags = match &topic {
        WatchTopic::Peer | WatchTopic::Membership => crate::realtime::MESH,
        WatchTopic::AgentMessages { .. } => unreachable!(),
        WatchTopic::Assignment { .. } => {
            crate::realtime::MESH | crate::realtime::WORK | crate::realtime::ACTIVITY
        }
    };
    // Subscribe before the first authorization and snapshot read.
    let changes = state.db.realtime.listen(flags);
    let cursor = match &topic {
        WatchTopic::Assignment { after, .. } => {
            ensure!(*after >= 0, "invalid_mesh_cursor");
            *after
        }
        _ => 0,
    };
    let source = MeshWatch {
        state: state.clone(),
        peer,
        topic,
        cursor,
    };
    let mut latest = Value::Null;
    Ok(zork_notify::stream::spawn(
        source,
        changes,
        16,
        Some(zork_mesh::feed::HEARTBEAT),
        move |event| {
            use zork_notify::stream::Event;
            Some(match event {
                Event::Data(value) => {
                    latest = value.clone();
                    // Legacy assignment clients can receive keepalives too, without
                    // reapplying old journal pages. New feeds skip heartbeat frames.
                    if latest.get("events").is_some() {
                        latest["events"] = json!([]);
                    }
                    json!({"v":1,"ok":true,"data":value})
                }
                Event::Heartbeat => json!({"v":1,"ok":true,"heartbeat":true,"data":latest}),
                Event::Error(error) => json!({"v":1,"ok":false,"error":error.to_string()}),
            })
        },
    ))
}

struct MeshWatch {
    state: AppState,
    peer: Peer,
    topic: WatchTopic,
    cursor: i64,
}
impl MeshWatch {
    fn authorize(&self, config: &zork_config::FileConfig) -> Result<Option<Link>> {
        ensure!(
            config
                .mesh
                .peers
                .iter()
                .any(|peer| peer.origin == self.peer.origin),
            "mesh_peer_not_paired"
        );
        match &self.topic {
            WatchTopic::Assignment { assignment_id, .. } => Ok(Some(authorized_assignment(
                &self.state,
                &self.peer.origin,
                assignment_id,
            )?)),
            WatchTopic::Membership => {
                let group = config
                    .mesh
                    .group
                    .as_ref()
                    .context("mesh_membership_missing")?;
                let service = self.state.mesh.get().context("mesh_not_ready")?;
                ensure!(
                    group.authority == service.origin() && group.contains(&self.peer.origin),
                    "mesh_member_required"
                );
                Ok(None)
            }
            WatchTopic::Peer => Ok(None),
            WatchTopic::AgentMessages { .. } => unreachable!(),
        }
    }
}
impl zork_notify::stream::Source for MeshWatch {
    type Item = Value;
    type Error = anyhow::Error;
    fn check_access(&self) -> Result<()> {
        self.authorize(&zork_config::load_config(&self.state.config.data_root)?)
            .map(|_| ())
    }
    async fn read(&mut self) -> Result<zork_notify::stream::Page<Value>> {
        use zork_notify::stream::Page;
        let config = zork_config::load_config(&self.state.config.data_root)?;
        let link = self.authorize(&config)?;
        let _paired = config
            .mesh
            .peers
            .iter()
            .find(|p| p.origin == self.peer.origin)
            .context("mesh_peer_not_paired")?;
        Ok(match &self.topic {
            WatchTopic::AgentMessages { .. } => unreachable!(),
            WatchTopic::Peer => Page::snapshot(
                json!({"execute_workspaces":config.mesh.workspaces.iter().map(|w|w.id.clone()).collect::<Vec<_>>()}),
            ),
            WatchTopic::Membership => {
                let group = config.mesh.group.context("mesh_membership_missing")?;
                Page::snapshot(
                    json!({"group":group,"change_token":self.state.db.realtime.token(self.state.db.realtime.current().mesh)}),
                )
            }
            WatchTopic::Assignment { assignment_id, .. } => {
                let link = link.context("mesh_assignment_missing")?;
                let events = self
                    .state
                    .db
                    .mesh_events(assignment_id, self.cursor, true)?;
                let mut activity = link
                    .session_key
                    .as_deref()
                    .and_then(|key| self.state.entries.local_activity(key));
                if let (Some(activity), Some(target)) =
                    (activity.as_mut(), link.assignment.worker.as_ref())
                {
                    if let Some(worker) = self.state.db.node_agent(&target.worker_id)? {
                        activity["actor_name"] = json!(worker.name);
                        activity["actor_avatar"] = json!(worker.avatar);
                    }
                }
                let execution = if let Some(key) = link.session_key.as_deref() {
                    if self.state.entries.execution_snapshot(key).is_none() {
                        if let Some(session) = self.state.db.get_session(key)? {
                            if let Some(runtime) = session.id.as_deref() {
                                if self.state.agent.service.contains(runtime) {
                                    let snapshot =
                                        self.state.agent.session_snapshot(runtime).await?;
                                    self.state
                                        .entries
                                        .store_execution_snapshot(key, &snapshot)?;
                                }
                            }
                        }
                    }
                    self.state.entries.execution_snapshot(key)
                } else {
                    None
                };
                let more = !events.is_empty();
                let value = json!({"events":events,"activity":activity,"execution":execution});
                if more {
                    Page::chunk(value, true, false)
                } else {
                    Page::snapshot(value)
                }
            }
        })
    }
    fn delivered(&mut self, value: &Value) {
        if let Some(sequence) = value["events"]
            .as_array()
            .and_then(|events| events.last())
            .and_then(|event| event["sequence"].as_i64())
        {
            self.cursor = sequence;
        }
    }
}

#[cfg(test)]
mod native_routes_tests {
    #[test]
    fn paired_client_routes_allow_model_management_without_raw_agent_access() {
        for (method, path) in [
            ("GET", "/v1/node/info"),
            ("GET", "/v1/node/resources"),
            ("PUT", "/v1/node/name"),
            ("GET", "/v1/node/update"),
            ("POST", "/v1/node/update"),
            ("GET", "/v1/node/conversations/read-markers"),
            ("GET", "/v1/node/profiles/p"),
            ("PUT", "/v1/node/profiles/p/models"),
            ("PUT", "/v1/node/profiles/p/models/enabled"),
            ("POST", "/v1/node/profiles/p/models/refresh"),
            ("PUT", "/v1/node/profiles/p/name"),
            ("POST", "/v1/node/profiles/p/refresh"),
            ("PATCH", "/v1/node/agents/a/model"),
            ("POST", "/v1/node/chats/c/archive"),
            ("POST", "/v1/node/chats/c/messages/m/agent-configuration"),
            ("GET", "/v1/node/chats/c/messages/m/provider-login"),
            ("POST", "/v1/node/chats/c/messages/m/provider-login"),
            ("DELETE", "/v1/node/chats/c/messages/m/provider-login"),
        ] {
            assert!(super::client_route(method, path), "{method} {path}");
        }
        for (method, path) in [
            ("PATCH", "/v1/node/agents/a/grants"),
            ("POST", "/v1/node/resources"),
            ("DELETE", "/v1/node/resources"),
            ("GET", "/profiles/p"),
            ("GET", "/v1/node/profiles/p/auth"),
            ("GET", "/v1/node/profiles/p/refresh"),
            ("GET", "/v1/node/profiles/p/models/enabled"),
            ("PUT", "/v1/node/profiles/p/models/refresh"),
            ("PUT", "/v1/node/profiles/../models/enabled"),
            ("POST", "/v1/node/profiles/../refresh"),
            ("PUT", "/v1/node/profiles/../models"),
            ("POST", "/v1/node/profiles/p/name"),
            ("PUT", "/v1/node/profiles/../name"),
            ("GET", "/v1/node/chats/c/archive"),
            ("POST", "/v1/node/chats/c/archive/extra"),
            ("GET", "/v1/node/chats/c/messages/m/agent-configuration"),
            ("POST", "/v1/node/chats/c/messages/m/respond"),
            ("GET", "/v1/node/chats/c/messages/m/private"),
            (
                "POST",
                "/v1/node/chats/c/messages/m/agent-configuration/extra",
            ),
            ("POST", "/v1/node/chats/../messages/m/agent-configuration"),
        ] {
            assert!(!super::client_route(method, path), "{method} {path}");
        }
    }
}
