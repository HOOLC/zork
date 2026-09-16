//! The Provider login business owns its card, private input, authorization and
//! final result in the original tool invocation.
use crate::{
    db::{
        chats::Topic,
        interaction_registry::{Cleanup, Registration},
    },
    node::auth,
    node_access,
    state::AppState,
};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use zork_client_types::interaction::{Outcome, Request};
fn field<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args[key]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .with_context(|| format!("invalid_{key}"))
}

pub(crate) fn cleanup<'a>(
    state: &'a AppState,
    entry: &'a Registration,
    reason: Cleanup,
) -> futures_util::future::BoxFuture<'a, Result<()>> {
    Box::pin(async move {
        let _guard = state
            .entries
            .lock_local_task(&format!("user-interaction:{}", entry.request_id))
            .await;
        state.db.cleanup_provider_login(&entry.request_id, reason)?;
        let attempt =
            node_access::fingerprint(&(&entry.owner, &entry.invocation_id, "provider.login"))?;
        auth::cancel(state, &attempt).await;
        Ok(())
    })
}

pub(crate) async fn execute(
    state: &AppState,
    rpc: &crate::channels::Rpc,
    key: &str,
    message_id: &str,
) -> Result<Value> {
    let args = &rpc.arguments["oauth"];
    ensure!(args["kind"] == "profile", "unsupported_oauth_connection");
    let provider = zork_profile::providers::get(field(args, "provider")?)?;
    ensure!(
        provider.supports_device_code(field(args, "billing")?),
        "This provider and billing mode do not support browser sign-in"
    );
    let title = provider.info().label;
    let request = state.db.create_provider_login_card(
        &node_access::identity(state),
        &rpc.subject,
        &rpc.invocation_id,
        title,
    )?;
    let channel = state.db.chat(field(&rpc.arguments, "chat_id")?)?;
    let author = zork_client_types::chat::Author {
        id: crate::channels::request_actor(&rpc.subject),
        kind: zork_client_types::chat::AuthorKind::Agent,
        name: None,
    };
    let content = zork_client_types::interaction::MessageContent::linked(
        request.request_id.clone(),
        request.handler.clone(),
        request.request.clone(),
        request.result.clone(),
    );
    let mentions: Vec<String> = rpc
        .arguments
        .get("mentions")
        .cloned()
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or_default();
    let message = state.db.post_chat_content(
        None,
        message_id,
        &channel.channel.chat_id,
        &author,
        rpc.arguments["text"].as_str().unwrap_or(""),
        &[],
        rpc.arguments["reply_to"].as_str(),
        &mentions,
        &[],
        Some(&serde_json::to_value(&content)?),
    )?;
    state
        .entries
        .publish_visible_message(&state.db.chat_visible_message(&message.message_id)?);
    let id = &request.request_id;
    if let Some(result) = request.result.as_ref().filter(|r| r.outcome.terminal()) {
        return Ok(
            json!({"status":if result.outcome==Outcome::Cancelled{"cancelled"}else{"rejected"},"result":result,"request_id":id}),
        );
    }
    let attempt = node_access::fingerprint(&(&rpc.subject, &rpc.invocation_id, "provider.login"))?;
    let result:Result<Value>=async {
        let authorization=auth::begin(state,auth::Begin{profile_id:field(args,"profile_id")?.into(),provider:field(args,"provider")?.into(),billing:field(args,"billing")?.into()},Some(&attempt))
            .await.map_err(anyhow::Error::new)?;
        let mut private=private_session(state,id,authorization);
        state.db.provider_login_progress(id)?;
        let mut changes=state.db.chat_topics.subscribe([Topic::UserInteraction(id.clone())]);
        let mut body=auth::Poll::default();
        loop {
            changes.checkpoint();
            let _guard=state.entries.lock_local_task(&format!("user-interaction:{id}")).await;
            if let Some(result)=state.db.business_card(id)?.result.filter(|r|r.outcome.terminal()){
                return Ok(json!({"status":if result.outcome==Outcome::Cancelled{"cancelled"}else{"rejected"},"request_id":id,"result":result}));
            }
            ensure!(state.db.interaction_registration(id)?.active,"interaction_cancelled");
            let reply=auth::poll(state,&attempt,body).await.map_err(anyhow::Error::new)?;
            if reply["status"]!="pending" {
                let result=json!({"profile_id":args["profile_id"],"connected":true,"request_id":id});
                state.db.finish_provider_login(key,id,Outcome::Completed,&result)?;
                return Ok(result);
            }
            let delay=reply["retry_after_seconds"].as_u64().unwrap_or(1).max(1);
            drop(_guard);
            body=tokio::select! {
                input=private.input.recv()=>serde_json::from_value(input.context("interaction_private_channel_closed")?)?,
                _=tokio::time::sleep(std::time::Duration::from_secs(delay))=>auth::Poll::default(),
                _=changes.changed()=>auth::Poll::default(),
            };
        }
    }.await;
    if let Err(error) = &result {
        let current = state.db.business_card(id)?;
        if current
            .result
            .as_ref()
            .is_some_and(|r| r.outcome == Outcome::Cancelled)
        {
            auth::cancel(state, &attempt).await;
            return Ok(json!({"status":"cancelled","request_id":id,"result":current.result}));
        }
        if current
            .result
            .as_ref()
            .is_none_or(|r| !r.outcome.terminal())
        {
            let outcome = if let Some(failure) = error.downcast_ref::<auth::Failure>() {
                if failure.effects_unconfirmed {
                    Outcome::Unknown
                } else if failure.status == axum::http::StatusCode::GONE {
                    Outcome::Expired
                } else {
                    Outcome::Failed
                }
            } else if error.chain().any(|e| e.is::<rusqlite::Error>()) {
                Outcome::Unknown
            } else {
                Outcome::Failed
            };
            let value = json!({"status":"rejected","error":error.to_string(),"request_id":id,"effects_may_have_occurred":outcome==Outcome::Unknown});
            state.db.finish_provider_login(key, id, outcome, &value)?;
        }
    }
    if state
        .db
        .business_card(id)?
        .result
        .as_ref()
        .is_some_and(|r| r.outcome.terminal())
    {
        auth::cancel(state, &attempt).await;
    }
    match result {
        Ok(value) => Ok(value),
        Err(error) => Ok(
            json!({"status":"rejected","error":error.to_string(),"request_id":id,"result":state.db.business_card(id)?.result}),
        ),
    }
}
/// Private view and callback channel owned by this login operation.
#[derive(Default)]
pub(crate) struct Hub {
    slots: std::sync::Mutex<std::collections::HashMap<String, PrivateSlot>>,
}
struct PrivateSlot {
    view: Value,
    input: tokio::sync::mpsc::Sender<Value>,
}
pub(crate) struct PrivateSession {
    pub input: tokio::sync::mpsc::Receiver<Value>,
    hub: std::sync::Arc<Hub>,
    id: String,
}
impl Drop for PrivateSession {
    fn drop(&mut self) {
        self.hub.slots.lock().unwrap().remove(&self.id);
    }
}
pub(crate) fn private_session(state: &AppState, id: &str, view: Value) -> PrivateSession {
    let (input, receive) = tokio::sync::mpsc::channel(1);
    state
        .login_cards
        .slots
        .lock()
        .unwrap()
        .insert(id.into(), PrivateSlot { view, input });
    PrivateSession {
        input: receive,
        hub: state.login_cards.clone(),
        id: id.into(),
    }
}
pub(crate) async fn private_response(
    state: &AppState,
    id: &str,
    body: Option<Value>,
    cancel: bool,
    _actor: &str,
) -> Result<Value> {
    let request = state.db.business_card(id)?;
    ensure!(
        request.handler == zork_client_types::interaction::PROVIDER_LOGIN,
        "interaction_handler_mismatch"
    );
    ensure!(
        matches!(request.request, Request::OAuth { .. }),
        "interaction_private_channel_required"
    );
    if request
        .result
        .as_ref()
        .is_some_and(|r| r.outcome.terminal())
    {
        return Ok(json!({"result":request.result}));
    }
    if cancel {
        crate::interaction_registry::cancel(state, &request.owner, &request.invocation_id).await?;
        return Ok(json!({"result":state.db.business_card(id)?.result}));
    }
    let slots = state.login_cards.slots.lock().unwrap();
    let slot = slots
        .get(id)
        .context("interaction_private_channel_unavailable")?;
    if let Some(input) = body {
        ensure!(
            serde_json::to_vec(&input)?.len() <= 32768,
            "interaction_input_too_large"
        );
        slot.input
            .try_send(input)
            .map_err(|_| anyhow::anyhow!("interaction_input_pending"))?;
    }
    Ok(json!({"result":request.result,"authorization":slot.view}))
}
