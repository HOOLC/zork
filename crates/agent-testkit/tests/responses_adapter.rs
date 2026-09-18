use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use reqwest::StatusCode;
use serde_json::{json, Value};
use zork_agent::provider::ProviderRouter;
use zork_agent::session::events::{SessionEvent, TurnOutcome};
use zork_agent::session::model::{
    ModelError, ModelRequest, ModelStreamObserver, SilentStreamObserver,
};
use zork_agent::session::ports::{ModelExecutor, ModelLimits, ProfileExecution};
use zork_agent::session::wire::{ProviderMessage, SessionSelection, TranscriptRole};
use zork_agent_testkit::{
    ControlledHttpProvider, PausedTimeIoGuard, PendingHttpRequest, RealAgent,
};

const MODEL: &str = "responses-model";

fn selection(profile_id: &str) -> SessionSelection {
    SessionSelection {
        profile_id: profile_id.into(),
        model: MODEL.into(),
        thinking: "xhigh".into(),
    }
}

fn profile(provider_base_url: &str, streaming: bool) -> Value {
    json!({
        "provider": "openai-compatible",
        "billing": "usage",
        "base_url": format!("{provider_base_url}/v1"),
        "headers": {"x-profile-header": "responses-fixture"},
        "auth": {"type": "api_key", "key": "responses-secret"},
        "models": [{
            "id": MODEL,
            "api": "openai-responses",
            "streaming": streaming,
            "parallel_tool_calls": false,
            "thinking": ["xhigh"],
            "default_thinking": "xhigh",
            "capabilities": {"input": ["text"]},
            "limits": {"context_window_tokens": 1000000, "max_output_tokens": 56000},
            "default": true
        }]
    })
}

fn completed(response_id: &str, input_tokens: u64, output_tokens: u64) -> Value {
    json!({
        "type": "response.completed",
        "response": {
            "id": response_id,
            "created_at": 1,
            "model": MODEL,
            "incomplete_details": null,
            "usage": {
                "input_tokens": input_tokens,
                "input_tokens_details": {"cached_tokens": input_tokens.saturating_sub(1)},
                "output_tokens": output_tokens,
                "output_tokens_details": {"reasoning_tokens": output_tokens.saturating_sub(1)}
            }
        }
    })
}

fn text_events(response_id: &str, text: &str) -> Vec<Value> {
    let item_id = format!("msg_{response_id}");
    vec![
        json!({
            "type": "response.created",
            "response": {"id": response_id, "created_at": 1, "model": MODEL}
        }),
        json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {
                "id": item_id,
                "type": "message",
                "status": "in_progress",
                "role": "assistant",
                "content": []
            }
        }),
        json!({
            "type": "response.output_text.delta",
            "item_id": item_id,
            "output_index": 0,
            "delta": text
        }),
        json!({
            "type": "response.output_item.done",
            "output_index": 0,
            "item": {
                "id": item_id,
                "type": "message",
                "status": "completed",
                "role": "assistant",
                "content": [{"type": "output_text", "text": text, "annotations": []}]
            }
        }),
        completed(response_id, 123, 45),
    ]
}

fn respond_text(request: PendingHttpRequest, response_id: &str, text: &str) {
    request.respond_sse(text_events(response_id, text)).unwrap();
}

fn response_call_item(item_id: &str, call_id: &str, tool: &str, arguments: Value) -> Value {
    json!({
        "id": item_id,
        "type": "function_call",
        "status": "completed",
        "arguments": json!({"tool": tool, "goal":"完成测试请求", "action":"执行本次测试操作", "arguments": arguments}).to_string(),
        "call_id": call_id,
        "name": "call"
    })
}

fn respond_call(
    request: PendingHttpRequest,
    response_id: &str,
    item_id: &str,
    call_id: &str,
    tool: &str,
    arguments: Value,
) {
    let item = response_call_item(item_id, call_id, tool, arguments);
    let arguments = item["arguments"].clone();
    request
        .respond_sse([
            json!({
                "type": "response.created",
                "response": {"id": response_id, "created_at": 1, "model": MODEL}
            }),
            json!({
                "type": "response.output_item.added",
                "output_index": 0,
                "item": {
                    "id": item_id,
                    "type": "function_call",
                    "status": "in_progress",
                    "arguments": "",
                    "call_id": call_id,
                    "name": "call"
                }
            }),
            json!({
                "type": "response.function_call_arguments.delta",
                "item_id": item_id,
                "output_index": 0,
                "delta": arguments
            }),
            json!({
                "type": "response.function_call_arguments.done",
                "item_id": item_id,
                "output_index": 0,
                "arguments": arguments
            }),
            json!({"type": "response.output_item.done", "output_index": 0, "item": item}),
            completed(response_id, 10, 2),
        ])
        .unwrap();
}

fn reasoning_item(id: &str, encrypted: bool) -> Value {
    if encrypted {
        json!({
            "id": id,
            "type": "reasoning",
            "status": "completed",
            "encrypted_content": "encrypted-reasoning",
            "summary": [{"type": "summary_text", "text": "Inspect the file."}]
        })
    } else {
        json!({
            "id": id,
            "type": "reasoning",
            "status": null,
            "summary": [],
            "content": [{"type": "reasoning_text", "text": "Inspect the file before answering.\n"}],
            "encrypted_content": null
        })
    }
}

