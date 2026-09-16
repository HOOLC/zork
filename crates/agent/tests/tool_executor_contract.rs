use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use zork_agent::session::events::{ToolInvocation, ToolOutcome};
use zork_agent::session::executor::ToolExecutor;
use zork_agent::session::tools::{
    NoToolState, ToolContext, ToolContract, ToolExecution, ToolImplementation, ToolInstance,
    ToolRegistry, ToolVersion,
};

struct ImmediateTool;

impl ToolImplementation for ImmediateTool {
    fn execute<'a>(
        &'a self,
        _context: &'a ToolContext,
        _arguments: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolExecution> + Send + 'a>> {
        Box::pin(async { ToolExecution::success("done") })
    }
}

struct EchoArguments;

impl ToolImplementation for EchoArguments {
    fn execute<'a>(
        &'a self,
        _context: &'a ToolContext,
        arguments: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolExecution> + Send + 'a>> {
        Box::pin(async move { ToolExecution::success(arguments.clone()) })
    }
}

struct WaitingTool {
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

impl ToolImplementation for WaitingTool {
    fn execute<'a>(
        &'a self,
        _context: &'a ToolContext,
        _arguments: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolExecution> + Send + 'a>> {
        Box::pin(async move {
            self.started.notify_one();
            self.release.notified().await;
            ToolExecution::success("released")
        })
    }
}

fn register(registry: &ToolRegistry, name: &str, implementation: Arc<dyn ToolImplementation>) {
    registry.register(Arc::new(
        ToolInstance::new(
            ToolContract {
                name: name.into(),
                version: ToolVersion::new("v1").unwrap(),
                initial_description: format!("initial {name}"),
                detailed_description: format!("detailed {name}"),
                input_schema: json!({
                    "type": "object",
                    "properties": {"value": {"type": "string"}},
                    "required": ["value"],
                    "additionalProperties": false
                }),
            },
            implementation,
            Arc::new(NoToolState),
        )
        .unwrap(),
    ));
}

fn invocation(id: &str, tool: &str) -> ToolInvocation {
    ToolInvocation {
        invocation_id: id.into(),
        provider_call_id: format!("provider-{id}"),
        turn_id: "turn-1".into(),
        started_at_ms: 1,
        tool: tool.into(),
        arguments: json!({"value": "ok"}),
        activity: None,
        tool_version: Some(ToolVersion::new("v1").unwrap()),
        rejection: None,
    }
}

#[tokio::test]
// Contract: docs/design/agent-runtime.md [TOOL-11]
async fn successful_tool_execution_returns_its_normal_result() {
    let registry = Arc::new(ToolRegistry::default());
    register(&registry, "test.immediate", Arc::new(ImmediateTool));
    let executor = ToolExecutor::new(registry, Duration::from_secs(5));

    let execution = executor
        .execute(
            ToolContext {
                control: None,
                session_id: "session".into(),
                invocation_id: "invocation-1".into(),
                workspace: "/workspace".into(),
            },
            invocation("invocation-1", "test.immediate"),
        )
        .await;

    assert_eq!(execution.outcome, ToolOutcome::Succeeded);
    assert_eq!(execution.data, json!("done"));
}

#[tokio::test]
// Contract: docs/design/agent-runtime.md [TOOL-10]
async fn original_arguments_reach_the_tools_own_parser() {
    let registry = Arc::new(ToolRegistry::default());
    registry.register(Arc::new(
        ToolInstance::new(
            ToolContract {
                name: "test.arguments".into(),
                version: ToolVersion::new("v1").unwrap(),
                initial_description: "Echo original arguments.".into(),
                detailed_description:
                    "Return the exact original arguments without schema-driven rewriting.".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "request": {
                            "type": "object",
                            "properties": {"path": {"type": "string"}},
                            "required": ["path"],
                            "additionalProperties": false
                        }
                    },
                    "required": ["request"],
                    "additionalProperties": false
                }),
            },
            Arc::new(EchoArguments),
            Arc::new(NoToolState),
        )
        .unwrap(),
    ));
    let executor = ToolExecutor::new(registry, Duration::from_secs(5));
    let mut call = invocation("extra", "test.arguments");
    call.arguments = json!({
        "request": {"path": "README.md", "reason": "ignored"},
        "comment": "ignored"
    });

    let execution = executor
        .execute(
            ToolContext {
                control: None,
                session_id: "session".into(),
                invocation_id: "extra".into(),
                workspace: "/workspace".into(),
            },
            call,
        )
        .await;

    assert_eq!(execution.outcome, ToolOutcome::Succeeded);
    assert_eq!(
        execution.data,
        json!({"request": {"path": "README.md", "reason": "ignored"}, "comment": "ignored"})
    );
}

