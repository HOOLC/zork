use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use store::{ClientStore, QueuedMessage, RemoteNode, SavedNode};
use zork_config::{MeshConfig, MeshPeer};
use zork_mesh::managed;

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Network {
    #[serde(default)]
    pub direct_only: bool,
    pub relay_urls: Option<Vec<String>>,
    pub discovery_url: Option<String>,
    pub relay_quic_port: Option<u16>,
    pub quic_discovery_urls: Option<Vec<String>>,
    pub channel: Option<zork_config::channel::Channel>,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Account {
        operation: relay_account::controller::Action,
    },
    LocalScript {
        operation: local_scripts::Action,
    },
    Adb {
        operation: adb::Action,
    },
    AdbBackgroundService {
        running: bool,
        instance: String,
    },
    ChatFiles {
        operation: chat_files::Action,
    },
    /// Adds a platform-provided private copy to the draft under its display name.
    AttachFile {
        peer: String,
        session: String,
        path: String,
        name: String,
    },
    RemoveFile {
        peer: String,
        session: String,
        id: String,
    },
    NotificationSettings {
        operation: Option<notifications::mobile::Action>,
    },
    NotificationReceipt {
        peer: String,
        id: String,
    },
    TestNotification,
    OpenNotification {
        tag: String,
    },
    ReportView {
        peer: Option<String>,
        session: Option<String>,
        visible: bool,
    },
    HostVisibility {
        visible: bool,
        #[serde(default)]
        generation: u64,
    },
    BackgroundService {
        running: bool,
        instance: String,
    },
    Resources {
        peer: Option<String>,
        query: Option<resources::Inspection>,
    },
    DiagnoseConnections,
    SelectPeer {
        peer: Option<String>,
    },
    RestoreNavigation {
        peer: Option<String>,
    },
    DraftAction {
        peer: String,
        session: String,
        operation: state::DraftAction,
    },
    SubmitDraft {
        peer: String,
        session: String,
        text: String,
    },
    ArchiveChat {
        peer: String,
        chat: String,
        archived: bool,
        expected_message_count: u64,
    },
    NewChat {
        peer: String,
        operation: zork_client_types::new_chat::Action,
    },
    RespondToInteraction {
        peer: String,
        session: String,
        operation: crate::interactions::Command,
    },
    CachedMessages {
        peer: String,
        session: String,
    },
    SettingsAction {
        peer: String,
        operation: settings_actions::SettingsAction,
        #[serde(default)]
        request_id: Option<String>,
    },
    OpenService {
        view_id: String,
        url: String,
    },
    CloseService {
        view_id: String,
    },
    Resume,
    Pause,
    Snapshot,
    Network {
        network: Network,
    },
    SavePeer {
        origin: String,
        name: String,
        address: Option<String>,
    },
    RemovePeer {
        peer: String,
    },
    Read {
        peer: String,
        path: String,
        #[serde(default)]
        cached_only: bool,
    },
    Preferences {
        #[serde(default)]
        theme: Option<preferences::Theme>,
    },
    Settings {
        peer: String,
        #[serde(default)]
        cached_only: bool,
    },
    ModelConnections {
        #[serde(default)]
        cached_only: bool,
    },
    Request {
        peer: String,
        method: String,
        path: String,
        body: Option<Value>,
    },
    Draft {
        peer: String,
        session: String,
        content: String,
    },
    Compose {
        peer: String,
        session: String,
        content: String,
        comments: Vec<comments::DraftComment>,
        #[serde(default)]
        attachments: Vec<comments::TextAttachment>,
        #[serde(default)]
        files: Vec<zork_client_types::files::FileRef>,
        #[serde(default)]
        send: bool,
    },
    Conversation {
        peer: String,
        session: String,
    },
    Enqueue {
        peer: String,
        session: String,
        content: String,
    },
    Withdraw {
        peer: String,
        request_id: String,
    },
    Retry {
        peer: String,
        request_id: String,
    },
    DeleteFailed {
        peer: String,
        request_id: String,
    },
    Flush {
        peer: String,
    },
}

impl Command {
    /// Local edits must never queue behind a slow connection or request.
    pub fn is_local(&self) -> bool {
        matches!(
            self,
            Self::Account { .. }
                | Self::LocalScript { .. }
                | Self::Adb { .. }
                | Self::NotificationSettings { .. }
                | Self::ChatFiles { .. }
                | Self::AttachFile { .. }
                | Self::RemoveFile { .. }
                | Self::TestNotification
                | Self::NotificationReceipt { .. }
                | Self::OpenNotification { .. }
                | Self::ReportView { .. }
                | Self::SelectPeer { .. }
                | Self::RestoreNavigation { .. }
                | Self::DraftAction { .. }
                | Self::SubmitDraft { .. }
                | Self::ArchiveChat { .. }
                | Self::NewChat { .. }
                | Self::RespondToInteraction { .. }
                | Self::CachedMessages { .. }
                | Self::CloseService { .. }
                | Self::Preferences { .. }
                | Self::Draft { .. }
                | Self::Compose { .. }
                | Self::Conversation { .. }
                | Self::Enqueue { .. }
                | Self::Withdraw { .. }
                | Self::Retry { .. }
                | Self::DeleteFailed { .. }
                | Self::Settings {
                    cached_only: true,
                    ..
                }
                | Self::ModelConnections { cached_only: true }
                | Self::Read {
                    cached_only: true,
                    ..
                }
        )
    }
}

