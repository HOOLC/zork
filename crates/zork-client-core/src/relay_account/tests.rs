use super::*;
use tokio::sync::mpsc;

fn saved(origin: &str, expires: i64) -> RelaySession {
    RelaySession {
        origin: origin.into(),
        token: "test-access".into(),
        refresh_token: "test-refresh".into(),
        subject: "test-google".into(),
        email: None,
        session_id: ulid::Ulid::new().to_string(),
        expires_at: expires,
        session_expires_at: storage::now() + 3600,
        refresh_expires_at: storage::now() + 3600,
        pending_refresh: None,
    }
}
async fn save(root: &Path, current: RelaySession) {
    let account = Account::new(root, &current.origin).unwrap();
    let _lock = account.lock().await.unwrap();
    let file = AccountFile {
        current: Some(current),
        ..Default::default()
    };
    storage::write(root, &file).unwrap();
}
#[tokio::test]
async fn credentials_never_follow_an_origin_change() {
    let root = tempfile::tempdir().unwrap();
    save(
        root.path(),
        saved("https://old.example", storage::now() + 300),
    )
    .await;
    let account = Account::new(root.path(), "https://new.example").unwrap();
    assert!(account.cached_access().unwrap().is_none());
    assert!(account.access(true).await.unwrap().is_none());
    assert!(!account.status(false).await.unwrap().authenticated);
    assert_eq!(
        storage::load(root.path()).unwrap().unwrap().origin,
        "https://old.example"
    );
}
#[tokio::test]
async fn expiry_reaches_transport_while_refresh_is_stalled() {
    let root = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = requests.clone();
    let app = axum::Router::new().fallback(move || {
        let count = count.clone();
        async move {
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            tokio::time::sleep(Duration::from_secs(10)).await;
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        }
    });
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let expires = storage::now() + 2;
    save(root.path(), saved(&origin, expires)).await;
    let account = Account::new(root.path(), &origin).unwrap();
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let task = account
        .maintain(move |access| {
            let sender = sender.clone();
            async move {
                sender.send(access.is_some()).unwrap();
                Ok(())
            }
        })
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await
            .unwrap(),
        Some(true)
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), receiver.recv())
            .await
            .unwrap(),
        Some(false)
    );
    assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 1);
    drop(task);
    server.abort();
}
#[tokio::test]
async fn atomic_login_and_offline_logout_hot_apply_without_idle_polling() {
    let root = tempfile::tempdir().unwrap();
    // Reserve then release an unused loopback port so all remote requests fail.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let account = Account::new(root.path(), &origin).unwrap();
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let task = account
        .maintain(move |access| {
            let sender = sender.clone();
            async move {
                let _ = sender.send(access.is_some());
                Ok(())
            }
        })
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .unwrap(),
        Some(false)
    );
    save(root.path(), saved(&origin, storage::now() + 300)).await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .unwrap(),
        Some(true)
    );
    assert_eq!(account.logout(false).await.unwrap(), 1);
    assert!(storage::load(root.path()).unwrap().is_none());
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .unwrap(),
        Some(false)
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(150), receiver.recv())
            .await
            .is_err(),
        "no duplicate snapshot"
    );
    assert_eq!(
        storage::read(root.path())
            .unwrap()
            .pending_revocations
            .len(),
        1
    );
    drop(task);
}

#[tokio::test]
async fn rejected_logout_does_not_claim_remote_revocation() {
    let root = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new().fallback(|| async {
        (
            axum::http::StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({"error":"invalid_session"})),
        )
    });
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    save(root.path(), saved(&origin, storage::now() + 300)).await;
    let account = Account::new(root.path(), &origin).unwrap();
    assert_eq!(account.logout(false).await.unwrap(), 1);
    assert!(storage::load(root.path()).unwrap().is_none());
    assert_eq!(
        storage::read(root.path())
            .unwrap()
            .pending_revocations
            .len(),
        1
    );
    server.abort();
}

