use super::*;
use crate::api::{MessagePage, StationClient};
use serde_json::json;
use std::time::Duration;

fn seed(store: &ClientStore, id: &str, bytes: &[u8]) -> Result<()> {
    let file = FileRef {
        id: id.into(),
        name: "report.txt".into(),
        byte_len: bytes.len(),
        content_root: zork_mesh::content_root(bytes),
    };
    store.cache_message_page(
        "peer",
        "chat",
        &MessagePage {
            source_epoch: Some("source".into()),
            older_cursor: None,
            items: vec![serde_json::from_value(
                json!({"type":"message", "role":"assistant", "id":id,
            "content":crate::files::compose("", &[file])}),
            )?],
        },
        None,
    )
}

fn open(source: &Arc<Controller>, device: &Arc<Device>, id: &str) -> Result<()> {
    source.apply(
        Action::Open {
            peer: "peer".into(),
            session: "chat".into(),
            message: id.into(),
            file: id.into(),
        },
        Some(device.clone()),
    )
}

async fn ready(source: &Controller) -> Result<Preview> {
    let mut updates = source.source.subscribe();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(batch) = updates.prepare() {
                let preview = batch.snapshot.value.preview.clone();
                updates.acknowledge(batch.id);
                if let Some(preview) = preview.filter(|p| !p.loading) {
                    return preview;
                }
            }
            updates.ready().await.unwrap();
        }
    })
    .await
    .context("file preview did not settle")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn frozen_export_survives_picker_pause_and_revocation_invalidates_bytes_and_batches(
) -> Result<()> {
    let root = tempfile::tempdir()?;
    let store = Arc::new(ClientStore::open(root.path())?);
    let bytes = b"frozen attachment";
    seed(&store, "file-one", bytes)?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let client = Arc::new(StationClient::new(
        format!("http://{}", listener.local_addr()?),
        None,
    ));
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().route(
                "/v1/artifacts/{id}/content",
                axum::routing::get(|| async { bytes.as_slice() }),
            ),
        )
        .await
        .unwrap()
    });
    let device = Device::open(client, Some((store.clone(), "peer".into())), true);
    let source = Controller::new(store.clone());
    open(&source, &device, "file-one")?;
    let preview = ready(&source).await?;
    ensure!(preview.text.as_deref() == Some("frozen attachment"));
    source.apply(
        Action::PrepareSave {
            key: preview.key.clone(),
        },
        None,
    )?;
    let ticket = source
        .source
        .read()
        .preview
        .as_ref()
        .unwrap()
        .save_ticket
        .clone()
        .unwrap();
    source.pause();
    server.abort();
    let mut saved = vec![];
    source.write_copy(&ticket, &mut saved)?;
    ensure!(saved == bytes && source.write_copy(&ticket, &mut vec![]).is_err());
    source.apply(
        Action::PrepareSave {
            key: preview.key.clone(),
        },
        None,
    )?;
    let ticket = source
        .source
        .read()
        .preview
        .as_ref()
        .unwrap()
        .save_ticket
        .clone()
        .unwrap();
    let mut wire = crate::subscriptions::WireSubscription::from_chat_files(source.clone());
    let old = wire.prepare()?.unwrap()["batch"].as_u64().unwrap();
    store.revoke_replica("peer")?;
    ensure!(!wire.valid(old));
    ensure!(source.preview_bytes(&preview.key).is_none());
    ensure!(source.write_copy(&ticket, &mut vec![]).is_err());
    ensure!(wire.prepare()?.unwrap()["snapshot"]["preview"].is_null());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn late_reads_cannot_replace_another_selection_and_wrong_content_cannot_be_saved(
) -> Result<()> {
    let root = tempfile::tempdir()?;
    let store = Arc::new(ClientStore::open(root.path())?);
    for (id, bytes) in [
        ("file-slow", b"slow".as_slice()),
        ("file-fast", b"fast"),
        ("file-bad", b"expected"),
    ] {
        seed(&store, id, bytes)?;
    }
    let started = Arc::new(tokio::sync::Semaphore::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let (started_server, release_server) = (started.clone(), release.clone());
    let app = axum::Router::new().route(
        "/v1/artifacts/{id}/content",
        axum::routing::get(
            move |axum::extract::Path(id): axum::extract::Path<String>| {
                let (started, release) = (started_server.clone(), release_server.clone());
                async move {
                    if id == "file-slow" {
                        started.add_permits(1);
                        release.acquire().await.unwrap().forget();
                        "slow"
                    } else if id == "file-fast" {
                        "fast"
                    } else {
                        "corrupted"
                    }
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let device = Device::open(
        Arc::new(StationClient::new(
            format!("http://{}", listener.local_addr()?),
            None,
        )),
        Some((store.clone(), "peer".into())),
        true,
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let source = Controller::new(store);
    open(&source, &device, "file-slow")?;
    let old_key = source.source.read().preview.as_ref().unwrap().key.clone();
    tokio::time::timeout(Duration::from_secs(5), started.acquire())
        .await??
        .forget();
    open(&source, &device, "file-fast")?;
    let fast = ready(&source).await?;
    release.add_permits(1);
    source.apply(Action::Close { key: old_key }, None)?;
    ensure!(source.source.read().preview.as_ref().unwrap().key == fast.key);
    ensure!(source.preview_bytes(&fast.key).unwrap().as_slice() == b"fast");
    open(&source, &device, "file-bad")?;
    let bad = ready(&source).await?;
    ensure!(bad.error.is_some() && !bad.content_ready && bad.save_ticket.is_none());
    source.apply(
        Action::PrepareSave {
            key: bad.key.clone(),
        },
        None,
    )?;
    let bad = ready(&source).await?;
    ensure!(bad.error.is_some() && bad.save_ticket.is_none());
    source.leave_conversation(Some("peer"), Some("other-chat"));
    ensure!(source.source.read().preview.is_none());
    server.abort();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_explicit_remote_revocation_cannot_fall_back_to_cached_attachment_bytes() -> Result<()> {
    use axum::response::IntoResponse;
    use std::sync::atomic::{AtomicBool, Ordering};
    let root = tempfile::tempdir()?;
    let store = Arc::new(ClientStore::open(root.path())?);
    let revoked = Arc::new(AtomicBool::new(false));
    let flag = revoked.clone();
    let app = axum::Router::new().route(
        "/v1/artifacts/{id}/content",
        axum::routing::get(move || {
            let revoked = flag.load(Ordering::SeqCst);
            async move {
                if revoked {
                    (
                        axum::http::StatusCode::FORBIDDEN,
                        axum::Json(json!({"error":"device_removed_from_mesh"})),
                    )
                        .into_response()
                } else {
                    "cached bytes".into_response()
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let device = Device::open(
        Arc::new(StationClient::new(
            format!("http://{}", listener.local_addr()?),
            None,
        )),
        Some((store.clone(), "peer".into())),
        true,
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    ensure!(device.artifact_content("file-cached").await? == b"cached bytes");
    ensure!(store.blob("peer", "file-cached")?.is_some());
    revoked.store(true, Ordering::SeqCst);
    ensure!(device.artifact_content("file-cached").await.is_err());
    ensure!(device.snapshot().revoked && store.replica_revoked("peer")?);
    server.abort();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attachment_read_finishing_after_revocation_cannot_restore_cached_bytes() -> Result<()> {
    let root = tempfile::tempdir()?;
    let store = Arc::new(ClientStore::open(root.path())?);
    let started = Arc::new(tokio::sync::Semaphore::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let (start_server, release_server) = (started.clone(), release.clone());
    let app = axum::Router::new().route(
        "/v1/artifacts/{id}/content",
        axum::routing::get(move || {
            let (started, release) = (start_server.clone(), release_server.clone());
            async move {
                started.add_permits(1);
                release.acquire().await.unwrap().forget();
                "late bytes"
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let device = Device::open(
        Arc::new(StationClient::new(
            format!("http://{}", listener.local_addr()?),
            None,
        )),
        Some((store.clone(), "peer".into())),
        true,
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let read = tokio::spawn(async move { device.artifact_content("file-late").await });
    tokio::time::timeout(Duration::from_secs(5), started.acquire())
        .await??
        .forget();
    store.revoke_replica("peer")?;
    release.add_permits(1);
    ensure!(read.await?.is_err());
    ensure!(store.blob("peer", "file-late")?.is_none());
    server.abort();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inline_thumbnails_are_verified_images_only() -> Result<()> {
    let root = tempfile::tempdir()?;
    let store = Arc::new(ClientStore::open(root.path())?);
    let bytes: &'static [u8] = b"\x89PNG inline";
    let files = [
        FileRef {
            id: "file-image".into(),
            name: "shot.png".into(),
            byte_len: bytes.len(),
            content_root: zork_mesh::content_root(bytes),
        },
        FileRef {
            id: "file-doc".into(),
            name: "report.pdf".into(),
            byte_len: bytes.len(),
            content_root: zork_mesh::content_root(bytes),
        },
    ];
    store.cache_message_page(
        "peer",
        "chat",
        &MessagePage {
            source_epoch: Some("source".into()),
            older_cursor: None,
            items: vec![serde_json::from_value(
                json!({"type":"message", "role":"assistant", "id":"message",
                "content":crate::files::compose("看图", &files)}),
            )?],
        },
        None,
    )?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let client = Arc::new(StationClient::new(
        format!("http://{}", listener.local_addr()?),
        None,
    ));
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().route(
                "/v1/artifacts/{id}/content",
                axum::routing::get(move || async move { bytes }),
            ),
        )
        .await
        .unwrap()
    });
    let device = Device::open(client, Some((store.clone(), "peer".into())), true);
    let image = inline_bytes(
        &store,
        device.clone(),
        "peer",
        "chat",
        "message",
        "file-image",
    )
    .await?;
    ensure!(image == bytes);
    for missing in ["file-doc", "file-missing"] {
        ensure!(
            inline_bytes(&store, device.clone(), "peer", "chat", "message", missing)
                .await
                .is_err()
        );
    }
    store.revoke_replica("peer")?;
    ensure!(
        inline_bytes(&store, device, "peer", "chat", "message", "file-image")
            .await
            .is_err()
    );
    server.abort();
    Ok(())
}