/// An independent store handle: no Tokio/transport lock is held during edits.
#[derive(Clone)]
pub struct LocalClient {
    directory: Arc<zork_observe::ValueSource<Value>>,
    account: Arc<relay_account::controller::Controller>,
    data_reset: Arc<data_reset::Controller>,
    local_scripts: Arc<local_scripts::Controller>,
    adb: Arc<adb::Controller>,
    chat_files: Arc<chat_files::Controller>,
    resources: Arc<resources::Resources>,
    services: Arc<services::Views>,
    store: Arc<ClientStore>,
}
impl LocalClient {
    pub fn data_reset(&self) -> Arc<data_reset::Controller> {
        self.data_reset.clone()
    }
    pub fn chat_files(&self) -> Arc<chat_files::Controller> {
        self.chat_files.clone()
    }
    /// Independent observation lane. Opening/reading/waiting never takes the
    /// command executor lock or starts a second device controller.
    pub fn observe(&self, key: subscriptions::Key) -> Result<subscriptions::WireSubscription> {
        if matches!(key, subscriptions::Key::Account) {
            return Ok(subscriptions::WireSubscription::from_account(
                &self.account.source,
            ));
        }
        if matches!(key, subscriptions::Key::DataReset) {
            return Ok(subscriptions::WireSubscription::from_data_reset(
                &self.data_reset.source,
            ));
        }
        if matches!(key, subscriptions::Key::LocalScripts) {
            return Ok(subscriptions::WireSubscription::from_local_scripts(
                &self.local_scripts.source,
            ));
        }
        if matches!(key, subscriptions::Key::Adb) {
            return Ok(subscriptions::WireSubscription::from_adb(self.adb.clone()));
        }
        if matches!(key, subscriptions::Key::ChatFiles) {
            return Ok(subscriptions::WireSubscription::from_chat_files(
                self.chat_files.clone(),
            ));
        }
        if matches!(key, subscriptions::Key::Notifications) {
            return subscriptions::WireSubscription::from_notifications(self.store.clone());
        }
        if matches!(key, subscriptions::Key::Navigation) {
            return Ok(subscriptions::WireSubscription::from_navigation(
                self.directory.clone(),
                self.store.clone(),
            ));
        }
        if let subscriptions::Key::Resources { peer, kind, query } = key {
            if let Some(peer) = &peer {
                self.peer(peer)?;
            }
            return subscriptions::WireSubscription::from_resources(
                self.resources.clone(),
                self.store.clone(),
                peer,
                kind,
                query,
            );
        }
        if matches!(key, subscriptions::Key::Directory) {
            return Ok(subscriptions::WireSubscription::from_directory(
                &self.directory,
            ));
        }

        self.peer(key.peer())?;
        let device = self
            .device_state(key.peer())
            .context("客户端连接已暂停，请重新连接")?;
        let conversation = match &key {
            subscriptions::Key::Conversation {
                session: Some(session),
                ..
            }
            | subscriptions::Key::History { session, .. } => {
                valid_session(session)?;
                Some(device.conversation(session))
            }
            _ => None,
        };
        let delivery = matches!(key, subscriptions::Key::Conversation { .. });
        let history = matches!(key, subscriptions::Key::History { .. });
        let observer =
            subscriptions::WireSubscription::from_device(key, device.clone(), self.store.clone())?;
        if delivery {
            device.start_delivery();
        }
        if let Some(conversation) = conversation {
            conversation.start();
            if history {
                conversation.history().load(false);
            }
        }
        Ok(observer)
    }
    fn device_state(&self, peer: &str) -> Option<Arc<state::Device>> {
        self.store
            .1
            .lock()
            .unwrap()
            .get(peer)
            .and_then(std::sync::Weak::upgrade)
    }

    fn peer(&self, peer: &str) -> Result<SavedNode> {
        find_peer(&self.store, peer)
    }

    /// Bounded image bytes for an inline thumbnail: a draft file when `message`
    /// is absent, otherwise a file of that cached message.
    pub async fn file_preview(
        &self,
        peer: &str,
        session: &str,
        message: Option<&str>,
        file: &str,
    ) -> Result<Vec<u8>> {
        self.peer(peer)?;
        valid_session(session)?;
        let device = self
            .device_state(peer)
            .context("客户端连接已暂停，请重新连接")?;
        match message {
            None => device.draft_file_bytes(session, file),
            Some(message) => {
                chat_files::inline_bytes(&self.store, device, peer, session, message, file).await
            }
        }
    }

