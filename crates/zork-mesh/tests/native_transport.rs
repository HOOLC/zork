use anyhow::Result;
use zork_mesh::{managed, node::ObjectRef};
fn config() -> zork_config::MeshConfig {
    zork_config::MeshConfig {
        enabled: true,
        offline: true,
        ..Default::default()
    }
}
#[tokio::test]
async fn device_identity_is_persisted_and_exclusively_owned() -> Result<()> {
    let root = tempfile::tempdir()?;
    let directory = managed::data_dir(root.path());
    std::fs::create_dir_all(&directory)?;
    let mut first = managed::start(root.path(), &config()).await?;
    let expected = first.node().identity().await?;
    assert!(managed::start(root.path(), &config()).await.is_err());
    first.shutdown().await?;
    let mut second = managed::start(root.path(), &config()).await?;
    assert_eq!(second.node().identity().await?, expected);
    second.shutdown().await?;
    Ok(())
}
#[tokio::test]
async fn direct_attachments_are_fixed_and_revocation_blocks_cached_reads() -> Result<()> {
    let a = tempfile::tempdir()?;
    let b = tempfile::tempdir()?;
    let mut sender = managed::start(a.path(), &config()).await?;
    let mut receiver = managed::start_client(b.path(), &config()).await?;
    let left = sender.node();
    let right = receiver.node();
    let left_id = left.identity().await?;
    let right_id = right.identity().await?;
    left.trust(&right_id, "receiver", None).await?;
    right.trust(&left_id, "sender", None).await?;
    left.remember_peer_address(&right_id, serde_json::to_value(right.address()?)?)
        .await?;
    right
        .remember_peer_address(&left_id, serde_json::to_value(left.address()?)?)
        .await?;
    let bytes = vec![37; 1024 * 1024 + 7];
    let fixed = left.put("attachments", "result", &bytes).await?;
    left.put("attachments", "result", b"new revision").await?;
    assert_eq!(right.read(&fixed).await?, bytes);
    let tampered = ObjectRef {
        size: fixed.size - 1,
        ..fixed.clone()
    };
    assert!(right.read(&tampered).await.is_err());
    right.untrust(&left_id).await?;
    assert!(right.read(&fixed).await.is_err());
    sender.shutdown().await?;
    receiver.shutdown().await?;
    assert!(!managed::data_dir(a.path())
        .join("synchronicity.db")
        .exists());
    Ok(())
}
#[tokio::test]
async fn invitation_address_hints_do_not_grant_membership() -> Result<()> {
    let root = tempfile::tempdir()?;
    let mut runtime = managed::start(root.path(), &config()).await?;
    let key = iroh::SecretKey::generate().public();
    let origin = format!("key:{}", key.to_z32());
    let address = iroh::EndpointAddr::new(key).with_ip_addr("127.0.0.1:12345".parse()?);
    runtime
        .node()
        .remember_peer_address(&origin, serde_json::to_value(&address)?)
        .await?;
    assert_eq!(runtime.node().peer_address(&origin).await?, Some(address));
    assert!(!runtime.node().is_trusted(&origin).await?);
    runtime.shutdown().await?;
    Ok(())
}
