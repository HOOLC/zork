use super::*;
use zork_client_types::interaction::{Content, Field, Outcome};

fn finish_cleanup(db: &StationDb) {
    for (entry, reason) in db.pending_registration_cleanup().unwrap() {
        match entry.handler.as_str() {
            zork_client_types::interaction::AGENT_CONFIGURATION => db
                .cleanup_configuration_review(&entry.request_id, reason)
                .unwrap(),
            zork_client_types::interaction::PROVIDER_LOGIN => db
                .cleanup_provider_login(&entry.request_id, reason)
                .unwrap(),
            _ => panic!("unregistered test business"),
        }
        db.acknowledge_registration_cleanup(&entry.request_id)
            .unwrap();
    }
}

fn setup() -> (tempfile::TempDir, StationDb, Channel, Subject) {
    let root = tempfile::tempdir().unwrap();
    let db = StationDb::open(root.path(), &root.path().join("workspaces")).unwrap();
    let receipt = db.chat_begin("chat", "chat").unwrap();
    let chat = db.create_chat("chat", &receipt.object_id, "Work").unwrap();
    let owner = Subject {
        origin: "local".into(),
        agent: "leader".into(),
        session: "session".into(),
    };
    (root, db, chat, owner)
}
fn create(db: &StationDb, owner: &Subject, invocation: &str) -> CardRecord {
    db.create_configuration_review(
        "node",
        owner,
        invocation,
        &Request::Input {
            title: "Business parameters".into(),
            fields: vec![Field {
                id: "name".into(),
                label: "Name".into(),
                required: true,
                default: "Proposed".into(),
                ..Default::default()
            }],
        },
    )
    .unwrap()
}
fn response(id: &str, value: &str) -> Response {
    Response {
        response_id: id.into(),
        accept: true,
        values: [("name".into(), value.into())].into(),
    }
}
fn publish(db: &StationDb, chat: &str, request: &CardRecord, actor: &str) -> Message {
    let command = ulid::Ulid::new().to_string();
    let receipt = db.chat_begin(&command, &command).unwrap();
    db.post_chat_content(
        Some(&command),
        &receipt.object_id,
        chat,
        &Author {
            id: actor.into(),
            kind: AuthorKind::Agent,
            name: None,
        },
        "Review",
        &[],
        None,
        &[],
        &[],
        Some(
            &serde_json::to_value(MessageContent::linked(
                request.request_id.clone(),
                request.handler.clone(),
                request.request.clone(),
                None,
            ))
            .unwrap(),
        ),
    )
    .unwrap()
}
fn accept(db: &StationDb, request: &CardRecord) -> SubmittedResponse {
    db.submit_configuration_response(
        &request.request_id,
        &response("accepted", "User value"),
        "user",
    )
    .unwrap();
    let submitted = db
        .business_card(&request.request_id)
        .unwrap()
        .submission
        .unwrap();
    db.acknowledge_configuration_response(
        &request.request_id,
        &submitted,
        json!({"values":submitted.response.values}),
    )
    .unwrap();
    submitted
}
#[test]
fn submitting_input_cannot_finish_the_business_or_mutate_the_source_message() {
    let (_root, db, chat, owner) = setup();
    let request = create(&db, &owner, "invocation");
    assert!(db.next_user_notice(&owner).unwrap().is_some());
    assert_eq!(
        create(&db, &owner, "invocation").request_id,
        request.request_id
    );
    let card = publish(&db, &chat.chat_id, &request, &owner.agent);
    let pending = db
        .submit_configuration_response(
            &request.request_id,
            &response("input", "User value"),
            "user",
        )
        .unwrap();
    assert!(pending.result.is_none());
    assert!(pending.submission.is_some());
    assert!(db
        .interaction_result(&chat.chat_id, &card.message_id)
        .unwrap()
        .is_none());
    let input = pending.submission.unwrap();
    db.acknowledge_configuration_response(
        &request.request_id,
        &input,
        json!({"values":input.response.values}),
    )
    .unwrap();
    assert_eq!(
        db.business_card(&request.request_id)
            .unwrap()
            .result
            .unwrap()
            .outcome,
        Outcome::Pending
    );
    db.finish_configuration_review(
        &request.request_id,
        "input",
        Outcome::Completed,
        json!({"business_result":"done"}),
        "business",
    )
    .unwrap();
    assert_eq!(
        db.chat_message(&chat.chat_id, &card.message_id).unwrap(),
        card
    );
    assert_eq!(db.chat(&chat.chat_id).unwrap().channel.message_count, 3);
}
#[test]
fn rejected_business_input_can_be_edited_in_the_same_request() {
    let (_root, db, _chat, owner) = setup();
    let request = create(&db, &owner, "invocation");
    assert!(db
        .submit_configuration_response(&request.request_id, &response("empty", ""), "user")
        .is_err());
    db.submit_configuration_response(
        &request.request_id,
        &response("first", "Unavailable name"),
        "user",
    )
    .unwrap();
    db.reject_configuration_response(&request.request_id, "first", "name already exists")
        .unwrap();
    assert_eq!(
        db.configuration_response_state(&request.request_id, "first")
            .unwrap()
            .unwrap()
            .0,
        "rejected"
    );
    assert!(db
        .business_card(&request.request_id)
        .unwrap()
        .result
        .is_none());
    db.submit_configuration_response(
        &request.request_id,
        &response("second", "Available name"),
        "user",
    )
    .unwrap();
    assert_eq!(
        db.business_card(&request.request_id)
            .unwrap()
            .submission
            .unwrap()
            .response
            .values["name"],
        "Available name"
    );
    assert!(db
        .submit_configuration_response(
            &request.request_id,
            &response("second", "Changed retry"),
            "user"
        )
        .is_err());
}
#[test]
fn cancellation_wins_over_late_responses_and_late_cards() {
    let (_root, db, chat, owner) = setup();
    let request = create(&db, &owner, "invocation");
    db.revoke_interaction_registrations(&owner, "invocation")
        .unwrap();
    finish_cleanup(&db);
    let late = db
        .submit_configuration_response(&request.request_id, &response("late", "x"), "user")
        .unwrap();
    assert_eq!(late.result.unwrap().outcome, Outcome::Cancelled);
    db.finish_configuration_review(
        &request.request_id,
        "late",
        Outcome::Completed,
        json!({}),
        "business",
    )
    .unwrap();
    let card = publish(&db, &chat.chat_id, &request, &owner.agent);
    assert_eq!(
        MessageContent::parse(card.interaction.as_ref().unwrap())
            .unwrap()
            .snapshot
            .unwrap()
            .outcome,
        Outcome::Cancelled
    );
    assert!(crate::db::interaction_registry::register(
        &db.conn.lock().unwrap(),
        "node",
        &owner,
        "invocation",
        "new",
        "another-business"
    )
    .is_err());
}
#[test]
fn restart_distinguishes_unanswered_waits_from_accepted_business_operations() {
    let (root, db, _chat, owner) = setup();
    let waiting = create(&db, &owner, "waiting");
    let accepted = create(&db, &owner, "accepted");
    accept(&db, &accepted);
    drop(db);
    let db = StationDb::open(root.path(), &root.path().join("workspaces")).unwrap();
    db.recover_interaction_registrations().unwrap();
    finish_cleanup(&db);
    assert_eq!(
        db.business_card(&waiting.request_id)
            .unwrap()
            .result
            .unwrap()
            .outcome,
        Outcome::Failed
    );
    let result = db
        .business_card(&accepted.request_id)
        .unwrap()
        .result
        .unwrap();
    assert_eq!(result.outcome, Outcome::Unknown);
    assert_eq!(result.output["operation_replayed"], false);
}
#[test]
fn source_cancellation_is_durable_before_remote_request_registration() {
    let (root, db, _chat, owner) = setup();
    db.begin_user_call(&owner, "invocation", "local").unwrap();
    let mut remote = owner.clone();
    remote.origin = "node".into();
    db.begin_user_call(&remote, "invocation", "remote").unwrap();
    let calls = db.cancel_user_calls(&owner, "invocation").unwrap();
    assert_eq!(calls.len(), 2);
    drop(db);
    let db = StationDb::open(root.path(), &root.path().join("workspaces")).unwrap();
    assert_eq!(db.pending_user_call_cancellations().unwrap().len(), 2);
    assert!(db.begin_user_call(&remote, "invocation", "remote").is_err());
    for call in calls {
        db.acknowledge_user_call_cancellation(&call).unwrap();
    }
    assert!(db.pending_user_call_cancellations().unwrap().is_empty());
}
#[test]
fn foreign_publication_recovers_a_phase_before_the_card_and_rejects_execution() {
    let (_root, authority, _chat, owner) = setup();
    let (_other, destination, chat, _) = setup();
    let request = create(&authority, &owner, "invocation");
    let grant = authority
        .grant_user_publication(&request.request_id, &owner, "destination", &chat.chat_id)
        .unwrap();
    let command = destination.chat_begin("foreign", "foreign").unwrap();
    let snapshot = authority
        .redeem_user_publication(&grant, &chat.chat_id, &command.object_id, "node")
        .unwrap();
    accept(&authority, &request);
    destination
        .import_user_publication(
            &grant,
            &chat.chat_id,
            &command.object_id,
            &snapshot,
            "node/leader",
        )
        .unwrap();
    let phase = authority.pending_user_publications().unwrap().remove(0);
    destination.receive_user_publication(&phase).unwrap();
    let card = destination
        .post_chat_content(
            Some("foreign"),
            &command.object_id,
            &chat.chat_id,
            &Author {
                id: "node/leader".into(),
                kind: AuthorKind::Agent,
                name: None,
            },
            "Review",
            &[],
            None,
            &[],
            &[],
            Some(
                &serde_json::to_value(MessageContent::linked(
                    request.request_id.clone(),
                    request.handler.clone(),
                    request.request.clone(),
                    None,
                ))
                .unwrap(),
            ),
        )
        .unwrap();
    assert_eq!(
        MessageContent::parse(card.interaction.as_ref().unwrap())
            .unwrap()
            .snapshot
            .unwrap()
            .outcome,
        Outcome::Pending
    );
    assert!(destination
        .submit_configuration_response(&request.request_id, &response("forged", "x"), "user")
        .is_err());
    authority.acknowledge_user_publication(&phase).unwrap();
    authority
        .finish_configuration_review(
            &request.request_id,
            "accepted",
            Outcome::Completed,
            json!({"done":true}),
            "business",
        )
        .unwrap();
    let final_phase = authority.pending_user_publications().unwrap().remove(0);
    destination.receive_user_publication(&final_phase).unwrap();
    destination.receive_user_publication(&final_phase).unwrap();
    let result = destination
        .interaction_result(&chat.chat_id, &card.message_id)
        .unwrap()
        .unwrap();
    let Content::Result { result } = MessageContent::parse(result.interaction.as_ref().unwrap())
        .unwrap()
        .content
    else {
        panic!("result")
    };
    assert_eq!(result.outcome, Outcome::Completed);
}
#[test]
fn agent_effect_receipt_and_final_card_commit_atomically() {
    let (_root, db, chat, owner) = setup();
    let request = create(&db, &owner, "invocation");
    publish(&db, &chat.chat_id, &request, &owner.agent);
    accept(&db, &request);
    let receipt = db.chat_begin("business", "business").unwrap();
    let agent:crate::db::agents::NodeAgent=serde_json::from_value(json!({"id":receipt.object_id,"name":"Created","role":"worker","profile_id":"fixture","model":"model","thinking":"off","instructions":"","allowed_leaders":[]})).unwrap();
    db.conn.lock().unwrap().execute_batch("CREATE TRIGGER reject_final BEFORE INSERT ON tool_interaction_messages WHEN json_extract(NEW.value,'$.result.outcome')='completed' BEGIN SELECT RAISE(ABORT,'injected final write failure'); END;").unwrap();
    assert!(db
        .save_channel_agent("business", &agent, None, Some(&request.request_id))
        .is_err());
    assert!(db.node_agent(&agent.id).unwrap().is_none());
    assert_eq!(
        db.business_card(&request.request_id)
            .unwrap()
            .result
            .unwrap()
            .outcome,
        Outcome::Pending
    );
    db.conn
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_final")
        .unwrap();
    db.save_channel_agent("business", &agent, None, Some(&request.request_id))
        .unwrap();
    assert!(db.node_agent(&agent.id).unwrap().is_some());
    assert_eq!(
        db.business_card(&request.request_id)
            .unwrap()
            .result
            .unwrap()
            .outcome,
        Outcome::Completed
    );
}