    pub fn execute(&self, command: Command) -> Result<Value> {
        match command {
            Command::Account { operation } => {
                self.account.submit(operation)?;
                Ok(json!({}))
            }
            Command::ChatFiles { operation } => {
                let device = if let chat_files::Action::Open { peer, session, .. } = &operation {
                    self.peer(peer)?;
                    valid_session(session)?;
                    self.device_state(peer)
                } else { None };
                self.chat_files.apply(operation, device)?;
                Ok(json!({}))
            }
            Command::AttachFile {
                peer,
                session,
                path,
                name,
            } => {
                self.peer(&peer)?;
                valid_session(&session)?;
                let device = self
                    .device_state(&peer)
                    .context("请先打开对话，再添加文件")?;
                let file = device.attach_named_path(&session, Path::new(&path), &name)?;
                Ok(serde_json::to_value(file_io::view(&file))?)
            }
            Command::RemoveFile { peer, session, id } => {
                self.peer(&peer)?;
                valid_session(&session)?;
                let device = self
                    .device_state(&peer)
                    .context("请先打开对话，再移除文件")?;
                device.remove_file(&session, &id)?;
                Ok(json!({}))
            }
            Command::LocalScript { operation } => self.local_scripts.apply(operation),
            Command::Adb { operation } => self.adb.execute(operation),
            Command::NotificationSettings { operation } => {
                notifications::mobile::apply(&self.store, operation)
            }
            Command::TestNotification => notifications::mobile::test(&self.store),
            Command::OpenNotification { tag } => notifications::mobile::resolve(&self.store, &tag),
            Command::ReportView {
                peer,
                session,
                visible,
            } => {
                if let Some(peer) = &peer {
                    self.peer(peer)?;
                }
                if let Some(session) = &session {
                    valid_session(session)?;
                }
                self.chat_files.leave_conversation(peer.as_deref(), session.as_deref());
                let devices = self
                    .store
                    .1
                    .lock()
                    .unwrap()
                    .iter()
                    .filter_map(|(id, source)| source.upgrade().map(|d| (id.clone(), d)))
                    .collect::<Vec<_>>();
                for (id, device) in devices {
                    let selected = peer.as_ref() == Some(&id);
                    device.report_view(
                        if selected { session.clone() } else { None },
                        selected && visible,
                        false,
                    );
                }
                Ok(json!({}))
            }
            Command::NotificationReceipt { peer, id } => {
                self.peer(&peer)?;
                anyhow::ensure!(!self.store.replica_revoked(&peer)?, "设备访问权限已撤销");
                if let Some(device) = self.device_state(&peer) {
                    device.acknowledge_notification(&id)?;
                } else {
                    let mut ledger = self
                        .store
                        .get::<notifications::Ledger>(&peer, notifications::KEY)?
                        .unwrap_or_default();
                    ledger.filter(
                        &peer,
                        None,
                        &notifications::preferences(&self.store)?,
                        store::delivery_now_ms(),
                    );
                    ledger.acknowledge(&id);
                    self.store.put(&peer, notifications::KEY, &ledger)?;
                }
                Ok(json!({}))
            }
            Command::SelectPeer { peer } => {
                if let Some(peer) = &peer {
                    self.peer(peer)?;
                }
                self.store.put("device", "last-node", &peer)?;
                Ok(json!({}))
            }
            Command::RestoreNavigation { peer } => {
                if self.store.get::<Value>("device", "last-node")?.is_none() {
                    let nodes = self.store.nodes()?;
                    let selected = peer.filter(|id| nodes.iter().any(|n| n.id == *id));
                    self.store.put("device", "last-node", &selected)?;
                }
                Ok(json!({}))
            }
            Command::DraftAction {
                peer,
                session,
                operation,
            } => {
                self.peer(&peer)?;
                valid_session(&session)?;
                let devices = self.store.1.lock().unwrap();
                if let Some(device) = devices.get(&peer).and_then(std::sync::Weak::upgrade) {
                    device.edit_draft_action(&session, operation)?;
                } else {
                    self.store.edit_draft(&peer, &session, operation)?;
                }
                Ok(json!({}))
            }
            Command::SubmitDraft {
                peer,
                session,
                text,
            } => {
                self.peer(&peer)?;
                valid_session(&session)?;
                let devices = self.store.1.lock().unwrap();
                if let Some(device) = devices.get(&peer).and_then(std::sync::Weak::upgrade) {
                    return Ok(serde_json::to_value(device.submit_draft(&session, &text)?)?);
                }
                Ok(serde_json::to_value(
                    self.store.submit_current_draft(&peer, &session, text)?,
                )?)
            }
            Command::ArchiveChat { peer, chat, archived, expected_message_count } => {
                self.peer(&peer)?;
                self.device_state(&peer).context("客户端连接已暂停，请重新连接")?.set_chat_archived(&chat, archived, expected_message_count)?;
                Ok(json!({}))
            }
            Command::NewChat {peer,operation} => {
                self.peer(&peer)?;
                let device=self.device_state(&peer).context("客户端连接已暂停，请重新连接")?;
                device.new_chat().apply(operation)?;
                Ok(json!({}))
            }
            Command::CachedMessages { peer, session } => {
                self.peer(&peer)?;
                valid_session(&session)?;
                ensure!(!self.store.replica_revoked(&peer)?, "设备访问权限已撤销");
                Ok(
                    json!({"snapshot":cached_message_snapshot(&self.store, &peer, &session)?,"cached":true}),
                )
            }
            Command::RespondToInteraction {
                peer,
                session,
                operation,
            } => {
                self.peer(&peer)?;
                valid_session(&session)?;
                if let crate::interactions::Command::Activate {
                    message_id,
                    choice,
                    values,
                } = &operation
                {
                    if choice == "run_local_script" {
                        ensure!(
                            values.is_empty(),
                            "Script activation does not accept parameters"
                        );
                        return self
                            .local_scripts
                            .start_message(&peer, &session, message_id);
                    }
                }
                let device = self
                    .store
                    .1
                    .lock()
                    .unwrap()
                    .get(&peer)
                    .and_then(std::sync::Weak::upgrade)
                    .context("Open the conversation before responding")?;
                device
                    .conversation(&session)
                    .respond_to_interaction(operation)?;
                Ok(json!({}))
            }
            Command::CloseService { view_id } => {
                self.services.close(&view_id);
                Ok(json!({"ok":true}))
            }
            Command::Preferences { theme } => {
                let value = match theme {
                    Some(theme) => preferences::save_theme(&self.store, theme)?,
                    None => preferences::read(&self.store),
                };
                Ok(serde_json::to_value(value)?)
            }

            Command::Draft {
                peer,
                session,
                content,
            } => self.execute(Command::DraftAction {
                peer,
                session,
                operation: state::DraftAction::Edit { text: content },
            }),
            Command::Compose {
                peer,
                session,
                content,
                comments,
                attachments,
                files,
                send,
            } => {
                self.peer(&peer)?;
                valid_session(&session)?;
                let draft = state::Draft {
                    text: content,
                    comments,
                    attachments,
                    files,
                };
                let payload = if send {
                    draft.submission(&draft.text)
                } else {
                    draft.encoded()
                };
                ensure!(
                    zork_client_types::files::valid(&draft.files),
                    "invalid attachments"
                );
                valid_content(&payload, !send)?;
                if send {
                    if let Some(device) = self.device_state(&peer) {
                        return Ok(serde_json::to_value(device.enqueue(&session, payload)?)?);
                    }
                    let queued = QueuedMessage {
                        request_id: ulid::Ulid::new().to_string(),
                        session_id: session,
                        content: payload,
                        attempted: false,
                        sent_at_ms: crate::store::delivery_now_ms(),
                        ..Default::default()
                    };
                    let queued = self.store.enqueue_and_clear_draft(&peer, &queued)?;
                    Ok(serde_json::to_value(queued)?)
                } else {
                    if let Some(device) = self.device_state(&peer) {
                        device.edit_document(&session, draft)?;
                        return Ok(json!({}));
                    }
                    self.store
                        .put(&peer, &format!("draft:{session}"), &payload)?;
                    Ok(json!({}))
                }
            }
            Command::Conversation { peer, session } => {
                self.peer(&peer)?;
                valid_session(&session)?;
                let raw = self
                    .store
                    .get::<String>(&peer, &format!("draft:{session}"))?
                    .unwrap_or_default();
                let (raw, files) = zork_client_types::files::decode(&raw).unwrap_or((raw, vec![]));
                let (draft, comments, attachments) =
                    comments::decode_document(&raw).unwrap_or((raw, vec![], vec![]));
                let outbox: Vec<_> = self
                    .store
                    .outbox(&peer)?
                    .into_iter()
                    .filter(|m| m.session_id == session)
                    .map(|m| {
                        let mut value = serde_json::to_value(&m).expect("queued message");
                        value["delivery_status"] = json!(m.delivery_status());
                        conversation::project_payload(&mut value);
                        value
                    })
                    .collect();
                Ok(
                    json!({"draft":draft,"comments":comments,"attachments":attachments,"file_views":file_io::views(&files),"files":files,"outbox":outbox}),
                )
            }
            Command::Enqueue {
                peer,
                session,
                content,
            } => {
                self.peer(&peer)?;
                valid_session(&session)?;
                valid_content(&content, false)?;
                if let Some(device) = self.device_state(&peer) {
                    return Ok(serde_json::to_value(device.enqueue(&session, content)?)?);
                }
                let queued = QueuedMessage {
                    request_id: ulid::Ulid::new().to_string(),
                    session_id: session,
                    content,
                    attempted: false,
                    sent_at_ms: crate::store::delivery_now_ms(),
                    ..Default::default()
                };
                let queued = self.store.enqueue_and_clear_draft(&peer, &queued)?;
                Ok(serde_json::to_value(queued)?)
            }
            Command::Retry { peer, request_id } => {
                self.peer(&peer)?;
                if let Some(device) = self.device_state(&peer) {
                    device.retry_delivery(&request_id)?;
                } else {
                    self.store.retry_delivery(&peer, &request_id)?;
                }
                Ok(json!({}))
            }
            Command::DeleteFailed { peer, request_id } => {
                self.peer(&peer)?;
                if let Some(device) = self.device_state(&peer) {
                    device.delete_failed_delivery(&request_id)?;
                } else {
                    self.store.delete_failed(&peer, &request_id)?;
                }
                Ok(json!({}))
            }
            Command::Withdraw { peer, request_id } => {
                self.peer(&peer)?;
                let message = if let Some(device) = self.device_state(&peer) {
                    device.withdraw_delivery(&request_id)?
                } else {
                    self.store.withdraw_to_draft(&peer, &request_id)?
                };
                Ok(json!({"withdrawn":message}))
            }
            Command::Settings {
                peer,
                cached_only: true,
            } => {
                self.peer(&peer)?;
                settings::snapshot(&self.store, &peer)
            }
            Command::ModelConnections { cached_only: true } => {
                model_connections::cached(&self.store)
            }
            Command::Read {
                peer,
                path,
                cached_only: true,
            } => {
                self.peer(&peer)?;
                ensure!(!self.store.replica_revoked(&peer)?, "设备访问权限已撤销");
                if let Some(session) = message_session(&path) {
                    return Ok(
                        json!({"snapshot":cached_message_snapshot(&self.store, &peer, session)?,"cached":true}),
                    );
                }
                if let Some(body) = catalog::read_response(&self.store, &peer, &path)? {
                    return Ok(json!({"snapshot":{"body":body},"cached":true}));
                }
                let cached = self.store.get::<Value>(&peer, &format!("http:{path}"))?;
                Ok(json!({"snapshot":cached,"cached":true}))
            }
            _ => anyhow::bail!("operation requires a network client"),
        }
    }
}
fn find_peer(store: &ClientStore, peer: &str) -> Result<SavedNode> {
    store
        .nodes()?
        .into_iter()
        .find(|node| node.id == peer)
        .context("设备尚未添加")
}

