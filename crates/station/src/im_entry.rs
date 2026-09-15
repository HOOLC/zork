use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::connections::ConnectionManager;
use crate::db::{GatewayDb, SessionBindingRow, SessionRow, VisibleMessageRow};

pub const LOCAL_GUI_ENTRY_ID: &str = "local_gui";
pub const LOCAL_GUI_PLATFORM: &str = "local_gui";
const LOCAL_EVENT_CAPACITY: usize = 256;
type LocalTaskLocks = Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>;

#[derive(Clone, Debug)]
pub struct EntryEvent {
    pub name: &'static str,
    pub data: Value,
}

#[derive(Clone)]
struct LocalGuiEntry {
    events: zork_notify::events::EventHub<String, EntryEvent>,
    statuses: Arc<Mutex<HashMap<String, Value>>>,
    overviews: Arc<Mutex<HashMap<String, Value>>>,
}

impl Default for LocalGuiEntry {
    fn default() -> Self {
        Self {
            events: zork_notify::events::EventHub::new(LOCAL_EVENT_CAPACITY),
            statuses: Default::default(),
            overviews: Default::default(),
        }
    }
}
impl LocalGuiEntry {
    fn subscribe(&self, session_key: &str) -> zork_notify::events::Events<EntryEvent> {
        self.events.subscribe(session_key.to_owned())
    }
    fn publish(&self, session_key: &str, event: EntryEvent) {
        self.events.publish_with(session_key, || event);
    }

    fn publish_message(&self, message: &VisibleMessageRow, data: Value) {
        self.publish(
            &message.session_key,
            EntryEvent {
                name: "message",
                data,
            },
        );
    }

    fn publish_status(&self, session_key: &str, status: Value) {
        let mut statuses = self.statuses.lock().expect("local GUI statuses mutex");
        statuses.insert(session_key.to_owned(), status.clone());
        self.publish(
            session_key,
            EntryEvent {
                name: "status",
                data: status,
            },
        );
    }
}

/// Provider-neutral gateway boundary for deliberate messages and activity.
///
/// Configured external connections and the built-in desktop entry both pass
/// through this type. Adding an external provider extends this dispatch point;
/// it does not change the Agent transcript or GUI message contract.
#[derive(Clone)]
pub struct ImEntryGateway {
    db: Arc<GatewayDb>,
    connections: Arc<ConnectionManager>,
    local_gui: LocalGuiEntry,
    task_locks: LocalTaskLocks,
}

impl ImEntryGateway {
    pub(crate) fn activity_target(
        &self,
        target: &zork_agent::session::tools::ActivityTarget,
    ) -> Option<String> {
        use zork_agent::session::tools::ActivityTarget;
        let name = match target {
            ActivityTarget::Agent(id) => self
                .db
                .node_agent(id)
                .ok()
                .flatten()
                .map(|agent| agent.name),
            ActivityTarget::Task(id) => self
                .db
                .product_task(id)
                .ok()
                .flatten()
                .map(|task| task.title),
        }?;
        Some(zork_agent::session::tools::activity::bounded(&name))
    }

    pub fn new(db: Arc<GatewayDb>, connections: Arc<ConnectionManager>) -> Self {
        Self {
            db,
            connections,
            local_gui: LocalGuiEntry::default(),
            task_locks: LocalTaskLocks::default(),
        }
    }

    pub async fn lock_local_task(&self, session_id: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = self
            .task_locks
            .lock()
            .expect("task locks mutex")
            .entry(session_id.to_owned())
            .or_default()
            .clone();
        lock.lock_owned().await
    }

    pub fn project_task_run(
        &self,
        session_key: &str,
        agent_session_id: &str,
        event_id: &str,
        event: &Value,
    ) -> Result<()> {
        self.db
            .project_task_run(session_key, agent_session_id, event_id, event)?;
        self.local_gui.publish(
            session_key,
            EntryEvent {
                name: "history_changed",
                data: json!({}),
            },
        );
        Ok(())
    }

    #[cfg(test)]
    pub fn subscribe_local(&self, session_key: &str) -> zork_notify::events::Events<EntryEvent> {
        self.local_gui.subscribe(session_key)
    }

