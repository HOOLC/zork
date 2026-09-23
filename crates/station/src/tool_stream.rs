//! Internal event stream for an ordinary Agent tool invocation. Receipts and
//! byte offsets are transport details, never a second model-facing lifecycle.
use crate::{
    node_access::{self, Subject},
    state::AppState,
};
use anyhow::{ensure, Context, Result};
use axum::{
    extract::State,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Watch {
    pub subject: Subject,
    pub domain: String,
    pub id: String,
    #[serde(default)]
    pub offset: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Input {
    session_id: String,
    domain: String,
    id: String,
    #[serde(default)]
    offset: u64,
}
fn active(value: &Value) -> bool {
    matches!(
        value["state"].as_str(),
        Some("accepted" | "dispatching" | "running" | "cancel_requested")
    )
}
pub(crate) async fn local(
    state: AppState,
    request: Watch,
    is_local: bool,
) -> Result<mpsc::Receiver<Value>> {
    ensure!(request.domain == "device", "invalid_tool_domain");
    let changes =
        crate::node_tools::changes(&state).merge(state.db.realtime.listen(crate::realtime::MESH));
    Ok(zork_notify::stream::spawn(
        ToolSource {
            state,
            request,
            is_local,
        },
        changes,
        8,
        None,
        |event| match event {
            zork_notify::stream::Event::Data(value) => Some(value),
            zork_notify::stream::Event::Error(error) => Some(json!({"error":error.to_string()})),
            zork_notify::stream::Event::Heartbeat => None,
        },
    ))
}

struct ToolSource {
    state: AppState,
    request: Watch,
    is_local: bool,
}
impl zork_notify::stream::Source for ToolSource {
    type Item = Value;
    type Error = anyhow::Error;
    fn check_access(&self) -> Result<()> {
        node_access::manage(&self.state, &self.request.subject, self.is_local)
    }
    async fn read(&mut self) -> Result<zork_notify::stream::Page<Value>> {
        use zork_notify::stream::Page;
        let request = &self.request;
        let value = crate::node_tools::snapshot(
            &self.state,
            &request.subject,
            &request.id,
            request.offset,
            self.is_local,
        )
        .await?;
        let more = value["chunk"]["eof"] == false;
        let done = !active(&value) && !more;
        let frame = json!({"value":value,"done":done});
        Ok(if more || done {
            Page::chunk(frame, more, done)
        } else {
            Page::snapshot(frame)
        })
    }
    fn delivered(&mut self, value: &Value) {
        if let Some(offset) = value["value"]["chunk"]["next_offset"].as_u64() {
            self.request.offset = offset;
        }
    }
}

pub(crate) async fn remote(
    state: AppState,
    peer: &str,
    request: Watch,
) -> Result<mpsc::Receiver<Value>> {
    node_access::remote(&request.subject, peer)?;
    ensure!(
        zork_config::load_config(&state.config.data_root)?
            .mesh
            .peers
            .iter()
            .any(|p| p.origin == peer),
        "mesh_peer_not_paired"
    );
    local(state, request, false).await
}
pub(crate) async fn http(State(state): State<AppState>, Json(input): Json<Input>) -> Response {
    match subscribe(&state, input).await {
        Ok(rx) => {
            let stream = futures_util::stream::unfold(rx, |mut rx| async move {
                rx.recv().await.map(|value| {
                    let mut bytes = serde_json::to_vec(&value).expect("tool event JSON");
                    bytes.push(b'\n');
                    (Ok::<_, std::convert::Infallible>(bytes), rx)
                })
            });
            (
                [("content-type", "application/x-ndjson")],
                axum::body::Body::from_stream(stream),
            )
                .into_response()
        }
        Err(error) => (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error":error.to_string()})),
        )
            .into_response(),
    }
}
async fn subscribe(state: &AppState, input: Input) -> Result<mpsc::Receiver<Value>> {
    node_access::ready(state)?;
    let mut subject = node_access::subject(state, &input.session_id)?;
    let owner = match input.domain.as_str() {
        "device" => crate::node_tools::watch_owner(state, &subject, &input.id)?,
        _ => anyhow::bail!("invalid_tool_domain"),
    };
    if owner == "local" || owner == node_access::identity(state) {
        return local(
            state.clone(),
            Watch {
                subject,
                domain: input.domain,
                id: input.id,
                offset: input.offset,
            },
            true,
        )
        .await;
    }
    subject.origin = node_access::identity(state);
    state
        .mesh
        .get()
        .context("mesh_unavailable")?
        .watch_tool(
            &owner,
            Watch {
                subject,
                domain: input.domain,
                id: input.id,
                offset: input.offset,
            },
        )
        .await
}

/// Only constructed at a boundary that knows dispatch never occurred.
#[derive(Debug)]
pub(crate) struct Rejected(pub anyhow::Error);
impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}
impl std::error::Error for Rejected {}
