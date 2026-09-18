//! Run separately from compilation/UI load:
//! cargo bench --locked -p zork-client-core --bench messages
//! One million messages in one chat, plus one million spread across 100 chats.
//! JSON stdout is the report; temporary databases are removed on exit.
#[path = "support/messages.rs"]
mod fixture;
use anyhow::Result;
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::{hint::black_box, time::Instant};
use zork_client_core::{
    api::{MessageMetadata, MessagePage, Role, TranscriptMessage},
    store::ClientStore,
};

const MESSAGES: usize = 1_000_000;
const CHATS: usize = 100;
const PAGE: usize = 100;
const SAMPLES: usize = 500;

fn message(index: usize) -> TranscriptMessage {
    TranscriptMessage::Message {
        role: if index % 2 == 0 {
            Role::User
        } else {
            Role::Assistant
        },
        content: fixture::content(index),
        metadata: MessageMetadata {
            id: Some(format!("message-{index:09}")),
            created_at: Some("2026-09-08T00:00:00Z".into()),
            author_agent_id: Some(format!("agent-{}", index % 8)),
            author_name: Some(format!("队员 {}", index % 8)),
            device: Some("fixture-device".into()),
            ..Default::default()
        },
    }
}

fn page(start: usize, count: usize) -> MessagePage {
    MessagePage {
        source_epoch: None,
        items: (start..start + count).map(message).collect(),
        older_cursor: None,
    }
}

fn distribution(mut values: Vec<f64>) -> Value {
    values.sort_by(f64::total_cmp);
    let percentile = |p: usize| values[(values.len() * p).div_ceil(100).saturating_sub(1)];
    json!({"samples":values.len(),"p50_ms":percentile(50),"p95_ms":percentile(95),"p99_ms":percentile(99),"max_ms":values.last()})
}

