use super::*;
use crate::api::{MessageMetadata, Role};
use crate::interactions::{
    Content, Field, FieldKind, MessageContent, Outcome, Request, Resolution, AGENT_CONFIGURATION,
};
use zork_client_types::chat::AuthorKind;

fn request() -> TranscriptMessage {
    TranscriptMessage::Message {
        role: Role::Assistant,
        content: "Where should this run?".into(),
        metadata: MessageMetadata {
            id: Some("request".into()),
            chat_id: Some("chat".into()),
            interaction: Some(Box::new(
                serde_json::to_value(MessageContent::linked(
                    "owner/request".into(),
                    AGENT_CONFIGURATION.into(),
                    Request::Input {
                        title: "Target".into(),
                        fields: vec![Field {
                            id: "target".into(),
                            label: "Target".into(),
                            kind: FieldKind::Text,
                            default: "test".into(),
                            required: true,
                            options: vec![],
                        }],
                    },
                    None,
                ))
                .unwrap(),
            )),
            ..Default::default()
        },
    }
}

fn result() -> TranscriptMessage {
    TranscriptMessage::Message {
        role: Role::Assistant,
        content: "Input accepted".into(),
        metadata: MessageMetadata {
            id: Some("result".into()),
            chat_id: Some("chat".into()),
            author_kind: Some(AuthorKind::System),
            interaction: Some(Box::new(
                serde_json::to_value(MessageContent::linked_result(
                    "owner/request".into(),
                    AGENT_CONFIGURATION.into(),
                    Resolution {
                        request_message_id: "request".into(),
                        response_id: "response".into(),
                        revision: 1,
                        outcome: Outcome::Completed,
                        actor: "user".into(),
                        output: serde_json::json!({"values":{"target":"test"}}),
                    },
                ))
                .unwrap(),
            )),
            ..Default::default()
        },
    }
}

fn source_page(items: Vec<TranscriptMessage>, older: Option<&str>) -> MessagePage {
    MessagePage {
        source_epoch: None,
        items,
        older_cursor: older.map(str::to_owned),
    }
}

