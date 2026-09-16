//! Device execution and managed skill packages use one authenticated route and
//! durable receipt contract. MCP shares its target identity and access policy.
mod jobs;
mod skills;
pub(crate) use skills::client_resources;
mod store;
#[cfg(test)]
mod tests;
use crate::{
    node_access::{self, Subject},
    state::AppState,
};
use anyhow::{ensure, Context, Result};
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use futures_util::{stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{watch, Notify, Semaphore};
fn id() -> String {
    ulid::Ulid::new().to_string()
}
fn valid_id(value: &str) -> Result<()> {
    ensure!(
        value.len() == 26 && ulid::Ulid::from_string(value).is_ok_and(|id| id.to_string() == value),
        "device_invalid_id"
    );
    Ok(())
}
fn fingerprint(value: &impl Serialize) -> Result<String> {
    node_access::fingerprint(value)
}

fn error(e: &anyhow::Error) -> String {
    let s = e.to_string();
    if ["device_", "skill_", "node_"]
        .iter()
        .any(|p| s.starts_with(p))
        && s.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
    {
        s
    } else {
        "device_operation_failed".into()
    }
}
fn field<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args[key]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 8192)
        .context("device_missing_parameter")
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rpc {
    #[serde(default)]
    interrupt: bool,
    subject: Subject,
    invocation_id: String,
    tool: String,
    arguments: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolRequest {
    session_id: String,
    invocation_id: String,
    tool: String,
    arguments: Value,
}
pub struct Hub {
    store: store::Store,
    active: Mutex<HashMap<String, watch::Sender<bool>>>,
    slots: Arc<Semaphore>,
    gate: Mutex<()>,
    finished: Notify,
}
impl Hub {
    pub fn open(root: &std::path::Path) -> Result<Self> {
        Ok(Self {
            store: store::Store::open(root)?,
            active: Mutex::new(HashMap::new()),
            slots: Arc::new(Semaphore::new(8)),
            gate: Mutex::new(()),
            finished: Notify::new(),
        })
    }
    pub async fn shutdown(&self) {
        self.slots.close();
        for stop in self.active.lock().expect("device jobs").values() {
            stop.send_replace(true);
        }
        loop {
            let finished = self.finished.notified();
            if self.active.lock().expect("device jobs").is_empty() {
                break;
            }
            finished.await;
        }
    }
}
fn mutating(tool: &str) -> bool {
    tool == "device.exec"
}

pub async fn tool(State(state): State<AppState>, Json(input): Json<ToolRequest>) -> Response {
    match api(&state, input).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({"error":error(&e),"delivery_rejected":e.is::<crate::tool_stream::Rejected>()}))).into_response(),
    }
}
async fn api(state: &AppState, input: ToolRequest) -> Result<Value> {
    node_access::ready(state)?;
    ensure!(
        !input.invocation_id.is_empty() && input.invocation_id.len() <= 256,
        "device_invalid_invocation"
    );
    let who = node_access::subject(state, &input.session_id)?;
    ensure!(input.arguments.is_object(), "device_invalid_arguments");
    if input.tool == "device.list" {
        return list(state, &who).await;
    }
    let args = input.arguments;
    if let Some(target) = args.get("target") {
        ensure!(
            target.as_str().is_some_and(|t| !t.is_empty()),
            "device_invalid_target"
        );
    }
    let target = args
        .get("target")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            if matches!(
                input.tool.as_str(),
                "device.status" | "device.read" | "device.cancel"
            ) {
                state
                    .node_tools
                    .store
                    .target(&who, args["operation_id"].as_str().unwrap_or(""))
                    .ok()
            } else {
                None
            }
        })
        .unwrap_or_else(|| "local".into());
    let rpc = Rpc {
        interrupt: false,
        subject: who.clone(),
        invocation_id: input.invocation_id.clone(),
        tool: input.tool.clone(),
        arguments: args.clone(),
    };
    let mut first_delivery = false;
    if mutating(&input.tool) {
        first_delivery = state.node_tools.store.enqueue(
            &who,
            &input.invocation_id,
            &fingerprint(&(&who, &input.tool, &args))?,
            &target,
            &rpc,
        )?;
    }
    let result = route(state, &target, rpc).await;
    match result {
        Ok(value) => {
            if mutating(&input.tool) {
                state.node_tools.store.ack(
                    &who,
                    &input.invocation_id,
                    field(&value, "operation_id")?,
                )?;
            }
            Ok(value)
        }
        Err(e)
            if first_delivery
                && matches!(
                    error(&e).as_str(),
                    "node_management_denied"
                        | "device_wrong_target"
                        | "device_unknown_tool"
                        | "device_request_limit"
                        | "device_unsupported"
                ) =>
        {
            state.node_tools.store.reject(&who, &input.invocation_id)?;
            Err(crate::tool_stream::Rejected(e).into())
        }
        Err(e) if mutating(&input.tool) => Ok(
            json!({"pending_delivery":true,"error":error(&e),"instruction":"The original invocation is still being tracked; do not repeat it."}),
        ),
        Err(e) => Err(e),
    }
}
async fn route(state: &AppState, target: &str, mut rpc: Rpc) -> Result<Value> {
    if target == "local" || target == node_access::identity(state) {
        let mut value = execute(state, rpc, true).await?;
        value["target"] = json!(node_access::identity(state));
        return Ok(value);
    }
    ensure!(
        node_access::targets(state)?.iter().any(|p| p == target),
        "node_management_denied"
    );
    rpc.subject.origin = node_access::identity(state);
    state
        .mesh
        .get()
        .context("node_starting")?
        .node_tool_call(target, rpc)
        .await
}
pub async fn remote(state: &AppState, peer: &str, rpc: Rpc) -> Result<Value> {
    node_access::remote(&rpc.subject, peer)?;
    let subject = rpc.subject.clone();
    let mutation = mutating(&rpc.tool);
    let mut value = execute(state, rpc, false).await?;
    // Once accepted, return its receipt. Subsequent output access rechecks
    // permissions, and the running job independently observes revocation.
    if !mutation {
        node_access::manage(state, &subject, false)?;
    }
    value["target"] = json!(node_access::identity(state));
    Ok(value)
}
async fn list(state: &AppState, who: &Subject) -> Result<Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut targets = Vec::new();
    let mut unavailable = Vec::new();
    let replies = stream::iter(node_access::targets(state)?.into_iter().map(|target| {
        let rpc = Rpc {
            interrupt: false,
            subject: who.clone(),
            invocation_id: "inspect".into(),
            tool: "device.inspect".into(),
            arguments: json!({}),
        };
        async move {
            let reply = tokio::time::timeout_at(deadline, route(state, &target, rpc)).await;
            (target, reply)
        }
    }))
    .buffer_unordered(4)
    .collect::<Vec<_>>()
    .await;
    for (target, reply) in replies {
        match reply {
            Ok(Ok(v)) => targets.push(v),
            Ok(Err(e)) => unavailable.push(json!({"target":target,"error":error(&e)})),
            Err(_) => unavailable.push(json!({"target":target,"error":"device_timeout"})),
        }
    }
    targets.sort_by_key(|v| v["target"].to_string());
    Ok(
        json!({"current_target":node_access::identity(state),"targets":targets,"unavailable_nodes":unavailable}),
    )
}
async fn execute(state: &AppState, rpc: Rpc, local: bool) -> Result<Value> {
    node_access::manage(state, &rpc.subject, local)?;
    ensure!(
        !rpc.invocation_id.is_empty()
            && rpc.invocation_id.len() <= 256
            && serde_json::to_vec(&rpc)?.len() <= 192 * 1024,
        "device_request_limit"
    );
    if let Some(target) = rpc.arguments.get("target").and_then(Value::as_str) {
        ensure!(
            target == node_access::identity(state) || (local && target == "local"),
            "device_wrong_target"
        );
    }
    if rpc.interrupt {
        return interrupt_remote(state, rpc).await;
    }
    match rpc.tool.as_str() {
        "device.inspect" => {
            let mut v = node_access::environment(state);
            v["capabilities"] = json!(["shell.run", "mcp.call"]);
            Ok(v)
        }
        "device.status" | "device.read" | "device.cancel" => jobs::status(state, &rpc).await,
        op if mutating(op) => submit(state, rpc, local).await,
        _ => anyhow::bail!("device_unknown_tool"),
    }
}
async fn submit(state: &AppState, rpc: Rpc, local: bool) -> Result<Value> {
    let subject = rpc.subject.clone();
    let fingerprint = fingerprint(&(&rpc.subject, &rpc.tool, &rpc.arguments))?;
    if let Some(id) =
        state
            .node_tools
            .store
            .existing(&rpc.subject, &rpc.invocation_id, &fingerprint)?
    {
        return state.node_tools.store.view(&id, &subject);
    }
    let slot = tokio::select! {
        permit = state.node_tools.slots.clone().acquire_owned() => permit.map_err(|_|anyhow::anyhow!("device_stopping"))?,
        cancelled = state.node_tools.store.cancellation(&rpc.subject,&rpc.invocation_id) => {
            cancelled?;
            return interrupt_remote(state,rpc).await;
        }
    };
    node_access::manage(state, &subject, local)?;
    let mut active = state.node_tools.active.lock().expect("device jobs");
    let (id, fresh) = state.node_tools.store.accept(&rpc, &fingerprint)?;
    if fresh {
        let (stop, stopped) = watch::channel(false);
        if state
            .node_tools
            .store
            .cancelled(&rpc.subject, &rpc.invocation_id)?
        {
            stop.send_replace(true);
        }
        active.insert(id.clone(), stop);
        let state = state.clone();
        let operation_id = id.clone();
        tokio::spawn(async move {
            let _slot = slot;
            let result = jobs::run(&state, &rpc, &operation_id, stopped, local).await;
            if let Err(e) = result {
                let previous = state
                    .node_tools
                    .store
                    .view(&operation_id, &rpc.subject)
                    .ok();
                let unknown = previous.as_ref().is_some_and(|v| {
                    matches!(v["state"].as_str(), Some("dispatching" | "running"))
                });
                let mut detail = previous
                    .as_ref()
                    .and_then(|v| v["result"].as_object())
                    .cloned()
                    .unwrap_or_default();
                detail.insert("error".into(), json!(error(&e)));
                if unknown {
                    detail.insert("process_state".into(), json!("unknown"));
                }
                let _ = state.node_tools.store.finish(
                    &operation_id,
                    if unknown { "outcome_unknown" } else { "failed" },
                    Some(Value::Object(detail)),
                );
            }
            state
                .node_tools
                .active
                .lock()
                .expect("device jobs")
                .remove(&operation_id);
            state.node_tools.finished.notify_one();
        });
    }
    state.node_tools.store.view(&id, &subject)
}