#[test]
fn login_has_its_own_card_and_cannot_accept_configuration_responses() {
    let (_root, db, _chat, owner) = setup();
    let request = db
        .create_provider_login_card("node", &owner, "login", "Sign in")
        .unwrap();
    db.provider_login_progress(&request.request_id).unwrap();
    assert!(db
        .submit_configuration_response(
            &request.request_id,
            &Response {
                response_id: "extra-approval".into(),
                accept: true,
                values: Default::default()
            },
            "user"
        )
        .is_err());
    assert!(db
        .business_card(&request.request_id)
        .unwrap()
        .submission
        .is_none());
}

#[test]
fn registration_revocation_does_not_interpret_business_state() {
    let (_root, db, _, owner) = setup();
    let request = create(&db, &owner, "invocation");
    db.revoke_interaction_registrations(&owner, "invocation")
        .unwrap();
    assert!(
        !db.interaction_registration(&request.request_id)
            .unwrap()
            .active
    );
    assert!(db
        .business_card(&request.request_id)
        .unwrap()
        .result
        .is_none());
    db.cleanup_configuration_review(
        &request.request_id,
        crate::db::interaction_registry::Cleanup::Cancelled,
    )
    .unwrap();
    assert_eq!(
        db.business_card(&request.request_id)
            .unwrap()
            .result
            .unwrap()
            .outcome,
        Outcome::Cancelled
    );
}

