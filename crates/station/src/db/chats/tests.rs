use super::*;
use zork_client_types::chat::{MessageFilter, PreferenceChanges};

fn database() -> (tempfile::TempDir, StationDb) {
    let root = tempfile::tempdir().unwrap();
    let db = StationDb::open(root.path(), &root.path().join("workspaces")).unwrap();
    (root, db)
}
fn channel(db: &StationDb, key: &str) -> Channel {
    let receipt = db.chat_begin(key, key).unwrap();
    db.create_chat(key, &receipt.object_id, key).unwrap()
}
fn author(id: &str) -> Author {
    Author {
        id: id.into(),
        kind: AuthorKind::Agent,
        name: Some(id.into()),
    }
}
fn preferences(
    db: &StationDb,
    key: &str,
    chat: &str,
    agent: &str,
    changes: PreferenceChanges,
    start: Option<StartAt>,
) -> Preferences {
    db.chat_begin(key, key).unwrap();
    db.update_chat_preferences(
        key,
        chat,
        agent,
        &UpdatePreferences {
            changes,
            expected_revision: None,
            start,
        },
    )
    .unwrap()
}
fn post(
    db: &StationDb,
    key: &str,
    chat: &str,
    who: &str,
    reply: Option<&str>,
    mentions: &[String],
) -> Message {
    let receipt = db.chat_begin(key, key).unwrap();
    db.post_chat_message(
        key,
        &receipt.object_id,
        chat,
        &author(who),
        key,
        &[],
        reply,
        mentions,
    )
    .unwrap()
}

#[test]
fn pages_commit_with_explicit_channel_authorship_and_delivery_notices() {
    let (_root, db) = database();
    let chat = channel(&db, "page-channel");
    let other = channel(&db, "other-channel");
    preferences(
        &db,
        "page-reader",
        &chat.chat_id,
        "reader",
        PreferenceChanges {
            subscribed: Some(true),
            ..Default::default()
        },
        None,
    );
    let receipt = db.chat_begin("page-send", "same-page").unwrap();
    let page = crate::db::pages::page_link("Report", "https://example.test/report", "").unwrap();
    let text = crate::db::pages::page_text(&page);
    let mut forged = page.clone();
    forged.id = "incorrect".into();
    let send = |page| {
        db.post_chat_with_pages(
            "page-send",
            &receipt.object_id,
            &chat.chat_id,
            &author("writer"),
            &text,
            &[],
            None,
            &[],
            &[zork_client_types::pages::DeliveredPage { page }],
        )
    };
    assert!(send(forged).is_err());
    assert_eq!(db.chat(&chat.chat_id).unwrap().channel.message_count, 0);
    assert!(db
        .chat_notice_page("local", None, 0)
        .unwrap()
        .items
        .is_empty());
    let message = send(page.clone()).unwrap();
    let catalog = db.page_catalog().unwrap();
    assert_eq!(catalog.references.len(), 1);
    assert_eq!(catalog.references[0].session_id, chat.chat_id);
    assert_eq!(catalog.references[0].page, page);
    assert!(catalog.applications.is_empty());
    assert_eq!(db.chat(&other.chat_id).unwrap().channel.message_count, 0);
    assert_eq!(
        db.chat_notice_page("local", None, 0).unwrap().items[0].message,
        message
    );
    assert_eq!(message.author.id, "writer");
    assert!(db.chat_receipt("page-send").unwrap().is_some());
}

#[test]
fn posting_subscription_and_participation_are_independent() {
    let (_root, db) = database();
    let chat = channel(&db, "channel");
    assert!(db.list_product_tasks().unwrap().is_empty());
    assert_eq!(
        db.chat_preferences(&chat.chat_id, "writer").unwrap(),
        Preferences::default()
    );
    preferences(
        &db,
        "subscribe",
        &chat.chat_id,
        "reader",
        PreferenceChanges {
            subscribed: Some(true),
            ..Default::default()
        },
        None,
    );
    assert!(db.chat_participants(&chat.chat_id).unwrap().is_empty());
    let sent = post(&db, "first", &chat.chat_id, "writer", None, &[]);
    let participants = db.chat_participants(&chat.chat_id).unwrap();
    assert_eq!(participants.len(), 1);
    assert_eq!(participants[0].author.id, "writer");
    assert!(!participants[0].subscribed);
    let page = db.chat_notice_page("local", None, 0).unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].agent_ref, "reader");
    assert_eq!(page.items[0].message, sent);
    preferences(
        &db,
        "writer-follows",
        &chat.chat_id,
        "writer",
        PreferenceChanges {
            subscribed: Some(true),
            ..Default::default()
        },
        None,
    );
    let next = post(&db, "second", &chat.chat_id, "writer", None, &[]);
    let page = db
        .chat_notice_page("local", Some(&page.epoch), page.through)
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].agent_ref, "reader");
    assert_eq!(page.items[0].message, next);
    preferences(
        &db,
        "writer-leaves",
        &chat.chat_id,
        "writer",
        PreferenceChanges {
            subscribed: Some(false),
            ..Default::default()
        },
        None,
    );
    assert_eq!(
        db.chat_participants(&chat.chat_id).unwrap()[0].message_count,
        2
    );
    assert!(!db.chat_participants(&chat.chat_id).unwrap()[0].subscribed);
    assert_eq!(db.chat(&chat.chat_id).unwrap().channel.message_count, 2);
}

