//! Native control and verified file transfer through isolated embedded nodes.
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, OnceLock},
    time::Duration,
};
use zork_config::MeshConfig;
use zork_mesh::{
    control::{ControlHandler, ControlStream, Peer},
    managed,
    node::MeshNode,
};

#[derive(Debug, Default)]
struct Echo(OnceLock<MeshNode>);
impl ControlHandler for Echo {
    fn serve(
        &self,
        peer: Peer,
        payload: Value,
        mut stream: ControlStream,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> {
        let node = self.0.get().cloned();
        Box::pin(async move {
            let node = node.context("not ready")?;
            ensure!(node.is_trusted(&peer.origin).await?, "unpaired peer");
            stream
                .write(&json!({"v":1,"authenticated_peer":peer,"echo":payload}))
                .await?;
            stream.finish()
        })
    }
}
struct Node {
    runtime: managed::Runtime,
    root: PathBuf,
    control: MeshNode,
    origin: String,
}
fn config() -> MeshConfig {
    MeshConfig {
        enabled: true,
        offline: true,
        bind: Some("127.0.0.1:0".into()),
        ..Default::default()
    }
}
async fn start(root: &Path) -> Result<Node> {
    let handler = Arc::new(Echo::default());
    let runtime = managed::start_with_control(root, &config(), handler.clone()).await?;
    let control = runtime.node();
    handler.0.set(control.clone()).unwrap();
    let origin = control.identity().await?;
    Ok(Node {
        runtime,
        root: root.into(),
        control,
        origin,
    })
}
async fn trust(a: &Node, b: &Node) -> Result<()> {
    a.control.trust(&b.origin, "isolated peer", None).await
}
#[tokio::main]
async fn main() -> Result<()> {
    let root = tempfile::Builder::new().prefix("zmesh-native-").tempdir()?;
    let mut a = start(&root.path().join("a")).await?;
    let mut b = start(&root.path().join("b")).await?;
    let mut stranger = start(&root.path().join("stranger")).await?;
    let result = async {
        trust(&a, &b).await?;
        trust(&b, &a).await?;
        trust(&stranger, &b).await?;
        let bytes = "不可变任务产物\n".repeat(16384).into_bytes();
        let object = a.control.put("zork-lab", "artifacts/example/v1", &bytes).await?;
        ensure!(b.control.read(&object).await? == bytes, "remote bytes mismatch");
        let mut forged = object.clone(); forged.root = "00".repeat(32);
        ensure!(b.control.read(&forged).await.is_err(), "accepted incorrect hash");
        println!("PASS: verified remote file transfer and incorrect hash rejection");
        let payload = json!({"from_node":"key:forged","padding":"x".repeat(70000)});
        let hello = a.control.exchange(&b.origin, &payload).await?;
        ensure!(hello["authenticated_peer"]["origin"] == a.origin && hello["echo"] == payload, "identity or fragmented payload mismatch");
        ensure!(stranger.control.exchange(&b.origin, &json!({})).await.is_err(), "unpaired peer admitted");
        ensure!(!b.root.join("mesh/control-source").exists(), "control content program deployed");
        println!("PASS: native ALPN, pinned identity, fragmented frame and unpaired peer rejection");
        b.runtime.shutdown().await?;
        let previous = b.origin.clone();
        b = start(&b.root).await?;
        ensure!(previous == b.origin, "identity changed");
        let reply = tokio::time::timeout(Duration::from_secs(25), a.control.exchange(&b.origin, &json!({"restart":true}))).await??;
        ensure!(reply["echo"]["restart"] == true, "control did not recover");
        ensure!(b.control.read(&object).await? == bytes, "pinned file lost");
        b.control.untrust(&a.origin).await?;
        ensure!(a.control.exchange(&b.origin, &json!({})).await.is_err(), "revoked peer used cached connection");
        println!("PASS: restart with a new port, preserved identity/file and revocation on a cached connection");
        Ok::<_, anyhow::Error>(())
    }.await;
    a.runtime.shutdown().await?;
    b.runtime.shutdown().await?;
    stranger.runtime.shutdown().await?;
    result
}