pub struct Client {
    account: Arc<relay_account::controller::Controller>,
    data_reset: Arc<data_reset::Controller>,
    local_scripts: Arc<local_scripts::Controller>,
    adb: Arc<adb::Controller>,
    adb_background_service: Option<String>,
    chat_files: Arc<chat_files::Controller>,
    foreground: bool,
    host_generation: u64,
    background_service: Option<String>,
    resources: Arc<resources::Resources>,
    directory: Arc<client_directory::Directory>,
    services: Arc<services::Views>,
    root: PathBuf,
    store: Arc<ClientStore>,
    runtime: Option<transport::Runtime>,
    account_devices: Option<zork_notify::Task<()>>,
}

impl Drop for Client {
    fn drop(&mut self) {
        self.adb.pause();
    }
}

impl Client {
    pub fn local(&self) -> LocalClient {
        LocalClient {
            directory: self.directory.source.clone(),
            account: self.account.clone(),
            data_reset: self.data_reset.clone(),
            local_scripts: self.local_scripts.clone(),
            adb: self.adb.clone(),
            chat_files: self.chat_files.clone(),
            resources: self.resources.clone(),
            services: self.services.clone(),
            store: self.store.clone(),
        }
    }
    pub fn open(root: &Path) -> Result<Self> {
        ensure!(root.is_absolute(), "client data directory must be absolute");
        let channel = zork_config::channel::activate_for_data(root)?;
        zork_config::channel::claim(root, channel)?;
        let store = Arc::new(ClientStore::open(root)?);
        settings_actions::recover_operations(&store)?;
        let adb = adb::Controller::new(store.clone())?;
        let resources = resources::Resources::new(vec![]);
        let chat_files = chat_files::Controller::new(store.clone());
        let directory = client_directory::Directory::new(
            store.clone(),
            resources.clone(),
            chat_files.clone(),
            adb.clone(),
        )?;
        Ok(Self {
            account: relay_account::controller::Controller::open(root)?,
            data_reset: Arc::new(data_reset::Controller::default()),
            local_scripts: local_scripts::Controller::new(store.clone()),
            adb,
            adb_background_service: None,
            chat_files: chat_files.clone(),
            foreground: false,
            host_generation: 0,
            background_service: None,
            resources,
            directory,
            root: root.to_owned(),
            store,
            runtime: None,
            account_devices: None,
            services: Default::default(),
        })
    }

    fn config(&self, network: &Network) -> Result<MeshConfig> {
        let mut config = MeshConfig {
            enabled: true,
            offline: network.direct_only,
            // Mobile peers must be reachable over LAN, including in direct-only mode.
            bind: None,
            relay_urls: network.relay_urls.clone(),
            discovery_url: network.discovery_url.clone(),
            relay_quic_port: network.relay_quic_port,
            quic_discovery_urls: network.quic_discovery_urls.clone(),
            channel: network.channel,
            peers: self
                .store
                .nodes()?
                .into_iter()
                .filter_map(|n| {
                    n.mesh.map(|remote| MeshPeer {
                        routes: remote.routes,
                        origin: remote.origin,
                        name: n.name,
                        addr: remote.addr,
                        execute: vec![],
                        client: false,
                        collaborate: false,
                    })
                })
                .collect(),
            ..Default::default()
        };
        if !config.offline {
            zork_config::services::ServicesConfig::load_for_data_root(
                &zork_config::relay_account::resolve_root(&self.root)?,
            )?
            .apply_defaults(&mut config)?;
        }
        managed::validate(&config)?;
        Ok(config)
    }