fn respond_reasoning_and_read(
    request: PendingHttpRequest,
    response_id: &str,
    reasoning_id: &str,
    call_item_id: &str,
    call_id: &str,
    encrypted: bool,
) -> Vec<Value> {
    let reasoning = reasoning_item(reasoning_id, encrypted);
    let call = response_call_item(
        call_item_id,
        call_id,
        "file.read",
        json!({"path": "README.md"}),
    );
    let call_arguments = call["arguments"].clone();
    request
        .respond_sse([
            json!({
                "type": "response.created",
                "response": {"id": response_id, "created_at": 1, "model": MODEL}
            }),
            json!({
                "type": "response.output_item.added",
                "output_index": 0,
                "item": {
                    "id": reasoning_id,
                    "type": "reasoning",
                    "status": "in_progress",
                    "summary": []
                }
            }),
            json!({"type": "response.output_item.done", "output_index": 0, "item": reasoning}),
            json!({
                "type": "response.output_item.added",
                "output_index": 1,
                "item": {
                    "id": call_item_id,
                    "type": "function_call",
                    "status": "in_progress",
                    "arguments": "",
                    "call_id": call_id,
                    "name": "call"
                }
            }),
            json!({
                "type": "response.function_call_arguments.delta",
                "item_id": call_item_id,
                "output_index": 1,
                "delta": call_arguments
            }),
            json!({
                "type": "response.function_call_arguments.done",
                "item_id": call_item_id,
                "output_index": 1,
                "arguments": call_arguments
            }),
            json!({"type": "response.output_item.done", "output_index": 1, "item": call}),
            completed(response_id, 200, 60),
        ])
        .unwrap();
    vec![reasoning, call]
}

fn input(body: &Value) -> &[Value] {
    body["input"].as_array().unwrap()
}

fn assert_raw_items(body: &Value, raw_items: &[Value], call_id: &str) {
    let input = input(body);
    let start = input
        .iter()
        .position(|item| item.get("id") == raw_items[0].get("id"))
        .expect("raw provider context is present");
    assert_eq!(&input[start..start + raw_items.len()], raw_items);
    assert!(input[start + raw_items.len()..]
        .iter()
        .any(|item| { item["type"] == "function_call_output" && item["call_id"] == call_id }));
}

fn non_streaming_response(output: Vec<Value>, input_tokens: u64, output_tokens: u64) -> Value {
    json!({
        "id": "resp_non_streaming",
        "object": "response",
        "created_at": 1,
        "model": MODEL,
        "status": "completed",
        "incomplete_details": null,
        "output": output,
        "usage": {
            "input_tokens": input_tokens,
            "input_tokens_details": {"cached_tokens": input_tokens.saturating_sub(1)},
            "output_tokens": output_tokens,
            "output_tokens_details": {"reasoning_tokens": output_tokens.saturating_sub(1)}
        }
    })
}

fn respond_output_items(request: PendingHttpRequest, id: &str, items: &[Value], streaming: bool) {
    if !streaming {
        request
            .respond_json(
                StatusCode::OK,
                non_streaming_response(items.to_vec(), 200, 60),
            )
            .unwrap();
        return;
    }
    let mut events = vec![json!({
        "type": "response.created", "response": {"id": id, "created_at": 1, "model": MODEL},
    })];
    for (index, item) in items.iter().enumerate() {
        let mut added = item.clone();
        added["status"] = json!("in_progress");
        if item["type"] == "function_call" {
            added["arguments"] = json!("");
        }
        events.push(
            json!({"type": "response.output_item.added", "output_index": index, "item": added}),
        );
        if item["type"] == "function_call" {
            events.push(
                json!({"type": "response.function_call_arguments.delta", "output_index": index,
                "item_id": item["id"], "delta": item["arguments"]}),
            );
        }
        events.push(
            json!({"type": "response.output_item.done", "output_index": index, "item": item}),
        );
    }
    events.push(completed(id, 200, 60));
    request.respond_sse(events).unwrap();
}