#[tokio::test]
async fn only_changed_preferences_and_matching_recipients_wake_readers() {
    let (_root, db) = database();
    let chat = channel(&db, "channel");
    let mut channel_changes = db
        .chat_topics
        .subscribe([Topic::Channel(chat.chat_id.clone())]);
    let mut remote_changes = db
        .chat_topics
        .subscribe([Topic::Recipient("other-node".into())]);
    let mut work = db.realtime.listen(crate::realtime::WORK);
    preferences(
        &db,
        "default-noop",
        &chat.chat_id,
        "reader",
        PreferenceChanges {
            subscribed: Some(false),
            ..Default::default()
        },
        None,
    );
    assert!(tokio::time::timeout(
        std::time::Duration::from_millis(20),
        channel_changes.changed()
    )
    .await
    .is_err());
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), work.changed())
            .await
            .is_err()
    );
    preferences(
        &db,
        "remote-subscribe",
        &chat.chat_id,
        "node/reader",
        PreferenceChanges {
            subscribed: Some(true),
            filter: Some(MessageFilter::Mentions),
            ..Default::default()
        },
        None,
    );
    channel_changes.changed().await.unwrap();
    channel_changes.checkpoint();
    post(&db, "unmentioned", &chat.chat_id, "writer", None, &[]);
    assert!(db
        .chat_notice_page("node", None, 0)
        .unwrap()
        .items
        .is_empty());
    assert!(tokio::time::timeout(
        std::time::Duration::from_millis(20),
        remote_changes.changed()
    )
    .await
    .is_err());
    let mut recipient = db.chat_topics.subscribe([Topic::Recipient("node".into())]);
    post(
        &db,
        "mentioned",
        &chat.chat_id,
        "writer",
        None,
        &["node/reader".into()],
    );
    recipient.changed().await.unwrap();
    assert_eq!(db.chat_notice_page("node", None, 0).unwrap().items.len(), 1);
}

#[test]
fn replay_is_explicit_filtering_and_unsubscription_preserve_message_facts() {
    let (_root, db) = database();
    let chat = channel(&db, "channel");
    let first = post(&db, "first", &chat.chat_id, "reader", None, &[]);
    let reply = post(
        &db,
        "reply",
        &chat.chat_id,
        "writer",
        Some(&first.message_id),
        &[],
    );
    post(&db, "unrelated", &chat.chat_id, "writer", None, &[]);
    preferences(
        &db,
        "future",
        &chat.chat_id,
        "reader",
        PreferenceChanges {
            subscribed: Some(true),
            filter: Some(MessageFilter::Replies),
            delivery: Some(DeliveryMode::OnNextTurn),
        },
        None,
    );
    assert!(db
        .chat_notice_page("local", None, 0)
        .unwrap()
        .items
        .is_empty());
    preferences(
        &db,
        "replay",
        &chat.chat_id,
        "reader",
        PreferenceChanges::default(),
        Some(StartAt::After {
            message_id: first.message_id.clone(),
        }),
    );
    let page = db.chat_notice_page("local", None, 0).unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].message, reply);
    assert_eq!(page.items[0].delivery, DeliveryMode::OnNextTurn);
    preferences(
        &db,
        "unsubscribe",
        &chat.chat_id,
        "reader",
        PreferenceChanges {
            subscribed: Some(false),
            ..Default::default()
        },
        None,
    );
    assert!(!db.chat_notice_page("local", None, 0).unwrap().items[0].active);
    assert_eq!(db.chat_participants(&chat.chat_id).unwrap().len(), 2);
    assert_eq!(db.chat_messages(&chat.chat_id, None, 100).unwrap().len(), 3);
}

#[test]
fn client_message_identity_and_receipt_survive_recovery() {
    let (root, db) = database();
    let chat = channel(&db, "client-chat");
    let id = format!("client-{}-request", chat.chat_id);
    let receipt = db.chat_begin_with_id("send", "content", &id).unwrap();
    assert_eq!(receipt.object_id, id);
    db.post_chat_message(
        "send",
        &receipt.object_id,
        &chat.chat_id,
        &Author {
            id: "local-user".into(),
            kind: AuthorKind::User,
            name: None,
        },
        "content",
        &[],
        None,
        &[],
    )
    .unwrap();
    drop(db);

    let db = StationDb::open(root.path(), &root.path().join("workspaces")).unwrap();
    let recovered = db
        .chat_begin_with_id("send", "content", "replacement")
        .unwrap();
    assert_eq!(recovered.object_id, id);
    assert_eq!(recovered.result.unwrap()["message_id"], id);
    assert_eq!(db.chat_visible_message(&id).unwrap().message_id, id);
    assert_eq!(db.chat(&chat.chat_id).unwrap().channel.message_count, 1);
    assert!(db.chat_begin_with_id("send", "changed", &id).is_err());

    // A pre-fix receipt must remain recoverable without rewriting its message.
    let legacy = db.chat_begin("legacy-send", "legacy-content").unwrap();
    assert_eq!(
        db.chat_begin_with_id("legacy-send", "legacy-content", "client-new-id")
            .unwrap()
            .object_id,
        legacy.object_id
    );
}

