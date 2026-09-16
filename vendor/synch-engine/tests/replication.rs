//! A replica, end to end over two real nodes (`docs/REPLICATION.md`).
//!
//! The publisher serves a filesystem source; the replica publishes nothing and holds
//! everything. What is being tested is the lifecycle a version goes through
//! there — wanted, held, superseded, scheduled, released — and the discipline
//! that decides the last two.

use std::time::Duration;

use synch_engine::{reconcile::HeadOutcome, replica::ViewState, Node};
use synch_store::{PinHolder, ReplicaPolicy};

mod common;
use common::{off_runtime, shutdown, spawn_node as spawn, spawn_node_with, trust_all as introduce};

fn holder() -> PinHolder {
    PinHolder::Replica("media".into())
}

/// Syncs the replica from the publisher and runs one full replication pass.
async fn converge(replica: &Node, publisher: &Node) {
    replica.sync_with_peer(&publisher.node_id()).await.unwrap();
    let sweeping = replica.clone();
    off_runtime(move || sweeping.sweep_replicas(None).unwrap()).await;
    replica.fetch_content_wants().await.unwrap();
}

/// What the replica holds and wants for `media`.
async fn coverage(replica: &Node) -> synch_store::ReplicaCoverage {
    let store = replica.store().clone();
    off_runtime(move || {
        store
            .replica_coverage(&holder(), synch_engine::replica::UNREACHABLE_ATTEMPTS)
            .unwrap()
    })
    .await
}

/// The whole point: a node that publishes nothing ends up holding every
/// version of every path, and goes on holding a version after the publisher
/// has replaced it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replica_holds_what_the_publisher_publishes_and_keeps_what_it_supersedes() {
    let _blocking = synch_core::BlockingScope::enter();
    let publisher = spawn("publisher").await;
    let replica = spawn("replica").await;
    introduce(&[&publisher, &replica]);

    publisher
        .node
        .add_filesystem_source("media", publisher.space.path())
        .unwrap();
    std::fs::write(publisher.space.path().join("keynote.mp4"), b"first cut").unwrap();
    publisher.node.scan_publish_push().await.unwrap();

    // The replica holds no checkout of `media` at all: replication is not a
    // source, and the space it replicates is one it never publishes.
    let replicating = replica.node.clone();
    off_runtime(move || {
        replicating
            .add_replica("media", ReplicaPolicy::Current, Some(3600), None, None)
            .unwrap();
    })
    .await;

    converge(&replica.node, &publisher.node).await;
    let first = coverage(&replica.node).await;
    assert_eq!(first.held, 1, "the published version should be held");
    assert_eq!(first.wanted, 0, "and nothing should still be wanted");
    assert_eq!(first.held_bytes, b"first cut".len() as u64);

    // The publisher replaces the file. The replica takes the new version and
    // schedules — but does not yet drop — the old one.
    std::fs::write(
        publisher.space.path().join("keynote.mp4"),
        b"the second cut",
    )
    .unwrap();
    publisher.node.scan_publish_push().await.unwrap();
    converge(&replica.node, &publisher.node).await;

    let second = coverage(&replica.node).await;
    assert_eq!(
        second.held, 2,
        "both versions are held while the first is inside its grace window"
    );
    assert_eq!(
        second.releasing, 1,
        "and exactly one of them is on its way out"
    );

    // Inside the window the superseded bytes are still readable, which is the
    // whole reason to have one.
    let store = replica.node.store().clone();
    let held: Vec<synch_store::PinRow> =
        off_runtime(move || store.pins().unwrap().into_iter().collect()).await;
    let leaving = held
        .iter()
        .find(|pin| pin.release_after.is_some())
        .expect("a scheduled release");
    let store = replica.node.store().clone();
    let root = leaving.root;
    let bytes = off_runtime(move || store.read_all(&root).unwrap()).await;
    assert_eq!(bytes, b"first cut");

    shutdown(&[&publisher.node, &replica.node]).await;
}

