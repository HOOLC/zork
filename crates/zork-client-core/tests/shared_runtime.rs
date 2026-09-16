use axum::{
    http::StatusCode,
    response::{sse::Event, IntoResponse, Sse},
    routing::{get, post},
    Json, Router,
};
use futures_util::{stream, StreamExt};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use zork_client_core::{
    api::*,
    delivery::{self, DeliveryPump},
    live::LiveEvent,
    store::{ClientStore, QueuedMessage},
};

async fn server(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    (
        url,
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        }),
    )
}
fn message(id: &str) -> Value {
    json!({"type":"message","role":"assistant","id":id,"content":id})
}

#[tokio::test]
async fn snapshot_precedes_chat_catchup_and_only_explicit_detail_reads_fetch_history() {
    let overview = |input| {
        json!({
            "session_id":"chat","cursor":"10","runtime":{"model":"test-model"},
            "aggregates":{"complete":true,"usage":{"input":input,"output":1,"cached":0,"reported_steps":100,"cache_reported_steps":0,"cache_input":0},"recent":[]}
        })
    };
    let initial = json!({"session_id":"chat","execution":overview(1000)});
    let (events, _) = tokio::sync::broadcast::channel::<Value>(8);
    let publisher = events.clone();
    let gate = Arc::new(tokio::sync::Notify::new());
    let message_gate = gate.clone();
    let reads = Arc::new(AtomicUsize::new(0));
    let history_reads = reads.clone();
    let (url, server) = server(Router::new()
        .route("/v1/im/sessions/chat/events", get(move || {
            let initial = initial.clone();
            let receiver = publisher.subscribe();
            async move {
                Sse::new(stream::once(async move {
                    Ok::<_, std::convert::Infallible>(Event::default().event("snapshot").data(initial.to_string()))
                }).chain(stream::unfold(receiver, |mut receiver| async move {
                    let value = receiver.recv().await.ok()?;
                    Some((Ok::<_, std::convert::Infallible>(Event::default().event("session_updated").data(value.to_string())), receiver))
                })))
            }
        }))
        .route("/v1/im/sessions/chat/messages", get(move || {
            let gate = message_gate.clone();
            async move { gate.notified().await; Json(json!({"items":[],"older_cursor":null})) }
        }))
        .route("/v1/im/sessions/chat/history", get(move || {
            history_reads.fetch_add(1, Ordering::SeqCst);
            async { Json(json!({"items":[
                {"event_id":"1","event":{"kind":"step_started","step_id":"old","started_at_ms":1}},
                {"event_id":"2","event":{"kind":"step_completed","step_id":"old","completed_at_ms":2,"usage":{"input_tokens":999999,"output_tokens":100}}}
            ],"older_cursor":null,"latest_cursor":"2","server_time_ms":3,"has_more":false})) }
        }))
    ).await;
    let device =
        zork_client_core::state::Device::open(Arc::new(GatewayClient::new(url, None)), None, false);
    let chat = device.conversation("chat");
    chat.start();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !chat.snapshot().overview.loaded {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        !chat.snapshot().loaded,
        "the snapshot must be available while chat catch-up is blocked"
    );
    assert_eq!(chat.snapshot().overview.usage().input, 1000);
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    gate.notify_one();
    let details = chat.history();
    let mut reader = details.subscribe();
    details.load(false);
    tokio::time::timeout(Duration::from_secs(3), async {
        while !reader.snapshot().state.loaded {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    assert!(!reader.snapshot().state.entries.is_empty());
    assert_eq!(chat.snapshot().overview.usage().input, 1000);
    events.send(overview(1100)).unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while chat.snapshot().overview.usage().input != 1100 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        reads.load(Ordering::SeqCst),
        1,
        "aggregate updates must not refresh detail history"
    );
    server.abort();
}

#[tokio::test]
async fn device_feed_observes_each_new_connection_route() {
    let connections = Arc::new(AtomicUsize::new(0));
    let count = connections.clone();
    let (url, server) = server(Router::new().route(
        "/v1/im/events",
        get(move || {
            let index = count.fetch_add(1, Ordering::SeqCst);
            async move {
                if index >= 2 {
                    return (
                        StatusCode::FORBIDDEN,
                        Json(json!({"error":"mesh_client_not_granted"})),
                    )
                        .into_response();
                }
                Sse::new(stream::iter(vec![Ok::<_, std::convert::Infallible>(
                    Event::default().event("changed").data("{}"),
                )]))
                .into_response()
            }
        }),
    ))
    .await;
    let client = Arc::new(GatewayClient::new(url, None));
    let mut feed = client.live(None, 100);
    let mut routes = vec![];
    let mut connected = false;
    tokio::time::timeout(Duration::from_secs(8), async {
        while let Some(event) = feed.next().await {
            match event {
                LiveEvent::Connected => connected = true,
                LiveEvent::Route(route) => {
                    assert!(connected, "route escaped its connection lifetime");
                    routes.push(route);
                }
                LiveEvent::Disconnected { .. } => connected = false,
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        routes,
        vec![
            ConnectionRoute {
                scope: ConnectionScope::Local,
                direct: true
            };
            2
        ]
    );
    server.abort();
}

#[tokio::test]
async fn reconnect_subscribes_before_catchup_and_revocation_ends_feed() {
    let connections = Arc::new(AtomicUsize::new(0));
    let reads = Arc::new(AtomicUsize::new(0));
    let count = connections.clone();
    let stream_route = get(move || {
        let index = count.fetch_add(1, Ordering::SeqCst);
        async move {
            if index >= 2 {
                return (
                    StatusCode::FORBIDDEN,
                    Json(json!({"error":"mesh_client_not_granted"})),
                )
                    .into_response();
            }
            Sse::new(stream::iter(vec![Ok::<_, std::convert::Infallible>(
                Event::default()
                    .event("message")
                    .data(message("echo").to_string()),
            )]))
            .into_response()
        }
    });
    let count = connections.clone();
    let read_count = reads.clone();
    let (url, server) = server(Router::new().route("/v1/im/sessions/chat/events", stream_route).route("/v1/im/sessions/chat/messages", get(move || {
        let subscribed = count.load(Ordering::SeqCst);
        let page = read_count.fetch_add(1, Ordering::SeqCst);
        async move {
            assert!(subscribed > page, "read raced ahead of subscription");
            Json(json!({"items":[message(if page == 0 { "first" } else { "missed-while-offline" })],"older_cursor":null}))
        }
    }))).await;
    let client = Arc::new(GatewayClient::new(url, None));
    let mut feed = client.live(Some("chat".into()), 100);
    let mut pages = vec![];
    let mut revoked = false;
    tokio::time::timeout(Duration::from_secs(8), async {
        while let Some(event) = feed.next().await {
            match event {
                LiveEvent::Page(page) => pages.push(page),
                LiveEvent::Disconnected { revoked: true, .. } => revoked = true,
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    assert!(revoked);
    assert_eq!(connections.load(Ordering::SeqCst), 3);
    assert_eq!(pages.len(), 2);
    assert!(serde_json::to_string(&pages[1])
        .unwrap()
        .contains("missed-while-offline"));
    drop(client); // dropping the owned reactor inside Tokio must be safe
    server.abort();
}

#[tokio::test]
async fn feed_distinguishes_live_imports_from_history_recovery() {
    let (url, server) = server(
        Router::new()
            .route(
                "/v1/im/sessions/chat/events",
                get(|| async {
                    Sse::new(
                        stream::iter(["messages_changed", "resync"].map(|name| {
                            Ok::<_, std::convert::Infallible>(
                                Event::default().event(name).data("{}"),
                            )
                        }))
                        .chain(stream::pending()),
                    )
                }),
            )
            .route(
                "/v1/im/sessions/chat/messages",
                get(|| async { Json(json!({"items":[message("same-id")],"older_cursor":null})) }),
            ),
    )
    .await;
    let client = Arc::new(GatewayClient::new(url, None));
    let mut feed = client.live(Some("chat".into()), 100);
    let mut sources = vec![];
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(event) = feed.next().await {
            match event {
                LiveEvent::Page(_) => sources.push("history"),
                LiveEvent::Messages(_) => sources.push("delivery"),
                _ => {}
            }
            if sources.len() == 3 {
                break;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(sources, ["history", "delivery", "history"]);
    server.abort();
}

#[tokio::test]
async fn shared_delivery_attempts_each_send_once_and_manual_resend_has_a_new_identity() {
    let attempts = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let recorded = attempts.clone();
    let (url, server) = server(Router::new().route(
        "/v1/im/sessions/chat/messages",
        post(move |Json(body): Json<Value>| {
            let mut ids = recorded.lock().unwrap();
            ids.push(body["request_id"].as_str().unwrap().into());
            let attempt = ids.len();
            async move {
                if attempt == 1 {
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({"error":"retry"})),
                    )
                } else {
                    (StatusCode::OK, Json(json!({})))
                }
            }
        }),
    ))
    .await;
    let client = Arc::new(GatewayClient::new(url, None));
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(ClientStore::open(root.path()).unwrap());
    let queued = QueuedMessage {
        request_id: "stable-id".into(),
        session_id: "chat".into(),
        content: "hello".into(),
        attempted: false,
        sent_at_ms: zork_client_core::store::delivery_now_ms(),
        ..Default::default()
    };
    store.enqueue_and_clear_draft("node", &queued).unwrap();
    let first = delivery::flush(&client, &store, "node").await;
    assert!(first.error.is_some());
    assert_eq!(store.outbox("node").unwrap()[0].delivery_status(), "failed");
    let pump = DeliveryPump::start(client.clone(), store.clone(), "node".into());
    pump.set_connected(true);
    tokio::time::sleep(Duration::from_millis(1200)).await;
    pump.set_connected(false);
    pump.set_connected(true);
    assert_eq!(*attempts.lock().unwrap(), ["stable-id"]);
    assert!(delivery::flush(&client, &store, "node")
        .await
        .delivered
        .is_empty());
    store.retry_delivery("node", "stable-id").unwrap();
    let pending = store.outbox("node").unwrap();
    let resend = pending
        .iter()
        .find(|m| m.request_id != "stable-id")
        .unwrap()
        .request_id
        .clone();
    tokio::time::timeout(Duration::from_secs(5), async {
        while attempts.lock().unwrap().len() != 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        *attempts.lock().unwrap(),
        ["stable-id".to_owned(), resend.clone()]
    );
    assert_eq!(
        store.outbox("node").unwrap().len(),
        2,
        "HTTP success cannot confirm a message"
    );
    let echo = |id: &str| serde_json::from_value(message(&format!("client-chat-{id}"))).unwrap();
    store
        .cache_message_page(
            "node",
            "chat",
            &MessagePage {
                source_epoch: None,
                items: vec![echo("stable-id")],
                older_cursor: None,
            },
            None,
        )
        .unwrap();
    assert_eq!(
        store.outbox("node").unwrap()[0].request_id,
        resend,
        "old echo cannot confirm the new send"
    );
    store
        .cache_message_page(
            "node",
            "chat",
            &MessagePage {
                source_epoch: None,
                items: vec![echo("stable-id"), echo(&resend)],
                older_cursor: None,
            },
            None,
        )
        .unwrap();
    assert!(store.outbox("node").unwrap().is_empty());
    assert_eq!(
        store
            .cached_messages("node", "chat", None, 100)
            .unwrap()
            .unwrap()
            .items
            .len(),
        2
    );
    drop(pump);
    server.abort();
}

#[tokio::test]
async fn incompatible_message_endpoint_keeps_the_body_and_explains_the_failure() {
    let (url, server) = server(Router::new().route(
        "/v1/im/sessions/chat/messages",
        post(|| async { StatusCode::UNPROCESSABLE_ENTITY }),
    ))
    .await;
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(ClientStore::open(root.path()).unwrap());
    store
        .enqueue(
            "node",
            &QueuedMessage {
                request_id: "incompatible".into(),
                session_id: "chat".into(),
                content: "原消息正文".into(),
                sent_at_ms: zork_client_core::store::delivery_now_ms(),
                ..Default::default()
            },
        )
        .unwrap();
    let report = delivery::flush(&GatewayClient::new(url, None), &store, "node").await;
    assert_eq!(
        report.error.as_deref(),
        Some("目标设备版本过旧，不支持当前客户端发送消息，请先更新目标设备。")
    );
    let failed = store.outbox("node").unwrap().remove(0);
    assert_eq!(failed.content, "原消息正文");
    assert_eq!(failed.error, report.error);
    server.abort();
}

#[tokio::test]
async fn long_message_manual_resend_keeps_file_bytes_and_creates_a_new_message() {
    use std::collections::HashMap;
    use zork_client_core::{
        files,
        state::{Device, DraftAction},
    };
    #[derive(Default)]
    struct Captured {
        uploads: HashMap<String, Vec<u8>>,
        receipts: HashMap<String, Value>,
        posts: usize,
    }
    let captured = Arc::new(std::sync::Mutex::new(Captured::default()));
    let upload = captured.clone();
    let messages = captured.clone();
    let (url, server) = server(
        Router::new()
            .route(
                "/v1/im/sessions/chat/files",
                post(move |Json(body): Json<Value>| {
                    let state = upload.clone();
                    async move {
                        let file: files::FileRef =
                            serde_json::from_value(body["file"].clone()).unwrap();
                        let bytes: Vec<u8> = serde_json::from_value(body["bytes"].clone()).unwrap();
                        let offset = body["offset"].as_u64().unwrap() as usize;
                        let mut state = state.lock().unwrap();
                        let stored = state.uploads.entry(file.id).or_default();
                        if stored.len() == offset {
                            stored.extend_from_slice(&bytes);
                        } else {
                            assert_eq!(&stored[offset..offset + bytes.len()], bytes.as_slice());
                        }
                        Json(json!({"received":stored.len()}))
                    }
                }),
            )
            .route(
                "/v1/im/sessions/chat/messages",
                post(move |Json(body): Json<Value>| {
                    let state = messages.clone();
                    async move {
                        let mut state = state.lock().unwrap();
                        let (text, files) =
                            files::decode(body["content"].as_str().unwrap()).unwrap();
                        assert!(text.is_empty());
                        assert_eq!(files.len(), 1);
                        for file in files {
                            let bytes = &state.uploads[&file.id];
                            assert_eq!(bytes.len(), file.byte_len);
                            assert_eq!(zork_mesh::content_root(bytes), file.content_root);
                        }
                        let id = body["request_id"].as_str().unwrap().to_owned();
                        if let Some(old) = state.receipts.get(&id) {
                            assert_eq!(old, &body);
                        }
                        state.receipts.insert(id, body);
                        state.posts += 1;
                        if state.posts == 1 {
                            (
                                StatusCode::SERVICE_UNAVAILABLE,
                                Json(json!({"error":"reply lost after commit"})),
                            )
                        } else {
                            (StatusCode::OK, Json(json!({})))
                        }
                    }
                }),
            ),
    )
    .await;
    let client = Arc::new(GatewayClient::new(&url, None));
    let root = tempfile::tempdir().unwrap();
    let text = format!("  {}\n", "完整文本 🐈\n".repeat(4000));
    let queued;
    {
        let store = Arc::new(ClientStore::open(root.path()).unwrap());
        let device = Device::open(client.clone(), Some((store.clone(), "node".into())), false);
        device
            .edit_draft_action("chat", DraftAction::Edit { text: text.clone() })
            .unwrap();
        queued = device.submit_draft("chat", &text).unwrap().unwrap();
        assert!(delivery::flush(&client, &store, "node")
            .await
            .error
            .is_some());
        let state = captured.lock().unwrap();
        assert_eq!(state.uploads.len(), 1);
        assert_eq!(state.uploads.values().next().unwrap(), text.as_bytes());
        assert_eq!(state.receipts.len(), 1);
    }
    let store = ClientStore::open(root.path()).unwrap();
    assert_eq!(store.outbox("node").unwrap()[0].content, queued.content);
    store.retry_delivery("node", &queued.request_id).unwrap();
    let resend = store
        .outbox("node")
        .unwrap()
        .into_iter()
        .find(|m| m.request_id != queued.request_id)
        .unwrap();
    assert_eq!(resend.content, queued.content);
    let result = delivery::flush(&client, &store, "node").await;
    assert!(result.error.is_none(), "{:?}", result.error);
    assert!(result.delivered.is_empty());
    assert_eq!(store.outbox("node").unwrap().len(), 2);
    let state = captured.lock().unwrap();
    assert_eq!(state.posts, 2);
    assert_eq!(state.uploads.len(), 1);
    assert_eq!(state.receipts.len(), 2);
    assert!(state.receipts.contains_key(&queued.request_id));
    assert!(state.receipts.contains_key(&resend.request_id));
    drop(state);
    let echo = |id: &str| {
        serde_json::from_value(json!({"type":"message","role":"user",
        "id":format!("client-chat-{id}"),"content":queued.content}))
        .unwrap()
    };
    store
        .cache_message_page(
            "node",
            "chat",
            &MessagePage {
                source_epoch: None,
                items: vec![echo(&queued.request_id), echo(&resend.request_id)],
                older_cursor: None,
            },
            None,
        )
        .unwrap();
    assert!(store.outbox("node").unwrap().is_empty());
    assert_eq!(
        store
            .cached_messages("node", "chat", None, 100)
            .unwrap()
            .unwrap()
            .items
            .len(),
        2
    );
    server.abort();
}

#[tokio::test]
async fn catchup_crosses_multiple_pages_and_merge_preserves_loaded_history() {
    use axum::extract::Query;
    let (url, server) = server(Router::new().route("/v1/im/sessions/chat/messages", get(|Query(query): Query<std::collections::HashMap<String,String>>| async move {
        let end = query.get("before").and_then(|v| v.parse::<usize>().ok()).unwrap_or(351);
        let start = end.saturating_sub(100).max(1);
        Json(json!({"items":(start..end).map(|id| message(&id.to_string())).collect::<Vec<_>>(),"older_cursor":if start > 1 { Some(start.to_string()) } else { None }}))
    }))).await;
    let client = GatewayClient::new(url, None);
    let page = client
        .catch_up_messages("chat", Some("90"), 100)
        .await
        .unwrap();
    assert_eq!(page.items.len(), 300);
    let earlier: Vec<TranscriptMessage> = (1..=90)
        .map(|id| serde_json::from_value(message(&id.to_string())).unwrap())
        .collect();
    let mut lines = zork_client_core::transcript::project_page_with_pending(&earlier, &mut vec![]);
    zork_client_core::transcript::merge_history_page(&mut lines, &page.items);
    zork_client_core::transcript::merge_history_page(&mut lines, &page.items);
    assert_eq!(lines.len(), 350);
    for (i, line) in lines.iter().enumerate() {
        let zork_client_core::transcript::TranscriptLine::Message { metadata, .. } = line;
        assert_eq!(metadata.id.as_deref(), Some((i + 1).to_string().as_str()));
    }
    server.abort();
}

#[tokio::test]
async fn sending_is_visible_immediately_and_abort_becomes_manual_failure() {
    let (url, server) = server(Router::new().route(
        "/v1/im/sessions/chat/messages",
        post(|| async {
            tokio::time::sleep(Duration::from_secs(20)).await;
            Json(json!({}))
        }),
    ))
    .await;
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(ClientStore::open(root.path()).unwrap());
    store
        .enqueue(
            "node",
            &QueuedMessage {
                request_id: "slow".into(),
                session_id: "chat".into(),
                content: "hello".into(),
                sent_at_ms: zork_client_core::store::delivery_now_ms(),
                ..Default::default()
            },
        )
        .unwrap();
    let client = Arc::new(GatewayClient::new(url, None));
    let pump = DeliveryPump::start(client, store.clone(), "node".into());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        store.outbox("node").unwrap()[0].delivery_status(),
        "sending"
    );
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(
        store.outbox("node").unwrap()[0].delivery_status(),
        "sending"
    );
    drop(pump);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(store.outbox("node").unwrap()[0].delivery_status(), "failed");
    store.delete_failed("node", "slow").unwrap();
    assert!(store.outbox("node").unwrap().is_empty());
    server.abort();
}

#[tokio::test]
async fn transcript_ack_settles_a_lost_post_response_without_retry() {
    let (url, server) = server(Router::new().route(
        "/v1/im/sessions/chat/messages",
        post(|| async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error":"response lost"})),
            )
        }),
    ))
    .await;
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(ClientStore::open(root.path()).unwrap());
    store
        .enqueue(
            "node",
            &QueuedMessage {
                request_id: "ack".into(),
                session_id: "chat".into(),
                content: "hello".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let client = Arc::new(GatewayClient::new(url, None));
    let tx_store = store.clone();
    let sending = tokio::spawn(async move { delivery::flush(&client, &tx_store, "node").await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let confirmed = serde_json::from_value(message("client-chat-ack")).unwrap();
    store
        .cache_message_page(
            "node",
            "chat",
            &zork_client_core::api::MessagePage {
                source_epoch: None,
                items: vec![confirmed],
                older_cursor: None,
            },
            None,
        )
        .unwrap();
    let report = sending.await.unwrap();
    assert!(report.error.is_none());
    assert_eq!(report.delivered, ["ack"]);
    assert!(store.outbox("node").unwrap().is_empty());
    server.abort();
}

#[tokio::test]
async fn core_receives_confirmation_after_all_chat_views_are_gone() {
    use zork_client_core::state::Device;
    let (events, _) = tokio::sync::broadcast::channel::<Value>(8);
    let listeners = Arc::new(AtomicUsize::new(0));
    let posts = Arc::new(AtomicUsize::new(0));
    let publish = events.clone();
    let connections = listeners.clone();
    let sends = posts.clone();
    let (url, server) = server(
        Router::new()
            .route(
                "/v1/im/sessions/chat/events",
                get(move || {
                    let receiver = publish.subscribe();
                    connections.fetch_add(1, Ordering::SeqCst);
                    async move {
                        Sse::new(
                            stream::once(async {
                                Ok::<_, std::convert::Infallible>(
                                    Event::default().event("snapshot").data(
                                        json!({"session_id":"chat","execution":null}).to_string(),
                                    ),
                                )
                            })
                            .chain(stream::unfold(
                                receiver,
                                |mut receiver| async move {
                                    let value = receiver.recv().await.ok()?;
                                    Some((
                                        Ok::<_, std::convert::Infallible>(
                                            Event::default()
                                                .event("message")
                                                .data(value.to_string()),
                                        ),
                                        receiver,
                                    ))
                                },
                            )),
                        )
                    }
                }),
            )
            .route(
                "/v1/im/sessions/chat/messages",
                get(|| async { Json(json!({"items":[],"older_cursor":null})) }).post(move || {
                    sends.fetch_add(1, Ordering::SeqCst);
                    async { Json(json!({"accepted":true})) }
                }),
            ),
    )
    .await;
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(ClientStore::open(root.path()).unwrap());
    let device = Device::open(
        Arc::new(GatewayClient::new(url, None)),
        Some((store.clone(), "node".into())),
        false,
    );
    let queued = device
        .enqueue("chat", "saved locally first".into())
        .unwrap();
    assert_eq!(
        store.outbox("node").unwrap()[0].delivery_status(),
        "sending"
    );
    device.start_delivery();
    tokio::time::timeout(Duration::from_secs(5), async {
        while listeners.load(Ordering::SeqCst) == 0 || posts.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    // Evict the recent-view cache. Only pending delivery's core receive interest
    // retains this Chat; no UI handle or UI subscription ever existed.
    for i in 0..32 {
        device.conversation(&format!("other-{i}"));
    }
    assert_eq!(
        store.outbox("node").unwrap().len(),
        1,
        "HTTP success is not source confirmation"
    );
    let id = format!("client-chat-{}", queued.request_id);
    events
        .send(json!({"type":"message","id":id,"role":"user","content":"authoritative source body"}))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !store.outbox("node").unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let history = store
        .cached_messages("node", "chat", None, 100)
        .unwrap()
        .unwrap();
    assert_eq!(history.items.len(), 1);
    let TranscriptMessage::Message {
        content, metadata, ..
    } = &history.items[0];
    assert_eq!(metadata.id.as_deref(), Some(id.as_str()));
    assert_eq!(content, "authoritative source body");
    assert_eq!(posts.load(Ordering::SeqCst), 1);
    assert_eq!(listeners.load(Ordering::SeqCst), 1);
    drop(device);
    server.abort();
}