fn assert_runtime_notice_reasoning(body: &Value) -> usize {
    let items = input(body);
    let mut count = 0;
    for (index, item) in items.iter().enumerate() {
        if item["type"] != "function_call"
            || !item["call_id"]
                .as_str()
                .is_some_and(|id| id.starts_with("call_notice_"))
        {
            continue;
        }
        count += 1;
        assert!(index > 0);
        assert_eq!(items[index - 1]["type"], "reasoning");
        assert!(items[index - 1]["content"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("Runtime-generated notification;"));
        assert_eq!(items[index + 1]["type"], "function_call_output");
        assert_eq!(items[index + 1]["call_id"], item["call_id"]);
    }
    assert!(count > 0);
    count
}

#[tokio::test(flavor = "multi_thread")]
async fn deepseek_routes_keep_notification_reasoning_and_replay_after_restart() {
    let mut agent = RealAgent::new().unwrap();
    for (provider, model, streaming) in [
        ("deepseek", MODEL, true),
        ("deepseek", MODEL, false),
        ("opencode-go", "deepseek-flash", true),
        ("opencode-go", "deepseek-flash", false),
        ("opencode-go", "deepseek-v4-pro", true),
        ("opencode-go", "deepseek-v4-pro", false),
    ] {
        let mut settings = profile(agent.provider_base_url(), streaming);
        settings["provider"] = json!(provider);
        if provider == "opencode-go" {
            settings["billing"] = json!("subscription");
        }
        settings["models"][0]["id"] = json!(model);
        let profile_id = format!("{provider}-{model}-{streaming}");
        agent.install_profile(&profile_id, settings).unwrap();
        let mut selected = selection(&profile_id);
        selected.model = model.into();
        let session = agent
            .create_configured_session(selected, None)
            .await
            .unwrap();
        agent
            .send_mail(&session, "wait, then continue this task")
            .await
            .unwrap();
        let first = agent.request().await;
        let first_body = first.json().unwrap();
        assert_eq!(first_body["model"], model);
        if provider == "opencode-go" {
            assert_eq!(
                first.headers.get("x-opencode-session").map(String::as_str),
                Some(session.as_str())
            );
        }
        let original = vec![
            reasoning_item("rs_wait", false),
            response_call_item("fc_wait", "real_wait", "wait", json!({"seconds": 0.02})),
        ];
        respond_output_items(first, "wait", &original, streaming);
        let next = tokio::time::timeout(Duration::from_secs(5), agent.request())
            .await
            .unwrap();
        let before_restart = next.json().unwrap();
        assert!(input(&before_restart).starts_with(input(&first_body)));
        assert_raw_items(&before_restart, &original, "real_wait");
        let notices = assert_runtime_notice_reasoning(&before_restart);
        assert!(before_restart.to_string().contains("reached its deadline"));
        assert_eq!(
            input(&before_restart)
                .iter()
                .filter(|item| item["role"] == "user")
                .count(),
            1
        );

        agent.stop_agent().await;
        drop(next);
        agent.start_agent().await.unwrap();
        let replay = tokio::time::timeout(Duration::from_secs(5), agent.request())
            .await
            .unwrap();
        let replay_body = replay.json().unwrap();
        assert!(input(&replay_body).starts_with(input(&before_restart)));
        assert_raw_items(&replay_body, &original, "real_wait");
        assert!(assert_runtime_notice_reasoning(&replay_body) >= notices);
        let second = vec![
            reasoning_item("rs_wait_again", false),
            response_call_item(
                "fc_wait_again",
                "real_wait_again",
                "wait",
                json!({"seconds": 0.02}),
            ),
        ];
        respond_output_items(replay, "wait_again", &second, streaming);
        let last = tokio::time::timeout(Duration::from_secs(5), agent.request())
            .await
            .unwrap();
        let last_body = last.json().unwrap();
        assert!(input(&last_body).starts_with(input(&replay_body)));
        assert_raw_items(&last_body, &original, "real_wait");
        assert_raw_items(&last_body, &second, "real_wait_again");
        assert!(assert_runtime_notice_reasoning(&last_body) > notices);
        let finish = vec![
            reasoning_item("rs_end", false),
            response_call_item("fc_end", "real_end", "end", json!({})),
        ];
        respond_output_items(last, "end", &finish, streaming);
        agent
            .wait_for_state(&session, |state| {
                state.last_turn_outcome == Some(TurnOutcome::Finished)
            })
            .await;
        let history = agent.history(&session, None, 200).unwrap();
        assert!(!history
            .iter()
            .any(|event| matches!(event.event, SessionEvent::StepFailed { .. })));
        assert!(
            !serde_json::to_string(&history)
                .unwrap()
                .contains("Runtime-generated notification;"),
            "wire compatibility markers must not replace persisted provider output"
        );
    }
    agent.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn deepseek_missing_reasoning_400_ends_the_turn_after_one_attempt() {
    let mut agent = RealAgent::new().unwrap();
    let mut settings = profile(agent.provider_base_url(), true);
    settings["provider"] = json!("deepseek");
    agent.install_profile("deepseek", settings).unwrap();
    let session = agent
        .create_configured_session(selection("deepseek"), None)
        .await
        .unwrap();
    agent.send_mail(&session, "continue").await.unwrap();
    agent.request().await.respond_json(StatusCode::BAD_REQUEST, json!({"error": {
        "type": "invalid_request_error", "code": "invalid_request_error",
        "message": "The `reasoning_text` in the thinking mode must be passed back to the API.",
    }})).unwrap();
    agent
        .wait_for_state(&session, |state| {
            state.last_turn_outcome == Some(TurnOutcome::Failed)
        })
        .await;
    let history = agent.history(&session, None, 200).unwrap();
    assert_eq!(
        history
            .iter()
            .filter(|event| matches!(event.event, SessionEvent::StepStarted { .. }))
            .count(),
        1
    );
    assert!(history.iter().any(|event| matches!(&event.event,
        SessionEvent::StepFailed { error, .. } if !error.retryable && error.status_code == Some(400))));
    agent.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
// Contract: docs/design/agent-runtime.md [RETRY-02]
async fn arbitrary_http_400_stops_streaming_and_non_streaming_after_one_request() {
    let mut agent = RealAgent::new().unwrap();
    for streaming in [true, false] {
        let profile_id = format!("http-400-{streaming}");
        agent.install_profile(&profile_id, profile(agent.provider_base_url(), streaming)).unwrap();
        for (code, message) in [
            ("invalid_request_error", "Request is missing x-opencode-session"),
            ("context_length_exceeded", "maximum context length exceeded"),
        ] {
            let session = agent.create_configured_session(selection(&profile_id), None).await.unwrap();
            agent.send_mail(&session, "continue").await.unwrap();
            agent.request().await.respond_json(StatusCode::BAD_REQUEST, json!({"error": {
                "type": "invalid_request_error", "code": code, "message": message
            }})).unwrap();
            agent.wait_for_state(&session, |state| {
                state.last_turn_outcome == Some(TurnOutcome::Failed) && state.active_turn.is_none()
            }).await;
            let history = agent.history(&session, None, 200).unwrap();
            assert_eq!(history.iter().filter(|e| matches!(e.event, SessionEvent::StepStarted { .. })).count(), 1);
            assert!(!history.iter().any(|e| matches!(e.event, SessionEvent::ContextApplied { .. })));
            assert!(history.iter().any(|e| matches!(&e.event, SessionEvent::StepFailed { error, .. }
                if !error.retryable && error.status_code == Some(400))));
        }
    }
    agent.shutdown().await;
}

struct DisconnectingStation {
    url: String,
    disconnect: Arc<AtomicBool>,
    stop: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

impl DisconnectingStation {
    async fn start(upstream: &str) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let upstream = upstream.strip_prefix("http://").unwrap().to_owned();
        let disconnect = Arc::new(AtomicBool::new(true));
        let flag = disconnect.clone();
        let (stop, mut stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    _ = &mut stopped => break,
                    Some(_) = connections.join_next(), if !connections.is_empty() => {},
                    accepted = listener.accept() => {
                        let (mut incoming, _) = accepted.unwrap();
                        if flag.load(Ordering::SeqCst) {
                            // No HTTP response: exercise the actual client send error.
                            drop(incoming);
                            continue;
                        }
                        let upstream = upstream.clone();
                        connections.spawn(async move {
                            if let Ok(mut outgoing) = tokio::net::TcpStream::connect(upstream).await {
                                let _ = tokio::io::copy_bidirectional(&mut incoming, &mut outgoing).await;
                            }
                        });
                    }
                }
            }
            connections.abort_all();
            while connections.join_next().await.is_some() {}
        });
        Self {
            url,
            disconnect,
            stop,
            task,
        }
    }

    async fn shutdown(self) {
        let _ = self.stop.send(());
        self.task.await.unwrap();
    }
}

async fn wait_for_failure_count(agent: &RealAgent, session: &str, count: usize) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let failures = agent
                .history(session, None, 200)
                .unwrap()
                .iter()
                .filter(|event| matches!(event.event, SessionEvent::StepFailed { .. }))
                .count();
            if failures >= count {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("provider failure became durable");
}