    pub fn subscribe_local_with_snapshot(
        &self,
        session: &SessionRow,
    ) -> (zork_notify::events::Events<EntryEvent>, Value) {
        let statuses = self
            .local_gui
            .statuses
            .lock()
            .expect("local GUI statuses mutex");
        let overviews = self
            .local_gui
            .overviews
            .lock()
            .expect("local GUI overviews mutex");
        let receiver = self.local_gui.subscribe(&session.key);
        (
            receiver,
            json!({"session_id":session.id,"status":statuses.get(&session.key),"execution":overviews.get(&session.key)}),
        )
    }

    pub fn publish_execution_snapshot(
        &self,
        session_key: &str,
        snapshot: &zork_agent_api::SessionSnapshot,
    ) -> Result<()> {
        self.db.project_task_snapshot(session_key, snapshot)?;
        let value = serde_json::to_value(snapshot)?;
        let mut overviews = self
            .local_gui
            .overviews
            .lock()
            .expect("local GUI overviews mutex");
        // server_time is a clock sample, not a business change.
        if overviews.get(session_key).is_some_and(|old| {
            old["cursor"] == value["cursor"]
                && old["runtime"] == value["runtime"]
                && old["aggregates"] == value["aggregates"]
                && old["execution"] == value["execution"]
        }) {
            return Ok(());
        }
        overviews.insert(session_key.into(), value.clone());
        self.local_gui.publish(
            session_key,
            EntryEvent {
                name: "session_updated",
                data: value,
            },
        );
        Ok(())
    }

    pub fn publish_imported_message(&self, session_key: &str, _role: &str, _text: &str) {
        // Import was committed before publication. Reload durable history so
        // IDs, timestamps and author information match the history endpoint.
        self.local_gui.publish(
            session_key,
            EntryEvent {
                name: "messages_changed",
                data: json!({}),
            },
        );
    }

    pub fn publish_visible_message(&self, message: &VisibleMessageRow) {
        self.local_gui
            .publish_message(message, self.message_json(message));
    }

    pub fn message_json(&self, message: &VisibleMessageRow) -> Value {
        let mut value = visible_message_json(message);
        if let Ok(channel) = self.db.chat(&message.session_key) {
            if let Ok(fact) = self
                .db
                .chat_message(&channel.channel.chat_id, &message.message_id)
            {
                value["chat_id"] = json!(fact.chat_id);
                value["author"] = json!(fact.author);
                value["author_kind"] = json!(fact.author.kind);
                value["mentions"] = json!(fact.mentions);
                value["reply_to"] = json!(fact.reply_to);
                if let Some(interaction) = fact.interaction {
                    value["interaction"] = interaction;
                }
                value["role"] = json!(if fact.author.kind
                    == zork_client_types::chat::AuthorKind::User
                {
                    "user"
                } else {
                    "assistant"
                });
                value["author_name"] = json!(fact.author.name);
                if fact.author.kind == zork_client_types::chat::AuthorKind::Agent {
                    value["author_agent_id"] = json!(fact.author.id);
                    value["author_name"] = json!(fact.author.name);
                    if let Ok(Some(agent)) = self.db.node_agent(&fact.author.id) {
                        value["author_name"] = json!(agent.name);
                        value["author_avatar"] = json!(agent.avatar);
                    }
                    if let Some((origin, _)) = fact.author.id.split_once('/') {
                        value["device"] = json!(origin);
                    }
                }
            }
        }
        value
    }

    pub fn accept_local_user_message(
        &self,
        session: &SessionRow,
        message_id: &str,
        text: &str,
    ) -> Result<VisibleMessageRow> {
        ensure_local_session(session)?;
        let message = self.db.record_visible_message(
            message_id,
            &session.key,
            &session.connection_id,
            &session.channel_id,
            &session.root_thread_ts,
            "user",
            text,
            None,
        )?;
        self.publish_visible_message(&message);
        Ok(message)
    }

    pub async fn post_message(
        &self,
        session_key: &str,
        conversation_id: &str,
        root_message_id: &str,
        text: &str,
        kind: Option<&str>,
    ) -> Result<()> {
        let binding = self
            .db
            .get_binding(session_key)?
            .context("session_not_found")?;
        validate_destination(&binding, conversation_id, root_message_id)?;
        match binding.platform() {
            LOCAL_GUI_PLATFORM => {
                let message = self.db.record_visible_message(
                    &ulid::Ulid::new().to_string(),
                    binding.key(),
                    binding.connection_id(),
                    conversation_id,
                    root_message_id,
                    "assistant",
                    text,
                    kind,
                )?;
                self.publish_visible_message(&message);
            }
            "slack" => {
                let connection = self
                    .connections
                    .runtime(binding.connection_id())
                    .await
                    .context("IM connection is not configured")?;
                connection
                    .slack
                    .post_thread_message(conversation_id, root_message_id, text)
                    .await?;
                if matches!(binding, SessionBindingRow::Normal(_)) {
                    self.db.touch_reply(binding.key())?;
                }
            }
            platform => anyhow::bail!("unsupported_im_entry: {platform}"),
        }
        Ok(())
    }