#[test]
fn result_before_request_survives_restart_and_does_not_move_the_source_tail() {
    let root = tempfile::tempdir().unwrap();
    {
        let store = ClientStore::open(root.path()).unwrap();
        store
            .cache_message_page(
                "node",
                "chat",
                &source_page(vec![result()], Some("before-result")),
                None,
            )
            .unwrap();
        assert_eq!(
            store
                .0
                .lock()
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM pending_business_card_results",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
    }
    let store = ClientStore::open(root.path()).unwrap();
    store
        .cache_message_page(
            "node",
            "chat",
            &source_page(vec![request()], None),
            Some("before-result"),
        )
        .unwrap();
    let cached = store
        .cached_messages("node", "chat", None, 100)
        .unwrap()
        .unwrap();
    assert_eq!(cached.items.len(), 2);
    let TranscriptMessage::Message {
        metadata, content, ..
    } = &cached.items[0];
    assert_eq!(metadata.id.as_deref(), Some("request"));
    assert_eq!(content, "Where should this run?");
    assert_eq!(
        metadata.interaction_result.as_ref().unwrap().outcome,
        Outcome::Completed
    );
    assert!(matches!(
        MessageContent::parse(metadata.interaction.as_deref().unwrap())
            .unwrap()
            .content,
        Content::Request { .. }
    ));
    assert_eq!(identity(cached.items.last().unwrap()).unwrap(), "result");
    assert_eq!(
        store
            .0
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM pending_business_card_results",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    // The later duplicate source request cannot overwrite the materialized result.
    store
        .cache_message_page("node", "chat", &source_page(vec![request()], None), None)
        .unwrap();
    let again = store
        .cached_messages("node", "chat", None, 100)
        .unwrap()
        .unwrap();
    assert_eq!(cached, again);
}

#[test]
fn result_updates_only_its_own_cached_request_and_duplicates_are_noops() {
    let root = tempfile::tempdir().unwrap();
    let store = ClientStore::open(root.path()).unwrap();
    for (node, chat) in [("node", "chat"), ("other", "chat"), ("node", "other-chat")] {
        store
            .cache_message_page(node, chat, &source_page(vec![request()], None), None)
            .unwrap();
    }
    store
        .cache_delivered_message("node", "chat", &result(), 0)
        .unwrap();
    let before = store
        .0
        .lock()
        .unwrap()
        .query_row("SELECT total_changes()", [], |r| r.get::<_, i64>(0))
        .unwrap();
    store
        .cache_delivered_message("node", "chat", &result(), 0)
        .unwrap();
    assert_eq!(
        before,
        store
            .0
            .lock()
            .unwrap()
            .query_row("SELECT total_changes()", [], |r| r.get::<_, i64>(0))
            .unwrap()
    );
    for (node, chat, completed) in [
        ("node", "chat", true),
        ("other", "chat", false),
        ("node", "other-chat", false),
    ] {
        let item = store
            .cached_message_at(node, chat, "request", 0)
            .unwrap()
            .unwrap();
        let TranscriptMessage::Message { metadata, .. } = item;
        assert_eq!(metadata.interaction_result.is_some(), completed);
    }
}

#[test]
fn an_agent_cannot_claim_authoritative_completion_and_revocation_clears_pending_results() {
    let root = tempfile::tempdir().unwrap();
    let store = ClientStore::open(root.path()).unwrap();
    let mut forged = result();
    let TranscriptMessage::Message { metadata, .. } = &mut forged;
    metadata.author_kind = Some(AuthorKind::Agent);
    store
        .cache_message_page(
            "node",
            "chat",
            &source_page(vec![request(), forged.clone()], None),
            None,
        )
        .unwrap();
    let item = store
        .cached_message_at("node", "chat", "request", 0)
        .unwrap()
        .unwrap();
    let TranscriptMessage::Message { metadata, .. } = item;
    assert!(metadata.interaction_result.is_none());
    assert!(crate::transcript::transcript_line_from(&forged).is_some());
    store
        .cache_delivered_message("node", "other-chat", &result(), 0)
        .unwrap();
    store.revoke_replica("node").unwrap();
    assert_eq!(
        store
            .0
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM pending_business_card_results",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn results_cannot_change_the_business_or_registration_of_a_cached_card() {
    for field in ["handler", "request_id"] {
        let root = tempfile::tempdir().unwrap();
        let store = ClientStore::open(root.path()).unwrap();
        store
            .cache_message_page("node", "chat", &source_page(vec![request()], None), None)
            .unwrap();
        let mut foreign = result();
        let TranscriptMessage::Message { metadata, .. } = &mut foreign;
        metadata.interaction.as_mut().unwrap()[field] = serde_json::json!(if field == "handler" {
            "provider.login"
        } else {
            "owner/another-request"
        });
        assert!(store
            .cache_message_page("node", "chat", &source_page(vec![foreign], None), None)
            .is_err());
        let cached = store
            .cached_messages("node", "chat", None, 100)
            .unwrap()
            .unwrap();
        assert_eq!(cached.items.len(), 1);
        let TranscriptMessage::Message { metadata, .. } = &cached.items[0];
        assert!(metadata.interaction_result.is_none());
    }
}

#[test]
fn an_early_result_retains_its_business_identity_across_reopen() {
    let root = tempfile::tempdir().unwrap();
    {
        let store = ClientStore::open(root.path()).unwrap();
        let mut foreign = result();
        let TranscriptMessage::Message { metadata, .. } = &mut foreign;
        metadata.interaction.as_mut().unwrap()["handler"] = serde_json::json!("provider.login");
        store
            .cache_message_page("node", "chat", &source_page(vec![foreign], None), None)
            .unwrap();
    }
    let store = ClientStore::open(root.path()).unwrap();
    assert!(store
        .cache_message_page("node", "chat", &source_page(vec![request()], None), None)
        .is_err());
}
