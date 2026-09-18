//! Station-owned MCP services and durable Mesh invocation routing.
mod api;
mod calls;
mod management;
pub use api::{
    admin_create, admin_get, admin_list, admin_probe, admin_remove, admin_update, interrupt, tool,
};
use calls::execute;
mod runtime;
mod store;
#[cfg(test)]
mod tests;
use crate::state::AppState;
use anyhow::{ensure, Context, Result};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use futures_util::{stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{watch, Mutex as AsyncMutex, Semaphore};

fn new_id() -> String {
    ulid::Ulid::new().to_string()
}
fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
fn valid_id(id: &str) -> Result<()> {
    ensure!(
        id.len() == 26 && ulid::Ulid::from_string(id).is_ok_and(|u| u.to_string() == id),
        "mcp_invalid_id"
    );
    Ok(())
}
fn digest(value: &impl Serialize) -> Result<String> {
    fn canonical(value: Value) -> Value {
        match value {
            Value::Object(object) => Value::Object(
                object
                    .into_iter()
                    .map(|(k, v)| (k, canonical(v)))
                    .collect::<std::collections::BTreeMap<_, _>>()
                    .into_iter()
                    .collect(),
            ),
            Value::Array(items) => Value::Array(items.into_iter().map(canonical).collect()),
            other => other,
        }
    }
    Ok(
        blake3::hash(&serde_json::to_vec(&canonical(serde_json::to_value(
            value,
        )?))?)
        .to_hex()
        .to_string(),
    )
}

pub use crate::node_access::Subject;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerInput {
    name: String,
    #[serde(default)]
    description: String,
    transport: runtime::Transport,
    #[serde(default, rename = "grant", skip_serializing)]
    _grant: Value,
    /// None grants all tools; an empty list grants none.
    #[serde(default)]
    tool_allowlist: Option<Vec<String>>,
    #[serde(default = "enabled")]
    enabled: bool,
}
fn enabled() -> bool {
    true
}
impl ServerInput {
    fn validate(&self) -> Result<()> {
        ensure!(
            !self.name.trim().is_empty() && self.name.len() <= 80 && self.description.len() <= 2048,
            "mcp_invalid_name"
        );
        ensure!(
            serde_json::to_vec(self)?.len() <= 32 * 1024,
            "mcp_config_limit"
        );
        self.transport.validate()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Server {
    id: String,
    revision: String,
    #[serde(flatten)]
    config: ServerInput,
}
impl Server {
    fn permission(&self, _who: &Subject, _local: bool, tool: Option<&str>) -> Result<()> {
        ensure!(
            tool.is_none_or(|t| self
                .config
                .tool_allowlist
                .as_ref()
                .is_none_or(|list| list.iter().any(|v| v == t))),
            "mcp_tool_not_allowed"
        );
        Ok(())
    }
    fn access(&self, who: &Subject, local: bool, tool: Option<&str>) -> Result<()> {
        self.permission(who, local, tool)?;
        ensure!(self.config.enabled, "mcp_disabled");
        Ok(())
    }
    fn allows(&self, who: &Subject, local: bool, tool: Option<&str>) -> bool {
        self.access(who, local, tool).is_ok()
    }
    fn descriptor(&self, origin: &str) -> Value {
        json!({"server_ref":{"owner_origin":origin,"server_id":self.id},"name":self.config.name,"description":self.config.description,"config_revision":self.revision,"availability":if self.config.enabled {"unprobed"}else{"disabled"}})
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerRef {
    owner_origin: String,
    server_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operation {
    op: String,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    server_ref: Option<ServerRef>,
    #[serde(default)]
    tool: Option<String>,
    #[serde(default)]
    binding_revision: Option<String>,
    #[serde(default)]
    arguments: Option<Value>,
    #[serde(default)]
    call_id: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    config: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    grant: Option<Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolRequest {
    session_id: String,
    invocation_id: String,
    request: Operation,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rpc {
    #[serde(default)]
    interrupt: bool,
    pub subject: Subject,
    pub invocation_id: String,
    request: Operation,
}
struct Session {
    connection: AsyncMutex<Option<runtime::Client>>,
    used: Mutex<Instant>,
}
struct Active {
    server: String,
    stop: watch::Sender<bool>,
}
pub struct Hub {
    store: store::Store,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    active: Mutex<HashMap<String, Active>>,
    slots: Arc<Semaphore>,
    health: Mutex<HashMap<String, String>>,
    policy: Mutex<()>,
}
impl Hub {
    pub fn open(root: &std::path::Path) -> Result<Self> {
        Ok(Self {
            store: store::Store::open(root)?,
            sessions: Mutex::new(HashMap::new()),
            active: Mutex::new(HashMap::new()),
            slots: Arc::new(Semaphore::new(16)),
            health: Mutex::new(HashMap::new()),
            policy: Mutex::new(()),
        })
    }
    pub fn reap_idle(&self) {
        self.sessions.lock().expect("mcp sessions").retain(|_, s| {
            Arc::strong_count(s) > 1
                || s.used.lock().expect("mcp used").elapsed() < Duration::from_secs(300)
        });
    }
    fn descriptor(&self, server: &Server, origin: &str) -> Value {
        let mut value = server.descriptor(origin);
        if server.config.enabled {
            if let Some(health) = self.health.lock().expect("mcp health").get(&server.id) {
                value["availability"] = json!(health);
            }
        }
        value
    }
    fn healthy(&self, id: &str, result: &Result<Value>) {
        let health = match result {
            Ok(_) => "ready",
            Err(e) if safe_error(e) == "mcp_auth_required" => "auth_required",
            Err(_) => "failed",
        };
        self.health
            .lock()
            .expect("mcp health")
            .insert(id.into(), health.into());
    }
    pub fn shutdown(&self) {
        self.slots.close();
        for (_, task) in self.active.lock().expect("mcp active").drain() {
            task.stop.send_replace(true);
        }
        self.sessions.lock().expect("mcp sessions").clear();
    }
    fn invalidate(&self, server: &str) {
        self.health.lock().expect("mcp health").remove(server);
        for task in self
            .active
            .lock()
            .expect("mcp active")
            .values()
            .filter(|a| a.server == server)
        {
            task.stop.send_replace(true);
        }
        self.sessions
            .lock()
            .expect("mcp sessions")
            .retain(|key, _| !key.starts_with(&format!("{server}:")));
    }
    fn connection(&self, server: &Server, who: &Subject) -> Result<Arc<Session>> {
        let key = format!("{}:{}:{}", server.id, server.revision, digest(who)?);
        let mut sessions = self.sessions.lock().expect("mcp sessions");
        sessions.retain(|_, s| {
            Arc::strong_count(s) > 1
                || s.used.lock().expect("mcp used").elapsed() < Duration::from_secs(300)
        });
        if let Some(s) = sessions.get(&key) {
            return Ok(s.clone());
        }
        if sessions.len() >= 32 {
            let idle = sessions
                .iter()
                .filter(|(_, s)| Arc::strong_count(s) == 1)
                .min_by_key(|(_, s)| *s.used.lock().expect("mcp used"))
                .map(|(k, _)| k.clone());
            if let Some(key) = idle {
                sessions.remove(&key);
            }
        }
        ensure!(sessions.len() < 32, "mcp_session_limit");
        let s = Arc::new(Session {
            connection: AsyncMutex::new(None),
            used: Mutex::new(Instant::now()),
        });
        sessions.insert(key, s.clone());
        Ok(s)
    }
}
fn identity_ready(state: &AppState) -> Result<()> {
    ensure!(
        !zork_config::load_config(&state.config.data_root)?
            .mesh
            .enabled
            || state.mesh.get().is_some(),
        "mcp_mesh_starting"
    );
    Ok(())
}
fn own_origin(state: &AppState) -> String {
    crate::node_access::identity(state)
}

fn check_permission(
    state: &AppState,
    who: &Subject,
    local: bool,
    id: &str,
    tool: Option<&str>,
) -> Result<Server> {
    if !local {
        ensure!(
            zork_config::load_config(&state.config.data_root)?
                .mesh
                .peers
                .iter()
                .any(|p| p.origin == who.origin),
            "mcp_access_denied"
        );
    }
    let server = state.mcp.store.server(id)?;
    server.permission(who, local, tool)?;
    Ok(server)
}
fn check(
    state: &AppState,
    who: &Subject,
    local: bool,
    id: &str,
    tool: Option<&str>,
) -> Result<Server> {
    let server = check_permission(state, who, local, id, tool)?;
    ensure!(server.config.enabled, "mcp_disabled");
    Ok(server)
}
fn field<'a>(value: &'a Option<String>) -> Result<&'a str> {
    value
        .as_deref()
        .filter(|s| !s.is_empty() && s.len() <= 256)
        .context("mcp_missing_parameter")
}
fn response(value: Result<Value>) -> Response {
    match value {
        Ok(v) => Json(v).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":safe_error(&e),"delivery_rejected":e.is::<crate::tool_stream::Rejected>()})),
        )
            .into_response(),
    }
}
fn safe_error(e: &anyhow::Error) -> String {
    let message = e.to_string();
    if message.starts_with("mcp_") && message.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') {
        message
    } else {
        "mcp_operation_failed".into()
    }
}
async fn route(state: &AppState, owner: &str, mut rpc: Rpc) -> Result<Value> {
    if owner == "local" || owner == own_origin(state) {
        return execute(state, rpc, true).await;
    }
    ensure!(
        zork_config::load_config(&state.config.data_root)?
            .mesh
            .peers
            .iter()
            .any(|p| p.origin == owner),
        "mcp_access_denied"
    );
    rpc.subject.origin = own_origin(state);
    state
        .mesh
        .get()
        .context("mcp_mesh_unavailable")?
        .mcp_call(owner, rpc)
        .await
}
pub async fn remote(state: &AppState, origin: &str, mut rpc: Rpc) -> Result<Value> {
    ensure!(
        rpc.subject.origin == origin
            && !rpc.subject.agent.is_empty()
            && rpc.subject.agent.len() <= 256
            && !rpc.subject.session.is_empty()
            && rpc.subject.session.len() <= 256
            && !rpc.invocation_id.is_empty()
            && rpc.invocation_id.len() <= 256,
        "mcp_invalid_subject"
    );
    rpc.subject.origin = origin.into();
    execute(state, rpc, false).await
}
async fn search(state: &AppState, who: &Subject, request: &Operation) -> Result<Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let config = zork_config::load_config(&state.config.data_root)?;
    let mut owners = vec![own_origin(state)];
    if state.mesh.get().is_some() {
        owners.extend(config.mesh.peers.iter().map(|p| p.origin.clone()));
    }
    owners.sort();
    owners.dedup();
    owners.retain(|o| {
        request
            .owner
            .as_ref()
            .is_none_or(|v| v == o || (v == "local" && *o == own_origin(state)))
    });
    let mut query = request.clone();
    query.op = "catalog".into();
    query.cursor = None;
    let replies = stream::iter(owners.into_iter().map(|owner| {
        let mut rpc = Rpc {
            interrupt: false,
            subject: who.clone(),
            invocation_id: "catalog".into(),
            request: query.clone(),
        };
        async move {
            let fetch = async {
                let mut all = Vec::new();
                for _ in 0..8 {
                    let value = route(state, &owner, rpc.clone()).await?;
                    let items = value["items"].as_array().context("mcp_invalid_catalog")?;
                    ensure!(
                        items
                            .iter()
                            .all(|v| v["server_ref"]["owner_origin"] == owner),
                        "mcp_wrong_owner"
                    );
                    all.extend(items.clone());
                    rpc.request.cursor = value["next_cursor"].as_str().map(str::to_owned);
                    if rpc.request.cursor.is_none() {
                        return Ok::<_, anyhow::Error>(json!({"items":all}));
                    }
                }
                anyhow::bail!("mcp_catalog_limit")
            };
            (
                owner.clone(),
                tokio::time::timeout_at(deadline, fetch).await,
            )
        }
    }))
    .buffer_unordered(4)
    .collect::<Vec<_>>()
    .await;
    let mut items = Vec::new();
    let mut unavailable = Vec::new();
    for (owner, reply) in replies {
        match reply {
            Ok(Ok(value)) => {
                if let Some(list) = value["items"].as_array() {
                    items.extend(list.clone());
                }
            }
            _ => unavailable.push(owner),
        }
    }
    items.sort_by_key(|v| v["server_ref"].to_string());
    let mut page = page(items, &request.cursor)?;
    page["partial"] = json!(!unavailable.is_empty());
    page["unavailable_nodes"] = json!(unavailable);
    Ok(page)
}
fn page(items: Vec<Value>, cursor: &Option<String>) -> Result<Value> {
    let revision = digest(&items)?;
    let start = if let Some(cursor) = cursor {
        let (rev, start) = cursor.split_once(':').context("mcp_invalid_cursor")?;
        ensure!(rev == revision, "mcp_cursor_expired");
        start
            .parse::<usize>()
            .map_err(|_| anyhow::anyhow!("mcp_invalid_cursor"))?
    } else {
        0
    };
    ensure!(start <= items.len(), "mcp_invalid_cursor");
    let mut selected = Vec::new();
    let mut size = 0;
    for item in items.iter().skip(start).take(20) {
        let bytes = serde_json::to_vec(item)?.len();
        ensure!(bytes <= 96 * 1024, "mcp_definition_limit");
        if size + bytes > 128 * 1024 {
            break;
        }
        size += bytes;
        selected.push(item.clone());
    }
    let end = start + selected.len();
    Ok(
        json!({"items":selected,"revision":revision,"next_cursor":if end<items.len(){Some(format!("{revision}:{end}"))}else{None}}),
    )
}

pub(crate) fn watch_owner(state: &AppState, who: &Subject, id: &str) -> Result<String> {
    state.mcp.store.owner(who, id)
}
pub(crate) fn changes(state: &AppState) -> zork_notify::Changes {
    state.mcp.store.subscribe()
}
pub(crate) fn client_resources(
    state: &AppState,
) -> Result<Vec<zork_client_types::resources::Resource>> {
    use zork_client_types::resources::{Resource, ResourceKind};
    state
        .mcp
        .store
        .servers()?
        .into_iter()
        .map(|server| {
            let descriptor = state.mcp.descriptor(&server, &own_origin(state));
            let mut item = Resource::new(
                ResourceKind::Mcp,
                server.id,
                server.config.name,
                descriptor["availability"]
                    .as_str()
                    .unwrap_or("unprobed")
                    .into(),
                "mesh".into(),
            );
            item.description = server.config.description;
            item.revision = Some(server.revision);
            item.tool_allowlist = server.config.tool_allowlist;
            Ok(item)
        })
        .collect()
}
pub(crate) async fn client_details(
    state: &AppState,
    id: &str,
) -> Result<zork_client_types::resources::ResourceDetails> {
    use zork_client_types::resources::{ResourceDetails, ResourceTool};
    let server = state.mcp.store.server(id)?;
    let descriptor = state.mcp.descriptor(&server, &own_origin(state));
    let mut details = ResourceDetails {
        title: server.config.name.clone(),
        description: server.config.description.clone(),
        facts: vec![
            (
                "status".into(),
                descriptor["availability"]
                    .as_str()
                    .unwrap_or("unprobed")
                    .into(),
            ),
            ("protocol".into(), "MCP".into()),
        ],
        ..Default::default()
    };
    if !server.config.enabled {
        return Ok(details);
    }
    let who = Subject {
        origin: "local".into(),
        agent: "admin".into(),
        session: "resource-inspection".into(),
    };
    let session = state.mcp.connection(&server, &who)?;
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        let _slot = state.mcp.slots.clone().acquire_owned().await?;
        let mut connection = session.connection.lock().await;
        let mut client = match connection.take() {
            Some(client) => client,
            None => runtime::Client::connect(&server.config.transport).await?,
        };
        let tools = client.list_tools().await?;
        let current = state.mcp.store.server(id)?;
        ensure!(
            current.revision == server.revision && current.config.enabled,
            "mcp_definition_changed"
        );
        *connection = Some(client);
        *session.used.lock().unwrap() = Instant::now();
        Ok::<_, anyhow::Error>(tools)
    })
    .await;
    match result {
        Ok(Ok(tools)) => {
            state.mcp.healthy(id, &Ok(Value::Null));
            details.tools = tools
                .into_iter()
                .filter(|tool| {
                    tool["name"].as_str().is_some_and(|name| {
                        server
                            .config
                            .tool_allowlist
                            .as_ref()
                            .is_none_or(|allow| allow.iter().any(|t| t == name))
                    })
                })
                .map(|tool| ResourceTool {
                    name: tool["name"].as_str().unwrap().into(),
                    description: tool["description"].as_str().unwrap_or_default().into(),
                    input_schema: tool["inputSchema"].clone(),
                })
                .collect();
            details.facts[0].1 = "ready".into();
        }
        Ok(Err(error)) => {
            details
                .facts
                .push(("inspection_error".into(), safe_error(&error)));
            state.mcp.healthy(id, &Err(error));
        }
        Err(_) => {
            details
                .facts
                .push(("inspection_error".into(), "mcp_inspection_timeout".into()));
            state
                .mcp
                .healthy(id, &Err(anyhow::anyhow!("mcp_inspection_timeout")));
        }
    }
    details.facts[0].1 = state.mcp.descriptor(&server, &own_origin(state))["availability"]
        .as_str()
        .unwrap_or("unprobed")
        .into();
    Ok(details)
}
pub(crate) fn authorize_snapshot(
    state: &AppState,
    who: &Subject,
    id: &str,
    local: bool,
) -> Result<()> {
    let (server, tool) = state.mcp.store.call_access(id, who)?;
    check_permission(state, who, local, &server, Some(&tool))?;
    Ok(())
}
pub(crate) async fn snapshot(
    state: &AppState,
    who: &Subject,
    id: &str,
    _offset: u64,
    local: bool,
) -> Result<Value> {
    let (server, status, result) = state.mcp.store.call(id, who)?;
    let tool = state.mcp.store.call_tool(id)?;
    // Disabling prevents new work, not delivery of the original caller's
    // completion. Membership and invocation ownership still apply.
    check_permission(state, who, local, &server, Some(&tool))?;
    Ok(
        json!({"operation_id":id,"call_id":id,"state":status,"result":result.map(|text|serde_json::from_str::<Value>(&text)).transpose()?}),
    )
}