/// A pending head is not a replacement for the last complete materialized
/// view. The sweep keeps roots named by seq 1 while seq 2 is unavailable, but
/// it does not let that pending head block an unrelated stale release.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pending_update_keeps_the_last_complete_roots_without_blocking_the_sweep() {
    let _blocking = synch_core::BlockingScope::enter();
    let publisher = spawn("publisher").await;
    let replica = spawn_node_with("replica", |config| config.replica_release_floor = 0).await;
    introduce(&[&publisher, &replica]);

    publisher
        .node
        .add_filesystem_source("media", publisher.space.path())
        .unwrap();
    std::fs::write(publisher.space.path().join("a.bin"), b"version one").unwrap();
    let first = publisher.node.scan_publish_push().await.unwrap().unwrap();

    let replicating = replica.node.clone();
    off_runtime(move || {
        replicating
            .add_replica("media", ReplicaPolicy::Current, Some(3600), None, None)
            .unwrap();
    })
    .await;
    converge(&replica.node, &publisher.node).await;

    let referenced = replica
        .node
        .store()
        .entry(publisher.node.origin(), "media", "a.bin")
        .unwrap()
        .unwrap()
        .content
        .unwrap();
    assert_eq!(
        replica
            .node
            .store()
            .complete_head(publisher.node.origin())
            .unwrap(),
        Some(first.clone())
    );

    // A stale pin gives the sweep independent work to do while seq 2 waits.
    let stale = replica
        .node
        .store()
        .ingest_bytes(b"unreferenced replica content", synch_core::now_ns())
        .unwrap();
    replica
        .node
        .store()
        .pin(&stale, &holder(), synch_core::now_ns())
        .unwrap();

    // Stop the publisher pushing seq 2 to the replica, then take it offline so
    // the offered head cannot be fetched and promoted in the background.
    publisher
        .node
        .store()
        .remove_origin_bindings(replica.node.origin())
        .unwrap();
    std::fs::write(publisher.space.path().join("a.bin"), b"version two").unwrap();
    let second = publisher.node.scan_publish_push().await.unwrap().unwrap();
    publisher.node.shutdown().await.unwrap();

    assert_eq!(
        replica
            .node
            .syncer()
            .offer_head(&second, synch_core::now_ns())
            .unwrap(),
        HeadOutcome::Pending
    );
    assert_eq!(
        replica
            .node
            .store()
            .complete_head(publisher.node.origin())
            .unwrap(),
        Some(first),
        "seq 1 remains the materialized view while seq 2 is pending"
    );
    assert_eq!(
        replica
            .node
            .store()
            .entry(publisher.node.origin(), "media", "a.bin")
            .unwrap()
            .unwrap()
            .content,
        Some(referenced)
    );

    let sweeping = replica.node.clone();
    let report = off_runtime(move || sweeping.sweep_replicas(None).unwrap()).await;
    assert_eq!(report[0].1.scheduled, 1, "only the unrelated pin is stale");
    assert_eq!(
        replica.node.store().pins_for(&referenced).unwrap()[0].release_after,
        None,
        "the last complete entry remains a GC root"
    );
    assert!(
        replica.node.store().pins_for(&stale).unwrap()[0]
            .release_after
            .is_some(),
        "the pending update must not block unrelated release work"
    );

    shutdown(&[&replica.node]).await;
}

/// The live path does the work, not the sweep (§3.4).
///
/// Staging and release-scheduling happen inside the transaction that flips the
/// head, so one anti-entropy round — with no sweep behind it — is enough to
/// want the new version and to schedule the one it replaced. The sweep exists
/// to catch what this misses, not to do this.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_promotion_stages_and_schedules_without_any_sweep() {
    let _blocking = synch_core::BlockingScope::enter();
    let publisher = spawn("publisher").await;
    let replica = spawn("replica").await;
    introduce(&[&publisher, &replica]);

    publisher
        .node
        .add_filesystem_source("media", publisher.space.path())
        .unwrap();
    std::fs::write(publisher.space.path().join("keynote.mp4"), b"first cut").unwrap();
    publisher.node.scan_publish_push().await.unwrap();

    let replicating = replica.node.clone();
    off_runtime(move || {
        replicating
            .add_replica("media", ReplicaPolicy::Current, Some(3600), None, None)
            .unwrap();
    })
    .await;

    // One round, no sweep: the promotion alone must want the content.
    replica
        .node
        .sync_with_peer(&publisher.node.node_id())
        .await
        .unwrap();
    let staged = coverage(&replica.node).await;
    assert_eq!(
        staged.wanted, 1,
        "the promotion transaction should have staged the want itself"
    );
    assert_eq!(staged.held, 0, "and nothing is held until the fetch runs");

    // Fetch it, then supersede it — again with no sweep anywhere.
    replica.node.fetch_content_wants().await.unwrap();
    assert_eq!(coverage(&replica.node).await.held, 1);

    std::fs::write(
        publisher.space.path().join("keynote.mp4"),
        b"the second cut",
    )
    .unwrap();
    publisher.node.scan_publish_push().await.unwrap();
    replica
        .node
        .sync_with_peer(&publisher.node.node_id())
        .await
        .unwrap();

    let after = coverage(&replica.node).await;
    assert_eq!(
        after.releasing, 1,
        "the promotion that replaced the root should have scheduled its release"
    );
    assert_eq!(
        after.wanted, 1,
        "and wanted the version that replaced it, before any sweep ran"
    );

    shutdown(&[&publisher.node, &replica.node]).await;
}