#[tokio::test]
// Contract: docs/design/agent-runtime.md [TOOL-10, TOOL-11]
async fn cancelling_a_live_tool_produces_one_normal_cancelled_result() {
    let registry = Arc::new(ToolRegistry::default());
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    register(
        &registry,
        "test.waiting",
        Arc::new(WaitingTool {
            started: started.clone(),
            release,
        }),
    );
    let executor = Arc::new(ToolExecutor::new(registry, Duration::from_secs(5)));
    let (results, mut completed) = tokio::sync::mpsc::channel(1);

    executor.dispatch_batch(
        "session",
        "/workspace",
        vec![invocation("invocation-2", "test.waiting")],
        results,
        None,
    );
    started.notified().await;
    assert_eq!(
        executor.live("session").into_iter().collect::<Vec<_>>(),
        vec!["invocation-2"]
    );
    assert!(executor.cancel("session", "invocation-2"));

    let result = tokio::time::timeout(Duration::from_secs(1), completed.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.invocation.invocation_id, "invocation-2");
    assert_eq!(result.execution.outcome, ToolOutcome::Cancelled);
    assert!(executor.live("session").is_empty());
}

#[tokio::test]
// Contract: docs/design/agent-runtime.md [TOOL-11, DELETE-01]
async fn session_cancellation_waits_until_every_tool_has_stopped() {
    let registry = Arc::new(ToolRegistry::default());
    let started = Arc::new(tokio::sync::Notify::new());
    register(
        &registry,
        "test.wait-for-stop",
        Arc::new(WaitingTool {
            started: started.clone(),
            release: Arc::new(tokio::sync::Notify::new()),
        }),
    );
    let executor = Arc::new(ToolExecutor::new(registry, Duration::from_secs(5)));
    let (results, mut completed) = tokio::sync::mpsc::channel(1);
    executor.dispatch_batch(
        "session",
        "/workspace",
        vec![invocation("wait-for-stop", "test.wait-for-stop")],
        results,
        None,
    );
    started.notified().await;

    assert_eq!(executor.cancel_session_and_wait("session").await, 1);
    assert!(executor.live("session").is_empty());
    let result = completed.recv().await.unwrap();
    assert_eq!(result.execution.outcome, ToolOutcome::Cancelled);
}

#[tokio::test(start_paused = true)]
// Contract: docs/design/agent-runtime.md [TOOL-11, EVENT-05]
async fn completed_tools_stay_live_until_their_result_can_be_queued() {
    let registry = Arc::new(ToolRegistry::default());
    register(&registry, "test.immediate", Arc::new(ImmediateTool));
    let executor = Arc::new(ToolExecutor::new(registry, Duration::from_secs(5)));
    let (results, mut completed) = tokio::sync::mpsc::channel(1);
    let capacity = results.reserve().await.unwrap();
    executor.dispatch_batch(
        "session",
        "/workspace",
        vec![invocation("blocked-result", "test.immediate")],
        results.clone(),
        None,
    );

    // Paused time advances only after the immediate tool blocks on the full queue.
    tokio::time::sleep(Duration::from_millis(1)).await;
    assert!(executor.live("session").contains("blocked-result"));
    assert!(completed.try_recv().is_err());
    drop(capacity);
    let result = completed.recv().await.unwrap();
    assert_eq!(result.execution.outcome, ToolOutcome::Succeeded);
    assert!(executor.live("session").is_empty());

    executor.dispatch_batch(
        "session",
        "/workspace",
        vec![
            invocation("queued-result", "test.immediate"),
            invocation("closing-result", "test.immediate"),
        ],
        results,
        None,
    );
    tokio::time::sleep(Duration::from_millis(1)).await;
    assert_eq!(executor.live("session").len(), 1);
    completed.close();
    tokio::time::timeout(
        Duration::from_secs(1),
        executor.cancel_session_and_wait("session"),
    )
    .await
    .expect("closing a full queue must allow shutdown");
    assert!(executor.live("session").is_empty());
}

struct PanickingTool;

