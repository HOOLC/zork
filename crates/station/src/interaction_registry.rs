//! Registration lifecycle and notifications. Registered businesses supply their
//! own cleanup implementation; cards, user requests and results stay with them.
use crate::{
    db::{
        chats::Topic,
        interaction_registry::{Cleanup, Notice, Registration},
    },
    node_access::{self, Subject},
    state::AppState,
};
use anyhow::{ensure, Context, Result};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;

pub(crate) type CleanupHandler =
    for<'a> fn(&'a AppState, &'a Registration, Cleanup) -> BoxFuture<'a, Result<()>>;

#[derive(Default)]
pub(crate) struct Handlers {
    entries: HashMap<&'static str, CleanupHandler>,
}
impl Handlers {
    pub(crate) fn register(&mut self, key: &'static str, handler: CleanupHandler) -> Result<()> {
        ensure!(
            !key.is_empty() && !self.entries.contains_key(key),
            "duplicate_interaction_handler"
        );
        self.entries.insert(key, handler);
        Ok(())
    }
    fn get(&self, key: &str) -> Result<CleanupHandler> {
        self.entries
            .get(key)
            .copied()
            .context("interaction_handler_unavailable")
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Remote {
    Cancel {
        subject: Subject,
        invocation_id: String,
    },
    Notice {
        notice: Notice,
    },
}

fn access(state: &AppState, peer: &str) -> Result<()> {
    ensure!(
        zork_config::load_config(&state.config.data_root)?
            .mesh
            .peers
            .iter()
            .any(|p| p.origin == peer),
        "interaction_access_denied"
    );
    Ok(())
}
fn local(state: &AppState, target: &str) -> bool {
    target == "local" || target == node_access::identity(state)
}

async fn route(state: &AppState, target: &str, request: Remote) -> Result<Value> {
    if local(state, target) {
        execute(state, request).await
    } else {
        access(state, target)?;
        state
            .mesh
            .get()
            .context("node_starting")?
            .registration_call(target, request)
            .await
    }
}

async fn recover_route(state: &AppState, target: &str, request: Remote) -> Result<Value> {
    let mut retry = zork_notify::retry::Retry::default();
    loop {
        match route(state, target, request.clone()).await {
            Ok(value) => return Ok(value),
            Err(error) => {
                let reason = error.to_string();
                if local(state, target)
                    || reason.starts_with("invalid_")
                    || reason.starts_with("interaction_")
                    || reason == "idempotency_conflict"
                {
                    return Err(error);
                }
                access(state, target)?;
                retry.wait().await;
            }
        }
    }
}

pub(crate) async fn remote(state: &AppState, peer: &str, request: Remote) -> Result<Value> {
    access(state, peer)?;
    match &request {
        Remote::Cancel { subject, .. } => node_access::remote(subject, peer)?,
        Remote::Notice { notice } => {
            ensure!(
                notice.request_id.rsplit_once('/').map(|(origin, _)| origin) == Some(peer),
                "interaction_notice_source_mismatch"
            );
            ensure!(
                local(state, &notice.owner.origin),
                "interaction_notice_target_mismatch"
            );
        }
    }
    execute(state, request).await
}

async fn execute(state: &AppState, request: Remote) -> Result<Value> {
    match request {
        Remote::Cancel {
            subject,
            invocation_id,
        } => cancel(state, &subject, &invocation_id).await?,
        Remote::Notice { notice } => deliver_local(state, &notice).await?,
    }
    Ok(json!({"accepted":true}))
}

async fn cleanup(state: &AppState, entry: &Registration, reason: Cleanup) -> Result<()> {
    (state.interaction_handlers.get(&entry.handler)?)(state, entry, reason).await?;
    state.db.acknowledge_registration_cleanup(&entry.request_id)
}

pub(crate) async fn cancel(state: &AppState, owner: &Subject, invocation: &str) -> Result<()> {
    state
        .db
        .revoke_interaction_registrations(owner, invocation)?;
    for (entry, reason) in state.db.pending_registration_cleanup()? {
        if entry.owner == *owner && entry.invocation_id == invocation {
            cleanup(state, &entry, reason).await?;
        }
    }
    Ok(())
}

pub(crate) async fn runtime(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::Json(command): axum::Json<zork_agent_station_tools::user_actions::Command>,
) -> axum::response::Response {
    use axum::{http::StatusCode, response::IntoResponse, Json};
    use zork_agent_station_tools::user_actions::Command;
    let result: Result<Value> = async {
        node_access::ready(&state)?;
        let (session, invocation, target) = command.identity();
        let mut who = node_access::subject(&state, session)?;
        if !local(&state, target) {
            who.origin = node_access::identity(&state);
        }
        match &command {
            Command::Track { .. } => {
                state.db.begin_user_call(&who, invocation, target)?;
            }
            Command::Finish { .. } => {
                // Retire every remaining user wait when its business invocation ends.
                // Completed business results remain authoritative; cleanup is durable.
                state.db.finish_user_calls(&who, invocation)?;
            }
            Command::Cancel { .. } => {
                let calls = state.db.cancel_user_calls(&who, invocation)?;
                for call in calls {
                    recover_route(
                        &state,
                        &call.target,
                        Remote::Cancel {
                            subject: call.subject.clone(),
                            invocation_id: call.invocation_id.clone(),
                        },
                    )
                    .await?;
                    state.db.acknowledge_user_call_cancellation(&call)?;
                }
            }
        }
        Ok(json!({"accepted":true}))
    }
    .await;
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => (
            if error.chain().any(|e| e.is::<rusqlite::Error>()) {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::BAD_REQUEST
            },
            Json(json!({"error":error.to_string()})),
        )
            .into_response(),
    }
}

async fn deliver_local(state: &AppState, notice: &Notice) -> Result<()> {
    let subject = node_access::subject(state, &notice.owner.session)?;
    ensure!(
        subject.agent == notice.owner.agent,
        "interaction_notice_owner_mismatch"
    );
    let origin = notice
        .request_id
        .rsplit_once('/')
        .context("invalid_interaction_reference")?
        .0;
    // This registry has a new durable sequence. Its source identity must not
    // reuse watermarks from the retired interaction service.
    let source = format!(
        "interaction-registrations-{}",
        node_access::fingerprint(&origin)?
    );
    state
        .agent
        .append_ordered_mailbox(
            notice.owner.session.clone(),
            source,
            notice.sequence,
            serde_json::to_string(&json!({"kind":"user_action_required","request_id":notice.request_id,"target":notice.origin,"invocation_id":notice.invocation_id,"instruction":"Publish this registered interaction with chat.post_message and interaction: {request_id}. It belongs to the referenced original tool invocation; do not repeat the operation or poll its status."}))?,
            true,
        )
        .await?;
    Ok(())
}

async fn dispatch_owner(state: AppState, owner: Subject) {
    let mut retry = zork_notify::retry::Retry::default();
    loop {
        let result: Result<bool> = async {
            let Some(notice) = state.db.next_user_notice(&owner)? else {
                return Ok(false);
            };
            if owner.origin == "local" {
                deliver_local(&state, &notice).await?;
            } else {
                route(
                    &state,
                    &owner.origin,
                    Remote::Notice {
                        notice: notice.clone(),
                    },
                )
                .await?;
            }
            state.db.acknowledge_user_notice(notice.sequence)?;
            Ok(true)
        }
        .await;
        match result {
            Ok(false) => return,
            Ok(true) => retry.reset(),
            Err(_) => retry.wait().await,
        }
    }
}

pub(crate) fn start(state: AppState) -> Result<zork_notify::Task<()>> {
    state.db.recover_interaction_registrations()?;
    Ok(zork_notify::Task(tokio::spawn(async move {
        let mut changes = state
            .db
            .chat_topics
            .subscribe([Topic::UserInteractionDelivery])
            .merge(state.db.realtime.listen(crate::realtime::MESH));
        let mut running = std::collections::HashSet::new();
        let mut tasks = tokio::task::JoinSet::new();
        let mut retry = zork_notify::retry::Retry::default();
        loop {
            changes.checkpoint();
            let pending_cleanup = match state.db.pending_registration_cleanup() {
                Ok(entries) => entries,
                Err(_) => {
                    retry.wait().await;
                    continue;
                }
            };
            for (entry, reason) in pending_cleanup {
                let key = format!("cleanup-{}", entry.request_id);
                if !running.insert(key.clone()) {
                    continue;
                }
                let state = state.clone();
                tasks.spawn(async move {
                    let mut retry = zork_notify::retry::Retry::default();
                    loop {
                        if cleanup(&state, &entry, reason).await.is_ok() {
                            break;
                        }
                        retry.wait().await;
                    }
                    key
                });
            }
            let pending = state
                .db
                .pending_user_call_cancellations()
                .and_then(|calls| state.db.user_notice_owners().map(|owners| (calls, owners)));
            let (calls, owners) = match pending {
                Ok(value) => {
                    retry.reset();
                    value
                }
                Err(_) => {
                    retry.wait().await;
                    continue;
                }
            };
            for call in calls {
                let key = format!(
                    "cancel-{}",
                    node_access::fingerprint(&(
                        call.subject.clone(),
                        &call.invocation_id,
                        &call.target
                    ))
                    .expect("call identity")
                );
                if !running.insert(key.clone()) {
                    continue;
                }
                let state = state.clone();
                tasks.spawn(async move {
                    let mut retry = zork_notify::retry::Retry::default();
                    loop {
                        let result: Result<()> = async {
                            route(
                                &state,
                                &call.target,
                                Remote::Cancel {
                                    subject: call.subject.clone(),
                                    invocation_id: call.invocation_id.clone(),
                                },
                            )
                            .await?;
                            state.db.acknowledge_user_call_cancellation(&call)
                        }
                        .await;
                        if result.is_ok() {
                            break;
                        }
                        retry.wait().await;
                    }
                    key
                });
            }
            for owner in owners {
                let key = format!(
                    "notice-{}",
                    node_access::fingerprint(&owner).expect("subject identity")
                );
                if !running.insert(key.clone()) {
                    continue;
                }
                let state = state.clone();
                tasks.spawn(async move {
                    dispatch_owner(state, owner).await;
                    key
                });
            }
            tokio::select! {
                changed = changes.changed() => if changed.is_err() { return },
                Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                    match result {
                        Ok(key) => { running.remove(&key); },
                        Err(error) => { tracing::error!(%error, "User interaction delivery worker failed"); return },
                    }
                }
            }
        }
    })))
}