    async fn reconcile_host(&mut self) -> Result<()> {
        let requested = notifications::mobile::settings(&self.store)?["service_requested"] == true;
        if self.foreground
            || (requested && self.background_service.is_some())
            || (self.adb.requested() && self.adb_background_service.is_some())
        {
            self.resume().await
        } else {
            self.pause().await
        }
    }

    async fn resume(&mut self) -> Result<()> {
        if self.runtime.as_ref().is_some_and(|r| r.is_finished()) {
            self.pause().await?;
        }
        if self.runtime.is_none() {
            self.directory
                .set_mesh_readiness(device_status::MeshReadiness::Preparing);
            let started = async {
                let network = self.store.get::<Network>("device", "network")?.unwrap_or_default();
                let config = self.config(&network)?;
                let (mut runtime, identity) = transport::start(&self.root, &config).await?;
                if let Err(error) = self.store.put("device", "identity", &identity) {
                    let _ = runtime.shutdown().await;
                    return Err(error.into());
                }
                Ok(runtime)
            }
            .await;
            let runtime = match started {
                Ok(started) => started,
                Err(error) => {
                    self.directory
                        .set_mesh_readiness(device_status::MeshReadiness::Failed(format!(
                            "{error:#}"
                        )));
                    return Err(error);
                }
            };
            self.account_devices = Some(relay_account::devices::start_client(&self.root, runtime.node(), self.store.clone())?);
            self.runtime = Some(runtime);
        }
        self.watch_devices().await?;
        self.adb.start(self.node()?);
        Ok(())
    }

    async fn watch_devices(&self) -> Result<()> {
        if let Some(runtime) = &self.runtime {
            self.directory.start(runtime.node()).await?;
        }
        Ok(())
    }

    pub async fn pause(&mut self) -> Result<()> {
        self.account_devices.take();
        self.directory
            .set_mesh_readiness(device_status::MeshReadiness::Stopping);
        self.adb.pause();
        self.directory.stop().await;
        self.chat_files.pause();
        self.services.clear();
        if let Some(mut runtime) = self.runtime.take() {
            runtime.shutdown().await?;
        }
        self.directory
            .set_mesh_readiness(device_status::MeshReadiness::Stopped);
        Ok(())
    }

    fn node(&self) -> Result<zork_mesh::node::MeshNode> {
        let runtime = self.runtime.as_ref().context("客户端连接已暂停")?;
        ensure!(!runtime.is_finished(), "客户端连接已结束，请重新连接");
        Ok(runtime.node())
    }

    fn peer(&self, peer: &str) -> Result<SavedNode> {
        find_peer(&self.store, peer)
    }

    fn snapshot(&self) -> Result<Value> {
        Ok(json!({"identity":self.store.get::<String>("device","identity")?,
            "running":self.runtime.as_ref().is_some_and(|r| !r.is_finished()),
            "nodes":self.store.nodes()?,
            "selected_peer":self.store.get::<Option<String>>("device","last-node")?.flatten(),
            "network":self.store.get::<Network>("device","network")?.unwrap_or_default()}))
    }

    async fn request(
        &self,
        peer: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        self.peer(peer)?;
        ensure!(
            matches!(method, "GET" | "POST" | "PUT" | "PATCH" | "DELETE"),
            "unsupported request method"
        );
        ensure!(
            path.starts_with("/v1/") && !path.contains('#') && path.len() < 4096,
            "invalid Station path"
        );
        self.directory.refresh().await?;
        let client = self.station(peer)?;
        let device = state::Device::open(
            client.clone(),
            Some((self.store.clone(), peer.into())),
            true,
        );
        if let Some(result) = device.mutate_request(method, path, body.as_ref()).await? {
            return Ok(result);
        }
        let result = client
            .node_request(
                reqwest::Method::from_bytes(method.as_bytes())?,
                path.into(),
                body,
            )
            .await?;
        device_metadata::record(&self.store, peer, path, &result)?;
        if method != "GET"
            && (path == "/v1/node/name"
                || path.starts_with("/v1/node/profiles/")
                || path.starts_with("/v1/node/agents")
                || path.starts_with("/v1/tasks/"))
        {
            device.refresh(state::Domains::ALL).await;
        }
        Ok(result)
    }

    fn station(&self, peer: &str) -> Result<Arc<api::StationClient>> {
        self.directory.station(peer)
    }

