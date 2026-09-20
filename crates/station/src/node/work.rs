//! Explicit work allocation reuses the durable local/Mesh assignment contract.
//! Read, subscription and home-opening operations do not allocate work contexts.
use super::*;
use anyhow::{ensure, Context, Result};

pub(crate) async fn assign_chat(
    state: &AppState,
    creator: &NodeAgent,
    request: &str,
    worker_id: &str,
    goal: &str,
) -> Result<Value> {
    ensure!(
        valid_id(request) && !goal.trim().is_empty() && goal.len() <= 32768,
        "invalid_work_request"
    );
    let _guard = state
        .entries
        .lock_local_task(&format!("assignment:{}:{request}", creator.id))
        .await;
    if worker_id.contains('/') {
        let mesh = state.mesh.get().context("mesh_disabled")?;
        ensure!(
            mesh.remote_worker_available(&creator.id, worker_id).await != Some(false),
            "worker_not_granted"
        );
        mesh.assign_worker(state, creator, worker_id, request, goal)?;
    } else {
        let worker = state
            .db
            .node_agent(worker_id)?
            .context("worker_not_found")?;
        let (key, runtime, status) =
            state
                .db
                .worker_task_allocation(&creator.id, request, worker_id, goal)?;
        if status != "sent" {
            let session = ensure_agent_session(state, &worker, &key, &runtime).await?;
            state.db.ensure_product_task(&key)?;
            let goal = state.db.copy_message_files(
                creator
                    .session_key
                    .as_deref()
                    .context("creator_session_missing")?,
                &session,
                goal,
            )?;
            state.entries.accept_local_user_message(
                &session,
                &format!("assignment-{}-{request}", creator.id),
                &goal,
            )?;
            state.db.set_worker_task_state(&key, "sending")?;
            let chat = state.db.chat(&key)?.channel;
            let input =
                crate::http::conversation_files::agent_content(state, &session.key, &goal).await?;
            let input = json!({"source":"assignment","target":crate::node_access::identity(state),"chat_id":chat.chat_id,"text":input}).to_string();
            crate::agent::append_mailbox_id(
                &state.agent,
                &runtime,
                &format!("assignment-{request}"),
                &input,
            )
            .await?;
            state.db.record_chat_source(worker_id, "local")?;
            state.db.set_worker_task_state(&key, "sent")?;
        }
    }
    let (key, _, _) = state
        .db
        .worker_task_allocation(&creator.id, request, worker_id, goal)?;
    let chat = state.db.chat(&key)?.channel;
    let preference_key = format!("assignment-receive:{}:{request}", creator.id);
    let receipt = state.db.chat_begin(&preference_key, &chat.chat_id)?;
    if receipt.result.is_none() {
        state.db.update_chat_preferences(
            &preference_key,
            &chat.chat_id,
            &creator.id,
            &zork_client_types::chat::UpdatePreferences {
                changes: zork_client_types::chat::PreferenceChanges {
                    subscribed: Some(true),
                    ..Default::default()
                },
                expected_revision: None,
                start: Some(zork_client_types::chat::StartAt::After {
                    message_id: if worker_id.contains('/') {
                        format!("mesh-worker-{}-goal", chat.chat_id)
                    } else {
                        format!("assignment-{}-{request}", creator.id)
                    },
                }),
            },
        )?;
    }
    state.db.record_chat_source(&creator.id, "local")?;
    Ok(json!({"state":"queued","chat":chat,"worker_id":worker_id}))
}