#[tokio::test]
async fn profile_account_is_shared_without_copying_refresh_credentials() {
    let root = tempfile::tempdir().unwrap();
    storage::bind_profile(root.path()).unwrap();
    storage::bind_profile(root.path()).unwrap();
    let origin = "http://127.0.0.1:9";
    let client = Account::new(root.path().join("transport"), origin).unwrap();
    let station = Account::new(root.path().join("node"), origin).unwrap();
    assert_eq!(client.data_root(), station.data_root());
    save(root.path(), saved(origin, storage::now() + 300)).await;
    assert!(client.cached_access().unwrap().is_some());
    assert!(station.cached_access().unwrap().is_some());
    client.logout(false).await.unwrap();
    assert!(station.cached_access().unwrap().is_none());
    assert!(!storage::path(&root.path().join("node")).exists());
    assert!(!storage::path(&root.path().join("transport")).exists());
}

#[tokio::test]
async fn account_controller_cancellation_fences_a_late_device_start_and_observers_see_no_tokens() {
    use axum::{routing::post, Json, Router};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    let root = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let started = Arc::new(tokio::sync::Notify::new());
    let released = Arc::new(tokio::sync::Notify::new());
    let cancelled = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route("/v1/auth/device", post({
        let (origin, started, released) = (origin.clone(), started.clone(), released.clone());
        move |Json(body): Json<serde_json::Value>| {
            let (origin, started, released) = (origin.clone(), started.clone(), released.clone());
            async move {
                started.notify_one(); released.notified().await;
                Json(serde_json::json!({"verification_uri":format!("{origin}/v1/auth/device/{}",body["id"].as_str().unwrap()),"expires_at":storage::now()+300,"interval":3}))
            }
        }
    })).route("/v1/auth/device/cancel", post({
        let cancelled = cancelled.clone();
        move || { let cancelled = cancelled.clone(); async move { cancelled.fetch_add(1,Ordering::SeqCst); Json(serde_json::json!({"cancelled":true})) } }
    }));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let controller =
        controller::Controller::new(Account::new(root.path(), &origin).unwrap()).unwrap();
    let mut wire = crate::subscriptions::WireSubscription::from_account(&controller.source);
    let first = wire.prepare().unwrap().unwrap();
    assert_eq!(first["snapshot"]["authenticated"], false);
    assert!(wire.finish(first["batch"].as_u64().unwrap(), false));
    controller.submit(controller::Action::Login).unwrap();
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    controller.submit(controller::Action::Cancel).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while storage::read(root.path()).unwrap().login_attempt.is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    released.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        while cancelled.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let state = controller.snapshot();
    assert!(!state.busy());
    assert!(state.login_url.is_none());
    assert!(!state.authenticated);
    assert!(storage::load(root.path()).unwrap().is_none());
    save(root.path(), saved(&origin, storage::now() + 300)).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while !controller.snapshot().authenticated {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let frame = wire.prepare().unwrap().unwrap();
    assert_eq!(frame["from"], 0);
    assert_eq!(frame["snapshot"]["authenticated"], true);
    assert!(!frame.to_string().contains("test-access"));
    assert!(!frame.to_string().contains("test-refresh"));
    assert!(wire.finish(frame["batch"].as_u64().unwrap(), true));
    drop(controller);
    server.abort();
}

#[tokio::test]
async fn cancelling_one_owner_does_not_cancel_a_newer_process_login() {
    let root = tempfile::tempdir().unwrap();
    let account = Account::new(root.path(), "https://relay.example").unwrap();
    account.reserve_login("first").await.unwrap();
    account.reserve_login("second").await.unwrap();
    account.cancel_login("first").await.unwrap();
    assert_eq!(
        storage::read(root.path()).unwrap().login_attempt.as_deref(),
        Some("second")
    );
}

#[tokio::test]
async fn malformed_private_and_remote_values_never_escape_through_error_chains() {
    let root = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new()
        .fallback(|| async { axum::Json(serde_json::json!({"sessions":"sensitive-test-marker"})) });
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    save(root.path(), saved(&origin, storage::now() + 300)).await;
    let account = Account::new(root.path(), &origin).unwrap();
    let error = account.sessions().await.unwrap_err();
    assert!(!format!("{error:#}").contains("sensitive-test-marker"));
    std::fs::write(
        storage::path(root.path()),
        br#"{"version":"sensitive-test-marker"}"#,
    )
    .unwrap();
    let error = storage::read(root.path()).unwrap_err();
    assert!(!format!("{error:#}").contains("sensitive-test-marker"));
    server.abort();
}