#[test]
fn configuration_competing_input_gets_an_explicit_business_rejection() {
    let (_root, db, _, owner) = setup();
    let request = create(&db, &owner, "invocation");
    db.submit_configuration_response(&request.request_id, &response("first", "first"), "a")
        .unwrap();
    let rejected = db
        .submit_configuration_response(&request.request_id, &response("second", "second"), "b")
        .unwrap_err();
    assert_eq!(
        rejected.to_string(),
        "agent_configuration_submission_pending"
    );
    db.reject_configuration_response(&request.request_id, "first", "invalid business value")
        .unwrap();
    db.submit_configuration_response(&request.request_id, &response("second", "second"), "b")
        .unwrap();
    assert_eq!(
        db.business_card(&request.request_id)
            .unwrap()
            .submission
            .unwrap()
            .response
            .values["name"],
        "second"
    );
}

#[test]
fn revocation_before_business_commit_prevents_agent_effects() {
    let (_root, db, _, owner) = setup();
    let request = create(&db, &owner, "invocation");
    accept(&db, &request);
    let receipt = db.chat_begin("business", "business").unwrap();
    let agent:crate::db::agents::NodeAgent=serde_json::from_value(json!({"id":receipt.object_id,"name":"Cancelled","role":"worker","profile_id":"fixture","model":"model","thinking":"off","instructions":"","allowed_leaders":[]})).unwrap();
    db.revoke_interaction_registrations(&owner, "invocation")
        .unwrap();
    assert!(db
        .save_channel_agent("business", &agent, None, Some(&request.request_id))
        .is_err());
    assert!(db.node_agent(&agent.id).unwrap().is_none());
    finish_cleanup(&db);
    assert_eq!(
        db.business_card(&request.request_id)
            .unwrap()
            .result
            .unwrap()
            .outcome,
        Outcome::Cancelled
    );
}

