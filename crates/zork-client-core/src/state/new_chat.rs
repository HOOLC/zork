use super::{Device, Domains, Observable, Subscription};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex, Weak};
use zork_client_types::chat::{Channel, StartChat};

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NewChatData {
    pub text: String,
    pub model: String,
    pub thinking: String,
    pub profile: String,
    pub pending: Option<StartChat>,
    pub created: Option<Channel>,
    #[serde(skip)]
    pub busy: bool,
    #[serde(skip)]
    pub error: Option<String>,
}

pub struct NewChat {
    device: Weak<Device>,
    owned: Mutex<NewChatData>,
    state: Observable<NewChatData>,
    pub(crate) view: Observable<zork_client_types::new_chat::Snapshot>,
    watch: Mutex<Option<tokio::task::JoinHandle<()>>>,
}
impl Drop for NewChat {
    fn drop(&mut self) {
        if let Some(task) = self.watch.get_mut().unwrap().take() {
            task.abort();
        }
    }
}
impl NewChat {
    pub(super) fn new(device: &Arc<Device>) -> Arc<Self> {
        let data: NewChatData = device
            .cache
            .as_ref()
            .and_then(|(store, node)| store.get(node, "new-chat").ok().flatten())
            .unwrap_or_default();
        let source = Arc::new(Self {
            device: Arc::downgrade(device),
            owned: Mutex::new(data.clone()),
            state: Observable::new(data),
            view: Observable::new(Default::default()),
            watch: Mutex::new(None),
        });
        let profiles = device.profiles();
        let mut changes = profiles.subscribe_state();
        let mut connection = device.subscribe_domains(Domains::CONNECTION);
        source.publish_view();
        let weak = Arc::downgrade(&source);
        *source.watch.lock().unwrap() = Some(device.client.spawn(async move {
            profiles.refresh().await;
            loop {
                if let Some(source) = weak.upgrade() {
                    source.ensure_default_model();
                    source.publish_view();
                } else {
                    break;
                }
                tokio::select! {
                    value=changes.changed()=>if value.is_none(){break},
                    value=connection.changed()=>if value.is_none(){break},
                }
            }
        }));
        source
    }
    pub fn snapshot(&self) -> Arc<NewChatData> {
        self.state.read()
    }
    pub fn subscribe(&self) -> Subscription<NewChatData> {
        self.state.subscribe()
    }
    pub fn subscribe_view(&self) -> Subscription<zork_client_types::new_chat::Snapshot> {
        self.view.subscribe()
    }
    fn change(
        &self,
        change: impl FnOnce(&mut NewChatData) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        let mut owned = self.owned.lock().unwrap();
        let mut next = owned.clone();
        change(&mut next)?;
        if next == *owned {
            return Ok(());
        }
        if let Some(device) = self.device.upgrade() {
            anyhow::ensure!(!device.snapshot().revoked, "设备访问权限已撤销");
            if let Some((store, node)) = &device.cache {
                store.save_new_chat(node, &owned, &next)?;
            }
        }
        *owned = next.clone();
        self.state.publish(next);
        drop(owned);
        self.publish_view();
        Ok(())
    }
    pub fn edit(&self, text: String) -> anyhow::Result<()> {
        self.change(|s| {
            anyhow::ensure!(!s.busy && s.pending.is_none(), "请先重试未确认的创建操作");
            s.text = text;
            s.created = None;
            s.error = None;
            Ok(())
        })
    }
    pub fn begin(&self) -> anyhow::Result<()> {
        self.change(|s| {
            s.created = None;
            Ok(())
        })
    }
    fn ensure_default_model(&self) {
        let state = self.snapshot();
        if !state.model.is_empty() || state.busy || state.pending.is_some() {
            return;
        }
        let Some(device) = self.device.upgrade() else {
            return;
        };
        let profiles = device.profiles().snapshot();
        if !profiles.loaded {
            return;
        }
        let selectable = crate::new_chat::selectable(&profiles.profiles);
        let choices = crate::agent_edit::choices(&selectable, "auto", "", "");
        let Some(model) = choices["models"]
            .as_array()
            .and_then(|models| models.first())
            .and_then(|model| model["id"].as_str())
            .map(str::to_owned)
        else {
            return;
        };
        let _ = self.choose(model, String::new(), "auto".into());
    }
    pub fn choose(&self, model: String, thinking: String, profile: String) -> anyhow::Result<()> {
        let device = self
            .device
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("设备不可用"))?;
        let (model, thinking, profile) = crate::new_chat::choose(
            &device.profiles().snapshot().profiles,
            &model,
            &thinking,
            &profile,
        );
        self.change(|s| {
            anyhow::ensure!(!s.busy && s.pending.is_none(), "请先重试未确认的创建操作");
            s.model = model;
            s.thinking = thinking;
            s.profile = profile;
            s.created = None;
            s.error = None;
            Ok(())
        })
    }
    pub fn presentation(&self) -> zork_client_types::new_chat::Snapshot {
        self.view.read().as_ref().clone()
    }
    fn publish_view(&self) {
        let _serial = self.owned.lock().unwrap();
        let state = self.snapshot();
        if self
            .device
            .upgrade()
            .is_none_or(|device| device.snapshot().revoked)
        {
            if !self.view.read().revoked {
                self.view.invalidate(zork_client_types::new_chat::Snapshot {
                    revoked: true,
                    error: Some("设备访问权限已撤销".into()),
                    ..Default::default()
                });
            }
            return;
        }
        let profiles = self
            .device
            .upgrade()
            .map(|d| d.profiles().snapshot())
            .unwrap_or_default();
        let mut view = crate::new_chat::present(
            &profiles.profiles,
            &state.text,
            &state.model,
            &state.thinking,
            &state.profile,
        );
        view.busy = state.busy;
        view.uncertain = state.pending.is_some() && !state.busy;
        view.editable = !state.busy && state.pending.is_none();
        view.can_submit = !state.busy && (state.pending.is_some() || view.can_submit);
        view.loading = !profiles.loaded && profiles.error.is_none();
        view.needs_model = profiles.loaded && view.needs_model;
        view.error = state
            .error
            .clone()
            .or_else(|| profiles.error.clone())
            .or(view.error);
        view.created =
            state
                .created
                .as_ref()
                .map(|c| zork_client_types::navigation::NavigationChat {
                    chat_id: c.chat_id.clone(),
                    message_count: c.message_count,
                    title: c.title.clone(),
                    can_send: true,
                    can_stop: true,
                    updated_at: c
                        .last_message_at
                        .clone()
                        .unwrap_or_else(|| c.created_at.clone()),
                    ..Default::default()
                });
        self.view.publish(view);
    }
    pub fn apply(
        self: &Arc<Self>,
        action: zork_client_types::new_chat::Action,
    ) -> anyhow::Result<()> {
        use zork_client_types::new_chat::Action;
        let s = self.snapshot();
        match action {
            Action::Begin => self.begin(),
            Action::Edit { text } => self.edit(text),
            Action::Model { value } => self.choose(value, s.thinking.clone(), s.profile.clone()),
            Action::Thinking { value } => self.choose(s.model.clone(), value, s.profile.clone()),
            Action::Profile { value } => self.choose(s.model.clone(), s.thinking.clone(), value),
            Action::Select { profile, model } => self.choose(model, s.thinking.clone(), profile),
            Action::Submit { text } => self.submit_text(text),
        }
    }
    /// The core owns the task even when the initiating page is no longer visible.
    pub fn submit(self: &Arc<Self>) -> anyhow::Result<()> {
        self.submit_text(self.snapshot().text.clone())
    }
    pub fn submit_text(self: &Arc<Self>, text: String) -> anyhow::Result<()> {
        let device = self
            .device
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("设备不可用"))?;
        anyhow::ensure!(!device.snapshot().revoked, "设备访问权限已撤销");
        self.change(|s| {
            anyhow::ensure!(!s.busy, "正在创建 Chat");
            if s.pending.is_none() {
                s.text = text;
                crate::valid_content(&s.text, false)?;
                crate::agent_edit::validate_selection(
                    &crate::new_chat::selectable(&device.profiles().snapshot().profiles),
                    &s.profile,
                    &s.model,
                    &s.thinking,
                )?;
                s.pending = Some(StartChat {
                    request_id: ulid::Ulid::new().to_string(),
                    content: s.text.clone(),
                    model: s.model.clone(),
                    thinking: s.thinking.clone(),
                    profile_id: s.profile.clone(),
                    title: None,
                    client_id: Some(device.client.client_id().into()),
                });
                s.pending
                    .as_ref()
                    .unwrap()
                    .validate()
                    .map_err(anyhow::Error::msg)?;
            }
            s.busy = true;
            s.error = None;
            s.created = None;
            Ok(())
        })?;
        let request = self.snapshot().pending.clone().unwrap();
        let source = self.clone();
        let client = device.client.clone();
        device.client.clone().spawn(async move {
            let result = client.start_chat(&request).await;
            // Make the committed Session available to navigation/composer
            // consumers before publishing the creation transition.
            if result.is_ok() {
                device.refresh(Domains::SESSIONS).await;
            }
            if let Err(error) = &result {
                if let Some((store, node)) = &device.cache {
                    let _ = store.fail_delivery(node, &request.request_id, &error.to_string());
                }
            }
            let committed = source.change(|s| {
                s.busy = false;
                match result {
                    Ok(chat) => {
                        s.created = Some(chat);
                        s.pending = None;
                        s.text.clear();
                    }
                    Err(error) => {
                        // Only a definite pre-commit rejection unlocks editing.
                        if matches!(error.status(), Some(400 | 404 | 422)) {
                            s.pending = None;
                        }
                        s.error = Some(error.to_string());
                    }
                }
                Ok(())
            });
            if let Err(error) = committed {
                let mut state = source.owned.lock().unwrap();
                state.busy = false;
                state.error = Some(format!("无法保存创建结果，请重试原请求：{error}"));
                source.state.publish(state.clone());
                drop(state);
                source.publish_view();
            }
            device.reload_outbox();
            device.start_delivery();
        });
        Ok(())
    }
}