#[tokio::test(flavor = "multi_thread")]
// Contract: docs/design/agent-runtime.md [PROJECTION-02, RETRY-01, RETRY-02, RECOVERY-01]
async fn network_disconnect_and_truncated_output_recover_without_repeating_tools() {
    let mut agent = RealAgent::new().unwrap();
    for streaming in [true, false] {
        let station = DisconnectingStation::start(agent.provider_base_url()).await;
        let profile_id = format!("go-disconnect-{streaming}");
        let mut settings = profile(&station.url, streaming);
        settings["provider"] = json!("opencode-go");
        settings["billing"] = json!("subscription");
        settings["models"][0]["id"] = json!("deepseek-flash");
        agent.install_profile(&profile_id, settings).unwrap();
        let mut selected = selection(&profile_id);
        selected.model = "deepseek-flash".into();
        let session = agent
            .create_configured_session(selected, None)
            .await
            .unwrap();
        agent
            .send_mail(&session, "run a tool after the connection recovers")
            .await
            .unwrap();
        wait_for_failure_count(&agent, &session, 2).await;
        station.disconnect.store(false, Ordering::SeqCst);
        let first = tokio::time::timeout(Duration::from_secs(10), agent.request())
            .await
            .unwrap();
        assert_runtime_notice_reasoning(&first.json().unwrap());
        let original = vec![
            reasoning_item("rs_after_disconnect", false),
            response_call_item(
                "fc_once",
                "call_once",
                "shell.run",
                json!({
                    "command": "printf 'once\\n' >> count.txt",
                }),
            ),
        ];
        respond_output_items(first, "after-disconnect", &original, streaming);
        let next = agent.request().await;
        let before_truncation = next.json().unwrap();
        assert_raw_items(&before_truncation, &original, "call_once");
        let workspace = agent.workspace(&session).unwrap().to_owned();
        assert_eq!(
            std::fs::read_to_string(workspace.join("count.txt")).unwrap(),
            "once\n"
        );
        if streaming {
            let stream = next.begin_sse(8).unwrap();
            let partial = response_call_item(
                "fc_uncommitted",
                "call_uncommitted",
                "file.write",
                json!({
                    "path": "must-not-exist.txt", "content": "uncommitted response",
                }),
            );
            stream
                .send_json(json!({"type":"response.created", "response":{"id":"truncated"}}))
                .await
                .unwrap();
            stream.send_json(json!({"type":"response.output_item.added", "output_index":0, "item":{
                "type":"function_call", "id":"fc_uncommitted", "call_id":"call_uncommitted", "name":"call", "arguments":"",
            }})).await.unwrap();
            stream
                .send_json(
                    json!({"type":"response.function_call_arguments.delta", "output_index":0,
                        "item_id":"fc_uncommitted", "delta":partial["arguments"],
                    }),
                )
                .await
                .unwrap();
            stream
                .send_json(
                    json!({"type":"response.output_item.done", "output_index":0, "item":partial}),
                )
                .await
                .unwrap();
            // End the body without a terminal response: completed tool JSON alone
            // must never commit or execute an incomplete model response.
            stream.finish().await.unwrap();
        } else {
            next.respond_raw(
                StatusCode::OK,
                "application/json",
                "{\"id\":\"truncated\",\"output\":[",
            )
            .unwrap();
        }
        wait_for_failure_count(&agent, &session, 3).await;
        let recovered = agent.request().await;
        let recovered_body = recovered.json().unwrap();
        assert!(input(&recovered_body).starts_with(input(&before_truncation)));
        assert_raw_items(&recovered_body, &original, "call_once");
        assert_runtime_notice_reasoning(&recovered_body);
        assert!(!workspace.join("must-not-exist.txt").exists());
        let finish = vec![
            reasoning_item("rs_finish", false),
            response_call_item("fc_finish", "call_finish", "end", json!({})),
        ];
        respond_output_items(recovered, "finish", &finish, streaming);
        agent
            .wait_for_state(&session, |state| {
                state.last_turn_outcome == Some(TurnOutcome::Finished)
            })
            .await;
        let history = agent.history(&session, None, 200).unwrap();
        let errors = history
            .iter()
            .filter_map(|event| match &event.event {
                SessionEvent::StepFailed { error, .. } => Some(error),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(errors.len(), 3);
        assert!(errors
            .iter()
            .all(|error| error.retryable && error.status_code.is_none()));
        assert_eq!(
            std::fs::read_to_string(workspace.join("count.txt")).unwrap(),
            "once\n"
        );

        agent
            .send_mail(&session, "a request that the provider rejects")
            .await
            .unwrap();
        agent
            .request()
            .await
            .respond_json(
                StatusCode::BAD_REQUEST,
                json!({"error":{"message":"invalid input"}}),
            )
            .unwrap();
        agent
            .wait_for_state(&session, |state| {
                state.last_turn_outcome == Some(TurnOutcome::Failed)
            })
            .await;
        agent.restart().await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(200), agent.request())
                .await
                .is_err()
        );
        agent
            .send_mail(&session, "continue after correcting the bad request")
            .await
            .unwrap();
        let resumed = agent.request().await;
        assert_raw_items(&resumed.json().unwrap(), &original, "call_once");
        respond_output_items(resumed, "resumed", &finish, streaming);
        agent
            .wait_for_state(&session, |state| {
                state.last_turn_outcome == Some(TurnOutcome::Finished)
            })
            .await;
        assert_eq!(
            std::fs::read_to_string(workspace.join("count.txt")).unwrap(),
            "once\n"
        );
        assert!(!workspace.join("must-not-exist.txt").exists());
        station.shutdown().await;
    }
    agent.shutdown().await;
}

async fn advance_real_transport(duration: Duration) {
    let mut remaining = duration;
    while !remaining.is_zero() {
        let step = remaining.min(Duration::from_secs(1));
        tokio::time::advance(step).await;
        tokio::task::yield_now().await;
        remaining -= step;
    }
}

#[derive(Debug)]
struct DeltaObserver {
    deltas: tokio::sync::mpsc::UnboundedSender<String>,
}