#[test]
fn message_file_receipt_and_delivery_commit_together() {
    let (root, db) = database();
    let chat = channel(&db, "channel");
    preferences(
        &db,
        "subscribe",
        &chat.chat_id,
        "reader",
        PreferenceChanges {
            subscribed: Some(true),
            ..Default::default()
        },
        None,
    );
    let path = root.path().join("file.txt");
    std::fs::write(&path, "original").unwrap();
    let file = StationDb::prepare_chat_file(root.path(), &path).unwrap();
    let receipt = db.chat_begin("send", "fingerprint").unwrap();
    assert!(db
        .post_chat_message(
            "send",
            &receipt.object_id,
            &chat.chat_id,
            &author("writer"),
            "message",
            &[file],
            Some("missing"),
            &[]
        )
        .is_err());
    assert_eq!(db.chat(&chat.chat_id).unwrap().channel.message_count, 0);
    assert!(db.list_artifacts(None).unwrap().is_empty());
    assert!(db.chat_receipt("send").unwrap().is_none());
    let file = StationDb::prepare_chat_file(root.path(), &path).unwrap();
    let reference = file.reference.clone();
    let sent = db
        .post_chat_message(
            "send",
            &receipt.object_id,
            &chat.chat_id,
            &author("writer"),
            "message",
            &[file],
            None,
            &[],
        )
        .unwrap();
    std::fs::remove_file(path).unwrap();
    assert_eq!(
        db.chat_begin("send", "fingerprint").unwrap().result,
        Some(serde_json::to_value(&sent).unwrap())
    );
    assert!(db.chat_begin("send", "different").is_err());
    assert_eq!(
        db.chat_cancel("send", "fingerprint").unwrap(),
        Some(serde_json::to_value(&sent).unwrap())
    );
    assert_eq!(
        db.conversation_file_bytes(&db.chat(&chat.chat_id).unwrap().session_key, &reference)
            .unwrap(),
        b"original"
    );
    assert_eq!(
        db.chat_notice_page("local", None, 0).unwrap().items.len(),
        1
    );
    db.chat_begin("cancelled", "cancelled").unwrap();
    db.chat_cancel("cancelled", "cancelled").unwrap();
    assert!(db
        .post_chat_message(
            "cancelled",
            "new-id",
            &chat.chat_id,
            &author("writer"),
            "no effect",
            &[],
            None,
            &[]
        )
        .is_err());
}

#[test]
fn peer_feed_multiplexes_agents_and_receiver_cursor_commits_with_mailbox() {
    let (_root, publisher) = database();
    let (root, receiver) = database();
    let chat = channel(&publisher, "channel");
    for agent in ["recipient/a", "recipient/b", "other/c"] {
        preferences(
            &publisher,
            agent,
            &chat.chat_id,
            agent,
            PreferenceChanges {
                subscribed: Some(true),
                ..Default::default()
            },
            None,
        );
    }
    post(&publisher, "one", &chat.chat_id, "writer", None, &[]);
    post(&publisher, "two", &chat.chat_id, "writer", None, &[]);
    receiver.record_chat_source("a", "publisher").unwrap();
    receiver.record_chat_source("b", "publisher").unwrap();
    assert_eq!(receiver.chat_sources().unwrap().len(), 1);
    let page = publisher.chat_notice_page("recipient", None, 0).unwrap();
    assert_eq!(page.items.len(), 4);
    assert!(receiver
        .accept_chat_notices("recipient", "publisher", &page)
        .unwrap());
    assert!(!receiver
        .accept_chat_notices("recipient", "publisher", &page)
        .unwrap());
    let pending = receiver.pending_chat_inputs().unwrap();
    assert_eq!(pending.len(), 2);
    for (id, _, _, _) in pending {
        receiver.finish_chat_input(&id).unwrap();
    }
    assert_eq!(receiver.pending_chat_inputs().unwrap().len(), 2);
    let mut invalid = page.clone();
    invalid.after = page.through + 1;
    invalid.through += 2;
    invalid.items.clear();
    assert!(receiver
        .accept_chat_notices("recipient", "publisher", &invalid)
        .is_err());
    assert_eq!(receiver.chat_sources().unwrap()[0].3, page.through);
    drop(receiver);
    let receiver = StationDb::open(root.path(), &root.path().join("workspaces")).unwrap();
    assert_eq!(receiver.pending_chat_inputs().unwrap().len(), 2);
    assert_eq!(receiver.chat_sources().unwrap()[0].3, page.through);
    assert!(receiver
        .accept_chat_notices(
            "recipient",
            "publisher",
            &publisher.chat_notice_page("other", None, 0).unwrap()
        )
        .is_err());
}