    pub fn local_activity(&self, key: &str) -> Option<Value> {
        self.local_gui
            .statuses
            .lock()
            .expect("local GUI statuses mutex")
            .get(key)
            .cloned()
    }

    pub fn import_remote_activity(&self, key: &str, activity: Value) {
        self.local_gui.publish_status(key, activity);
        self.db.realtime.notify(crate::realtime::ACTIVITY);
    }

    /// Participants are actual authors, with their independent receiving policy.
    pub fn local_participants(&self, session: &SessionRow) -> Result<Vec<Value>> {
        let channel = self.db.chat(&session.key)?;
        let mut participants = Vec::new();
        for participant in self.db.chat_participants(&channel.channel.chat_id)? {
            let author = participant.author;
            let agent = self.db.node_agent(&author.id)?;
            let binding = if let Some(agent) = &agent {
                agent
                    .session_key
                    .as_deref()
                    .map(|key| self.db.get_session(key))
                    .transpose()?
                    .flatten()
            } else if author.id == format!("session:{}", session.key) {
                Some(session.clone())
            } else {
                None
            };
            let name = agent
                .as_ref()
                .map(|a| a.name.clone())
                .or(author.name)
                .unwrap_or_else(|| {
                    if author.kind == zork_client_types::chat::AuthorKind::User {
                        "User".into()
                    } else {
                        author.id.clone()
                    }
                });
            let key = binding.as_ref().map(|s| s.key.as_str());
            let activity = key.and_then(|key| self.local_activity(key));
            participants.push(json!({"id":author.id,"name":name,"author_kind":author.kind,
                "avatar":agent.as_ref().and_then(|a|a.avatar.as_ref()),"subscribed":participant.subscribed,
                "message_count":participant.message_count,
                "session_id":binding.as_ref().and_then(|s|s.id.as_deref()).unwrap_or(""),
                "session_key":key,"activity":activity}));
        }
        Ok(participants)
    }

    pub async fn set_status(
        &self,
        connection_id: &str,
        session_key: &str,
        conversation_id: &str,
        root_message_id: &str,
        status_event: Value,
        rendered_status: &str,
    ) {
        let platform = self
            .db
            .get_binding(session_key)
            .ok()
            .flatten()
            .map(|binding| binding.platform().to_owned());
        match platform.as_deref() {
            Some(LOCAL_GUI_PLATFORM) => {
                self.local_gui.publish_status(session_key, status_event);
                self.db.realtime.notify(crate::realtime::ACTIVITY);
            }
            Some("slack") => {
                let rendered_status = slack_rendered_status(&status_event, rendered_status);
                if let Some(runtime) = self.connections.runtime(connection_id).await {
                    runtime
                        .status
                        .set_thread(conversation_id, root_message_id, rendered_status)
                        .await;
                }
            }
            _ => {}
        }
    }

    pub async fn refresh_status(
        &self,
        connection_id: &str,
        session_key: &str,
        conversation_id: &str,
        root_message_id: &str,
        rendered_status: &str,
    ) {
        let is_slack = self
            .db
            .get_binding(session_key)
            .ok()
            .flatten()
            .is_some_and(|binding| binding.platform() == "slack");
        if is_slack {
            if let Some(runtime) = self.connections.runtime(connection_id).await {
                runtime
                    .status
                    .set_thread(conversation_id, root_message_id, rendered_status)
                    .await;
            }
        }
    }

    pub async fn clear_status(
        &self,
        connection_id: &str,
        session_key: &str,
        conversation_id: &str,
        root_message_id: &str,
    ) {
        let platform = self
            .db
            .get_binding(session_key)
            .ok()
            .flatten()
            .map(|binding| binding.platform().to_owned());
        match platform.as_deref() {
            Some(LOCAL_GUI_PLATFORM) => {
                self.local_gui
                    .publish_status(session_key, json!({ "state": "clear" }));
                self.db.realtime.notify(crate::realtime::ACTIVITY);
            }
            Some("slack") => {
                if let Some(runtime) = self.connections.runtime(connection_id).await {
                    runtime
                        .status
                        .clear_thread(conversation_id, root_message_id)
                        .await;
                }
            }
            _ => {}
        }
    }
}

