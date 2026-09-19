//! Persistent service intent, authenticated sharing, and owned process lifetimes.
mod logs;
pub(crate) mod process;
mod store;
use crate::state::AppState;
use anyhow::{ensure, Context, Result};
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::watch;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Record {
    id: String,
    owner: String,
    name: String,
    port: u16,
    shared: bool,
    command: Option<Vec<String>>,
    cwd: Option<PathBuf>,
    desired_running: bool,
    generation: u64,
    revision: u64,
    state: String,
    last_exit_code: Option<i32>,
    last_error: Option<String>,
    last_started_at: Option<String>,
}
impl Record {
    fn external(id: String, owner: String, name: String, port: u16, shared: bool) -> Self {
        Self {
            id,
            owner,
            name,
            port,
            shared,
            command: None,
            cwd: None,
            desired_running: false,
            generation: 0,
            revision: 0,
            state: "external".into(),
            last_exit_code: None,
            last_error: None,
            last_started_at: None,
        }
    }
    fn value(&self, origin: &str, root: &Path) -> Value {
        let logs = root.join("services").join(&self.id).join("logs");
        json!({"id":self.id,"name":self.name,"node":origin,"port":self.port,"mode":if self.command.is_some(){"managed"}else{"external"},
            "shared":self.shared,"state":self.state,"desired_running":self.desired_running,"revision":self.revision,
            "command":self.command,"cwd":self.cwd,"last_exit_code":self.last_exit_code,"last_error":self.last_error,"last_started_at":self.last_started_at,
            "url":self.shared.then(||format!("zork://service/{}/{}/",origin.trim_start_matches("key:"),self.id)),"persistent":true,"expires_at_ms":null,
            "logs":self.command.as_ref().map(|_|json!({"node":origin,"directory":logs,"stdout":logs.join("stdout.log"),"stderr":logs.join("stderr.log"),"max_file_bytes":logs::LIMIT,"rotated_files_per_stream":logs::BACKUPS}))})
    }
}
struct Entry {
    record: Record,
    cancel: watch::Sender<bool>,
}
pub struct Registry {
    root: PathBuf,
    state: Mutex<RegistryState>,
    gate: tokio::sync::Mutex<()>,
    wake: zork_notify::Notifier,
}
struct RegistryState {
    store: store::Store,
    entries: HashMap<String, Entry>,
    live: HashMap<String, process::Live>,
    attempted: HashMap<String, u64>,
    stopped: bool,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ToolRequest {
    session_id: String,
    action: String,
    name: Option<String>,
    port: Option<u16>,
    id: Option<String>,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    command: Option<Vec<String>>,
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default)]
    expected_revision: Option<u64>,
}
impl Registry {
    pub fn open(root: &Path) -> Result<Self> {
        let store = store::Store::open(root)?;
        let root = root.canonicalize()?;
        let entries = store
            .load()?
            .into_iter()
            .map(|record| {
                (
                    record.id.clone(),
                    Entry {
                        record,
                        cancel: watch::channel(false).0,
                    },
                )
            })
            .collect();
        Ok(Self {
            root,
            state: Mutex::new(RegistryState {
                store,
                entries,
                live: HashMap::new(),
                attempted: HashMap::new(),
                stopped: false,
            }),
            gate: Default::default(),
            wake: zork_notify::Notifier::default(),
        })
    }
    fn record(&self, owner: &str, id: &str) -> Result<Record> {
        let state = self.state.lock().expect("service registry");
        ensure!(!state.stopped, "station_stopping");
        let entry = state.entries.get(id).context("service_not_found")?;
        ensure!(entry.record.owner == owner, "service_owner_mismatch");
        Ok(entry.record.clone())
    }
    fn list(&self, owner: &str, origin: &str, shared_only: bool) -> Result<Vec<Value>> {
        let state = self.state.lock().expect("service registry");
        ensure!(!state.stopped, "station_stopping");
        let mut entries = state
            .entries
            .values()
            .filter(|entry| entry.record.owner == owner && (!shared_only || entry.record.shared))
            .collect::<Vec<_>>();
        entries.sort_by(|a, b| a.record.id.cmp(&b.record.id));
        Ok(entries
            .into_iter()
            .map(|e| e.record.value(origin, &self.root))
            .collect())
    }
    pub(crate) fn client_resources(
        &self,
        origin: &str,
    ) -> Result<Vec<zork_client_types::resources::Resource>> {
        use zork_client_types::resources::{Resource, ResourceKind};
        let state = self.state.lock().expect("service registry");
        ensure!(!state.stopped, "station_stopping");
        let mut items = state
            .entries
            .values()
            .map(|entry| {
                let record = &entry.record;
                let mut item = Resource::new(
                    ResourceKind::Service,
                    record.id.clone(),
                    record.name.clone(),
                    if record.command.is_none() {
                        "external".into()
                    } else {
                        record.state.clone()
                    },
                    if record.shared { "shared" } else { "private" }.into(),
                );
                item.owner_session = Some(record.owner.clone());
                item.revision = Some(record.revision.to_string());
                item.port = Some(record.port);
                item.url = record.shared.then(|| {
                    format!(
                        "zork://service/{}/{}/",
                        origin.trim_start_matches("key:"),
                        record.id
                    )
                });
                item
            })
            .collect::<Vec<_>>();
        items.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        Ok(items)
    }
    pub(crate) fn client_details(
        &self,
        id: &str,
        log: Option<&str>,
    ) -> Result<zork_client_types::resources::ResourceDetails> {
        use std::io::{Read, Seek, SeekFrom};
        use zork_client_types::resources::{ResourceDetails, ResourceDocument, ResourceFile};
        let record = self
            .state
            .lock()
            .expect("service registry")
            .entries
            .get(id)
            .context("service_not_found")?
            .record
            .clone();
        let mut details = ResourceDetails {
            title: record.name.clone(),
            description: String::new(),
            facts: vec![
                ("status".into(), record.state.clone()),
                (
                    "mode".into(),
                    if record.command.is_some() {
                        "managed"
                    } else {
                        "external"
                    }
                    .into(),
                ),
                ("port".into(), record.port.to_string()),
                ("shared".into(), record.shared.to_string()),
                ("owner_session".into(), record.owner),
            ],
            ..Default::default()
        };
        if let Some(start) = record.last_started_at {
            details.facts.push(("last_started_at".into(), start));
        }
        if let Some(error) = record.last_error {
            details.facts.push(("last_error".into(), error));
        }
        if record.command.is_some() {
            for name in ["stdout.log", "stderr.log"] {
                let path = self.root.join("services").join(id).join("logs").join(name);
                details.files.push(ResourceFile {
                    path: name.into(),
                    byte_len: path.metadata().map(|m| m.len()).unwrap_or(0),
                });
            }
        }
        if let Some(log) = log {
            ensure!(
                record.command.is_some() && matches!(log, "stdout.log" | "stderr.log"),
                "invalid_service_log"
            );
            let path = self.root.join("services").join(id).join("logs").join(log);
            ensure!(
                !std::fs::symlink_metadata(&path)?.file_type().is_symlink(),
                "invalid_service_log"
            );
            let mut file = std::fs::File::open(path)?;
            let length = file.metadata()?.len();
            let start = length.saturating_sub(64 * 1024);
            file.seek(SeekFrom::Start(start))?;
            let mut bytes = Vec::new();
            file.take(64 * 1024).read_to_end(&mut bytes)?;
            details.document = Some(ResourceDocument {
                path: log.into(),
                text: String::from_utf8_lossy(&bytes).into_owned(),
                truncated: start > 0,
            });
        }
        Ok(details)
    }
    /// The journal covers the intent change, including restarts, before side effects.
    fn mutate(&self, request: &ToolRequest) -> Result<(String, bool)> {
        let fingerprint = blake3::hash(&serde_json::to_vec(request)?)
            .to_hex()
            .to_string();
        let mut state = self.state.lock().expect("service registry");
        ensure!(!state.stopped, "station_stopping");
        if let Some(key) = &request.request_id {
            ensure!(
                !key.is_empty() && key.len() <= 128,
                "invalid_service_request_id"
            );
            if let Some(id) = state
                .store
                .receipt(&request.session_id, key, &fingerprint)?
            {
                return Ok((id, true));
            }
        } else {
            ensure!(
                matches!(request.action.as_str(), "share" | "unshare"),
                "service_request_id_required"
            );
        }
        let legacy_share = request.action == "share" && request.id.is_none();
        let create = matches!(request.action.as_str(), "start" | "attach") || legacy_share;
        let mut record = if create {
            let name = request.name.as_deref().context("missing_service_name")?;
            let port = request.port.context("missing_service_port")?;
            ensure!(
                !name.trim().is_empty() && name.len() <= 80 && port > 0,
                "invalid_service_name_or_port"
            );
            if let Some(entry) = state
                .entries
                .values()
                .find(|e| e.record.owner == request.session_id && e.record.name == name)
            {
                entry.record.clone()
            } else {
                ensure!(state.entries.len() < 128, "service_limit");
                Record::external(
                    ulid::Ulid::new().to_string(),
                    request.session_id.clone(),
                    name.into(),
                    port,
                    false,
                )
            }
        } else {
            let entry = state
                .entries
                .get(request.id.as_deref().context("missing_service_id")?)
                .context("service_not_found")?;
            ensure!(
                entry.record.owner == request.session_id,
                "service_owner_mismatch"
            );
            entry.record.clone()
        };
        if let Some(revision) = request.expected_revision {
            ensure!(record.revision == revision, "service_revision_conflict");
        }
        match request.action.as_str() {
            "start" => {
                let command = request
                    .command
                    .as_ref()
                    .context("missing_service_command")?;
                ensure!(
                    !command.is_empty()
                        && command.len() <= 128
                        && command.iter().all(|s| !s.contains('\0'))
                        && command.iter().map(String::len).sum::<usize>() <= 32768
                        && !command[0].is_empty(),
                    "invalid_service_command"
                );
                let cwd = request.cwd.as_ref().context("missing_service_cwd")?;
                ensure!(cwd.is_absolute() && cwd.is_dir(), "invalid_service_cwd");
                let existing = state.entries.contains_key(&record.id);
                ensure!(
                    !existing || record.command.is_some(),
                    "external_service_cannot_be_started"
                );
                let changed = record.command.as_ref() != Some(command)
                    || record.cwd.as_ref() != Some(cwd)
                    || Some(record.port) != request.port;
                ensure!(
                    !existing || !changed || request.expected_revision.is_some(),
                    "service_configuration_conflict"
                );
                if changed || !record.desired_running || !state.live.contains_key(&record.id) {
                    record.generation += 1;
                    record.state = "starting".into();
                }
                record.command = Some(command.clone());
                record.cwd = Some(cwd.clone());
                record.port = request.port.unwrap();
                record.desired_running = true;
            }
            "attach" => {
                ensure!(
                    record.command.is_none(),
                    "managed_service_cannot_be_attached"
                );
                ensure!(
                    Some(record.port) == request.port,
                    "service_configuration_conflict"
                );
                record.shared = true;
            }
            "restart" => {
                ensure!(
                    record.command.is_some(),
                    "external_service_cannot_be_restarted"
                );
                record.desired_running = true;
                record.generation += 1;
                record.state = "starting".into();
            }
            "stop" => {
                ensure!(
                    record.command.is_some(),
                    "external_service_cannot_be_stopped"
                );
                record.desired_running = false;
                record.state = "stopped".into();
            }
            "share" => {
                if legacy_share {
                    ensure!(
                        Some(record.port) == request.port,
                        "service_configuration_conflict"
                    );
                }
                record.shared = true;
            }
            "unshare" => record.shared = false,
            _ => anyhow::bail!("invalid_service_action"),
        }
        record.revision += 1;
        state.store.save(
            &record,
            request
                .request_id
                .as_deref()
                .map(|key| (key, fingerprint.as_str())),
        )?;
        let entry = state
            .entries
            .entry(record.id.clone())
            .or_insert_with(|| Entry {
                record: record.clone(),
                cancel: watch::channel(false).0,
            });
        if !record.shared
            || record.generation != entry.record.generation
            || (record.command.is_some() && !record.desired_running)
        {
            entry.cancel.send_replace(true);
            entry.cancel = watch::channel(false).0;
        }
        entry.record = record.clone();
        self.wake.notify();
        Ok((record.id, false))
    }
    async fn inspect(&self, owner: &str, id: &str, origin: &str) -> Result<Value> {
        let record = self.record(owner, id)?;
        let (status, managed_alive) = {
            let mut state = self.state.lock().expect("service registry");
            let mut live = state.live.get_mut(id);
            let alive = live.as_mut().is_some_and(|live| live.is_running());
            (live.map(|live| live.status()), alive)
        };
        let eligible = record.command.is_none() || (record.desired_running && managed_alive);
        let ready = eligible
            && matches!(
                tokio::time::timeout(
                    Duration::from_millis(500),
                    tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, record.port))
                )
                .await,
                Ok(Ok(_))
            );
        let mut value = record.value(origin, &self.root);
        value["ready"] = json!(ready);
        if let Some(status) = status {
            value["pid"] = json!(status.pid);
            if status.error.is_some() {
                value["last_error"] = json!(status.error);
            }
        }
        Ok(value)
    }
    fn target(&self, id: &str) -> Result<(u16, watch::Receiver<bool>)> {
        let mut state = self.state.lock().expect("service registry");
        ensure!(!state.stopped, "station_stopping");
        let entry = state.entries.get(id).context("service_not_shared")?;
        ensure!(entry.record.shared, "service_not_shared");
        let port = entry.record.port;
        let cancel = entry.cancel.subscribe();
        if entry.record.command.is_some() {
            ensure!(entry.record.desired_running, "service_stopped");
            let live = state.live.get_mut(id).context("service_not_running")?;
            ensure!(live.is_running(), "service_not_running");
        }
        Ok((port, cancel))
    }
    async fn reconcile(&self, id: &str) -> Result<()> {
        let (record, live) = {
            let mut state = self.state.lock().expect("service registry");
            let record = state
                .entries
                .get(id)
                .context("service_not_found")?
                .record
                .clone();
            let remove = state.live.get_mut(id).is_some_and(|live| {
                !record.desired_running
                    || live.generation != record.generation
                    || !live.is_running()
            });
            (record, if remove { state.live.remove(id) } else { None })
        };
        if let Some(live) = live {
            let status = live.finish(true).await;
            let mut state = self.state.lock().expect("service registry");
            let mut current = state.entries[id].record.clone();
            current.last_exit_code = status.exit_code;
            current.last_error = if current.desired_running {
                status.error
            } else {
                None
            };
            current.state = if current.desired_running {
                "exited"
            } else {
                "stopped"
            }
            .into();
            state.store.save(&current, None)?;
            let entry = state.entries.get_mut(id).unwrap();
            entry.cancel.send_replace(true);
            entry.cancel = watch::channel(false).0;
            entry.record = current;
        }
        let start = {
            let mut state = self.state.lock().expect("service registry");
            !state.stopped
                && record.command.is_some()
                && record.desired_running
                && !state.live.contains_key(id)
                && state.attempted.insert(id.into(), record.generation) != Some(record.generation)
        };
        if start {
            let result = async {
                // Binding can fail while the old guard finishes after a crash.
                // Retry the failed reservation with a deadline, not TCP probes.
                let mut retry = zork_notify::retry::Retry::new(
                    Duration::from_millis(100),
                    Duration::from_millis(500),
                );
                tokio::time::timeout(Duration::from_secs(3), async {
                    loop {
                        match tokio::net::TcpListener::bind((
                            std::net::Ipv4Addr::LOCALHOST,
                            record.port,
                        ))
                        .await
                        {
                            Ok(listener) => {
                                drop(listener);
                                return Ok::<_, anyhow::Error>(());
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                                retry.wait().await
                            }
                            Err(error) => return Err(error.into()),
                        }
                    }
                })
                .await
                .context("service_port_in_use")??;
                process::Live::spawn(&self.root, &record, self.wake.clone()).await
            }
            .await;
            let mut current = record.clone();
            match result {
                Ok(live) => {
                    current.state = "running".into();
                    current.last_error = None;
                    current.last_exit_code = None;
                    current.last_started_at = Some(crate::config::now_rfc3339());
                    let saved = self
                        .state
                        .lock()
                        .expect("service registry")
                        .store
                        .save(&current, None);
                    if let Err(error) = saved {
                        live.finish(true).await;
                        return Err(error);
                    }
                    let mut state = self.state.lock().expect("service registry");
                    state.entries.get_mut(id).unwrap().record = current;
                    state.live.insert(id.into(), live);
                }
                Err(error) => {
                    current.state = "failed".into();
                    current.last_error = Some(error.to_string());
                    let mut state = self.state.lock().expect("service registry");
                    state.store.save(&current, None)?;
                    state.entries.get_mut(id).unwrap().record = current;
                }
            }
        }
        Ok(())
    }
    pub async fn run(self: Arc<Self>) {
        let mut changes = self.wake.subscribe();
        let mut retry = zork_notify::retry::Retry::default();
        loop {
            changes.checkpoint();
            let ids = {
                let state = self.state.lock().expect("service registry");
                if state.stopped {
                    return;
                }
                state.entries.keys().cloned().collect::<Vec<_>>()
            };
            let mut failed = false;
            for id in ids {
                let _gate = self.gate.lock().await;
                if self.state.lock().expect("service registry").stopped {
                    return;
                }
                if let Err(error) = self.reconcile(&id).await {
                    tracing::warn!(%error, service=%id, "service reconciliation failed");
                    failed = true;
                }
            }
            if failed {
                tokio::select! {
                    _ = retry.wait() => {},
                    changed = changes.changed() => if changed.is_err() { return; },
                }
            } else {
                retry.reset();
                if changes.changed().await.is_err() {
                    return;
                }
            }
        }
    }

    pub async fn disconnect(&self) {
        let _gate = self.gate.lock().await;
        let lives = {
            let mut state = self.state.lock().expect("service registry");
            state.stopped = true;
            for entry in state.entries.values() {
                entry.cancel.send_replace(true);
            }
            state.live.drain().map(|(_, live)| live).collect::<Vec<_>>()
        };
        self.wake.notify();
        futures_util::future::join_all(lives.into_iter().map(|live| live.finish(true))).await;
    }
}
fn check_port(state: &AppState, port: u16) -> Result<()> {
    ensure!(port > 0, "invalid_port");
    let config = zork_config::load_config(&state.config.data_root)?;
    for bind in [
        &config.bind.station,
        &config.bind.runtime,
        &config.bind.control,
        &config.bind.agent,
    ] {
        ensure!(
            zork_config::parse_bind(bind)?.port() != port,
            "cannot_share_zork_control_service"
        );
    }
    ensure!(
        state.config.bind_addr.port() != port,
        "cannot_share_zork_control_service"
    );
    Ok(())
}
pub async fn tool(State(state): State<AppState>, Json(request): Json<ToolRequest>) -> Response {
    let result = async {
        state
            .db
            .get_binding_by_id(&request.session_id)?
            .context("unknown_session")?;
        let mesh = state.mesh.get().context("mesh_not_ready")?;
        if request.action == "list" {
            let services = mesh
                .services
                .list(&request.session_id, mesh.origin(), false)?;
            return Ok(json!({"services": services}));
        }
        if request.action == "inspect" {
            return mesh
                .services
                .inspect(
                    &request.session_id,
                    request.id.as_deref().context("missing_service_id")?,
                    mesh.origin(),
                )
                .await;
        }
        if let Some(port) = request.port {
            check_port(&state, port)?;
        }
        ensure!(request.action == "attach", "invalid_service_action");
        let _gate = mesh.services.gate.lock().await;
        let (id, replayed) = mesh.services.mutate(&request)?;
        mesh.services.reconcile(&id).await?;
        let mut value = mesh
            .services
            .inspect(&request.session_id, &id, mesh.origin())
            .await?;
        let session = state
            .db
            .get_session_by_id(&request.session_id)?
            .context("unknown_session")?;
        let page = crate::db::pages::page_link(
            value["name"].as_str().context("service_name_missing")?,
            value["url"].as_str().context("service_url_missing")?,
            "",
        )?;
        state.db.publish_page(
            &session,
            request
                .request_id
                .as_deref()
                .context("service_request_id_required")?,
            &page,
        )?;
        value["replayed"] = json!(replayed);
        Ok::<_, anyhow::Error>(value)
    }
    .await;
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":error.to_string()})),
        )
            .into_response(),
    }
}