#[test]
fn prepared_files_survive_source_edits_and_are_scoped_to_destination() {
    let (root, db) = database();
    let path = root.path().join("file.txt");
    std::fs::write(&path, "original").unwrap();
    let file = StationDb::prepare_chat_file(root.path(), &path).unwrap();
    let id = file.reference.id.clone();
    db.chat_begin("command", "fingerprint").unwrap();
    db.save_chat_outgoing("command", "peer", &json!({"frozen":true}), &[file])
        .unwrap();
    std::fs::write(path, "changed").unwrap();
    assert_eq!(
        db.prepared_chat_chunk("command", "peer", &id, 0).unwrap()["base64"],
        "b3JpZ2luYWw="
    );
    assert!(db.prepared_chat_chunk("command", "other", &id, 0).is_err());
    drop(db);
    let db = StationDb::open(root.path(), &root.path().join("workspaces")).unwrap();
    assert_eq!(
        db.chat_outgoing("command").unwrap(),
        Some(json!({"frozen":true}))
    );
    db.finish_chat_outgoing("command", &json!({"done":true}))
        .unwrap();
    assert!(db.prepared_chat_chunk("command", "peer", &id, 0).is_err());
}

#[test]
fn receiving_policy_rejects_in_flight_old_pages_and_pending_inputs_after_unsubscribe() {
    let (_root, publisher) = database();
    let (root, receiver) = database();
    let chat = channel(&publisher, "channel");
    preferences(
        &publisher,
        "subscribe",
        &chat.chat_id,
        "receiver/a",
        PreferenceChanges {
            subscribed: Some(true),
            ..Default::default()
        },
        None,
    );
    post(&publisher, "old", &chat.chat_id, "writer", None, &[]);
    let page = publisher.chat_notice_page("receiver", None, 0).unwrap();
    receiver.record_chat_source("a", "publisher").unwrap();
    receiver
        .accept_chat_notices("receiver", "publisher", &page)
        .unwrap();
    receiver
        .record_receiving_policy("a", "publisher", &chat.chat_id, 2, false)
        .unwrap();
    assert!(!receiver
        .accepts_chat_input("a", "publisher", &page.items[0])
        .unwrap());
    // A late reply from an earlier subscribe cannot undo the newer unsubscribe.
    receiver
        .record_receiving_policy("a", "publisher", &chat.chat_id, 1, true)
        .unwrap();
    assert!(!receiver
        .accepts_chat_input("a", "publisher", &page.items[0])
        .unwrap());
    drop(receiver);
    let receiver = StationDb::open(root.path(), &root.path().join("workspaces")).unwrap();
    assert!(!receiver
        .accepts_chat_input("a", "publisher", &page.items[0])
        .unwrap());
    let (_new_root, fresh) = database();
    fresh.record_chat_source("a", "publisher").unwrap();
    fresh
        .record_receiving_policy("a", "publisher", &chat.chat_id, 2, false)
        .unwrap();
    fresh
        .accept_chat_notices("receiver", "publisher", &page)
        .unwrap();
    assert!(fresh.pending_chat_inputs().unwrap().is_empty());
    assert_eq!(fresh.chat_sources().unwrap()[0].3, page.through);
}

#[test]
fn navigation_creator_is_immutable_and_independent_of_authorship_and_receiving() {
    let (_root, db) = database();
    let receipt = db.chat_begin("create-owned", "same").unwrap();
    let created = db
        .create_chat_as(
            "create-owned",
            &receipt.object_id,
            "Owned chat",
            Some(&author("creator")),
        )
        .unwrap();
    assert_eq!(created.creator.as_ref().unwrap().id, "creator");
    assert!(created.last_message_at.is_none());
    assert!(db.chat_participants(&created.chat_id).unwrap().is_empty());
    assert!(db.channel_agent_sessions("creator").unwrap().is_empty());
    assert_eq!(
        db.chat_begin("create-owned", "same")
            .unwrap()
            .result
            .unwrap()["creator"]["id"],
        "creator"
    );
    preferences(
        &db,
        "reader-only",
        &created.chat_id,
        "receiver",
        PreferenceChanges {
            subscribed: Some(true),
            ..Default::default()
        },
        None,
    );
    let sent = post(
        &db,
        "first-message",
        &created.chat_id,
        "other-writer",
        None,
        &[],
    );
    let actual = db.chat(&created.chat_id).unwrap().channel;
    assert_eq!(actual.creator, created.creator);
    assert_eq!(
        actual.last_message_at.as_deref(),
        Some(sent.created_at.as_str())
    );
    assert_eq!(actual.message_count, 1);
    let before = db
        .sync_cursor(
            &db.sync_local_owner().unwrap(),
            zork_client_types::sync::Scope::Catalog {},
        )
        .unwrap();
    for _ in 0..10 {
        assert!(db.chat_navigation().unwrap().contains(&actual));
    }
    assert_eq!(
        before,
        db.sync_cursor(
            &db.sync_local_owner().unwrap(),
            zork_client_types::sync::Scope::Catalog {}
        )
        .unwrap()
    );
    let projected: String = db
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT value FROM sync_entities WHERE kind='resource' AND id=?1",
            [&created.chat_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&projected).unwrap()["creator"]["id"],
        "creator"
    );
}

