use axum::{
    extract::Path,
    routing::{delete, get, post},
    Json, Router,
};
use serde_json::json;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use zork_client_core::{
    api::StationClient,
    state::{Device, DraftAction, Profiles},
    store::ClientStore,
};

async fn serve(router: Router) -> (Arc<StationClient>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = Arc::new(StationClient::new(
        format!("http://{}", listener.local_addr().unwrap()),
        None,
    ));
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (client, server)
}

#[tokio::test]
async fn cancelled_authorization_start_cleans_up_late_attempt_without_replacing_the_new_one() {
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let count = Arc::new(AtomicUsize::new(0));
    let deleted = Arc::new(Mutex::new(Vec::new()));
    let router = Router::new().route("/v1/node/auth", post({
        let (started, release, count) = (started.clone(), release.clone(), count.clone());
        move || { let (started, release, count) = (started.clone(), release.clone(), count.clone()); async move {
            let index = count.fetch_add(1, Ordering::SeqCst);
            if index == 0 { started.notify_one(); release.notified().await; }
            Json(json!({"id":format!("attempt-{index}"),"flow":"browser_callback","url":"https://example.test/login"}))
        }}
    })).route("/v1/node/auth/{id}", delete({
        let deleted = deleted.clone();
        move |Path(id): Path<String>| { deleted.lock().unwrap().push(id); async { Json(json!({})) } }
    }));
    let (client, server) = serve(router).await;
    let source = Profiles::new(client);
    let pending = tokio::spawn({
        let source = source.clone();
        async move {
            source
                .start_authorization("old".into(), "test".into(), "subscription".into())
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .unwrap();
    source.cancel_authorization().await.unwrap();
    source
        .start_authorization("new".into(), "test".into(), "subscription".into())
        .await
        .unwrap();
    release.notify_one();
    assert!(pending.await.unwrap().is_err());
    let state = source.snapshot();
    assert_eq!(state.authorization.as_ref().unwrap()["id"], "attempt-1");
    assert!(!state.authorization_busy);
    assert!(!state.authorization_complete);
    assert!(state.authorization_error.is_none());
    assert_eq!(*deleted.lock().unwrap(), vec!["attempt-0"]);
    server.abort();
}

#[tokio::test]
async fn detail_refresh_replaces_every_business_field_and_publishes_the_record() {
    let current = json!({"profile_id":"fixture","provider":"openai","billing":"usage","name":"renamed", "base_url":"https://models.example.test", "checkedAt":chrono::Utc::now().to_rfc3339(), "account":{"ok":true}, "models":[{"id":"new-model","enabled":false,"api":"openai-responses","limits":{"context_window_tokens":64000,"max_output_tokens":8000},"default":true}]});
    let router = Router::new().route(
        "/v1/node/profiles/fixture",
        get({
            let current = current.clone();
            move || {
                let current = current.clone();
                async { Json(current) }
            }
        }),
    );
    let (client, server) = serve(router).await;
    let source = Profiles::new(client);
    let mut updates = source.subscribe();
    updates.snapshot();
    let old = json!({"profile_id":"fixture","provider":"old","name":"old","models":[{"id":"removed-model"}]});
    source.open_detail("fixture").await.unwrap();
    let update = tokio::time::timeout(Duration::from_secs(5), updates.changed())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(update.records, vec!["fixture"]);
    let detail = source.detail(old);
    for key in ["provider", "billing", "name", "base_url", "account"] {
        assert_eq!(detail[key], current[key], "{key}");
    }
    assert_eq!(detail["models"][0]["id"], "new-model");
    assert_eq!(
        detail["models"][0]["limits"],
        current["models"][0]["limits"]
    );
    assert_eq!(detail["models"][0]["enabled"], false);
    assert_eq!(detail["verified"], true);
    server.abort();
}

#[test]
fn text_edits_preserve_attachments_across_reopen_and_invalid_input_does_not_mutate_the_draft() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(ClientStore::open(root.path()).unwrap());
    let client = Arc::new(StationClient::new("http://127.0.0.1:9", None));
    let device = Device::open(client.clone(), Some((store.clone(), "node".into())), true);
    device
        .edit_draft_action(
            "chat",
            DraftAction::AttachText {
                name: "notes.txt".into(),
                bytes: b"important context".to_vec(),
            },
        )
        .unwrap();
    device
        .edit_draft_action(
            "chat",
            DraftAction::Edit {
                text: "new message".into(),
            },
        )
        .unwrap();
    let expected = device.draft("chat");
    assert!(device
        .edit_draft_action(
            "chat",
            DraftAction::AttachText {
                name: "bad.txt".into(),
                bytes: vec![0xff]
            }
        )
        .is_err());
    assert_eq!(device.draft("chat"), expected);
    drop(device);
    drop(store);
    let store = Arc::new(ClientStore::open(root.path()).unwrap());
    let reopened = Device::open(client, Some((store, "node".into())), true);
    assert_eq!(reopened.draft("chat"), expected);
    assert_eq!(expected.text, "new message");
    assert_eq!(expected.attachments.len(), 1);
    assert_eq!(expected.attachments[0].content, "important context");
}