    /// Every JNI operation has a bounded payload. Stable messages are committed
    /// before network I/O and never acquire a new ID merely because of a retry.
    pub async fn execute(&mut self, command: Command) -> Result<Value> {
        match command {
            Command::Account { operation } => {
                self.account.submit(operation)?;
                Ok(json!({}))
            }
            command @ Command::ChatFiles { .. } => self.local().execute(command),
            Command::LocalScript { operation } => self.local_scripts.apply(operation),
            Command::Adb { operation } => self.adb.execute(operation),
            Command::AdbBackgroundService { running, instance } => {
                crate::model_edit::valid_id(&instance)?;
                if running {
                    self.adb_background_service = Some(instance);
                } else if self.adb_background_service.as_ref() == Some(&instance) {
                    self.adb_background_service = None;
                }
                self.reconcile_host().await?;
                let mut value = self.adb.snapshot();
                value["network_active"] = json!(self.runtime.is_some());
                Ok(value)
            }
            Command::HostVisibility {
                visible,
                generation,
            } => {
                if generation >= self.host_generation {
                    self.host_generation = generation;
                    self.foreground = visible;
                    if !visible {
                        for device in self.directory.devices() {
                            device.report_view(None, false, false);
                        }
                    }
                    self.reconcile_host().await?;
                }
                let mut value = self.snapshot()?;
                value["network_active"] = json!(self.runtime.is_some());
                value["host_generation"] = json!(self.host_generation);
                value["host_visible"] = json!(self.foreground);
                Ok(value)
            }
            Command::BackgroundService { running, instance } => {
                crate::model_edit::valid_id(&instance)?;
                if running {
                    self.background_service = Some(instance);
                } else if self.background_service.as_ref() == Some(&instance) {
                    self.background_service = None;
                }
                self.reconcile_host().await?;
                let mut value = notifications::mobile::settings(&self.store)?;
                value["network_active"] = json!(self.runtime.is_some());
                Ok(value)
            }
            Command::Resources { peer, query } => {
                if let Some(peer) = &peer {
                    self.peer(peer)?;
                }
                anyhow::ensure!(query.is_none() || peer.is_some(), "请选择所属设备");
                self.directory.refresh().await?;
                let resources = self.resources.clone();
                tokio::spawn(async move {
                    if let Some(query) = query {
                        resources.inspect(peer.as_deref().unwrap(), query).await;
                    } else {
                        resources.refresh_scope(peer.as_deref()).await;
                    }
                });
                Ok(json!({}))
            }
            Command::DiagnoseConnections => {
                let connections = self
                    .store
                    .nodes()?
                    .into_iter()
                    .map(|node| {
                        let station = self.station(&node.id);
                        (node.name, station)
                    })
                    .collect::<Vec<_>>();
                let items = futures_util::future::join_all(connections.into_iter().map(
                    |(name, station)| async move {
                        let reachable = match station {
                            Ok(client) => client
                                .node_request(http::Method::GET, "/v1/node/info".into(), None)
                                .await
                                .is_ok(),
                            Err(_) => false,
                        };
                        json!({"name":name,"reachable":reachable})
                    },
                ))
                .await;
                Ok(json!({"items":items}))
            }
            Command::SettingsAction {
                peer,
                operation,
                request_id,
            } => {
                self.tracked_settings_action(peer, operation, request_id)
                    .await
            }
            Command::OpenService { view_id, url } => self.open_service(view_id, url).await,
            Command::CloseService { view_id } => {
                self.services.close(&view_id);
                Ok(json!({"ok":true}))
            }
            Command::Resume => {
                self.resume().await?;
                self.snapshot()
            }
            Command::Pause => {
                self.pause().await?;
                self.snapshot()
            }
            Command::Snapshot => {
                self.watch_devices().await?;
                self.snapshot()
            }
            Command::Network { network } => {
                self.config(&network)?;
                self.pause().await?;
                self.store.put("device", "network", &network)?;
                self.resume().await?;
                self.snapshot()
            }
            Command::SavePeer {
                origin,
                name,
                address,
            } => {
                let name = name.trim();
                ensure!(!name.is_empty() && name.len() <= 128, "请输入设备名称");
                let address = address.filter(|a| !a.trim().is_empty());
                if let Some(addr) = &address {
                    addr.parse::<std::net::SocketAddr>()
                        .context("地址格式为 IP:端口")?;
                }
                let peer = MeshPeer {
                    routes: None,
                    origin: origin.clone(),
                    name: name.into(),
                    addr: address.clone(),
                    execute: vec![],
                    client: false,
                    collaborate: false,
                };
                managed::validate(&MeshConfig {
                    peers: vec![peer],
                    ..Default::default()
                })?;
                let nodes = self.store.nodes()?;
                ensure!(
                    nodes.len() < 16 || nodes.iter().any(|n| n.id == origin),
                    "最多添加 16 台设备"
                );
                self.node()?
                    .trust(&origin, name, address.as_deref())
                    .await?;
                self.store.save_node(&SavedNode {
                    id: origin.clone(),
                    name: name.into(),
                    url: String::new(),
                    token: None,
                    local: false,
                    mesh: Some(RemoteNode {
                        routes: None,
                        origin,
                        addr: address,
                    }),
                    group: None,
                })?;
                self.watch_devices().await?;
                self.snapshot()
            }
            Command::RemovePeer { peer } => {
                self.chat_files.revoke(&peer);
                let removed = self.peer(&peer)?;
                if let Some(remote) = removed.mesh {
                    self.services.remove_peer(&remote.origin);
                }
                if let Ok(node) = self.node() {
                    node.untrust(&peer).await?;
                }
                if let Some(device) = self.directory.device(&peer) {
                    device.stop_sync();
                    device.revoke_replica_access()?;
                } else {
                    self.store.revoke_replica(&peer)?;
                }
                self.store.remove_node(&peer)?;
                self.adb.peer_revoked(&peer);
                self.directory.refresh().await?;
                self.snapshot()
            }
            Command::Settings { peer, cached_only } => {
                self.peer(&peer)?;
                if cached_only {
                    return settings::snapshot(&self.store, &peer);
                }
                settings::refresh(self.station(&peer)?, self.store.clone(), &peer).await
            }
            Command::ModelConnections { cached_only } => {
                if cached_only {
                    return model_connections::cached(&self.store);
                }
                self.directory.refresh().await?;
                model_connections::refresh(self.store.clone(), |peer| self.station(peer)).await
            }
            Command::Read {
                peer,
                path,
                cached_only,
            } => {
                self.peer(&peer)?;
                if let Some(session) = message_session(&path) {
                    let page = self.store.cached_messages(&peer, session, None, 100)?;
                    let anchor =
                        page.as_ref()
                            .and_then(|page| page.items.last())
                            .and_then(|message| {
                                let api::TranscriptMessage::Message { metadata, .. } = message;
                                metadata.id.clone()
                            });
                    let submissions = self.store.configuration_submissions(
                        &peer,
                        session,
                        self.store.replica_generation(&peer)?,
                    )?;
                    let cached = page
                        .map(|page| message_snapshot(page, &submissions))
                        .transpose()?;
                    if cached_only {
                        return Ok(json!({"snapshot":cached,"cached":true}));
                    }
                    let generation = self.store.replica_generation(&peer)?;
                    let result: Result<_> = async {
                        let page = self
                            .station(&peer)?
                            .catch_up_messages(session, anchor.as_deref(), 100)
                            .await?;
                        self.store
                            .cache_message_page_at(&peer, session, &page, None, generation)?;
                        cached_message_snapshot(&self.store, &peer, session)
                    }
                    .await;
                    return match result {
                        Ok(snapshot) => Ok(json!({"snapshot":snapshot,"cached":false})),
                        Err(error) => {
                            Ok(json!({"snapshot":cached,"cached":true,"error":error.to_string()}))
                        }
                    };
                }
                if catalog::Catalog::supports(&path) {
                    if cached_only {
                        if let Some(body) = catalog::read_response(&self.store, &peer, &path)? {
                            return Ok(json!({"snapshot":{"body":body},"cached":true}));
                        }
                    } else {
                        let device = state::Device::open(
                            self.station(&peer)?,
                            Some((self.store.clone(), peer.clone())),
                            true,
                        );
                        device.refresh(state::Domains::ALL).await;
                        if let Some(body) = device.replica_response(&path)? {
                            let status = device.snapshot();
                            return Ok(
                                json!({"snapshot":{"body":body,"fetched_at":status.confirmed_at_ms},"cached":status.connection_error.is_some(),"error":status.connection_error}),
                            );
                        }
                    }
                }
                let key = format!("http:{path}");
                let cached = self.store.get::<Value>(&peer, &key)?;
                if cached_only {
                    return Ok(json!({"snapshot":cached,"cached":true}));
                }
                match self.request(&peer, "GET", &path, None).await {
                    Ok(body) => {
                        let snapshot = json!({"body":body,"fetched_at":now()});
                        self.store.put(&peer, &key, &snapshot)?;
                        Ok(json!({"snapshot":snapshot,"cached":false}))
                    }
                    Err(error) => {
                        Ok(json!({"snapshot":cached,"cached":true,"error":error.to_string()}))
                    }
                }
            }
            Command::Request {
                peer,
                method,
                path,
                body,
            } => {
                let result = self.request(&peer, &method, &path, body).await?;
                if method == "POST" && path.ends_with("/cancel") {
                    if let Some(id) = path
                        .strip_prefix("/v1/im/sessions/")
                        .and_then(|p| p.strip_suffix("/cancel"))
                    {
                        if let Some(device) = self.directory.device(&peer) {
                            device.conversation(id).stopping();
                        }
                    }
                }
                Ok(result)
            }
            command @ (Command::SelectPeer { .. }
            | Command::RestoreNavigation { .. }
            | Command::DraftAction { .. }
            | Command::AttachFile { .. }
            | Command::RemoveFile { .. }
            | Command::SubmitDraft { .. }
            | Command::ArchiveChat { .. }
            | Command::NewChat { .. }
            | Command::RespondToInteraction { .. }
            | Command::CachedMessages { .. }
            | Command::Preferences { .. }
            | Command::NotificationSettings { .. }
            | Command::TestNotification
            | Command::NotificationReceipt { .. }
            | Command::OpenNotification { .. }
            | Command::ReportView { .. }
            | Command::Compose { .. }
            | Command::Draft { .. }
            | Command::Conversation { .. }
            | Command::Enqueue { .. }
            | Command::Withdraw { .. }
            | Command::Retry { .. }
            | Command::DeleteFailed { .. }) => self.local().execute(command),
            Command::Flush { peer } => {
                self.peer(&peer)?;
                let client = self.station(&peer)?;
                let report = delivery::flush(&client, &self.store, &peer).await;
                let mut result = serde_json::to_value(report)?;
                result["outbox"] = serde_json::to_value(self.store.outbox(&peer)?)?;
                Ok(result)
            }
        }
    }
}