fn assigned_chat(
    db: &StationDb,
    creator: &str,
    request: &str,
    worker: &str,
) -> (Channel, SessionRow) {
    let (key, runtime, _) = db
        .worker_task_allocation(creator, request, worker, request)
        .unwrap();
    let parts = key.split(':').collect::<Vec<_>>();
    let session = db
        .ensure_session(EnsureSession {
            connection_id: "local_gui",
            platform: "local_gui",
            channel_id: parts[1],
            root_thread_ts: parts[2],
            channel_type: Some("worker_task"),
            initiator_user_id: Some(creator),
            initiator_message_ts: None,
        })
        .unwrap();
    db.set_agent_session(
        &key,
        &runtime,
        &session.workspace_path,
        "fixture",
        "model",
        "off",
    )
    .unwrap();
    db.ensure_product_task(&key).unwrap();
    db.record_visible_message(
        &format!("assignment-{creator}-{request}"),
        &key,
        "local_gui",
        &session.channel_id,
        &session.root_thread_ts,
        "user",
        request,
        None,
    )
    .unwrap();
    (
        db.chat(&key).unwrap().channel,
        db.get_session(&key).unwrap().unwrap(),
    )
}

#[test]
fn work_contexts_and_initial_delivery_are_bound_to_each_chat() {
    let (_root, db) = database();
    let (one, first) = assigned_chat(&db, "creator", "one", "worker");
    let (two, second) = assigned_chat(&db, "creator", "two", "worker");
    assert_ne!(first.id, second.id);
    assert_eq!(one.title, "one");
    assert_eq!(two.title, "two");
    for (chat, session) in [(&one, &first), (&two, &second)] {
        assert_eq!(
            db.session_chat_destination(session.id.as_deref().unwrap())
                .unwrap(),
            Some(("local".into(), chat.chat_id.clone()))
        );
        assert_eq!(
            db.chat_execution("worker", "local", &chat.chat_id)
                .unwrap()
                .unwrap()
                .id,
            session.id
        );
        assert_eq!(chat.creator.as_ref().unwrap().id, "creator");
        assert_eq!(
            db.chat_executor(&chat.chat_id).unwrap().as_deref(),
            Some("worker")
        );
        assert!(db
            .chat_execution("other-worker", "local", &chat.chat_id)
            .unwrap()
            .is_none());
    }
    let sent = post(&db, "follow-up", &one.chat_id, "creator", None, &[]);
    let notices = db.chat_notice_page("local", None, 0).unwrap().items;
    let follow_up = notices
        .iter()
        .find(|n| n.message.message_id == sent.message_id)
        .unwrap();
    assert_eq!(
        follow_up.work.as_ref().unwrap().assignment_id,
        format!("worker-{}", one.chat_id)
    );
    assert!(!follow_up.work.as_ref().unwrap().initial);
    assert!(notices
        .iter()
        .filter(|n| n.message.message_id.starts_with("assignment-"))
        .all(|n| n.work.as_ref().unwrap().initial));
}

#[test]
fn remote_worker_default_publication_uses_its_owner_chat_not_the_local_mirror_or_home() {
    let (_root, db) = database();
    let (mirror, session) = assigned_chat(&db, "creator", "remote", "worker");
    db.insert_node_agent(
        &serde_json::from_value(json!({
            "id":"worker", "name":"Worker", "role":"worker", "profile_id":"fixture",
            "model":"model", "thinking":"off", "instructions":"", "allowed_leaders":["creator"]
        }))
        .unwrap(),
    )
    .unwrap();
    let home = channel(&db, "unrelated-worker-home");
    db.set_agent_home("worker", &home.chat_id).unwrap();
    let owner_chat = ulid::Ulid::new().to_string();
    let assignment = crate::db::mesh::Assignment {
        assignment_id: format!("worker-{owner_chat}"),
        task_id: "owner-task".into(),
        owner_origin: "key:owner".into(),
        executor_origin: "key:executor".into(),
        workspace_id: "workspace".into(),
        goal: "Report back to the owning Chat".into(),
        worker: Some(crate::db::mesh::WorkerTarget {
            leader_id: "creator".into(),
            worker_id: "worker".into(),
        }),
    };
    db.mesh_receive_assignment(&assignment).unwrap();
    db.mesh_bind_executor(&assignment.assignment_id, &session)
        .unwrap();
    assert_ne!(mirror.chat_id, owner_chat);
    assert_eq!(
        db.session_chat_destination(session.id.as_deref().unwrap())
            .unwrap(),
        Some(("key:owner".into(), owner_chat))
    );
    assert!(db
        .session_chat_destination("missing-session")
        .unwrap()
        .is_none());
}

#[test]
fn creator_migration_preserves_unknown_sources_and_remains_quiet_on_reopen() {
    let (root, db) = database();
    let (assigned, _) = assigned_chat(&db, "original-creator", "legacy", "worker");
    let unknown = channel(&db, "unknown");
    post(
        &db,
        "unknown-first-author",
        &unknown.chat_id,
        "not-the-creator",
        None,
        &[],
    );
    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "UPDATE chat_channels SET creator=NULL,last_message_at=NULL",
            [],
        )
        .unwrap();
        conn.execute("DELETE FROM chat_metadata WHERE key='navigation-v1'", [])
            .unwrap();
    }
    drop(db);
    let db = StationDb::open(root.path(), &root.path().join("workspaces")).unwrap();
    assert_eq!(
        db.chat(&assigned.chat_id)
            .unwrap()
            .channel
            .creator
            .unwrap()
            .id,
        "original-creator"
    );
    assert!(db.chat(&unknown.chat_id).unwrap().channel.creator.is_none());
    assert!(db
        .chat(&unknown.chat_id)
        .unwrap()
        .channel
        .last_message_at
        .is_some());
    let owner = db.sync_local_owner().unwrap();
    let before = db
        .sync_cursor(&owner, zork_client_types::sync::Scope::Catalog {})
        .unwrap();
    drop(db);
    let db = StationDb::open(root.path(), &root.path().join("workspaces")).unwrap();
    assert_eq!(
        before,
        db.sync_cursor(&owner, zork_client_types::sync::Scope::Catalog {})
            .unwrap()
    );
}

