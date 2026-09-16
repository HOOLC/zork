//! Three in-process nodes on localhost iroh endpoints with static trust,
//! converging over the real protocols with no relay and no discovery.

use std::time::Duration;

use synch_engine::{Node, VersionPolicy};

mod common;
use common::{
    big_payload, off_runtime, shutdown, spawn_node as spawn, spawn_node_with as spawn_with, trust,
    trust_all as introduce,
};

/// Trust between bare nodes, so a multi-thread test can run it inside a
/// blocking scope.
fn introduce_nodes(nodes: &[&Node]) {
    for a in nodes {
        for b in nodes {
            if a.origin() != b.origin() {
                trust(a, b);
            }
        }
    }
}

/// A restored database may trail an acknowledged own-origin publish. The peer
/// copy is a full signed head, so startup adopts it before minting another.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn startup_readopts_a_newer_own_head_retained_by_a_peer() {
    let _blocking = synch_core::BlockingScope::enter();
    let publisher = spawn("publisher").await;
    let witness = spawn("witness").await;
    introduce(&[&publisher, &witness]);

    publisher
        .node
        .add_filesystem_source("media", publisher.space.path())
        .unwrap();
    std::fs::write(publisher.space.path().join("version.txt"), b"one").unwrap();
    let first = publisher.node.scan_publish_push().await.unwrap().unwrap();
    witness
        .node
        .sync_with_peer(&publisher.node.node_id())
        .await
        .unwrap();

    // Take an actual closed SQLite snapshot after the first acknowledged
    // publish, as Litestream would. The checkout and CAS volume are not part of
    // this metadata backup.
    let common::Peer {
        _data: publisher_data,
        space: publisher_space,
        node: first_publisher,
    } = publisher;
    first_publisher.shutdown().await.unwrap();
    drop(first_publisher);
    let backup = publisher_data.path().join("first-publish.backup");
    std::fs::copy(publisher_data.path().join(synch_store::DB_FILE), &backup).unwrap();
    let mut publisher = Node::open(synch_engine::NodeConfig::loopback(publisher_data.path()))
        .await
        .unwrap();
    introduce_nodes(&[&publisher, &witness.node]);

    std::fs::write(publisher_space.path().join("version.txt"), b"two").unwrap();
    let second = publisher.scan_publish_push().await.unwrap().unwrap();
    witness
        .node
        .sync_with_peer(&publisher.node_id())
        .await
        .unwrap();
    assert_eq!(
        witness
            .node
            .store()
            .complete_head(publisher.origin())
            .unwrap(),
        Some(second.clone())
    );
    let expected_entry = publisher
        .store()
        .entry(publisher.origin(), "media", "version.txt")
        .unwrap()
        .unwrap();

    // Restore the older file, reopen from it, and only then contact the peer.
    publisher.shutdown().await.unwrap();
    drop(publisher);
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(
            publisher_data
                .path()
                .join(format!("{}{suffix}", synch_store::DB_FILE)),
        );
    }
    std::fs::copy(&backup, publisher_data.path().join(synch_store::DB_FILE)).unwrap();
    publisher = Node::open(synch_engine::NodeConfig::loopback(publisher_data.path()))
        .await
        .unwrap();
    introduce_nodes(&[&publisher, &witness.node]);
    assert_eq!(publisher.own_head().unwrap(), Some(first));

    assert!(publisher.readopt_self_on_startup().await.unwrap());
    assert_eq!(publisher.own_head().unwrap(), Some(second));
    assert_eq!(
        publisher
            .store()
            .entry(publisher.origin(), "media", "version.txt")
            .unwrap()
            .unwrap(),
        expected_entry
    );

    shutdown(&[&publisher, &witness.node]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_source_adoption_promotes_to_cloud_before_publishing_its_own_reference() {
    let _blocking = synch_core::BlockingScope::enter();
    let source = spawn("source").await;
    let adopter = spawn_with("adopter", |config| {
        config.cloud = Some(synch_store::cloud::CloudConfig {
            service: synch_store::cloud::CloudService::Memory,
            options: Default::default(),
            scratch_dir: config.data_dir.join("cloud-scratch"),
            io_timeout: Duration::from_secs(5),
            upload_policy: synch_store::cloud::CloudUploadPolicy::Own,
            cache_bytes: Some(512 * 1024 * 1024),
        });
    })
    .await;
    introduce(&[&source, &adopter]);

    source
        .node
        .add_filesystem_source("media", source.space.path())
        .unwrap();
    let payload = big_payload(200_000);
    std::fs::write(source.space.path().join("adopt.bin"), &payload).unwrap();
    source.node.scan_publish_push().await.unwrap().unwrap();
    adopter
        .node
        .sync_with_peer(&source.node.node_id())
        .await
        .unwrap();
    adopter.node.add_api_source("media").unwrap();

    adopter
        .node
        .adopt_from(source.node.origin(), "media", "adopt.bin")
        .await
        .unwrap();
    let adopted = adopter
        .node
        .store()
        .entry(adopter.node.origin(), "media", "adopt.bin")
        .unwrap();
    assert!(adopted.is_none(), "the reference is staged until publish");
    adopter.node.flush_staged().await.unwrap().unwrap();
    let adopted = adopter
        .node
        .store()
        .entry(adopter.node.origin(), "media", "adopt.bin")
        .unwrap()
        .unwrap();
    let root = adopted.content.unwrap();
    assert!(
        adopter.node.store().blob(&root).unwrap().unwrap().durable,
        "take must override cache-only upload policy before publishing"
    );

    adopter
        .node
        .store()
        .reconcile_scratch_generation("replacement-container")
        .unwrap();
    assert_eq!(
        adopter
            .node
            .cas_backend()
            .read_range(root, 0, payload.len() as u64)
            .await
            .unwrap(),
        payload
    );

    shutdown(&[&source.node, &adopter.node]).await;
}

/// The §14 walkthrough: scan-publish-push, pull, a verified partial range
/// read, and a milestone ad (§6.3) turning a fetcher into a provider.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn three_nodes_converge_and_fetch_verified_content() {
    let _blocking = synch_core::BlockingScope::enter();
    let nas = spawn("nas").await;
    let laptop = spawn("laptop").await;
    let vps = spawn("vps").await;
    introduce(&[&nas, &laptop, &vps]);

    // The NAS publishes a filesystem source with a mix of small and large files.
    let keynote = big_payload(200_000);
    nas.node
        .add_filesystem_source("media", nas.space.path())
        .unwrap();
    std::fs::create_dir_all(nas.space.path().join("talks")).unwrap();
    std::fs::write(nas.space.path().join("notes.txt"), b"read me").unwrap();
    std::fs::write(nas.space.path().join("talks/keynote.mp4"), &keynote).unwrap();
    std::fs::write(nas.space.path().join("talks/slide00.txt"), b"slide 0").unwrap();
    std::fs::write(nas.space.path().join("talks/slide01.txt"), b"slide 1").unwrap();
    let head = nas.node.scan_publish_push().await.unwrap().unwrap();
    assert_eq!(head.seq, 1);

    // Both peers pull from the NAS; the push delivered the head, the pull completes its trie.
    for peer in [&laptop, &vps] {
        let report = peer.node.sync_with_peer(&nas.node.node_id()).await.unwrap();
        assert_eq!(report.tries_completed, 1, "{report:?}");
        assert_eq!(
            peer.node.store().complete_head(nas.node.origin()).unwrap(),
            Some(head.clone()),
            "head mismatch on {}",
            peer.node.origin()
        );
    }

    // The published tree's listing, and nobody holds content yet.
    let expected = nas
        .node
        .store()
        .list_entries(Some(nas.node.origin()), "media", "", None, None)
        .unwrap();
    assert_eq!(expected.len(), 4);
    let keynote_root = expected
        .iter()
        .find(|e| e.path == "talks/keynote.mp4")
        .unwrap()
        .content
        .unwrap();
    assert!(laptop.node.store().blob(&keynote_root).unwrap().is_none());

    // A range read in the middle of the large object fetches only the groups covering it.
    let slice = laptop
        .node
        .read_range(
            "media",
            "talks/keynote.mp4",
            &VersionPolicy::Origin(nas.node.origin().clone()),
            100_000,
            Some(4096),
        )
        .await
        .unwrap();
    assert_eq!(slice, &keynote[100_000..104_096]);
    let held = laptop.node.local_groups(&keynote_root).unwrap();
    assert!(
        held.count() < synch_core::group_count(keynote.len() as u64),
        "a range read must not drag the whole object across"
    );

    // A full read completes the object, byte for byte.
    assert_eq!(
        laptop
            .node
            .read_entry(nas.node.origin(), "media", "talks/keynote.mp4")
            .await
            .unwrap(),
        keynote
    );

    // The completed fetch published a milestone ad (§6.3); once the head
    // carrying it propagates, the object has a second provider.
    assert!(laptop
        .node
        .published_ad(&keynote_root)
        .unwrap()
        .expect("a completed object must be advertised")
        .is_complete());
    let ad_head = laptop.node.own_head().unwrap().unwrap();
    laptop.node.push_head(&ad_head).await.unwrap();
    vps.node
        .sync_with_peer(&laptop.node.node_id())
        .await
        .unwrap();
    let providers = vps.node.store().providers(&keynote_root).unwrap();
    assert!(
        providers.iter().any(|(o, _)| o == laptop.node.origin()),
        "the laptop's partial-then-complete copy must be discoverable"
    );

    shutdown(&[&nas.node, &laptop.node, &vps.node]).await;
}

