//! Durable delivery shared by desktop and mobile. Commit attempted before IO;
//! source echo updates the same local row. Every manual resend has a new ID.
use crate::{api::StationClient, store::ClientStore};
use serde::Serialize;
#[derive(Clone, Default, Serialize)]
pub struct DeliveryReport {
    pub delivered: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
pub async fn flush(client: &StationClient, store: &ClientStore, node: &str) -> DeliveryReport {
    let _serial = client.delivery_gate.lock().await;
    let mut report = DeliveryReport::default();
    let result: anyhow::Result<()> = async {
        for pending in store.outbox(node)? {
            let Some(message) = store.begin_delivery(node, &pending.request_id)? else {
                continue;
            };
            let mut guard = AttemptGuard {
                store,
                node,
                id: &message.request_id,
                settled: false,
            };
            let result = tokio::time::timeout(
                delivery_timeout(&message),
                deliver(client, store, node, &message),
            )
            .await;
            guard.settled = true;
            match result {
                Ok(Ok(_)) => {
                    // A successful request is not the message echo. Leave the
                    // saved row sending until the receive path updates it.
                    if store
                        .outbox(node)?
                        .iter()
                        .any(|m| m.request_id == message.request_id)
                    {
                        continue;
                    }
                }
                result => {
                    if !store
                        .outbox(node)?
                        .iter()
                        .any(|m| m.request_id == message.request_id)
                    {
                        report.delivered.push(message.request_id.clone());
                        continue;
                    }
                    let error = match result {
                        Ok(Err(e)) => delivery_error(&e),
                        _ => "发送超时".into(),
                    };
                    store.fail_delivery(node, &message.request_id, &error)?;
                    report.error = Some(error);
                    continue;
                }
            }
            report.delivered.push(message.request_id.clone());
        }
        if let Some(error) =
            crate::interactions::agent_configuration::delivery::flush(client, store, node).await?
        {
            report.error = Some(error);
        }
        Ok(())
    }
    .await;
    if let Err(error) = result {
        report.error = Some(error.to_string());
    }
    report
}

fn delivery_error(error: &anyhow::Error) -> String {
    if error
        .downcast_ref::<crate::api::ApiError>()
        .and_then(crate::api::ApiError::status)
        == Some(422)
    {
        return "目标设备版本过旧，不支持当前客户端发送消息，请先更新目标设备。".into();
    }
    error.to_string()
}

fn delivery_timeout(message: &crate::store::QueuedMessage) -> std::time::Duration {
    let seconds = match zork_client_types::files::decode(&message.content) {
        Some((_, references)) => {
            let bytes = references
                .iter()
                .fold(0u64, |total, file| {
                    total.saturating_add(file.byte_len as u64)
                })
                .min(zork_client_types::files::MAX_MESSAGE_BYTES as u64);
            // Keep setup/receipt grace, then allow time proportional to the
            // bounded payload instead of cutting off every upload at 2 minutes.
            120 + bytes.div_ceil(256 * 1024)
        }
        None => 15,
    };
    std::time::Duration::from_secs(seconds)
}

async fn deliver(
    client: &StationClient,
    store: &ClientStore,
    node: &str,
    message: &crate::store::QueuedMessage,
) -> anyhow::Result<()> {
    use anyhow::{ensure, Context};
    use zork_client_types::files;
    if let Some((_, references)) = files::decode(&message.content) {
        ensure!(files::valid(&references), "invalid attachments");
        for file in references {
            let bytes = store
                .blob(node, &format!("upload:{}", file.id))?
                .context("attachment snapshot missing")?;
            ensure!(
                bytes.len() == file.byte_len
                    && zork_mesh::content_root(&bytes) == file.content_root,
                "attachment snapshot changed"
            );
            let mut offset = 0;
            loop {
                let end = (offset + files::CHUNK_BYTES).min(bytes.len());
                let reply = client.node_request(reqwest::Method::POST,
                    format!("/v1/im/sessions/{}/files", message.session_id),
                    Some(serde_json::json!({"file":file,"offset":offset,"bytes":&bytes[offset..end]}))).await?;
                let received = reply["received"]
                    .as_u64()
                    .context("invalid upload receipt")? as usize;
                ensure!(
                    received >= end && received <= bytes.len(),
                    "invalid upload offset"
                );
                if received == bytes.len() {
                    break;
                }
                ensure!(received > offset, "upload made no progress");
                offset = received;
            }
        }
    }
    client
        .post_message_id(&message.session_id, &message.content, &message.request_id)
        .await?;
    Ok(())
}

/// Dispatch each explicit send once. Reconnection never retries a failed request.
/// The store marks interrupted requests failed when reopened.
pub struct DeliveryPump {
    online: tokio::sync::watch::Sender<bool>,
    reports: tokio::sync::watch::Receiver<DeliveryReport>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for DeliveryPump {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl DeliveryPump {
    pub fn start(
        client: std::sync::Arc<StationClient>,
        store: std::sync::Arc<ClientStore>,
        node: String,
    ) -> Self {
        let (online, _connection) = tokio::sync::watch::channel(false);
        let (reports_tx, reports) = tokio::sync::watch::channel(DeliveryReport::default());
        let executor = client.clone();
        let mut changes = store.delivery_events();
        let task = executor.spawn(async move {
            loop {
                changes.borrow_and_update();
                let pending = store.outbox(&node).unwrap_or_default();
                let messages = pending.iter().any(|m| !m.attempted && m.error.is_none());
                let interactions =
                    crate::interactions::agent_configuration::delivery::has_pending(&store, &node);
                if messages || interactions {
                    reports_tx.send_replace(flush(&client, &store, &node).await);
                    continue;
                }
                let now = crate::store::delivery_now_ms();
                let mut next = None;
                let mut expired = false;
                for message in pending.iter().filter(|message|message.attempted && message.error.is_none()) {
                    let deadline = message.sent_at_ms.saturating_add(delivery_timeout(message).as_millis() as u64);
                    if deadline <= now {
                        let _ = store.fail_delivery(&node, &message.request_id, "发送超时，未收到消息确认");
                        expired = true;
                    } else {
                        next = Some(next.map_or(deadline, |old: u64|old.min(deadline)));
                    }
                }
                if expired { continue; }
                if let Some(next) = next {
                    tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_millis(next.saturating_sub(now))) => {},
                        result = changes.changed() => if result.is_err() { return; },
                    }
                } else if changes.changed().await.is_err() {
                    return;
                }
            }
        });
        Self {
            online,
            reports,
            task,
        }
    }
    pub fn set_connected(&self, connected: bool) {
        self.online.send_if_modified(|old| {
            if *old == connected {
                false
            } else {
                *old = connected;
                true
            }
        });
    }
    pub fn reports(&self) -> tokio::sync::watch::Receiver<DeliveryReport> {
        self.reports.clone()
    }
}

struct AttemptGuard<'a> {
    store: &'a ClientStore,
    node: &'a str,
    id: &'a str,
    settled: bool,
}
impl Drop for AttemptGuard<'_> {
    fn drop(&mut self) {
        if !self.settled {
            let _ = self
                .store
                .fail_delivery(self.node, self.id, "发送中断，请手动重发");
        }
    }
}
