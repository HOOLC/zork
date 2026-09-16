//! Provider login card endpoints. Their content and requests belong to login.
use super::*;
async fn action(
    state: NodeState,
    headers: HeaderMap,
    chat: String,
    message: String,
    body: Option<Value>,
    cancel: bool,
) -> Response {
    if !authorized(&state, &headers) {
        return error(
            StatusCode::UNAUTHORIZED,
            "Node administrator token required",
        );
    }
    let result: anyhow::Result<Value> = async {
        let id = state.app.db.bound_business_card(&chat, &message)?;
        anyhow::ensure!(
            state.app.db.business_card(&id)?.handler
                == zork_client_types::interaction::PROVIDER_LOGIN,
            "interaction_handler_mismatch"
        );
        let local = headers.get("authorization").and_then(|h| h.to_str().ok())
            == Some(format!("Bearer {}", state.local_token).as_str());
        if let Some(grant) = state.app.db.user_publication_grant(&chat, &message)? {
            crate::business_cards::provider_login_action(
                &state.app, &grant, &chat, &message, local, body, cancel,
            )
            .await
        } else {
            crate::provider_login::private_response(
                &state.app,
                &id,
                body,
                cancel,
                &format!(
                    "{}/{}",
                    crate::node_access::identity(&state.app),
                    if local {
                        "local-client"
                    } else {
                        "administrator"
                    }
                ),
            )
            .await
        }
    }
    .await;
    match result {
        Ok(value) => Json(value).into_response(),
        Err(err) => error(StatusCode::BAD_REQUEST, &err.to_string()),
    }
}
pub(super) async fn get(
    State(state): State<NodeState>,
    headers: HeaderMap,
    Path((chat, message)): Path<(String, String)>,
) -> Response {
    action(state, headers, chat, message, None, false).await
}
pub(super) async fn post(
    State(state): State<NodeState>,
    headers: HeaderMap,
    Path((chat, message)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Response {
    action(state, headers, chat, message, Some(body), false).await
}
pub(super) async fn cancel(
    State(state): State<NodeState>,
    headers: HeaderMap,
    Path((chat, message)): Path<(String, String)>,
) -> Response {
    action(state, headers, chat, message, None, true).await
}
