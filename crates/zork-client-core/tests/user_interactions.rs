//! Linked phase ordering and private login actions through the real shared core.
use axum::{extract::State, http::HeaderMap, routing::get, Json, Router};
use serde_json::{json, Value};
use std::{
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

fn phase(outcome: Outcome, revision: u64) -> Resolution {
    Resolution {
        request_message_id: "card".into(),
        response_id: "attempt".into(),
        revision,
        outcome,
        actor: "node/user".into(),
        output: json!({"profile_id":"account"}),
    }
}
fn proposal() -> Request {
    Request::OAuth {
        title: "Connect account".into(),
    }
}
fn card_message(snapshot: Option<Resolution>) -> TranscriptMessage {
    serde_json::from_value(json!({"type":"message","id":"card","chat_id":"chat","role":"assistant","content":"Connect your account",
        "interaction":MessageContent::linked("owner/request".into(),PROVIDER_LOGIN.into(),proposal(),snapshot)})).unwrap()
}
fn result_message(result: Resolution) -> TranscriptMessage {
    serde_json::from_value(json!({"type":"message","id":format!("phase-{}",result.revision),"chat_id":"chat","role":"assistant","author_kind":"system","content":"Account state changed",
        "interaction":MessageContent::linked_result("owner/request".into(),PROVIDER_LOGIN.into(),result)})).unwrap()
}
fn page(items: Vec<TranscriptMessage>) -> MessagePage {
    MessagePage {
        source_epoch: None,
        items,
        older_cursor: None,
    }
}
fn device(url: &str, store: Arc<ClientStore>) -> Arc<Device> {
    Device::open(
        Arc::new(StationClient::new(url, Some("node-token".into()))),
        Some((store, "node".into())),
        false,
    )
}
fn card(conversation: &zork_client_core::state::Conversation) -> Card {
    let state = conversation.snapshot();
    let TranscriptLine::Message { metadata, .. } = &state.lines[0];
    metadata.interaction_view.as_deref().unwrap().clone()
}
async fn wait(check: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(10)).await
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn completed_snapshot_is_never_presented_as_an_actionable_request() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(ClientStore::open(root.path()).unwrap());
    store
        .cache_message_page(
            "node",
            "chat",
            &page(vec![card_message(Some(phase(Outcome::Cancelled, 2)))]),
            None,
        )
        .unwrap();
    let device = device("http://127.0.0.1:1", store);
    let conversation = device.conversation("chat");
    let view = card(&conversation);
    assert_eq!(view.status_key, "interaction_cancelled");
    assert!(!view.editable);
    assert!(view.actions.is_empty());
    conversation
        .respond_to_interaction(Command::ContinueLogin {
            message_id: "card".into(),
        })
        .unwrap();
    assert_eq!(card(&conversation), view);
}

#[tokio::test]
async fn final_result_before_request_survives_restart_and_older_phases() {
    let root = tempfile::tempdir().unwrap();
    {
        let store = ClientStore::open(root.path()).unwrap();
        store
            .cache_message_page(
                "node",
                "chat",
                &MessagePage {
                    source_epoch: None,
                    items: vec![result_message(phase(Outcome::Completed, 2))],
                    older_cursor: Some("phase-2".into()),
                },
                None,
            )
            .unwrap();
    }
    let store = Arc::new(ClientStore::open(root.path()).unwrap());
    store
        .cache_message_page(
            "node",
            "chat",
            &page(vec![card_message(Some(phase(Outcome::Pending, 1)))]),
            Some("phase-2"),
        )
        .unwrap();
    store
        .cache_message_page(
            "node",
            "chat",
            &page(vec![result_message(phase(Outcome::Pending, 1))]),
            None,
        )
        .unwrap();
    let cached = store
        .cached_messages("node", "chat", None, 100)
        .unwrap()
        .unwrap();
    let request = cached
        .items
        .iter()
        .find(|m| {
            let TranscriptMessage::Message { metadata, .. } = m;
            metadata.id.as_deref() == Some("card")
        })
        .unwrap();
    let TranscriptMessage::Message { metadata, .. } = request;
    assert_eq!(
        metadata.interaction_result.as_ref().unwrap().outcome,
        Outcome::Completed
    );
    assert_eq!(
        MessageContent::parse(metadata.interaction.as_deref().unwrap())
            .unwrap()
            .snapshot
            .unwrap()
            .outcome,
        Outcome::Pending
    );
}

