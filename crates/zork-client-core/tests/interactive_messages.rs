//! Actual core submission, durable delivery and cache projection over HTTP.
use axum::{
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use zork_client_core::{
    api::{MessagePage, StationClient, TranscriptMessage},
    interactions::*,
    state::Device,
    store::ClientStore,
    transcript::TranscriptLine,
};

#[derive(Default)]
struct ServerState {
    messages: Vec<Value>,
    submissions: Vec<String>,
    effects: usize,
    lose_first_reply: bool,
    reject_first: bool,
    reject_first_read: bool,
}

fn request() -> Value {
    json!({"type":"message","id":"request","chat_id":"chat","role":"assistant","content":"Please supply a value",
        "interaction":MessageContent::linked("owner/request".into(),AGENT_CONFIGURATION.into(), Request::Input { title:"Question".into(), fields:vec![Field {
            id:"answer".into(), label:"Answer".into(), kind:FieldKind::Text, required:true, default:String::new(), options:vec![],
        }] }, None)})
}

async fn server(
    lose_first_reply: bool,
) -> (String, Arc<Mutex<ServerState>>, tokio::task::JoinHandle<()>) {
    let state = Arc::new(Mutex::new(ServerState {
        messages: vec![request()],
        lose_first_reply,
        ..Default::default()
    }));
    let writer = state.clone();
    let reader = state.clone();
    let router = Router::new()
        .route("/v1/node/chats/chat/messages/request/agent-configuration", post(move |Json(response): Json<Response>| {
            let state = writer.clone();
            async move {
                let mut state = state.lock().unwrap();
                state.submissions.push(response.response_id.clone());
                if state.reject_first {
                    state.reject_first = false;
                    return (StatusCode::BAD_REQUEST, Json(json!({"error":"agent_configuration_conflict"}))).into_response();
                }
                if state.effects == 0 {
                    state.effects += 1;
                    // This message is between the client's old tail and the
                    // HTTP response. The delivery pump must not skip it.
                    state.messages.push(json!({"type":"message","id":"middle","role":"assistant","content":"A concurrent message"}));
                    state.messages.push(json!({"type":"message","id":"result","chat_id":"chat","role":"assistant","author_kind":"system","content":"Input accepted",
                        "reply_to":"request","interaction":MessageContent::linked_result("owner/request".into(),AGENT_CONFIGURATION.into(), Resolution {
                            request_message_id:"request".into(), response_id:response.response_id, revision:1,
                            outcome:if response.accept {Outcome::Completed} else {Outcome::Declined}, actor:"user".into(), output:json!({"values":response.values}),
                        })}));
                }
                if state.lose_first_reply {
                    state.lose_first_reply = false;
                    return (StatusCode::BAD_GATEWAY, Json(json!({"error":"reply lost after commit"}))).into_response();
                }
                Json(json!({"message":state.messages.last()})).into_response()
            }
        }))
        .route("/v1/im/sessions/chat/messages", get(move || {
            let state = reader.clone();
            async move {
                let mut state = state.lock().unwrap();
                if state.reject_first_read {
                    state.reject_first_read = false;
                    return (StatusCode::BAD_REQUEST, Json(json!({"error":"read unavailable after acceptance"}))).into_response();
                }
                Json(json!({"items":state.messages,"older_cursor":null})).into_response()
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    (
        url,
        state,
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        }),
    )
}

fn core(url: &str, store: Arc<ClientStore>) -> Arc<Device> {
    Device::open(
        Arc::new(StationClient::new(url, None)),
        Some((store, "node".into())),
        false,
    )
}

async fn wait(check: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("core projection did not settle");
}

fn card(conversation: &zork_client_core::state::Conversation) -> Card {
    let state = conversation.snapshot();
    let TranscriptLine::Message { metadata, .. } = &state.lines[0];
    metadata.interaction_view.as_deref().unwrap().clone()
}

#[tokio::test]
async fn interaction_submission_updates_the_card_and_keeps_intermediate_source_messages() {
    let (url, server, task) = server(false).await;
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(ClientStore::open(root.path()).unwrap());
    store
        .cache_message_page(
            "node",
            "chat",
            &MessagePage {
                source_epoch: None,
                items: vec![serde_json::from_value(request()).unwrap()],
                older_cursor: None,
            },
            None,
        )
        .unwrap();
    let device = core(&url, store.clone());
    let conversation = device.conversation("chat");
    assert!(card(&conversation).editable);
    conversation
        .respond_to_interaction(Command::Submit {
            message_id: "request".into(),
            values: [("answer".into(), "chosen value".into())].into(),
        })
        .unwrap();
    wait(|| card(&conversation).status_key == "interaction_completed").await;
    let resolved = card(&conversation);
    assert!(resolved.actions.is_empty());
    assert_eq!(resolved.fields[0].value, "chosen value");
    let page = store
        .cached_messages("node", "chat", None, 100)
        .unwrap()
        .unwrap();
    let ids: Vec<_> = page
        .items
        .iter()
        .map(|m| {
            let TranscriptMessage::Message { metadata, .. } = m;
            metadata.id.clone().unwrap()
        })
        .collect();
    assert_eq!(ids, ["request", "middle", "result"]);
    conversation
        .respond_to_interaction(Command::Submit {
            message_id: "request".into(),
            values: BTreeMap::new(),
        })
        .unwrap();
    assert_eq!(server.lock().unwrap().submissions.len(), 1);
    assert_eq!(server.lock().unwrap().effects, 1);
    task.abort();
}

#[tokio::test]
async fn interaction_invalid_form_never_posts_and_reports_field_errors_in_core() {
    let (url, server, task) = server(false).await;
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(ClientStore::open(root.path()).unwrap());
    store
        .cache_message_page(
            "node",
            "chat",
            &MessagePage {
                source_epoch: None,
                items: vec![serde_json::from_value(request()).unwrap()],
                older_cursor: None,
            },
            None,
        )
        .unwrap();
    let device = core(&url, store);
    let conversation = device.conversation("chat");
    conversation
        .respond_to_interaction(Command::Activate {
            message_id: "request".into(),
            choice: "submit".into(),
            values: BTreeMap::new(),
        })
        .unwrap();
    let invalid = card(&conversation);
    assert_eq!(
        invalid.fields[0].error_key.as_deref(),
        Some("input_required")
    );
    assert!(invalid.editable);
    assert!(server.lock().unwrap().submissions.is_empty());
    task.abort();
}

#[tokio::test]
async fn interaction_uncertain_response_survives_restart_and_recovery_keeps_its_identity() {
    let (url, server, task) = server(true).await;
    let root = tempfile::tempdir().unwrap();
    {
        let store = Arc::new(ClientStore::open(root.path()).unwrap());
        store
            .cache_message_page(
                "node",
                "chat",
                &MessagePage {
                    source_epoch: None,
                    items: vec![serde_json::from_value(request()).unwrap()],
                    older_cursor: None,
                },
                None,
            )
            .unwrap();
        let device = core(&url, store);
        let conversation = device.conversation("chat");
        conversation
            .respond_to_interaction(Command::Submit {
                message_id: "request".into(),
                values: [("answer".into(), "stable input".into())].into(),
            })
            .unwrap();
        wait(|| card(&conversation).status_key == "interaction_unconfirmed").await;
        assert_eq!(server.lock().unwrap().effects, 1);
    }
    let store = Arc::new(ClientStore::open(root.path()).unwrap());
    let device = core(&url, store);
    let conversation = device.conversation("chat");
    assert_eq!(card(&conversation).actions[0].id, "retry");
    conversation
        .respond_to_interaction(Command::Retry {
            message_id: "request".into(),
        })
        .unwrap();
    wait(|| card(&conversation).status_key == "interaction_completed").await;
    let state = server.lock().unwrap();
    assert_eq!(state.effects, 1);
    assert_eq!(state.submissions.len(), 2);
    assert_eq!(state.submissions[0], state.submissions[1]);
    task.abort();
}

#[tokio::test]
async fn interaction_rejected_response_can_be_changed_after_restart_without_reusing_the_attempt() {
    let (url, server, task) = server(false).await;
    server.lock().unwrap().reject_first = true;
    let root = tempfile::tempdir().unwrap();
    {
        let store = Arc::new(ClientStore::open(root.path()).unwrap());
        store
            .cache_message_page(
                "node",
                "chat",
                &MessagePage {
                    source_epoch: None,
                    items: vec![serde_json::from_value(request()).unwrap()],
                    older_cursor: None,
                },
                None,
            )
            .unwrap();
        let device = core(&url, store);
        let conversation = device.conversation("chat");
        conversation
            .respond_to_interaction(Command::Submit {
                message_id: "request".into(),
                values: [("answer".into(), "original input".into())].into(),
            })
            .unwrap();
        wait(|| card(&conversation).status_key == "interaction_submission_failed").await;
        assert_eq!(server.lock().unwrap().effects, 0);
        assert!(card(&conversation).editable);
    }
    let store = Arc::new(ClientStore::open(root.path()).unwrap());
    let device = core(&url, store);
    let conversation = device.conversation("chat");
    let rejected = card(&conversation);
    assert_eq!(rejected.status_key, "interaction_submission_failed");
    assert_eq!(rejected.fields[0].value, "original input");
    assert!(rejected.actions.iter().any(|action| action.id == "decline"));
    conversation
        .respond_to_interaction(Command::Decline {
            message_id: "request".into(),
        })
        .unwrap();
    wait(|| card(&conversation).status_key == "interaction_declined").await;
    let state = server.lock().unwrap();
    assert_eq!(state.submissions.len(), 2);
    assert_ne!(state.submissions[0], state.submissions[1]);
    task.abort();
}

#[tokio::test]
async fn interaction_accepted_response_recovers_its_stream_after_restart_without_posting_again() {
    let (url, server, task) = server(false).await;
    server.lock().unwrap().reject_first_read = true;
    let root = tempfile::tempdir().unwrap();
    {
        let store = Arc::new(ClientStore::open(root.path()).unwrap());
        store
            .cache_message_page(
                "node",
                "chat",
                &MessagePage {
                    source_epoch: None,
                    items: vec![serde_json::from_value(request()).unwrap()],
                    older_cursor: None,
                },
                None,
            )
            .unwrap();
        let device = core(&url, store);
        let conversation = device.conversation("chat");
        conversation
            .respond_to_interaction(Command::Submit {
                message_id: "request".into(),
                values: [("answer".into(), "accepted input".into())].into(),
            })
            .unwrap();
        wait(|| card(&conversation).status_key == "interaction_unconfirmed").await;
        assert!(!card(&conversation).editable);
    }
    let store = Arc::new(ClientStore::open(root.path()).unwrap());
    let device = core(&url, store);
    let conversation = device.conversation("chat");
    conversation
        .respond_to_interaction(Command::Retry {
            message_id: "request".into(),
        })
        .unwrap();
    wait(|| card(&conversation).status_key == "interaction_completed").await;
    assert_eq!(server.lock().unwrap().submissions.len(), 1);
    assert_eq!(server.lock().unwrap().effects, 1);
    task.abort();
}
