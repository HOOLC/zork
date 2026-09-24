//! The public embedded API is exercised without an HTTP server or HTTP adapter dependency.
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::json;
use zork_agent::session::service::{LiveSessionEvent, ServiceOptions};
use zork_agent::{AgentOptions, AgentRuntime};
use zork_agent_api::{
    ApiErrorCode, CreateSessionRequest, EventQuery, HistoryQuery, MailboxRequest, MessageQuery,
    SessionStatus,
};

fn options(root: &std::path::Path) -> AgentOptions {
    AgentOptions {
        data_root: root.to_owned(),
        fake_agent: true,
        service: ServiceOptions {
            live_event_capacity: 2,
            ..Default::default()
        },
        // Keep the owned refresh task enabled to exercise its shutdown path too.
        profile_refresh_interval: Some(Duration::from_secs(60)),
        ..Default::default()
    }
}

fn request(root: &std::path::Path) -> CreateSessionRequest {
    CreateSessionRequest {
        profile_id: "fixture".into(),
        model: "model".into(),
        thinking: "high".into(),
        system_prompt: None,
        workspace: Some(root.to_string_lossy().into_owned()),
        context: None,
    }
}

async fn fixture(root: &std::path::Path) -> AgentRuntime {
    let runtime = AgentRuntime::start(options(root)).unwrap();
    runtime.agent().put_profile("fixture".into(), serde_json::from_value(json!({
        "provider": "openai-compatible", "billing": "usage", "base_url": "https://example.invalid/v1",
        "models": [{ "id": "model", "api": "openai-completions", "thinking": ["high"],
            "default_thinking": "high", "capabilities": {"input": ["text"]},
            "limits": {"context_window_tokens": 100000, "max_output_tokens": 10000}, "default": true }]
    })).unwrap()).await.unwrap();
    runtime
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prepared_storage_keeps_one_writer_and_rejects_another_data_root() {
    let root = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let path = root.path().to_owned();
    // Storage preparation does not depend on a Tokio context on this worker.
    let prepared = std::thread::spawn(move || AgentRuntime::prepare(&path))
        .join()
        .unwrap()
        .unwrap();
    assert!(AgentRuntime::prepare(root.path()).is_err());
    assert!(prepared.start(options(other.path())).is_err());
    assert!(!other.path().join(".zork-agent.lock").exists());

    let prepared = AgentRuntime::prepare(root.path()).unwrap();
    let mut runtime = prepared.start(options(root.path())).unwrap();
    assert!(AgentRuntime::start(options(root.path())).is_err());
    runtime.shutdown().await;
    drop(runtime);
    // Neither an abandoned preparation nor an orderly runtime retains the lock.
    drop(AgentRuntime::prepare(root.path()).unwrap());
    assert!(AgentRuntime::prepare(root.path()).is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
// Contract: docs/design/agent-runtime.md [EMBED-01, EMBED-02, PERSIST-01]
async fn embedded_execution_validates_requests_and_releases_storage_on_shutdown() {
    let root = tempfile::tempdir().unwrap();
    let mut runtime = fixture(root.path()).await;
    // A second writer is rejected while this instance owns the data root.
    assert!(AgentRuntime::start(options(root.path())).is_err());
    let mut invalid = request(root.path());
    invalid.model = "missing".into();
    assert_eq!(
        runtime
            .agent()
            .create_session(invalid)
            .await
            .unwrap_err()
            .code,
        ApiErrorCode::SelectionUnavailable
    );
    let session = runtime
        .agent()
        .create_session(request(root.path()))
        .await
        .unwrap();
    assert_eq!(
        runtime
            .agent()
            .append_mailbox_id(
                session.session_id.clone(),
                "bad/id".into(),
                MailboxRequest {
                    content: "hello".into()
                }
            )
            .await
            .unwrap_err()
            .code,
        ApiErrorCode::InvalidRequest
    );
    runtime
        .agent()
        .append_mailbox_id(
            session.session_id.clone(),
            "request-1".into(),
            MailboxRequest {
                content: "embedded hello".into(),
            },
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if runtime
                .agent()
                .get_session(session.session_id.clone())
                .await
                .unwrap()
                .status
                == SessionStatus::Finished
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let messages = runtime
        .agent()
        .list_messages(session.session_id.clone(), MessageQuery::default())
        .await
        .unwrap();
    assert!(messages
        .items
        .iter()
        .any(|m| m.content.contains("embedded hello")));
    runtime.shutdown().await;
    runtime.shutdown().await;
    drop(runtime);
    // No process exit or new Tokio runtime is needed to release the writer lock.
    let mut restarted = AgentRuntime::start(options(root.path())).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if restarted.agent().service.contains(&session.session_id) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let recovered = restarted
        .agent()
        .get_session(session.session_id)
        .await
        .unwrap();
    assert_eq!(recovered.workspace, root.path().to_string_lossy());
    restarted.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
// Contract: docs/design/agent-runtime.md [EMBED-01, SSE-01, SSE-02]
async fn embedded_events_reset_snapshot_after_lag_and_reconnect_without_history_replay() {
    let root = tempfile::tempdir().unwrap();
    let mut runtime = fixture(root.path()).await;
    let id = runtime
        .agent()
        .create_session(request(root.path()))
        .await
        .unwrap()
        .session_id;
    let mut events = Box::pin(
        runtime
            .agent()
            .events(id.clone(), None, EventQuery::default())
            .await
            .unwrap(),
    );
    let first = events.next().await.unwrap().unwrap();
    let LiveSessionEvent::Snapshot(initial) = first else {
        panic!("subscription must start with a snapshot")
    };
    assert_eq!(initial.session_id, id);
    let old_cursor = initial.cursor.clone();

    // Overflow the paused consumer. The new contract resets to a current bounded
    // snapshot instead of replaying each historical durable event.
    for keep_recent_tokens in 1..=12 {
        runtime
            .agent()
            .set_context(
                id.clone(),
                zork_agent_api::ContextConfig {
                    keep_recent_tokens,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }
    let history = runtime
        .agent()
        .list_history(id.clone(), HistoryQuery::default())
        .await
        .unwrap();
    let recovered = tokio::time::timeout(Duration::from_secs(5), events.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let LiveSessionEvent::Snapshot(recovered) = recovered else {
        panic!("lag must reset the snapshot baseline")
    };
    assert_eq!(recovered.cursor, history.latest_cursor);
    assert!(recovered.cursor > old_cursor);

    runtime
        .agent()
        .set_context(
            id.clone(),
            zork_agent_api::ContextConfig {
                keep_recent_tokens: 13,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let next = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let LiveSessionEvent::Durable(event) = events.next().await.unwrap().unwrap() {
                break event;
            }
        }
    })
    .await
    .unwrap();
    assert!(Some(&next.event_id) > recovered.cursor.as_ref());
    drop(events);

    // Even an old Last-Event-ID receives a fresh snapshot, then future events.
    let mut resumed = Box::pin(
        runtime
            .agent()
            .events(id.clone(), old_cursor, EventQuery::default())
            .await
            .unwrap(),
    );
    let current = resumed.next().await.unwrap().unwrap();
    let LiveSessionEvent::Snapshot(current) = current else {
        panic!("reconnection must start with a snapshot")
    };
    assert_eq!(current.cursor.as_ref(), Some(&next.event_id));
    runtime
        .agent()
        .set_context(
            id.clone(),
            zork_agent_api::ContextConfig {
                keep_recent_tokens: 14,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let after_reconnect = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let LiveSessionEvent::Durable(event) = resumed.next().await.unwrap().unwrap() {
                break event;
            }
        }
    })
    .await
    .unwrap();
    assert!(Some(&after_reconnect.event_id) > current.cursor.as_ref());
    drop(resumed);
    runtime.shutdown().await;
    drop(runtime);
    let mut reopened = AgentRuntime::start(options(root.path())).unwrap();
    reopened.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embedded_runtime_provisions_bundled_skills_and_supplies_catalog() {
    use zork_agent::session::state::GenerationEntry;
    let root = tempfile::tempdir().unwrap();
    let user = root.path().join("skills/local-notes");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(
        user.join("SKILL.md"),
        "---\nname: local-notes\ndescription: Keep notes locally\n---\nbody",
    )
    .unwrap();
    let mut runtime = fixture(root.path()).await;
    let bundled = root
        .path()
        .join("skills/bundled/device-onboarding/SKILL.md");
    assert!(std::fs::metadata(&bundled)
        .unwrap()
        .permissions()
        .readonly());
    let session = runtime
        .agent()
        .create_session(request(root.path()))
        .await
        .unwrap();
    runtime
        .agent()
        .append_mailbox_id(
            session.session_id.clone(),
            "request-1".into(),
            MailboxRequest {
                content: "hello".into(),
            },
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let state = runtime
                .agent()
                .service
                .state(&session.session_id)
                .await
                .unwrap();
            let catalog = state
                .generation
                .entries
                .iter()
                .find_map(|entry| match entry {
                    GenerationEntry::Notice { message }
                        if message.starts_with(zork_agent::skills::CATALOG_NOTICE) =>
                    {
                        Some(message.clone())
                    }
                    _ => None,
                });
            if let Some(catalog) = catalog {
                assert!(catalog.contains("Keep notes locally"));
                assert!(catalog.contains("device-onboarding (bundled)"));
                assert!(!catalog.contains("body"));
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    runtime.shutdown().await;
}
