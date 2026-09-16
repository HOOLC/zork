//! HTTP/SSE client for the gateway-owned local IM entry.
//!
//! Visible `message` events exist only after gateway ingress or an explicit
//! Agent `chat.post_message`. `status` events carry non-message activity.
//! Durable execution records are available through the separate `/history`
//! inspector endpoint; they never become delivered conversation messages.

#[cfg(feature = "headless-bench")]
#[path = "api/offline.rs"]
mod offline;

use bytes::Bytes;
use futures_channel::mpsc;
use futures_util::{SinkExt, Stream, StreamExt};
use serde::{Deserialize, Serialize};
use thiserror::Error;
pub use zork_config::{ContextConfig, ContextStrategy, MeshConfig};
pub use zork_mesh::content_root;
pub use zork_mesh::route::{ConnectionRoute, ConnectionScope};

/// Roles deliberately delivered through the IM gateway.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
}

impl Role {
    pub fn label(self) -> &'static str {
        match self {
            Role::User => "you",
            Role::Assistant => "agent",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Wait,
    Working,
}

impl SessionStatus {
    pub fn label(self) -> &'static str {
        match self {
            SessionStatus::Wait => "wait",
            SessionStatus::Working => "working",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionSummary {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub kind: String,
    pub session_id: String,
    #[serde(default)]
    pub can_send: Option<bool>,
    pub profile_id: String,
    pub model: String,
    pub thinking: String,
    pub workspace: String,
    pub status: SessionStatus,
    #[serde(default)]
    pub runtime_available: bool,
    #[serde(default)]
    pub task: Option<ProductTask>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Open,
    Review,
    Completed,
    Cancelled,
}
impl TaskState {
    pub fn is_closed(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProductTask {
    pub task_id: String,
    pub session_id: Option<String>,
    pub conversation_id: String,
    pub title: String,
    #[serde(default)]
    pub goal: String,
    pub workspace: String,
    pub state: TaskState,
    pub revision: i64,
    pub result_message_id: Option<String>,
    pub result_text: Option<String>,
    pub last_run_status: Option<String>,
    pub run_count: i64,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub mesh: Option<MeshTask>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeshTask {
    pub role: String,
    pub assignment_id: String,
    pub state: String,
    pub error: Option<String>,
    pub owner_origin: String,
    pub executor_origin: String,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MeshStatus {
    pub enabled: bool,
    /// Opaque owner/epoch hint for Mesh administration, including invitations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_token: Option<String>,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub group: Option<zork_config::membership::MeshGroup>,
    #[serde(default)]
    pub peers: Vec<MeshPeer>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeshPeer {
    pub origin: String,
    pub name: String,
    pub online: bool,
    #[serde(default)]
    pub last_seen_at: Option<String>,
    pub execute_workspaces: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TaskRun {
    pub run_id: String,
    pub agent_session_id: String,
    pub turn_id: String,
    pub status: String,
    pub started_at_ms: i64,
    pub finished_at_ms: Option<i64>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TaskDetail {
    pub task: ProductTask,
    pub runs: Vec<TaskRun>,
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    pub artifact_id: String,
    pub task_id: Option<String>,
    pub session_id: Option<String>,
    pub task_title: String,
    pub workspace: String,
    pub name: String,
    pub source_path: String,
    pub media_type: String,
    pub caption: Option<String>,
    pub byte_len: i64,
    pub version: i64,
    pub created_at: String,
}
impl Artifact {
    pub fn collection_id(&self) -> &str {
        self.session_id
            .as_deref()
            .or(self.task_id.as_deref())
            .unwrap_or(&self.artifact_id)
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskAction {
    Accept,
    Reopen,
    Cancel,
}

/// Optional identity supplied by the Gateway, never inferred from the current viewer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_epoch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_sequence: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_avatar: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_kind: Option<zork_client_types::chat::AuthorKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction: Option<Box<serde_json::Value>>,
    /// Client-only derived state. The source request and result stay immutable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_result: Option<Box<zork_client_types::interaction::Resolution>>,
    #[serde(skip)]
    pub interaction_view: Option<Box<crate::interactions::Card>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationReadMarker {
    pub session_id: String,
    pub last_message_id: String,
    pub created_at: String,
    pub role: Role,
}

/// One item of a gateway `/messages` page or SSE `message` event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TranscriptMessage {
    Message {
        role: Role,
        content: String,
        #[serde(flatten)]
        metadata: MessageMetadata,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MessagePage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_epoch: Option<String>,
    pub items: Vec<TranscriptMessage>,
    pub older_cursor: Option<String>,
}

mod profile_types;
pub use profile_types::{ProfileInfo, ProfileModel, ProfileQuota};

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("request error: {0}")]
    Request(#[from] reqwest::Error),
    #[error("api error {status}: {message}")]
    Api { status: u16, message: String },
    #[error("gateway task failed: {0}")]
    Task(std::io::Error),
}

impl ApiError {
    pub fn access_revoked(&self) -> bool {
        matches!(self, Self::Api { status: 401, .. })
            || matches!(self,Self::Api{status:403,message} if matches!(message.as_str(),"mesh_peer_not_paired"|"mesh_client_not_granted"|"device_removed_from_mesh"))
    }
    pub fn status(&self) -> Option<u16> {
        match self {
            ApiError::Api { status, .. } => Some(*status),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct SseEvent {
    pub name: String,
    pub data: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicToolCall {
    pub tool_call_id: String,
    pub tool_name: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub labels: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub action: String,
}

impl PublicToolCall {
    pub fn activity_label(&self, locale: &str) -> String {
        if !self.action.is_empty() {
            return self.action.clone();
        }
        let action = self
            .labels
            .get(locale)
            .filter(|label| !label.is_empty())
            .or_else(|| self.labels.get("en").filter(|label| !label.is_empty()))
            .map(String::as_str)
            .unwrap_or(if locale == "zh-CN" {
                "执行操作"
            } else {
                "Working"
            });
        if self.detail.is_empty() {
            action.to_owned()
        } else {
            format!("{action} {}", self.detail)
        }
    }
}

/// Rich Agent activity projected by the gateway's `status` SSE events.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AgentStatus {
    Live {
        presentation: crate::activity::Presentation,
    },
    Clear,
    Thinking,
    ToolsStarted {
        calls: Vec<PublicToolCall>,
        #[serde(default)]
        thinking: bool,
    },
    ToolFinished {
        tool_call_id: String,
    },
    ToolsWaiting {
        calls: Vec<PublicToolCall>,
        deadline_ms: i64,
    },
    Waiting {
        reason: String,
        deadline_ms: i64,
    },
    Failed {
        reason: String,
    },
    Finished,
    Interrupted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Read-only composer membership. `assigned` is a work allocation fact;
/// it does not assert that the Agent has authored a message.
pub struct ParticipantStatus {
    pub id: String,
    pub name: String,
    pub avatar: Option<String>,
    #[serde(default)]
    pub subscribed: bool,
    #[serde(default)]
    pub assigned: bool,
    #[serde(default)]
    pub session_id: String,
    pub activity: Option<AgentStatus>,
}

/// HTTP client for the gateway IM API.
///
/// All requests run on a dedicated Tokio runtime because reqwest's
/// connection pool requires a Tokio reactor, while the GUI's GPUI
/// background executor is not Tokio.
#[derive(Clone)]
struct Transport {
    http: reqwest::Client,
    mesh: Option<(zork_mesh::node::MeshNode, String)>,
}
pub struct GatewayClient {
    #[cfg(feature = "headless-bench")]
    fixture: Option<std::sync::Arc<offline::Fixture>>,
    client_id: String,
    browser_generation: std::sync::atomic::AtomicU64,
    service_mesh: Option<zork_mesh::node::MeshNode>,
    http: Transport,
    sse_http: Transport,
    pub base_url: String,
    token: Option<String>,
    rt: ClientRuntime,
    pub(crate) delivery_gate: tokio::sync::Mutex<()>,
}

// GPUI needs its own reactor; JNI already owns one. Runtime shutdown must
// never block inside an async caller (including dropping the last Arc).
enum ClientRuntime {
    Owned(Option<tokio::runtime::Runtime>),
    Borrowed(tokio::runtime::Handle),
}
impl ClientRuntime {
    fn handle(&self) -> &tokio::runtime::Handle {
        match self {
            Self::Owned(rt) => rt.as_ref().expect("runtime alive").handle(),
            Self::Borrowed(handle) => handle,
        }
    }
    fn spawn<F>(&self, future: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.handle().spawn(future)
    }
}
impl Drop for ClientRuntime {
    fn drop(&mut self) {
        if let Self::Owned(runtime) = self {
            if let Some(runtime) = runtime.take() {
                runtime.shutdown_background();
            }
        }
    }
}

fn join_task_error(err: tokio::task::JoinError) -> ApiError {
    ApiError::Task(std::io::Error::other(err.to_string()))
}

async fn send_request(
    http: &Transport,
    base_url: &str,
    token: &Option<String>,
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<reqwest::Response, ApiError> {
    if let Some((control, origin)) = &http.mesh {
        let response=control.exchange(origin,&serde_json::json!({"v":1,"request":{"kind":"client","method":method.as_str(),"path":path,"body":body}})).await.map_err(|e|ApiError::Task(std::io::Error::other(format!("{e:#}"))))?;
        if response["v"] != 1 || response["ok"] != true {
            return Err(ApiError::Api {
                status: 403,
                message: response["error"]
                    .as_str()
                    .unwrap_or("Mesh client request failed")
                    .into(),
            });
        }
        let data = &response["data"];
        let status = data["status"]
            .as_u64()
            .filter(|n| *n >= 100 && *n <= 599)
            .ok_or_else(|| ApiError::Task(std::io::Error::other("invalid Mesh response status")))?
            as u16;
        if !(200..300).contains(&status) {
            return Err(ApiError::Api {
                status,
                message: data["body"]["error"]
                    .as_str()
                    .unwrap_or("Remote request failed")
                    .into(),
            });
        }
        let bytes = if let Some(object) = data.get("object") {
            let object: zork_mesh::node::ObjectRef = serde_json::from_value(object.clone())
                .map_err(|e| ApiError::Task(std::io::Error::other(e)))?;
            if object.origin != *origin {
                return Err(ApiError::Task(std::io::Error::other(
                    "Remote object origin mismatch",
                )));
            }
            control
                .read(&object)
                .await
                .map_err(|e| ApiError::Task(std::io::Error::other(format!("{e:#}"))))?
        } else {
            serde_json::to_vec(&data["body"])
                .map_err(|e| ApiError::Task(std::io::Error::other(e)))?
        };
        return Ok(reqwest::Response::from(
            http::Response::builder()
                .status(status)
                .body(bytes)
                .expect("validated Mesh response"),
        ));
    }
    let mut request = http.http.request(method, format!("{base_url}{path}"));
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await?;
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status().as_u16();
    let message = response
        .json::<serde_json::Value>()
        .await
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .or_else(|| value.get("error"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| format!("status {status}"));
    Err(ApiError::Api { status, message })
}

pub(crate) type ClientTask = tokio::task::JoinHandle<()>;

impl GatewayClient {
    #[cfg(feature = "headless-bench")]
    pub fn fixture(data: serde_json::Value, providers: serde_json::Value) -> Self {
        let mut client = Self::new(format!("fixture://{}", ulid::Ulid::new()), None);
        client.fixture = Some(std::sync::Arc::new(offline::Fixture::new(data, providers)));
        client
    }

    pub(crate) async fn wait(&self, duration: std::time::Duration) {
        let _ = self
            .spawn(async move { tokio::time::sleep(duration).await })
            .await;
    }
    pub(crate) fn same_connection(&self, other: &Self) -> bool {
        self.base_url == other.base_url
            && self.token == other.token
            && self.is_mesh() == other.is_mesh()
    }
    pub(crate) fn client_id(&self) -> &str {
        &self.client_id
    }
    #[cfg(feature = "desktop")]
    pub(crate) fn next_browser_generation(&self) -> u64 {
        self.browser_generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
            + 1
    }
    pub fn new(base_url: impl Into<String>, token: Option<String>) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("reqwest client");
        // SSE is intentionally not given a total request timeout. Connection
        // establishment is bounded, but a healthy event stream is long-lived.
        let sse_http = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .build()
            .expect("reqwest SSE client");
        let rt = ClientRuntime::Owned(Some(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("zork-client-http")
                .enable_io()
                .enable_time()
                .build()
                .expect("failed to build tokio runtime"),
        ));
        Self {
            #[cfg(feature = "headless-bench")]
            fixture: None,
            client_id: ulid::Ulid::new().to_string(),
            browser_generation: std::sync::atomic::AtomicU64::new(0),
            service_mesh: None,
            http: Transport { http, mesh: None },
            sse_http: Transport {
                http: sse_http,
                mesh: None,
            },
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            token,
            rt,
            delivery_gate: tokio::sync::Mutex::new(()),
        }
    }

    pub fn new_mesh(control: zork_mesh::node::MeshNode, origin: String) -> Self {
        let mut client = Self::new(format!("mesh:{origin}"), None);
        client.http.mesh = Some((control.clone(), origin.clone()));
        client.sse_http.mesh = Some((control, origin));
        client
    }
    /// Use the platform-owned Tokio runtime without creating per-request threads.
    pub fn mesh_on(
        control: zork_mesh::node::MeshNode,
        origin: String,
        handle: tokio::runtime::Handle,
    ) -> Self {
        let http = reqwest::Client::new();
        let mesh = Some((control, origin.clone()));
        Self {
            #[cfg(feature = "headless-bench")]
            fixture: None,
            client_id: ulid::Ulid::new().to_string(),
            browser_generation: std::sync::atomic::AtomicU64::new(0),
            service_mesh: None,
            http: Transport {
                http: http.clone(),
                mesh: mesh.clone(),
            },
            sse_http: Transport { http, mesh },
            base_url: format!("mesh:{origin}"),
            token: None,
            rt: ClientRuntime::Borrowed(handle),
            delivery_gate: tokio::sync::Mutex::new(()),
        }
    }
    pub fn with_service_mesh(mut self, node: zork_mesh::node::MeshNode) -> Self {
        self.service_mesh = Some(node);
        self
    }
    pub(crate) fn file_tree(&self) -> Option<zork_mesh::node::MeshNode> {
        self.service_mesh
            .clone()
            .or_else(|| self.http.mesh.as_ref().map(|(node, _)| node.clone()))
    }
    pub async fn open_shared_service(
        &self,
        url: &str,
    ) -> anyhow::Result<zork_mesh::services::LocalService> {
        let link = zork_mesh::services::ServiceLink::parse(url)?;
        let node = self
            .service_mesh
            .clone()
            .or_else(|| self.http.mesh.as_ref().map(|(node, _)| node.clone()))
            .ok_or_else(|| anyhow::anyhow!("Mesh 尚未连接"))?;
        let task = self
            .rt
            .spawn(async move { zork_mesh::services::LocalService::open(node, &link).await });
        let _guard = AbortOnDrop(Some(task.abort_handle()));
        task.await?
    }
    pub(crate) fn spawn<F>(&self, future: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.rt.spawn(future)
    }
    pub fn authenticated_mesh_origin(&self) -> Option<&str> {
        self.http.mesh.as_ref().map(|(_, origin)| origin.as_str())
    }
    pub fn is_mesh(&self) -> bool {
        self.http.mesh.is_some()
    }
    /// Runs `fut` on the dedicated runtime; the future must own all its state.
    async fn run_on<T, F>(&self, fut: F) -> Result<T, ApiError>
    where
        F: std::future::Future<Output = Result<T, ApiError>> + Send + 'static,
        T: Send + 'static,
    {
        let task = self.rt.spawn(fut);
        let _guard = AbortOnDrop(Some(task.abort_handle()));
        task.await.map_err(join_task_error)?
    }

    pub async fn node_request(
        &self,
        method: reqwest::Method,
        path: String,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, ApiError> {
        #[cfg(feature = "headless-bench")]
        if let Some(fixture) = &self.fixture {
            return fixture.node_request(method, path, body).map_err(|error| ApiError::Api { status: 400, message: error.to_string() });
        }
        let http = self.http.clone();
        let base_url = self.base_url.clone();
        let token = self.token.clone();
        self.run_on(async move {
            let response = send_request(&http, &base_url, &token, method, &path, body).await?;
            if response.status() == reqwest::StatusCode::NO_CONTENT {
                return Ok(serde_json::Value::Null);
            }
            Ok(response.json().await?)
        })
        .await
    }

    pub async fn list_sessions(&self) -> Result<Vec<SessionSummary>, ApiError> {
        let http = self.http.clone();
        let base_url = self.base_url.clone();
        let token = self.token.clone();
        self.run_on(async move {
            let response = send_request(
                &http,
                &base_url,
                &token,
                reqwest::Method::GET,
                "/v1/im/sessions",
                None,
            )
            .await?;
            let body: serde_json::Value = response.json().await?;
            Ok(body["items"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| serde_json::from_value(item.clone()).ok())
                        .collect()
                })
                .unwrap_or_default())
        })
        .await
    }

    pub async fn task_detail(&self, task_id: &str) -> Result<TaskDetail, ApiError> {
        let http = self.http.clone();
        let base_url = self.base_url.clone();
        let token = self.token.clone();
        let path = format!("/v1/tasks/{task_id}");
        self.run_on(async move {
            let response =
                send_request(&http, &base_url, &token, reqwest::Method::GET, &path, None).await?;
            response.json().await.map_err(ApiError::from)
        })
        .await
    }

    pub async fn inbox_tasks(&self) -> Result<Vec<ProductTask>, ApiError> {
        #[derive(Deserialize)]
        struct Inbox {
            items: Vec<ProductTask>,
        }
        let http = self.http.clone();
        let base_url = self.base_url.clone();
        let token = self.token.clone();
        self.run_on(async move {
            let response = send_request(
                &http,
                &base_url,
                &token,
                reqwest::Method::GET,
                "/v1/inbox",
                None,
            )
            .await?;
            Ok(response.json::<Inbox>().await?.items)
        })
        .await
    }
    pub async fn mesh_status(&self) -> Result<MeshStatus, ApiError> {
        let http = self.http.clone();
        let base_url = self.base_url.clone();
        let token = self.token.clone();
        self.run_on(async move {
            send_request(
                &http,
                &base_url,
                &token,
                reqwest::Method::GET,
                "/v1/mesh",
                None,
            )
            .await?
            .json()
            .await
            .map_err(ApiError::from)
        })
        .await
    }
    pub async fn delegate_task(
        &self,
        task: &ProductTask,
        origin: &str,
        workspace: &str,
        goal: &str,
    ) -> Result<(), ApiError> {
        let http = self.http.clone();
        let base_url = self.base_url.clone();
        let token = self.token.clone();
        let path = format!("/v1/tasks/{}/delegate", task.task_id);
        let body = serde_json::json!({"command_id":format!("delegate-{}",task.task_id),"expected_revision":task.revision,"executor_origin":origin,"workspace_id":workspace,"goal":goal});
        self.run_on(async move {
            send_request(
                &http,
                &base_url,
                &token,
                reqwest::Method::POST,
                &path,
                Some(body),
            )
            .await?;
            Ok(())
        })
        .await
    }

    pub async fn page_catalog(&self) -> Result<Option<crate::pages::PageCatalog>, ApiError> {
        match self
            .node_request(reqwest::Method::GET, "/v1/node/pages".into(), None)
            .await
        {
            Ok(value) => serde_json::from_value(value)
                .map(Some)
                .map_err(|error| ApiError::Task(std::io::Error::other(error))),
            Err(error) if error.status() == Some(404) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub async fn artifacts(&self) -> Result<Vec<Artifact>, ApiError> {
        #[derive(Deserialize)]
        struct Items {
            items: Vec<Artifact>,
        }
        let http = self.http.clone();
        let base_url = self.base_url.clone();
        let token = self.token.clone();
        self.run_on(async move {
            let response = send_request(
                &http,
                &base_url,
                &token,
                reqwest::Method::GET,
                "/v1/artifacts",
                None,
            )
            .await?;
            Ok(response.json::<Items>().await?.items)
        })
        .await
    }

    pub async fn artifact_content(&self, artifact_id: &str) -> Result<Vec<u8>, ApiError> {
        let http = self.http.clone();
        let base_url = self.base_url.clone();
        let token = self.token.clone();
        let path = format!("/v1/artifacts/{artifact_id}/content");
        self.run_on(async move {
            let response =
                send_request(&http, &base_url, &token, reqwest::Method::GET, &path, None).await?;
            Ok(response.bytes().await?.to_vec())
        })
        .await
    }

    pub async fn register_artifact(
        &self,
        task_id: &str,
        file_path: String,
    ) -> Result<Artifact, ApiError> {
        #[derive(Deserialize)]
        struct Registered {
            artifact: Artifact,
        }
        let http = self.http.clone();
        let base_url = self.base_url.clone();
        let token = self.token.clone();
        let path = format!("/v1/tasks/{task_id}/artifacts");
        self.run_on(async move {
            let response = send_request(
                &http,
                &base_url,
                &token,
                reqwest::Method::POST,
                &path,
                Some(serde_json::json!({"path":file_path})),
            )
            .await?;
            Ok(response.json::<Registered>().await?.artifact)
        })
        .await
    }

    pub async fn transition_task(
        &self,
        task_id: &str,
        expected_revision: i64,
        action: TaskAction,
    ) -> Result<ProductTask, ApiError> {
        let http = self.http.clone();
        let base_url = self.base_url.clone();
        let token = self.token.clone();
        let path = format!("/v1/tasks/{task_id}/transitions");
        let body = serde_json::json!({ "expected_revision": expected_revision, "action": action });
        self.run_on(async move {
            let response = send_request(
                &http,
                &base_url,
                &token,
                reqwest::Method::POST,
                &path,
                Some(body),
            )
            .await?;
            response.json().await.map_err(ApiError::from)
        })
        .await
    }

    pub async fn create_session(
        &self,
        profile_id: &str,
        model: &str,
        thinking: &str,
        workspace: &str,
    ) -> Result<SessionSummary, ApiError> {
        let body = serde_json::json!({
            "profile_id": profile_id,
            "model": model,
            "thinking": thinking,
            "workspace": workspace,
        });
        let http = self.http.clone();
        let base_url = self.base_url.clone();
        let token = self.token.clone();
        self.run_on(async move {
            let response = send_request(
                &http,
                &base_url,
                &token,
                reqwest::Method::POST,
                "/v1/im/sessions",
                Some(body),
            )
            .await?;
            response.json().await.map_err(ApiError::from)
        })
        .await
    }

    pub async fn update_selection(
        &self,
        session_id: &str,
        profile_id: &str,
        model: &str,
        thinking: &str,
    ) -> Result<SessionSummary, ApiError> {
        let body = serde_json::json!({
            "profile_id": profile_id,
            "model": model,
            "thinking": thinking,
        });
        let path = format!("/v1/im/sessions/{session_id}/selection");
        let http = self.http.clone();
        let base_url = self.base_url.clone();
        let token = self.token.clone();
        self.run_on(async move {
            let response = send_request(
                &http,
                &base_url,
                &token,
                reqwest::Method::PUT,
                &path,
                Some(body),
            )
            .await?;
            response.json().await.map_err(ApiError::from)
        })
        .await
    }

    pub async fn session_context(
        &self,
        session_id: &str,
        update: Option<ContextConfig>,
    ) -> Result<ContextConfig, ApiError> {
        let path = format!("/v1/im/sessions/{session_id}/context");
        let http = self.http.clone();
        let base_url = self.base_url.clone();
        let token = self.token.clone();
        self.run_on(async move {
            let (method, body) = match update {
                Some(config) => (reqwest::Method::PUT, Some(serde_json::json!(config))),
                None => (reqwest::Method::GET, None),
            };
            let response = send_request(&http, &base_url, &token, method, &path, body).await?;
            response.json().await.map_err(ApiError::from)
        })
        .await
    }

    pub async fn post_message(&self, session_id: &str, content: &str) -> Result<(), ApiError> {
        self.post_message_id(session_id, content, &ulid::Ulid::new().to_string())
            .await
    }
    pub async fn post_message_id(
        &self,
        session_id: &str,
        content: &str,
        request_id: &str,
    ) -> Result<(), ApiError> {
        let body = serde_json::json!({ "content": content, "request_id":request_id, "client_id":self.client_id });
        let path = format!("/v1/im/sessions/{session_id}/messages");
        let http = self.http.clone();
        let base_url = self.base_url.clone();
        let token = self.token.clone();
        self.run_on(async move {
            send_request(
                &http,
                &base_url,
                &token,
                reqwest::Method::POST,
                &path,
                Some(body),
            )
            .await?;
            Ok(())
        })
        .await
    }

    pub async fn cancel_session(&self, session_id: &str) -> Result<(), ApiError> {
        let path = format!("/v1/im/sessions/{session_id}/cancel");
        let http = self.http.clone();
        let base_url = self.base_url.clone();
        let token = self.token.clone();
        self.run_on(async move {
            send_request(&http, &base_url, &token, reqwest::Method::POST, &path, None).await?;
            Ok(())
        })
        .await
    }

    pub async fn list_messages(
        &self,
        session_id: &str,
        before: Option<&str>,
        limit: u32,
    ) -> Result<MessagePage, ApiError> {
        let mut query = format!("limit={limit}");
        if let Some(cursor) = before {
            let encoded = percent_encode(cursor);
            query.push_str(&format!("&before={encoded}"));
        }
        let path = format!("/v1/im/sessions/{session_id}/messages?{query}");
        let http = self.http.clone();
        let base_url = self.base_url.clone();
        let token = self.token.clone();
        self.run_on(async move {
            let response =
                send_request(&http, &base_url, &token, reqwest::Method::GET, &path, None).await?;
            response.json().await.map_err(ApiError::from)
        })
        .await
    }

    pub async fn list_profiles(&self) -> Result<Vec<ProfileInfo>, ApiError> {
        #[cfg(feature = "headless-bench")]
        if let Some(fixture) = &self.fixture {
            return fixture.list_profiles().map_err(|error| ApiError::Api { status: 400, message: error.to_string() });
        }
        let http = self.http.clone();
        let base_url = self.base_url.clone();
        let token = self.token.clone();
        self.run_on(async move {
            let response = send_request(
                &http,
                &base_url,
                &token,
                reqwest::Method::GET,
                "/v1/im/profiles",
                None,
            )
            .await?;
            let body: serde_json::Value = response.json().await?;
            Ok(body["items"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| serde_json::from_value(item.clone()).ok())
                        .collect()
                })
                .unwrap_or_default())
        })
        .await
    }

    /// Open the SSE event stream for a session. The stream ends when the
    /// gateway closes the entry channel or the connection drops.
    pub async fn stream_events(&self, session_id: &str) -> Result<SseStream, ApiError> {
        self.stream_path(format!("/v1/im/sessions/{session_id}/events"))
            .await
    }

    pub async fn stream_updates(&self) -> Result<SseStream, ApiError> {
        self.stream_path("/v1/im/events".into()).await
    }

    async fn stream_path(&self, path: String) -> Result<SseStream, ApiError> {
        self.stream_request(path, None).await
    }

    pub(crate) async fn stream_request(
        &self,
        path: String,
        body: Option<serde_json::Value>,
    ) -> Result<SseStream, ApiError> {
        // One observer per device, shared by all retained conversation views.
        let track_route = path == "/v1/im/events";
        let http = self.sse_http.clone();
        let base_url = self.base_url.clone();
        let token = self.token.clone();
        let (mut tx, rx) = mpsc::channel::<Result<Bytes, ApiError>>(64);
        let (ready_tx, ready_rx) = futures_channel::oneshot::channel();
        let (route_tx, route_rx) = tokio::sync::watch::channel(ConnectionRoute::default());
        let task = self.rt.spawn(async move {
            if let Some((control, origin)) = &http.mesh {
                let payload = serde_json::json!({"v":1,"request":{"kind":"subscribe","path":path,"body":body}});
                let mut source = match control.subscribe(origin, &payload).await {
                    Ok(source) => source,
                    Err(e) => {
                        let _ = ready_tx
                            .send(Err(ApiError::Task(std::io::Error::other(format!("{e:#}")))));
                        return;
                    }
                };
                let first = match source.next().await {
                    Ok(Some(value)) if value["v"] == 1 && value["ok"] == true => value,
                    Ok(Some(value)) => {
                        let _ = ready_tx.send(Err(ApiError::Api {
                            status: 403,
                            message: value["error"]
                                .as_str()
                                .unwrap_or("Mesh subscription rejected")
                                .into(),
                        }));
                        return;
                    }
                    result => {
                        let _ = ready_tx.send(Err(ApiError::Task(std::io::Error::other(format!(
                            "Mesh subscription rejected: {result:?}"
                        )))));
                        return;
                    }
                };
                let mut routes = if track_route {
                    source.routes()
                } else {
                    futures_util::stream::empty().boxed()
                };
                let initial_route = routes.next().await.unwrap_or_default();
                route_tx.send_replace(initial_route);
                if ready_tx.send(Ok(())).is_err() {
                    return;
                }
                let mut routes_open = track_route;
                let mut frame = Some(first);
                loop {
                    let value = match frame.take() {
                        Some(value) => value,
                        None => match tokio::select! {
                            value = source.next() => value,
                            route = routes.next(), if routes_open => {
                                if let Some(route) = route {
                                    route_tx.send_if_modified(|old| {
                                        if *old == route { return false; }
                                        *old = route;
                                        true
                                    });
                                } else {
                                    routes_open = false;
                                }
                                continue;
                            }
                        } {
                            Ok(Some(value)) => value,
                            Ok(None) => break,
                            Err(e) => {
                                let _ = tx
                                    .send(Err(ApiError::Task(std::io::Error::other(format!(
                                        "{e:#}"
                                    )))))
                                    .await;
                                break;
                            }
                        },
                    };
                    if value["v"] != 1 || value["ok"] != true {
                        let _ = tx
                            .send(Err(ApiError::Api {
                                status: 403,
                                message: value["error"]
                                    .as_str()
                                    .unwrap_or("Mesh subscription rejected")
                                    .into(),
                            }))
                            .await;
                        break;
                    }
                    let event = &value["data"];
                    let bytes = Bytes::from(format!(
                        "event: {}\ndata: {}\n\n",
                        event["name"].as_str().unwrap_or("message"),
                        event["data"]
                    ));
                    if tx.send(Ok(bytes)).await.is_err() {
                        break;
                    }
                }
                return;
            }
            let response =
                match send_request(&http, &base_url, &token, if body.is_some() { reqwest::Method::POST } else { reqwest::Method::GET }, &path, body)
                    .await
                {
                    Ok(response) => response,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };
            // HTTP exposes its connected peer, rather than the configured URL.
            // Without the local interface, non-loopback directness is unknown
            // (the socket may terminate at a proxy).
            if response.url().host_str().is_some_and(|host| {
                host.eq_ignore_ascii_case("localhost")
                    || host
                        .trim_matches(['[', ']'])
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|ip| ip.is_loopback())
            }) && response
                .remote_addr()
                .is_some_and(|addr| addr.ip().is_loopback())
            {
                route_tx.send_replace(ConnectionRoute {
                    scope: ConnectionScope::Local,
                    direct: true,
                });
            }
            if ready_tx.send(Ok(())).is_err() {
                return;
            }
            let mut bytes = response.bytes_stream();
            while let Some(chunk) = bytes.next().await {
                match chunk {
                    Ok(bytes) => {
                        if tx.send(Ok(bytes)).await.is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = tx.send(Err(ApiError::Request(error))).await;
                        break;
                    }
                }
            }
        });
        let abort_handle = task.abort_handle();
        // Also cancel if the caller times out while waiting for the handshake.
        let mut guard = AbortOnDrop(Some(abort_handle.clone()));
        match ready_rx.await {
            Ok(Ok(())) => {
                // Dropping a Tokio JoinHandle detaches rather than cancels.
                // SseStream owns the abort handle and cancels on drop.
                drop(task);
            }
            Ok(Err(error)) => {
                let _ = task.await;
                return Err(error);
            }
            Err(_) => match task.await {
                Err(join_error) => return Err(join_task_error(join_error)),
                Ok(()) => {
                    return Err(ApiError::Task(std::io::Error::other(
                        "SSE task ended before response headers",
                    )))
                }
            },
        }
        guard.0.take();
        let mut stream = SseStream::with_abort(rx, abort_handle);
        stream.route = track_route.then_some(route_rx);
        Ok(stream)
    }
}

fn percent_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Minimal SSE parser over a byte stream. Emits one `SseEvent` per blank
/// line; comment lines (`: ping` keepalives) are ignored. The upstream byte
/// stream is bridged from the dedicated Tokio runtime via an mpsc channel,
/// so `poll_next` can run on the GPUI executor.
pub struct SseStream {
    pub(crate) route: Option<tokio::sync::watch::Receiver<ConnectionRoute>>,
    inner: futures_util::stream::BoxStream<'static, Result<Bytes, ApiError>>,
    buf: Vec<u8>,
    name: String,
    data: String,
    abort_handle: Option<tokio::task::AbortHandle>,
}

impl SseStream {
    pub fn new(
        inner: impl Stream<Item = Result<Bytes, ApiError>> + Unpin + Send + 'static,
    ) -> Self {
        Self {
            route: None,
            inner: inner.boxed(),
            buf: Vec::new(),
            name: String::new(),
            data: String::new(),
            abort_handle: None,
        }
    }

    fn with_abort(
        inner: impl Stream<Item = Result<Bytes, ApiError>> + Unpin + Send + 'static,
        abort_handle: tokio::task::AbortHandle,
    ) -> Self {
        let mut stream = Self::new(inner);
        stream.abort_handle = Some(abort_handle);
        stream
    }
}

impl Drop for SseStream {
    fn drop(&mut self) {
        if let Some(abort_handle) = self.abort_handle.take() {
            abort_handle.abort();
        }
    }
}

impl Stream for SseStream {
    type Item = Result<SseEvent, ApiError>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if let Some(nl) = this.buf.iter().position(|byte| *byte == b'\n') {
                let mut line = this.buf.drain(..=nl).collect::<Vec<_>>();
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                let line = String::from_utf8_lossy(&line);
                if line.is_empty() {
                    if !this.name.is_empty() || !this.data.is_empty() {
                        let name = if this.name.is_empty() {
                            "message".to_owned()
                        } else {
                            std::mem::take(&mut this.name)
                        };
                        let data = std::mem::take(&mut this.data);
                        return std::task::Poll::Ready(Some(Ok(SseEvent { name, data })));
                    }
                } else if let Some(value) = line.strip_prefix("event:") {
                    this.name = value.trim().to_owned();
                } else if let Some(value) = line.strip_prefix("data:") {
                    if !this.data.is_empty() {
                        this.data.push('\n');
                    }
                    this.data.push_str(value.trim());
                }
                // comment lines and unknown fields are ignored
                continue;
            }
            match this.inner.poll_next_unpin(cx) {
                std::task::Poll::Ready(Some(Ok(chunk))) => {
                    this.buf.extend_from_slice(&chunk);
                }
                std::task::Poll::Ready(Some(Err(error))) => {
                    return std::task::Poll::Ready(Some(Err(error)));
                }
                std::task::Poll::Ready(None) => return std::task::Poll::Ready(None),
                std::task::Poll::Pending => return std::task::Poll::Pending,
            }
        }
    }
}

struct AbortOnDrop(Option<tokio::task::AbortHandle>);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        if let Some(handle) = &self.0 {
            handle.abort();
        }
    }
}