/// §8 executed across real nodes: two origins publish different content for
/// the same `(space, path)`, a third sees one tree with one divergent path,
/// selects deterministically, and adoption ends the divergence.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_unified_tree_carries_every_version_of_a_divergent_path() {
    let _blocking = synch_core::BlockingScope::enter();
    let nas = spawn("nas").await;
    let laptop = spawn("laptop").await;
    let vps = spawn("vps").await;
    introduce(&[&nas, &laptop, &vps]);

    nas.node
        .add_filesystem_source("media", nas.space.path())
        .unwrap();
    laptop
        .node
        .add_filesystem_source("media", laptop.space.path())
        .unwrap();
    std::fs::write(nas.space.path().join("shared.txt"), b"from the nas").unwrap();
    nas.node.scan_publish_push().await.unwrap().unwrap();
    // Distinct mtimes, which `newest` reads as published.
    tokio::time::sleep(Duration::from_millis(20)).await;
    std::fs::write(laptop.space.path().join("shared.txt"), b"from the laptop").unwrap();
    laptop.node.scan_publish_push().await.unwrap().unwrap();

    // The observer pulls both versions; the NAS pulls the laptop's too, so the adoption below can read what it replaces.
    nas.node
        .sync_with_peer(&laptop.node.node_id())
        .await
        .unwrap();
    vps.node.sync_with_peer(&nas.node.node_id()).await.unwrap();
    vps.node
        .sync_with_peer(&laptop.node.node_id())
        .await
        .unwrap();

    // The observer's unified view: one tree, one path, two versions.
    let listing = vps.node.unified_listing("media", "", None, None).unwrap();
    assert_eq!(listing.len(), 1, "one path, not one per origin");
    assert_eq!(listing[0].path, "shared.txt");
    assert_eq!(listing[0].version_count(), 2);
    assert!(listing[0].exists());

    // `newest` is a deterministic total order over the same tree.
    let selected = vps
        .node
        .resolve("media", "shared.txt", &VersionPolicy::Newest)
        .unwrap();
    assert_eq!(
        selected.origin,
        *laptop.node.origin(),
        "the newer mtime wins"
    );
    assert_eq!(
        vps.node
            .read_path("media", "shared.txt", &VersionPolicy::Newest)
            .await
            .unwrap(),
        b"from the laptop"
    );

    // An origin pin — what `--from` builds — reads the other version.
    assert_eq!(
        vps.node
            .read_path(
                "media",
                "shared.txt",
                &VersionPolicy::Origin(nas.node.origin().clone())
            )
            .await
            .unwrap(),
        b"from the nas"
    );

    // `strict` refuses, and names both versions in the refusal.
    let err = vps
        .node
        .resolve("media", "shared.txt", &VersionPolicy::Strict)
        .unwrap_err();
    let text = err.to_string();
    assert!(text.contains("nas@cluster.example"), "{text}");
    assert!(text.contains("laptop@cluster.example"), "{text}");

    // Adoption ends divergence: the nas takes the laptop's version as its own,
    // republishing with `prev` pointing at what it replaced.
    let theirs = nas
        .node
        .read_entry(laptop.node.origin(), "media", "shared.txt")
        .await
        .unwrap();
    nas.node.adopt("media", "shared.txt", &theirs).unwrap();
    nas.node.scan_publish_push().await.unwrap().unwrap();
    let mine = nas
        .node
        .store()
        .entry(nas.node.origin(), "media", "shared.txt")
        .unwrap()
        .unwrap();
    assert_eq!(mine.prev, Some(synch_core::Hash::new(b"from the nas")));
    vps.node.sync_with_peer(&nas.node.node_id()).await.unwrap();

    let set = vps.node.versions("media", "shared.txt").unwrap();
    assert_eq!(set.version_count(), 1, "one version, two attestors");
    assert!(!set.is_divergent());
    assert_eq!(set.versions[0].attestors.len(), 2);
    // And `strict` now reads it without complaint.
    assert_eq!(
        vps.node
            .read_path("media", "shared.txt", &VersionPolicy::Strict)
            .await
            .unwrap(),
        b"from the laptop"
    );

    shutdown(&[&nas.node, &laptop.node, &vps.node]).await;
}