#[test]
fn archive_survives_restart_and_new_messages_restore_chat() {
    let (root, db) = database();
    let chat = channel(&db, "archive");
    assert!(db.set_chat_archived(&chat.chat_id, true, 0).unwrap());
    assert!(db.chat_navigation().unwrap()[0].archived);
    let cursor = db
        .sync_cursor("owner", zork_client_types::sync::Scope::Catalog {})
        .unwrap();
    assert!(db.set_chat_archived(&chat.chat_id, true, 0).unwrap());
    assert_eq!(
        db.sync_cursor("owner", zork_client_types::sync::Scope::Catalog {})
            .unwrap(),
        cursor
    );
    let value: String = db
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT value FROM sync_entities WHERE kind='resource' AND id=?1",
            [&chat.chat_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&value).unwrap()["archived"],
        true
    );
    drop(db);
    let db = StationDb::open(root.path(), &root.path().join("workspaces")).unwrap();
    assert!(db.chat(&chat.chat_id).unwrap().channel.archived);
    let incoming = post(&db, "arrival", &chat.chat_id, "writer", None, &[]);
    assert!(!db.chat(&chat.chat_id).unwrap().channel.archived);
    // The old archive request cannot hide the newly committed message.
    assert!(!db.set_chat_archived(&chat.chat_id, true, 0).unwrap());
    assert!(db.set_chat_archived(&chat.chat_id, true, 1).unwrap());
    // Receiving the same source record again does not count as a new message.
    let key = db.chat(&chat.chat_id).unwrap().session_key;
    db.record_visible_message(
        &incoming.message_id,
        &key,
        "local_gui",
        &chat.chat_id,
        "",
        "assistant",
        "arrival",
        None,
    )
    .unwrap();
    assert!(db.chat(&chat.chat_id).unwrap().channel.archived);
    // Rebuilding a missing derived fact from the source log is history replay.
    db.conn
        .lock()
        .unwrap()
        .execute(
            "DELETE FROM chat_message_facts WHERE message_id=?1",
            [&incoming.message_id],
        )
        .unwrap();
    drop(db);
    let db = StationDb::open(root.path(), &root.path().join("workspaces")).unwrap();
    assert!(db.chat(&chat.chat_id).unwrap().channel.archived);
    assert_eq!(db.chat(&chat.chat_id).unwrap().channel.message_count, 1);
    assert!(db.set_chat_archived(&chat.chat_id, false, 0).unwrap());
    assert!(!db.chat_navigation().unwrap()[0].archived);
    assert!(!db.set_chat_archived("missing", true, 0).unwrap());
}

#[test]
fn archive_migrates_existing_catalog_without_requiring_new_messages() {
    let (root, db) = database();
    let chat = channel(&db, "legacy-archive");
    {
        let conn = db.conn.lock().unwrap();
        conn.execute_batch("DROP TRIGGER sync_chat_channels_insert;
            DROP TRIGGER sync_chat_channels_update; DROP TRIGGER sync_chat_channels_delete;
            ALTER TABLE chat_channels DROP COLUMN archived;
            CREATE TRIGGER sync_chat_channels_insert AFTER INSERT ON chat_channels BEGIN SELECT 1; END;
            UPDATE sync_entities SET value=json_remove(value,'$.archived') WHERE kind='resource' AND json_extract(value,'$.resource_type')='chat_summary';").unwrap();
    }
    drop(db);
    let db = StationDb::open(root.path(), &root.path().join("workspaces")).unwrap();
    assert!(!db.chat(&chat.chat_id).unwrap().channel.archived);
    assert!(db.set_chat_archived(&chat.chat_id, true, 0).unwrap());
    let value: String = db
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT value FROM sync_entities WHERE kind='resource' AND id=?1",
            [&chat.chat_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&value).unwrap()["archived"],
        true
    );
}

fn quote(text: &str, kind: zork_client_types::chat::QuoteKind) -> MessageQuote {
    MessageQuote {
        text: text.into(),
        kind,
    }
}
fn reply_with_quote(
    db: &StationDb,
    chat: &str,
    id: &str,
    reply_to: Option<&str>,
    quote: Option<&MessageQuote>,
) -> Result<Message> {
    db.post_chat_content_from_client(
        None,
        id,
        chat,
        &author("builder"),
        "已改好，其余不变。",
        &[],
        reply_to,
        quote,
        &[],
        &[],
        None,
        None,
    )
}
fn projected(db: &StationDb, id: &str) -> Value {
    let value: String = db
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT value FROM sync_entities WHERE kind='message' AND id=?1",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    serde_json::from_str(&value).unwrap()
}

