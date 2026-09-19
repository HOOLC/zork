use super::*;

#[test]
fn invitation_replacement_rejects_an_unapplied_old_claim_without_a_device_controller() {
    let state = crate::enrollment::InvitationState::new(
        json!({"invitation":{"id":"first","status":"waiting"}}),
    );
    let mut observer = WireSubscription::from_invitation(&state.source);
    let first = observer.prepare().unwrap().unwrap();
    let batch = first["batch"].as_u64().unwrap();
    state.replace(json!({"invitation":{"id":"second","status":"waiting"}}));
    assert!(!observer.valid(batch));
    assert!(!observer.finish(batch, true));
    let second = observer.prepare().unwrap().unwrap();
    assert_eq!(second["snapshot"]["invitation"]["id"], "second");
    assert_eq!(second["from"], 0);
    assert!(observer.finish(second["batch"].as_u64().unwrap(), true));
    assert!(observer.prepare().unwrap().is_none());
}
use crate::{
    api::{MessageMetadata, Role, SseEvent, StationClient},
    state::ConversationData,
    transcript::TranscriptLine,
};
use futures_util::FutureExt;

fn event(id: &str, text: &str) -> SseEvent {
    SseEvent {
        name: "message".into(),
        data: json!({"type":"message","role":"user","id":id,"content":text}).to_string(),
    }
}
fn open(device: &Arc<Device>, store: &Arc<ClientStore>) -> WireSubscription {
    WireSubscription::from_device(
        Key::Conversation {
            peer: "peer".into(),
            session: Some("chat".into()),
        },
        device.clone(),
        store.clone(),
    )
    .unwrap()
}
fn apply(rows: &mut Vec<Value>, frame: &Value) {
    let state = &frame["state"];
    if let Some(messages) = state["messages"].as_array() {
        *rows = messages.clone();
    }
    if let Some(edits) = state["message_edits"].as_array() {
        for edit in edits {
            let start = edit["start"].as_u64().unwrap() as usize;
            let end = edit["end"].as_u64().unwrap() as usize;
            rows.splice(start..end, edit["insert"].as_array().unwrap().clone());
        }
    }
}
fn consume(reader: &mut WireSubscription, rows: &mut Vec<Value>) -> Arc<Value> {
    let frame = reader.prepare().unwrap().unwrap();
    apply(rows, &frame);
    assert!(reader.finish(frame["batch"].as_u64().unwrap(), true));
    frame
}

#[test]
fn wire_retries_from_applied_window_and_encodes_only_changed_rows() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _runtime = runtime.enter();
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(ClientStore::open(root.path()).unwrap());
    let device = Device::open(
        Arc::new(StationClient::new("http://127.0.0.1:9", None)),
        None,
        true,
    );
    let chat = device.conversation("chat");
    let mut early = open(&device, &store);
    let initial = early.prepare().unwrap().unwrap();
    assert_eq!(initial["state"]["messages"], json!([]));
    assert!(early.finish(initial["batch"].as_u64().unwrap(), true));
    chat.seed(ConversationData {
        lines: (0..100_000)
            .map(|i| TranscriptLine::Message {
                role: Role::User,
                content: format!("body {i}"),
                metadata: MessageMetadata {
                    id: Some(format!("m{i}")),
                    ..Default::default()
                },
            })
            .collect(),
        loaded: true,
        ..Default::default()
    });
    let mut fast = open(&device, &store);
    let mut slow = open(&device, &store);
    let mut fast_rows = vec![];
    let mut slow_rows = vec![];
    let frame = consume(&mut fast, &mut fast_rows);
    consume(&mut slow, &mut slow_rows);
    assert_eq!(frame["state"]["message_total"], 100_000);
    assert_eq!(fast_rows.len(), 100);
    chat.seed_event(&event("m100000", "new"));
    let cancelled = slow.prepare().unwrap().unwrap();
    assert!(Arc::ptr_eq(&cancelled, &slow.prepare().unwrap().unwrap()));
    let frame = consume(&mut fast, &mut fast_rows);
    assert!(frame["state"].get("messages").is_none());
    assert!(frame["state"].get("message_order").is_none());
    assert_eq!(
        frame["state"]["message_edits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["insert"].as_array().unwrap().len())
            .sum::<usize>(),
        1
    );
    for i in 100001..100601 {
        chat.seed_event(&event(&format!("m{i}"), "burst"));
        consume(&mut fast, &mut fast_rows);
    }
    assert!(slow.finish(cancelled["batch"].as_u64().unwrap(), false));
    let recovery = consume(&mut slow, &mut slow_rows);
    assert_eq!(slow_rows, fast_rows);
    assert!(recovery["reset"].as_bool().unwrap());
    assert_eq!(recovery["state"]["messages"].as_array().unwrap().len(), 100);
    chat.seed_event(&event("m100600", "burst"));
    assert!(
        fast.prepare().unwrap().is_none(),
        "duplicate delivery advanced wire cursor"
    );
    fast.older().unwrap();
    let larger = consume(&mut fast, &mut fast_rows);
    assert!(larger["reset"].as_bool().unwrap());
    assert_eq!(fast_rows.len(), 200);
    assert!(
        slow.prepare().unwrap().is_none(),
        "one reader changed another's window"
    );
    let old = early.prepare().unwrap().unwrap();
    chat.seed(ConversationData::default());
    assert!(!early.valid(old["batch"].as_u64().unwrap()));
    let cleared = early.prepare().unwrap().unwrap();
    assert_eq!(cleared["state"]["messages"], json!([]));
    assert!(!early.finish(old["batch"].as_u64().unwrap(), true));
}

