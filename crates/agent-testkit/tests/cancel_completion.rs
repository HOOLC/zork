use serde_json::{json, Value};
use std::{future::Future, pin::Pin, sync::Arc};
use zork_agent::session::{
    events::{SessionEvent, ToolOutcome},
    model::ModelOutcome,
    service::ServiceOptions,
    tools::{
        NoToolState, ToolContext, ToolContract, ToolExecution, ToolImplementation, ToolInstance,
        ToolVersion,
    },
    wire::{ProviderToolCall, SessionSelection},
};
use zork_agent_testkit::TestWorld;

struct Cleanup {
    started: tokio::sync::Notify,
    cleaning: tokio::sync::Notify,
    release: tokio::sync::Notify,
    unknown: bool,
}
impl ToolImplementation for Cleanup {
    fn execute<'a>(
        &'a self,
        _: &'a ToolContext,
        _: &'a Value,
    ) -> Pin<Box<dyn Future<Output = ToolExecution> + Send + 'a>> {
        Box::pin(async move {
            self.started.notify_one();
            std::future::pending().await
        })
    }
    fn cancel<'a>(
        &'a self,
        _: &'a ToolContext,
        _: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Option<ToolExecution>> + Send + 'a>> {
        Box::pin(async move {
            self.cleaning.notify_one();
            self.release.notified().await;
            let mut result = ToolExecution::success(if self.unknown {
                json!({"state":"outcome_unknown","effects_may_have_occurred":true})
            } else {
                json!({"process_state":"exited"})
            });
            result.outcome = if self.unknown {
                ToolOutcome::Failed
            } else {
                ToolOutcome::Cancelled
            };
            Some(result)
        })
    }
}
async fn setup(unknown: bool) -> (TestWorld, String, Arc<Cleanup>, String) {
    let options = ServiceOptions::default();
    let mut world = TestWorld::with_options(options);
    let tool = Arc::new(Cleanup {
        started: Default::default(),
        cleaning: Default::default(),
        release: Default::default(),
        unknown,
    });
    world.tools.register(Arc::new(
        ToolInstance::new(
            ToolContract {
                name: "test.cleanup".into(),
                version: ToolVersion::new("v1").unwrap(),
                initial_description: "Delayed cleanup".into(),
                detailed_description: "Delayed cleanup".into(),
                input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            },
            tool.clone(),
            Arc::new(NoToolState),
        )
        .unwrap(),
    ));
    let session = world
        .create_session(
            SessionSelection {
                profile_id: "test-profile".into(),
                model: "test-model".into(),
                thinking: "medium".into(),
            },
            None,
            "/virtual/cancel-completion",
        )
        .await
        .unwrap();
    world.send_mail(&session, "start work").await.unwrap();
    world
        .request()
        .await
        .respond(Ok(ModelOutcome {
            text: String::new(),
            tool_calls: vec![ProviderToolCall {
                tool_call_id: "start".into(),
                tool_name: "call".into(),
                arguments: json!({"tool":"test.cleanup","action":"start","arguments":{},"wait":0}),
            }],
            provider_context: None,
            usage: None,
            provider_input: None,
        }))
        .unwrap();
    tool.started.notified().await;
    let target = world
        .state(&session)
        .await
        .unwrap()
        .pending_tools
        .values()
        .find(|p| p.invocation.tool == "test.cleanup")
        .unwrap()
        .invocation
        .invocation_id
        .clone();
    world
        .request()
        .await
        .respond_call("cancel", "tool.cancel", json!({"invocation_id":target}))
        .unwrap();
    tool.cleaning.notified().await;
    (world, session, tool, target)
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_waits_for_persisted_target_result_without_blocking_the_runner() {
    for unknown in [false, true] {
        let (mut world, session, tool, target) = setup(unknown).await;
        // Querying the session while cleanup is blocked must still work.
        assert!(world
            .state(&session)
            .await
            .unwrap()
            .pending_tools
            .values()
            .any(|p| p.invocation.tool == "tool.cancel" && p.result.is_none()));
        assert!(!world.events(&session).iter().any(
            |e| matches!(&e.event,SessionEvent::ToolResult{result} if result.tool=="tool.cancel")
        ));
        tool.release.notify_one();
        let resumed = world.request().await;
        let events = world.events(&session);
        let target_pos=events.iter().position(|e|matches!(&e.event,SessionEvent::ToolResult{result} if result.invocation_id==target)).unwrap();
        let cancel_pos=events.iter().position(|e|matches!(&e.event,SessionEvent::ToolResult{result} if result.tool=="tool.cancel")).unwrap();
        assert!(target_pos < cancel_pos);
        let SessionEvent::ToolResult { result } = &events[cancel_pos].event else {
            unreachable!()
        };
        assert_eq!(
            result.data["target_outcome"],
            if unknown { "failed" } else { "cancelled" }
        );
        assert_eq!(result.data["signalled"], true);
        if unknown {
            assert_eq!(result.data["target_result"]["state"], "outcome_unknown");
        } else {
            assert_eq!(result.data["target_result"]["process_state"], "exited");
        }
        assert!(resumed
            .transcript
            .iter()
            .any(|m| m.content.contains("target_outcome")));
        assert!(resumed
            .transcript
            .iter()
            .any(|m| m.content.contains(&target)));
        resumed.respond_text("done").unwrap();
        world
            .wait_for_state(&session, |state| state.active_turn.is_none())
            .await;
        world.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_the_turn_while_cancel_tool_waits_does_not_deadlock() {
    let (mut world, session, tool, _) = setup(false).await;
    world.cancel(&session).await.unwrap();
    tool.release.notify_one();
    world
        .wait_for_state(&session, |state| state.active_turn.is_none())
        .await;
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_target_does_not_claim_cancellation_or_create_a_false_request() {
    let mut world = TestWorld::new();
    let session = world
        .create_session(
            SessionSelection {
                profile_id: "test-profile".into(),
                model: "test-model".into(),
                thinking: "medium".into(),
            },
            None,
            "/virtual/cancel-missing",
        )
        .await
        .unwrap();
    world.send_mail(&session, "cancel missing").await.unwrap();
    world
        .request()
        .await
        .respond_call("cancel", "tool.cancel", json!({"invocation_id":"missing"}))
        .unwrap();
    let resumed = world.request().await;
    let events = world.events(&session);
    assert!(!events
        .iter()
        .any(|e| matches!(&e.event, SessionEvent::ToolCancelRequested { .. })));
    let result = events
        .iter()
        .find_map(|e| {
            if let SessionEvent::ToolResult { result } = &e.event {
                Some(result)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(result.data["signalled"], false);
    assert!(result.data["target_result"].is_null());
    resumed.respond_text("done").unwrap();
    world
        .wait_for_state(&session, |s| s.active_turn.is_none())
        .await;
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn slow_cleanup_does_not_block_cancelling_another_running_invocation() {
    let (mut world, session, tool, first) = setup(false).await;
    let mut independent = world
        .install_tool(ToolContract {
            name: "test.independent".into(),
            version: ToolVersion::new("v1").unwrap(),
            initial_description: "Independent pending work".into(),
            detailed_description: "Independent pending work".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
        })
        .unwrap();
    world
        .send_mail(&session, "start another operation")
        .await
        .unwrap();
    world
        .request()
        .await
        .respond(Ok(ModelOutcome {
            text: String::new(),
            tool_calls: vec![ProviderToolCall {
                tool_call_id: "second".into(),
                tool_name: "call".into(),
                arguments: json!({"tool":"test.independent","action":"start","arguments":{},"wait":0}),
            }],
            provider_context: None,
            usage: None,
            provider_input: None,
        }))
        .unwrap();
    let _pending_second = independent.request().await;
    let next = world.request().await;
    let second = world
        .state(&session)
        .await
        .unwrap()
        .pending_tools
        .values()
        .find(|p| p.invocation.tool == "test.independent" && p.invocation.invocation_id != first)
        .unwrap()
        .invocation
        .invocation_id
        .clone();
    next.respond_call(
        "cancel-second",
        "tool.cancel",
        json!({"invocation_id":second}),
    )
    .unwrap();
    let resumed = tokio::time::timeout(std::time::Duration::from_secs(2), world.request())
        .await
        .expect("second cancellation cannot wait for first cleanup");
    let events = world.events(&session);
    assert!(events.iter().any(|e|matches!(&e.event,SessionEvent::ToolResult{result} if result.tool=="tool.cancel" && result.data["target_invocation_id"]==second && result.data["target_outcome"]=="cancelled")));
    assert!(!events.iter().any(
        |e| matches!(&e.event,SessionEvent::ToolResult{result} if result.invocation_id==first)
    ));
    world.cancel(&session).await.unwrap();
    tool.release.notify_one();
    drop(resumed);
    world
        .wait_for_state(&session, |state| state.active_turn.is_none())
        .await;
    world.shutdown().await;
}
