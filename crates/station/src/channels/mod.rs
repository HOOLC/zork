//! Public Chat channels, Agent configuration and durable Mesh delivery.
mod agents;
mod api;
mod attachments;
pub mod client;
mod delivery;
use crate::{
    node_access::{self, Subject},
    state::AppState,
};
pub(crate) use agents::open_home;
use anyhow::{ensure, Context, Result};
pub use api::{tool, FileRequest, Rpc};
pub use delivery::{start, subscribe};
use serde_json::{json, Value};

pub async fn prepare_start_chat(
    state: &AppState,
    request: &zork_client_types::chat::StartChat,
    author: &zork_client_types::chat::Author,
) -> Result<(Option<String>, Option<zork_client_types::chat::Channel>)> {
    request.validate().map_err(anyhow::Error::msg)?;
    ensure!(
        ulid::Ulid::from_string(&request.request_id).is_ok(),
        "invalid_request_id"
    );
    let client = request
        .client_id
        .as_deref()
        .map(|id| -> Result<String> {
            ensure!(
                id.len() == 26 && ulid::Ulid::from_string(id).is_ok(),
                "invalid_client_id"
            );
            Ok(format!("{}/{id}", node_access::identity(state)))
        })
        .transpose()?;
    if let Some(chat) = state.db.started_chat(request, author, client.as_deref())? {
        return Ok((client, Some(chat)));
    }
    let profiles = crate::agent::list_profiles(&state.agent).await?;
    ensure!(
        crate::agent::resolve_selection(
            &profiles,
            &crate::agent::SessionSelection {
                profile_id: request.profile_id.clone(),
                model: request.model.clone(),
                thinking: request.thinking.clone(),
            }
        )
        .is_some(),
        "chat_selection_unavailable"
    );
    Ok((client, None))
}

fn fingerprint(value: &impl serde::Serialize) -> Result<String> {
    node_access::fingerprint(value)
}
fn field<'a>(args: &'a Value, name: &str) -> Result<&'a str> {
    args[name]
        .as_str()
        .filter(|v| !v.is_empty())
        .context("channel_missing_parameter")
}
fn local(state: &AppState, target: &str) -> bool {
    target == "local" || target == node_access::identity(state)
}
pub(crate) fn access(state: &AppState, peer: &str) -> Result<()> {
    let config = zork_config::load_config(&state.config.data_root)?.mesh;
    ensure!(
        config.enabled && config.peers.iter().any(|p| p.origin == peer),
        "chat_access_denied"
    );
    Ok(())
}
fn actor(who: &Subject) -> String {
    if who.origin == "local" {
        who.agent.clone()
    } else {
        format!("{}/{}", who.origin, who.agent)
    }
}
pub(crate) fn request_actor(who: &Subject) -> String {
    actor(who)
}
fn error(error: &anyhow::Error) -> String {
    let code = error.to_string();
    if code.len() <= 128 && code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') {
        code
    } else {
        "channel_operation_failed".into()
    }
}

pub async fn remote(state: &AppState, peer: &str, rpc: Rpc) -> Result<Value> {
    node_access::remote(&rpc.subject, peer)?;
    access(state, peer)?;
    Ok(match api::execute(state, rpc, false).await {
        Ok(value) => value,
        Err(err) => json!({"status":"rejected","error":error(&err)}),
    })
}
pub async fn remote_file(state: &AppState, peer: &str, request: FileRequest) -> Result<Value> {
    access(state, peer)?;
    attachments::chunk(state, peer, request)
}
