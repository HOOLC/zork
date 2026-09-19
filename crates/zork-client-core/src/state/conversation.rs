use super::{Device, MessageActivity, MessageArrivals};
use zork_observe::{BatchId, Changes, Cursor, JournalLimits, ListEdit, Readiness, Source, Topics};
mod agent_configuration;
mod data;
mod login;
#[cfg(test)]
mod pagination_tests;
use crate::{
    api::{
        AgentStatus, MessagePage, ParticipantStatus, SessionSummary, SseEvent, StationClient,
        TranscriptMessage,
    },
    conversation::{apply_status, decode_sse_event, DecodedSseEvent},
    live::LiveEvent,
    transcript::{transcript_line_from, Transcript, TranscriptLine},
};
use data::Owned;
pub use data::TranscriptLookup;
use futures_util::StreamExt;
use std::{
    ops::Range,
    sync::{Arc, Mutex, Weak},
};

// Retain recently visited conversations independently of their views. The
// budget includes their live feeds; evicting an unobserved conversation drops
// its IO, while delivered history remains in SQLite. External handles stay valid.
const RECENT_CONVERSATIONS: usize = 8;
const RECENT_MESSAGE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConversationData {
    pub overview: Arc<super::SessionOverview>,
    pub lines: Transcript,
    pub lookup: TranscriptLookup,
    /// Absence means the row came from an authoritative transcript. Removing
    /// an outbox receipt alone never changes a pending row into a delivery.
    pub deliveries: MessageDeliveries,
    pub message_activity: MessageActivity,
    pub activity: Option<AgentStatus>,
    pub participants: Arc<Vec<ParticipantStatus>>,
    pub older_cursor: Option<String>,
    pub loading_older: bool,
    pub loaded: bool,
    pub loading: bool,
    pub error: Option<String>,
    pub stop_pending: bool,
    pub canceling: bool,
    pub connected: bool,
    pub revoked: bool,
    pub history_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct DeliveryState {
    pub request_id: String,
    pub attempted: bool,
    pub status: String,
    pub error: Option<String>,
}
pub type MessageDeliveries = imbl::HashMap<String, DeliveryState>;

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;

    fn message(id: &str, text: &str) -> TranscriptMessage {
        serde_json::from_value(
            serde_json::json!({"type":"message","role":"user","id":id,"content":text}),
        )
        .unwrap()
    }

    fn delivered(id: &str) -> SseEvent {
        SseEvent {
            name: "message".into(),
            data: serde_json::to_string(&message(id, id)).unwrap(),
        }
    }

    #[tokio::test]
    async fn overview_is_independent_of_message_and_history_rows_and_revocation_clears_it() {
        let device = Device::open(
            Arc::new(StationClient::new("http://127.0.0.1:9", None)),
            None,
            true,
        );
        let chat = device.conversation("chat");
        let mut updates = chat.subscribe_topics(ConversationTopics::OVERVIEW);
        updates.snapshot();
        let value = |id: &str, cursor: &str, input: u64| {
            serde_json::json!({
                "session_id":id,"cursor":cursor,"runtime":{"model":"test-model"},
                "aggregates":{"complete":true,"usage":{"input":input,"output":2,"cached":0,"reported_steps":5,"cache_reported_steps":0,"cache_input":0},"recent":[]}
            })
        };
        let initial = SseEvent {
            name: "snapshot".into(),
            data: serde_json::json!({"session_id":"chat","execution":value("chat", "10", 1000)})
                .to_string(),
        };
        chat.apply_event(&initial);
        let update = updates.changed().await.unwrap();
        assert!(update.overview_changed);
        assert_eq!(update.state.overview.usage().input, 1000);
        assert!(update.state.lines.is_empty());
        assert!(
            chat.history.get().is_none(),
            "opening an overview must not create a detail loader"
        );
        chat.apply_event(&SseEvent {
            name: "session_updated".into(),
            data: value("other", "11", 1).to_string(),
        });
        chat.apply_event(&SseEvent {
            name: "session_updated".into(),
            data: value("chat", "11", 1000).to_string(),
        });
        chat.apply_page(&MessagePage {
            source_epoch: None,
            items: vec![message("old", "old")],
            older_cursor: None,
        });
        assert!(
            updates.changed().now_or_never().is_none(),
            "cursor and chat pages are not aggregate changes"
        );
        chat.apply_event(&SseEvent {
            name: "session_updated".into(),
            data: value("chat", "12", 1100).to_string(),
        });
        assert_eq!(
            updates
                .changed()
                .await
                .unwrap()
                .state
                .overview
                .usage()
                .input,
            1100
        );
        chat.revoke_content();
        assert_eq!(
            updates
                .changed()
                .await
                .unwrap()
                .state
                .overview
                .usage()
                .input,
            0
        );
        chat.apply_event(&initial);
        assert_eq!(chat.snapshot().overview.usage().input, 0);
    }

    #[tokio::test]
    async fn activity_distinguishes_history_from_delivery_without_connection_heuristics() {
        let device = Device::open(
            Arc::new(StationClient::new("http://127.0.0.1:9", None)),
            None,
            true,
        );
        let chat = device.conversation("chat");
        let mut updates = chat.subscribe();
        assert_eq!(updates.snapshot().message_arrivals.count, 0);
        // Even with a connection already published, restoration is silent.
        chat.commit(|s| s.connected = true);
        chat.apply_page(&MessagePage {
            source_epoch: None,
            items: vec![message("cached", "cached"), message("offline", "offline")],
            older_cursor: None,
        });
        assert_eq!(updates.changed().await.unwrap().message_arrivals.count, 0);
        chat.apply_event(&delivered("offline"));
        assert_eq!(updates.snapshot().message_arrivals.count, 0);
        // A delivery is authoritative even when the UI missed a connection update.
        chat.commit(|s| s.connected = false);
        chat.apply_event(&delivered("live"));
        chat.apply_event(&delivered("live"));
        let arrivals = updates.changed().await.unwrap().message_arrivals;
        assert_eq!(arrivals.count, 1);
        assert_eq!(arrivals.ids, ["live"]);
        // Imported-message notifications fetch history but emit only new deliveries.
        chat.apply_message_page(
            &MessagePage {
                source_epoch: None,
                items: vec![
                    message("older", "older"),
                    message("live", "live"),
                    message("imported", "imported"),
                ],
                older_cursor: None,
            },
            true,
        );
        let arrivals = updates.changed().await.unwrap().message_arrivals;
        assert_eq!(arrivals.count, 1);
        assert_eq!(arrivals.ids, ["imported"]);
        chat.apply_event(&delivered("imported"));
        chat.apply_page(&MessagePage {
            source_epoch: None,
            items: vec![message("recovered", "recovered")],
            older_cursor: None,
        });
        assert_eq!(updates.changed().await.unwrap().message_arrivals.count, 0);
    }

    #[tokio::test]
    async fn activity_coalesces_bursts_and_new_subscribers_do_not_replay_them() {
        let device = Device::open(
            Arc::new(StationClient::new("http://127.0.0.1:9", None)),
            None,
            true,
        );
        let chat = device.conversation("chat");
        let other = device.conversation("other");
        let mut updates = chat.subscribe();
        updates.snapshot();
        for index in 0..100 {
            chat.apply_event(&delivered(&format!("live-{index}")));
        }
        other.apply_event(&delivered("other-chat"));
        let mut reopened = chat.subscribe();
        assert_eq!(reopened.snapshot().message_arrivals.count, 0);
        let arrivals = updates.changed().await.unwrap().message_arrivals;
        assert_eq!(arrivals.count, 100);
        assert_eq!(arrivals.ids.len(), 32);
        assert_eq!(arrivals.ids.first().unwrap(), "live-68");
        chat.apply_event(&delivered("after-subscribe"));
        assert_eq!(
            updates.changed().await.unwrap().message_arrivals.ids,
            ["after-subscribe"]
        );
        assert_eq!(
            reopened.changed().await.unwrap().message_arrivals.ids,
            ["after-subscribe"]
        );
        assert!(updates.changed().now_or_never().is_none());
    }

    #[tokio::test]
    async fn slow_subscriber_reconciles_page_echo_and_withdrawal_without_duplicates() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::store::ClientStore::open(directory.path()).unwrap());
        let device = Device::open(
            Arc::new(StationClient::new("http://127.0.0.1:9", None)),
            Some((store.clone(), "node".into())),
            true,
        );
        let conversation = device.conversation("chat");
        assert!(Arc::ptr_eq(&conversation, &device.conversation("chat")));
        let mut updates = conversation.subscribe();
        updates.snapshot();
        device.edit_draft("chat", "first".into()).unwrap();
        let sent = device.enqueue("chat", "first".into()).unwrap();
        let id = format!("client-chat-{}", sent.request_id);
        let page = MessagePage {
            source_epoch: None,
            items: vec![message(&id, "first")],
            older_cursor: None,
        };
        conversation.apply_page(&page);
        conversation.apply_page(&page);
        device.reload_outbox();
        assert!(device.outbox().items.is_empty());
        let update = updates.changed().await.unwrap();
        let splices = update.messages.unwrap();
        assert_eq!(splices.len(), 1);
        assert_eq!(splices[0].remove, 0..0);
        assert_eq!(splices[0].insert.len(), 1);
        assert_eq!(update.state.lines.len(), 1);
        assert_eq!(update.message_arrivals.count, 1);
        assert_eq!(update.message_arrivals.ids, [id]);
        assert_eq!(
            conversation.subscribe().snapshot().message_arrivals.count,
            0
        );
        conversation.apply_page(&page);
        assert!(
            updates.changed().now_or_never().is_none(),
            "duplicate page caused publication"
        );
        let failed = device.enqueue("chat", "second".into()).unwrap();
        store
            .fail_delivery("node", &failed.request_id, "offline")
            .unwrap();
        device.reload_outbox();
        let failed_id = format!("client-chat-{}", failed.request_id);
        let failed_delivery = conversation
            .snapshot()
            .deliveries
            .get(&failed_id)
            .cloned()
            .unwrap();
        assert_eq!(failed_delivery.error.as_deref(), Some("offline"));
        device.withdraw_delivery(&failed.request_id).unwrap();
        assert_eq!(device.draft("chat").text, "second");
        assert_eq!(conversation.snapshot().lines.len(), 1);
        drop(conversation);
        drop(device);
        assert!(updates.changed().await.is_some());
        assert!(
            updates.changed().await.is_none(),
            "subscription retained its producer"
        );
    }

    #[tokio::test]
    async fn switching_reuses_recent_chats_and_evicted_history_pages_offline() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::store::ClientStore::open(directory.path()).unwrap());
        let device = Device::open(
            Arc::new(StationClient::new("http://127.0.0.1:9", None)),
            Some((store.clone(), "node".into())),
            true,
        );
        let conversation = device.conversation("a");
        conversation.apply_page(&MessagePage {
            source_epoch: None,
            items: (0..250)
                .map(|i| message(&format!("a-{i}"), "cached"))
                .collect(),
            older_cursor: None,
        });
        let pending = device.enqueue("a", "unsent".into()).unwrap();
        assert_eq!(
            store
                .cached_messages("node", "a", None, 300)
                .unwrap()
                .unwrap()
                .items
                .len(),
            250
        );
        let original = Arc::downgrade(&conversation);
        drop(conversation);
        let b = device.conversation("b");
        b.apply_page(&MessagePage {
            source_epoch: None,
            items: vec![message("b-1", "other chat")],
            older_cursor: None,
        });
        assert!(Arc::ptr_eq(
            &original.upgrade().unwrap(),
            &device.conversation("a")
        ));
        for i in 0..RECENT_CONVERSATIONS {
            device.conversation(&format!("c-{i}"));
        }
        assert!(
            original.upgrade().is_none(),
            "unobserved LRU entry was retained forever"
        );
        let restored = device.conversation("a");
        assert_eq!(
            restored.snapshot().lines.len(),
            101,
            "reopen reads one page plus its outbox"
        );
        assert_eq!(
            restored.confirmed_anchor.lock().unwrap().as_deref(),
            Some("a-249")
        );
        // Network refresh must preserve access to the locally cached older pages.
        restored.apply_page(&MessagePage {
            source_epoch: None,
            items: (150..250)
                .map(|i| message(&format!("a-{i}"), "cached"))
                .collect(),
            older_cursor: Some("150".into()),
        });
        let mut updates = restored.subscribe();
        while restored.snapshot().older_cursor.is_some() {
            restored.load_older();
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                while restored.snapshot().loading_older {
                    updates.changed().await.unwrap();
                }
            })
            .await
            .unwrap();
            assert!(
                restored.snapshot().error.is_none(),
                "cached history attempted network IO"
            );
        }
        assert_eq!(restored.snapshot().lines.len(), 251);
        store
            .fail_delivery("node", &pending.request_id, "offline")
            .unwrap();
        device.withdraw_delivery(&pending.request_id).unwrap();
        assert_eq!(restored.snapshot().lines.len(), 250);
        assert_eq!(device.draft("a").text, "unsent");
    }

    #[tokio::test]
    async fn oversized_conversation_is_not_retained_after_its_view_closes() {
        let device = Device::open(
            Arc::new(StationClient::new("http://127.0.0.1:9", None)),
            None,
            true,
        );
        let conversation = device.conversation("large");
        conversation.apply_page(&MessagePage {
            source_epoch: None,
            items: vec![message("large", &"x".repeat(RECENT_MESSAGE_BYTES))],
            older_cursor: None,
        });
        let weak = Arc::downgrade(&conversation);
        assert_eq!(
            conversation.snapshot().lines.len(),
            1,
            "active view must keep its data"
        );
        drop(conversation);
        assert!(weak.upgrade().is_none(), "cache exceeded its byte budget");
    }

    #[tokio::test]
    async fn background_chat_keeps_receiving_without_a_view_or_reconnecting() {
        use axum::{
            extract::Path,
            response::sse::{Event, Sse},
            routing::get,
            Json, Router,
        };
        use futures_util::{stream, StreamExt};
        use std::{
            convert::Infallible,
            sync::atomic::{AtomicUsize, Ordering},
            time::Duration,
        };
        let (events, _) = tokio::sync::broadcast::channel::<(String, String)>(16);
        let connections = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route(
                "/v1/im/sessions/{id}/events",
                get({
                    let events = events.clone();
                    let connections = connections.clone();
                    move |Path(id): Path<String>| {
                        let receiver = events.subscribe();
                        connections.fetch_add(1, Ordering::Relaxed);
                        async move {
                            let initial = Event::default().event("snapshot").data(
                                serde_json::json!({"session_id":id,"execution":null,"status":null,"participants":[]}).to_string()
                            );
                            Sse::new(stream::once(async move { Ok::<_, Infallible>(initial) }).chain(stream::unfold(
                                (receiver, id),
                                |(mut receiver, id)| async move {
                                    loop {
                                        let (session, data) = receiver.recv().await.ok()?;
                                        if session == id {
                                            return Some((
                                                Ok::<_, Infallible>(
                                                    Event::default().event("message").data(data),
                                                ),
                                                (receiver, id),
                                            ));
                                        }
                                    }
                                },
                            )))
                        }
                    }
                }),
            )
            .route(
                "/v1/im/sessions/{id}/messages",
                get(|Path(id): Path<String>| async move {
                    Json(MessagePage { source_epoch: None,
                        items: vec![message(&format!("{id}-initial"), "initial")],
                        older_cursor: None,
                    })
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::store::ClientStore::open(directory.path()).unwrap());
        let device = Device::open(
            Arc::new(StationClient::new(url, None)),
            Some((store.clone(), "node".into())),
            true,
        );
        for id in ["a", "b"] {
            let conversation = device.conversation(id);
            let mut updates = conversation.subscribe();
            conversation.start();
            tokio::time::timeout(Duration::from_secs(3), async {
                while !conversation.snapshot().loaded {
                    updates.changed().await.unwrap();
                }
            })
            .await
            .unwrap();
        }
        // Both strong view handles and subscriptions have now been dropped.
        events
            .send((
                "a".into(),
                serde_json::to_string(&message("a-background", "arrived while viewing b")).unwrap(),
            ))
            .unwrap();
        let a = device.conversation("a");
        let mut updates = a.subscribe();
        a.start();
        tokio::time::timeout(Duration::from_secs(3), async {
            while a.snapshot().lines.len() < 2 {
                updates.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        assert_eq!(connections.load(Ordering::Relaxed), 2);
        assert_eq!(
            store
                .cached_messages("node", "a", None, 100)
                .unwrap()
                .unwrap()
                .items
                .len(),
            2
        );
        assert_eq!(
            store
                .cached_messages("node", "b", None, 100)
                .unwrap()
                .unwrap()
                .items
                .len(),
            1
        );
        server.abort();
    }

    #[tokio::test]
    async fn failed_local_commit_reconnects_from_the_applied_source_anchor() {
        use axum::{
            extract::Query,
            response::sse::{Event, Sse},
            routing::get,
            Json, Router,
        };
        use futures_util::{stream, StreamExt};
        use std::{
            collections::HashMap,
            sync::atomic::{AtomicUsize, Ordering},
            time::Duration,
        };
        let old_reads = Arc::new(AtomicUsize::new(0));
        let reads = old_reads.clone();
        let app = Router::new()
            .route(
                "/v1/im/sessions/chat/events",
                get(|| async {
                    Sse::new(
                        stream::once(async {
                            Ok::<_, std::convert::Infallible>(
                                Event::default()
                                    .event("snapshot")
                                    .data("{\"session_id\":\"chat\",\"execution\":null}"),
                            )
                        })
                        .chain(stream::pending()),
                    )
                }),
            )
            .route(
                "/v1/im/sessions/chat/messages",
                get(move |Query(query): Query<HashMap<String, String>>| {
                    let reads = reads.clone();
                    async move {
                        let older = query.get("before").is_some();
                        if older {
                            reads.fetch_add(1, Ordering::SeqCst);
                        }
                        Json(MessagePage {
                            source_epoch: Some("source".into()),
                            items: if older {
                                vec![message("known", "known"), message("missed", "missed")]
                            } else {
                                vec![message("new-1", "new 1"), message("new-2", "new 2")]
                            },
                            older_cursor: (!older).then(|| "older".into()),
                        })
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::store::ClientStore::open(root.path()).unwrap());
        store
            .cache_message_page(
                "node",
                "chat",
                &MessagePage {
                    source_epoch: Some("source".into()),
                    items: vec![message("known", "known")],
                    older_cursor: None,
                },
                None,
            )
            .unwrap();
        let fault = rusqlite::Connection::open(root.path().join("client.db")).unwrap();
        fault.execute_batch("CREATE TRIGGER fail_message_commit BEFORE INSERT ON messages WHEN NEW.id='new-1' BEGIN SELECT RAISE(ABORT,'fixture persistence failure'); END;").unwrap();
        let device = Device::open(
            Arc::new(StationClient::new(url, None)),
            Some((store.clone(), "node".into())),
            false,
        );
        let conversation = device.conversation("chat");
        conversation.start();
        tokio::time::timeout(Duration::from_secs(5), async {
            while conversation.snapshot().error.is_none() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            store
                .cached_messages("node", "chat", None, 100)
                .unwrap()
                .unwrap()
                .items
                .len(),
            1
        );
        assert_eq!(
            conversation.confirmed_anchor.lock().unwrap().as_deref(),
            Some("known")
        );
        fault
            .execute_batch("DROP TRIGGER fail_message_commit;")
            .unwrap();
        tokio::time::timeout(Duration::from_secs(8), async {
            while conversation.snapshot().lines.len() != 4 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let page = store
            .cached_messages("node", "chat", None, 100)
            .unwrap()
            .unwrap();
        let ids = page
            .items
            .iter()
            .map(|item| {
                let TranscriptMessage::Message { metadata, .. } = item;
                metadata.id.as_deref().unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(ids, ["known", "missed", "new-1", "new-2"]);
        assert!(
            old_reads.load(Ordering::SeqCst) >= 2,
            "transport advanced past a rejected local commit"
        );
        server.abort();
    }
}
#[derive(Clone, Debug)]
pub struct MessageSplice {
    pub remove: Range<usize>,
    pub insert: usize,
}

/// Business interests, independent of the platform's component tree.
pub struct ConversationTopics;
impl ConversationTopics {
    pub const MESSAGES: Topics = Topics::new(1);
    pub const ACTIVITY: Topics = Topics::new(2);
    pub const PARTICIPANTS: Topics = Topics::new(4);
    pub const LOADING: Topics = Topics::new(8);
    pub const HISTORY: Topics = Topics::new(16);
    pub const ARRIVALS: Topics = Topics::new(32);
    pub const OVERVIEW: Topics = Topics::new(64);
    pub const ALL: Topics = Topics::new(127);
}

struct ConversationChange {
    messages: Vec<ListEdit<TranscriptLine>>,
}

pub struct ConversationUpdate {
    pub state: Arc<ConversationData>,
    pub messages: Option<Vec<ListEdit<TranscriptLine>>>,
    pub message_arrivals: MessageArrivals,
    pub activity_changed: bool,
    pub participants_changed: bool,
    pub loading_changed: bool,
    pub history_changed: bool,
    pub overview_changed: bool,
    pub cursor: Cursor,
    pub batch: Option<BatchId>,
    pub reset: bool,
}
pub struct ConversationSubscription {
    source: zork_observe::Subscription<ConversationData, ConversationChange>,
    applied_len: usize,
    activity_cursor: u64,
    prepared: Option<(BatchId, usize, u64)>,
}
impl ConversationSubscription {
    pub fn valid(&self, batch: BatchId) -> bool {
        self.source.valid(batch)
    }
    pub fn reset(&mut self) {
        self.prepared = None;
        self.source.reset();
    }
    pub fn readiness(&self) -> Readiness {
        self.source.readiness()
    }
    pub async fn ready(&mut self) -> Result<(), zork_observe::Closed> {
        self.source.ready().await
    }

    pub fn prepare(&mut self) -> Option<ConversationUpdate> {
        let batch = self.source.prepare()?;
        let state = batch.snapshot.value.clone();
        let topics = batch.topics;
        let messages = if topics.intersects(ConversationTopics::MESSAGES) {
            let mut messages = Vec::new();
            match &batch.changes {
                Changes::Reset(_) => messages.push(ListEdit {
                    remove: 0..self.applied_len,
                    insert: state.lines.clone(),
                }),
                Changes::Delta { records, .. } => {
                    for record in records {
                        for edit in &record.value.messages {
                            ListEdit::push_coalesced(&mut messages, edit.clone());
                        }
                    }
                }
            }
            (!messages.is_empty()).then_some(messages)
        } else {
            None
        };
        self.prepared = Some((batch.id, state.lines.len(), state.message_activity.sequence));
        Some(ConversationUpdate {
            messages,
            message_arrivals: if state.revoked || !topics.intersects(ConversationTopics::ARRIVALS) {
                MessageArrivals::default()
            } else {
                state.message_activity.since(self.activity_cursor)
            },
            activity_changed: topics.intersects(ConversationTopics::ACTIVITY),
            participants_changed: topics.intersects(ConversationTopics::PARTICIPANTS),
            loading_changed: topics.intersects(ConversationTopics::LOADING),
            history_changed: topics.intersects(ConversationTopics::HISTORY),
            overview_changed: topics.intersects(ConversationTopics::OVERVIEW),
            state,
            cursor: batch.snapshot.cursor,
            batch: Some(batch.id),
            reset: batch.is_reset(),
        })
    }

    pub fn acknowledge(&mut self, batch: BatchId) -> bool {
        let Some((id, length, activity)) = self.prepared else {
            return false;
        };
        if id != batch || !self.source.acknowledge(batch) {
            return false;
        }
        self.applied_len = length;
        self.activity_cursor = activity;
        self.prepared = None;
        true
    }

    pub fn discard(&mut self, batch: BatchId) -> bool {
        if !self.source.discard(batch) {
            return false;
        }
        self.prepared = None;
        true
    }

    /// Synchronous compatibility read for core reducers and fixtures. Platform
    /// scheduling uses prepare/acknowledge so cancelled delivery keeps its base.
    pub fn snapshot(&mut self) -> ConversationUpdate {
        if let Some(update) = self.prepare() {
            self.acknowledge(update.batch.unwrap());
            return update;
        }
        let state = self.source.snapshot();
        ConversationUpdate {
            state: state.value,
            cursor: state.cursor,
            batch: None,
            reset: false,
            messages: None,
            message_arrivals: MessageArrivals::default(),
            activity_changed: false,
            participants_changed: false,
            loading_changed: false,
            history_changed: false,
            overview_changed: false,
        }
    }
    pub async fn changed(&mut self) -> Option<ConversationUpdate> {
        loop {
            self.ready().await.ok()?;
            if let Some(update) = self.prepare() {
                self.acknowledge(update.batch.unwrap());
                return Some(update);
            }
        }
    }
}

pub struct Conversation {
    device: Weak<Device>,
    client: Arc<StationClient>,
    id: String,
    owned: Mutex<Owned>,
    state: Source<ConversationData, ConversationChange>,
    stream: Mutex<Option<tokio::task::JoinHandle<()>>>,
    older: Mutex<Option<tokio::task::JoinHandle<()>>>,
    reload: Mutex<Option<tokio::task::JoinHandle<()>>>,
    cancel: Mutex<Option<tokio::task::JoinHandle<()>>>,
    history: std::sync::OnceLock<Arc<super::History>>,
    login_tasks: Mutex<std::collections::HashSet<String>>,
    confirmed_anchor: Mutex<Option<String>>,
    source_epoch: Mutex<Option<String>>,
    message_bytes: std::sync::atomic::AtomicUsize,
    cache_generation: u64,
}
impl Drop for Conversation {
    fn drop(&mut self) {
        for task in [
            &mut self.stream,
            &mut self.older,
            &mut self.reload,
            &mut self.cancel,
        ] {
            if let Some(task) = task.get_mut().unwrap().take() {
                task.abort();
            }
        }
    }
}
impl Device {
    pub fn conversation(self: &Arc<Self>, id: &str) -> Arc<Conversation> {
        let mut registry = self.conversations.lock().unwrap();
        registry.retain(|_, conversation| conversation.strong_count() > 0);
        let conversation = registry
            .get(id)
            .and_then(Weak::upgrade)
            .filter(|c| !c.snapshot().revoked)
            .unwrap_or_else(|| Conversation::new(self, id));
        registry.insert(id.into(), Arc::downgrade(&conversation));
        let mut recent = self.recent_conversations.lock().unwrap();
        recent.retain(|conversation| conversation.id != id);
        recent.push_back(conversation.clone());
        Self::trim_recent(&mut recent);
        conversation
    }

    fn trim_recent(recent: &mut std::collections::VecDeque<Arc<Conversation>>) {
        let mut bytes = recent
            .iter()
            .map(|c| c.message_bytes.load(std::sync::atomic::Ordering::Relaxed))
            .sum::<usize>();
        while recent.len() > RECENT_CONVERSATIONS || bytes > RECENT_MESSAGE_BYTES {
            let Some(oldest) = recent.pop_front() else {
                break;
            };
            bytes = bytes.saturating_sub(
                oldest
                    .message_bytes
                    .load(std::sync::atomic::Ordering::Relaxed),
            );
        }
    }
}
impl Conversation {
    pub(super) fn revoke_content(&self) {
        if let Some(history) = self.history.get() {
            history.revoke_content();
        }
        for task in [&self.stream, &self.older, &self.reload, &self.cancel] {
            if let Some(task) = task.lock().unwrap().take() {
                task.abort();
            }
        }
        self.commit(|state| {
            state.clear_messages();
            state.participants = Default::default();
            state.activity = None;
            state.overview = Arc::new(super::SessionOverview::unavailable());
            state.connected = false;
            state.revoked = true;
            state.loaded = false;
            state.loading = false;
            state.error = Some("设备访问权限已撤销".into());
        });
        *self.confirmed_anchor.lock().unwrap() = None;
    }
    #[cfg(any(test, feature = "headless-bench"))]
    pub fn seed(&self, state: ConversationData) {
        self.commit(|current| current.replace(state));
    }
    #[cfg(any(test, feature = "headless-bench"))]
    pub fn seed_event(&self, event: &SseEvent) {
        self.apply_event(event);
    }
    #[cfg(feature = "headless-bench")]
    pub fn subscription_journal_retained(&self) -> (usize, usize) {
        self.state.retained()
    }
    fn new(device: &Arc<Device>, id: &str) -> Arc<Self> {
        let generation = device
            .cache
            .as_ref()
            .map(|(store, node)| store.replica_generation(node))
            .transpose();
        let cache_generation = generation.as_ref().ok().copied().flatten().unwrap_or(0);
        let page = generation.and_then(|_| {
            device
                .cache
                .as_ref()
                .map(|(store, node)| {
                    store.cached_messages_at(node, id, None, 100, cache_generation)
                })
                .transpose()
        });
        let mut data = ConversationData::default();
        let page = match page {
            Ok(page) => page.flatten(),
            Err(error) => {
                data.error = Some(error.to_string());
                None
            }
        };
        let source_epoch = page.as_ref().and_then(|page| page.source_epoch.clone());
        let confirmed_anchor =
            page.as_ref()
                .and_then(|page| page.items.last())
                .and_then(|message| {
                    let TranscriptMessage::Message { metadata, .. } = message;
                    metadata.id.clone()
                });
        let source_head = page
            .as_ref()
            .and_then(|page| page.items.first())
            .and_then(|message| {
                let TranscriptMessage::Message { metadata, .. } = message;
                metadata.id.clone()
            });
        if let Some(page) = page {
            data.lines = page.items.iter().filter_map(transcript_line_from).collect();
            data.older_cursor = page.older_cursor;
            data.loaded = true;
        }
        data.stop_pending = device
            .cache
            .as_ref()
            .and_then(|(store, node)| {
                store
                    .get(node, &format!("stop-pending:{id}"))
                    .ok()
                    .flatten()
            })
            .unwrap_or(false);
        let mut owned = Owned::new(data);
        owned.cache_backed = device.cache.is_some();
        owned.source_head = source_head;
        let data = owned.data.clone();
        let message_bytes = owned.bytes;
        let conversation = Arc::new(Self {
            device: Arc::downgrade(device),
            client: device.client.clone(),
            id: id.into(),
            owned: Mutex::new(owned),
            message_bytes: std::sync::atomic::AtomicUsize::new(message_bytes),
            state: Source::new(data, JournalLimits::default()),
            stream: Mutex::new(None),
            older: Mutex::new(None),
            reload: Mutex::new(None),
            cancel: Mutex::new(None),
            history: std::sync::OnceLock::new(),
            login_tasks: Mutex::new(Default::default()),
            confirmed_anchor: Mutex::new(confirmed_anchor),
            source_epoch: Mutex::new(source_epoch),
            cache_generation,
        });
        conversation.sync_outbox();
        conversation.sync_interactions();
        conversation
    }
    pub fn history(&self) -> Arc<super::History> {
        self.history
            .get_or_init(|| super::History::new(self.client.clone(), self.id.clone()))
            .clone()
    }

    /// Dispatch a card input to the business that supplied the authoritative card.
    pub fn respond_to_interaction(
        self: &Arc<Self>,
        command: crate::interactions::Command,
    ) -> anyhow::Result<()> {
        let id = command.message_id();
        let device = self
            .device
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("Device unavailable"))?;
        let (store, node) = device
            .cache
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Persistent card operations unavailable"))?;
        let message = store
            .cached_message_at(node, &self.id, id, self.cache_generation)?
            .ok_or_else(|| anyhow::anyhow!("Business card unavailable"))?;
        let TranscriptMessage::Message { metadata, .. } = &message;
        let content = crate::interactions::MessageContent::parse(
            metadata
                .interaction
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("Business card required"))?,
        )
        .ok_or_else(|| anyhow::anyhow!("Unsupported business card"))?;
        anyhow::ensure!(
            crate::interactions::request(metadata).is_some(),
            "Unsupported business card"
        );
        crate::interactions::dispatch(self, &content.handler, command)
    }

    pub(super) fn sync_interactions(&self) {
        let Some(device) = self.device.upgrade() else {
            return;
        };
        let Some((store, node)) = &device.cache else {
            return;
        };
        let updates: anyhow::Result<_> = (|| {
            let submissions =
                store.configuration_submissions(node, &self.id, self.cache_generation)?;
            let mut ids: std::collections::HashSet<String> = self
                .owned
                .lock()
                .unwrap()
                .interaction_submissions
                .keys()
                .cloned()
                .collect();
            ids.extend(submissions.keys().cloned());
            let mut records = Vec::new();
            for id in ids {
                if let Some(record) =
                    store.cached_message_at(node, &self.id, &id, self.cache_generation)?
                {
                    records.push(record);
                }
            }
            Ok((submissions, records))
        })();
        match updates {
            Ok((submissions, records)) => self.commit(|s| {
                s.set_interaction_submissions(submissions);
                s.update_cached_interactions(&records);
            }),
            Err(error) => self.commit(|s| s.error = Some(error.to_string())),
        }
    }
    pub fn snapshot(&self) -> Arc<ConversationData> {
        self.state.snapshot().value
    }
    pub fn subscribe(&self) -> ConversationSubscription {
        self.subscribe_topics(ConversationTopics::ALL)
    }
    pub fn subscribe_topics(&self, topics: Topics) -> ConversationSubscription {
        let (source, opening) = self.state.subscribe_topics(topics);
        ConversationSubscription {
            source,
            applied_len: 0,
            prepared: None,
            activity_cursor: opening.value.message_activity.sequence,
        }
    }
    fn commit(&self, change: impl FnOnce(&mut Owned)) {
        let mut state = self.owned.lock().unwrap();
        if state.revoked {
            return;
        }
        let before = state.data.clone();
        change(&mut state);
        let messages_changed = state.replaced || !state.edits.is_empty();
        let mut topics = if messages_changed {
            ConversationTopics::MESSAGES
        } else {
            Topics::NONE
        };
        if before.activity != state.activity
            || before.stop_pending != state.stop_pending
            || before.canceling != state.canceling
        {
            topics |= ConversationTopics::ACTIVITY;
        }
        if before.participants != state.participants {
            topics |= ConversationTopics::PARTICIPANTS;
        }
        if before.loaded != state.loaded
            || before.loading != state.loading
            || before.loading_older != state.loading_older
            || before.older_cursor != state.older_cursor
            || before.error != state.error
            || before.connected != state.connected
            || before.revoked != state.revoked
        {
            topics |= ConversationTopics::LOADING;
        }
        if before.history_revision != state.history_revision {
            topics |= ConversationTopics::HISTORY;
        }
        if !Arc::ptr_eq(&before.overview, &state.overview) && before.overview != state.overview {
            topics |= ConversationTopics::OVERVIEW;
        }
        if before.message_activity.sequence != state.message_activity.sequence {
            topics |= ConversationTopics::ARRIVALS;
        }
        if topics.is_empty() {
            return;
        }
        if messages_changed {
            self.message_bytes
                .store(state.bytes, std::sync::atomic::Ordering::Relaxed);
        }
        if before.stop_pending != state.stop_pending {
            if let Some(device) = self.device.upgrade() {
                if let Some((store, node)) = &device.cache {
                    let _ = store.put(
                        node,
                        &format!("stop-pending:{}", self.id),
                        &state.stop_pending,
                    );
                }
            }
        }
        let messages = std::mem::take(&mut state.edits);
        if std::mem::take(&mut state.replaced) {
            if state.revoked {
                self.state.invalidate(state.data.clone());
            } else {
                self.state.replace(state.data.clone());
            }
        } else {
            let bytes = messages
                .iter()
                .map(|edit| edit.insert.iter().map(data::row_bytes).sum::<usize>())
                .sum();
            self.state.publish(
                state.data.clone(),
                ConversationChange { messages },
                topics,
                bytes,
            );
        }
        drop(state);
        if messages_changed {
            if let Some(device) = self.device.upgrade() {
                Device::trim_recent(&mut device.recent_conversations.lock().unwrap());
            }
        }
    }
    pub fn start(self: &Arc<Self>) {
        if self.snapshot().revoked {
            return;
        }
        let mut task = self.stream.lock().unwrap();
        if task.as_ref().is_some_and(|task| !task.is_finished()) {
            return;
        }
        self.commit(|s| s.loading = !s.loaded);
        let anchor = self.confirmed_anchor.lock().unwrap().clone();
        let client = self.client.clone();
        let id = self.id.clone();
        let weak = Arc::downgrade(self);
        *task = Some(self.client.spawn(async move {
            let mut feed = client.live_from(Some(id), 100, anchor);
            while let Some(event) = feed.next().await {
                let Some(conversation) = weak.upgrade() else {
                    return;
                };
                let applied = match event {
                    LiveEvent::Connected => {
                        conversation.commit(|s| {
                            s.connected = true;
                            s.revoked = false;
                            s.error = None;
                        });
                        true
                    }
                    LiveEvent::Route(_) => true,
                    LiveEvent::Page(page) => conversation.apply_page(&page),
                    LiveEvent::Messages(page) => conversation.apply_message_page(&page, true),
                    LiveEvent::Event(event) => conversation.apply_event(&event),
                    LiveEvent::Disconnected { error, revoked } => {
                        conversation.commit(|s| {
                            s.connected = false;
                            s.revoked = revoked;
                            s.loading = false;
                            s.error = Some(error);
                        });
                        if revoked {
                            return;
                        }
                        true
                    }
                };
                if applied {
                    feed.acknowledge_messages(
                        conversation.confirmed_anchor.lock().unwrap().clone(),
                    );
                } else {
                    feed.resync_messages();
                }
            }
        }));
    }
    pub fn refresh(self: &Arc<Self>) {
        let mut task = self.reload.lock().unwrap();
        if task.as_ref().is_some_and(|t| !t.is_finished()) {
            return;
        }
        let weak = Arc::downgrade(self);
        let client = self.client.clone();
        let id = self.id.clone();
        let anchor = self.confirmed_anchor.lock().unwrap().clone();
        *task = Some(self.client.spawn(async move {
            let result = client.catch_up_messages(&id, anchor.as_deref(), 100).await;
            if let Some(conversation) = weak.upgrade() {
                match result {
                    Ok(page) => {
                        conversation.apply_page(&page);
                    }
                    Err(e) => conversation.commit(|s| {
                        s.loading = false;
                        s.error = Some(e.to_string());
                    }),
                }
            }
        }));
    }
    pub fn load_older(self: &Arc<Self>) {
        let Some(cursor) = self.snapshot().older_cursor.clone() else {
            return;
        };
        let mut task = self.older.lock().unwrap();
        if task.as_ref().is_some_and(|t| !t.is_finished()) {
            return;
        }
        self.commit(|s| s.loading_older = true);
        let weak = Arc::downgrade(self);
        let client = self.client.clone();
        let id = self.id.clone();
        let cache = self
            .device
            .upgrade()
            .and_then(|device| device.cache.clone());
        let generation = self.cache_generation;
        *task = Some(self.client.spawn(async move {
            let result: anyhow::Result<MessagePage> = async {
                if let Some((store, node)) = &cache {
                    if let Some(page) =
                        store.cached_messages_at(node, &id, Some(&cursor), 100, generation)?
                    {
                        return Ok(page);
                    }
                }
                let page = client.list_messages(&id, Some(&cursor), 100).await?;
                if let Some((store, node)) = &cache {
                    store.cache_message_page_at(node, &id, &page, Some(&cursor), generation)?;
                }
                Ok(page)
            }
            .await;
            if let Some(conversation) = weak.upgrade() {
                match result {
                    Ok(page)
                        if page.source_epoch.as_ref().is_some_and(|epoch| {
                            conversation
                                .source_epoch
                                .lock()
                                .unwrap()
                                .as_ref()
                                .is_some_and(|current| current != epoch)
                        }) =>
                    {
                        conversation.commit(|state| state.loading_older = false);
                    }
                    Ok(page) => match conversation.cached_interaction_updates(&page.items) {
                        Ok(updates) => conversation.commit(|s| {
                            let current_page = s.older_cursor.as_ref() == Some(&cursor);
                            s.prepend(&page.items);
                            s.update_cached_interactions(&updates);
                            s.error = None;
                            conversation.update_older_cursor(s, &page, current_page);
                            s.loading_older = false;
                        }),
                        Err(error) => conversation.commit(|s| {
                            s.error = Some(error.to_string());
                            s.loading_older = false;
                        }),
                    },
                    Err(e) => conversation.commit(|s| {
                        s.loading_older = false;
                        s.error = Some(e.to_string());
                    }),
                }
            }
        }));
    }
    fn apply_page(&self, page: &MessagePage) -> bool {
        self.apply_message_page(page, false)
    }
    fn apply_message_page(&self, page: &MessagePage, delivery: bool) -> bool {
        if let Some(device) = self.device.upgrade() {
            if let Some((store, node)) = &device.cache {
                if let Err(error) =
                    store.cache_message_page_at(node, &self.id, page, None, self.cache_generation)
                {
                    self.commit(|s| {
                        s.error = Some(error.to_string());
                        s.loading = false;
                    });
                    return false;
                }
            }
        }
        if let Some(epoch) = &page.source_epoch {
            let mut current = self.source_epoch.lock().unwrap();
            if current.as_ref().is_some_and(|current| current != epoch) {
                if let Some(older) = self.older.lock().unwrap().take() {
                    older.abort();
                }
                *self.confirmed_anchor.lock().unwrap() = None;
                self.commit(|state| {
                    state.clear_messages();
                    state.loading_older = false;
                });
            }
            *current = Some(epoch.clone());
        }
        let updates = match self.cached_interaction_updates(&page.items) {
            Ok(updates) => updates,
            Err(error) => {
                self.commit(|s| {
                    s.error = Some(error.to_string());
                    s.loading = false;
                });
                return false;
            }
        };
        let arrival_start = if delivery {
            let anchor = self.confirmed_anchor.lock().unwrap();
            anchor
                .as_ref()
                .and_then(|anchor| {
                    page.items.iter().rposition(|message| {
                        let TranscriptMessage::Message { metadata, .. } = message;
                        metadata.id.as_ref() == Some(anchor)
                    })
                })
                .map_or(0, |index| index + 1)
        } else {
            page.items.len()
        };
        if let Some(TranscriptMessage::Message { metadata, .. }) = page.items.last() {
            if let Some(id) = &metadata.id {
                *self.confirmed_anchor.lock().unwrap() = Some(id.clone());
            }
        }
        self.commit(|state| {
            state.merge(&page.items, arrival_start);
            state.update_cached_interactions(&updates);
            state.loaded = true;
            state.loading = false;
            state.error = None;
            self.update_older_cursor(state, page, false);
        });
        self.sync_outbox();
        true
    }
    fn cached_interaction_updates(
        &self,
        items: &[TranscriptMessage],
    ) -> anyhow::Result<Vec<TranscriptMessage>> {
        if let Some(device) = self.device.upgrade() {
            if let Some((store, node)) = &device.cache {
                return store.cached_interaction_updates(
                    node,
                    &self.id,
                    items,
                    self.cache_generation,
                );
            }
        }
        Ok(vec![])
    }
    fn update_older_cursor(&self, state: &mut Owned, page: &MessagePage, older: bool) {
        let candidate = page.items.first().and_then(|message| {
            let TranscriptMessage::Message { metadata, .. } = message;
            metadata.id.as_deref()
        });
        if let Some(device) = self.device.upgrade() {
            if let Some((store, node)) = &device.cache {
                match store.cached_history_boundary(
                    node,
                    &self.id,
                    state.source_head.as_deref(),
                    candidate,
                    self.cache_generation,
                ) {
                    Ok((head, cursor)) => {
                        state.source_head = head;
                        state.older_cursor = cursor;
                    }
                    Err(error) => state.error = Some(error.to_string()),
                }
                return;
            }
        }
        let reaches_head = state.source_head.as_ref().is_none_or(|head| {
            page.items.iter().any(|message| {
                let TranscriptMessage::Message { metadata, .. } = message;
                metadata.id.as_ref() == Some(head)
            })
        });
        if older || reaches_head {
            if let Some(candidate) = candidate {
                state.source_head = Some(candidate.into());
            }
            state.older_cursor = page.older_cursor.clone();
        }
    }
    fn apply_event(&self, event: &SseEvent) -> bool {
        if event.name == "snapshot" {
            if let Ok(snapshot) = serde_json::from_str::<super::InitialSessionSnapshot>(&event.data)
            {
                if snapshot.session_id != self.id
                    || snapshot
                        .execution
                        .as_ref()
                        .is_some_and(|execution| execution.session_id != self.id)
                {
                    return true;
                }
                self.commit(|state| {
                    state.overview = Arc::new(
                        snapshot
                            .execution
                            .map(|execution| execution.overview())
                            .unwrap_or_else(super::SessionOverview::unavailable),
                    );
                    state.participants = Arc::new(snapshot.participants);
                    state.activity = None;
                    if let Some(status) = snapshot.status {
                        let state = &mut state.data;
                        apply_status(
                            &mut state.activity,
                            Arc::make_mut(&mut state.participants).as_mut_slice(),
                            &mut state.stop_pending,
                            Some(&self.id),
                            status,
                        );
                    }
                });
                if let Some(history) = self.history.get() {
                    history.refresh_if_observed();
                }
            }
            return true;
        }
        if event.name == "session_updated" {
            if let Ok(snapshot) = serde_json::from_str::<super::SessionSnapshot>(&event.data) {
                if snapshot.session_id == self.id {
                    self.commit(|state| state.overview = Arc::new(snapshot.overview()));
                }
            }
            return true;
        }
        let Ok(Some(event)) = decode_sse_event(event) else {
            return true;
        };
        match event {
            DecodedSseEvent::Transcript(message) => {
                let TranscriptMessage::Message { metadata, .. } = &message;
                if metadata.source_epoch.as_ref().is_some_and(|epoch| {
                    self.source_epoch
                        .lock()
                        .unwrap()
                        .as_ref()
                        .is_some_and(|current| current != epoch)
                }) {
                    return true;
                }
                if let Some(device) = self.device.upgrade() {
                    if let Some((store, node)) = &device.cache {
                        if let Err(error) = store.cache_delivered_message(
                            node,
                            &self.id,
                            &message,
                            self.cache_generation,
                        ) {
                            self.commit(|s| s.error = Some(error.to_string()));
                            return false;
                        }
                    }
                }
                let TranscriptMessage::Message { metadata, .. } = &message;
                let updates = match self.cached_interaction_updates(std::slice::from_ref(&message))
                {
                    Ok(updates) => updates,
                    Err(error) => {
                        self.commit(|s| s.error = Some(error.to_string()));
                        return false;
                    }
                };
                if let Some(id) = &metadata.id {
                    *self.confirmed_anchor.lock().unwrap() = Some(id.clone());
                }
                self.commit(|s| {
                    if s.source_head.is_none() {
                        s.source_head = metadata.id.clone();
                    }
                    if s.delivered_message(&message) {
                        if let Some(id) = &metadata.id {
                            s.message_activity.record(id.clone());
                        }
                    }
                    s.update_cached_interactions(&updates);
                    s.loaded = true;
                    s.loading = false;
                });
            }
            DecodedSseEvent::Status(status) => self.commit(|s| {
                let s = &mut s.data;
                apply_status(
                    &mut s.activity,
                    Arc::make_mut(&mut s.participants).as_mut_slice(),
                    &mut s.stop_pending,
                    Some(&self.id),
                    status,
                )
            }),
            DecodedSseEvent::Participants(participants) => self.commit(|s| {
                s.activity = participants
                    .iter()
                    .find(|p| p.session_id == self.id)
                    .and_then(|p| p.activity.clone());
                s.participants = Arc::new(participants);
            }),
            DecodedSseEvent::HistoryChanged => {
                self.commit(|s| s.history_revision = s.history_revision.wrapping_add(1));
                if let Some(history) = self.history.get() {
                    history.refresh_if_observed();
                }
            }
            _ => {}
        }
        true
    }
    pub(super) fn confirm_status(&self, sessions: &[SessionSummary], online: bool) {
        let status = sessions
            .iter()
            .find(|s| s.session_id == self.id)
            .map(|s| s.status);
        if crate::conversation::stop_confirmed(online, status) {
            self.commit(|s| s.stop_pending = false);
        }
    }
    pub(crate) fn stopping(&self) {
        self.commit(|s| s.stop_pending = true);
    }
    pub fn stop(self: &Arc<Self>) {
        let mut task = self.cancel.lock().unwrap();
        if task.as_ref().is_some_and(|t| !t.is_finished()) {
            return;
        }
        self.commit(|s| {
            s.canceling = true;
            s.stop_pending = true;
            s.error = None;
        });
        let weak = Arc::downgrade(self);
        let client = self.client.clone();
        let id = self.id.clone();
        *task = Some(self.client.spawn(async move {
            let result = client.cancel_session(&id).await;
            if let Some(conversation) = weak.upgrade() {
                conversation.commit(|s| {
                    s.canceling = false;
                    if let Err(e) = result {
                        if s.stop_pending {
                            s.error = Some(e.to_string());
                        }
                    }
                });
            }
        }));
    }
    pub(super) fn sync_outbox(&self) {
        let Some(device) = self.device.upgrade() else {
            return;
        };
        let queued = device.outbox();
        let mut confirmed = Vec::new();
        if let Some((store, node)) = &device.cache {
            let ids: Vec<_> = self
                .owned
                .lock()
                .unwrap()
                .deliveries
                .keys()
                .cloned()
                .collect();
            for id in ids {
                if !queued.by_message_id.contains_key(&id) {
                    match store.confirmed_message(node, &self.id, &id, self.cache_generation) {
                        Ok(Some(message)) => confirmed.push(message),
                        Ok(None) => {}
                        Err(error) => {
                            self.commit(|state| state.error = Some(error.to_string()));
                            return;
                        }
                    }
                }
            }
        }
        self.commit(|s| {
            for message in confirmed {
                s.delivered_message(&message);
            }
            for message in queued.items.iter().filter(|m| m.session_id == self.id) {
                let id = format!("client-{}-{}", self.id, message.request_id);
                let missing = s.index_of(&id).is_none();
                if missing {
                    s.message_activity.record(id.clone());
                    s.delivered(TranscriptLine::Message {
                        role: crate::api::Role::User,
                        content: message.content.clone(),
                        metadata: crate::api::MessageMetadata {
                            id: Some(id.clone()),
                            ..Default::default()
                        },
                    });
                }
                if missing || s.deliveries.contains_key(&id) {
                    s.set_delivery(
                        &id,
                        Some(DeliveryState {
                            request_id: message.request_id.clone(),
                            attempted: message.attempted,
                            status: queued
                                .phases
                                .get(&message.request_id)
                                .cloned()
                                .unwrap_or_default(),
                            error: message.error.clone(),
                        }),
                    );
                }
            }
        });
    }
    pub(super) fn remove_failed(&self, id: &str) {
        let message = format!("client-{}-{id}", self.id);
        self.commit(|s| s.remove_id(&message));
    }
}