/// The standing loop converges on its own, without anyone calling the sweep.
///
/// Every other test here drives `sweep_replicas` and `fetch_content_wants` by
/// hand, which is precisely how a missing wake goes unnoticed: the loop was
/// rung only by a configuration change, so a replica lagged a whole interval
/// behind every publish and an operator saw a claim of zero while the node held
/// terabytes. This test only publishes and waits.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_standing_loop_converges_without_being_driven() {
    let _blocking = synch_core::BlockingScope::enter();
    let peer = spawn("peer").await;

    let node = peer.node.clone();
    let path = peer.space.path().to_path_buf();
    off_runtime(move || {
        node.add_filesystem_source("media", &path).unwrap();
        node.add_replica("media", ReplicaPolicy::Current, None, None, None)
            .unwrap();
    })
    .await;

    let (stop, mut rx) = tokio::sync::broadcast::channel::<()>(1);
    let running = peer.node.clone();
    let loop_handle = tokio::spawn(async move {
        running
            .run_replicas(async move {
                let _ = rx.recv().await;
            })
            .await
    });

    std::fs::write(peer.space.path().join("published.bin"), b"by the scanner").unwrap();
    peer.node.scan_publish_push().await.unwrap();

    // The publish rings the replication bell, so the loop should hold the
    // content without anything else asking it to.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        if coverage(&peer.node).await.held > 0 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the standing loop never held the published content"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let _ = stop.send(());
    tokio::time::timeout(Duration::from_secs(10), loop_handle)
        .await
        .expect("the loop must observe shutdown promptly")
        .unwrap();

    shutdown(&[&peer.node]).await;
}

/// The grace window is what makes a deletion recoverable, and expiry is what
/// finally ends a claim — so that every other predicate over `pins` can stay
/// free of the clock (§3.1).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deleted_version_survives_its_grace_window_and_not_a_moment_longer() {
    let _blocking = synch_core::BlockingScope::enter();
    let publisher = spawn("publisher").await;
    let replica = spawn("replica").await;
    introduce(&[&publisher, &replica]);

    publisher
        .node
        .add_filesystem_source("media", publisher.space.path())
        .unwrap();
    std::fs::write(publisher.space.path().join("notes.txt"), b"keep me").unwrap();
    publisher.node.scan_publish_push().await.unwrap();

    let replicating = replica.node.clone();
    off_runtime(move || {
        replicating
            .add_replica("media", ReplicaPolicy::Current, Some(3600), None, None)
            .unwrap();
    })
    .await;
    converge(&replica.node, &publisher.node).await;
    assert_eq!(coverage(&replica.node).await.held, 1);

    // The publisher deletes it. A tombstone is a version like any other, and
    // the content it supersedes is scheduled rather than dropped.
    std::fs::remove_file(publisher.space.path().join("notes.txt")).unwrap();
    publisher.node.scan_publish_push().await.unwrap();
    converge(&replica.node, &publisher.node).await;

    let after = coverage(&replica.node).await;
    assert_eq!(after.held, 1, "still held: the grace window has not passed");
    assert_eq!(after.releasing, 1);

    // Expiry is what removes the claim. Nothing before its instant does.
    let store = replica.node.store().clone();
    let (before, after_expiry) = off_runtime(move || {
        let scheduled = store.pins().unwrap()[0].release_after.unwrap();
        let before = store.expire_pins(scheduled - 1).unwrap();
        let after = store.expire_pins(scheduled).unwrap();
        (before, after)
    })
    .await;
    assert_eq!(before, 0, "a scheduled release is a plan, not a departure");
    assert_eq!(after_expiry, 1);
    assert_eq!(coverage(&replica.node).await.held, 0);

    shutdown(&[&publisher.node, &replica.node]).await;
}