fn allowed(state: &AppState, origin: &str) -> bool {
    zork_config::load_config(&state.config.data_root)
        .ok()
        .is_some_and(|c| c.mesh.peers.iter().any(|p| p.origin == origin))
}
pub async fn tunnel(
    state: AppState,
    peer: zork_mesh::control::Peer,
    request: Value,
) -> Result<zork_mesh::control::Reply> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Envelope {
        v: u8,
        request: Open,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Open {
        kind: String,
        id: String,
    }
    let request: Envelope = serde_json::from_value(request)?;
    ensure!(
        request.v == 1 && request.request.kind == "service",
        "invalid_service_request"
    );
    let mut policy = state.db.realtime.listen(crate::realtime::MESH);
    ensure!(allowed(&state, &peer.origin), "mesh_client_not_granted");
    let mesh = state.mesh.get().context("mesh_not_ready")?;
    let (port, stopped) = mesh.services.target(&request.request.id)?;
    check_port(&state, port)?;
    let upstream = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)),
    )
    .await??;
    ensure!(
        !*stopped.borrow() && allowed(&state, &peer.origin),
        "service_access_revoked"
    );
    let (cancel, cancelled) = watch::channel(false);
    tokio::spawn(async move {
        let mut service_changes = stopped.clone();
        let revoked = policy.until(|| {
            (*stopped.borrow()
                || check_port(&state, port).is_err()
                || !allowed(&state, &peer.origin))
            .then_some(())
        });
        tokio::select! {
            _ = cancel.closed() => return,
            _ = service_changes.changed() => {},
            _ = revoked => {},
        }
        cancel.send_replace(true);
    });
    Ok(zork_mesh::control::Reply::Tunnel {
        upstream,
        cancelled,
        guard: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(action: &str, key: &str, fields: Value) -> ToolRequest {
        let mut value = fields;
        value["session_id"] = json!("owner");
        value["action"] = json!(action);
        value["request_id"] = json!(key);
        serde_json::from_value(value).unwrap()
    }
    #[test]
    fn migration_preserves_shared_identity_and_disabling_keeps_the_record() -> Result<()> {
        let root = tempfile::tempdir()?;
        std::fs::create_dir(root.path().join("state"))?;
        let db = rusqlite::Connection::open(root.path().join("state/shared-services.sqlite"))?;
        db.execute_batch("CREATE TABLE shared_services(id TEXT PRIMARY KEY,owner TEXT,name TEXT,port INTEGER,UNIQUE(owner,name)); INSERT INTO shared_services VALUES('legacy','owner','preview',3000);")?;
        drop(db);
        let registry = Registry::open(root.path())?;
        let (port, cancel) = registry.target("legacy")?;
        assert_eq!(port, 3000);
        registry.mutate(&request("unshare", "off", json!({"id":"legacy"})))?;
        assert!(*cancel.borrow());
        assert!(registry.target("legacy").is_err());
        drop(registry);
        let registry = Registry::open(root.path())?;
        assert_eq!(registry.list("owner", "key:node", false)?.len(), 1);
        assert!(registry.list("owner", "key:node", true)?.is_empty());
        registry.mutate(&request("share", "on", json!({"id":"legacy"})))?;
        assert_eq!(registry.target("legacy")?.0, 3000);
        Ok(())
    }
    #[test]
    fn replay_does_not_repeat_restart_or_undo_later_stop_and_rejects_changed_payload() -> Result<()>
    {
        let root = tempfile::tempdir()?;
        let registry = Registry::open(root.path())?;
        let (id, _) = registry.mutate(&request(
            "start",
            "create",
            json!({"name":"app","port":3000,"command":["test-command"],"cwd":root.path()}),
        ))?;
        let restart = request("restart", "restart-once", json!({"id":id}));
        registry.mutate(&restart)?;
        let generation = registry.record("owner", &id)?.generation;
        assert!(registry.mutate(&restart)?.1);
        assert_eq!(registry.record("owner", &id)?.generation, generation);
        registry.mutate(&request("stop", "stop", json!({"id":id})))?;
        drop(registry);
        let registry = Registry::open(root.path())?;
        assert!(registry.mutate(&restart)?.1);
        assert!(!registry.record("owner", &id)?.desired_running);
        assert!(registry
            .mutate(&request("share", "restart-once", json!({"id":id})))
            .is_err());
        let mut other = request("share", "other", json!({"id":id}));
        other.session_id = "other-owner".into();
        assert!(registry.mutate(&other).is_err());
        Ok(())
    }
    #[test]
    fn relative_node_directory_returns_absolute_log_paths() -> Result<()> {
        let directory = tempfile::Builder::new()
            .prefix(".service-root-test-")
            .tempdir_in(".")?;
        let relative = PathBuf::from(directory.path().file_name().unwrap());
        let registry = Registry::open(&relative)?;
        let (id, _) = registry.mutate(&request("start", "create", json!({"name":"app","port":3000,"command":["test-command"],"cwd":directory.path().canonicalize()?})))?;
        let record = registry.record("owner", &id)?;
        let value = record.value("key:node", &registry.root);
        assert!(Path::new(value["logs"]["stdout"].as_str().unwrap()).is_absolute());
        Ok(())
    }
    #[test]
    fn failed_write_does_not_disable_a_live_share() -> Result<()> {
        let root = tempfile::tempdir()?;
        let registry = Registry::open(root.path())?;
        let (id, _) = registry.mutate(&request(
            "attach",
            "attach",
            json!({"name":"app","port":3000}),
        ))?;
        registry.mutate(&request("share", "share", json!({"id":id})))?;
        let (_, cancel) = registry.target(&id)?;
        registry.state.lock().unwrap().store.read_only();
        assert!(registry
            .mutate(&request("unshare", "off", json!({"id":id})))
            .is_err());
        assert!(!*cancel.borrow());
        assert!(registry.record("owner", &id)?.shared);
        Ok(())
    }
}
