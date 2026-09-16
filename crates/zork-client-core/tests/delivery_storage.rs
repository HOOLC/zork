//! Standalone persistence measurement. Build first, then run only this ignored
//! test without concurrent builds; it does not measure UI rendering or FPS.
#[path = "../benches/support/messages.rs"]
mod fixture;

use rusqlite::{params, Connection};
use serde_json::json;
use std::time::Instant;
use zork_client_core::{
    api::{MessageMetadata, MessagePage, Role, TranscriptMessage},
    store::{ClientStore, QueuedMessage},
};

fn message(index: usize) -> TranscriptMessage {
    TranscriptMessage::Message {
        role: if index % 2 == 0 {
            Role::User
        } else {
            Role::Assistant
        },
        content: fixture::content(index),
        metadata: MessageMetadata {
            id: Some(format!("m{index}")),
            source_epoch: Some("source".into()),
            source_sequence: Some(index as i64 + 1),
            ..Default::default()
        },
    }
}

fn distribution(mut values: Vec<f64>) -> serde_json::Value {
    values.sort_by(f64::total_cmp);
    let at = |percent: usize| values[(values.len() * percent).div_ceil(100) - 1];
    json!({"samples":values.len(),"p50_ms":at(50),"p95_ms":at(95),"p99_ms":at(99)})
}

#[test]
#[ignore = "standalone 100000-message persistence and dispatch measurement"]
fn hundred_thousand_messages_keep_pending_dispatch_indexed_and_replays_quiet() {
    let root = tempfile::tempdir().unwrap();
    let store = ClientStore::open(root.path()).unwrap();
    let started = Instant::now();
    for start in (0..100_000).step_by(100) {
        store
            .cache_message_page(
                "node",
                "chat",
                &MessagePage {
                    source_epoch: Some("source".into()),
                    items: (start..start + 100).map(message).collect(),
                    older_cursor: None,
                },
                None,
            )
            .unwrap();
    }
    let import_ms = started.elapsed().as_secs_f64() * 1000.0;
    for index in 0..3 {
        store
            .enqueue(
                "node",
                &QueuedMessage {
                    request_id: format!("pending-{index}"),
                    session_id: "chat".into(),
                    content: fixture::content(index),
                    attempted: false,
                    ..Default::default()
                },
            )
            .unwrap();
    }
    let conn = Connection::open(root.path().join("client.db")).unwrap();
    let plan = conn.prepare("EXPLAIN QUERY PLAN SELECT request_id,session,value,attempted,sent_at_ms,error FROM messages WHERE node=?1 AND status IN ('sending','failed') ORDER BY rowid").unwrap()
        .query_map(["node"], |row|row.get::<_,String>(3)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
    assert!(
        plan.iter().any(|line| line.contains("node=? AND status=?")),
        "{plan:?}"
    );
    let mut pending = Vec::new();
    let mut pages = Vec::new();
    for index in 0..120 {
        let started = Instant::now();
        assert_eq!(store.outbox("node").unwrap().len(), 3);
        pending.push(started.elapsed().as_secs_f64() * 1000.0);
        let position = 100 + (index * 7919) % 99_900;
        let started = Instant::now();
        assert_eq!(
            store
                .cached_messages(
                    "node",
                    "chat",
                    Some(&format!("zork-cache:source:{position}")),
                    100
                )
                .unwrap()
                .unwrap()
                .items
                .len(),
            100
        );
        pages.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    let tail = store
        .cached_messages("node", "chat", None, 100)
        .unwrap()
        .unwrap();
    let before: u64 = conn
        .query_row("PRAGMA data_version", [], |row| row.get(0))
        .unwrap();
    let mut replay = tail.clone();
    replay.older_cursor = None;
    for _ in 0..20 {
        store
            .cache_message_page("node", "chat", &replay, None)
            .unwrap();
    }
    let after: u64 = conn
        .query_row("PRAGMA data_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(before, after, "repeated source pages wrote to the database");
    let row_before: i64 = conn
        .query_row(
            "SELECT rowid FROM messages WHERE request_id='pending-0'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut echo = message(100_000);
    let TranscriptMessage::Message {
        content, metadata, ..
    } = &mut echo;
    *content = fixture::content(0);
    metadata.id = Some("client-chat-pending-0".into());
    store
        .cache_message_page(
            "node",
            "chat",
            &MessagePage {
                source_epoch: Some("source".into()),
                items: vec![message(99_999), echo],
                older_cursor: None,
            },
            None,
        )
        .unwrap();
    let row_after: (i64, String) = conn
        .query_row(
            "SELECT rowid,status FROM messages WHERE request_id=?1",
            params!["pending-0"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(row_after, (row_before, "sent".into()));
    assert_eq!(store.outbox("node").unwrap().len(), 2);
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    println!(
        "{}",
        json!({"records":100000,"pending_rows":3,"page_messages":100,"import_ms":import_ms,
        "pending_dispatch":distribution(pending),"indexed_page":distribution(pages),"query_plan":plan,
        "database_bytes":std::fs::metadata(root.path().join("client.db")).unwrap().len(),
        "fixture":"12 mixed message forms: prose, Markdown, code, comments, attachments and long Unicode; warm OS cache; dev profile"})
    );
}