/// Under `forever` nothing is ever released, whatever the tree does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forever_retention_releases_nothing() {
    let _blocking = synch_core::BlockingScope::enter();
    let publisher = spawn("publisher").await;
    let replica = spawn("replica").await;
    introduce(&[&publisher, &replica]);

    publisher
        .node
        .add_filesystem_source("media", publisher.space.path())
        .unwrap();
    std::fs::write(publisher.space.path().join("draft.txt"), b"v1").unwrap();
    publisher.node.scan_publish_push().await.unwrap();

    let replicating = replica.node.clone();
    off_runtime(move || {
        replicating
            .add_replica("media", ReplicaPolicy::Forever, None, None, None)
            .unwrap();
    })
    .await;
    converge(&replica.node, &publisher.node).await;

    std::fs::remove_file(publisher.space.path().join("draft.txt")).unwrap();
    publisher.node.scan_publish_push().await.unwrap();
    converge(&replica.node, &publisher.node).await;

    let after = coverage(&replica.node).await;
    assert_eq!(after.held, 1);
    assert_eq!(
        after.releasing, 0,
        "an archive schedules nothing, deletion included"
    );

    shutdown(&[&publisher.node, &replica.node]).await;
}

/// Only complete materialized heads contribute GC roots. Changing scope
/// demotes foreign heads and removes their old materialized entries, so those
/// origins stop contributing until they are promoted under the new scope.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sweep_continues_from_the_materialized_view_while_sync_is_incomplete() {
    let _blocking = synch_core::BlockingScope::enter();
    let publisher = spawn("publisher").await;
    // Isolate the materialized-view rule from the independent provider floor:
    // the scope transition deliberately clears provider rows too.
    let replica = spawn_node_with("replica", |config| config.replica_release_floor = 0).await;
    introduce(&[&publisher, &replica]);

    publisher
        .node
        .add_filesystem_source("media", publisher.space.path())
        .unwrap();
    std::fs::write(publisher.space.path().join("a.bin"), b"content").unwrap();
    publisher.node.scan_publish_push().await.unwrap();

    let replicating = replica.node.clone();
    off_runtime(move || {
        replicating
            .add_replica("media", ReplicaPolicy::Current, Some(0), None, None)
            .unwrap();
    })
    .await;
    converge(&replica.node, &publisher.node).await;
    assert_eq!(coverage(&replica.node).await.held, 1);

    // A scope change discards every foreign origin's entries and drops their
    // complete heads back to pending. Until promotion, that origin contributes
    // no roots to the new scoped materialized view.
    let scoped = replica.node.clone();
    let state = off_runtime(move || {
        scoped
            .store()
            .set_read_scope(Some(&["media".to_string()]))
            .unwrap();
        let state = scoped.view_state().unwrap();
        scoped.sweep_replicas(None).unwrap();
        state
    })
    .await;

    assert!(
        matches!(state, ViewState::Incomplete(_)),
        "a demoted head must make the view incomplete, got {state:?}"
    );
    let after = coverage(&replica.node).await;
    assert_eq!(
        after.releasing, 0,
        "zero grace expires the release in the same sweep"
    );
    assert_eq!(
        after.held, 0,
        "an incomplete origin no longer blocks the sweep"
    );

    shutdown(&[&publisher.node, &replica.node]).await;
}