#[test]
fn reply_quotes_persist_deliver_project_and_survive_restart_and_rebuild() {
    use zork_client_types::chat::QuoteKind;
    let (root, db) = database();
    let chat = channel(&db, "quoted").chat_id;
    preferences(
        &db,
        "quote-reader",
        &chat,
        "reader",
        PreferenceChanges {
            subscribed: Some(true),
            ..Default::default()
        },
        None,
    );
    let original = post(&db, "original", &chat, "review", None, &[]);
    let summary = quote("密码框缺少显示切换，错误提示对比度不够", QuoteKind::Summary);
    let reply = reply_with_quote(
        &db,
        &chat,
        "reply",
        Some(&original.message_id),
        Some(&summary),
    )
    .unwrap();
    assert_eq!(reply.reply_quote(), Some(summary.clone()));
    assert_eq!(reply.quote_kind, Some(QuoteKind::Summary));
    let excerpt = quote("对比度", QuoteKind::Excerpt);
    let second = reply_with_quote(
        &db,
        &chat,
        "second",
        Some(&original.message_id),
        Some(&excerpt),
    )
    .unwrap();
    assert_eq!(second.quote.as_deref(), Some("对比度"));
    assert_eq!(second.quote_kind, Some(QuoteKind::Excerpt));

    // A quote describes a reply target; without one nothing is committed.
    let count = db.chat(&chat).unwrap().channel.message_count;
    let error = reply_with_quote(&db, &chat, "orphan", None, Some(&excerpt)).unwrap_err();
    assert_eq!(error.to_string(), "quote_requires_reply_to");
    assert_eq!(db.chat(&chat).unwrap().channel.message_count, count);

    // Readers, receivers and the client sync projection all carry the fields.
    assert_eq!(db.chat_message(&chat, "reply").unwrap(), reply);
    let history = db.chat_messages(&chat, None, 10).unwrap();
    assert!(history.iter().any(|(_, m)| m == &reply));
    let notices = db.chat_notice_page("local", None, 0).unwrap();
    let delivered = notices
        .items
        .iter()
        .find(|n| n.message.message_id == "reply")
        .unwrap();
    assert_eq!(delivered.message.reply_quote(), Some(summary.clone()));
    // Remote peers receive the same notice JSON; older ones ignore the fields.
    let wire = serde_json::to_value(delivered).unwrap();
    assert_eq!(wire["message"]["quote_kind"], "summary");
    let decoded: Notice = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(&decoded, delivered);
    let mut legacy = wire;
    legacy["message"].as_object_mut().unwrap().remove("quote");
    legacy["message"]
        .as_object_mut()
        .unwrap()
        .remove("quote_kind");
    assert_eq!(
        serde_json::from_value::<Notice>(legacy)
            .unwrap()
            .message
            .quote,
        None
    );
    let value = projected(&db, "reply");
    assert_eq!(value["quote"], summary.text);
    assert_eq!(value["quote_kind"], "summary");
    assert_eq!(value["reply_to"], original.message_id);
    let plain = projected(&db, &original.message_id);
    assert!(plain.get("quote").is_none() && plain.get("quote_kind").is_none());

    // Restart reads the complete source record.
    drop(db);
    let db = StationDb::open(root.path(), &root.path().join("workspaces")).unwrap();
    assert_eq!(db.chat_message(&chat, "reply").unwrap(), reply);
    assert_eq!(db.chat_message(&chat, "second").unwrap(), second);
    // Losing the rebuildable indexes restores the facts from the source log.
    {
        let conn = db.conn.lock().unwrap();
        conn.execute_batch(
            "DELETE FROM chat_notices; DELETE FROM chat_message_facts; DELETE FROM visible_messages;",
        )
        .unwrap();
    }
    drop(db);
    std::fs::remove_dir_all(root.path().join("cache")).unwrap();
    let db = StationDb::open(root.path(), &root.path().join("workspaces")).unwrap();
    assert_eq!(db.chat_message(&chat, "reply").unwrap(), reply);
    assert_eq!(
        db.chat_message(&chat, &original.message_id).unwrap(),
        original
    );
    let facts: (Option<String>, Option<String>) = db
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT quote,quote_kind FROM chat_message_facts WHERE message_id='reply'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(facts, (Some(summary.text.clone()), Some("summary".into())));
    assert_eq!(projected(&db, "reply")["quote"], summary.text);
}

