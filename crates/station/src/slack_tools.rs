//! Explicit connection routing. Session identity authenticates the caller; it
//! does not choose a Slack connection or rewrite the requested destination.
use crate::state::AppState;
use axum::{
    extract::{Query, State},
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
pub struct Caller {
    session_id: String,
}
#[derive(Deserialize)]
pub struct Forward {
    session_id: String,
    connect_id: String,
    method: String,
    arguments: Value,
}

fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"ok":false,"error":message}))).into_response()
}
async fn caller(state: &AppState, id: &str) -> Result<(), Response> {
    state
        .agent
        .get_session(id.into())
        .await
        .map(|_| ())
        .map_err(|_| error(StatusCode::NOT_FOUND, "session_not_found"))
}
pub async fn connections(State(state): State<AppState>, Query(request): Query<Caller>) -> Response {
    if let Err(response) = caller(&state, &request.session_id).await {
        return response;
    }
    let mut rows=state.connections.configs().await.into_iter().filter(|c|c.provider_name()=="slack")
        .map(|c|json!({"connect_id":c.id,"name":c.name,"enabled":c.enabled,"configured":c.configured()})).collect::<Vec<_>>();
    rows.sort_by(|a, b| a["connect_id"].as_str().cmp(&b["connect_id"].as_str()));
    Json(json!({"connections":rows})).into_response()
}
pub async fn forward(State(state): State<AppState>, Json(request): Json<Forward>) -> Response {
    if let Err(response) = caller(&state, &request.session_id).await {
        return response;
    }
    if request.connect_id.trim().is_empty() {
        return error(StatusCode::BAD_REQUEST, "connect_id_required");
    }
    let Some(connection) = state.connections.runtime(&request.connect_id).await else {
        return error(StatusCode::NOT_FOUND, "connect_id_not_found");
    };
    if !connection.config.enabled || !connection.config.configured() {
        return error(StatusCode::CONFLICT, "connect_id_unavailable");
    }
    match connection
        .slack
        .api()
        .forward(&request.method, &request.arguments)
        .await
    {
        Ok(reply) => {
            let status = StatusCode::from_u16(reply.status).unwrap_or(StatusCode::BAD_GATEWAY);
            let mut response = (status, Json(reply.body)).into_response();
            if let Some(value) = reply
                .retry_after
                .and_then(|v| HeaderValue::from_str(&v).ok())
            {
                response.headers_mut().insert("retry-after", value);
            }
            response
        }
        Err(failure) => {
            let message = failure.to_string();
            error(
                if message.starts_with("slack_delivery_unknown") {
                    StatusCode::BAD_GATEWAY
                } else {
                    StatusCode::BAD_REQUEST
                },
                &message,
            )
        }
    }
}