/// A want survives a restart, which is what makes the guarantee about observed
/// versions rather than about uptime (§3.3).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unfetchable_want_persists_and_backs_off() {
    let _blocking = synch_core::BlockingScope::enter();
    let replica = spawn("replica").await;

    let node = replica.node.clone();
    let (wanted, first_attempt) = off_runtime(move || {
        node.add_replica("media", ReplicaPolicy::Current, None, None, None)
            .unwrap();
        // An entry naming content nobody here holds and nobody advertises: the
        // shape of a version whose last provider left before this node
        // reached it.
        let absent = synch_core::Hash::new(b"gone from the cluster");
        node.store()
            .put_entry(
                &synch_core::OriginId::named("nas", "cluster.example").unwrap(),
                "media",
                "lost.bin",
                &synch_core::FileEntry::file(64, 1, absent, 1),
            )
            .unwrap();
        node.sweep_replicas(None).unwrap();
        let wanted = node.store().wants_of(&holder()).unwrap();
        // Ready immediately, before any attempt has been made.
        let ready = node
            .store()
            .wants_to_attempt(0, 60_000_000_000, 3_600_000_000_000, 8, 0)
            .unwrap();
        (wanted, ready.len())
    })
    .await;
    assert_eq!(wanted.len(), 1, "the sweep should want the missing object");
    assert_eq!(wanted[0].size, 64, "carrying the size the entry knew");
    assert_eq!(first_attempt, 1);

    // The fetch cannot succeed, and the failure is recorded rather than lost.
    let report = replica.node.fetch_content_wants().await.unwrap();
    assert_eq!(report.held, 0);
    assert_eq!(report.failed, 1);

    let store = replica.node.store().clone();
    let (attempts, ready_now) = off_runtime(move || {
        let wants = store.wants_of(&holder()).unwrap();
        let now = synch_core::now_ns();
        let ready = store
            .wants_to_attempt(now, 60_000_000_000, 3_600_000_000_000, 8, 0)
            .unwrap();
        (wants[0].attempts, ready.len())
    })
    .await;
    assert_eq!(attempts, 1, "the want records its failure and stays");
    assert_eq!(
        ready_now, 0,
        "and waits out its backoff rather than spinning"
    );

    shutdown(&[&replica.node]).await;
}

/// Removing a source leaves the independent replica and its holds intact.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removing_a_source_does_not_remove_its_replica() {
    let _blocking = synch_core::BlockingScope::enter();
    let publisher = spawn("publisher").await;
    let replica = spawn("replica").await;
    introduce(&[&publisher, &replica]);

    publisher
        .node
        .add_filesystem_source("media", publisher.space.path())
        .unwrap();
    std::fs::write(publisher.space.path().join("a.bin"), b"content").unwrap();
    publisher.node.scan_publish_push().await.unwrap();

    let replicating = replica.node.clone();
    off_runtime(move || {
        replicating.add_api_source("media").unwrap();
        replicating
            .add_replica("media", ReplicaPolicy::Current, None, None, None)
            .unwrap();
    })
    .await;
    converge(&replica.node, &publisher.node).await;

    let node = replica.node.clone();
    let staged = off_runtime(move || node.source_removal("media").unwrap()).await;
    replica.node.publish(&staged).unwrap();
    replica.node.finish_source_removal("media").unwrap();
    let node = replica.node.clone();
    let held_after = off_runtime(move || {
        assert!(node.store().source("media").unwrap().is_none());
        assert!(node.store().replica("media").unwrap().is_some());
        node.store().pins().unwrap().len()
    })
    .await;
    assert_eq!(held_after, 1, "the independent replica keeps its content");

    let node = replica.node.clone();
    let released_after = off_runtime(move || {
        node.remove_replica("media", false).unwrap();
        node.store().pins().unwrap().len()
    })
    .await;
    assert_eq!(released_after, 0, "removing the replica releases its holds");

    shutdown(&[&publisher.node, &replica.node]).await;
}

/// A batch that mixes a success with a failure attributes each to itself.
///
/// `futures_join` returns outputs in completion order, so pairing them with the
/// input order credits whichever want finished in another's place. The failure
/// then lands on the object that succeeded — whose want row is already gone, so
/// nothing records it — and the object that failed is counted as held, keeping
/// no attempt, no backoff, and no path to the `unreachable` count that exists
/// to say a version is gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mixed_batch_records_each_outcome_against_its_own_want() {
    let _blocking = synch_core::BlockingScope::enter();
    let publisher = spawn("publisher").await;
    let replica = spawn("replica").await;
    introduce(&[&publisher, &replica]);

    publisher
        .node
        .add_filesystem_source("media", publisher.space.path())
        .unwrap();
    std::fs::write(publisher.space.path().join("real.bin"), b"fetchable").unwrap();
    publisher.node.scan_publish_push().await.unwrap();

    let replicating = replica.node.clone();
    let unreachable = synch_core::Hash::new(b"no provider will ever serve this");
    off_runtime(move || {
        replicating
            .add_replica("media", ReplicaPolicy::Current, None, None, None)
            .unwrap();
    })
    .await;
    replica
        .node
        .sync_with_peer(&publisher.node.node_id())
        .await
        .unwrap();

    // A second want in the same batch that nobody can serve.
    let staging = replica.node.clone();
    off_runtime(move || {
        staging
            .store()
            .stage_want(&unreachable, &holder(), 4096, None, 1)
            .unwrap();
    })
    .await;

    let report = replica.node.fetch_content_wants().await.unwrap();
    assert_eq!(report.held, 1, "exactly the fetchable object was held");
    assert_eq!(report.failed, 1, "exactly the unreachable one failed");

    let store = replica.node.store().clone();
    let wants = off_runtime(move || store.wants_of(&holder()).unwrap()).await;
    assert_eq!(wants.len(), 1, "the failed want survives");
    assert_eq!(wants[0].root, unreachable, "and it is the one that failed");
    assert_eq!(
        wants[0].attempts, 1,
        "with its own failure recorded against it, so it can back off and \
         eventually be reported as unreachable"
    );

    shutdown(&[&publisher.node, &replica.node]).await;
}

