use std::{sync::Arc, time::Duration};

use futures_util::StreamExt;
use reqwest::StatusCode;
use serde_json::{json, Value};
use tempfile::TempDir;
use zork_agent::session::events::{Selection, SessionEvent, TurnOutcome};
use zork_agent::session::service::{LiveSessionEvent, ServiceOptions};
use zork_agent_api::{
    DurableEvent, EventQuery, SessionSnapshot, TextDeltaEvent, DURABLE_EVENT_NAME,
    TEXT_DELTA_EVENT_NAME,
};
use zork_agent_testkit::{PendingHttpRequest, RealAgent, TestWorld};

#[derive(Debug)]
struct SseFrame {
    event: String,
    id: Option<String>,
    data: Value,
}

fn body_contains(request: &PendingHttpRequest, needle: &str) -> bool {
    request
        .json()
        .expect("provider request is JSON")
        .to_string()
        .contains(needle)
}

fn profile_document(provider_base_url: &str) -> Value {
    json!({
        "provider": "openai",
        "billing": "usage",
        "base_url": format!("{provider_base_url}/v1"),
        "headers": {"x-profile-header": "fixture"},
        "auth": {"type": "api_key", "key": "secret-that-must-not-be-returned"},
        "models": [{
            "id": "fixture-model",
            "api": "openai-completions",
            "streaming": true,
            "parallel_tool_calls": false,
            "thinking": ["low", "high"],
            "default_thinking": "high",
            "capabilities": {"input": ["text", "image"]},
            "limits": {"context_window_tokens": 256000, "max_output_tokens": 32000},
            "default": true
        }]
    })
}

async fn put_profile(agent: &RealAgent, profile_id: &str, document: &Value) -> Value {
    let response = agent
        .client()
        .put(format!("{}/profiles/{profile_id}", agent.base_url()))
        .json(document)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.json().await.unwrap()
}

