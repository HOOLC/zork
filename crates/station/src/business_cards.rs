//! Business card publication and its registered business endpoints. This layer
//! carries card content; the separate registration runtime does not.
use crate::{
    db::chats::{
        cards::{
            publication::{Delivery, Grant},
            CardRecord,
        },
        Topic,
    },
    node_access::{self, Subject},
    state::AppState,
};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
mod publication;
pub(crate) use publication::{prepare, provider_login_action, publish, respond_configuration};

pub(crate) fn lifecycle_handlers() -> Result<crate::interaction_registry::Handlers> {
    let mut handlers = crate::interaction_registry::Handlers::default();
    for adapter in crate::db::chats::cards::adapters() {
        handlers.register(adapter.key, adapter.cleanup)?;
    }
    Ok(handlers)
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Remote {
    Grant {
        subject: Subject,
        request_id: String,
        destination: String,
        chat: String,
    },
    Bind {
        grant: Grant,
        chat: String,
        message: String,
    },
    Phase {
        delivery: Delivery,
    },
    ConfigurationResponse {
        grant: Grant,
        chat: String,
        message: String,
        local_client: bool,
        input: zork_client_types::interaction::Response,
    },
    ProviderLogin {
        grant: Grant,
        chat: String,
        message: String,
        local_client: bool,
        body: Option<Value>,
        cancel: bool,
    },
}

fn access(state: &AppState, peer: &str) -> Result<()> {
    ensure!(
        zork_config::load_config(&state.config.data_root)?
            .mesh
            .peers
            .iter()
            .any(|p| p.origin == peer),
        "interaction_access_denied"
    );
    Ok(())
}
fn local(state: &AppState, target: &str) -> bool {
    target == "local" || target == node_access::identity(state)
}

async fn route(state: &AppState, target: &str, request: Remote) -> Result<Value> {
    if local(state, target) {
        execute(state, request).await
    } else {
        access(state, target)?;
        state
            .mesh
            .get()
            .context("node_starting")?
            .business_card_call(target, request)
            .await
    }
}

pub(crate) async fn remote(state: &AppState, peer: &str, request: Remote) -> Result<Value> {
    access(state, peer)?;
    match &request {
        Remote::Grant { subject, .. } => node_access::remote(subject, peer)?,
        Remote::Bind { grant, .. } => {
            ensure!(grant.destination == peer, "interaction_publication_denied")
        }
        Remote::ConfigurationResponse { grant, .. } | Remote::ProviderLogin { grant, .. } => {
            ensure!(grant.destination == peer, "interaction_publication_denied");
            node_access::manage(
                state,
                &Subject {
                    origin: peer.into(),
                    agent: String::new(),
                    session: String::new(),
                },
                false,
            )?;
        }
        Remote::Phase { delivery } => ensure!(
            publication::origin(&delivery.grant.request_id)? == peer
                && local(state, &delivery.grant.destination),
            "interaction_publication_denied"
        ),
    }
    execute(state, request).await
}

async fn execute(state: &AppState, request: Remote) -> Result<Value> {
    match request {
        Remote::Grant {
            subject,
            request_id,
            destination,
            chat,
        } => {
            access(state, &destination)?;
            Ok(serde_json::to_value(state.db.grant_user_publication(
                &request_id,
                &subject,
                &destination,
                &chat,
            )?)?)
        }
        Remote::Bind {
            grant,
            chat,
            message,
        } => Ok(serde_json::to_value(state.db.redeem_user_publication(
            &grant,
            &chat,
            &message,
            &node_access::identity(state),
        )?)?),
        Remote::Phase { delivery } => {
            state.db.receive_user_publication(&delivery)?;
            Ok(json!({"accepted":true}))
        }
        Remote::ConfigurationResponse {
            grant,
            chat,
            message,
            local_client,
            input,
        } => {
            state.db.check_user_publication(&grant, &chat, &message)?;
            let actor = publication::principal(&grant.destination, local_client);
            Ok(serde_json::to_value(
                crate::agent_configuration::respond_request(
                    state,
                    &grant.request_id,
                    &input,
                    &actor,
                )
                .await?,
            )?)
        }
        Remote::ProviderLogin {
            grant,
            chat,
            message,
            local_client,
            body,
            cancel,
        } => {
            state.db.check_user_publication(&grant, &chat, &message)?;
            crate::provider_login::private_response(
                state,
                &grant.request_id,
                body,
                cancel,
                &publication::principal(&grant.destination, local_client),
            )
            .await
        }
    }
}

pub(crate) fn start(state: AppState) -> Result<zork_notify::Task<()>> {
    Ok(zork_notify::Task(tokio::spawn(async move {
        let mut changes = state
            .db
            .chat_topics
            .subscribe([Topic::UserInteractionDelivery])
            .merge(state.db.realtime.listen(crate::realtime::MESH));
        let mut running = std::collections::HashSet::new();
        let mut tasks = tokio::task::JoinSet::new();
        let mut retry = zork_notify::retry::Retry::default();
        loop {
            changes.checkpoint();
            match state.db.pending_user_publications() {
                Ok(deliveries) => {
                    for delivery in deliveries {
                        let key = format!("publication-{}", delivery.grant.token);
                        if !running.insert(key.clone()) {
                            continue;
                        }
                        let state = state.clone();
                        tasks.spawn(async move {
                            let mut retry = zork_notify::retry::Retry::default();
                            loop {
                                if route(
                                    &state,
                                    &delivery.grant.destination,
                                    Remote::Phase {
                                        delivery: delivery.clone(),
                                    },
                                )
                                .await
                                .is_ok()
                                    && state.db.acknowledge_user_publication(&delivery).is_ok()
                                {
                                    break;
                                }
                                retry.wait().await;
                            }
                            key
                        });
                    }
                }
                Err(_) => {
                    retry.wait().await;
                    continue;
                }
            }
            tokio::select! {
                changed = changes.changed() => if changed.is_err() { return },
                Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                    match result {
                        Ok(key) => { running.remove(&key); },
                        Err(error) => { tracing::error!(%error, "User interaction delivery worker failed"); return },
                    }
                }
            }
        }
    })))
}
