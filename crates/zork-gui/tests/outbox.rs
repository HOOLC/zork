use zork_gui::desktop::store::{ClientStore, QueuedMessage};

#[test]
fn legacy_queue_entries_do_not_claim_that_delivery_was_never_attempted() {
    let root = tempfile::tempdir().unwrap();
    let db = rusqlite::Connection::open(root.path().join("client.db")).unwrap();
    db.execute_batch("CREATE TABLE outbox(node TEXT,request_id TEXT,value TEXT);")
        .unwrap();
    db.execute(
        "INSERT INTO outbox(node,request_id,value) VALUES('mini1','legacy',?1)",
        [r#"{"request_id":"legacy","session_id":"task","content":"existing work"}"#],
    )
    .unwrap();
    drop(db);
    let store = ClientStore::open(root.path()).unwrap();
    assert!(store.outbox("mini1").unwrap()[0].attempted);
    assert!(store.cancel_pending("mini1", "legacy").is_err());
    assert!(store.begin_delivery("mini1", "legacy").unwrap().is_none());
}

#[test]
fn a_cancelled_message_cannot_be_delivered_from_an_old_queue_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let store = ClientStore::open(root.path()).unwrap();
    let message = QueuedMessage {
        request_id: "one".into(),
        session_id: "task".into(),
        content: "draft".into(),
        attempted: false,
        sent_at_ms: zork_client_core::store::delivery_now_ms(),
        ..Default::default()
    };
    store.enqueue("mini1", &message).unwrap();
    let snapshot = store.outbox("mini1").unwrap();
    assert_eq!(
        store
            .cancel_pending("mini1", "one")
            .unwrap()
            .unwrap()
            .content,
        "draft"
    );
    assert!(store
        .begin_delivery("mini1", &snapshot[0].request_id)
        .unwrap()
        .is_none());
    assert!(store.outbox("mini1").unwrap().is_empty());
}

#[test]
fn uncertain_delivery_survives_restart_and_cannot_be_claimed_as_withdrawn() {
    let root = tempfile::tempdir().unwrap();
    {
        let store = ClientStore::open(root.path()).unwrap();
        let message = QueuedMessage {
            request_id: "one".into(),
            session_id: "task".into(),
            content: "work".into(),
            attempted: false,
            sent_at_ms: zork_client_core::store::delivery_now_ms(),
            ..Default::default()
        };
        store.enqueue("mini1", &message).unwrap();
        assert!(
            store
                .begin_delivery("mini1", "one")
                .unwrap()
                .unwrap()
                .attempted
        );
    }
    let store = ClientStore::open(root.path()).unwrap();
    assert!(store.cancel_pending("mini1", "one").is_err());
    assert!(store.begin_delivery("mini1", "one").unwrap().is_none());
    store.retry_delivery("mini1", "one").unwrap();
    let next = store
        .outbox("mini1")
        .unwrap()
        .into_iter()
        .find(|message| !message.attempted)
        .unwrap();
    let retry = store
        .begin_delivery("mini1", &next.request_id)
        .unwrap()
        .unwrap();
    assert_ne!(retry.request_id, "one");
    assert_eq!(retry.content, "work");
    assert!(store.outbox("mini2").unwrap().is_empty());
    let page = zork_client_core::api::MessagePage {
        source_epoch: None,
        items: ["one", retry.request_id.as_str()].into_iter().map(|request| {
            serde_json::from_value(serde_json::json!({"type":"message", "id":format!("client-task-{request}"), "role":"user", "content":"work"})).unwrap()
        }).collect(),
        older_cursor: None,
    };
    store
        .cache_message_page("mini1", "task", &page, None)
        .unwrap();
    assert!(store.outbox("mini1").unwrap().is_empty());
    assert_eq!(
        store
            .cached_messages("mini1", "task", None, 100)
            .unwrap()
            .unwrap()
            .items
            .len(),
        2
    );
}