async fn create_session(
    agent: &RealAgent,
    workspace: &std::path::Path,
    profile_id: &str,
    thinking: &str,
) -> Value {
    let response = agent
        .client()
        .post(format!("{}/sessions", agent.base_url()))
        .json(&json!({
            "profile_id": profile_id,
            "model": "fixture-model",
            "thinking": thinking,
            "workspace": workspace
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    response.json().await.unwrap()
}

async fn send_mail(agent: &RealAgent, session_id: &str, content: &str) {
    let response = agent
        .client()
        .post(format!(
            "{}/sessions/{session_id}/mailbox",
            agent.base_url()
        ))
        .json(&json!({"content": content}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
}

#[tokio::test(flavor = "multi_thread")]
// Contract: docs/design/agent-runtime.md [CONTEXT-01, HTTP-01, HTTP-02]
async fn session_context_configuration_defaults_overrides_and_survives_restart() {
    let mut agent = RealAgent::new().unwrap();
    put_profile(
        &agent,
        "fixture",
        &profile_document(agent.provider_base_url()),
    )
    .await;
    let workspace = TempDir::new().unwrap();
    let session = create_session(&agent, workspace.path(), "fixture", "low").await;
    assert_eq!(
        session["context"],
        json!({"strategy": "compaction", "keep_recent_tokens": 20_000})
    );
    let id = session["session_id"].as_str().unwrap();
    let changed = agent
        .client()
        .put(format!("{}/sessions/{id}/context", agent.base_url()))
        .json(&json!({"strategy": "handoff", "keep_recent_tokens": 0}))
        .send()
        .await
        .unwrap();
    assert_eq!(changed.status(), StatusCode::OK);
    assert_eq!(
        changed.json::<Value>().await.unwrap()["context"]["strategy"],
        "handoff"
    );
    for invalid in [
        json!({"strategy": "unknown"}),
        json!({"keep_recent_tokens": -1}),
        json!({"unexpected": true}),
    ] {
        let response = agent
            .client()
            .put(format!("{}/sessions/{id}/context", agent.base_url()))
            .json(&invalid)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
    agent.restart().await.unwrap();
    let read = agent
        .client()
        .get(format!("{}/sessions/{id}", agent.base_url()))
        .send()
        .await
        .unwrap();
    assert_eq!(
        read.json::<Value>().await.unwrap()["context"],
        json!({"strategy": "handoff", "keep_recent_tokens": 0})
    );
    let response = agent
        .client()
        .post(format!("{}/sessions", agent.base_url()))
        .json(&json!({
            "profile_id": "fixture", "model": "fixture-model", "thinking": "low",
            "workspace": workspace.path(), "context": {"strategy": "handoff"}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(
        response.json::<Value>().await.unwrap()["context"]["strategy"],
        "handoff"
    );
    agent.shutdown().await;
}

async fn read_messages(agent: &RealAgent, session_id: &str, query: &str) -> Value {
    let response = agent
        .client()
        .get(format!(
            "{}/sessions/{session_id}/messages{query}",
            agent.base_url()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.json().await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
// Contract: docs/design/agent-runtime.md [HTTP-01, HTTP-02, QUERY-01, SSE-01, SSE-02, SSE-03]
async fn real_http_lifecycle_covers_profiles_sessions_messages_sse_and_auth() {
    let token = "agent-app-api-token";
    let mut agent = RealAgent::fake_with_token(Some(token.into())).unwrap();
    let unauthenticated = reqwest::Client::new();

    assert_eq!(
        unauthenticated
            .get(format!("{}/readyz", agent.base_url()))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    for route in ["/profiles", "/sessions"] {
        assert_eq!(
            unauthenticated
                .get(format!("{}{route}", agent.base_url()))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    for route in [
        "/healthz",
        "/v1/identity",
        "/v1/health",
        "/v1/capabilities",
        "/v1/auth-replicas",
        "/v1/models",
    ] {
        assert_eq!(
            unauthenticated
                .get(format!("{}{route}", agent.base_url()))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }

    let mut profile = profile_document(agent.provider_base_url());
    profile["models"][0]["enabled"] = json!(true);
    let public_profile = put_profile(&agent, "fixture", &profile).await;
    assert_eq!(public_profile["profile_id"], "fixture");
    assert_eq!(public_profile["provider"], "openai");
    assert_eq!(public_profile["billing"], "usage");
    assert_eq!(public_profile["auth_configured"], true);
    assert_eq!(
        public_profile["account"],
        json!({"ok": false, "error": "not_probed"})
    );
    assert_eq!(
        public_profile["rateLimits"],
        json!({"ok": false, "error": "not_probed"})
    );
    assert_eq!(public_profile["models"], profile["models"]);
    assert!(!public_profile
        .to_string()
        .contains("secret-that-must-not-be-returned"));

    let unknown_session_id = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    let unknown_selection = json!({
        "profile_id": "fixture",
        "model": "fixture-model",
        "thinking": "low"
    });
    let unknown_session_requests = [
        (
            "read session",
            agent.client().get(format!(
                "{}/sessions/{unknown_session_id}",
                agent.base_url()
            )),
        ),
        (
            "delete session",
            agent.client().delete(format!(
                "{}/sessions/{unknown_session_id}",
                agent.base_url()
            )),
        ),
        (
            "append mailbox",
            agent
                .client()
                .post(format!(
                    "{}/sessions/{unknown_session_id}/mailbox",
                    agent.base_url()
                ))
                .json(&json!({"content": "must not create a session"})),
        ),
        (
            "list messages",
            agent.client().get(format!(
                "{}/sessions/{unknown_session_id}/messages",
                agent.base_url()
            )),
        ),
        (
            "stream events",
            agent.client().get(format!(
                "{}/sessions/{unknown_session_id}/events",
                agent.base_url()
            )),
        ),
        (
            "cancel session",
            agent.client().post(format!(
                "{}/sessions/{unknown_session_id}/cancel",
                agent.base_url()
            )),
        ),
        (
            "change selection",
            agent
                .client()
                .put(format!(
                    "{}/sessions/{unknown_session_id}/selection",
                    agent.base_url()
                ))
                .json(&unknown_selection),
        ),
    ];
    for (operation, request) in unknown_session_requests {
        assert_eq!(
            request.send().await.unwrap().status(),
            StatusCode::NOT_FOUND,
            "{operation} must not create an unknown session"
        );
    }
    let sessions_after_unknown_requests: Value = agent
        .client()
        .get(format!("{}/sessions", agent.base_url()))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(sessions_after_unknown_requests, json!({"items": []}));

    let read_profile: Value = agent
        .client()
        .get(format!("{}/profiles/fixture", agent.base_url()))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(read_profile, public_profile);
    let profiles: Value = agent
        .client()
        .get(format!("{}/profiles", agent.base_url()))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(profiles, json!({"items": [public_profile]}));

    let mut replacement = profile.clone();
    replacement["models"][0]["thinking"] = json!(["low"]);
    replacement["models"][0]["default_thinking"] = json!("low");
    let replaced = put_profile(&agent, "fixture", &replacement).await;
    assert_eq!(replaced["models"], replacement["models"]);
    let mut invalid = profile.clone();
    invalid["models"][0]["default_thinking"] = json!("xhigh");
    assert_eq!(
        agent
            .client()
            .put(format!("{}/profiles/invalid", agent.base_url()))
            .json(&invalid)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let open_code_go = json!({
        "provider": "opencode-go",
        "billing": "subscription",
        "auth": {"type": "api_key", "key": "open-code-go-secret"},
        "models": [{
            "id": "muse-spark-1.2-contributor",
            "enabled": true,
            "api": "openai-responses",
            "streaming": true,
            "parallel_tool_calls": false,
            "thinking": ["off", "minimal", "low", "medium", "high", "xhigh"],
            "default_thinking": "xhigh",
            "capabilities": {"input": ["text", "image"]},
            "limits": {"context_window_tokens": 1048576, "max_output_tokens": 131072},
            "default": true
        }]
    });
    let public_open_code = put_profile(&agent, "open-code-go", &open_code_go).await;
    assert_eq!(public_open_code["provider"], "opencode-go");
    assert_eq!(public_open_code["billing"], "subscription");
    assert_eq!(public_open_code["models"], open_code_go["models"]);
    assert!(!public_open_code.to_string().contains("open-code-go-secret"));

    put_profile(&agent, "alternate", &profile).await;
    let workspace_root = TempDir::new().unwrap();
    let session = create_session(&agent, workspace_root.path(), "fixture", "low").await;
    let session_id = session["session_id"].as_str().unwrap();
    assert_eq!(session["profile_id"], "fixture");
    assert_eq!(session["model"], "fixture-model");
    assert_eq!(session["thinking"], "low");
    assert_eq!(session["generation"], 1);
    assert_eq!(session["status"], "wait");
    assert_eq!(session_id.len(), 26);
    assert!(session_id.parse::<ulid::Ulid>().is_ok());
    assert_eq!(session.as_object().unwrap().len(), 8);

    let read_session: Value = agent
        .client()
        .get(format!("{}/sessions/{session_id}", agent.base_url()))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(read_session, session);
    let sessions: Value = agent
        .client()
        .get(format!("{}/sessions", agent.base_url()))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        sessions["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["session_id"] == session_id),
        Some(&json!({"session_id": session_id, "status": "wait"}))
    );

    let changed: Value = agent
        .client()
        .put(format!(
            "{}/sessions/{session_id}/selection",
            agent.base_url()
        ))
        .json(&json!({
            "profile_id": "alternate",
            "model": "fixture-model",
            "thinking": "high"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(changed["profile_id"], "alternate");
    assert_eq!(changed["model"], "fixture-model");
    assert_eq!(changed["thinking"], "high");
    assert_eq!(
        agent
            .client()
            .post(format!("{}/sessions/{session_id}/cancel", agent.base_url()))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );

    assert_eq!(
        agent
            .client()
            .delete(format!("{}/profiles/open-code-go", agent.base_url()))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        agent
            .client()
            .get(format!("{}/profiles/open-code-go", agent.base_url()))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    agent.shutdown().await;

    let mut agent = RealAgent::new().unwrap();
    assert_eq!(
        reqwest::Client::new()
            .get(format!("{}/profiles", agent.base_url()))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let mut runtime_profile = profile_document(agent.provider_base_url());
    runtime_profile["provider"] = json!("openai-compatible");
    let runtime_public = put_profile(&agent, "fixture", &runtime_profile).await;
    assert_eq!(runtime_public["account"]["ok"], true);
    assert_eq!(runtime_public["rateLimits"]["reported"], false);

    let message_workspace = TempDir::new().unwrap();
    let message_session = create_session(&agent, message_workspace.path(), "fixture", "low").await;
    let message_session_id = message_session["session_id"].as_str().unwrap();
    send_mail(&agent, message_session_id, "same").await;
    let first_message_request = agent.request().await;
    send_mail(&agent, message_session_id, "same").await;
    send_mail(&agent, message_session_id, "third").await;
    first_message_request
        .respond_openai_text("chatcmpl-messages-1", "accepted")
        .unwrap();
    agent
        .request()
        .await
        .respond_openai_calls(
            "chatcmpl-messages-2",
            [("provider-call-messages-end", "end", json!({}))],
        )
        .unwrap();
    agent
        .wait_for_state(message_session_id, |state| {
            state.last_turn_outcome == Some(TurnOutcome::Finished)
        })
        .await;
    let settled = read_messages(&agent, message_session_id, "?limit=200").await;
    let mailbox = settled["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "message" && item["role"] == "mailbox")
        .filter_map(|item| item["content"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(mailbox, vec!["same", "same", "third"]);
    assert!(settled["items"].as_array().unwrap().iter().all(|item| {
        let mut keys = item
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        keys.sort_unstable();
        match item["type"].as_str() {
            Some("wait") => keys == ["reason", "type"],
            Some("message") => keys == ["content", "role", "type"],
            _ => false,
        }
    }));
    let mut paged = Vec::new();
    let mut before = None;
    loop {
        let query = before
            .as_ref()
            .map(|cursor: &String| format!("?limit=2&before={cursor}"))
            .unwrap_or_else(|| "?limit=2".into());
        let page = read_messages(&agent, message_session_id, &query).await;
        let mut items = page["items"].as_array().unwrap().clone();
        items.extend(paged);
        paged = items;
        before = page["older_cursor"].as_str().map(str::to_owned);
        if let Some(cursor) = &before {
            assert_eq!(cursor.len(), 16);
        } else {
            break;
        }
    }
    assert_eq!(paged, *settled["items"].as_array().unwrap());

    let stream_workspace = TempDir::new().unwrap();
    let stream_session = create_session(&agent, stream_workspace.path(), "fixture", "low").await;
    let stream_session_id = stream_session["session_id"].as_str().unwrap().to_owned();
    let response = agent
        .client()
        .get(format!(
            "{}/sessions/{stream_session_id}/events?transient=true",
            agent.base_url()
        ))
        .header("accept", "text/event-stream")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let (frames, mut received_frames) = tokio::sync::mpsc::channel(64);
    let pump = tokio::spawn(pump_sse(response, frames));

    send_mail(&agent, &stream_session_id, "stream me").await;
    agent
        .request()
        .await
        .respond_openai_text("chatcmpl-stream-1", "stream me")
        .unwrap();

    let observed = tokio::time::timeout(Duration::from_secs(5), async {
        let mut observed = Vec::new();
        loop {
            let frame = received_frames
                .recv()
                .await
                .expect("SSE stream remains open");
            observed.push(frame);
            let has_delta = observed
                .iter()
                .any(|frame| frame.event == "text_delta" && frame.data["text"] == "stream me");
            let has_step = observed.iter().any(|frame| {
                frame.event == "event"
                    && frame.data["event"]["kind"] == "step_completed"
                    && frame.data["event"]["assistant_text"] == "stream me"
            });
            let has_finish = observed.iter().any(|frame| {
                frame.event == "event" && frame.data["event"]["kind"] == "turn_finished"
            });
            if has_delta && has_step && has_finish {
                return observed;
            }
        }
    })
    .await
    .expect("SSE delivered transient and durable events");
    assert_eq!(observed.first().unwrap().event, "snapshot");
    assert!(observed
        .iter()
        .filter(|frame| frame.event == "event")
        .all(|frame| {
            frame.id.as_ref().is_some_and(|id| {
                id.len() == 16
                    && id.bytes().all(|byte| byte.is_ascii_digit())
                    && frame.data["event_id"] == *id
            })
        }));
    for frame in &observed {
        match frame.event.as_str() {
            DURABLE_EVENT_NAME => {
                serde_json::from_value::<DurableEvent<Value>>(frame.data.clone()).unwrap();
            }
            TEXT_DELTA_EVENT_NAME => {
                serde_json::from_value::<TextDeltaEvent>(frame.data.clone()).unwrap();
            }
            _ => {}
        }
    }
    pump.abort();

    let reconnect_cursor = observed
        .iter()
        .find(|frame| {
            frame.event == "event"
                && frame.data["event"]["kind"] == "step_completed"
                && frame.data["event"]["assistant_text"] == "stream me"
        })
        .and_then(|frame| frame.id.clone())
        .expect("the durable step supplies an SSE reconnect cursor");
    let (reconnect_pump, mut reconnect_frames) =
        open_sse(&agent, &stream_session_id, Some(&reconnect_cursor), false).await;
    let initial = tokio::time::timeout(Duration::from_secs(5), reconnect_frames.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(initial.event, "snapshot");
    let snapshot: SessionSnapshot = serde_json::from_value(initial.data).unwrap();
    assert_eq!(snapshot.session_id, stream_session_id);
    assert!(snapshot
        .cursor
        .as_deref()
        .is_some_and(|id| id >= reconnect_cursor.as_str()));
    assert_eq!(snapshot.aggregates.run_count, 1);
    assert_eq!(snapshot.aggregates.usage.reported_steps, 1);

    send_mail(
        &agent,
        &stream_session_id,
        "durable without transient subscription",
    )
    .await;
    agent
        .request()
        .await
        .respond_openai_text("chatcmpl-durable-only-1", "durable only")
        .unwrap();
    let durable_only = receive_until(&mut reconnect_frames, |frames| {
        let has_step = frames.iter().any(|frame| {
            frame.event == "event"
                && frame.data["event"]["kind"] == "step_completed"
                && frame.data["event"]["assistant_text"] == "durable only"
        });
        let has_finish = frames
            .iter()
            .any(|frame| frame.event == "event" && frame.data["event"]["kind"] == "turn_finished");
        has_step && has_finish
    })
    .await;
    assert!(durable_only.iter().all(|frame| frame.event != "text_delta"));

    let isolated_workspace = TempDir::new().unwrap();
    let isolated = create_session(&agent, isolated_workspace.path(), "fixture", "low").await;
    let isolated_session_id = isolated["session_id"].as_str().unwrap().to_owned();
    let isolated_cursor = agent
        .history(&isolated_session_id, None, 1)
        .unwrap()
        .pop()
        .unwrap()
        .event_id;
    let (isolated_pump, mut isolated_frames) =
        open_sse(&agent, &isolated_session_id, Some(&isolated_cursor), false).await;

    send_mail(&agent, &stream_session_id, "primary isolated input").await;
    send_mail(&agent, &isolated_session_id, "secondary isolated input").await;
    for _ in 0..2 {
        let request = agent.request().await;
        if body_contains(&request, "primary isolated input") {
            request
                .respond_openai_calls(
                    "chatcmpl-primary-isolated",
                    [("provider-call-primary-isolated-end", "end", json!({}))],
                )
                .unwrap();
        } else {
            assert!(body_contains(&request, "secondary isolated input"));
            request
                .respond_openai_calls(
                    "chatcmpl-secondary-isolated",
                    [("provider-call-secondary-isolated-end", "end", json!({}))],
                )
                .unwrap();
        }
    }
    let primary_frames = receive_until(&mut reconnect_frames, |frames| {
        contains_event_text(frames, "primary isolated input") && contains_turn_finished(frames)
    })
    .await;
    let secondary_frames = receive_until(&mut isolated_frames, |frames| {
        contains_event_text(frames, "secondary isolated input") && contains_turn_finished(frames)
    })
    .await;
    assert!(primary_frames
        .iter()
        .all(|frame| !frame.data.to_string().contains("secondary isolated input")));
    assert!(secondary_frames
        .iter()
        .all(|frame| !frame.data.to_string().contains("primary isolated input")));
    reconnect_pump.abort();
    isolated_pump.abort();

    agent.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
// Contract: docs/design/agent-runtime.md [SSE-01]
async fn sse_lag_replaces_the_snapshot_without_querying_history() {
    let mut world = TestWorld::with_options(ServiceOptions {
        live_event_capacity: 1,
        ..ServiceOptions::default()
    });
    let session_id = world
        .create_session(
            Selection {
                profile_id: "test-profile".into(),
                model: "test-model".into(),
                thinking: "medium".into(),
            },
            None,
            "/virtual/sse-lag",
        )
        .await
        .unwrap();
    let data = TempDir::new().unwrap();
    let agent = zork_agent::Agent {
        service: world.service_handle(),
        profiles: Arc::new(zork_agent::ProfileStore::open(
            data.path().to_owned(),
            true,
            true,
        )),
    };
    let scans = world.query.history_scan_calls(&session_id);
    let stream = agent
        .events(session_id.clone(), None, EventQuery { transient: false })
        .await
        .unwrap();
    futures_util::pin_mut!(stream);
    let LiveSessionEvent::Snapshot(initial) = stream.next().await.unwrap().unwrap() else {
        panic!("the first observation must be a snapshot");
    };
    assert!(initial.aggregates.complete);
    assert_eq!(initial.aggregates.run_count, 0);

    // Hold this subscriber still while more than one durable commit overflows
    // its channel. The next poll must restore current state without replay.
    for content in ["lag-one", "lag-two", "lag-three"] {
        world.send_mail(&session_id, content).await.unwrap();
    }
    let pending = world.request().await;
    let current = world.state(&session_id).await.unwrap();
    let next = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let LiveSessionEvent::Snapshot(reset) = next else {
        panic!("lag must start a fresh snapshot");
    };
    assert!(reset.cursor > initial.cursor);
    assert_eq!(reset.cursor, current.overview.cursor);
    assert_eq!(reset.aggregates.run_count, 1);
    assert!(reset.execution.active_step.is_some());
    assert_eq!(world.query.history_scan_calls(&session_id), scans);
    drop(pending);
    world.shutdown().await;
}

async fn open_sse(
    agent: &RealAgent,
    session_id: &str,
    cursor: Option<&str>,
    transient: bool,
) -> (
    tokio::task::JoinHandle<()>,
    tokio::sync::mpsc::Receiver<SseFrame>,
) {
    let mut request = agent
        .client()
        .get(format!(
            "{}/sessions/{session_id}/events?transient={transient}",
            agent.base_url()
        ))
        .header("accept", "text/event-stream");
    if let Some(cursor) = cursor {
        request = request.header("last-event-id", cursor);
    }
    let response = request.send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let (frames, received) = tokio::sync::mpsc::channel(64);
    (tokio::spawn(pump_sse(response, frames)), received)
}

async fn receive_until(
    frames: &mut tokio::sync::mpsc::Receiver<SseFrame>,
    complete: impl Fn(&[SseFrame]) -> bool,
) -> Vec<SseFrame> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut observed = Vec::new();
        loop {
            observed.push(frames.recv().await.expect("SSE stream remains open"));
            if complete(&observed) {
                return observed;
            }
        }
    })
    .await
    .expect("SSE delivered the expected durable events")
}

fn contains_event_text(frames: &[SseFrame], text: &str) -> bool {
    frames
        .iter()
        .any(|frame| frame.event == "event" && frame.data.to_string().contains(text))
}

fn contains_turn_finished(frames: &[SseFrame]) -> bool {
    frames
        .iter()
        .any(|frame| frame.event == "event" && frame.data["event"]["kind"] == "turn_finished")
}

async fn pump_sse(response: reqwest::Response, frames: tokio::sync::mpsc::Sender<SseFrame>) {
    let mut bytes = response.bytes_stream();
    let mut buffer = String::new();
    while let Some(chunk) = bytes.next().await {
        let Ok(chunk) = chunk else {
            return;
        };
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        buffer = buffer.replace("\r\n", "\n");
        while let Some(boundary) = buffer.find("\n\n") {
            let frame = buffer[..boundary].to_owned();
            buffer.drain(..boundary + 2);
            let mut event = "message".to_owned();
            let mut id = None;
            let mut data = None;
            for line in frame.lines() {
                if let Some(value) = line.strip_prefix("event: ") {
                    event = value.to_owned();
                } else if let Some(value) = line.strip_prefix("id: ") {
                    id = Some(value.to_owned());
                } else if let Some(value) = line.strip_prefix("data: ") {
                    data = serde_json::from_str(value).ok();
                }
            }
            if let Some(data) = data {
                if frames.send(SseFrame { event, id, data }).await.is_err() {
                    return;
                }
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn idempotent_creation_retries_incomplete_disk_state_without_overwriting_corruption() {
    use zork_agent::session::event_id::EventId;
    let mut agent = RealAgent::fake_with_token(None).unwrap();
    put_profile(
        &agent,
        "fixture",
        &profile_document(agent.provider_base_url()),
    )
    .await;
    let workspace = TempDir::new().unwrap();
    for (index, tail) in [None, Some("{\"torn\""), Some("not-json\n")]
        .into_iter()
        .enumerate()
    {
        let id = ulid::Ulid::from(1000 + index as u128);
        let segments = agent
            .data_root()
            .join("shared-files/sessions")
            .join(id.to_string())
            .join("segments");
        std::fs::create_dir_all(&segments).unwrap();
        let path = segments.join(format!("{}.jsonl", EventId::from_sequence(id, 1).unwrap()));
        if let Some(tail) = tail {
            std::fs::write(&path, tail).unwrap();
        }
        let url = format!("{}/sessions/{id}", agent.base_url());
        let body = json!({"profile_id":"fixture", "model":"fixture-model", "thinking":"low", "workspace":workspace.path()});
        for _ in 0..2 {
            let response = agent
                .client()
                .put(&url)
                .json(&body)
                .timeout(Duration::from_secs(3))
                .send()
                .await
                .unwrap();
            if index == 2 {
                assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
                assert_eq!(std::fs::read_to_string(&path).unwrap(), tail.unwrap());
            } else {
                assert_eq!(
                    response.status(),
                    StatusCode::CREATED,
                    "{}",
                    response.text().await.unwrap()
                );
                let history = agent.history(&id.to_string(), None, 100).unwrap();
                assert_eq!(
                    history
                        .iter()
                        .filter(|event| matches!(event.event, SessionEvent::SessionCreated { .. }))
                        .count(),
                    1
                );
            }
        }
    }
    agent.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn snapshot_reports_runtime_and_usage_while_history_is_details_only() {
    let mut agent = RealAgent::new().unwrap();
    put_profile(
        &agent,
        "fixture",
        &profile_document(agent.provider_base_url()),
    )
    .await;
    let workspace = TempDir::new().unwrap();
    let session = create_session(&agent, workspace.path(), "fixture", "low").await;
    let id = session["session_id"].as_str().unwrap();
    let (pump, mut frames) = open_sse(&agent, id, None, false).await;
    let before = tokio::time::timeout(Duration::from_secs(5), frames.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.event, "snapshot");
    assert_eq!(before.data["runtime"]["model"], "fixture-model");
    assert_eq!(before.data["runtime"]["thinking"], "low");
    assert_eq!(before.data["runtime"]["context_limit"], 256000);
    assert!(before.data["runtime"]["context_tokens"].is_null());
    assert_eq!(before.data["runtime"]["profile"]["profile_id"], "fixture");
    assert!(!before
        .data
        .to_string()
        .contains("secret-that-must-not-be-returned"));
    send_mail(&agent, id, "Report actual usage").await;
    agent
        .request()
        .await
        .respond_openai_text(
            "usage",
            "A long response must not be estimated into the API input count",
        )
        .unwrap();
    let observed = receive_until(&mut frames, |frames| {
        frames.iter().any(|frame| {
            frame.event == "session_updated" && frame.data["runtime"]["context_tokens"] == 20
        })
    })
    .await;
    let updated = observed
        .iter()
        .rev()
        .find(|frame| frame.event == "session_updated")
        .unwrap();
    assert_eq!(updated.data["aggregates"]["usage"]["input"], 20);
    assert_eq!(updated.data["aggregates"]["usage"]["reported_steps"], 1);
    let page: Value = agent
        .client()
        .get(format!(
            "{}/sessions/{id}/history?limit=1",
            agent.base_url()
        ))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(page.get("runtime").is_none());
    assert!(page.get("snapshot").is_none());
    assert_eq!(page["items"].as_array().unwrap().len(), 1);

    // Pending content leaves the provider-reported input count unchanged.
    send_mail(&agent, id, &"new content ".repeat(100)).await;
    let pending = agent.request().await;
    let active = receive_until(&mut frames, |frames| {
        frames.iter().any(|frame| {
            frame.event == "session_updated" && frame.data["execution"]["active_step"].is_object()
        })
    })
    .await;
    assert_eq!(active.last().unwrap().data["runtime"]["context_tokens"], 20);
    pending.respond_openai_text("next", "done").unwrap();
    pump.abort();
    agent.shutdown().await;
}