/// A budget stops fetching and never shortens a release (§3.8).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_budget_stops_fetching_and_releases_nothing() {
    let _blocking = synch_core::BlockingScope::enter();
    let publisher = spawn("publisher").await;
    let replica = spawn("replica").await;
    introduce(&[&publisher, &replica]);

    publisher
        .node
        .add_filesystem_source("media", publisher.space.path())
        .unwrap();
    std::fs::write(publisher.space.path().join("big.bin"), vec![7u8; 4096]).unwrap();
    publisher.node.scan_publish_push().await.unwrap();

    // A zero-byte ceiling admits no non-empty object; it is not an alias for
    // the absence of a ceiling.
    let replicating = replica.node.clone();
    off_runtime(move || {
        replicating
            .add_replica("media", ReplicaPolicy::Current, Some(3600), Some(0), None)
            .unwrap();
    })
    .await;
    converge(&replica.node, &publisher.node).await;

    let after = coverage(&replica.node).await;
    assert_eq!(after.held, 0, "the object does not fit under the ceiling");
    assert_eq!(
        after.wanted, 1,
        "and stays wanted: a budget is a storage problem, and dropping the want \
         would turn it into a silent data-loss one"
    );
    let store = replica.node.store().clone();
    let attempts = off_runtime(move || store.wants_of(&holder()).unwrap()[0].attempts).await;
    assert_eq!(
        attempts, 0,
        "a budget is not the want's fault, so it must not be charged an attempt \
         and pushed toward being reported unreachable"
    );

    // Raising the ceiling lets the same want through, untouched.
    let raising = replica.node.clone();
    off_runtime(move || {
        raising
            .set_replica("media", None, None, Some(Some(1 << 20)), None)
            .unwrap();
    })
    .await;
    replica.node.fetch_content_wants().await.unwrap();
    assert_eq!(coverage(&replica.node).await.held, 1);

    shutdown(&[&publisher.node, &replica.node]).await;
}

/// Over-budget spaces must not consume candidate selection before the engine
/// reaches a healthy space in the same pass.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn over_budget_holders_do_not_hide_an_in_budget_holder() {
    let _blocking = synch_core::BlockingScope::enter();
    let replica = spawn_node_with("replica", |config| config.replica_concurrency = 2).await;

    let node = replica.node.clone();
    let healthy = off_runtime(move || {
        for space in ["a", "b"] {
            node.add_replica(space, ReplicaPolicy::Current, None, Some(0), None)
                .unwrap();
            node.store()
                .stage_want(
                    &synch_core::Hash::new(format!("over budget {space}").as_bytes()),
                    &PinHolder::Replica(space.to_string()),
                    10,
                    None,
                    1,
                )
                .unwrap();
        }
        node.add_replica("c", ReplicaPolicy::Current, None, None, None)
            .unwrap();
        let healthy = node
            .store()
            .ingest_bytes(b"fits the healthy holder", 1)
            .unwrap();
        node.store()
            .stage_want(
                &healthy,
                &PinHolder::Replica("c".to_string()),
                b"fits the healthy holder".len() as u64,
                None,
                1,
            )
            .unwrap();
        healthy
    })
    .await;

    let report = replica.node.fetch_content_wants().await.unwrap();
    assert_eq!(report.over_budget, 2, "both zero-budget wants are skipped");
    assert_eq!(
        report.held, 1,
        "the healthy holder must run in the same pass"
    );
    assert!(replica
        .node
        .store()
        .pins_for(&healthy)
        .unwrap()
        .iter()
        .any(|pin| pin.holder == PinHolder::Replica("c".to_string())));

    shutdown(&[&replica.node]).await;
}