/// §5.3: the periodic pull is what guarantees convergence after a partition
/// heals — one anti-entropy round jumps straight to the newest head.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn convergence_survives_a_partition() {
    let _blocking = synch_core::BlockingScope::enter();
    let nas = spawn("nas").await;
    let laptop = spawn("laptop").await;

    nas.node
        .add_filesystem_source("media", nas.space.path())
        .unwrap();
    for round in 1..=3 {
        std::fs::write(
            nas.space.path().join(format!("round{round}.txt")),
            format!("content {round}").as_bytes(),
        )
        .unwrap();
        // Publish without pushing: the laptop is "partitioned" — it does not know the NAS exists yet.
        nas.node.scan_and_publish().unwrap();
    }
    let final_head = nas
        .node
        .store()
        .complete_head(nas.node.origin())
        .unwrap()
        .unwrap();
    assert_eq!(final_head.seq, 3);

    // The partition heals: trust is established and one pull round runs.
    introduce(&[&nas, &laptop]);
    let report = tokio::time::timeout(Duration::from_secs(30), laptop.node.anti_entropy_round())
        .await
        .unwrap()
        .unwrap();
    assert!(report.peer.is_some(), "{report:?}");

    // One round is enough: the laptop jumps straight to the newest head, not each intermediate root.
    assert_eq!(
        laptop
            .node
            .store()
            .complete_head(nas.node.origin())
            .unwrap(),
        Some(final_head)
    );

    shutdown(&[&nas.node, &laptop.node]).await;
}

