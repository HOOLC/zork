//! Local SSE and authenticated Mesh subscriptions share the same push source.
use crate::{im_entry::LOCAL_GUI_PLATFORM, state::AppState};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use tokio::sync::mpsc;

pub async fn subscribe(
    state: AppState,
    session_id: Option<String>,
) -> Result<mpsc::Receiver<Value>> {
    if session_id.is_none() {
        let changes = state.db.realtime.listen(
            crate::realtime::SESSIONS
                | crate::realtime::TASKS
                | crate::realtime::AGENTS
                | crate::realtime::ARTIFACTS
                | crate::realtime::PROFILES
                | crate::realtime::MESH
                | crate::realtime::SYNC,
        );
        return Ok(zork_notify::stream::spawn(
            CatalogSource(state),
            changes,
            64,
            None,
            |event| match event {
                zork_notify::stream::Event::Data(value) => Some(value),
                zork_notify::stream::Event::Error(_) => Some(json!({"name":"resync","data":{}})),
                zork_notify::stream::Event::Heartbeat => None,
            },
        ));
    }
    let changes = state.db.realtime.listen(
        crate::realtime::SESSIONS
            | crate::realtime::TASKS
            | crate::realtime::AGENTS
            | crate::realtime::ACTIVITY
            | crate::realtime::SYNC,
    );
    let session = state
        .db
        .get_session_by_id(session_id.as_deref().expect("session branch"))?
        .context("local_im_session_not_found")?;
    ensure!(
        session.platform == LOCAL_GUI_PLATFORM,
        "local_im_session_not_found"
    );
    let chat_id = state.db.chat(&session.key)?.channel.chat_id;
    let message_changes = state
        .db
        .chat_topics
        .subscribe([crate::db::chats::Topic::Channel(chat_id.clone())]);
    let changes = changes.merge(
        state
            .db
            .chat_topics
            .subscribe([crate::db::chats::Topic::Channel(chat_id)]),
    );
    // Start the shared snapshot-backed projection once. It reports readiness
    // after its first snapshot, without replaying the historical event archive.
    for mut participant in state.entries.local_members(&session)? {
        let key = participant
            .as_object_mut()
            .unwrap()
            .remove("session_key")
            .unwrap();
        let Some(key) = key.as_str() else { continue };
        if let Some(bound) = state.db.get_session(key)? {
            let remote = state
                .db
                .product_task_for_session(&bound.key)?
                .and_then(|task| task.mesh)
                .is_some_and(|mesh| mesh["role"] == "owner");
            if !remote {
                if let Some(id) = &bound.id {
                    if !state.agent.service.contains(id) {
                        continue;
                    }
                    state
                        .status_projection
                        .ensure(
                            &bound.key,
                            id,
                            &bound.connection_id,
                            &bound.channel_id,
                            &bound.root_thread_ts,
                        )
                        .await;
                }
            }
        }
    }
    let (messages, mut initial) = state.entries.subscribe_local_with_snapshot(&session);
    let mut initial_participants = state.entries.local_members(&session)?;
    for participant in &mut initial_participants {
        participant.as_object_mut().unwrap().remove("session_key");
    }
    initial["participants"] = json!(initial_participants);
    let messages = messages.forward(64, None, |event| match event {
        Ok(event) => Some(json!({"name":event.name,"data":event.data})),
        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
            Some(json!({"name":"resync","data":{}}))
        }
        Err(_) => None,
    });
    // Business results can append directly to the durable Chat source without
    // passing through an IM entry. Observe its tail independently of members.
    let source_messages = zork_notify::stream::spawn(
        MessagesSource {
            state: state.clone(),
            session_key: session.key.clone(),
        },
        message_changes,
        1,
        None,
        |event| match event {
            zork_notify::stream::Event::Data(value) => Some(value),
            zork_notify::stream::Event::Error(_) => Some(json!({"name":"resync","data":{}})),
            zork_notify::stream::Event::Heartbeat => None,
        },
    );
    let participants = zork_notify::stream::spawn(
        ParticipantsSource { state, session },
        changes,
        1,
        None,
        |event| match event {
            zork_notify::stream::Event::Data(value) => Some(value),
            zork_notify::stream::Event::Error(_) => Some(json!({"name":"resync","data":{}})),
            zork_notify::stream::Event::Heartbeat => None,
        },
    );
    let mut merged = zork_notify::stream::merge(vec![participants, messages, source_messages], 64);
    let (tx, rx) = mpsc::channel(64);
    // Prefill before starting the forwarder: no participant/status/live frame
    // can overtake this connection's initial snapshot, over either HTTP or Mesh.
    tx.send(json!({"name":"snapshot","data":initial})).await?;
    tokio::spawn(async move {
        loop {
            let frame =
                tokio::select! { _ = tx.closed() => return, frame = merged.recv() => frame };
            let Some(frame) = frame else {
                return;
            };
            if tx.send(frame).await.is_err() {
                return;
            }
        }
    });
    Ok(rx)
}

struct MessagesSource {
    state: AppState,
    session_key: String,
}
impl zork_notify::stream::Source for MessagesSource {
    type Item = Value;
    type Error = anyhow::Error;
    async fn read(&mut self) -> Result<zork_notify::stream::Page<Value>> {
        let (epoch, sequence) = self.state.db.chat_source_tail(&self.session_key)?;
        Ok(zork_notify::stream::Page::snapshot(json!({
            "name":"messages_changed","data":{"source_epoch":epoch,"through":sequence}
        })))
    }
}

struct ParticipantsSource {
    state: AppState,
    session: crate::db::SessionRow,
}
impl zork_notify::stream::Source for ParticipantsSource {
    type Item = Value;
    type Error = anyhow::Error;
    async fn read(&mut self) -> Result<zork_notify::stream::Page<Value>> {
        let mut items = self.state.entries.local_members(&self.session)?;
        for participant in &mut items {
            participant.as_object_mut().unwrap().remove("session_key");
        }
        Ok(zork_notify::stream::Page::snapshot(
            json!({"name":"participants","data":items}),
        ))
    }
}

struct CatalogSource(AppState);
impl zork_notify::stream::Source for CatalogSource {
    type Item = Value;
    type Error = anyhow::Error;
    async fn read(&mut self) -> Result<zork_notify::stream::Page<Value>> {
        let revision = self.0.db.realtime.current();
        let owner = self.0.db.sync_local_owner()?;
        let catalog = self
            .0
            .db
            .sync_cursor(&owner, zork_client_types::sync::Scope::Catalog {})?;
        Ok(zork_notify::stream::Page::snapshot(
            json!({"name":"changed","data":{
                "catalog":catalog,
                "sessions":revision.sessions.wrapping_add(revision.sync),
                "tasks":revision.tasks.wrapping_add(revision.sync),
                "agents":revision.agents.wrapping_add(revision.sync),
                "artifacts":revision.artifacts.wrapping_add(revision.sync),
                "profiles":revision.profiles.wrapping_add(revision.sync),
                "mesh":revision.mesh,
            }}),
        ))
    }
}