fn message_session(path: &str) -> Option<&str> {
    let session = path
        .strip_prefix("/v1/im/sessions/")?
        .strip_suffix("/messages")?;
    valid_session(session).ok()?;
    Some(session)
}

fn message_snapshot(
    page: api::MessagePage,
    submissions: &std::collections::HashMap<String, interactions::Submission>,
) -> Result<Value> {
    let tail = page.items.last().and_then(|item| {
        let api::TranscriptMessage::Message { metadata, .. } = item;
        metadata.id.clone()
    });
    let mut items = Vec::new();
    for item in &page.items {
        let api::TranscriptMessage::Message { metadata, .. } = item;
        if interactions::result(metadata).is_some() {
            continue;
        }
        let mut message = serde_json::to_value(item)?;
        conversation::project_payload(&mut message);
        if let Some(card) = interactions::view(
            metadata,
            metadata.id.as_ref().and_then(|id| submissions.get(id)),
            &Default::default(),
        ) {
            message["interaction_card"] = json!(card);
        }
        items.push(message);
    }
    Ok(json!({"body":{"items":items,"older_cursor":page.older_cursor,"source_tail":tail}}))
}

fn cached_message_snapshot(
    store: &store::ClientStore,
    peer: &str,
    session: &str,
) -> Result<Option<Value>> {
    let generation = store.replica_generation(peer)?;
    let submissions = store.configuration_submissions(peer, session, generation)?;
    store
        .cached_messages_at(peer, session, None, 100, generation)?
        .map(|page| message_snapshot(page, &submissions))
        .transpose()
}