#[test]
fn saved_echo_updates_pending_wire_message_without_another_receive_callback() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _runtime = runtime.enter();
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(ClientStore::open(root.path()).unwrap());
    let device = Device::open(
        Arc::new(StationClient::new("http://127.0.0.1:9", None)),
        Some((store.clone(), "peer".into())),
        true,
    );
    let mut reader = open(&device, &store);
    let mut rows = vec![];
    consume(&mut reader, &mut rows);
    device.edit_draft("chat", "hello".into()).unwrap();
    let sent = device.enqueue("chat", "hello".into()).unwrap();
    let frame = consume(&mut reader, &mut rows);
    assert_eq!(frame["state"]["draft_document"]["text"], "");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["pending"], true);
    let id = format!("client-chat-{}", sent.request_id);
    assert_eq!(rows[0]["request_id"], sent.request_id);
    store
        .fail_delivery("peer", &sent.request_id, "目标设备不支持当前消息协议")
        .unwrap();
    device.recover_outbox();
    consume(&mut reader, &mut rows);
    assert_eq!(rows[0]["content"], "hello");
    assert_eq!(rows[0]["delivery_status"], "failed");
    assert_eq!(rows[0]["delivery_error"], "目标设备不支持当前消息协议");
    let authoritative = event(&id, "hello");
    let message = serde_json::from_str(&authoritative.data).unwrap();
    store
        .cache_message_page(
            "peer",
            "chat",
            &crate::api::MessagePage {
                source_epoch: None,
                items: vec![message],
                older_cursor: None,
            },
            None,
        )
        .unwrap();
    device.recover_outbox();
    consume(&mut reader, &mut rows);
    assert_ne!(rows[0]["pending"], true);
    device.conversation("chat").seed_event(&authoritative);
    consume(&mut reader, &mut rows);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], id);
    assert_ne!(rows[0]["pending"], true);
}

#[test]
fn reading_window_keeps_its_anchor_through_a_burst_and_pages_in_both_directions() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(ClientStore::open(root.path()).unwrap());
    let device = Device::open(
        Arc::new(StationClient::new("http://127.0.0.1:9", None)),
        None,
        true,
    );
    let chat = device.conversation("chat");
    for i in 0..300 {
        chat.seed_event(&event(&format!("m{i}"), "old"));
    }
    let mut reader = open(&device, &store);
    let mut rows = vec![];
    consume(&mut reader, &mut rows);
    reader.window_anchor(Some("m200".into())).unwrap();
    consume(&mut reader, &mut rows);
    let old_rows = rows.clone();
    for i in 300..1300 {
        chat.seed_event(&event(&format!("m{i}"), "new"));
    }
    let frame = consume(&mut reader, &mut rows);
    assert_eq!(
        rows, old_rows,
        "new messages evicted what the user was reading"
    );
    assert_eq!(frame["state"]["newer_available"], true);
    assert_eq!(frame["state"]["message_arrivals"]["count"], 1000);
    reader.newer().unwrap();
    consume(&mut reader, &mut rows);
    assert_eq!(rows.len(), 200);
    assert_eq!(rows.last().unwrap()["id"], "m399");
    reader.older().unwrap();
    consume(&mut reader, &mut rows);
    assert_eq!(rows.len(), 300);
    assert_eq!(rows.first().unwrap()["id"], "m100");
    reader.window_anchor(None).unwrap();
    let tail = consume(&mut reader, &mut rows);
    assert_eq!(rows.len(), 300);
    assert_eq!(rows.last().unwrap()["id"], "m1299");
    assert_eq!(tail["state"]["newer_available"], false);
}

#[tokio::test]
async fn settings_wake_on_committed_changes_and_revocation_interrupts_prepared_data() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(ClientStore::open(root.path()).unwrap());
    let device = Device::open(
        Arc::new(StationClient::new("http://127.0.0.1:9", None)),
        Some((store.clone(), "peer".into())),
        true,
    );
    let catalog = json!({"ready":true,"profiles":[{"profile_id":"private-profile"}]});
    store.put("peer", "public-settings", &catalog).unwrap();
    let mut reader = WireSubscription::from_device(
        Key::Settings {
            peer: "peer".into(),
        },
        device,
        store.clone(),
    )
    .unwrap();
    let mut signals = reader.signals();
    signals.changed().await.unwrap();
    let first = reader.prepare().unwrap().unwrap();
    assert_eq!(
        first["snapshot"]["profiles"][0]["profile_id"],
        "private-profile"
    );
    assert!(reader.finish(first["batch"].as_u64().unwrap(), true));
    store.put("peer", "public-settings", &catalog).unwrap();
    store.put("peer", "draft:chat", &"unrelated").unwrap();
    tokio::task::yield_now().await;
    assert!(signals.changed().now_or_never().is_none());
    store
        .put("peer", "node-operation", &json!({"progress":42}))
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), signals.changed())
        .await
        .unwrap()
        .unwrap();
    let in_flight = reader.prepare().unwrap().unwrap();
    assert_eq!(in_flight["snapshot"]["operation"]["progress"], 42);
    store.revoke_replica("peer").unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), signals.changed())
            .await
            .unwrap()
            .unwrap()
    );
    assert!(!reader.valid(in_flight["batch"].as_u64().unwrap()));
    let clear = reader.prepare().unwrap().unwrap();
    assert_eq!(clear["snapshot"]["revoked"], true);
    assert!(!clear.to_string().contains("private-profile"));
    assert!(reader.finish(clear["batch"].as_u64().unwrap(), true));
    drop(reader);
    assert!(signals.changed().await.is_err());
}
