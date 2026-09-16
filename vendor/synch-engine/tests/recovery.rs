//! Key-loss recovery across real loopback endpoints (§3.4). The scenario
//! throughout: an origin's device key and database are lost, and the operator
//! brings it back under a fresh key with the same `id=` name. Its peers still
//! hold history signed by a key that is no longer bound, so those heads can
//! never be accepted (§4.4) — and their existence in the `Hello` summary is
//! the only thing recovery has to work with.

use std::time::Duration;

use synch_core::OriginId;
use synch_engine::{Node, NodeConfig, RecoveryOptions};
use synch_store::BindingSource;

mod common;
use common::{shutdown, spawn_node as spawn, trust, trust_all, Peer};

async fn open(data_dir: &std::path::Path, id: Option<OriginId>) -> Node {
    if let Some(id) = id {
        Node::init_named_by_zone(data_dir, id).unwrap();
    }
    Node::open(NodeConfig::loopback(data_dir)).await.unwrap()
}

fn origin(name: &str) -> OriginId {
    OriginId::named(name, "cluster.example").unwrap()
}

/// What the operator's replacement TXT record does to a peer: the origin now
/// resolves to the new device key, and the lost one speaks for nothing.
fn rebind(node: &Node, origin: &OriginId, lost: &synch_core::NodeId, recovered: &Node) {
    node.store()
        .remove_binding(origin, lost, BindingSource::Static)
        .unwrap();
    trust(node, recovered);
}

fn write(peer: &Peer, name: &str, body: &str) {
    std::fs::write(peer.space.path().join(name), body.as_bytes()).unwrap();
}

fn quick(wait: Duration, gap: u64) -> RecoveryOptions {
    RecoveryOptions {
        wait,
        gap,
        poll: Duration::from_millis(20),
    }
}

/// The whole arc: a wiped node refuses to publish, learns how far its origin
/// got from an ordinary `Hello` exchange, resumes above it, and is accepted
/// by the peer that holds the old history.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wiped_node_refuses_to_publish_then_resumes_above_its_peers() {
    let _blocking = synch_core::BlockingScope::enter();
    let nas = spawn("nas").await;
    let laptop = spawn("laptop").await;
    trust_all(&[&nas, &laptop]);
    let lost_key = nas.node.node_id();

    // The NAS publishes three roots; the laptop replicates them.
    nas.node
        .add_filesystem_source("media", nas.space.path())
        .unwrap();
    for round in 1..=3 {
        write(&nas, &format!("round{round}.txt"), "content");
        nas.node.scan_and_publish().unwrap();
    }
    laptop
        .node
        .sync_with_peer(&nas.node.node_id())
        .await
        .unwrap();
    assert_eq!(
        laptop
            .node
            .store()
            .complete_head(nas.node.origin())
            .unwrap()
            .unwrap()
            .seq,
        3
    );
    nas.node.shutdown().await.unwrap();

    // Key and database are gone; the operator brings the origin back under a
    // fresh key, with the same name, on an empty database.
    let data = tempfile::tempdir().unwrap();
    let recovered = open(data.path(), Some(origin("nas"))).await;
    assert_ne!(recovered.node_id(), lost_key);
    trust(&recovered, &laptop.node);
    rebind(&laptop.node, &origin("nas"), &lost_key, &recovered);

    // One ordinary exchange: the laptop's head for `nas` is signed by the lost
    // key, no longer bound, and rejected (§4.4); what survives is the summary.
    let report = recovered
        .sync_with_peer(&laptop.node.node_id())
        .await
        .unwrap();
    assert_eq!(report.heads_rejected, 1, "{report:?}");
    assert!(recovered
        .store()
        .complete_head(recovered.origin())
        .unwrap()
        .is_none());

    let state = recovered.recovery_state().unwrap();
    assert!(state.in_recovery, "{state:?}");

    // Publishing is refused, and the error says what to run — before the
    // scan, so nothing is recorded as published that was not.
    recovered
        .add_filesystem_source("media", nas.space.path())
        .unwrap();
    let err = recovered.scan_and_publish().unwrap_err();
    let message = err.to_string();
    assert!(message.contains("synch recover"), "{message}");
    assert!(message.contains("seq 3"), "{message}");
    assert!(recovered.store().local_files("media").unwrap().is_empty());
    assert!(recovered.source_removal("media").is_err());

    // `--wait 0` collects one round and returns promptly.
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let recovery = tokio::time::timeout(
        Duration::from_secs(30),
        recovered.recover(quick(Duration::ZERO, 1_000), tx),
    )
    .await
    .expect("a zero wait must not linger")
    .unwrap();
    assert_eq!(recovery.rounds, 1);
    assert_eq!(recovery.reached, 1, "{recovery:?}");
    assert_eq!(recovery.observed_seq, Some(3));
    assert_eq!(recovery.floor, Some(1_003));
    assert!(!recovered.recovery_state().unwrap().in_recovery);

    // The floor is durable: still there after a restart, before anything has been published under it.
    recovered.shutdown().await.unwrap();
    let recovered = open(data.path(), None).await;
    assert_eq!(recovered.next_seq().unwrap(), 1_003);

    // Publishing resumes strictly above everything the peer advertised, by the gap, and is accepted under the ordinary rule.
    let head = recovered.scan_publish_push().await.unwrap().unwrap();
    assert_eq!(head.seq, 1_003);
    // The push carries the head; the pull that follows completes its trie and flips the peer's complete slot (§5.2).
    laptop
        .node
        .sync_with_peer(&recovered.node_id())
        .await
        .unwrap();
    let theirs = laptop
        .node
        .store()
        .complete_head(recovered.origin())
        .unwrap()
        .unwrap();
    assert_eq!(theirs.seq, 1_003);

    // And the next publish carries on from there, not from the floor.
    write(&nas, "after-recovery.txt", "more");
    let head = recovered.scan_and_publish().unwrap().1.unwrap();
    assert_eq!(head.seq, 1_004);

    shutdown(&[&recovered, &laptop.node]).await;
}

