use axum::{routing::post, Json, Router};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use zork_client_core::{api::StationClient, store::ClientStore, sync::Coordinator};
use zork_client_types::sync::{Cursor, Page, Pull, Reply, Scope};

#[tokio::test]
async fn concurrent_scope_refreshes_share_work_and_newer_requests_catch_up() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let router = Router::new().route(
        "/v1/node/sync",
        post(move |Json(request): Json<Pull>| {
            let count = count.clone();
            async move {
                let index = count.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                Json(Reply::Page {
                    page: Page {
                        protocol: 1,
                        batch_id: format!("batch-{index}"),
                        from: request.after,
                        through: Cursor {
                            owner: "owner".into(),
                            epoch: "epoch".into(),
                            scope: request.scope,
                            sequence: index as u64,
                        },
                        index: 0,
                        last: true,
                        records: vec![],
                    },
                })
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(ClientStore::open(dir.path()).unwrap());
    let coordinator = Coordinator::new(
        Arc::new(StationClient::new(url, None)),
        store.clone(),
        "peer".into(),
        "owner".into(),
    )
    .unwrap();
    let futures = (0..50).map(|_| {
        let c = coordinator.clone();
        async move {
            c.refresh(Scope::Catalog {}).await.unwrap();
        }
    });
    futures_util::future::join_all(futures).await;
    let completed = calls.load(Ordering::SeqCst);
    assert!(
        (1..=2).contains(&completed),
        "refresh burst made {completed} network requests"
    );
    let old = store
        .replica_state("peer", &Scope::Catalog {})
        .unwrap()
        .cursor
        .unwrap();
    coordinator.refresh(Scope::Catalog {}).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), completed + 1);
    assert!(
        store
            .replica_state("peer", &Scope::Catalog {})
            .unwrap()
            .cursor
            .unwrap()
            .sequence
            > old.sequence
    );
    server.abort();
}
