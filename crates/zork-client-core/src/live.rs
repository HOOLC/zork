//! One reconnect policy for all clients. Subscribe before catch-up so messages
//! delivered during the read remain buffered. Dropping the feed cancels its IO.
use crate::api::{ApiError, GatewayClient, MessagePage, SseEvent};
use futures_channel::mpsc;
use futures_util::{SinkExt, Stream, StreamExt};
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

pub enum LiveEvent {
    Connected,
    Route(crate::api::ConnectionRoute),
    /// Initial/reconnect or lost-event recovery; restores history silently.
    Page(MessagePage),
    /// A live delivery notification whose payload is fetched from history.
    Messages(MessagePage),
    Event(SseEvent),
    Disconnected {
        error: String,
        revoked: bool,
    },
}
pub struct LiveFeed {
    rx: mpsc::Receiver<LiveEvent>,
    task: tokio::task::JoinHandle<()>,
    applied: tokio::sync::watch::Sender<Option<String>>,
    resync: tokio::sync::watch::Sender<u64>,
}
impl LiveFeed {
    /// Advance only after the consumer has committed and applied this source.
    pub fn acknowledge_messages(&self, anchor: Option<String>) {
        self.applied.send_replace(anchor);
    }
    pub fn resync_messages(&self) {
        self.resync
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }
}
impl Drop for LiveFeed {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Stream for LiveFeed {
    type Item = LiveEvent;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_next_unpin(cx)
    }
}
impl GatewayClient {
    pub fn live(self: &Arc<Self>, session: Option<String>, page_limit: u32) -> LiveFeed {
        self.live_from(session, page_limit, None)
    }
    pub fn live_from(
        self: &Arc<Self>,
        session: Option<String>,
        page_limit: u32,
        anchor: Option<String>,
    ) -> LiveFeed {
        self.follow_events(session, page_limit, anchor, None)
    }
    #[cfg(feature = "desktop")]
    pub(crate) fn browser_events(self: &Arc<Self>, registration: serde_json::Value) -> LiveFeed {
        self.follow_events(
            None,
            0,
            None,
            Some(("/v1/client/browser/events".into(), registration)),
        )
    }
    fn follow_events(
        self: &Arc<Self>,
        session: Option<String>,
        page_limit: u32,
        anchor: Option<String>,
        browser: Option<(String, serde_json::Value)>,
    ) -> LiveFeed {
        let (mut tx, rx) = mpsc::channel(64);
        let (applied, anchors) = tokio::sync::watch::channel(anchor);
        let (resync, mut resyncs) = tokio::sync::watch::channel(0u64);
        let client = self.clone();
        let task = self.spawn(async move {
            let mut retry = zork_notify::retry::Retry::default();
            let mut confirmed = false;
            'connection: loop {
                let result = tokio::time::timeout(Duration::from_secs(15), async {
                    if let Some((path, registration)) = &browser {
                        client.stream_request(path.clone(), Some(registration.clone())).await
                    } else {
                        match &session {
                            Some(id) => client.stream_events(id).await,
                            None => client.stream_updates().await,
                        }
                    }
                })
                .await
                .unwrap_or_else(|_| {
                    Err(ApiError::Task(std::io::Error::other(
                        "event connection timed out",
                    )))
                });
                let failure = match result {
                    Ok(mut stream) => {
                        confirmed = true;
                        if tx.send(LiveEvent::Connected).await.is_err() {
                            return;
                        }
                        let mut route = stream.route.take();
                        if let Some(receiver) = &mut route {
                            let current = *receiver.borrow_and_update();
                            if tx.send(LiveEvent::Route(current)).await.is_err() { return; }
                        }
                        let mut failure = None;
                        let mut deferred = None;
                        if session.is_some() {
                            match tokio::time::timeout(Duration::from_secs(15), stream.next()).await {
                                Ok(Some(Ok(event))) if event.name == "snapshot" => {
                                    if tx.send(LiveEvent::Event(event)).await.is_err() { return; }
                                }
                                Ok(Some(Ok(event))) => deferred = Some(event),
                                Ok(Some(Err(error))) => failure = Some(error),
                                _ => failure = Some(ApiError::Task(std::io::Error::other("session snapshot was not received"))),
                            }
                        }
                        // Execution state is initialized by the SSE snapshot
                        // before independently restoring delivered chat messages.
                        if let Some(id) = session.as_ref().filter(|_| failure.is_none()) {
                            let anchor = anchors.borrow().clone();
                            match client
                                .catch_up_messages(id, anchor.as_deref(), page_limit)
                                .await
                            {
                                Ok(page) => {
                                    if tx.send(LiveEvent::Page(page)).await.is_err() {
                                        return;
                                    }
                                }
                                Err(error) => failure = Some(error),
                            }
                        }
                        if failure.is_none() {
                            retry.reset();
                            loop {
                                let frame = if let Some(event) = deferred.take() { Ok(event) } else { tokio::select! {
                                    frame = async {
                                        if browser.is_some() {
                                            tokio::time::timeout(Duration::from_secs(35), stream.next()).await
                                                .unwrap_or_else(|_| Some(Err(ApiError::Task(std::io::Error::other("browser stream heartbeat timed out")))))
                                        } else { stream.next().await }
                                    } => match frame { Some(frame) => frame, None => break },
                                    changed = resyncs.changed() => {
                                        if changed.is_err() { return; }
                                        break;
                                    },
                                    changed = async {
                                        match &mut route {
                                            Some(receiver) => receiver.changed().await,
                                            None => std::future::pending().await,
                                        }
                                    } => {
                                        if changed.is_err() { route = None; continue; }
                                        let current = *route.as_mut().unwrap().borrow_and_update();
                                        if tx.send(LiveEvent::Route(current)).await.is_err() { return; }
                                        continue;
                                    }
                                }};
                                match frame {
                                    Ok(event) => {
                                        let refresh = session.is_some()
                                            && matches!(
                                                event.name.as_str(),
                                                "resync" | "messages_changed"
                                            );
                                        let delivery = event.name == "messages_changed";
                                        let reset = session.is_some() && event.name == "resync";
                                        if tx.send(LiveEvent::Event(event)).await.is_err() {
                                            return;
                                        }
                                        if reset {
                                            // Do not apply buffered pre-reset frames or fetch
                                            // execution history to rebuild aggregate state.
                                            retry.wait().await;
                                            continue 'connection;
                                        }
                                        if refresh {
                                            let anchor = anchors.borrow().clone();
                                            match client
                                                .catch_up_messages(
                                                    session.as_deref().unwrap(),
                                                    anchor.as_deref(),
                                                    page_limit,
                                                )
                                                .await
                                            {
                                                Ok(page) => {
                                                    let page = if delivery {
                                                        LiveEvent::Messages(page)
                                                    } else {
                                                        LiveEvent::Page(page)
                                                    };
                                                    if tx.send(page).await.is_err()
                                                    {
                                                        return;
                                                    }
                                                }
                                                Err(error) => {
                                                    failure = Some(error);
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                    Err(error) => {
                                        failure = Some(error);
                                        break;
                                    }
                                }
                            }
                        }
                        failure
                    }
                    Err(error) => Some(error),
                };
                let revoked = failure.as_ref().is_some_and(|error|
                    (confirmed && error.access_revoked()) || (browser.is_some() && (error.access_revoked() || matches!(error.status(), Some(400 | 404)))));
                let error = failure
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "Node event connection disconnected".into());
                if tx
                    .send(LiveEvent::Disconnected { error, revoked })
                    .await
                    .is_err()
                    || revoked
                {
                    return;
                }
                retry.wait().await;
            }
        });
        LiveFeed {
            rx,
            task,
            applied,
            resync,
        }
    }
}

fn message_id(message: &crate::api::TranscriptMessage) -> Option<String> {
    let crate::api::TranscriptMessage::Message { metadata, .. } = message;
    metadata.id.clone()
}
impl GatewayClient {
    /// Walk existing history cursors back to the last synchronized message. This
    /// also repairs offline gaps against older Gateway versions without a new API.
    pub async fn catch_up_messages(
        &self,
        session: &str,
        anchor: Option<&str>,
        limit: u32,
    ) -> Result<MessagePage, ApiError> {
        let mut page = self.list_messages(session, None, limit).await?;
        if let Some(anchor) = anchor {
            let mut seen = std::collections::HashSet::new();
            while !page
                .items
                .iter()
                .any(|m| message_id(m).as_deref() == Some(anchor))
            {
                let Some(cursor) = page.older_cursor.clone() else {
                    break;
                };
                if !seen.insert(cursor.clone()) {
                    return Err(ApiError::Task(std::io::Error::other(
                        "history cursor did not advance",
                    )));
                }
                let mut older = self.list_messages(session, Some(&cursor), limit).await?;
                if older.source_epoch != page.source_epoch {
                    return Err(ApiError::Task(std::io::Error::other(
                        "message source changed during catch-up",
                    )));
                }
                older.items.append(&mut page.items);
                page = older;
            }
        }
        Ok(page)
    }
}
