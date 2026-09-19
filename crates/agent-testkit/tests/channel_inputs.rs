use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use zork_agent::session::{
    events::{InputPosition, SessionEvent},
    runner::Configuration,
    service::ServiceOptions,
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
async fn session(world: &TestWorld) -> String {
    world
        .create_session(selection(), None, "/virtual/channel")
        .await
        .unwrap()
}
async fn send(world: &TestWorld, id: &str, source: &str, sequence: u64, wake: bool, text: &str) {
    world
        .service_handle()
        .submit_ordered_input(
            id,
            InputPosition {
                source: source.into(),
                sequence,
            },
            wake,
            text.into(),
        )
        .await
        .unwrap();
}
fn inputs(world: &TestWorld, id: &str) -> usize {
    world
        .events(id)
        .iter()
        .filter(|e| matches!(e.event, SessionEvent::InputAppended { .. }))
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn quiet_inputs_wait_for_a_real_turn_and_dedup_survives_interleaving_and_restart() {
    let mut world = TestWorld::new();
    let id = session(&world).await;
    send(
        &world,
        &id,
        "channel-publisher",
        10,
        false,
        "quiet channel content",
    )
    .await;
    send(
        &world,
        &id,
        "other-publisher",
        1,
        false,
        "other quiet content",
    )
    .await;
    send(
        &world,
        &id,
        "channel-publisher",
        10,
        false,
        "quiet channel content",
    )
    .await;
    assert_eq!(inputs(&world, &id), 2);
    assert!(world.state(&id).await.unwrap().active_turn.is_none());
    assert!(
        tokio::time::timeout(Duration::from_millis(30), world.request())
            .await
            .is_err()
    );
    world.restart().await.unwrap();
    send(
        &world,
        &id,
        "channel-publisher",
        10,
        false,
        "quiet channel content",
    )
    .await;
    assert!(world
        .service_handle()
        .submit_ordered_input(
            &id,
            InputPosition {
                source: "channel-publisher".into(),
                sequence: 10
            },
            false,
            "changed".into()
        )
        .await
        .is_err());
    world.send_mail(&id, "human request").await.unwrap();
    let request = world.request().await;
    let text = serde_json::to_string(&request.transcript).unwrap();
    assert_eq!(text.matches("quiet channel content").count(), 1);
    assert_eq!(text.matches("other quiet content").count(), 1);
    assert!(text.contains("human request"));
    request.respond_text("finished").unwrap();
    world
        .wait_for_state(&id, |s| {
            s.active_turn.is_none() && s.unconsumed_inputs.is_empty()
        })
        .await;
    send(&world, &id, "channel-publisher", 11, true, "new message").await;
    let request = world.request().await;
    send(
        &world,
        &id,
        "channel-publisher",
        10,
        false,
        "quiet channel content",
    )
    .await;
    request.respond_text("finished again").unwrap();
    world
        .wait_for_state(&id, |s| {
            s.active_turn.is_none() && s.unconsumed_inputs.is_empty()
        })
        .await;
    assert_eq!(
        world.state(&id).await.unwrap().input_streams["channel-publisher"].sequence,
        11
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(30), world.request())
            .await
            .is_err()
    );
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn interrupting_an_observed_run_cannot_cancel_its_successor() {
    let mut world = TestWorld::new();
    let id = session(&world).await;
    world.send_mail(&id, "first").await.unwrap();
    let first = world.request().await;
    let old = world.state(&id).await.unwrap().active_turn.unwrap().turn_id;
    first.respond_text("done").unwrap();
    world.wait_for_state(&id, |s| s.active_turn.is_none()).await;
    world.send_mail(&id, "second").await.unwrap();
    let second = world.request().await;
    let current = world.state(&id).await.unwrap().active_turn.unwrap().turn_id;
    assert_ne!(old, current);
    assert!(world
        .service_handle()
        .cancel_observed_turn(&id, old)
        .await
        .is_err());
    assert_eq!(
        world.state(&id).await.unwrap().active_turn.unwrap().turn_id,
        current
    );
    world
        .service_handle()
        .cancel_observed_turn(&id, current.clone())
        .await
        .unwrap();
    world.wait_for_state(&id, |s| s.active_turn.is_none()).await;
    assert!(second.respond_text("too late").is_err());
    assert_eq!(world.events(&id).iter().filter(|e|matches!(&e.event,SessionEvent::TurnCancelRequested{turn_id,..} if turn_id==&current)).count(),1);
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_configuration_is_applied_at_request_boundary_without_resetting_context() {
    let config = Arc::new(Mutex::new(Configuration {
        end_turn_confirmation: None,
        revision: "one".into(),
        selection: selection(),
        system_prompt: Some("First Agent instructions".into()),
    }));
    let source = config.clone();
    let mut options = ServiceOptions::default();
    options.runner.configuration_source =
        Some(Arc::new(move |_| Ok(Some(source.lock().unwrap().clone()))));
    let mut world = TestWorld::with_options(options);
    let id = session(&world).await;
    world.send_mail(&id, "remember this message").await.unwrap();
    let first = world.request().await;
    assert_eq!(
        first.transcript[0].content.as_ref(),
        "First Agent instructions"
    );
    config.lock().unwrap().revision = "two".into();
    config.lock().unwrap().selection.model = "updated-model".into();
    config.lock().unwrap().system_prompt = Some("Updated Agent instructions".into());
    first.respond_text("remembered").unwrap();
    world.wait_for_state(&id, |s| s.active_turn.is_none()).await;
    world.send_mail(&id, "continue").await.unwrap();
    let next = world.request().await;
    assert_eq!(next.selection.model, "updated-model");
    assert_eq!(
        next.transcript[0].content.as_ref(),
        "Updated Agent instructions"
    );
    assert!(next
        .transcript
        .iter()
        .any(|m| m.content.contains("remember this message")));
    assert!(next
        .transcript
        .iter()
        .any(|m| m.content.as_ref() == "remembered"));
    assert_eq!(
        world
            .state(&id)
            .await
            .unwrap()
            .configuration_revision
            .as_deref(),
        Some("two")
    );
    next.respond_text("done").unwrap();
    world.wait_for_state(&id, |s| s.active_turn.is_none()).await;
    // A product prompt/confirmation contract can change without editing the
    // Agent definition. The existing Session must adopt it at the next boundary.
    config.lock().unwrap().system_prompt = Some("New host prompt, same Agent revision".into());
    config.lock().unwrap().end_turn_confirmation = Some("Confirm with end".into());
    world.send_mail(&id, "one more turn").await.unwrap();
    let refreshed = world.request().await;
    assert_eq!(
        refreshed.transcript[0].content.as_ref(),
        "New host prompt, same Agent revision"
    );
    refreshed.respond_text("internal only").unwrap();
    let confirmation = world.request().await;
    assert!(confirmation
        .transcript
        .iter()
        .any(|message| message.content.contains("Confirm with end")));
    confirmation
        .respond_call("end", "end", serde_json::json!({}))
        .unwrap();
    world.wait_for_state(&id, |s| s.active_turn.is_none()).await;
    world.shutdown().await;
}
