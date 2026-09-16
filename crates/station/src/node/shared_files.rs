use super::{authorized, error, NodeState};
use anyhow::{Context, Result};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::Value;
use std::sync::Arc;
use zork_mesh::node::{ObjectRef, TreeQuery};

#[derive(Debug)]
struct AuthenticationRequired;
impl std::fmt::Display for AuthenticationRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Node administrator token required")
    }
}
impl std::error::Error for AuthenticationRequired {}
fn failure(value: anyhow::Error, status: StatusCode) -> Response {
    error(
        if value.is::<AuthenticationRequired>() {
            StatusCode::UNAUTHORIZED
        } else {
            status
        },
        &value.to_string(),
    )
}

fn service(
    state: &NodeState,
    headers: &HeaderMap,
) -> Result<Arc<crate::shared_files::SharedFiles>> {
    if !authorized(state, headers) {
        return Err(AuthenticationRequired.into());
    }
    state
        .app
        .mesh
        .get()
        .and_then(|m| m.shared_files.get())
        .cloned()
        .context("请先启用本机 Mesh")
}
fn reply<T: serde::Serialize>(result: Result<T>) -> Response {
    match result {
        Ok(value) => Json(value).into_response(),
        Err(e) => failure(e, StatusCode::BAD_REQUEST),
    }
}
pub(super) async fn catalog(State(state): State<NodeState>, headers: HeaderMap) -> Response {
    reply(async { service(&state, &headers)?.catalog().await }.await)
}
pub(super) async fn directory(
    State(state): State<NodeState>,
    headers: HeaderMap,
    Json(query): Json<TreeQuery>,
) -> Response {
    reply(async { service(&state, &headers)?.directory(query).await }.await)
}
pub(super) async fn content(
    State(state): State<NodeState>,
    headers: HeaderMap,
    Json(content): Json<ObjectRef>,
) -> Response {
    match async { service(&state, &headers)?.content(content).await }.await {
        Ok(bytes) => (
            [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
            bytes,
        )
            .into_response(),
        Err(e) => failure(e, StatusCode::CONFLICT),
    }
}
pub(super) async fn events(
    State(state): State<NodeState>,
    headers: HeaderMap,
    body: Option<Json<TreeQuery>>,
) -> Response {
    let result = async {
        service(&state, &headers)?
            .subscribe(body.map(|Json(q)| q))
            .await
    }
    .await;
    match result {
        Ok(rx) => {
            let stream = futures_util::stream::unfold(rx, |mut rx| async move {
                rx.recv().await.map(|value: Value| {
                    let event = axum::response::sse::Event::default()
                        .event(value["name"].as_str().unwrap_or("shared_files"))
                        .data(value["data"].to_string());
                    (Ok::<_, std::convert::Infallible>(event), rx)
                })
            });
            axum::response::Sse::new(stream).into_response()
        }
        Err(e) => failure(e, StatusCode::BAD_REQUEST),
    }
}
