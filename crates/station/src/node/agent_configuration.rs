//! The Agent configuration business endpoint for its review cards.
use super::*;
use anyhow::{ensure, Context};
use zork_client_types::interaction::{Content, MessageContent, Response as Input};

pub(super) async fn respond(
    State(state): State<NodeState>,
    headers: HeaderMap,
    Path((chat, message)): Path<(String, String)>,
    Json(input): Json<Input>,
) -> Response {
    if !authorized(&state, &headers) {
        return error(
            StatusCode::UNAUTHORIZED,
            "Node administrator token required",
        );
    }
    let local = headers.get("authorization").and_then(|h| h.to_str().ok())
        == Some(format!("Bearer {}", state.local_token).as_str());
    let actor = format!(
        "{}/{}",
        crate::node_access::identity(&state.app),
        if local {
            "local-client"
        } else {
            "administrator"
        }
    );
    let result: anyhow::Result<Value> = async {
        let chat = state.app.db.chat(&chat)?.channel.chat_id;
        let source = state.app.db.chat_message(&chat, &message)?;
        let content = source
            .interaction
            .as_ref()
            .and_then(MessageContent::parse)
            .context("interaction_request_required")?;
        ensure!(
            content.handler == zork_client_types::interaction::AGENT_CONFIGURATION,
            "interaction_handler_mismatch"
        );
        ensure!(
            matches!(content.content, Content::Request { .. }),
            "interaction_request_required"
        );
        ensure!(
            state.app.db.bound_business_card(&chat, &message)? == content.request_id,
            "interaction_binding_mismatch"
        );
        if let Some(grant) = state.app.db.user_publication_grant(&chat, &message)? {
            crate::business_cards::respond_configuration(
                &state.app,
                &grant,
                &chat,
                &message,
                local,
                input.clone(),
            )
            .await?;
        } else {
            crate::agent_configuration::respond_request(
                &state.app,
                &content.request_id,
                &input,
                &actor,
            )
            .await?;
        }
        let result = state
            .app
            .db
            .interaction_result(&chat, &message)?
            .context("interaction_result_missing")?;
        Ok(state
            .app
            .entries
            .message_json(&state.app.db.chat_visible_message(&result.message_id)?))
    }
    .await;
    match result {
        Ok(message) => Json(json!({"message":message})).into_response(),
        Err(err) => {
            let status = if err.to_string() == "agent_configuration_submission_pending" {
                StatusCode::CONFLICT
            } else if err.to_string() == "interaction_response_delivery_unknown"
                || err
                    .chain()
                    .any(|e| e.is::<rusqlite::Error>() || e.is::<serde_json::Error>())
            {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::BAD_REQUEST
            };
            error(status, &err.to_string())
        }
    }
}