/// A GC pass between the fetch and the pin does not take the bytes.
///
/// §3.5 asks for this by name: "Between the fetch's last commit and the pin
/// insert the object is complete, possibly unreferenced, and unpinned … that
/// deserves a test that runs a GC pass inside the window rather than a
/// paragraph asserting it." It is the one window in which a replica can lose
/// bytes it has just paid to fetch, and what protects it is an incidental
/// property of an unrelated clock — `last_access` is stamped by the write, and
/// `gc_content` skips anything newer than its horizon. A change to either
/// would open it with every other test still green.
///
/// Reaching that state takes more than fetching and not pinning. A replica
/// stages a want in the same transaction that materializes the entry, so a
/// wanted root is a referenced root, and a pass over one is decided by
/// `referenced_content` without the retention clock ever being consulted — the
/// test would then pass with both age checks deleted. So the reference is taken
/// away: the publisher removes the file, the replica syncs, its entry goes and
/// its want self-cleans, and what is left is precisely the window's state —
/// complete, unreferenced, unpinned, and spared by nothing but its age.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_gc_pass_between_the_fetch_and_the_pin_leaves_the_object_alone() {
    let _blocking = synch_core::BlockingScope::enter();
    let publisher = spawn("publisher").await;
    let replica = spawn("replica").await;
    introduce(&[&publisher, &replica]);

    publisher
        .node
        .add_filesystem_source("media", publisher.space.path())
        .unwrap();
    std::fs::write(publisher.space.path().join("a.bin"), b"paid for once").unwrap();
    publisher.node.scan_publish_push().await.unwrap();

    let replicating = replica.node.clone();
    off_runtime(move || {
        replicating
            .add_replica("media", ReplicaPolicy::Current, None, None, None)
            .unwrap();
    })
    .await;
    replica
        .node
        .sync_with_peer(&publisher.node.node_id())
        .await
        .unwrap();

    // Fetch the bytes without taking possession, which is the window.
    let want = {
        let store = replica.node.store().clone();
        off_runtime(move || store.wants_of(&holder()).unwrap()).await
    };
    assert_eq!(want.len(), 1, "the promotion should have staged the want");
    let fetched = replica
        .node
        .fetch_all(&want[0].root, want[0].size)
        .await
        .unwrap();
    assert!(fetched.complete);

    // Take the reference away, which is what puts the root into the window's
    // state rather than merely near it.
    std::fs::remove_file(publisher.space.path().join("a.bin")).unwrap();
    publisher.node.scan_publish_push().await.unwrap();
    replica
        .node
        .sync_with_peer(&publisher.node.node_id())
        .await
        .unwrap();

    // Nothing references it — no entry of this node's own names it, and no pin
    // stands for it. A maintenance pass here is entitled to collect it on every
    // rule except its age. Asserted, not assumed: if a later change leaves the
    // entry or the want standing, this says so instead of quietly going back to
    // testing nothing.
    let root = want[0].root;
    let store = replica.node.store().clone();
    let (referenced, pins, wants) = off_runtime(move || {
        (
            store.content_is_referenced(&root).unwrap(),
            store.pins_for(&root).unwrap().len(),
            store.wants_of(&holder()).unwrap().len(),
        )
    })
    .await;
    assert!(!referenced, "the entry that named the root should be gone");
    assert_eq!(pins, 0, "and the fetch took no possession");
    assert_eq!(wants, 0, "and the want self-cleaned with the tree");

    let sweeping = replica.node.clone();
    off_runtime(move || sweeping.maintenance_pass().unwrap()).await;

    let store = replica.node.store().clone();
    let survived = off_runtime(move || store.blob(&root).unwrap()).await;
    assert!(
        survived.is_some_and(|blob| blob.complete),
        "a GC pass inside the fetch-to-pin window must not take bytes the \
         replica has just paid for"
    );

    // And the bytes that survived are usable: the file comes back, and the
    // replica takes possession without paying for the transfer twice.
    std::fs::write(publisher.space.path().join("a.bin"), b"paid for once").unwrap();
    publisher.node.scan_publish_push().await.unwrap();
    converge(&replica.node, &publisher.node).await;
    assert_eq!(coverage(&replica.node).await.held, 1);

    shutdown(&[&publisher.node, &replica.node]).await;
}

