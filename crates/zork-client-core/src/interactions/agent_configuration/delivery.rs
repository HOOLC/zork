//! Durable transport of Agent configuration responses. Login uses its own
//! transient private requests and cannot enter this submission queue.
use crate::{api::GatewayClient, store::ClientStore};

pub(crate) async fn flush(
    client: &GatewayClient,
    store: &ClientStore,
    node: &str,
) -> anyhow::Result<Option<String>> {
    let mut last_error = None;
    for pending in store.configuration_deliveries(node)? {
        if pending.submission.attempted || pending.submission.error.is_some() {
            continue;
        }
        if !store.change_configuration_submission(node, &pending, |s| s.attempted = true)? {
            continue;
        }
        let mut guard = InteractionGuard {
            store,
            node,
            pending: pending.clone(),
            settled: false,
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            deliver(client, store, node, &pending),
        )
        .await;
        guard.settled = true;
        let error = match result {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some((error.to_string(), error.is::<InteractionRejected>())),
            Err(_) => Some((
                "Submission timed out; recover the original response.".into(),
                false,
            )),
        };
        if let Some((error, rejected)) = error {
            store.change_configuration_submission(node, &pending, |s| {
                s.error = Some(error.clone());
                s.rejected = rejected;
            })?;
            last_error = Some(error);
        }
    }
    Ok(last_error)
}

pub(crate) fn has_pending(store: &ClientStore, node: &str) -> bool {
    store.configuration_deliveries(node).is_ok_and(|items| {
        items
            .iter()
            .any(|item| !item.submission.attempted && item.submission.error.is_none())
    })
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct InteractionRejected(String);

async fn deliver(
    client: &GatewayClient,
    store: &ClientStore,
    node: &str,
    pending: &crate::store::ConfigurationDelivery,
) -> anyhow::Result<()> {
    use anyhow::{ensure, Context};
    if !pending.submission.accepted {
        let value = client
            .node_request(
                reqwest::Method::POST,
                format!(
                    "/v1/node/chats/{}/messages/{}/agent-configuration",
                    pending.session, pending.message_id
                ),
                Some(serde_json::to_value(&pending.submission.response)?),
            )
            .await
            .map_err(|error| {
                // Only a rejected POST permits editing/new admission. Errors
                // while catching up an accepted response remain uncertain.
                if matches!(
                    &error,
                    crate::api::ApiError::Api {
                        status: 400 | 401 | 403 | 404 | 409 | 422,
                        ..
                    }
                ) {
                    anyhow::Error::new(InteractionRejected(error.to_string()))
                } else {
                    anyhow::Error::from(error)
                }
            })?;
        let message: crate::api::TranscriptMessage =
            serde_json::from_value(value["message"].clone())?;
        let crate::api::TranscriptMessage::Message { metadata, .. } = &message;
        let result = crate::interactions::result(metadata)
            .context("Missing authoritative interaction result")?;
        ensure!(
            result.request_message_id == pending.message_id,
            "Interaction response belongs to another request"
        );
        store.change_configuration_submission(node, pending, |s| s.accepted = true)?;
    }
    // A command receipt can arrive ahead of intermediate Chat messages. Catch
    // up from the durable source tail instead of inserting that receipt as a
    // fictitious contiguous page or advancing the stream past unseen messages.
    let tail = store.source_message_tail(node, &pending.session, pending.generation)?;
    let page = client
        .catch_up_messages(&pending.session, tail.as_deref(), 100)
        .await?;
    store.cache_message_page_at(node, &pending.session, &page, None, pending.generation)?;
    ensure!(
        !store
            .configuration_submissions(node, &pending.session, pending.generation)?
            .contains_key(&pending.message_id),
        "Interaction result has not reached the message stream yet"
    );
    Ok(())
}

struct InteractionGuard<'a> {
    store: &'a ClientStore,
    node: &'a str,
    pending: crate::store::ConfigurationDelivery,
    settled: bool,
}
impl Drop for InteractionGuard<'_> {
    fn drop(&mut self) {
        if !self.settled {
            let _ = self
                .store
                .change_configuration_submission(self.node, &self.pending, |s| {
                    s.error = Some("Submission interrupted; recover the original response.".into())
                });
        }
    }
}