/// Why the gap is not an optimization to remove: a peer unreachable
/// throughout the quiesce comes back holding history *above* the floor, so
/// the recovered node's publishes are refused as not newer and the old
/// history stays as provable fork evidence for the operator (§3.4, §4.4).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn history_only_an_unreachable_peer_held_survives_as_fork_evidence() {
    let _blocking = synch_core::BlockingScope::enter();
    let nas = spawn("nas").await;
    let laptop = spawn("laptop").await;
    let vps = spawn("vps").await;
    trust_all(&[&nas, &laptop, &vps]);
    let lost_key = nas.node.node_id();

    nas.node
        .add_filesystem_source("media", nas.space.path())
        .unwrap();
    write(&nas, "a.txt", "one");
    nas.node.scan_and_publish().unwrap();
    // The laptop replicates seq 1 and then falls behind.
    laptop
        .node
        .sync_with_peer(&nas.node.node_id())
        .await
        .unwrap();
    for round in 2..=4 {
        write(&nas, &format!("round{round}.txt"), "more");
        nas.node.scan_and_publish().unwrap();
    }
    // The VPS has the newest history, and is then partitioned away entirely.
    vps.node.sync_with_peer(&nas.node.node_id()).await.unwrap();
    assert_eq!(
        vps.node
            .store()
            .complete_head(nas.node.origin())
            .unwrap()
            .unwrap()
            .seq,
        4
    );
    nas.node.shutdown().await.unwrap();
    vps.node.shutdown().await.unwrap();

    // Recovery sees only the laptop (which knows seq 1), with the smallest gap the protocol allows.
    let data = tempfile::tempdir().unwrap();
    let recovered = open(data.path(), Some(origin("nas"))).await;
    trust(&recovered, &laptop.node);
    rebind(&laptop.node, &origin("nas"), &lost_key, &recovered);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let report = recovered
        .recover(quick(Duration::ZERO, 1), tx)
        .await
        .unwrap();
    assert_eq!(report.observed_seq, Some(1));
    assert_eq!(report.floor, Some(2));

    recovered
        .add_filesystem_source("media", nas.space.path())
        .unwrap();
    let head = recovered.scan_publish_push().await.unwrap().unwrap();
    assert_eq!(head.seq, 2);

    // The VPS returns: its head is newer than anything the recovered node has
    // published, so monotonicity refuses the new head — a fork, not a silent overwrite.
    let vps = open(vps._data.path(), None).await;
    rebind(&vps, &origin("nas"), &lost_key, &recovered);
    trust(&recovered, &vps);
    let sync = vps.sync_with_peer(&recovered.node_id()).await.unwrap();
    assert_eq!(sync.heads_accepted, 0, "{sync:?}");
    assert_eq!(
        vps.store()
            .complete_head(&origin("nas"))
            .unwrap()
            .unwrap()
            .seq,
        4
    );

    // And it is reported as exactly that, on the side that holds the evidence.
    let report = vps.doctor().unwrap();
    let fork = report
        .unreconciled
        .iter()
        .find(|entry| entry.origin == origin("nas"))
        .unwrap_or_else(|| panic!("{:?}", report.unreconciled));
    assert_eq!(fork.seq, 4);
    assert_eq!(fork.signed_by, lost_key);
    // The content behind it is still there for manual salvage (§3.4).
    assert_eq!(
        vps.read_entry(&origin("nas"), "media", "a.txt")
            .await
            .unwrap(),
        b"one"
    );

    shutdown(&[&recovered, &laptop.node, &vps]).await;
}
