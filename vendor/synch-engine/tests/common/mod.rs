//! Shared fixtures for the engine's integration tests: in-process nodes on
//! loopback iroh endpoints with static, unilateral trust (§3.2) — a test opts
//! in with `mod common;` instead of re-typing the spawn/trust boilerplate.

pub(crate) mod wire;

use synch_core::{NodeId, OriginId};
use synch_engine::{Node, NodeConfig};
use synch_store::{Binding, BindingSource};

/// Runs a closure that touches the store on the blocking pool, as
/// `Store::conn` requires on a multi-thread runtime worker (§10).
#[allow(dead_code)]
pub(crate) async fn off_runtime<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(move || {
        let _scope = synch_core::BlockingScope::enter();
        f()
    })
    .await
    .unwrap()
}

/// A spawned node plus the tempdirs keeping its database and space alive.
#[allow(dead_code)]
pub(crate) struct Peer {
    pub _data: tempfile::TempDir,
    pub space: tempfile::TempDir,
    pub node: Node,
}

/// A node named `name@cluster.example` with an untouched loopback config.
#[allow(dead_code)]
pub(crate) async fn spawn_node(name: &str) -> Peer {
    spawn_node_with(name, |_| {}).await
}

/// A node named `name@cluster.example` whose configuration is adjusted before
/// it opens; the store opens off the runtime worker, so a test may spawn safely (§10).
#[allow(dead_code)]
pub(crate) async fn spawn_node_with(name: &str, tune: impl FnOnce(&mut NodeConfig)) -> Peer {
    let data = tempfile::tempdir().unwrap();
    let space = tempfile::tempdir().unwrap();
    let origin = OriginId::named(name, "cluster.example").unwrap();
    let dir = data.path().to_path_buf();
    off_runtime(move || Node::init_named_by_zone(&dir, origin))
        .await
        .unwrap();
    let mut config = NodeConfig::loopback(data.path());
    tune(&mut config);
    let node = Node::open(config).await.unwrap();
    Peer {
        _data: data,
        space,
        node,
    }
}

/// A static binding of `origin` to `key`, as an operator would configure one.
#[allow(dead_code)]
pub(crate) fn binding(origin: &OriginId, key: &NodeId) -> Binding {
    Binding {
        origin: origin.clone(),
        node_id: *key,
        source: BindingSource::Static,
        domain: None,
        issuer: None,
        spaces: Vec::new(),
        note: None,
        added_at: 0,
        expires_at: None,
    }
}

/// Trust is unilateral (§3.2): `node` admits `peer` and learns a direct address to dial.
#[allow(dead_code)]
pub(crate) fn trust(node: &Node, peer: &Node) {
    node.store()
        .put_binding(&binding(peer.origin(), &peer.node_id()))
        .unwrap();
    node.remember_peer(&peer.net().direct_addr()).unwrap();
}

/// Every node in the peer set trusts and can dial every other.
#[allow(dead_code)]
pub(crate) fn trust_all(peers: &[&Peer]) {
    for a in peers {
        for b in peers {
            if a.node.origin() != b.node.origin() {
                trust(&a.node, &b.node);
            }
        }
    }
}

/// Shuts a cluster down, so a test can tear down in one line.
#[allow(dead_code)]
pub(crate) async fn shutdown(nodes: &[&Node]) {
    for node in nodes {
        node.shutdown().await.unwrap();
    }
}

/// A deterministic payload of `len` bytes, varied so no chunk is mistaken for another.
#[allow(dead_code)]
pub(crate) fn big_payload(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 37 + 11) as u8).collect()
}
