//! Explicit capacity regression; keep hundreds of MiB out of routine unit runs.
use super::*;
use std::{sync::Arc, time::Instant};
use zork_client_core::{api::StationClient, state::Device, store::ClientStore};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "300 MiB client/storage/Mesh capacity regression; run explicitly"]
async fn large_file_round_trip_at_300_mib() -> anyhow::Result<()> {
    const SIZE: usize = 300 * 1024 * 1024;
    let root = tempfile::tempdir()?;
    let started = Instant::now();
    let path = root.path().join("large.bin");
    let expected = {
        let mut bytes = vec![0; SIZE];
        blake3::Hasher::new()
            .update(b"zork 300 MiB capacity fixture")
            .finalize_xof()
            .fill(&mut bytes);
        let hash = zork_mesh::content_root(&bytes);
        std::fs::write(&path, bytes)?;
        hash
    };
    let store = Arc::new(ClientStore::open(&root.path().join("client"))?);
    let device = Device::open(
        Arc::new(StationClient::new("http://127.0.0.1:9", None)),
        Some((store.clone(), "node".into())),
        false,
    );
    let file = device.attach_path("chat", &path)?;
    ensure!(
        file.byte_len == SIZE && file.content_root == expected,
        "client snapshot mismatch"
    );
    let queued = device
        .submit_draft("chat", "Large file regression")?
        .unwrap();
    ensure!(
        zork_client_types::files::decode(&queued.content).unwrap().1 == [file.clone()],
        "queued file mismatch"
    );
    std::fs::remove_file(&path)?;
    drop(device);
    let bytes = store.blob("node", &format!("upload:{}", file.id))?.unwrap();
    ensure!(
        bytes.len() == SIZE && zork_mesh::content_root(&bytes) == expected,
        "client lost source bytes"
    );
    eprintln!(
        "PASS client 300 MiB snapshot and queue after source removal: {:?}",
        started.elapsed()
    );

    let server_root = root.path().join("server");
    let db = StationDb::open(&server_root, &server_root.join("workspaces"))?;
    let session = db.create_session_at_workspace(
        EnsureSession {
            connection_id: "local_gui",
            platform: "local_gui",
            channel_id: "large",
            root_thread_ts: "large",
            channel_type: Some("leader_chat"),
            initiator_user_id: None,
            initiator_message_ts: None,
        },
        &server_root.join("workspace"),
    )?;
    for (index, chunk) in bytes.chunks(CHUNK_BYTES).enumerate() {
        let received = db.receive_file_chunk(&session, &file, index * CHUNK_BYTES, chunk)?;
        ensure!(
            received == index * CHUNK_BYTES + chunk.len(),
            "upload receipt mismatch"
        );
    }
    drop(db);
    let db = StationDb::open(&server_root, &server_root.join("workspaces"))?;
    {
        let restored = db.conversation_file_bytes(&session.key, &file)?;
        ensure!(
            restored.len() == SIZE && zork_mesh::content_root(&restored) == expected,
            "server restart changed bytes"
        );
    }
    ensure!(
        db.receive_file_chunk(&session, &file, 0, &bytes[..CHUNK_BYTES])? == SIZE,
        "completed upload did not resume"
    );
    let mut oversized = file.clone();
    oversized.byte_len += 1;
    ensure!(
        db.receive_file_chunk(&session, &oversized, 0, b"x")
            .is_err(),
        "accepted oversized file"
    );
    eprintln!(
        "PASS server 300 MiB chunk upload, restart and duplicate receipt: {:?}",
        started.elapsed()
    );
    drop(db);
    drop(store);

    let config = zork_config::MeshConfig {
        enabled: true,
        offline: true,
        bind: Some("127.0.0.1:0".into()),
        ..Default::default()
    };
    let mut a = zork_mesh::managed::start_client(&root.path().join("mesh-a"), &config).await?;
    let mut b = zork_mesh::managed::start_client(&root.path().join("mesh-b"), &config).await?;
    let result: anyhow::Result<()> = async {
        let a_node = a.node();
        let b_node = b.node();
        a_node
            .trust(&b_node.identity().await?, "capacity B", None)
            .await?;
        b_node
            .trust(&a_node.identity().await?, "capacity A", None)
            .await?;
        let object = a_node.put("capacity-files", "large.bin", &bytes).await?;
        drop(bytes);
        let received = b_node.read(&object).await?;
        ensure!(
            received.len() == SIZE && zork_mesh::content_root(&received) == expected,
            "Mesh changed large file bytes"
        );
        eprintln!(
            "PASS Mesh 300 MiB transfer and full content hash: {:?}",
            started.elapsed()
        );
        Ok(())
    }
    .await;
    let a_closed = a.shutdown().await;
    let b_closed = b.shutdown().await;
    result?;
    a_closed?;
    b_closed?;
    Ok(())
}