/// A reachable but stale member must not consume the one anti-entropy peer a
/// round contacts and leave newer state waiting for a later interval.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anti_entropy_reaches_a_fresh_peer_beside_a_stale_one() {
    let _blocking = synch_core::BlockingScope::enter();
    let source = spawn("source").await;
    let stale = spawn("stale").await;
    let replica = spawn("replica").await;
    introduce(&[&source, &stale, &replica]);

    source
        .node
        .add_filesystem_source("media", source.space.path())
        .unwrap();
    std::fs::write(source.space.path().join("fresh.txt"), b"fresh").unwrap();
    let head = source.node.scan_and_publish().unwrap().1.unwrap();
    assert_eq!(
        stale
            .node
            .store()
            .complete_head(source.node.origin())
            .unwrap(),
        None,
        "the stale peer must not learn the head before the tested round"
    );

    let report = replica.node.anti_entropy_round().await.unwrap();
    assert!(
        report.peer.is_some(),
        "at least one healthy peer should answer"
    );
    assert_eq!(
        replica
            .node
            .store()
            .complete_head(source.node.origin())
            .unwrap(),
        Some(head),
        "the stale peer must not hide the source's newer head"
    );

    shutdown(&[&source.node, &stale.node, &replica.node]).await;
}

/// §9.2: a pin starts by fetching what it guards — pinning content this node
/// has never read must not mark zero rows and report success.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pinning_fetches_what_it_promises_to_keep() {
    let _blocking = synch_core::BlockingScope::enter();
    let nas = spawn("nas").await;
    let laptop = spawn("laptop").await;
    introduce(&[&nas, &laptop]);

    nas.node
        .add_filesystem_source("media", nas.space.path())
        .unwrap();
    std::fs::write(nas.space.path().join("report.md"), b"keep these bytes").unwrap();
    nas.node.scan_and_publish().unwrap();
    laptop.node.anti_entropy_round().await.unwrap();

    let entry = laptop
        .node
        .store()
        .entry(nas.node.origin(), "media", "report.md")
        .unwrap()
        .unwrap();
    let root = entry.content.unwrap();
    // Metadata only so far: the pin is what brings the bytes here.
    assert!(laptop.node.store().blob(&root).unwrap().is_none());

    laptop
        .node
        .pin_object(&root, Some(entry.size))
        .await
        .unwrap();
    assert_eq!(laptop.node.store().pinned_blobs().unwrap(), vec![root]);
    assert!(laptop.node.store().blob(&root).unwrap().unwrap().complete);
    assert_eq!(
        laptop.node.store().read_all(&root).unwrap(),
        b"keep these bytes"
    );

    // A bare root nobody holds has no size to fetch by, and is refused rather
    // than recorded as a pin that guards nothing.
    let absent = synch_core::Hash::new(b"never published");
    assert!(laptop.node.pin_object(&absent, None).await.is_err());
    assert_eq!(laptop.node.store().pinned_blobs().unwrap(), vec![root]);

    shutdown(&[&nas.node, &laptop.node]).await;
}

