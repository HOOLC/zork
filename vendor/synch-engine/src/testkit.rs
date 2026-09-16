//! Shared fixtures for the engine's inline test modules.
//!
//! The async node fixture (a tempdir, `Node::init`, `Node::open` over a
//! loopback config) and the small helpers around it — published-entry lookup,
//! reopen-after-shutdown, and "soon, not at 50 ms" polling — were duplicated
//! across the inline test modules of checkout, scanner, control-plane, recovery,
//! `compare`, `tree`, `watcher`, `publisher`, and `aae`. They live here once.

use std::path::Path;
use std::time::Duration;

use synch_store::EntryRow;

use crate::config::NodeConfig;
use crate::node::Node;

/// A fresh node in a tempdir, offline and talking to nobody (loopback).
///
/// The tempdir is returned alongside so the test keeps it alive: dropping it
/// deletes the node's database out from under the node.
#[allow(dead_code)]
pub(crate) async fn node() -> (tempfile::TempDir, Node) {
    node_with(|_| {}).await
}

/// A fresh node whose loopback config is tuned before it opens — publisher
/// batch triggers, tombstone TTLs, and the like.
#[allow(dead_code)]
pub(crate) async fn node_with(tune: impl FnOnce(&mut NodeConfig)) -> (tempfile::TempDir, Node) {
    let dir = tempfile::tempdir().unwrap();
    Node::init(dir.path(), None).unwrap();
    let mut config = NodeConfig::loopback(dir.path());
    tune(&mut config);
    let node = Node::open(config).await.unwrap();
    (dir, node)
}

/// A fresh node initialized under a named origin, as the recovery tests need.
#[allow(dead_code)]
pub(crate) async fn node_as(origin: &synch_core::OriginId) -> (tempfile::TempDir, Node) {
    let dir = tempfile::tempdir().unwrap();
    Node::init_named_by_zone(dir.path(), origin.clone()).unwrap();
    let node = Node::open(NodeConfig::loopback(dir.path())).await.unwrap();
    (dir, node)
}

/// A fresh node with one space, `"media"`, rooted at its own tempdir.
#[allow(dead_code)]
pub(crate) async fn node_with_space() -> (tempfile::TempDir, tempfile::TempDir, Node) {
    let (data, node) = node().await;
    let space = tempfile::tempdir().unwrap();
    node.add_filesystem_source("media", space.path()).unwrap();
    (data, space, node)
}

/// The entry the node published at `space/path` under its own origin.
///
/// Panics naming the path when nothing is there: most assertions want the
/// row itself, not an `Option` to unwrap at arm's length.
#[allow(dead_code)]
pub(crate) fn published(node: &Node, space: &str, path: &str) -> EntryRow {
    node.store()
        .entry(node.origin(), space, path)
        .unwrap()
        .unwrap_or_else(|| panic!("{space}/{path} must be published"))
}

/// Opens the node in `data` again after its previous handle was shut down,
/// as the crash-recovery and reconciliation tests do.
#[allow(dead_code)]
pub(crate) async fn reopen(data: &Path) -> Node {
    Node::open(NodeConfig::loopback(data)).await.unwrap()
}

/// Polls until `ready` holds, giving up after two seconds. Timing-sensitive
/// tests assert "soon", never "at 50 ms".
#[allow(dead_code)]
pub(crate) async fn eventually(mut ready: impl FnMut() -> bool) -> bool {
    for _ in 0..200 {
        if ready() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}