impl ModelStreamObserver for DeltaObserver {
    fn text_delta(&self, _: &str, _: u64, _: &str, text: &str) {
        let _ = self.deltas.send(text.to_owned());
    }
}

fn provider_execution(base_url: &str, profile_id: &str, streaming: bool) -> ProfileExecution {
    ProfileExecution::new(
        profile_id.to_owned(),
        "openai-compatible".to_owned(),
        MODEL.to_owned(),
        "openai-responses".to_owned(),
        streaming,
        false,
        None,
        format!("{base_url}/v1"),
        HashMap::new(),
        "xhigh".to_owned(),
        ModelLimits {
            context_window_tokens: 1_000_000,
            max_output_tokens: 56_000,
            reserve_percent: 10,
        },
        "responses-secret".to_owned(),
    )
}

fn provider_request(profile_id: &str, observer: Arc<dyn ModelStreamObserver>) -> ModelRequest {
    ModelRequest {
        session_id: "session".to_owned(),
        generation: 1,
        step_id: "step".to_owned(),
        selection: selection(profile_id),
        transcript: Arc::new(vec![ProviderMessage {
            images: Vec::new(),
            role: TranscriptRole::User,
            content: "wait for the provider".into(),
            is_error: false,
            runtime_generated: false,
            tool_call_id: None,
            tool_calls: Vec::new(),
            provider_context: None,
        }]),
        tools: Arc::new(Vec::new()),
        max_output_tokens: Some(56000),
        independent: false,
        stream_observer: observer,
    }
}

