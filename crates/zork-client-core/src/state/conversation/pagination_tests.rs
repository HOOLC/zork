use super::*;
use axum::{extract::Query, routing::get, Json, Router};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

fn history(with_tail: bool) -> Vec<TranscriptMessage> {
    let mut items = Vec::new();
    for i in 0..200 {
        items.push(serde_json::from_value(json!({
            "type":"message", "id":format!("request-{i}"), "role":"assistant", "content":"Question",
            "interaction":{"version":crate::interactions::VERSION,"request_id":format!("owner/request-{i}"),"handler":crate::interactions::AGENT_CONFIGURATION,"kind":"request","request":{"action":"input","title":"Question",
                "fields":[{"id":"answer","label":"Answer"}]}}
        })).unwrap());
    }
    for i in 0..200 {
        items.push(serde_json::from_value(json!({
            "type":"message", "id":format!("result-{i}"), "role":"assistant", "content":"Input accepted", "author_kind":"system",
            "interaction":{"version":crate::interactions::VERSION,"request_id":format!("owner/request-{i}"),"handler":crate::interactions::AGENT_CONFIGURATION,"kind":"result","result":{"request_message_id":format!("request-{i}"),
                "response_id":format!("response-{i}"),"revision":1,"outcome":"completed","actor":"user","output":{"values":{"answer":"done"}}}}
        })).unwrap());
    }
    if with_tail {
        items.push(
            serde_json::from_value(
                json!({"type":"message","id":"tail","role":"assistant","content":"Latest message"}),
            )
            .unwrap(),
        );
    }
    items
}

fn page(items: &[TranscriptMessage], end: usize) -> MessagePage {
    let start = end.saturating_sub(100);
    MessagePage {
        source_epoch: None,
        items: items[start..end].to_vec(),
        older_cursor: (start > 0).then(|| format!("offset-{start}")),
    }
}

#[tokio::test]
async fn folded_history_pages_advance_by_source_and_refresh_preserves_progress() {
    // Exercise local-only cache paging, remote result-before-request paging,
    // and the in-memory fixture contract, with and without a visible tail.
    for (cached, all_cached) in [(true, true), (true, false), (false, false)] {
        for with_tail in [true, false] {
            let items = Arc::new(history(with_tail));
            let reads = Arc::new(AtomicUsize::new(0));
            let server_items = items.clone();
            let server_reads = reads.clone();
            let app = Router::new().route(
                "/v1/im/sessions/chat/messages",
                get(move |Query(query): Query<HashMap<String, String>>| {
                    let items = server_items.clone();
                    let reads = server_reads.clone();
                    async move {
                        reads.fetch_add(1, Ordering::Relaxed);
                        let end = query
                            .get("before")
                            .map(|cursor| cursor.strip_prefix("offset-").unwrap().parse().unwrap())
                            .unwrap_or(items.len());
                        Json(page(&items, end))
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
            if all_cached {
                store
                    .cache_message_page(
                        "node",
                        "chat",
                        &MessagePage {
                            source_epoch: None,
                            items: items.as_ref().clone(),
                            older_cursor: None,
                        },
                        None,
                    )
                    .unwrap();
            }
            let device = Device::open(
                Arc::new(GatewayClient::new(&url, None)),
                cached.then(|| (store.clone(), "node".into())),
                false,
            );
            let conversation = device.conversation("chat");
            let latest = page(&items, items.len());
            conversation.apply_page(&latest);
            assert_eq!(conversation.snapshot().lines.len(), usize::from(with_tail));
            let mut updates = conversation.subscribe();
            updates.snapshot();
            let mut pages = 0;
            while let Some(before) = conversation.snapshot().older_cursor.clone() {
                pages += 1;
                assert!(pages <= 5, "folded results stalled pagination");
                conversation.load_older();
                tokio::time::timeout(Duration::from_secs(5), async {
                    while conversation.snapshot().loading_older {
                        updates.changed().await.unwrap();
                    }
                })
                .await
                .unwrap();
                let state = conversation.snapshot();
                assert!(state.error.is_none(), "{:?}", state.error);
                assert_ne!(
                    state.older_cursor.as_ref(),
                    Some(&before),
                    "source page was not consumed"
                );
                let cursor = state.older_cursor.clone();
                conversation.apply_page(&latest);
                assert_eq!(
                    conversation.snapshot().older_cursor,
                    cursor,
                    "refresh reset source progress"
                );
            }
            let state = conversation.snapshot();
            assert_eq!(state.lines.len(), 200 + usize::from(with_tail));
            for i in 0..200 {
                let TranscriptLine::Message { metadata, .. } = &state.lines[i];
                assert_eq!(
                    metadata.id.as_deref(),
                    Some(format!("request-{i}").as_str())
                );
                let card = metadata.interaction_view.as_ref().unwrap();
                assert_eq!(card.status_key, "interaction_completed");
                assert!(card.actions.is_empty());
            }
            if all_cached {
                assert_eq!(reads.load(Ordering::Relaxed), 0);
            }
            server.abort();
        }
    }
}