#[test]
fn facts_from_before_quotes_migrate_without_changing_existing_projections() {
    let (root, db) = database();
    let chat = channel(&db, "legacy-quotes").chat_id;
    let old = post(&db, "old", &chat, "review", None, &[]);
    let before = projected(&db, &old.message_id);
    let revision = |db: &StationDb| -> i64 {
        db.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT revision FROM sync_entities WHERE kind='message' AND id=?1",
                [&old.message_id],
                |r| r.get(0),
            )
            .unwrap()
    };
    let old_revision = revision(&db);
    {
        // The schema and message projection of a Station before reply quotes.
        let conn = db.conn.lock().unwrap();
        conn.execute_batch(
            "DROP TRIGGER sync_visible_messages_insert; DROP TRIGGER sync_visible_messages_update;
             DROP TRIGGER sync_visible_messages_delete;
             ALTER TABLE chat_message_facts DROP COLUMN quote;
             ALTER TABLE chat_message_facts DROP COLUMN quote_kind;
             CREATE TRIGGER sync_visible_messages_insert AFTER INSERT ON visible_messages BEGIN SELECT 'chat_message_facts'; END;",
        )
        .unwrap();
        // Messages without a quote project byte-identically either way.
        let same: bool = conn
            .query_row(
                "SELECT json_patch(json_object('a',1,'b',NULL,'c','x','d',json('[1]')),'{}')
                 = json_object('a',1,'b',NULL,'c','x','d',json('[1]'))",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(same);
    }
    drop(db);
    let db = StationDb::open(root.path(), &root.path().join("workspaces")).unwrap();
    let old_read = db.chat_message(&chat, &old.message_id).unwrap();
    assert_eq!(old_read, old);
    assert_eq!((old_read.quote, old_read.quote_kind), (None, None));
    assert_eq!(projected(&db, &old.message_id), before);
    assert_eq!(revision(&db), old_revision);
    // New replies after the migration carry quotes.
    let reply = reply_with_quote(
        &db,
        &chat,
        "after",
        Some(&old.message_id),
        Some(&quote("摘录", zork_client_types::chat::QuoteKind::Excerpt)),
    )
    .unwrap();
    assert_eq!(projected(&db, &reply.message_id)["quote_kind"], "excerpt");
}

fn summary(db: &StationDb, chat: &str) -> Value {
    let value: String = db
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT value FROM sync_entities WHERE kind='resource' AND id=?1",
            [chat],
            |r| r.get(0),
        )
        .unwrap();
    serde_json::from_str(&value).unwrap()
}

#[test]
fn messages_record_the_model_that_wrote_them_and_chats_list_their_agents() {
    let (root, db) = database();
    let chat = channel(&db, "avatars").chat_id;
    // A Chat without Agent authors keeps its old summary shape.
    let plain = summary(&db, &chat);
    assert!(plain.get("agents").is_none() && plain.get("agent_count").is_none());
    for (id, model) in [("builder", "gpt-5"), ("review", "claude-sonnet-5")] {
        db.insert_node_agent(
            &serde_json::from_value(json!({
                "id": id, "name": id, "avatar": null, "role": "worker",
                "profile_id": "fixture", "model": model, "thinking": "off",
                "instructions": "", "allowed_leaders": []
            }))
            .unwrap(),
        )
        .unwrap();
    }
    let user = Author {
        id: "local-user".into(),
        kind: AuthorKind::User,
        name: None,
    };
    let send = |id: &str, who: &Author| {
        db.post_chat_content(None, id, &chat, who, id, &[], None, &[], &[], None)
            .unwrap()
    };
    let asked = send("ask", &user);
    assert_eq!(asked.author_model, None);
    let first = send("first", &author("builder"));
    assert_eq!(first.author_model.as_deref(), Some("gpt-5"));
    send("second", &author("review"));
    // A remote Agent's model is known to its own Station, not this one.
    let remote = send("remote", &author("key:peer/tester"));
    assert_eq!(remote.author_model, None);
    // Switching the model changes later messages only.
    db.conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE node_agents SET value=json_set(value,'$.model','claude-opus-5') WHERE id='builder'",
            [],
        )
        .unwrap();
    let later = send("later", &author("builder"));
    assert_eq!(later.author_model.as_deref(), Some("claude-opus-5"));
    assert_eq!(
        db.chat_message(&chat, "first").unwrap().author_model.as_deref(),
        Some("gpt-5")
    );
    assert_eq!(projected(&db, "first")["model"], "gpt-5");
    assert!(projected(&db, "ask").get("model").is_none());

    // The Chat summary lists Agents by first appearance with their latest model.
    let value = summary(&db, &chat);
    assert_eq!(value["agent_count"], 3);
    assert_eq!(
        value["agents"],
        json!([
            {"id":"builder","name":"builder","model":"claude-opus-5"},
            {"id":"review","name":"review","model":"claude-sonnet-5"},
            {"id":"key:peer/tester","name":"key:peer/tester","model":null}
        ])
    );
    let channel: Channel = serde_json::from_value(value).unwrap();
    assert_eq!(channel.agents[0].model.as_deref(), Some("claude-opus-5"));
    assert_eq!(channel.agents[2].model, None);

    // Restart and index rebuild keep the recorded models.
    {
        let conn = db.conn.lock().unwrap();
        conn.execute_batch(
            "DELETE FROM chat_notices; DELETE FROM chat_message_facts; DELETE FROM visible_messages;",
        )
        .unwrap();
    }
    drop(db);
    std::fs::remove_dir_all(root.path().join("cache")).unwrap();
    let db = StationDb::open(root.path(), &root.path().join("workspaces")).unwrap();
    assert_eq!(db.chat_message(&chat, "first").unwrap(), first);
    assert_eq!(db.chat_message(&chat, "later").unwrap(), later);
}