#[derive(Default)]
struct PrivateServer {
    callback: Option<String>,
    authorized: bool,
    completed: bool,
}
async fn challenge(
    State(state): State<Arc<Mutex<PrivateServer>>>,
    headers: HeaderMap,
) -> Json<Value> {
    state.lock().unwrap().authorized = headers
        .get("authorization")
        .is_some_and(|v| v == "Bearer node-token");
    Json(
        json!({"authorization":{"verification_url":"https://login.invalid/private-challenge","user_code":"private-device-code","flow":"browser_callback"}}),
    )
}
async fn callback(
    State(state): State<Arc<Mutex<PrivateServer>>>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let mut state = state.lock().unwrap();
    state.callback = body["callback"].as_str().map(str::to_owned);
    state.completed = true;
    Json(json!({"result":phase(Outcome::Completed,2)}))
}
async fn history(State(state): State<Arc<Mutex<PrivateServer>>>) -> Json<Value> {
    let complete = state.lock().unwrap().completed;
    let mut messages = vec![card_message(Some(phase(Outcome::Pending, 1)))];
    if complete {
        messages.push(result_message(phase(Outcome::Completed, 2)));
    }
    Json(json!({"items":messages,"older_cursor":null}))
}
fn assert_no_secret_files(path: &std::path::Path) {
    for entry in std::fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            assert_no_secret_files(&path)
        } else {
            let bytes = std::fs::read(&path).unwrap();
            for secret in [
                "private-challenge",
                "private-device-code",
                "private-oauth-callback",
            ] {
                assert!(
                    !bytes.windows(secret.len()).any(|w| w == secret.as_bytes()),
                    "private material persisted in {}",
                    path.display()
                );
            }
        }
    }
}

#[tokio::test]
async fn login_challenge_and_callback_bypass_public_cache_and_durable_outbox() {
    let state = Arc::new(Mutex::new(PrivateServer::default()));
    let router = Router::new()
        .route(
            "/v1/node/chats/chat/messages/card/provider-login",
            get(challenge).post(callback),
        )
        .route("/v1/im/sessions/chat/messages", get(history))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(ClientStore::open(root.path()).unwrap());
    store
        .cache_message_page(
            "node",
            "chat",
            &page(vec![card_message(Some(phase(Outcome::Pending, 1)))]),
            None,
        )
        .unwrap();
    let device = device(&url, store.clone());
    let conversation = device.conversation("chat");
    conversation
        .respond_to_interaction(Command::ContinueLogin {
            message_id: "card".into(),
        })
        .unwrap();
    wait(|| !card(&conversation).fields.is_empty()).await;
    assert!(state.lock().unwrap().authorized);
    assert!(card(&conversation).fields[0].sensitive);
    assert_eq!(
        conversation
            .interaction_open_url("card", "open_login")
            .as_deref(),
        Some("https://login.invalid/private-challenge")
    );
    let cached = store
        .cached_messages("node", "chat", None, 100)
        .unwrap()
        .unwrap();
    assert!(!serde_json::to_string(&cached.items)
        .unwrap()
        .contains("private-"));
    assert_no_secret_files(root.path());
    conversation
        .respond_to_interaction(Command::LoginCallback {
            message_id: "card".into(),
            callback: "private-oauth-callback".into(),
        })
        .unwrap();
    wait(|| state.lock().unwrap().callback.is_some()).await;
    assert_eq!(
        state.lock().unwrap().callback.as_deref(),
        Some("private-oauth-callback")
    );
    conversation.refresh();
    wait(|| card(&conversation).status_key == "interaction_completed").await;
    assert!(card(&conversation).fields.is_empty());
    assert!(conversation
        .interaction_open_url("card", "open_login")
        .is_none());
    assert_no_secret_files(root.path());
    server.abort();
}
