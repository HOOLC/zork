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
fn save(root: &Path, current: Option<RelaySession>) {
    let _lock = storage::try_lock(root).unwrap().unwrap();
    let file = AccountFile {
        current,
        ..Default::default()
    };
    storage::write(root, &file).unwrap();
}
#[tokio::test]
async fn credentials_never_follow_an_origin_change() {
    let root = tempfile::tempdir().unwrap();
    save(
        root.path(),
        Some(saved("https://old.example", storage::now() + 300)),
    );
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
    save(root.path(), Some(saved(&origin, expires)));
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
    save(root.path(), Some(saved(&origin, storage::now() + 300)));
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
    save(root.path(), Some(saved(&origin, storage::now() + 300)));
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