#[tokio::test]
async fn pending_user_work_does_not_limit_new_tool_invocations() {
    struct Pending {
        started: tokio::sync::mpsc::UnboundedSender<()>,
        released: tokio::sync::watch::Receiver<bool>,
    }
    impl ToolImplementation for Pending {
        fn execute<'a>(
            &'a self,
            _: &'a ToolContext,
            _: &'a Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolExecution> + Send + 'a>>
        {
            Box::pin(async move {
                let mut released = self.released.clone();
                self.started.send(()).unwrap();
                released.wait_for(|done| *done).await.unwrap();
                ToolExecution::success(json!({"finished":true}))
            })
        }
    }
    let registry = Arc::new(ToolRegistry::default());
    let (started, mut starts) = tokio::sync::mpsc::unbounded_channel();
    let (release, released) = tokio::sync::watch::channel(false);
    register(
        &registry,
        "test.user-wait",
        Arc::new(Pending { started, released }),
    );
    register(&registry, "test.publish", Arc::new(ImmediateTool));
    let executor = Arc::new(ToolExecutor::with_clock(
        registry,
        Arc::new(zork_agent::session::ports::SystemClock),
        None,
    ));
    let (results, mut completed) = tokio::sync::mpsc::channel(128);
    let invocations = (0..96)
        .map(|i| invocation(&format!("waiting-{i}"), "test.user-wait"))
        .collect();
    executor.dispatch_batch("session", "/workspace", invocations, results.clone(), None);
    tokio::time::timeout(Duration::from_secs(3), async {
        for _ in 0..96 {
            starts.recv().await.unwrap();
        }
    })
    .await
    .expect("every independent invocation must start without a global quota");
    executor.dispatch_batch(
        "session",
        "/workspace",
        vec![invocation("publish-card", "test.publish")],
        results,
        None,
    );
    let published = tokio::time::timeout(Duration::from_secs(1), completed.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(published.invocation.invocation_id, "publish-card");
    assert_eq!(executor.live("session").len(), 96);
    release.send(true).unwrap();
    for _ in 0..96 {
        assert_eq!(
            completed.recv().await.unwrap().execution.outcome,
            ToolOutcome::Succeeded
        );
    }
}

#[tokio::test]
async fn cancellation_before_dispatch_is_polled_does_not_start_the_tool() {
    let registry = Arc::new(ToolRegistry::default());
    let calls = Arc::new(AtomicUsize::new(0));
    register(
        &registry,
        "test.counted",
        Arc::new(CountedTool(calls.clone())),
    );
    let executor = Arc::new(ToolExecutor::new(registry, Duration::from_secs(5)));
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    executor.dispatch_batch(
        "session",
        "/workspace",
        vec![invocation("cancel-first", "test.counted")],
        tx,
        None,
    );
    assert!(executor.cancel("session", "cancel-first"));
    assert_eq!(
        rx.recv().await.unwrap().execution.outcome,
        ToolOutcome::Cancelled
    );
    assert_eq!(calls.load(Ordering::Relaxed), 0);
}

impl ToolImplementation for PanickingTool {
    fn execute<'a>(
        &'a self,
        _context: &'a ToolContext,
        _arguments: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolExecution> + Send + 'a>> {
        Box::pin(async { panic!("controlled tool panic") })
    }
}

struct CountedTool(Arc<AtomicUsize>);

impl ToolImplementation for CountedTool {
    fn execute<'a>(
        &'a self,
        _context: &'a ToolContext,
        arguments: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolExecution> + Send + 'a>> {
        Box::pin(async move {
            if arguments.get("value").and_then(Value::as_str).is_none() {
                return ToolExecution {
                    images: Vec::new(),
                    outcome: ToolOutcome::Failed,
                    data: json!({"error":"Invalid tool arguments: value must be a string"}),
                    result_schema_version: 1,
                    knowledge: None,
                };
            }
            self.0.fetch_add(1, Ordering::Relaxed);
            ToolExecution::success("counted")
        })
    }
}

