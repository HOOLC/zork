//! Reverse browser RPC. The client subscribes over its existing authenticated
//! HTTP/Mesh connection; Gateway never receives browser credentials.
use crate::state::AppState;
use anyhow::{ensure, Context, Result};
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::watch;
use zork_browser::Command;

const CLIENT_SCOPE: &str = "clients";
const LEASE: Duration = Duration::from_secs(20);
const DEADLINE: Duration = Duration::from_secs(18);
const MAX_RESULT: usize = 192 * 1024;
pub struct Hub {
    state: Mutex<HubState>,
    wake: zork_notify::Hub<(String, String)>,
    receipts: Mutex<rusqlite::Connection>,
}
#[derive(Default)]
struct HubState {
    clients: HashMap<(String, String), Client>,
    requests: HashMap<(String, String), Arc<Request>>,
}
struct Client {
    secret: String,
    name: String,
    seen: Instant,
    stream: Option<String>,
    streamed: bool,
}
struct Request {
    device: String,
    command: Command,
    created: Instant,
    result: watch::Sender<Option<Value>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Poll {
    #[serde(default)]
    generation: u64,
    client_id: String,
    secret: String,
    name: String,
    #[serde(default)]
    replies: Vec<Reply>,
    #[serde(default)]
    disconnect: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    request_id: String,
    result: Value,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolRequest {
    pub session_id: String,
    pub client_id: String,
    pub command: Command,
}
fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
fn failure(message: &str) -> Value {
    json!({"error":message})
}
impl Hub {
    pub fn open(root: &std::path::Path) -> Result<Self> {
        std::fs::create_dir_all(root.join("state"))?;
        Self::from_connection(rusqlite::Connection::open(
            root.join("state/browser.sqlite"),
        )?)
    }
    fn from_connection(conn: rusqlite::Connection) -> Result<Self> {
        conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS browser_receipts(session TEXT NOT NULL,request_id TEXT NOT NULL,device TEXT NOT NULL,command TEXT NOT NULL,result TEXT,PRIMARY KEY(session,request_id)); CREATE TABLE IF NOT EXISTS browser_clients(session TEXT NOT NULL,client TEXT NOT NULL,secret_hash TEXT NOT NULL,revoked INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(session,client));")?;
        let columns = conn
            .prepare("PRAGMA table_info(browser_clients)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if !columns.iter().any(|name| name == "generation") {
            conn.execute_batch(
                "ALTER TABLE browser_clients ADD COLUMN generation INTEGER NOT NULL DEFAULT 0;",
            )?;
        }
        Ok(Self {
            state: Mutex::new(HubState::default()),
            wake: Default::default(),
            receipts: Mutex::new(conn),
        })
    }
    fn finish(&self, session: &str, request: &Request, value: Value) -> Result<()> {
        let conn = self.receipts.lock().unwrap();
        conn.execute("UPDATE browser_receipts SET result=?3 WHERE session=?1 AND request_id=?2 AND result IS NULL",params![session,request.command.request_id,serde_json::to_string(&value)?])?;
        let saved: String = conn.query_row(
            "SELECT result FROM browser_receipts WHERE session=?1 AND request_id=?2",
            params![session, request.command.request_id],
            |r| r.get(0),
        )?;
        request
            .result
            .send_replace(Some(serde_json::from_str(&saved)?));
        self.wake
            .publish([(session.to_owned(), request.device.clone())]);
        Ok(())
    }
    fn poll_once(&self, session: &str, poll: &Poll) -> Result<Value> {
        self.exchange(session, poll, true)
    }
    fn exchange(&self, session: &str, poll: &Poll, register: bool) -> Result<Value> {
        ensure!(
            identifier(&poll.client_id)
                && poll.secret.len() == 64
                && poll.secret.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid_browser_client"
        );
        ensure!(
            poll.name.len() <= 160 && poll.replies.len() <= 16,
            "invalid_browser_poll"
        );
        let mut state = self.state.lock().unwrap();
        let rotated = {
            let conn = self.receipts.lock().unwrap();
            let stored: Option<(String, bool, u64)> = conn.query_row("SELECT secret_hash,revoked,generation FROM browser_clients WHERE session=?1 AND client=?2", params![session,poll.client_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
            let hash = zork_mesh::content_root(poll.secret.as_bytes());
            match stored {
                Some((_, _, generation))
                    if register && !poll.disconnect && poll.generation > generation =>
                {
                    conn.execute("UPDATE browser_clients SET secret_hash=?3,revoked=0,generation=?4 WHERE session=?1 AND client=?2", params![session,poll.client_id,hash,poll.generation])?;
                    true
                }
                Some((expected, revoked, generation)) => {
                    ensure!(
                        generation == poll.generation && expected == hash,
                        "browser_client_credential_mismatch"
                    );
                    ensure!(!register || !revoked, "browser_grant_revoked");
                    false
                }
                None => {
                    ensure!(register && !poll.disconnect, "browser_client_not_connected");
                    conn.execute("INSERT INTO browser_clients(session,client,secret_hash,generation) VALUES(?1,?2,?3,?4)", params![session,poll.client_id,hash,poll.generation])?;
                    false
                }
            }
        };
        if rotated {
            state
                .clients
                .remove(&(session.to_owned(), poll.client_id.clone()));
            for ((owner, _), request) in &state.requests {
                if owner == session
                    && request.device == poll.client_id
                    && request.result.borrow().is_none()
                {
                    self.finish(
                        session,
                        request,
                        failure("浏览器控制已切换；已投递的操作状态可能不确定"),
                    )?;
                }
            }
        }
        state
            .clients
            .retain(|_, c| c.stream.is_some() || c.seen.elapsed() < LEASE);
        state.requests.retain(|_, r| r.result.borrow().is_none());
        let key = (session.to_owned(), poll.client_id.clone());
        if let Some(client) = state.clients.get(&key) {
            ensure!(
                client.secret == poll.secret,
                "browser_client_credential_mismatch"
            );
        } else {
            ensure!(state.clients.len() < 64, "browser_client_capacity");
            ensure!(
                !register || !poll.disconnect,
                "browser_client_not_connected"
            );
        }
        if poll.disconnect {
            self.receipts.lock().unwrap().execute(
                "UPDATE browser_clients SET revoked=1 WHERE session=?1 AND client=?2",
                params![session, poll.client_id],
            )?;
            state.clients.remove(&key);
            for ((owner, _), r) in &state.requests {
                if owner == session && r.device == poll.client_id && r.result.borrow().is_none() {
                    self.finish(
                        session,
                        r,
                        failure("用户已接管浏览器；已投递的操作状态可能不确定"),
                    )?;
                }
            }
            self.wake
                .publish([(session.to_owned(), poll.client_id.clone())]);
            return Ok(json!({"commands":[],"accepted":[]}));
        }
        if register {
            state
                .clients
                .entry(key)
                .and_modify(|client| {
                    client.seen = Instant::now();
                    client.name = poll.name.clone();
                })
                .or_insert_with(|| Client {
                    secret: poll.secret.clone(),
                    name: poll.name.clone(),
                    seen: Instant::now(),
                    stream: None,
                    streamed: false,
                });
        }
        let mut accepted = vec![];
        for reply in &poll.replies {
            ensure!(
                identifier(&reply.request_id)
                    && serde_json::to_vec(&reply.result)?.len() <= MAX_RESULT,
                "invalid_browser_result"
            );
            if let Some(request) = state
                .requests
                .get(&(session.into(), reply.request_id.clone()))
            {
                ensure!(
                    request.device == poll.client_id,
                    "browser_result_wrong_device"
                );
                if request.result.borrow().is_none() {
                    self.finish(session, request, reply.result.clone())?;
                }
            } else {
                // A surviving client may finish after the Gateway restarts.
                // Recover the receipt without reconstructing/re-executing a command.
                let conn = self.receipts.lock().unwrap();
                let device: Option<String> = conn
                    .query_row(
                        "SELECT device FROM browser_receipts WHERE session=?1 AND request_id=?2",
                        params![session, reply.request_id],
                        |row| row.get(0),
                    )
                    .optional()?;
                ensure!(
                    device.as_deref() == Some(poll.client_id.as_str()),
                    "browser_result_wrong_device"
                );
                conn.execute("UPDATE browser_receipts SET result=?3 WHERE session=?1 AND request_id=?2 AND result IS NULL", params![session,reply.request_id,serde_json::to_string(&reply.result)?])?;
            }
            accepted.push(reply.request_id.clone());
        }
        if !register {
            return Ok(json!({"commands":[],"accepted":accepted}));
        }
        let mut pending: Vec<_> = state
            .requests
            .iter()
            .filter(|((owner, _), r)| {
                owner == session && r.device == poll.client_id && r.result.borrow().is_none()
            })
            .map(|(_, r)| r.clone())
            .collect();
        pending.sort_by_key(|r| r.created);
        let mut commands = vec![];
        let mut command_bytes = 0;
        for request in pending {
            if request.created.elapsed() >= DEADLINE {
                self.finish(
                    session,
                    &request,
                    failure("浏览器操作已超时，可能已执行；请先检查页面状态"),
                )?;
            } else if command_bytes + serde_json::to_vec(&request.command)?.len() <= 128 * 1024 {
                command_bytes += serde_json::to_vec(&request.command)?.len();
                commands.push(request.command.clone());
            }
        }
        Ok(json!({"commands":commands,"accepted":accepted}))
    }
    fn acknowledge(&self, session: &str, receipt: Poll) -> Result<Value> {
        ensure!(
            receipt.disconnect || !receipt.replies.is_empty(),
            "browser_stream_required"
        );
        self.exchange(session, &receipt, false)
    }
    pub async fn command(
        &self,
        session: &str,
        device: Option<String>,
        command: Command,
    ) -> Result<Value> {
        ensure!(
            identifier(&command.request_id),
            "invalid_browser_request_id"
        );
        ensure!(
            serde_json::to_vec(&command)?.len() <= 32 * 1024,
            "browser_command_too_large"
        );
        let request = {
            let mut state = self.state.lock().unwrap();
            let key = (session.to_owned(), command.request_id.clone());
            let saved:Option<(String,String,Option<String>)>=self.receipts.lock().unwrap().query_row("SELECT device,command,result FROM browser_receipts WHERE session=?1 AND request_id=?2",params![session,command.request_id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
            if let Some((saved_device, saved_command, result)) = saved {
                ensure!(
                    serde_json::from_str::<Value>(&saved_command)?
                        == serde_json::to_value(&command)?
                        && device.as_ref().is_none_or(|d| d == &saved_device),
                    "browser_request_id_conflict"
                );
                if let Some(result) = result {
                    return Ok(serde_json::from_str(&result)?);
                }
                if !state.requests.contains_key(&key) {
                    return Ok(failure(
                        "上次浏览器操作的回执未知；不会在重启后重发，请先检查页面状态。",
                    ));
                }
            }
            if let Some(old) = state.requests.get(&key) {
                ensure!(
                    serde_json::to_value(&old.command)? == serde_json::to_value(&command)?
                        && device.as_ref().is_none_or(|d| d == &old.device),
                    "browser_request_id_conflict"
                );
                old.clone()
            } else {
                let clients: Vec<_> = state
                    .clients
                    .iter()
                    .filter(|((owner, _), c)| {
                        owner == session
                            && (if c.streamed {
                                c.stream.is_some()
                            } else {
                                c.seen.elapsed() < LEASE
                            })
                    })
                    .map(|((_, id), c)| (id.clone(), c.name.clone()))
                    .collect();
                if clients.is_empty() {
                    return Ok(failure(
                        "没有已授权的客户端浏览器。请在目标设备的浏览器侧栏启用 Agent 操作。",
                    ));
                }
                let selected = match device {
                    Some(id) => {
                        ensure!(
                            clients.iter().any(|(client, _)| client == &id),
                            "browser_device_not_available"
                        );
                        id
                    }
                    None if clients.len() == 1 => clients[0].0.clone(),
                    None => {
                        return Ok(
                            json!({"devices":clients.iter().map(|(id,name)|json!({"id":id,"name":name})).collect::<Vec<_>>(),"message":"请明确选择 device_id"}),
                        )
                    }
                };
                ensure!(state.requests.len() < 256, "browser_request_capacity");
                let (result, _) = watch::channel(None);
                self.receipts.lock().unwrap().execute("INSERT INTO browser_receipts(session,request_id,device,command) VALUES(?1,?2,?3,?4)",params![session,command.request_id,selected,serde_json::to_string(&command)?])?;
                let request = Arc::new(Request {
                    device: selected,
                    command,
                    created: Instant::now(),
                    result,
                });
                state.requests.insert(key, request.clone());
                request
            }
        };
        let mut result = request.result.subscribe();
        self.wake
            .publish([(session.to_owned(), request.device.clone())]);
        let remaining = DEADLINE.saturating_sub(request.created.elapsed());
        if tokio::time::timeout(remaining, result.wait_for(|v| v.is_some()))
            .await
            .is_err()
        {
            self.finish(session,&request,failure("浏览器操作已超时，可能已执行；请先检查页面状态。重试此 request_id 不会重新执行。"))?;
        }
        let value = request
            .result
            .borrow()
            .clone()
            .unwrap_or_else(|| failure("浏览器连接已关闭"));
        self.state
            .lock()
            .unwrap()
            .requests
            .remove(&(session.to_owned(), request.command.request_id.clone()));
        Ok(value)
    }
}
pub async fn receipts(State(state): State<AppState>, Json(poll): Json<Poll>) -> Response {
    match state.browser.acknowledge(CLIENT_SCOPE, poll) {
        Ok(v) => Json(v).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(failure(&e.to_string()))).into_response(),
    }
}
pub async fn tool(State(state): State<AppState>, Json(request): Json<ToolRequest>) -> Response {
    let result = tool_command(&state, request).await;
    match result {
        Ok(v) => Json(v).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(failure(&e.to_string()))).into_response(),
    }
}
async fn tool_command(state: &AppState, request: ToolRequest) -> Result<Value> {
    crate::node_access::subject(state, &request.session_id)?;
    let (origin, client) = request
        .client_id
        .rsplit_once('/')
        .context("invalid_client_id")?;
    ensure!(identifier(client), "invalid_client_id");
    if origin == "local" || origin == crate::node_access::identity(state) {
        command_for_client(
            state,
            &crate::node_access::identity(state),
            &request.session_id,
            client,
            request.command,
        )
        .await
    } else {
        crate::mesh::browser_request(state, origin, &request.session_id, client, request.command)
            .await
    }
}
pub(crate) async fn command_for_client(
    state: &AppState,
    caller: &str,
    session: &str,
    client: &str,
    mut command: Command,
) -> Result<Value> {
    ensure!(
        identifier(client)
            && !session.is_empty()
            && session.len() <= 256
            && identifier(&command.request_id),
        "invalid_browser_request"
    );
    command.request_id = crate::node_access::fingerprint(&(caller, session, &command.request_id))?;
    state
        .browser
        .command(CLIENT_SCOPE, Some(client.into()), command)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use zork_browser::Action;
    impl Default for Hub {
        fn default() -> Self {
            Self::from_connection(rusqlite::Connection::open_in_memory().unwrap()).unwrap()
        }
    }
    fn poll_body(secret: &str) -> Poll {
        Poll {
            client_id: "client".into(),
            generation: 0,
            secret: secret.into(),
            name: "test".into(),
            replies: vec![],
            disconnect: false,
        }
    }
    #[test]
    fn client_regrant_rotates_secret_and_fences_old_receipts() {
        let hub = Hub::default();
        let first = poll_body(&"a".repeat(64));
        hub.poll_once(CLIENT_SCOPE, &first).unwrap();
        let mut replacement = poll_body(&"b".repeat(64));
        replacement.generation = 1;
        hub.poll_once(CLIENT_SCOPE, &replacement).unwrap();
        assert!(hub.poll_once(CLIENT_SCOPE, &first).is_err());
        assert!(hub.acknowledge(CLIENT_SCOPE, first).is_err());
        replacement.disconnect = true;
        hub.exchange(CLIENT_SCOPE, &replacement, false).unwrap();
        replacement.disconnect = false;
        assert!(hub.poll_once(CLIENT_SCOPE, &replacement).is_err());
        replacement.generation = 2;
        hub.poll_once(CLIENT_SCOPE, &replacement).unwrap();
    }
    #[tokio::test]
    async fn retries_replay_result_and_cannot_change_command_or_client() {
        let hub = Arc::new(Hub::default());
        let secret = "a".repeat(64);
        hub.poll_once("session", &poll_body(&secret)).unwrap();
        assert!(hub
            .poll_once("session", &poll_body(&"b".repeat(64)))
            .is_err());
        let command = Command {
            request_id: "r1".into(),
            action: Action::Open {
                url: "https://example.com".into(),
            },
        };
        let owner = hub.clone();
        let copy = command.clone();
        let pending =
            tokio::spawn(async move { owner.command("session", None, copy).await.unwrap() });
        tokio::task::yield_now().await;
        let first = hub.poll_once("session", &poll_body(&secret)).unwrap();
        assert_eq!(first["commands"][0]["request_id"], "r1");
        assert_eq!(
            hub.poll_once("session", &poll_body(&secret)).unwrap()["commands"],
            first["commands"]
        );
        let mut response = poll_body(&secret);
        response.replies.push(Reply {
            request_id: "r1".into(),
            result: json!({"ok":true}),
        });
        hub.poll_once("session", &response).unwrap();
        assert_eq!(pending.await.unwrap(), json!({"ok":true}));
        assert_eq!(
            hub.command("session", None, command.clone()).await.unwrap(),
            json!({"ok":true})
        );
        assert!(hub
            .command(
                "session",
                None,
                Command {
                    request_id: "r1".into(),
                    action: Action::List
                }
            )
            .await
            .is_err());
        assert!(hub
            .command("session", Some("other".into()), command)
            .await
            .is_err());
        assert!(hub
            .command(
                "other",
                None,
                Command {
                    request_id: "r2".into(),
                    action: Action::List
                }
            )
            .await
            .unwrap()
            .get("error")
            .is_some());
    }
    #[tokio::test]
    async fn restart_preserves_receipts_and_never_replays_an_uncertain_action() {
        let root = tempfile::tempdir().unwrap();
        let hub = Arc::new(Hub::open(root.path()).unwrap());
        let secret = "a".repeat(64);
        hub.poll_once("session", &poll_body(&secret)).unwrap();
        let command = Command {
            request_id: "persisted".into(),
            action: Action::List,
        };
        let sender = hub.clone();
        let copy = command.clone();
        let task =
            tokio::spawn(async move { sender.command("session", None, copy).await.unwrap() });
        tokio::task::yield_now().await;
        let mut reply = poll_body(&secret);
        reply.replies.push(Reply {
            request_id: "persisted".into(),
            result: json!({"tabs":[]}),
        });
        hub.poll_once("session", &reply).unwrap();
        assert_eq!(task.await.unwrap(), json!({"tabs":[]}));
        let uncertain = Command {
            request_id: "uncertain".into(),
            action: Action::Click {
                tab_id: "tab".into(),
                selector: "#button".into(),
            },
        };
        let sender = hub.clone();
        let copy = uncertain.clone();
        let task =
            tokio::spawn(async move { sender.command("session", None, copy).await.unwrap() });
        tokio::task::yield_now().await;
        assert_eq!(
            hub.poll_once("session", &poll_body(&secret)).unwrap()["commands"][0]["request_id"],
            "uncertain"
        );
        task.abort();
        let _ = task.await;
        drop(hub);
        let reopened = Hub::open(root.path()).unwrap();
        assert_eq!(
            reopened.command("session", None, command).await.unwrap(),
            json!({"tabs":[]})
        );
        assert!(
            reopened.command("session", None, uncertain).await.unwrap()["error"]
                .as_str()
                .unwrap()
                .contains("不会")
        );
        assert_eq!(
            reopened.poll_once("session", &poll_body(&secret)).unwrap()["commands"],
            json!([])
        );
    }

    #[tokio::test]
    async fn late_receipt_survives_restart_without_registering_or_restoring_a_grant() {
        let root = tempfile::tempdir().unwrap();
        let hub = Arc::new(Hub::open(root.path()).unwrap());
        let secret = "a".repeat(64);
        hub.poll_once("session", &poll_body(&secret)).unwrap();
        let command = Command {
            request_id: "late".into(),
            action: Action::List,
        };
        let mut changes = hub.wake.subscribe([("session".into(), "client".into())]);
        let sender = hub.clone();
        let copy = command.clone();
        let task = tokio::spawn(async move { sender.command("session", None, copy).await });
        changes.changed().await.unwrap();
        task.abort();
        let _ = task.await;
        drop(hub);

        let hub = Hub::open(root.path()).unwrap();
        let receipt = |credential: &str, value: Value| {
            let mut body = poll_body(credential);
            body.replies.push(Reply {
                request_id: "late".into(),
                result: value,
            });
            body
        };
        assert!(hub
            .acknowledge("session", receipt(&"b".repeat(64), json!({"wrong":true})))
            .is_err());
        assert_eq!(
            hub.acknowledge("session", receipt(&secret, json!({"done":true})))
                .unwrap()["accepted"],
            json!(["late"])
        );
        assert!(hub.state.lock().unwrap().clients.is_empty());
        assert_eq!(
            hub.command("session", None, command.clone()).await.unwrap(),
            json!({"done":true})
        );
        hub.acknowledge("session", receipt(&secret, json!({"duplicate":true})))
            .unwrap();
        assert_eq!(
            hub.command("session", None, command.clone()).await.unwrap(),
            json!({"done":true})
        );

        hub.poll_once("session", &poll_body(&secret)).unwrap();
        let mut revoked = poll_body(&secret);
        revoked.disconnect = true;
        hub.acknowledge("session", revoked).unwrap();
        drop(hub);
        let hub = Hub::open(root.path()).unwrap();
        assert_eq!(
            hub.poll_once("session", &poll_body(&secret))
                .unwrap_err()
                .to_string(),
            "browser_grant_revoked"
        );
        hub.acknowledge(
            "session",
            receipt(&secret, json!({"late_after_revoke":true})),
        )
        .unwrap();
        assert!(hub.state.lock().unwrap().clients.is_empty());
        assert_eq!(
            hub.command("session", None, command).await.unwrap(),
            json!({"done":true})
        );
    }
}

/// Registration opens only a read stream. Command results travel through the
/// idempotent receipt endpoint; reconnecting never submits a browser action.
pub(crate) fn subscribe(
    state: AppState,
    registration: Poll,
) -> Result<tokio::sync::mpsc::Receiver<Value>> {
    let session = CLIENT_SCOPE.to_owned();
    ensure!(
        registration.replies.is_empty() && !registration.disconnect,
        "browser_stream_registration_only"
    );
    let key = (session.clone(), registration.client_id.clone());
    let changes = state.browser.wake.subscribe([key.clone()]).merge(
        state
            .db
            .realtime
            .listen(crate::realtime::MESH | crate::realtime::SESSIONS),
    );
    state.browser.poll_once(&session, &registration)?;
    let generation = ulid::Ulid::new().to_string();
    {
        let mut owned = state.browser.state.lock().unwrap();
        let client = owned
            .clients
            .get_mut(&key)
            .context("browser_client_not_connected")?;
        client.stream = Some(generation.clone());
        client.streamed = true;
    }
    state.browser.wake.publish([key]);
    let source = BrowserSource {
        state,
        session,
        registration,
        generation,
    };
    Ok(zork_notify::stream::spawn(
        source,
        changes,
        8,
        Some(zork_mesh::feed::HEARTBEAT),
        |event| {
            use zork_notify::stream::Event;
            Some(match event {
                Event::Data(value) => json!({"name":"browser_commands","data":value}),
                Event::Heartbeat => json!({"name":"heartbeat","data":{}}),
                Event::Error(error) => json!({"name":"error","data":{"error":error.to_string()}}),
            })
        },
    ))
}
struct BrowserSource {
    state: AppState,
    session: String,
    registration: Poll,
    generation: String,
}
impl Drop for BrowserSource {
    fn drop(&mut self) {
        let mut state = self.state.browser.state.lock().unwrap();
        if let Some(client) = state
            .clients
            .get_mut(&(self.session.clone(), self.registration.client_id.clone()))
        {
            // An old connection cannot detach a newer connection after resume.
            if client.stream.as_ref() == Some(&self.generation) {
                client.stream = None;
                client.seen = Instant::now();
            }
        }
    }
}
impl zork_notify::stream::Source for BrowserSource {
    type Item = Value;
    type Error = anyhow::Error;
    fn check_access(&self) -> Result<()> {
        let state = self.state.browser.state.lock().unwrap();
        ensure!(
            state
                .clients
                .get(&(self.session.clone(), self.registration.client_id.clone()))
                .is_some_and(|client| client.secret == self.registration.secret
                    && client.stream.as_ref() == Some(&self.generation)),
            "browser_subscription_replaced_or_revoked"
        );
        Ok(())
    }
    async fn read(&mut self) -> Result<zork_notify::stream::Page<Value>> {
        self.check_access()?;
        Ok(zork_notify::stream::Page::snapshot(
            self.state
                .browser
                .poll_once(&self.session, &self.registration)?,
        ))
    }
}

pub async fn events(State(state): State<AppState>, Json(registration): Json<Poll>) -> Response {
    match subscribe(state, registration) {
        Ok(receiver) => {
            let stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
                receiver.recv().await.map(|value| {
                    let event = axum::response::sse::Event::default()
                        .event(value["name"].as_str().unwrap_or("error"))
                        .data(value["data"].to_string());
                    (Ok::<_, std::convert::Infallible>(event), receiver)
                })
            });
            axum::response::Sse::new(stream).into_response()
        }
        Err(error) => (StatusCode::BAD_REQUEST, Json(failure(&error.to_string()))).into_response(),
    }
}
