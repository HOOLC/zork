//! Read-only inspection of registered device services.
use super::{authorized, error, NodeState};
use anyhow::Result;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;

fn denied() -> Response {
    error(
        StatusCode::UNAUTHORIZED,
        "Node administrator token required",
    )
}
fn reply<T: serde::Serialize>(result: Result<T>) -> Response {
    match result {
        Ok(value) => Json(value).into_response(),
        Err(e) => error(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}
#[derive(Default, Deserialize)]
pub(super) struct ReadQuery {
    log: Option<String>,
}
pub(super) async fn service(
    State(state): State<NodeState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<ReadQuery>,
) -> Response {
    if !authorized(&state, &headers) {
        return denied();
    }
    let Some(mesh) = state.app.mesh.get().cloned() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "Mesh is not ready");
    };
    reply(
        tokio::task::spawn_blocking(move || {
            mesh.services.client_details(&id, query.log.as_deref())
        })
        .await
        .map_err(anyhow::Error::from)
        .and_then(|r| r),
    )
}