impl Device {
    pub fn new_chat(self: &Arc<Self>) -> Arc<NewChat> {
        self.new_chat.get_or_init(|| NewChat::new(self)).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        api::{ProfileInfo, StationClient},
        store::ClientStore,
    };
    use axum::{response::IntoResponse, routing::post, Json, Router};
    use std::time::Duration;

    async fn form(store: Arc<ClientStore>, url: &str) -> (Arc<Device>, Arc<NewChat>) {
        let device = Device::open(
            Arc::new(StationClient::new(url, None)),
            Some((store, "node".into())),
            false,
        );
        let source = device.new_chat();
        let watch = source.watch.lock().unwrap().take().unwrap();
        watch.abort();
        let _ = watch.await;
        let profiles:Vec<ProfileInfo>=serde_json::from_value(serde_json::json!([{"profile_id":"p","provider":"openai","auth_configured":true,"models":[{"id":"m","thinking":["off","high"],"default_thinking":"high","limits":{"context_window_tokens":100000,"max_output_tokens":1000}}]}])).unwrap();
        device.profiles().seed(super::super::ProfileData {
            profiles: Arc::new(profiles),
            loaded: true,
            ..Default::default()
        });
        source.publish_view();
        (device, source)
    }
    async fn settled(source: &NewChat) {
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut updates = source.subscribe_view();
            loop {
                if !source.presentation().busy {
                    break;
                }
                updates.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn editing_a_restorable_draft_never_creates_a_message_or_session() {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(ClientStore::open(root.path()).unwrap());
        let (device, source) = form(store.clone(), "http://127.0.0.1:9").await;
        source.edit("unsent draft".into()).unwrap();
        source
            .choose("m".into(), "high".into(), "auto".into())
            .unwrap();
        assert!(source.presentation().can_submit);
        assert!(source.snapshot().pending.is_none());
        assert!(source.snapshot().created.is_none());
        assert!(store.outbox("node").unwrap().is_empty());
        assert!(device.snapshot().sessions.is_empty());
        let reopened = Arc::new(ClientStore::open(root.path()).unwrap());
        let (_restored, restored) = form(reopened, "http://127.0.0.1:9").await;
        assert_eq!(restored.snapshot().text, "unsent draft");
        assert_eq!(restored.snapshot().model, "m");
        assert_eq!(restored.snapshot().thinking, "high");
    }

    #[tokio::test]
    async fn new_chat_wire_keeps_unapplied_baselines_and_revocation_clears_the_form() {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(ClientStore::open(root.path()).unwrap());
        let (device, source) = form(store.clone(), "http://127.0.0.1:9").await;
        let mut wire = crate::subscriptions::WireSubscription::from_device(
            crate::subscriptions::Key::NewChat {
                peer: "node".into(),
            },
            device.clone(),
            store,
        )
        .unwrap();
        let opening = wire.prepare().unwrap().unwrap();
        assert_eq!(opening["reset"], true);
        assert!(wire.finish(opening["batch"].as_u64().unwrap(), true));
        source.edit("draft one".into()).unwrap();
        let first = wire.prepare().unwrap().unwrap();
        source.edit("draft two".into()).unwrap();
        assert_eq!(wire.prepare().unwrap().unwrap(), first);
        assert!(wire.finish(first["batch"].as_u64().unwrap(), false));
        let retry = wire.prepare().unwrap().unwrap();
        assert_eq!(retry["snapshot"]["text"], "draft two");
        device.commit(|s| s.revoked = true);
        assert!(!wire.valid(retry["batch"].as_u64().unwrap()));
        let revoked = wire.prepare().unwrap().unwrap();
        assert_eq!(revoked["snapshot"]["revoked"], true);
        assert_eq!(revoked["snapshot"]["text"], "");
        assert!(wire.finish(revoked["batch"].as_u64().unwrap(), true));
        assert!(source.submit_text("blocked".into()).is_err());
    }
    #[tokio::test]
    async fn uncertain_creation_survives_reopen_and_retry_keeps_the_same_first_message_identity() {
        let requests = Arc::new(Mutex::new(Vec::<StartChat>::new()));
        let router = Router::new().route(
            "/v1/im/chats",
            post({
                let requests = requests.clone();
                move |Json(request): Json<StartChat>| {
                    let attempt = {
                        let mut all = requests.lock().unwrap();
                        all.push(request.clone());
                        all.len()
                    };
                    async move {
                        if attempt == 1 {
                            (
                                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                                Json(serde_json::json!({"error":"reply lost"})),
                            )
                                .into_response()
                        } else {
                            Json(Channel {
                                archived: false,
                                chat_id: request.request_id,
                                title: "created".into(),
                                created_at: "now".into(),
                                last_message_at: Some("now".into()),
                                message_count: 1,
                                creator: None,
                            })
                            .into_response()
                        }
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = zork_notify::Task(tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap()
        }));
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(ClientStore::open(root.path()).unwrap());
        let (device, source) = form(store.clone(), &url).await;
        source.edit("first message".into()).unwrap();
        source
            .choose("m".into(), "high".into(), "auto".into())
            .unwrap();
        source.submit().unwrap();
        assert!(source.submit().is_err());
        settled(&source).await;
        let pending = source.snapshot().pending.clone().unwrap();
        let saved = store.outbox("node").unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].session_id, pending.request_id);
        assert_eq!(saved[0].request_id, pending.request_id);
        assert!(saved[0].attempted);
        assert!(source.edit("changed".into()).is_err());
        device.stop_sync();
        let reopened = Arc::new(ClientStore::open(root.path()).unwrap());
        let (restored_device, restored) = form(reopened.clone(), &url).await;
        assert!(restored.presentation().uncertain);
        assert_eq!(restored.snapshot().pending, Some(pending.clone()));
        restored.submit().unwrap();
        settled(&restored).await;
        assert_eq!(
            restored.snapshot().created.as_ref().unwrap().chat_id,
            pending.request_id
        );
        assert_eq!(
            requests.lock().unwrap().as_slice(),
            &[pending.clone(), pending.clone()]
        );
        assert_eq!(reopened.outbox("node").unwrap().len(), 1);
        assert!(restored.snapshot().pending.is_none());
        assert!(restored.snapshot().text.is_empty());
        restored_device.stop_sync();
        drop(server);
    }
    #[tokio::test]
    async fn explicit_rejection_retains_editable_input_and_removes_only_the_uncommitted_message() {
        let router = Router::new().route(
            "/v1/im/chats",
            post(|| async {
                (
                    axum::http::StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error":"chat_selection_unavailable"})),
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = zork_notify::Task(tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap()
        }));
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(ClientStore::open(root.path()).unwrap());
        let (device, source) = form(store.clone(), &url).await;
        source.edit("keep me".into()).unwrap();
        source
            .choose("m".into(), "high".into(), "auto".into())
            .unwrap();
        source.submit().unwrap();
        settled(&source).await;
        assert_eq!(source.snapshot().text, "keep me");
        assert!(source.snapshot().pending.is_none());
        assert!(source.presentation().editable);
        assert!(store.outbox("node").unwrap().is_empty());
        source.edit("revised".into()).unwrap();
        device.stop_sync();
        drop(server);
    }
}