/// A space this node only replicates is not one it publishes: it advertises no
/// `m:space` record, so `source rm` has none to strand (§3.2).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replica_advertises_nothing_of_its_own() {
    let _blocking = synch_core::BlockingScope::enter();
    let publisher = spawn("publisher").await;
    let replica = spawn("replica").await;
    introduce(&[&publisher, &replica]);

    publisher
        .node
        .add_filesystem_source("media", publisher.space.path())
        .unwrap();
    std::fs::write(publisher.space.path().join("a.bin"), b"content").unwrap();
    publisher.node.scan_publish_push().await.unwrap();

    // The replica has its own space too, so this is not testing a node that
    // publishes nothing at all — it is testing that the two are told apart.
    replica
        .node
        .add_filesystem_source("mine", replica.space.path())
        .unwrap();
    let replicating = replica.node.clone();
    off_runtime(move || {
        replicating
            .add_replica("media", ReplicaPolicy::Current, None, None, None)
            .unwrap();
    })
    .await;
    std::fs::write(replica.space.path().join("b.bin"), b"mine").unwrap();
    replica.node.scan_publish_push().await.unwrap();
    converge(&replica.node, &publisher.node).await;

    let node = replica.node.clone();
    let (own, replicated, source_role, replica_role) = off_runtime(move || {
        (
            node.space_info_of(node.origin(), "mine").unwrap(),
            node.space_info_of(node.origin(), "media").unwrap(),
            node.store().source("media").unwrap(),
            node.store().replica("media").unwrap(),
        )
    })
    .await;
    assert!(own.is_some(), "a filesystem source is advertised");
    assert!(
        replicated.is_none(),
        "a replica-only namespace publishes no source manifest"
    );
    assert!(source_role.is_none());
    assert!(replica_role.is_some());

    shutdown(&[&publisher.node, &replica.node]).await;
}

/// A replica of a space it also indexes is two independent halves of one row.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn source_and_replica_roles_can_coexist() {
    let _blocking = synch_core::BlockingScope::enter();
    let publisher = spawn("publisher").await;
    let both = spawn("both").await;
    introduce(&[&publisher, &both]);

    publisher
        .node
        .add_filesystem_source("media", publisher.space.path())
        .unwrap();
    std::fs::write(publisher.space.path().join("theirs.bin"), b"theirs").unwrap();
    publisher.node.scan_publish_push().await.unwrap();

    // This node publishes its own copy of `media` *and* holds everyone else's.
    both.node
        .add_filesystem_source("media", both.space.path())
        .unwrap();
    let replicating = both.node.clone();
    off_runtime(move || {
        replicating
            .add_replica("media", ReplicaPolicy::Current, None, None, None)
            .unwrap();
    })
    .await;
    std::fs::write(both.space.path().join("mine.bin"), b"mine").unwrap();
    both.node.scan_publish_push().await.unwrap();
    converge(&both.node, &publisher.node).await;

    let after = coverage(&both.node).await;
    assert_eq!(
        after.held, 2,
        "both its own version and the publisher's are held"
    );

    // The checkout is untouched by any of it.
    assert!(both.space.path().join("mine.bin").exists());
    assert!(
        !both.space.path().join("theirs.bin").exists(),
        "replication without a checkout materializes nothing"
    );

    let node = both.node.clone();
    let path = off_runtime(move || node.store().source("media").unwrap().unwrap().local_path).await;
    assert!(path.is_some(), "and the space is still indexed");

    shutdown(&[&publisher.node, &both.node]).await;
}

/// Source and replica roles can be removed independently.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn source_and_replica_are_set_independently() {
    let _blocking = synch_core::BlockingScope::enter();
    let peer = spawn("peer").await;
    let node = peer.node.clone();
    let path = peer.space.path().to_path_buf();

    off_runtime(move || {
        node.add_filesystem_source("media", &path).unwrap();
        node.add_replica("media", ReplicaPolicy::Forever, None, Some(99), None)
            .unwrap();
        let row = node.store().replica("media").unwrap().unwrap();
        assert_eq!(row.retention, ReplicaPolicy::Forever);
        assert_eq!(row.budget, Some(99));
        assert!(node.store().source("media").unwrap().is_some());

        node.remove_replica("media", false).unwrap();
        assert!(node.store().replica("media").unwrap().is_none());
        assert!(
            node.store().source("media").unwrap().is_some(),
            "source remains"
        );
    })
    .await;

    tokio::time::timeout(Duration::from_secs(5), shutdown(&[&peer.node]))
        .await
        .unwrap();
}