#[tokio::test(flavor = "multi_thread")]
// Contract: docs/design/agent-runtime.md [PROVIDER-01, COMPACTION-01, RETRY-02]
async fn opencode_session_headers_reach_every_http_protocol_and_context_request() {
    let mut provider = ControlledHttpProvider::start(32).unwrap();
    for (api, path) in [
        ("openai-responses", "/v1/responses"),
        ("openai-completions", "/v1/chat/completions"),
        ("anthropic-messages", "/v1/messages"),
    ] {
        for streaming in [true, false] {
            for (session_id, step_id, independent) in [
                ("session-a", "step-1", false),
                ("session-a", "step-2", false),
                ("session-b", "step-3", false),
                ("session-a", "summary-4", true),
            ] {
                let mut request = provider_request("go", Arc::new(SilentStreamObserver));
                request.session_id = session_id.into();
                request.step_id = step_id.into();
                request.independent = independent;
                let execution = ProfileExecution::new(
                    "go".into(), "opencode-go".into(), MODEL.into(), api.into(),
                    streaming, false, None, format!("{}/v1", provider.base_url()),
                    HashMap::from([
                        ("X-OpenCode-Session".into(), "stale-profile-value".into()),
                        ("x-custom".into(), "keep".into()),
                    ]),
                    "xhigh".into(), ModelLimits {
                        context_window_tokens: 1_000_000,
                        max_output_tokens: 56_000,
                        reserve_percent: 10,
                    }, "test-secret".into(),
                );
                let task = tokio::spawn(async move {
                    ProviderRouter::new().complete(&request, execution).await
                });
                let received = provider.request().await;
                assert_eq!(received.path_and_query, path);
                assert_eq!(received.headers["x-opencode-session"], if independent { step_id } else { session_id });
                assert!(received.headers["user-agent"].starts_with("zork-agent/"));
                assert_eq!(received.headers["x-custom"], "keep");
                received.respond_json(StatusCode::BAD_REQUEST, json!({"error": {
                    "type": "invalid_request_error", "message": "controlled request rejection"
                }})).unwrap();
                let ModelError::ProviderFailed(failure) = task.await.unwrap().unwrap_err() else {
                    panic!("expected controlled provider rejection");
                };
                assert_eq!(failure.status_code, Some(400));
                assert!(!failure.retryable);
            }
        }
    }
    provider.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
// Contract: docs/design/agent-runtime.md [COMPACTION-01, PROVIDER-01, PROVIDER-02]
async fn compaction_is_tool_free_on_responses_and_chat_completions() {
    for api in ["openai-responses", "openai-completions"] {
        let mut agent = RealAgent::new().unwrap();
        let mut settings = profile(agent.provider_base_url(), true);
        settings["models"][0]["api"] = json!(api);
        settings["models"][0]["limits"] =
            json!({"context_window_tokens": 256_000, "max_output_tokens": 131_072});
        agent.install_profile("compact", settings).unwrap();
        let session = agent
            .create_configured_session(selection("compact"), None)
            .await
            .unwrap();
        agent
            .send_mail(&session, "Keep working on the same task")
            .await
            .unwrap();
        let normal = agent.request().await;
        agent
            .send_mail(&session, "Continue the remaining work.")
            .await
            .unwrap();
        if api == "openai-responses" {
            assert_eq!(normal.json().unwrap()["max_output_tokens"], 131_072);
            let mut events = text_events("normal", "progress so far");
            *events.last_mut().unwrap() = completed("normal", 123_000, 20);
            normal.respond_sse(events).unwrap();
        } else {
            normal.respond_sse([json!({"id": "normal", "object": "chat.completion.chunk",
                "choices": [{"index": 0, "delta": {"role": "assistant", "content": "progress so far"}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 123_000, "completion_tokens": 20, "total_tokens": 123_020}})]).unwrap();
        }
        let summary = tokio::time::timeout(Duration::from_secs(3), agent.request())
            .await
            .unwrap();
        let body = summary.json().unwrap();
        if api == "openai-responses" {
            assert_eq!(body["max_output_tokens"], 131_072);
        }
        assert!(
            body.get("tools")
                .is_none_or(|value| value.is_null() || value.as_array().is_some_and(Vec::is_empty)),
            "{api}: {body}"
        );
        assert!(body.get("previous_response_id").is_none());
        let input = body
            .get("input")
            .or_else(|| body.get("messages"))
            .unwrap()
            .as_array()
            .unwrap();
        assert!(input
            .iter()
            .all(|item| item["role"] != "assistant" && item["role"] != "tool"));
        if api == "openai-responses" {
            respond_text(summary, "summary", "PRIVATE SUMMARY: continue this task.");
        } else {
            summary
                .respond_openai_text("summary", "PRIVATE SUMMARY: continue this task.")
                .unwrap();
        }
        let successor = tokio::time::timeout(Duration::from_secs(3), agent.request())
            .await
            .unwrap();
        let body = successor.json().unwrap();
        if api == "openai-responses" {
            assert_eq!(body["max_output_tokens"], 131_072);
        }
        assert!(body.to_string().contains("PRIVATE SUMMARY"));
        assert_eq!(body["tools"].as_array().unwrap().len(), 1);
        assert!(!agent
            .messages(&session)
            .await
            .unwrap()
            .to_string()
            .contains("PRIVATE SUMMARY"));
        if api == "openai-responses" {
            respond_call(successor, "finish", "end", "end", "end", json!({}));
        } else {
            successor
                .respond_openai_calls("finish", [("end", "end", json!({}))])
                .unwrap();
        }
        agent
            .wait_for_state(&session, |state| {
                state.last_turn_outcome == Some(TurnOutcome::Finished)
            })
            .await;
        agent.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
// Contract: docs/design/agent-runtime.md [PERSIST-01, PROVIDER-01, PROVIDER-02, PROVIDER-03, PROVIDER-04]
async fn real_responses_adapter_preserves_wire_usage_reasoning_and_restart_replay() {
    let started = Instant::now();
    let mut agent = RealAgent::new().unwrap();
    agent
        .install_profile("responses", profile(agent.provider_base_url(), true))
        .unwrap();
    agent
        .install_profile(
            "responses-non-streaming",
            profile(agent.provider_base_url(), false),
        )
        .unwrap();

    let text_session = agent
        .create_configured_session(selection("responses"), None)
        .await
        .unwrap();
    agent.send_mail(&text_session, "say OK").await.unwrap();
    let text_request = agent.request().await;
    assert_eq!(text_request.method, "POST");
    assert_eq!(text_request.path_and_query, "/v1/responses");
    assert_eq!(
        text_request
            .headers
            .get("authorization")
            .map(String::as_str),
        Some("Bearer responses-secret")
    );
    assert_eq!(
        text_request
            .headers
            .get("x-profile-header")
            .map(String::as_str),
        Some("responses-fixture")
    );
    let body = text_request.json().unwrap();
    assert_eq!(body["model"], MODEL);
    assert_eq!(body["stream"], true);
    assert_eq!(body["store"], false);
    assert_eq!(body["parallel_tool_calls"], false);
    assert_eq!(body["max_output_tokens"], 56_000);
    assert_eq!(body["reasoning"]["effort"], "xhigh");
    assert_eq!(body["tools"].as_array().unwrap().len(), 1);
    assert_eq!(body["tools"][0]["name"], "call");
    assert_eq!(
        body["tools"][0]["parameters"]["required"],
        json!(["tool", "action", "arguments"])
    );
    assert!(input(&body)
        .iter()
        .any(|item| item.to_string().contains("say OK")));
    respond_text(text_request, "resp_text", "OK");
    agent
        .wait_for_state(&text_session, |state| {
            state.last_turn_outcome == Some(TurnOutcome::Finished)
        })
        .await;
    assert!(agent
        .history(&text_session, None, 200)
        .unwrap()
        .iter()
        .any(|event| {
            matches!(
                &event.event,
                SessionEvent::StepCompleted { usage: Some(usage), .. }
                    if usage.input_tokens == 123
                        && usage.output_tokens == 45
                        && usage.cached_input_tokens == Some(122)
            )
        }));

    let encrypted_session = agent
        .create_configured_session(selection("responses"), None)
        .await
        .unwrap();
    std::fs::write(
        agent
            .workspace(&encrypted_session)
            .unwrap()
            .join("README.md"),
        "encrypted reasoning replay\n",
    )
    .unwrap();
    agent
        .send_mail(&encrypted_session, "read README with encrypted reasoning")
        .await
        .unwrap();
    let encrypted_raw = respond_reasoning_and_read(
        agent.request().await,
        "resp_encrypted",
        "rs_encrypted",
        "fc_encrypted",
        "call_encrypted",
        true,
    );
    let encrypted_followup = agent.request().await;
    assert_raw_items(
        &encrypted_followup.json().unwrap(),
        &encrypted_raw,
        "call_encrypted",
    );

    let plain_session = agent
        .create_configured_session(selection("responses"), None)
        .await
        .unwrap();
    std::fs::write(
        agent.workspace(&plain_session).unwrap().join("README.md"),
        "plain reasoning replay\n",
    )
    .unwrap();
    agent
        .send_mail(&plain_session, "read README with plain reasoning")
        .await
        .unwrap();
    let plain_raw = respond_reasoning_and_read(
        agent.request().await,
        "resp_plain",
        "rs_plain",
        "fc_plain",
        "call_plain",
        false,
    );
    let plain_followup = agent.request().await;
    assert_raw_items(&plain_followup.json().unwrap(), &plain_raw, "call_plain");

    agent.stop_agent().await;
    drop(encrypted_followup);
    drop(plain_followup);
    agent.start_agent().await.unwrap();
    for _ in 0..2 {
        let replay = agent.request().await;
        let replay_body = replay.json().unwrap();
        if replay_body.to_string().contains("rs_encrypted") {
            assert_raw_items(&replay_body, &encrypted_raw, "call_encrypted");
            respond_call(
                replay,
                "resp_encrypted_end",
                "fc_encrypted_end",
                "call_encrypted_end",
                "end",
                json!({}),
            );
        } else {
            assert!(replay_body.to_string().contains("rs_plain"));
            assert_raw_items(&replay_body, &plain_raw, "call_plain");
            respond_call(
                replay,
                "resp_plain_end",
                "fc_plain_end",
                "call_plain_end",
                "end",
                json!({}),
            );
        }
    }
    for session_id in [&encrypted_session, &plain_session] {
        agent
            .wait_for_state(session_id, |state| {
                state.last_turn_outcome == Some(TurnOutcome::Finished)
            })
            .await;
        assert!(agent
            .history(session_id, None, 200)
            .unwrap()
            .iter()
            .any(|event| {
                matches!(
                    &event.event,
                    SessionEvent::StepCompleted {
                        provider_context: Some(context),
                        ..
                    } if context.output_items.iter().any(|item| {
                        item["type"] == "reasoning"
                    })
                )
            }));
    }

    // A completed end call must remain paired when the next user turn sends
    // the raw Responses history, including after disk recovery.
    agent.stop_agent().await;
    agent.start_agent().await.unwrap();
    agent
        .send_mail(&encrypted_session, "continue after end")
        .await
        .unwrap();
    let next_turn = agent.request().await;
    let body = next_turn.json().unwrap();
    let input = body["input"].as_array().unwrap();
    for kind in ["function_call", "function_call_output"] {
        assert_eq!(
            input
                .iter()
                .filter(|item| { item["type"] == kind && item["call_id"] == "call_encrypted_end" })
                .count(),
            1,
            "missing or duplicate {kind} after end"
        );
    }
    respond_call(
        next_turn,
        "resp_next_end",
        "fc_next_end",
        "call_next_end",
        "end",
        json!({}),
    );
    agent
        .wait_for_state(&encrypted_session, |state| {
            state.last_turn_outcome == Some(TurnOutcome::Finished)
        })
        .await;

    let non_streaming_session = agent
        .create_configured_session(selection("responses-non-streaming"), None)
        .await
        .unwrap();
    std::fs::write(
        agent
            .workspace(&non_streaming_session)
            .unwrap()
            .join("README.md"),
        "non-streaming reasoning\n",
    )
    .unwrap();
    agent
        .send_mail(&non_streaming_session, "read README without streaming")
        .await
        .unwrap();
    let non_streaming_request = agent.request().await;
    let non_streaming_body = non_streaming_request.json().unwrap();
    assert!(non_streaming_body.get("stream").is_none());
    assert_eq!(non_streaming_body["store"], false);
    let non_streaming_raw = vec![
        reasoning_item("rs_non_streaming", true),
        response_call_item(
            "fc_non_streaming",
            "call_non_streaming",
            "file.read",
            json!({"path": "README.md"}),
        ),
    ];
    non_streaming_request
        .respond_json(
            StatusCode::OK,
            non_streaming_response(non_streaming_raw.clone(), 200, 60),
        )
        .unwrap();
    let non_streaming_followup = agent.request().await;
    let non_streaming_followup_body = non_streaming_followup.json().unwrap();
    assert!(non_streaming_followup_body.get("stream").is_none());
    assert_raw_items(
        &non_streaming_followup_body,
        &non_streaming_raw,
        "call_non_streaming",
    );
    non_streaming_followup
        .respond_json(
            StatusCode::OK,
            non_streaming_response(
                vec![response_call_item(
                    "fc_non_streaming_end",
                    "call_non_streaming_end",
                    "end",
                    json!({}),
                )],
                10,
                2,
            ),
        )
        .unwrap();
    agent
        .wait_for_state(&non_streaming_session, |state| {
            state.last_turn_outcome == Some(TurnOutcome::Finished)
        })
        .await;
    assert!(agent
        .history(&non_streaming_session, None, 200)
        .unwrap()
        .iter()
        .any(|event| matches!(
            &event.event,
            SessionEvent::StepCompleted { usage: Some(usage), .. }
                if usage.input_tokens == 200 && usage.output_tokens == 60
        )));

    agent.shutdown().await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "Responses real lifecycle took {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn reasoning_output_updates_activity_before_text_or_response_completion() {
    #[derive(Debug)]
    struct OutputObserver {
        output: tokio::sync::mpsc::UnboundedSender<u64>,
        text: tokio::sync::mpsc::UnboundedSender<String>,
    }
    impl ModelStreamObserver for OutputObserver {
        fn output_delta(&self, _: &str, _: u64, _: &str, bytes: u64) {
            let _ = self.output.send(bytes);
        }
        fn text_delta(&self, _: &str, _: u64, _: &str, text: &str) {
            let _ = self.text.send(text.to_owned());
        }
    }
    // Include a delta as the very first SSE event as well as after metadata:
    // both parser entry paths must recognize DeepSeek and standard OpenAI.
    for event_type in [
        "response.reasoning_text.delta",
        "response.reasoning_summary_text.delta",
    ] {
        for metadata_first in [false, true] {
            let mut provider = ControlledHttpProvider::start(4).unwrap();
            let (output, mut observed) = tokio::sync::mpsc::unbounded_channel();
            let (text, mut visible) = tokio::sync::mpsc::unbounded_channel();
            let request = provider_request("reasoning", Arc::new(OutputObserver { output, text }));
            let execution = provider_execution(provider.base_url(), "reasoning", true);
            let task =
                tokio::spawn(
                    async move { ProviderRouter::new().complete(&request, execution).await },
                );
            let stream = provider.request().await.begin_sse(16).unwrap();
            if metadata_first {
                stream
                    .send_json(json!({
                        "type": "response.created", "response": {"id": "response-reasoning"}
                    }))
                    .await
                    .unwrap();
            }
            stream
                .send_json(json!({
                    "type": event_type, "item_id": "reasoning-1", "output_index": 0,
                    "content_index": 0, "summary_index": 0, "delta": "正在思考"
                }))
                .await
                .unwrap();
            let bytes = tokio::time::timeout(Duration::from_secs(2), observed.recv())
                .await
                .expect("first reasoning delta must publish activity immediately")
                .unwrap();
            assert_eq!(bytes, "正在思考".len() as u64);
            assert!(
                visible.try_recv().is_err(),
                "reasoning must not become visible text"
            );
            assert!(
                !task.is_finished(),
                "activity must precede response completion"
            );
            task.abort();
            let _ = task.await;
            drop(stream);
            provider.shutdown().await;
        }
    }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
// Contract: docs/design/agent-runtime.md [PROVIDER-01, RETRY-01]
async fn streaming_and_complete_responses_stay_on_one_request_across_thirty_silent_seconds() {
    let _io_guard = PausedTimeIoGuard::start();
    let router = Arc::new(ProviderRouter::new());
    let mut streaming_provider = ControlledHttpProvider::start(4).unwrap();
    let (deltas, mut observed_deltas) = tokio::sync::mpsc::unbounded_channel();
    let streaming_task = tokio::spawn({
        let router = router.clone();
        let request = provider_request("responses", Arc::new(DeltaObserver { deltas }));
        let execution = provider_execution(streaming_provider.base_url(), "responses", true);
        async move { router.complete(&request, execution).await }
    });
    let stream = streaming_provider.request().await.begin_sse(16).unwrap();
    let response_events = text_events("resp_silent_stream", "stream completed");
    for event in &response_events[..3] {
        stream.send_json(event.clone()).await.unwrap();
    }
    assert_eq!(observed_deltas.recv().await.unwrap(), "stream completed");

    advance_real_transport(Duration::from_secs(31)).await;
    assert!(!streaming_task.is_finished());

    for event in &response_events[3..] {
        stream.send_json(event.clone()).await.unwrap();
    }
    stream.finish().await.unwrap();
    let streaming_outcome = streaming_task.await.unwrap().unwrap();
    assert_eq!(streaming_outcome.text, "stream completed");
    streaming_provider.shutdown().await;

    let mut complete_provider = ControlledHttpProvider::start(4).unwrap();
    let complete_task = tokio::spawn({
        let request = provider_request("responses-non-streaming", Arc::new(SilentStreamObserver));
        let execution = provider_execution(
            complete_provider.base_url(),
            "responses-non-streaming",
            false,
        );
        async move { router.complete(&request, execution).await }
    });
    let complete_request = complete_provider.request().await;
    advance_real_transport(Duration::from_secs(31)).await;
    assert!(!complete_task.is_finished());
    complete_request
        .respond_json(
            StatusCode::OK,
            non_streaming_response(
                vec![json!({
                    "id": "msg_silent_complete",
                    "type": "message",
                    "status": "completed",
                    "role": "assistant",
                    "content": [{
                        "type": "output_text",
                        "text": "complete response",
                        "annotations": []
                    }]
                })],
                10,
                2,
            ),
        )
        .unwrap();
    let complete_outcome = complete_task.await.unwrap().unwrap();
    assert_eq!(complete_outcome.text, "complete response");
    complete_provider.shutdown().await;
}

#[tokio::test]
// Contract: docs/design/agent-runtime.md [PROVIDER-01, RETRY-02]
async fn responses_eof_without_a_terminal_event_is_a_diagnostic_provider_failure() {
    let router = ProviderRouter::new();
    let mut provider = ControlledHttpProvider::start(4).unwrap();
    let request = provider_request("responses", Arc::new(SilentStreamObserver));
    let execution = provider_execution(provider.base_url(), "responses", true);
    let task = tokio::spawn(async move { router.complete(&request, execution).await });

    let stream = provider.request().await.begin_sse(16).unwrap();
    for event in &text_events("resp_incomplete", "partial")[..3] {
        stream.send_json(event.clone()).await.unwrap();
    }
    stream.finish().await.unwrap();

    let ModelError::ProviderFailed(failure) = task.await.unwrap().unwrap_err() else {
        panic!("expected provider failure");
    };
    assert_eq!(failure.stage, "provider.stream.finish");
    assert!(failure.message.contains("terminal response event"));
    let diagnostics = failure.provider_input.expect("provider input diagnostics");
    assert_eq!(diagnostics.response_id.as_deref(), Some("resp_incomplete"));
    provider.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_outer_call_arguments_are_preserved_for_tool_error_feedback() {
    let mut agent = RealAgent::new().unwrap();
    agent
        .install_profile("responses", profile(agent.provider_base_url(), false))
        .unwrap();
    let id = agent
        .create_configured_session(selection("responses"), None)
        .await
        .unwrap();
    agent.send_mail(&id, "call a tool").await.unwrap();
    let first = agent.request().await;
    let mut item = response_call_item("bad-item", "bad-outer", "file.read", json!({}));
    item["arguments"] = json!("[]");
    first
        .respond_json(
            axum::http::StatusCode::OK,
            non_streaming_response(vec![item], 123, 4),
        )
        .unwrap();
    let retry = agent.request().await;
    let body = retry.json().unwrap();
    assert!(input(&body).iter().any(|item| item["type"] == "function_call_output" && item["call_id"] == "bad-outer"),
        "completed malformed call was discarded as StepFailed instead of receiving an error tool result");
    drop(retry);
    agent.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn file_image_reaches_responses_wire_only_when_profile_allows_it() {
    for enabled in [false, true] {
        let mut agent = RealAgent::new().unwrap();
        let mut settings = profile(agent.provider_base_url(), true);
        if enabled {
            settings["models"][0]["capabilities"]["input"] = json!(["text", "image"]);
        }
        agent.install_profile("images", settings).unwrap();
        let session = agent
            .create_configured_session(selection("images"), None)
            .await
            .unwrap();
        std::fs::write(agent.workspace(&session).unwrap().join("pixel.gif"), b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff\x21\xf9\x04\x01\x00\x00\x00\x00\x2c\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02\x44\x01\x00\x3b").unwrap();
        agent.send_mail(&session, "read pixel.gif").await.unwrap();
        respond_call(
            agent.request().await,
            "image-response",
            "image-item",
            "image-call",
            "file.read",
            json!({"path": "pixel.gif", "limit": 1}),
        );
        let next = agent.request().await;
        let body = next.json().unwrap();
        let output = body["input"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["type"] == "function_call_output" && item["call_id"] == "image-call")
            .unwrap();
        if enabled {
            assert_eq!(output["output"][1]["type"], "input_image");
            assert!(output["output"][1]["image_url"]
                .as_str()
                .unwrap()
                .starts_with("data:image/gif;base64,R0lG"));
        } else {
            assert!(output["output"]
                .as_str()
                .unwrap()
                .contains("Images were not sent"));
            assert!(!body.to_string().contains("data:image"));
        }
        respond_call(
            next,
            "end-response",
            "end-item",
            "end-call",
            "end",
            json!({}),
        );
        agent
            .wait_for_state(&session, |state| {
                state.last_turn_outcome == Some(TurnOutcome::Finished)
            })
            .await;
        agent.shutdown().await;
    }
}
