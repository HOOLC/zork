//! Agent configuration card operations. The original create/update tool owns
//! validation and effects; this business module owns receiving its card input.
use crate::{
    db::{
        chats::{cards::SubmittedResponse, Topic},
        interaction_registry::{Cleanup, Registration},
    },
    node_access::Subject,
    state::AppState,
};
use anyhow::Result;
use zork_client_types::interaction::{Request, Response};

pub(crate) fn begin(
    state: &AppState,
    owner: &Subject,
    invocation: &str,
    form: &Request,
) -> Result<crate::db::chats::cards::CardRecord> {
    state.db.create_configuration_review(
        &crate::node_access::identity(state),
        owner,
        invocation,
        form,
    )
}

pub(crate) fn cleanup<'a>(
    state: &'a AppState,
    entry: &'a Registration,
    reason: Cleanup,
) -> futures_util::future::BoxFuture<'a, Result<()>> {
    Box::pin(async move {
        state
            .db
            .cleanup_configuration_review(&entry.request_id, reason)
    })
}

pub(crate) async fn next_submission(state: &AppState, id: &str) -> Result<SubmittedResponse> {
    let mut changes = state
        .db
        .chat_topics
        .subscribe([Topic::UserInteraction(id.into())])
        .merge(state.db.realtime.listen(crate::realtime::MESH));
    loop {
        changes.checkpoint();
        let request = state.db.business_card(id)?;
        if request.owner.origin != "local" {
            crate::channels::access(state, &request.owner.origin)?;
        }
        if let Some(result) = request.result.as_ref().filter(|r| r.outcome.terminal()) {
            anyhow::bail!(
                "interaction_{}",
                serde_json::to_value(result.outcome)?
                    .as_str()
                    .unwrap_or("settled")
            );
        }
        if let Some(submitted) = request.submission {
            return Ok(submitted);
        }
        changes.changed().await?;
    }
}

pub(crate) async fn respond_request(
    state: &AppState,
    id: &str,
    input: &Response,
    actor: &str,
) -> anyhow::Result<crate::db::chats::cards::CardRecord> {
    let mut changes = state
        .db
        .chat_topics
        .subscribe([crate::db::chats::Topic::UserInteraction(id.into())]);
    state.db.submit_configuration_response(id, input, actor)?;
    loop {
        changes.checkpoint();
        if let Some((status, error)) = state
            .db
            .configuration_response_state(id, &input.response_id)?
        {
            if status == "rejected" {
                anyhow::bail!(
                    "invalid_agent_configuration_input: {}",
                    error.unwrap_or_default()
                );
            }
            if status == "accepted" {
                return state.db.business_card(id);
            }
        }
        let request = state.db.business_card(id)?;
        if request.result.as_ref().is_some_and(|r| {
            r.outcome.terminal()
                || request
                    .submission
                    .as_ref()
                    .is_some_and(|s| s.response.response_id != input.response_id)
        }) {
            return Ok(request);
        }
        changes.changed().await?;
    }
}
