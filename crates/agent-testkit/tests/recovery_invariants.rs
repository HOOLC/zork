use serde_json::json;
use zork_agent::session::{
    events::{SessionEvent, ToolOutcome, TurnOutcome},
    projection::provider_transcript,
    tools::{ToolContract, ToolVersion},
    wire::SessionSelection,
};
use zork_agent_testkit::TestWorld;
async fn session(world: &TestWorld) -> String {
    world
        .create_session(
            SessionSelection {
                profile_id: "test-profile".into(),
                model: "test-model".into(),
                thinking: "medium".into(),
            },
            None,
            "/virtual/audit",
        )
        .await
        .unwrap()
}
fn tool(name: &str) -> ToolContract {
    ToolContract {
        name: name.into(),
        version: ToolVersion::new("audit-1").unwrap(),
        initial_description: name.into(),
        detailed_description: name.into(),
        input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_rejects_invalid_target_type() {
    let mut w = TestWorld::new();
    let id = session(&w).await;
    w.send_mail(&id, "cancel").await.unwrap();
    w.request()
        .await
        .respond_call("cancel-call", "tool.cancel", json!({"invocation_id":42}))
        .unwrap();
    let next = w.request().await;
    let result = w
        .events(&id)
        .into_iter()
        .find_map(|e| match e.event {
            SessionEvent::ToolResult { result } if result.tool == "tool.cancel" => Some(result),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        result.outcome,
        ToolOutcome::Failed,
        "invalid target must not become a successful cancel"
    );
    drop(next);
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn repeated_wire_call_id_does_not_rewrite_a_previous_request() {
    let mut w = TestWorld::new();
    let mut t = w.install_tool(tool("test.echo")).unwrap();
    let id = session(&w).await;
    w.send_mail(&id, "start").await.unwrap();
    w.request()
        .await
        .respond_call("reused-call", "test.echo", json!({}))
        .unwrap();
    t.request()
        .await
        .succeed(json!({"message":"FIRST"}))
        .unwrap();
    let next = w.request().await;
    let prefix = next.transcript.clone();
    next.respond_call("reused-call", "test.echo", json!({}))
        .unwrap();
    t.request()
        .await
        .succeed(json!({"message":"SECOND"}))
        .unwrap();
    let last = w.request().await;
    assert!(
        &last.transcript[..prefix.len()] == prefix.as_slice(),
        "second response with reused call ID rewrote the first result"
    );
    drop(last);
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn completed_end_during_cancel_does_not_rewrite_sent_prefix() {
    let mut w = TestWorld::new();
    // Hold this invocation explicitly; global tool capacity is not a scheduling
    // contract and must not be used to manufacture a pending built-in call.
    let mut end = w.install_tool(tool("end")).unwrap();
    let id = session(&w).await;
    w.send_mail(&id, "start").await.unwrap();
    w.request()
        .await
        .respond_call("pending-end", "end", json!({}))
        .unwrap();
    let pending = end.request().await;
    w.send_mail(&id, "interrupt the pending end").await.unwrap();
    let in_flight = w.request().await;
    let prefix = in_flight.transcript.clone();
    assert!(prefix
        .iter()
        .any(|m| m.tool_call_id.as_deref() == Some("pending-end")
            && m.content.contains("still unfinished")));
    pending.succeed(json!({"ended":true})).unwrap();
    w.wait_for_state(&id, |s| {
        s.pending_tools
            .values()
            .any(|p| p.invocation.tool == "end" && p.result.is_some())
    })
    .await;
    w.cancel(&id).await.unwrap();
    w.wait_for_state(&id, |s| s.last_turn_outcome == Some(TurnOutcome::Cancelled))
        .await;
    let after = provider_transcript(&w.state(&id).await.unwrap());
    assert!(
        &after[..prefix.len()] == prefix.as_slice(),
        "TurnFinished inserted a late end notification into already sent history"
    );
    drop(in_flight);
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_turn_releases_the_idle_runner() {
    let mut w = TestWorld::new();
    let id = session(&w).await;
    w.send_mail(&id, "start").await.unwrap();
    w.request()
        .await
        .fail_provider("audit", false, "permanent rejection")
        .unwrap();
    w.wait_for_state(&id, |s| s.last_turn_outcome == Some(TurnOutcome::Failed))
        .await;
    w.wait_for_slot(&id, zork_agent::session::supervisor::PublicSlotStatus::Idle)
        .await;
    w.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_persistence_failure_cancels_the_previous_model_request() {
    let mut w = TestWorld::new();
    let id = session(&w).await;
    w.send_mail(&id, "start").await.unwrap();
    let original = w.request().await;
    let pause = w.store.pause_appends();
    let service = w.service_handle();
    let target = id.clone();
    let submit =
        tokio::spawn(async move { service.submit_input(&target, "new input".into()).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while w.store.blocked_appends() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    pause.release_with_error("audit injected sync failure");
    assert!(submit.await.unwrap().is_err());
    let retry = w.request().await;
    assert!(
        original.respond_text("orphan model response").is_err(),
        "old model request is still executing after runner recovery started another request"
    );
    drop(retry);
    w.shutdown().await;
}