fn measure(count: usize, mut run: impl FnMut(usize) -> Result<()>) -> Result<Value> {
    let mut values = Vec::with_capacity(count);
    for i in 0..count {
        let start = Instant::now();
        run(i)?;
        values.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    Ok(distribution(values))
}

fn query(store: &ClientStore, session: &str, before: Option<&str>) -> Result<()> {
    let page = store
        .cached_messages("node", session, before, PAGE)?
        .unwrap();
    assert_eq!(page.items.len(), PAGE);
    black_box(page);
    Ok(())
}

fn main() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let store = ClientStore::open(directory.path())?;
    let mut report = json!({"single_chat_messages":MESSAGES,"distributed_messages":MESSAGES,"distributed_chats":CHATS,"page_size":PAGE,"fixture":"12 mixed message forms: prose, Markdown, code, comments, attachments, long content","cache_note":"warm repeated queries and fresh SQLite connections; OS page cache is not purged"});
    // Establish the indexed-query baseline before the large import.
    store.cache_message_page("node", "single", &page(0, 1000), None)?;
    report["latest_page_at_1000"] = measure(SAMPLES, |_| query(&store, "single", None))?;
    let start = Instant::now();
    for offset in (1000..MESSAGES).step_by(PAGE) {
        store.cache_message_page("node", "single", &page(offset, PAGE), None)?;
    }
    report["single_import_ms"] = json!(start.elapsed().as_secs_f64() * 1000.0);
    eprintln!("single-chat import: {MESSAGES} messages");
    let start = Instant::now();
    for chat in 0..CHATS {
        let session = format!("chat-{chat}");
        for offset in (0..MESSAGES / CHATS).step_by(PAGE) {
            store.cache_message_page("node", &session, &page(offset, PAGE), None)?;
        }
    }
    report["distributed_import_ms"] = json!(start.elapsed().as_secs_f64() * 1000.0);
    eprintln!("distributed import: {MESSAGES} messages across {CHATS} chats");

    let conn = Connection::open(directory.path().join("client.db"))?;
    let count: usize = conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))?;
    assert_eq!(count, MESSAGES * 2);
    report["verified_rows"] = json!(count);
    let plan = conn.prepare("EXPLAIN QUERY PLAN SELECT position,value FROM messages WHERE node=?1 AND session=?2 AND position<?3 ORDER BY position DESC LIMIT ?4")?
        .query_map(params!["node", "single", MESSAGES / 2, PAGE + 1], |r| r.get::<_, String>(3))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    assert!(
        plan.iter()
            .any(|line| line.contains("SEARCH messages USING INDEX")),
        "{plan:?}"
    );
    assert!(
        !plan
            .iter()
            .any(|line| line.contains("SCAN messages") || line.contains("TEMP B-TREE")),
        "{plan:?}"
    );
    report["query_plan"] = json!(plan);
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    report["database_bytes"] = json!(std::fs::metadata(directory.path().join("client.db"))?.len());
    report["latest_page_at_million"] = measure(SAMPLES, |_| query(&store, "single", None))?;
    // Select real local page cursors returned by the store, not station offsets.
    let midpoint = store
        .cached_messages(
            "node",
            "single",
            Some(&format!("zork-cache:{}", MESSAGES / 2 + PAGE)),
            PAGE,
        )?
        .unwrap()
        .older_cursor
        .unwrap();
    report["middle_page"] = measure(SAMPLES, |_| query(&store, "single", Some(&midpoint)))?;
    let startpoint = store
        .cached_messages("node", "single", Some("zork-cache:200"), PAGE)?
        .unwrap()
        .older_cursor
        .unwrap();
    report["oldest_page"] = measure(SAMPLES, |_| query(&store, "single", Some(&startpoint)))?;
    report["switch_between_100_chats"] = measure(SAMPLES, |i| {
        query(&store, &format!("chat-{}", (i * 37) % CHATS), None)
    })?;
    // Reopening a connection discards SQLite's page cache, but leaves the OS cache.
    report["reopened_connection_latest_page"] = measure(100, |_| {
        let reopened = ClientStore::open(directory.path())?;
        query(&reopened, "single", None)
    })?;

    let replay = page(MESSAGES - PAGE, PAGE);
    let changes_before: u64 = conn.query_row("PRAGMA data_version", [], |r| r.get(0))?;
    report["duplicate_page"] = measure(SAMPLES, |_| {
        store.cache_message_page("node", "single", &replay, None)
    })?;
    let changes_after: u64 = conn.query_row("PRAGMA data_version", [], |r| r.get(0))?;
    assert_eq!(
        changes_before, changes_after,
        "duplicate pages wrote to the database"
    );
    let mut append_samples = Vec::new();
    for i in 0..SAMPLES {
        let incoming = page(MESSAGES + i, 1);
        let start = Instant::now();
        store.cache_message_page("node", "single", &incoming, None)?;
        append_samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    report["append_one_message"] = distribution(append_samples);
    let barrier = std::sync::Barrier::new(2);
    report["queries_during_background_appends"] = std::thread::scope(|scope| -> Result<Value> {
        let writer = scope.spawn(|| -> Result<()> {
            barrier.wait();
            for i in 0..SAMPLES {
                store.cache_message_page(
                    "node",
                    "single",
                    &page(MESSAGES + SAMPLES + i, 1),
                    None,
                )?;
            }
            Ok(())
        });
        barrier.wait();
        let samples = measure(SAMPLES, |i| {
            query(&store, &format!("chat-{}", i % CHATS), None)
        })?;
        writer.join().unwrap()?;
        Ok(samples)
    })?;
    #[cfg(unix)]
    {
        let output = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()?;
        report["rss_kib_before_legacy_baseline"] = json!(String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse::<u64>()
            .ok());
    }

    // The previous implementation decoded/wrote the complete loaded snapshot.
    // Use the same million-message fixture to quantify that storage operation,
    // without claiming these samples measure an entire old GUI render frame.
    eprintln!("measuring legacy whole-JSON snapshot on the same million-message fixture");
    let legacy = page(0, MESSAGES);
    let baseline_directory = tempfile::tempdir()?;
    let baseline = ClientStore::open(baseline_directory.path())?;
    if let Err(error) = baseline.put("node", "messages:single", &legacy) {
        report["legacy_million_message_snapshot_error"] = json!(error.to_string());
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    report["legacy_full_snapshot_read"] = measure(10, |_| {
        let loaded: MessagePage = baseline.get("node", "messages:single")?.unwrap();
        assert_eq!(loaded.items.len(), MESSAGES);
        black_box(loaded);
        Ok(())
    })?;
    let mut legacy = legacy;
    let mut values = Vec::new();
    for i in 0..10 {
        legacy.items.push(message(MESSAGES + i));
        let start = Instant::now();
        baseline.put("node", "messages:single", &legacy)?;
        values.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    report["legacy_append_rewrites_full_snapshot"] = distribution(values);
    report["legacy_sample_note"] = json!("10 expensive full-snapshot samples; their p95/p99 are the observed maximum, not robust tail estimates");
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
