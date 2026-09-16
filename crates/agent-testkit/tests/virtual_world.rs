use serde_json::json;
use std::time::Duration;
use zork_agent::session::events::{AutoWaitEndReason, SessionEvent, ToolOutcome, TurnOutcome};
use zork_agent::session::tools::{ToolContract, ToolVersion};
use zork_agent::session::wire::{SessionSelection, TranscriptRole};
use zork_agent_testkit::TestWorld;

#[tokio::test(flavor = "multi_thread")]
// Contract: docs/design/agent-runtime.md [TESTKIT-01, EVENT-05, WAIT-01]
async fn virtual_world_can_pause_assert_and_continue_at_effect_boundaries() {
    let mut world = TestWorld::new();
    let mut echo = world
        .install_tool(ToolContract {
            name: "test.echo".into(),
            version: ToolVersion::new("test-1").unwrap(),
            initial_description: "Echo controlled test data.".into(),
            detailed_description: "Return the value supplied by the test controller.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {"value": {"type": "string"}},
                "required": ["value"],
                "additionalProperties": false
            }),
        })
        .unwrap();
    let session_id = world
        .create_session(
            SessionSelection {
                profile_id: "test-profile".into(),
                model: "test-model".into(),
                thinking: "medium".into(),
            },
            Some("test system prompt".into()),
            "/virtual/workspace",
        )
        .await
        .unwrap();

    world.send_mail(&session_id, "first message").await.unwrap();
    let first = world.request().await;
    assert_eq!(first.session_id, session_id);
    assert_eq!(first.selection.model, "test-model");
    assert_eq!(first.tools.len(), 1);
    assert_eq!(first.tools[0].name, "call");
    assert!(first
        .transcript
        .iter()
        .any(|message| message.content.contains("first message")));
    first
        .respond_call("provider-call-1", "test.echo", json!({"value": "one"}))
        .unwrap();

    let pending_echo = echo.request().await;
    assert_eq!(pending_echo.context.session_id, session_id);
    assert_eq!(pending_echo.arguments, json!({"value": "one"}));
    let pending_state = world.state(&session_id).await.unwrap();
    assert!(pending_state.auto_wait.is_some());
    assert!(pending_state
        .pending_tools
        .values()
        .any(|tool| tool.invocation.tool == "test.echo" && tool.result.is_none()));

    world
        .send_mail(&session_id, "message during auto wait")
        .await
        .unwrap();
    let second = world.request().await;
    assert!(second
        .transcript
        .iter()
        .any(|message| message.content.contains("message during auto wait")));

    pending_echo
        .succeed(json!({"message": "echo completed", "value": "one"}))
        .unwrap();
    second
        .respond_text("I will incorporate the result.")
        .unwrap();

    let third = world.request().await;
    assert!(third
        .transcript
        .iter()
        .any(|message| message.content.contains("echo completed")));
    third.respond_text("done").unwrap();

    let finished = world
        .wait_for_state(&session_id, |state| {
            state.last_turn_outcome == Some(TurnOutcome::Finished)
        })
        .await;
    assert_eq!(finished.last_turn_outcome, Some(TurnOutcome::Finished));

    let events = world.events(&session_id);
    assert!(events.iter().any(|event| matches!(
        event.event,
        SessionEvent::AutoWaitEnded {
            reason: AutoWaitEndReason::NewInput,
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(
        &event.event,
        SessionEvent::ToolResult { result }
            if result.tool == "test.echo" && result.outcome == ToolOutcome::Succeeded
    )));
    assert!(events.iter().any(|event| matches!(
        event.event,
        SessionEvent::TurnFinished {
            outcome: TurnOutcome::Finished,
            ..
        }
    )));
    let history = world.history(&session_id, None, 100).unwrap();
    assert_eq!(history.len(), events.len());

    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
// Contract: docs/design/agent-runtime.md [TESTKIT-01, TOOL-13]
async fn virtual_builtins_share_vfs_with_the_controlled_process() {
    let mut world = TestWorld::new();
    let workspace = "/virtual/workspace";
    let session_id = world
        .create_session(
            SessionSelection {
                profile_id: "test-profile".into(),
                model: "test-model".into(),
                thinking: "medium".into(),
            },
            None,
            workspace,
        )
        .await
        .unwrap();
    world
        .send_mail(&session_id, "edit a virtual file")
        .await
        .unwrap();

    world
        .request()
        .await
        .respond_call(
            "provider-call-1",
            "file.write",
            json!({"path": "note.txt", "content": "before"}),
        )
        .unwrap();
    let after_write = world.request().await;
    assert_eq!(
        world.files.read_text(format!("{workspace}/note.txt")),
        Some("before".into())
    );
    after_write
        .respond_call(
            "provider-call-2",
            "shell.run",
            json!({"command": "replace note.txt"}),
        )
        .unwrap();

    let process = world.process_request().await;
    assert_eq!(process.request.command, "replace note.txt");
    assert_eq!(process.request.current_dir.to_string_lossy(), workspace);
    world
        .files
        .write_text(format!("{workspace}/note.txt"), "after");
    process.output("virtual shell output\n");
    process.succeed();

    let after_shell = world.request().await;
    assert!(after_shell
        .transcript
        .iter()
        .any(|message| message.content.contains("virtual shell output")));
    let events = world.events(&session_id);
    let invocation_id = events
        .iter()
        .find_map(|event| match &event.event {
            SessionEvent::StepCompleted { invocations, .. } => invocations
                .iter()
                .find(|invocation| invocation.tool == "shell.run")
                .map(|invocation| invocation.invocation_id.as_str()),
            _ => None,
        })
        .expect("shell invocation was durably recorded");
    assert_eq!(
        world
            .files
            .read_text(format!("{workspace}/.zork/live-{invocation_id}.log")),
        Some("virtual shell output\n".into())
    );
    assert_eq!(
        world.files.read_text(format!("{workspace}/note.txt")),
        Some("after".into())
    );

    after_shell.respond_text("done").unwrap();
    world
        .wait_for_state(&session_id, |state| {
            state.last_turn_outcome == Some(TurnOutcome::Finished)
        })
        .await;
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
// Contract: docs/design/agent-runtime.md [TESTKIT-01, TOOL-11]
async fn default_shell_survives_a_day_of_virtual_time_until_explicit_cancellation() {
    let mut world = TestWorld::new();
    let session_id = world
        .create_session(
            SessionSelection {
                profile_id: "test-profile".into(),
                model: "test-model".into(),
                thinking: "medium".into(),
            },
            None,
            "/virtual/workspace",
        )
        .await
        .unwrap();
    world
        .send_mail(&session_id, "run until cancelled")
        .await
        .unwrap();
    world
        .request()
        .await
        .respond_call(
            "provider-call-1",
            "shell.run",
            json!({"command": "long running"}),
        )
        .unwrap();
    let process = world.process_request().await;
    world
        .wait_for_state(&session_id, |state| state.auto_wait.is_some())
        .await;
    world.clock.advance(Duration::from_secs(24 * 60 * 60));
    let still_running = world.request().await;
    assert!(!process.was_killed());
    assert!(still_running.transcript.iter().any(|message| message
        .content
        .contains("invocation_id=")
        && message.content.contains("still unfinished")
        && !message.is_error));
    world.cancel(&session_id).await.unwrap();
    world
        .wait_for_state(&session_id, |state| {
            state.last_turn_outcome == Some(TurnOutcome::Cancelled)
        })
        .await;
    assert!(process.was_killed());
    assert!(!world.events(&session_id).iter().any(|event|matches!(&event.event,SessionEvent::ToolResult{result} if result.outcome==ToolOutcome::TimedOut)));
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
// Contract: docs/design/agent-runtime.md [STATE-01, RECOVERY-02]
async fn virtual_world_restarts_from_the_same_in_memory_event_stream() {
    let mut world = TestWorld::new();
    let session_id = world
        .create_session(
            SessionSelection {
                profile_id: "test-profile".into(),
                model: "test-model".into(),
                thinking: "medium".into(),
            },
            None,
            "/virtual/workspace",
        )
        .await
        .unwrap();
    world
        .send_mail(&session_id, "before restart")
        .await
        .unwrap();
    world.request().await.respond_text("done").unwrap();
    world
        .wait_for_state(&session_id, |state| {
            state.last_turn_outcome == Some(TurnOutcome::Finished)
        })
        .await;

    let event_count = world.events(&session_id).len();
    world.restart().await.unwrap();
    assert_eq!(
        world.state(&session_id).await.unwrap().last_turn_outcome,
        Some(TurnOutcome::Finished)
    );
    assert_eq!(world.events(&session_id).len(), event_count);

    world.send_mail(&session_id, "after restart").await.unwrap();
    let request = world.request().await;
    assert!(request
        .transcript
        .iter()
        .any(|message| message.content.contains("after restart")));
    request.respond_text("done").unwrap();
    world
        .wait_for_state(&session_id, |state| {
            state.last_turn_outcome == Some(TurnOutcome::Finished) && state.active_turn.is_none()
        })
        .await;
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
// Contract: docs/design/agent-runtime.md [QUERY-02]
async fn history_list_and_file_read_share_the_same_session_query_event() {
    let mut world = TestWorld::new();
    let session_id = world
        .create_session(
            SessionSelection {
                profile_id: "test-profile".into(),
                model: "test-model".into(),
                thinking: "medium".into(),
            },
            None,
            "/virtual/history-query",
        )
        .await
        .unwrap();
    world
        .send_mail(&session_id, "inspect this exact durable input")
        .await
        .unwrap();
    let input = world
        .events(&session_id)
        .into_iter()
        .find(|event| matches!(event.event, SessionEvent::InputAppended { .. }))
        .unwrap();

    world
        .request()
        .await
        .respond_call(
            "provider-history-list",
            "history.list",
            json!({"limit": 20}),
        )
        .unwrap();
    let listed = world.request().await;
    let uri = format!("zork://history/{}", input.event_id);
    assert!(listed
        .transcript
        .iter()
        .any(|message| message.content.contains(&uri)));
    listed
        .respond_call("provider-history-read", "file.read", json!({"path": uri}))
        .unwrap();

    let read = world.request().await;
    let serialized = serde_json::to_string_pretty(&input).unwrap();
    let rendered: serde_json::Value = serde_json::from_str(
        read.transcript
            .iter()
            .rev()
            .find(|message| message.role == TranscriptRole::Tool)
            .expect("file.read returned one direct tool result")
            .content
            .as_ref(),
    )
    .unwrap();
    assert_eq!(rendered["data"]["content"], serialized);
    read.respond_text("done").unwrap();
    world
        .wait_for_state(&session_id, |state| {
            state.last_turn_outcome == Some(TurnOutcome::Finished)
        })
        .await;
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
// Contract: docs/design/agent-runtime.md [TOOL-13, PROJECTION-01]
async fn file_read_projects_one_bounded_canonical_payload() {
    let mut world = TestWorld::new();
    let workspace = "/virtual/workspace";
    let content = "unique-file-content-".repeat(1_024);
    world
        .files
        .write_text(format!("{workspace}/large.txt"), &content);
    let session_id = world
        .create_session(
            SessionSelection {
                profile_id: "test-profile".into(),
                model: "test-model".into(),
                thinking: "medium".into(),
            },
            None,
            workspace,
        )
        .await
        .unwrap();

    world.send_mail(&session_id, "read the file").await.unwrap();
    world
        .request()
        .await
        .respond_call(
            "provider-file-read",
            "file.read",
            json!({"path": "large.txt", "limit": content.len()}),
        )
        .unwrap();

    let projected = world.request().await;
    let tool_message = projected
        .transcript
        .iter()
        .rev()
        .find(|message| message.role == TranscriptRole::Tool)
        .expect("file.read returned one direct tool result");
    let rendered: serde_json::Value = serde_json::from_str(&tool_message.content).unwrap();
    assert!(
        rendered.get("message").is_none(),
        "ToolResult must not carry a second display payload"
    );
    assert_eq!(rendered["data"]["content"], content);
    assert!(
        tool_message.content.len() <= content.len() + 512,
        "projected file payload was duplicated: {} bytes for {} bytes of content",
        tool_message.content.len(),
        content.len()
    );

    projected.respond_text("done").unwrap();
    world
        .wait_for_state(&session_id, |state| {
            state.last_turn_outcome == Some(TurnOutcome::Finished)
        })
        .await;
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn image_file_read_is_complete_and_survives_restart() {
    use zork_agent::session::ports::FileSystem;
    let mut world = TestWorld::new();
    // A complete 1x1 GIF; misleading suffix verifies content sniffing.
    let gif = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff\x21\xf9\x04\x01\x00\x00\x00\x00\x2c\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02\x44\x01\x00\x3b";
    world
        .files
        .write(std::path::Path::new("/virtual/workspace/picture.txt"), gif)
        .unwrap();
    let session_id = world
        .create_session(
            SessionSelection {
                profile_id: "test-profile".into(),
                model: "test-model".into(),
                thinking: "medium".into(),
            },
            None,
            "/virtual/workspace",
        )
        .await
        .unwrap();
    world
        .send_mail(&session_id, "read the image")
        .await
        .unwrap();
    world
        .request()
        .await
        .respond_call(
            "image-read",
            "file.read",
            json!({"path": "picture.txt", "limit": 1}),
        )
        .unwrap();
    let request = world.request().await;
    let message = request
        .transcript
        .iter()
        .find(|m| m.tool_call_id.as_deref() == Some("image-read"))
        .unwrap();
    assert_eq!(message.images.len(), 1);
    assert_eq!(message.images[0].media_type, "image/gif");
    assert!(message.images[0].base64.len() > gif.len());
    assert!(!message.content.contains("GIF89a"));
    let images = message.images.clone();
    request.respond_text("done").unwrap();
    world
        .wait_for_state(&session_id, |s| {
            s.last_turn_outcome == Some(TurnOutcome::Finished)
        })
        .await;
    world.restart().await.unwrap();
    world.send_mail(&session_id, "continue").await.unwrap();
    let request = world.request().await;
    assert_eq!(
        request
            .transcript
            .iter()
            .find(|m| m.tool_call_id.as_deref() == Some("image-read"))
            .unwrap()
            .images,
        images
    );
    request
        .respond_call(
            "bad-image-offset",
            "file.read",
            json!({"path": "picture.txt", "offset": 1}),
        )
        .unwrap();
    let request = world.request().await;
    let error = request
        .transcript
        .iter()
        .find(|m| m.tool_call_id.as_deref() == Some("bad-image-offset"))
        .unwrap();
    assert!(error.is_error);
    assert!(error.images.is_empty());
    assert!(error.content.contains("offset=0"));
    request.respond_text("done").unwrap();
    world
        .wait_for_state(&session_id, |s| {
            s.last_turn_outcome == Some(TurnOutcome::Finished)
        })
        .await;
    world.shutdown().await;
}