fn slack_rendered_status<'a>(status_event: &Value, rendered_status: &'a str) -> &'a str {
    if status_event.get("state").and_then(Value::as_str) == Some("clear") {
        ""
    } else {
        rendered_status
    }
}

pub fn visible_message_json(message: &VisibleMessageRow) -> Value {
    json!({
        "type": "message",
        "id": message.message_id,
        "created_at": message.created_at,
        "role": message.role,
        "content": message.text,
    })
}

fn ensure_local_session(session: &SessionRow) -> Result<()> {
    if session.platform != LOCAL_GUI_PLATFORM || session.connection_id != LOCAL_GUI_ENTRY_ID {
        anyhow::bail!("session_is_not_local_gui");
    }
    Ok(())
}

fn validate_destination(
    binding: &SessionBindingRow,
    conversation_id: &str,
    root_message_id: &str,
) -> Result<()> {
    if binding.mode() == zork_config::ImMode::Normal
        && (binding.conversation_id() != Some(conversation_id)
            || binding.root_message_id() != Some(root_message_id))
    {
        anyhow::bail!("session_destination_mismatch");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::EnsureSession;

    #[test]
    fn idle_slack_snapshot_cannot_render_as_working() {
        assert_eq!(
            slack_rendered_status(&json!({"state": "clear"}), "Working..."),
            ""
        );
        assert_eq!(
            slack_rendered_status(&json!({"state": "thinking"}), "Working..."),
            "Working..."
        );
    }

    #[tokio::test]
    async fn local_gui_and_unknown_provider_use_the_same_explicit_dispatch_boundary() {
        let dir = tempfile::tempdir().unwrap();
        zork_config::ensure_layout(dir.path()).unwrap();
        let db = Arc::new(
            GatewayDb::open(&dir.path().join("state"), &dir.path().join("workspaces")).unwrap(),
        );
        let connections = Arc::new(
            ConnectionManager::load(
                dir.path().to_path_buf(),
                reqwest::Client::builder().no_proxy().build().unwrap(),
            )
            .await
            .unwrap(),
        );
        let entries = ImEntryGateway::new(db.clone(), connections);
        let local = db
            .create_session_at_workspace(
                EnsureSession {
                    connection_id: LOCAL_GUI_ENTRY_ID,
                    platform: LOCAL_GUI_PLATFORM,
                    channel_id: "local-1",
                    root_thread_ts: "local-1",
                    channel_type: Some("desktop"),
                    initiator_user_id: None,
                    initiator_message_ts: None,
                },
                &dir.path().join("local-project"),
            )
            .unwrap();
        entries
            .local_gui
            .publish_status(&local.key, json!({"state":"thinking"}));
        let (mut current, snapshot) = entries.subscribe_local_with_snapshot(&local);
        assert_eq!(snapshot["status"]["state"], "thinking");
        entries
            .local_gui
            .publish_status(&local.key, json!({"state":"finished"}));
        assert_eq!(current.recv().await.unwrap().data["state"], "finished");
        let (_, snapshot) = entries.subscribe_local_with_snapshot(&local);
        assert_eq!(snapshot["status"]["state"], "finished");
        let mut events = entries.subscribe_local(&local.key);

        entries
            .post_message(
                &local.key,
                "local-1",
                "local-1",
                "only an explicit send is visible",
                Some("final"),
            )
            .await
            .unwrap();

        let event = events.recv().await.unwrap();
        assert_eq!(event.name, "message");
        assert_eq!(event.data["role"], "assistant");
        assert_eq!(
            db.list_visible_messages(&local.key, None, 10)
                .unwrap()
                .len(),
            1
        );

        let unsupported = db
            .create_session_at_workspace(
                EnsureSession {
                    connection_id: "matrix-1",
                    platform: "matrix",
                    channel_id: "room-1",
                    root_thread_ts: "room-1",
                    channel_type: Some("room"),
                    initiator_user_id: None,
                    initiator_message_ts: None,
                },
                &dir.path().join("matrix-project"),
            )
            .unwrap();
        let error = entries
            .post_message(
                &unsupported.key,
                "room-1",
                "room-1",
                "must not silently fall back",
                Some("final"),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("unsupported_im_entry: matrix"));
    }
}