/// §5.4 end to end: publish, overwrite, then one maintenance pass with
/// nothing in retention — the old root leaves `head_history`, its private trie
/// nodes are swept, its bytes leave the CAS; a pin survives, and a fresh peer
/// still pulls the current root (the former gc test's property).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn maintenance_prunes_history_sweeps_the_trie_and_reclaims_bytes() {
    let _blocking = synch_core::BlockingScope::enter();
    // Retention of nothing: everything published before "now" is swept.
    let peer = spawn_with("nas", |config| {
        config.root_retention = Duration::from_nanos(1)
    })
    .await;
    let node = &peer.node;
    node.add_filesystem_source("media", peer.space.path())
        .unwrap();

    std::fs::write(peer.space.path().join("notes.txt"), b"first revision").unwrap();
    node.scan_and_publish().unwrap();
    let old_root = node.current_root().unwrap();
    let old_content = node
        .store()
        .entry(node.origin(), "media", "notes.txt")
        .unwrap()
        .unwrap()
        .content
        .unwrap();
    assert!(node.store().blob(&old_content).unwrap().is_some());

    // A pinned object with no entry pointing at it: retention must not touch it.
    let pinned = node.store().ingest_bytes(b"pinned bytes", 0).unwrap();
    node.store()
        .pin(
            &pinned,
            &synch_store::PinHolder::Operator,
            synch_core::now_ns(),
        )
        .unwrap();

    std::fs::write(peer.space.path().join("notes.txt"), b"second revision").unwrap();
    // A distinct mtime, so the scanner cannot mistake the rewrite for no change.
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(peer.space.path().join("notes.txt"), b"second revision").unwrap();
    node.scan_and_publish().unwrap();
    assert_ne!(node.current_root().unwrap(), old_root);
    // Before maintenance: the old root is retained history.
    assert!(node
        .store()
        .head_history(node.origin())
        .unwrap()
        .iter()
        .any(|h| h.root == old_root));

    node.maintenance_pass().unwrap();

    assert!(
        !node
            .store()
            .head_history(node.origin())
            .unwrap()
            .iter()
            .any(|h| h.root == old_root),
        "the displaced root must leave retention"
    );
    assert!(
        !synch_mpt::NodeStore::has_node(node.store().as_ref(), &old_root).unwrap(),
        "the old root node itself is gone"
    );
    assert!(
        node.store().blob(&old_content).unwrap().is_none(),
        "the superseded content must leave the index"
    );
    assert!(node.store().read_all(&old_content).is_err());
    assert!(
        node.store().blob(&pinned).unwrap().is_some(),
        "a pin outlives retention"
    );
    assert_eq!(
        node.read_entry(node.origin(), "media", "notes.txt")
            .await
            .unwrap(),
        b"second revision"
    );

    // The current root is still fully servable to a peer that has never synced.
    let laptop = spawn("laptop").await;
    trust(&laptop.node, node);
    trust(node, &laptop.node);
    let report = laptop.node.anti_entropy_round().await.unwrap();
    assert!(report.peer.is_some(), "{report:?}");
    assert_eq!(
        laptop
            .node
            .store()
            .complete_head(node.origin())
            .unwrap()
            .map(|h| h.seq),
        Some(2)
    );
    assert_eq!(
        laptop
            .node
            .read_entry(node.origin(), "media", "notes.txt")
            .await
            .unwrap(),
        b"second revision"
    );

    shutdown(&[node, &laptop.node]).await;
}