#[test]
fn restart_during_unfinished_cleanup_preserves_uncertain_login_effects() {
    let (root, db, _, owner) = setup();
    let request = db
        .create_provider_login_card("node", &owner, "login", "Connect")
        .unwrap();
    db.provider_login_progress(&request.request_id).unwrap();
    db.revoke_interaction_registrations(&owner, "login")
        .unwrap();
    drop(db);
    let db = StationDb::open(root.path(), &root.path().join("workspaces")).unwrap();
    db.recover_interaction_registrations().unwrap();
    db.revoke_interaction_registrations(&owner, "login")
        .unwrap();
    finish_cleanup(&db);
    assert_eq!(
        db.business_card(&request.request_id)
            .unwrap()
            .result
            .unwrap()
            .outcome,
        Outcome::Unknown
    );
}

#[test]
fn finishing_a_call_allows_receipt_recovery_without_marking_it_cancelled() {
    let (_root, db, _, owner) = setup();
    db.begin_user_call(&owner, "done", "remote").unwrap();
    db.finish_user_calls(&owner, "done").unwrap();
    db.begin_user_call(&owner, "done", "remote").unwrap();
    let cleanup = db.pending_user_call_cancellations().unwrap();
    assert_eq!(cleanup.len(), 1);
    db.acknowledge_user_call_cancellation(&cleanup[0]).unwrap();
    db.begin_user_call(&owner, "done", "remote").unwrap();
    db.cancel_user_calls(&owner, "done").unwrap();
    assert!(db.begin_user_call(&owner, "done", "remote").is_err());
}
