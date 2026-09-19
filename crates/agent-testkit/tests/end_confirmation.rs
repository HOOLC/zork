use std::{sync::Arc, time::Duration};

use serde_json::json;
use zork_agent::session::{
    events::{Input, SessionEvent, TurnOutcome},
    runner::Configuration,
    service::ServiceOptions,
    store::SessionStore,
    tools::{ToolContract, ToolVersion},
    wire::SessionSelection,
};
use zork_agent_testkit::TestWorld;

fn selection() -> SessionSelection {
    SessionSelection {
        profile_id: "test-profile".into(),
        model: "test-model".into(),
        thinking: "medium".into(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn newly_consumed_input_gets_its_own_confirmation_attempts_after_recovery() {
    let mut world = world();
    let id = start(&world).await;
    for _ in 0..2 {
        world
            .request()
            .await
            .respond_text("Unconfirmed internal ending")
            .unwrap();
    }
    let interrupted = world.request().await;
    world.shutdown().await;
    world
        .store
        .append_batch(
            &id,
            &[SessionEvent::InputAppended {
                input: Input {
                    input_id: "new-input-during-downtime".into(),
                    request_id: None,
                    position: None,
                    wake: true,
                    content: "A new request after recovery".into(),
                    received_at_ms: 1,
                },
            }],
        )
        .unwrap();
    world.restart().await.unwrap();
    assert!(interrupted.respond_text("stale").is_err());
    let resumed = world.request().await;
    assert!(resumed
        .transcript
        .iter()
        .any(|m| m.content.contains("A new request after recovery")));
    resumed
        .respond_text("First internal response to the new request")
        .unwrap();
    let confirmation = world.request().await;
    assert!(world.state(&id).await.unwrap().last_turn_outcome.is_none());
    confirmation
        .respond_call("end-new-work", "end", json!({}))
        .unwrap();
    world
        .wait_for_state(&id, |s| s.last_turn_outcome == Some(TurnOutcome::Finished))
        .await;
    world.shutdown().await;
}

fn world() -> TestWorld {
    let mut options = ServiceOptions::default();
    options.runner.provider_retry_base = Duration::ZERO;
    options.runner.provider_retry_max = Duration::ZERO;
    options.runner.configuration_source = Some(Arc::new(|_| {
        Ok(Some(Configuration {
            revision: "chat-contract".into(),
            selection: selection(),
            system_prompt: None,
            end_turn_confirmation: Some(
                "Continue unfinished work, publish intended replies with chat.post_message, or confirm no further publication with end.".into(),
            ),
        }))
    }));
    TestWorld::with_options(options)
}

async fn start(world: &TestWorld) -> String {
    let id = world
        .create_session(selection(), None, "/virtual/end-confirmation")
        .await
        .unwrap();
    world
        .send_mail(&id, "finish the work and report")
        .await
        .unwrap();
    id
}

fn publication() -> ToolContract {
    ToolContract {
        name: "chat.post_message".into(),
        version: ToolVersion::new("test-1").unwrap(),
        initial_description: "Deliberately publish a reply".into(),
        detailed_description: "Deliberately publish a reply".into(),
        input_schema: json!({"type":"object","properties":{"text":{"type":"string"}}}),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn internal_result_is_not_published_and_end_explicitly_confirms_silence() {
    let mut world = world();
    let mut posts = world.install_tool(publication()).unwrap();
    let id = start(&world).await;
    world
        .request()
        .await
        .respond_text("PRIVATE result: work is done")
        .unwrap();
    let confirmation = world.request().await;
    assert!(confirmation
        .transcript
        .iter()
        .any(|m| m.content.contains("[runtime.end_confirmation]")));
    assert!(!world
        .events(&id)
        .iter()
        .any(|e| matches!(e.event, SessionEvent::TurnFinished { .. })));
    assert!(
        tokio::time::timeout(Duration::from_millis(30), posts.request())
            .await
            .is_err()
    );
    confirmation
        .respond_call("explicit-silence", "end", json!({}))
        .unwrap();
    world
        .wait_for_state(&id, |s| s.last_turn_outcome == Some(TurnOutcome::Finished))
        .await;
    world.restart().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(30), world.request())
            .await
            .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(30), posts.request())
            .await
            .is_err()
    );
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn promise_to_continue_resumes_tools_and_later_input_before_explicit_end() {
    let mut world = world();
    let mut posts = world.install_tool(publication()).unwrap();
    let id = start(&world).await;
    world
        .request()
        .await
        .respond_text("I will continue implementing now.")
        .unwrap();
    let continuation = world.request().await;
    world
        .send_mail(&id, "Also include the verification result")
        .await
        .unwrap();
    continuation
        .respond_call(
            "work",
            "file.write",
            json!({"path":"result.txt","content":"verified"}),
        )
        .unwrap();
    let next = world.request().await;
    assert!(next
        .transcript
        .iter()
        .any(|m| m.content.contains("Also include the verification result")));
    next.respond_call(
        "publish",
        "chat.post_message",
        json!({"text":"Implemented and verified"}),
    )
    .unwrap();
    let post = posts.request().await;
    assert_eq!(post.arguments["text"], "Implemented and verified");
    post.succeed(json!({"status":"committed","message_id":"reply-1"}))
        .unwrap();
    let after = world.request().await;
    assert!(after
        .transcript
        .iter()
        .any(|m| m.content.contains("reply-1")));
    after.respond_call("finish", "end", json!({})).unwrap();
    world
        .wait_for_state(&id, |s| s.last_turn_outcome == Some(TurnOutcome::Finished))
        .await;
    assert_eq!(
        world
            .events(&id)
            .iter()
            .filter(|e| matches!(e.event, SessionEvent::TurnStarted { .. }))
            .count(),
        1
    );
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn confirmation_survives_restart_and_a_failed_publication_is_not_retried_by_runtime() {
    let mut world = world();
    let mut posts = world.install_tool(publication()).unwrap();
    let id = start(&world).await;
    world.request().await.respond_text("Result ready").unwrap();
    let interrupted = world.request().await;
    world.restart().await.unwrap();
    assert!(interrupted.respond_text("stale response").is_err());
    let resumed = world.request().await;
    assert!(resumed
        .transcript
        .iter()
        .any(|m| m.content.contains("Result ready")));
    assert!(resumed
        .transcript
        .iter()
        .any(|m| m.content.contains("[runtime.end_confirmation]")));
    resumed
        .respond_call(
            "publish",
            "chat.post_message",
            json!({"text":"Selected public result"}),
        )
        .unwrap();
    posts
        .request()
        .await
        .fail("Publication outcome is unknown")
        .unwrap();
    world
        .request()
        .await
        .respond_text("The send outcome is unknown")
        .unwrap();
    let confirm = world.request().await;
    assert!(confirm
        .transcript
        .iter()
        .any(|m| m.content.contains("Publication outcome is unknown")));
    assert!(
        tokio::time::timeout(Duration::from_millis(30), posts.request())
            .await
            .is_err()
    );
    confirm
        .respond_call("do-not-resend", "end", json!({}))
        .unwrap();
    world
        .wait_for_state(&id, |s| s.last_turn_outcome == Some(TurnOutcome::Finished))
        .await;
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn ignored_confirmation_is_bounded_visible_after_restart_and_recoverable_by_input() {
    let mut world = world();
    let id = start(&world).await;
    for text in ["I will continue", "All done", ""] {
        world.request().await.respond_text(text).unwrap();
    }
    let failed = world
        .wait_for_state(&id, |s| s.last_turn_outcome == Some(TurnOutcome::Failed))
        .await;
    assert!(failed
        .overview()
        .execution
        .failure
        .unwrap()
        .contains("internal assistant text"));
    assert!(!world
        .events(&id)
        .iter()
        .any(|e| matches!(e.event, SessionEvent::StepFailed { .. })));
    assert!(world.events(&id).iter().any(|e| matches!(&e.event, SessionEvent::TurnFinished { outcome: TurnOutcome::Failed, reason: Some(reason), .. } if reason.contains("internal assistant text"))));
    world.restart().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(30), world.request())
            .await
            .is_err()
    );
    assert_eq!(
        world.state(&id).await.unwrap().last_turn_failure,
        failed.last_turn_failure
    );
    world.send_mail(&id, "Resume and confirm").await.unwrap();
    world
        .request()
        .await
        .respond_call("end-after-recovery", "end", json!({}))
        .unwrap();
    let done = world
        .wait_for_state(&id, |s| s.last_turn_outcome == Some(TurnOutcome::Finished))
        .await;
    assert!(done.last_turn_failure.is_none());
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn deferred_tool_result_wakes_an_ended_turn_and_requires_its_own_confirmation() {
    let mut world = world();
    let id = start(&world).await;
    world
        .request()
        .await
        .respond_calls([
            ("long-job", "shell.run", json!({"command":"long job"})),
            ("short-wait", "wait", json!({"seconds":1})),
        ])
        .unwrap();
    let process = world.process_request().await;
    world
        .wait_for_state(&id, |s| s.wait_deadline.is_some())
        .await;
    world.clock.advance(Duration::from_secs(1));
    let after_wait = world.request().await;
    after_wait
        .respond_text("Leave this running; no message needed yet")
        .unwrap();
    let disclosure = world.request().await;
    assert!(disclosure
        .transcript
        .iter()
        .any(|m| m.content.contains("unfinished") && m.content.contains("shell.run")));
    disclosure
        .respond_call("yield", "end", json!({"acknowledge_outstanding":true}))
        .unwrap();
    world
        .wait_for_state(&id, |s| s.last_turn_outcome == Some(TurnOutcome::Finished))
        .await;
    process.succeed();
    world
        .request()
        .await
        .respond_text("Background result is ready")
        .unwrap();
    let confirm = world.request().await;
    assert!(confirm
        .transcript
        .iter()
        .any(|m| m.content.contains("[runtime.end_confirmation]")));
    confirm
        .respond_call("background-finished", "end", json!({}))
        .unwrap();
    world
        .wait_for_state(&id, |s| s.last_turn_outcome == Some(TurnOutcome::Finished))
        .await;
    assert_eq!(
        world
            .events(&id)
            .iter()
            .filter(|e| matches!(e.event, SessionEvent::TurnStarted { .. }))
            .count(),
        2
    );
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn text_while_tools_are_pending_is_not_mistaken_for_an_unconfirmed_ending() {
    let mut world = world();
    let id = start(&world).await;
    world
        .request()
        .await
        .respond_calls([
            ("long-job", "shell.run", json!({"command":"long job"})),
            ("short-wait", "wait", json!({"seconds":1})),
        ])
        .unwrap();
    let process = world.process_request().await;
    world
        .wait_for_state(&id, |s| s.wait_deadline.is_some())
        .await;
    world.clock.advance(Duration::from_secs(1));
    for _ in 0..3 {
        world
            .request()
            .await
            .respond_text("Still observing the running tool")
            .unwrap();
    }
    let next = world.request().await;
    assert!(!next
        .transcript
        .iter()
        .any(|m| m.content.contains("[runtime.end_confirmation]")));
    assert!(world.state(&id).await.unwrap().last_turn_outcome.is_none());
    process.succeed();
    world
        .wait_for_state(&id, |s| {
            s.pending_tools.values().all(|tool| tool.result.is_some())
        })
        .await;
    next.respond_text("The work is now done").unwrap();
    let confirmation = world.request().await;
    assert!(confirmation
        .transcript
        .iter()
        .any(|m| m.content.contains("[runtime.end_confirmation]")));
    confirmation
        .respond_call("finish", "end", json!({}))
        .unwrap();
    world
        .wait_for_state(&id, |s| s.last_turn_outcome == Some(TurnOutcome::Finished))
        .await;
    world.shutdown().await;
}