/// §8: deletions are adoptable exactly as content is — a tombstone on one side
/// and a live file on the other is deletion divergence, ended by someone
/// taking the other's assertion.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deletion_is_adopted_and_the_path_leaves_the_tree() {
    let _blocking = synch_core::BlockingScope::enter();
    let nas = spawn("nas").await;
    let laptop = spawn("laptop").await;
    introduce(&[&nas, &laptop]);

    nas.node
        .add_filesystem_source("shared", nas.space.path())
        .unwrap();
    laptop
        .node
        .add_filesystem_source("shared", laptop.space.path())
        .unwrap();

    // Both publish the same file, so the path starts out unanimous.
    for peer in [&nas, &laptop] {
        std::fs::write(peer.space.path().join("notes.txt"), b"hello").unwrap();
        peer.node.scan_and_publish().unwrap();
    }
    laptop
        .node
        .sync_with_peer(&nas.node.node_id())
        .await
        .unwrap();
    let set = laptop.node.versions("shared", "notes.txt").unwrap();
    assert_eq!(set.version_count(), 1, "{:?}", set.versions);

    // The NAS deletes it. Now the tree carries a live version and a tombstone.
    std::fs::remove_file(nas.space.path().join("notes.txt")).unwrap();
    nas.node.scan_and_publish().unwrap();
    laptop
        .node
        .sync_with_peer(&nas.node.node_id())
        .await
        .unwrap();

    let set = laptop.node.versions("shared", "notes.txt").unwrap();
    assert_eq!(set.version_count(), 2, "deletion divergence");
    assert!(set.exists(), "a live version keeps the path visible");

    // The laptop takes the NAS's deletion.
    let removed = laptop
        .node
        .adopt_deletion("shared", "notes.txt")
        .unwrap()
        .expect("our copy was here");
    // The path is reported under the *stored* space root, canonicalized at
    // `source add` time — on macOS `/var/…` is a symlink to `/private/var/…`,
    // so the raw tempdir path would not compare equal.
    let canonical_space = laptop.space.path().canonicalize().unwrap();
    assert_eq!(removed, canonical_space.join("notes.txt"));
    assert!(!laptop.space.path().join("notes.txt").exists());

    let head = laptop.node.scan_publish_push().await.unwrap().unwrap();
    assert!(head.seq > 1);

    // The laptop now publishes its own tombstone, both sides agree, and the path has left the unified tree.
    let set = laptop.node.versions("shared", "notes.txt").unwrap();
    assert_eq!(set.version_count(), 1, "{:?}", set.versions);
    assert!(set.versions[0].is_tombstone());

    // The same guard content adoption takes: outside a configured space
    // nothing would publish the adoption, so the write would be a silent no-op
    // with a filesystem side effect — refused instead. A path that is simply
    // not here is not an error: the assertion being adopted is "this is gone",
    // and it already is.
    let err = laptop
        .node
        .adopt_deletion("nowhere", "notes.txt")
        .unwrap_err()
        .to_string();
    assert!(err.contains("space nowhere"), "{err}");
    assert!(laptop
        .node
        .adopt_deletion("shared", "never-existed.txt")
        .unwrap()
        .is_none());

    shutdown(&[&nas.node, &laptop.node]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pushed_head_is_followed_by_its_trie_without_waiting_for_the_interval() {
    let _blocking = synch_core::BlockingScope::enter();
    // Intervals of ten minutes: nothing clock-driven can complete this.
    let nas = spawn_with("nas", |c| c.aae_interval = Duration::from_secs(600)).await;
    let laptop = spawn_with("laptop", |c| c.aae_interval = Duration::from_secs(600)).await;
    {
        let (a, b) = (nas.node.clone(), laptop.node.clone());
        off_runtime(move || introduce_nodes(&[&a, &b])).await;
    }
    off_runtime({
        let node = nas.node.clone();
        let path = nas.space.path().to_path_buf();
        move || node.add_filesystem_source("media", &path).unwrap()
    })
    .await;
    std::fs::write(nas.space.path().join("a.txt"), b"hello").unwrap();

    // The follower's standing loop, which is what the bell has to reach.
    let (stop, _) = tokio::sync::broadcast::channel::<()>(1);
    let running = {
        let node = laptop.node.clone();
        let mut rx = stop.subscribe();
        tokio::spawn(async move {
            node.run_anti_entropy(async move {
                let _ = rx.recv().await;
            })
            .await
        })
    };

    // Publish and push: the head lands in the follower's pending slot; the bell turns that into a fetch.
    let head = tokio::time::timeout(Duration::from_secs(30), nas.node.scan_publish_push())
        .await
        .unwrap()
        .unwrap()
        .expect("a head");

    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let complete = loop {
        let got = {
            let (node, origin) = (laptop.node.clone(), nas.node.origin().clone());
            off_runtime(move || node.store().complete_head(&origin).unwrap()).await
        };
        if got.is_some() || std::time::Instant::now() > deadline {
            break got;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(
        complete.map(|h| h.root),
        Some(head.root),
        "the trie must follow the pushed head without a full interval passing"
    );

    let _ = stop.send(());
    let _ = running.await;
    shutdown(&[&nas.node, &laptop.node]).await;
}
