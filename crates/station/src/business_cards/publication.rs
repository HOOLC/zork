use super::*;

pub(super) fn origin(id: &str) -> Result<&str> {
    id.rsplit_once('/')
        .map(|(node, _)| node)
        .filter(|node| !node.is_empty())
        .context("invalid_interaction_reference")
}
pub(super) fn principal(origin: &str, local_client: bool) -> String {
    format!(
        "{origin}/{}",
        if local_client {
            "local-client"
        } else {
            "administrator"
        }
    )
}

/// Only the runtime can attach a publication grant to a Chat RPC. Its owner
/// authenticates the original Agent, and redemption authenticates the recipient.
pub(crate) async fn prepare(
    state: &AppState,
    subject: &Subject,
    target: &str,
    args: &Value,
) -> Result<Option<Grant>> {
    let Some(id) = args["interaction"]["request_id"].as_str() else {
        return Ok(None);
    };
    let owner = origin(id)?;
    let destination = if local(state, target) {
        node_access::identity(state)
    } else {
        target.into()
    };
    if owner == destination {
        return Ok(None);
    };
    let mut who = subject.clone();
    if !local(state, owner) {
        who.origin = node_access::identity(state)
    };
    Ok(Some(serde_json::from_value(
        route(
            state,
            owner,
            Remote::Grant {
                subject: who,
                request_id: id.into(),
                destination,
                chat: args["chat_id"].as_str().context("invalid_chat_id")?.into(),
            },
        )
        .await?,
    )?))
}

pub(crate) async fn publish(
    state: &AppState,
    grant: &Grant,
    chat: &str,
    root: &str,
    actor: &str,
) -> Result<CardRecord> {
    ensure!(
        local(state, &grant.destination),
        "interaction_publication_denied"
    );
    let mut request: CardRecord = serde_json::from_value(
        route(
            state,
            origin(&grant.request_id)?,
            Remote::Bind {
                grant: grant.clone(),
                chat: chat.into(),
                message: root.into(),
            },
        )
        .await?,
    )?;
    if local(state, &request.owner.origin) {
        request.owner.origin = "local".into()
    };
    state
        .db
        .import_user_publication(grant, chat, root, &request, actor)?;
    Ok(request)
}

pub(crate) async fn respond_configuration(
    state: &AppState,
    grant: &Grant,
    chat: &str,
    message: &str,
    local_client: bool,
    input: zork_client_types::interaction::Response,
) -> Result<()> {
    let record: CardRecord = serde_json::from_value(
        route(
            state,
            origin(&grant.request_id)?,
            Remote::ConfigurationResponse {
                grant: grant.clone(),
                chat: chat.into(),
                message: message.into(),
                local_client,
                input,
            },
        )
        .await
        .map_err(|error| {
            let reason = error.to_string();
            if reason.starts_with("invalid_")
                || reason.starts_with("interaction_")
                || reason == "idempotency_conflict"
                || reason.starts_with("agent_configuration_")
            {
                error
            } else {
                error.context("interaction_response_delivery_unknown")
            }
        })?,
    )?;
    if let Some(result) = record.result {
        state.db.receive_user_publication(&Delivery {
            grant: grant.clone(),
            chat: chat.into(),
            message: message.into(),
            result,
        })?;
    }
    Ok(())
}

pub(crate) async fn provider_login_action(
    state: &AppState,
    grant: &Grant,
    chat: &str,
    message: &str,
    local_client: bool,
    body: Option<Value>,
    cancel: bool,
) -> Result<Value> {
    // The authenticated private channel carries callback input; it is never
    // stored in the publication outbox or a public message.
    route(
        state,
        origin(&grant.request_id)?,
        Remote::ProviderLogin {
            grant: grant.clone(),
            chat: chat.into(),
            message: message.into(),
            local_client,
            body,
            cancel,
        },
    )
    .await
}
