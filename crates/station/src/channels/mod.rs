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