// The runtime subscribes before reading, so completion/output cannot be lost
// between the snapshot and the next notification.
pub(crate) fn watch_owner(state: &AppState, who: &Subject, id: &str) -> Result<String> {
    state.node_tools.store.target(who, id)
}
pub(crate) fn changes(state: &AppState) -> zork_notify::Changes {
    state.node_tools.store.subscribe()
}
pub(crate) async fn snapshot(
    state: &AppState,
    who: &Subject,
    id: &str,
    offset: u64,
    local: bool,
) -> Result<Value> {
    node_access::manage(state, who, local)?;
    let mut value = state.node_tools.store.view(id, who)?;
    if value["tool"] == "device.exec" {
        let rpc = Rpc {
            interrupt: false,
            subject: who.clone(),
            invocation_id: "output".into(),
            tool: "device.read".into(),
            arguments: json!({"operation_id":id,"offset":offset}),
        };
        match jobs::status(state, &rpc).await {
            Ok(chunk) => {
                value["chunk"] = chunk;
            }
            Err(error) if value["result"]["output_bytes"].as_u64().unwrap_or(0) == 0 => {
                let _ = error;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(value)
}

pub(crate) async fn interrupt(
    State(state): State<AppState>,
    Json(input): Json<ToolRequest>,
) -> Response {
    let result = async {
        let who = node_access::subject(&state, &input.session_id)?;
        state.node_tools.store.cancel(&who, &input.invocation_id)?;
        let Some((target, mut rpc)) = state.node_tools.store.invocation_route(&who, &input.invocation_id)? else {
            return Ok(json!({"state":"cancelled","result":{"process_state":"not_started","effects_may_have_occurred":false}}));
        };
        rpc.interrupt = true;
        let value = route(&state, &target, rpc).await?;
        state.node_tools.store.ack(&who, &input.invocation_id, field(&value,"operation_id")?)?;
        Ok::<_,anyhow::Error>(value)
    }.await;
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":error.to_string()})),
        )
            .into_response(),
    }
}
async fn interrupt_remote(state: &AppState, rpc: Rpc) -> Result<Value> {
    ensure!(mutating(&rpc.tool), "device_invalid_interrupt");
    let active = state.node_tools.active.lock().expect("device jobs");
    state
        .node_tools
        .store
        .cancel(&rpc.subject, &rpc.invocation_id)?;
    let hash = fingerprint(&(&rpc.subject, &rpc.tool, &rpc.arguments))?;
    let (id, fresh) = state.node_tools.store.accept(&rpc, &hash)?;
    if fresh {
        state.node_tools.store.finish(
            &id,
            "cancelled",
            Some(json!({"process_state":"not_started","effects_may_have_occurred":false})),
        )?;
    } else if let Some(stop) = active.get(&id) {
        stop.send_replace(true);
    }
    state.node_tools.store.view(&id, &rpc.subject)
}