#[tokio::test]
// Contract: docs/design/agent-runtime.md [TOOL-11]
async fn invalid_arguments_and_panics_become_results_and_the_executor_stays_usable() {
    let registry = Arc::new(ToolRegistry::default());
    let calls = Arc::new(AtomicUsize::new(0));
    register(
        &registry,
        "test.counted",
        Arc::new(CountedTool(calls.clone())),
    );
    register(&registry, "test.panics", Arc::new(PanickingTool));
    let executor = ToolExecutor::new(registry, Duration::from_secs(5));
    let context = ToolContext {
        control: None,
        session_id: "session".into(),
        invocation_id: "invocation".into(),
        workspace: "/workspace".into(),
    };

    let mut invalid = invocation("invalid", "test.counted");
    invalid.arguments = json!({});
    let invalid_result = executor.execute(context.clone(), invalid).await;
    assert_eq!(invalid_result.outcome, ToolOutcome::Failed);
    assert!(invalid_result.data["error"]
        .as_str()
        .is_some_and(|error| error.contains("Invalid tool arguments")));
    assert_eq!(calls.load(Ordering::Relaxed), 0);

    let panic_result = executor
        .execute(context.clone(), invocation("panic", "test.panics"))
        .await;
    assert_eq!(panic_result.outcome, ToolOutcome::Failed);
    assert!(panic_result.data["error"]
        .as_str()
        .is_some_and(|error| error.contains("Tool task panicked")));

    let healthy = executor
        .execute(context, invocation("healthy", "test.counted"))
        .await;
    assert_eq!(healthy.outcome, ToolOutcome::Succeeded);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

#[tokio::test]
// Contract: docs/design/agent-runtime.md [TOOL-01, TOOL-10]
async fn a_rejected_provider_call_never_reaches_the_tool() {
    let registry = Arc::new(ToolRegistry::default());
    let calls = Arc::new(AtomicUsize::new(0));
    register(
        &registry,
        "test.counted",
        Arc::new(CountedTool(calls.clone())),
    );
    let executor = ToolExecutor::new(registry, Duration::from_secs(5));
    let mut rejected = invocation("rejected", "test.counted");
    rejected.rejection = Some("missing field `arguments`".into());

    let result = executor
        .execute(
            ToolContext {
                control: None,
                session_id: "session".into(),
                invocation_id: "rejected".into(),
                workspace: "/workspace".into(),
            },
            rejected,
        )
        .await;

    assert_eq!(result.outcome, ToolOutcome::Failed);
    assert_eq!(
        result.data["error"],
        "Invalid provider call: missing field `arguments`"
    );
    assert_eq!(calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
// Contract: docs/design/agent-runtime.md [SUPERVISOR-02]
async fn bounded_internal_result_channel_waits_and_delivers_every_result() {
    let registry = Arc::new(ToolRegistry::default());
    register(&registry, "test.bounded", Arc::new(ImmediateTool));
    let executor = Arc::new(ToolExecutor::new(registry, Duration::from_secs(5)));
    let (results, mut completed) = tokio::sync::mpsc::channel(1);

    executor.dispatch_batch(
        "session",
        "/workspace",
        vec![
            invocation("bounded-1", "test.bounded"),
            invocation("bounded-2", "test.bounded"),
            invocation("bounded-3", "test.bounded"),
        ],
        results,
        None,
    );

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if executor.live("session").len() == 2 && completed.len() == 1 {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("all executions reached the capacity-one result channel");

    let mut ids = Vec::new();
    for _ in 0..3 {
        ids.push(completed.recv().await.unwrap().invocation.invocation_id);
    }
    ids.sort();
    assert_eq!(ids, ["bounded-1", "bounded-2", "bounded-3"]);
    assert!(executor.live("session").is_empty());
}

#[tokio::test]
async fn cancellation_waits_for_cleanup_without_blocking_other_tools() {
    struct CleaningTool {
        started: tokio::sync::Notify,
        cleaning: tokio::sync::Notify,
        finish_cleanup: tokio::sync::Notify,
    }
    impl ToolImplementation for CleaningTool {
        fn execute<'a>(
            &'a self,
            _: &'a ToolContext,
            _: &'a Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolExecution> + Send + 'a>>
        {
            Box::pin(async move {
                self.started.notify_one();
                std::future::pending().await
            })
        }
        fn cancel<'a>(
            &'a self,
            context: &'a ToolContext,
            arguments: &'a Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<ToolExecution>> + Send + 'a>>
        {
            Box::pin(async move {
                assert_eq!(context.invocation_id, "cleaning");
                assert_eq!(arguments, &json!({"value":"ok"}));
                self.cleaning.notify_one();
                self.finish_cleanup.notified().await;
                let mut result = ToolExecution::success(json!({"process_state":"exited"}));
                result.outcome = ToolOutcome::Cancelled;
                Some(result)
            })
        }
    }
    let registry = Arc::new(ToolRegistry::default());
    let tool = Arc::new(CleaningTool {
        started: Default::default(),
        cleaning: Default::default(),
        finish_cleanup: Default::default(),
    });
    register(&registry, "test.cleanup", tool.clone());
    register(&registry, "test.after", Arc::new(ImmediateTool));
    let executor = Arc::new(ToolExecutor::with_clock(
        registry,
        Arc::new(zork_agent::session::ports::SystemClock),
        None,
    ));
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    executor.dispatch_batch(
        "session",
        "/workspace",
        vec![invocation("cleaning", "test.cleanup")],
        tx.clone(),
        None,
    );
    tool.started.notified().await;
    assert!(executor.cancel("session", "cleaning"));
    tool.cleaning.notified().await;
    executor.dispatch_batch(
        "session",
        "/workspace",
        vec![invocation("after", "test.after")],
        tx,
        None,
    );
    let independent = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("cleanup must not block an independent invocation")
        .unwrap();
    assert_eq!(independent.invocation.invocation_id, "after");
    assert!(rx.try_recv().is_err());
    assert!(executor.live("session").contains("cleaning"));
    tool.finish_cleanup.notify_one();
    let first = rx.recv().await.unwrap();
    assert_eq!(first.invocation.invocation_id, "cleaning");
    assert_eq!(first.execution.data, json!({"process_state":"exited"}));
    assert_eq!(first.execution.outcome, ToolOutcome::Cancelled);
}