fn valid_session(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.len() <= 160
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
        "invalid conversation ID"
    );
    Ok(())
}
fn valid_content(content: &str, empty: bool) -> Result<()> {
    let decoded = zork_client_types::files::decode(content);
    let text = decoded.as_ref().map_or(content, |(text, _)| text.as_str());
    ensure!(
        text.len() <= zork_client_types::files::MAX_FILE_BYTES,
        "消息正文超过 300 MiB，无法作为单个文件发送"
    );
    ensure!(empty || !content.trim().is_empty(), "消息不能为空");
    Ok(())
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add_peer(client: &Client) -> String {
        let peer = format!("key:{}", "y".repeat(52));
        client
            .store
            .save_node(&SavedNode {
                id: peer.clone(),
                name: "test".into(),
                url: String::new(),
                token: None,
                local: false,
                group: None,
                mesh: Some(RemoteNode {
                    routes: None,
                    origin: peer.clone(),
                    addr: None,
                }),
            })
            .unwrap();
        peer
    }

    #[tokio::test]
    async fn picked_copies_attach_preview_and_remove_through_commands() -> Result<()> {
        let root = tempfile::tempdir()?;
        let mut client = Client::open(root.path())?;
        let peer = add_peer(&client);
        let copy = root.path().join("pick-1.tmp");
        std::fs::write(&copy, b"\x89PNG picked")?;
        let attach = || Command::AttachFile {
            peer: peer.clone(),
            session: "chat".into(),
            path: copy.to_string_lossy().into_owned(),
            name: "截图.png".into(),
        };
        ensure!(client.execute(attach()).await.is_err(), "no open conversation");
        let _device = state::Device::open(
            Arc::new(api::StationClient::new("http://127.0.0.1:9", None)),
            Some((client.store.clone(), peer.clone())),
            true,
        );
        let view = client.execute(attach()).await?;
        ensure!(view["name"] == "截图.png" && view["kind"] == "image" && view["thumbnail"] == true);
        let id = view["id"].as_str().unwrap().to_owned();
        std::fs::remove_file(&copy)?;
        fn send<T: Send>(value: T) -> T {
            value
        }
        let local = client.local();
        let bytes = send(local.file_preview(&peer, "chat", None, &id)).await?;
        ensure!(bytes == b"\x89PNG picked");
        let conversation = client
            .execute(Command::Conversation {
                peer: peer.clone(),
                session: "chat".into(),
            })
            .await?;
        ensure!(conversation["file_views"][0]["badge"] == "PNG");
        client
            .execute(Command::RemoveFile {
                peer: peer.clone(),
                session: "chat".into(),
                id: id.clone(),
            })
            .await?;
        ensure!(local.file_preview(&peer, "chat", None, &id).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn every_unobserved_send_entry_returns_the_persisted_text_file() -> Result<()> {
        let root = tempfile::tempdir()?;
        let mut client = Client::open(root.path())?;
        let peer = add_peer(&client);
        let text = format!("  {}\n", "正文 🐈\n".repeat(6000));
        for entry in 0..3 {
            client
                .execute(Command::Draft {
                    peer: peer.clone(),
                    session: "chat".into(),
                    content: text.clone(),
                })
                .await?;
            let command = match entry {
                0 => Command::SubmitDraft {
                    peer: peer.clone(),
                    session: "chat".into(),
                    text: text.clone(),
                },
                1 => Command::Enqueue {
                    peer: peer.clone(),
                    session: "chat".into(),
                    content: text.clone(),
                },
                _ => Command::Compose {
                    peer: peer.clone(),
                    session: "chat".into(),
                    content: text.clone(),
                    comments: vec![],
                    attachments: vec![],
                    files: vec![],
                    send: true,
                },
            };
            let queued: QueuedMessage = serde_json::from_value(client.execute(command).await?)?;
            let (body, files) = zork_client_types::files::decode(&queued.content).unwrap();
            assert!(body.is_empty());
            assert_eq!(files.len(), 1);
            assert_eq!(
                client
                    .store
                    .blob(&peer, &format!("upload:{}", files[0].id))?
                    .unwrap(),
                text.as_bytes()
            );
            assert_eq!(client.store.outbox(&peer)?.last(), Some(&queued));
            assert_eq!(
                client.store.get::<String>(&peer, "draft:chat")?.as_deref(),
                Some("")
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn crash_keeps_same_delivery_id_and_preserves_later_draft() -> Result<()> {
        let root = tempfile::tempdir()?;
        let mut client = Client::open(root.path())?;
        let peer = add_peer(&client);
        client
            .execute(Command::Draft {
                peer: peer.clone(),
                session: "chat".into(),
                content: "你好\nAndroid".into(),
            })
            .await?;
        let queued = client
            .execute(Command::Enqueue {
                peer: peer.clone(),
                session: "chat".into(),
                content: "你好\nAndroid".into(),
            })
            .await?;
        let id = queued["request_id"].as_str().unwrap().to_owned();
        assert_eq!(
            client.store.get::<String>(&peer, "draft:chat")?.unwrap(),
            ""
        );
        client.store.begin_delivery(&peer, &id)?;
        client
            .execute(Command::Draft {
                peer: peer.clone(),
                session: "chat".into(),
                content: "next".into(),
            })
            .await?;
        drop(client);
        let mut reopened = Client::open(root.path())?;
        let state = reopened
            .execute(Command::Conversation {
                peer: peer.clone(),
                session: "chat".into(),
            })
            .await?;
        assert_eq!(state["draft"], "next");
        assert_eq!(state["outbox"][0]["request_id"], id);
        assert_eq!(state["outbox"][0]["attempted"], true);
        assert!(reopened
            .execute(Command::Withdraw {
                peer: peer.clone(),
                request_id: id
            })
            .await
            .is_err());
        assert!(reopened
            .execute(Command::Enqueue {
                peer,
                session: "../escape".into(),
                content: "x".into()
            })
            .await
            .is_err());
        Ok(())
    }

    #[tokio::test]
    async fn disconnected_read_retains_cached_history_and_unsent_is_not_attempted() -> Result<()> {
        let root = tempfile::tempdir()?;
        let mut client = Client::open(root.path())?;
        let peer = add_peer(&client);
        client.store.put(
            &peer,
            "http:/v1/im/sessions/chat/messages",
            &json!({"body":{"items":[{"type":"message","role":"user","id":"one","content":"cached"}],"older_cursor":null},"fetched_at":1}),
        )?;
        let read = client
            .execute(Command::Read {
                peer: peer.clone(),
                path: "/v1/im/sessions/chat/messages".into(),
                cached_only: false,
            })
            .await?;
        assert_eq!(read["cached"], true);
        assert_eq!(read["snapshot"]["body"]["items"][0]["id"], "one");
        assert!(read["error"].is_string());
        let queued = client
            .execute(Command::Enqueue {
                peer: peer.clone(),
                session: "chat".into(),
                content: "queued".into(),
            })
            .await?;
        assert!(client
            .execute(Command::Flush { peer: peer.clone() })
            .await
            .is_err());
        assert!(!client.store.outbox(&peer)?[0].attempted);
        client.store.put(&peer, "draft:chat", &"later draft")?;
        client
            .execute(Command::Withdraw {
                peer: peer.clone(),
                request_id: queued["request_id"].as_str().unwrap().into(),
            })
            .await?;
        assert!(client.store.outbox(&peer)?.is_empty());
        assert_eq!(
            client.store.get::<String>(&peer, "draft:chat")?.as_deref(),
            Some("later draft\n\nqueued")
        );
        // A repeated withdrawal must not append another copy.
        client
            .execute(Command::Withdraw {
                peer: peer.clone(),
                request_id: queued["request_id"].as_str().unwrap().into(),
            })
            .await?;
        drop(client);
        let reopened = Client::open(root.path())?;
        assert_eq!(
            reopened
                .store
                .get::<String>(&peer, "draft:chat")?
                .as_deref(),
            Some("later draft\n\nqueued")
        );
        Ok(())
    }
}
