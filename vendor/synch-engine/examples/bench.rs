//! A benchmark of the operations a running node actually spends its time in.
//!
//! Two in-process nodes on loopback iroh endpoints, no relay and no discovery,
//! driven through the real protocols — this measures the code paths §5 and §6
//! describe, not a model of them. Everything is timed in one process against
//! temporary directories, so absolute numbers belong to the machine and the
//! filesystem it ran on; the ratios and the shapes are the point.
//!
//! ```sh
//! cargo run --release -p synch-engine --example bench
//! cargo run --release -p synch-engine --example bench -- --files 50000 --object-mib 512
//! ```
//!
//! Release mode matters: BLAKE3 and the trie are both several times slower
//! without optimization, and a debug number here would be worse than none.

use std::{
    path::Path,
    time::{Duration, Instant},
};

use synch_core::{now_ns, Hash, OriginId};
use synch_engine::{Node, NodeConfig, VersionPolicy};
use synch_store::{Binding, BindingSource};

/// How many files the metadata benchmarks index.
const DEFAULT_FILES: usize = 10_000;
/// How large the object the transfer benchmarks move is, in MiB.
const DEFAULT_OBJECT_MIB: usize = 256;
/// How many times each point-lookup benchmark runs.
const LOOKUP_ITERATIONS: usize = 2_000;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let options = Options::parse();
    println!(
        "synchronicity bench — {} files, {} MiB object, {} lookup iterations\n",
        options.files, options.object_mib, LOOKUP_ITERATIONS
    );

    let publisher = Peer::spawn("nas").await;
    let follower = Peer::spawn("laptop").await;
    introduce(&[&publisher, &follower]);

    metadata(&options, &publisher, &follower).await;
    lookups(&options, &publisher, &follower);
    content(&options, &publisher, &follower).await;

    publisher.node.shutdown().await.unwrap();
    follower.node.shutdown().await.unwrap();
}

// ---- metadata: scan, publish, anti-entropy ---------------------------------

async fn metadata(options: &Options, publisher: &Peer, follower: &Peer) {
    section("metadata");
    let files = options.files;

    publisher
        .node
        .add_filesystem_source("media", publisher.space.path())
        .unwrap();
    write_tree(publisher.space.path(), files);

    // Indexing: walk, stat, hash, stage, and turn the batch into one signed
    // root. This is `synch source scan` on a cold space.
    let scan = time(|| {
        let (report, head) = publisher.node.scan_and_publish().unwrap();
        assert_eq!(report.hashed, files);
        assert!(head.is_some());
    });
    rate("scan + publish, cold", scan, files as f64, "files");

    // A rescan that finds nothing changed: the stat check on every file and
    // nothing else. This is what a watcher-triggered rescan usually costs.
    let rescan = time(|| {
        let (report, head) = publisher.node.scan_and_publish().unwrap();
        assert_eq!(report.unchanged, files);
        assert!(head.is_none(), "an unchanged tree publishes no head");
    });
    rate("rescan, nothing changed", rescan, files as f64, "files");

    // Anti-entropy from empty: the follower learns every path and object root
    // in the trie, verifying each node against the hash it asked for.
    let cold = time_async(|| async {
        let report = follower
            .node
            .sync_with_peer(&publisher.node.node_id())
            .await
            .unwrap();
        assert_eq!(report.tries_completed, 1, "{report:?}");
    })
    .await;
    rate("AAE sync, cold (whole trie)", cold, files as f64, "entries");
    assert_eq!(entries(follower, publisher, "media").len(), files);

    // The first converged exchange pays for establishing that the trie it just
    // fetched is whole — a walk of everything reachable, owed once per root.
    let first_idle = time_async(|| async {
        follower
            .node
            .sync_with_peer(&publisher.node.node_id())
            .await
            .unwrap();
    })
    .await;
    line(
        "AAE sync, converged (first)",
        &format!("{first_idle:>12.2?}"),
    );

    // The steady state: every AAE round after that, whether anything changed or
    // not. This is the floor on what a quiet cluster spends per round per peer,
    // and it is the number that has to stay flat as metadata grows.
    let rounds = 20;
    let idle = time_async(|| async {
        for _ in 0..rounds {
            follower
                .node
                .sync_with_peer(&publisher.node.node_id())
                .await
                .unwrap();
        }
    })
    .await
        / rounds;
    line("AAE sync, converged (steady)", &format!("{idle:>12.2?}"));

    // One file changes out of `files`. Structural sharing should make this cost
    // the changed path, not the tree — the property §5.2 is built on.
    std::fs::write(publisher.space.path().join("dir00/file000000"), b"changed").unwrap();
    let republish = time(|| {
        let (report, head) = publisher.node.scan_and_publish().unwrap();
        assert_eq!(report.hashed, 1);
        assert!(head.is_some());
    });
    line(
        "rescan + republish, 1 changed",
        &format!("{republish:>12.2?}"),
    );

    let incremental = time_async(|| async {
        let report = follower
            .node
            .sync_with_peer(&publisher.node.node_id())
            .await
            .unwrap();
        assert_eq!(report.tries_completed, 1, "{report:?}");
    })
    .await;
    line(
        "AAE sync, 1 entry changed",
        &format!("{incremental:>12.2?}"),
    );
    // §5.2 wants this proportional to the change rather than to the tree, in
    // work as well as bandwidth. The absolute number above is the one to watch:
    // raise `--files` and it should barely move, while the cold sync beside it
    // grows with the tree.
    println!(
        "  {:<34} {:>12} — cold over incremental, on {} files",
        "structural sharing factor",
        format!("{:.0}x", cold.as_secs_f64() / incremental.as_secs_f64()),
        files
    );
}

// ---- local lookups ---------------------------------------------------------

fn lookups(options: &Options, publisher: &Peer, follower: &Peer) {
    section("local lookup");
    let space = "media";
    let paths: Vec<String> = (0..LOOKUP_ITERATIONS)
        .map(|i| format!("dir{:02}/file{:06}", i % 100, i % options.files))
        .collect();

    // One path, one origin: the materialized view, which is what every read
    // surface hits first.
    let entry = time(|| {
        for path in &paths {
            let row = follower
                .node
                .store()
                .entry(publisher.node.origin(), space, path)
                .unwrap();
            assert!(row.is_some());
        }
    });
    per_op("entry by path (one origin)", entry, paths.len());

    // The §8 merge: every origin's claim about one path, resolved under a
    // policy. This is what `cat`, `get`, checkouts and S3 all go through.
    let resolve = time(|| {
        for path in &paths {
            follower
                .node
                .resolve(space, path, &VersionPolicy::Newest)
                .unwrap();
        }
    });
    per_op("resolve version (unified tree)", resolve, paths.len());

    // The authoritative structure rather than the derived view: a trie get
    // against the complete head, walking nodes from SQLite.
    let root = follower
        .node
        .store()
        .complete_head(publisher.node.origin())
        .unwrap()
        .unwrap()
        .root;
    let trie_paths: Vec<Vec<u8>> = paths
        .iter()
        .map(|p| synch_core::file_key(space, p).unwrap())
        .collect();
    let trie = time(|| {
        let t = synch_mpt::Trie::new(follower.node.store().as_ref());
        for key in &trie_paths {
            assert!(t.get(root, key).unwrap().is_some());
        }
    });
    per_op("trie get (authoritative)", trie, trie_paths.len());

    // A directory listing, which is a prefix range scan (§4.1).
    let dir_listings = 200;
    let listing = time(|| {
        for i in 0..dir_listings {
            let rows = follower
                .node
                .unified_listing(space, &format!("dir{:02}/", i % 100), None, None)
                .unwrap();
            assert!(!rows.is_empty());
        }
    });
    per_op("unified listing of a directory", listing, dir_listings);

    // The whole space, paginated the way the S3 gateway pages it.
    let page_size = 1000.min(options.files);
    let page = time(|| {
        let rows = follower
            .node
            .unified_listing(space, "", None, Some(page_size))
            .unwrap();
        assert_eq!(rows.len(), page_size);
    });
    line(
        &format!("unified listing, {page_size}-row page"),
        &format!("{page:>12.2?}"),
    );
}

// ---- content: ingest, verified read, verified transfer ---------------------

async fn content(options: &Options, publisher: &Peer, follower: &Peer) {
    section("content");
    let bytes = options.object_mib * 1024 * 1024;
    let mib = bytes as f64 / (1024.0 * 1024.0);
    let payload: Vec<u8> = (0..bytes).map(|i| (i * 37 + 11) as u8).collect();
    let path = publisher.space.path().join("big.bin");
    std::fs::write(&path, &payload).unwrap();

    // Hash and write into the CAS: streaming BLAKE3 plus the outboard.
    let ingest = time(|| {
        publisher.node.store().ingest_file(&path, now_ns()).unwrap();
    });
    throughput("ingest (hash + CAS write)", ingest, mib);

    let (root, size) = publisher.node.store().ingest_file(&path, now_ns()).unwrap();

    // A verified local read, chunked the way the control socket streams it —
    // every 16 KiB group checked against the root on the way out.
    let read = time(|| {
        let mut offset = 0u64;
        while offset < size {
            let take = (256 * 1024).min(size - offset);
            let got = publisher
                .node
                .store()
                .read_range(&root, offset, take)
                .unwrap();
            offset += got.len() as u64;
        }
    });
    throughput("verified local read", read, mib);

    // The whole point: a verified transfer between two nodes over loopback
    // QUIC, in windows, with every group verified against the root before it
    // is committed.
    publisher.node.scan_publish_push().await.unwrap();
    follower
        .node
        .sync_with_peer(&publisher.node.node_id())
        .await
        .unwrap();
    let transfer = time_async(|| async {
        let report = follower.node.fetch_all(&root, size).await.unwrap();
        assert!(report.complete, "{report:?}");
    })
    .await;
    throughput("verified transfer (loopback)", transfer, mib);
    assert_eq!(follower.node.store().read_all(&root).unwrap().len(), bytes);

    // A range read of an object held nowhere locally: the latency a media
    // player's seek pays (§14), rather than a bandwidth number.
    let seek_root = seed_second_object(publisher, follower, options).await;
    let seek = time_async(|| async {
        let got = follower
            .node
            .read_range(
                "media",
                "seek.bin",
                &VersionPolicy::Origin(publisher.node.origin().clone()),
                (options.object_mib as u64 / 2) * 1024 * 1024,
                Some(4096),
            )
            .await
            .unwrap();
        assert_eq!(got.len(), 4096);
    })
    .await;
    line("cold seek + 4 KiB range read", &format!("{seek:>12.2?}"));
    let held = follower.node.local_groups(&seek_root).unwrap();
    println!(
        "  {:<34} {:>12} — of {} groups, so a seek moves ~{} KiB",
        "groups pulled by that seek",
        held.count(),
        synch_core::group_count((options.object_mib * 1024 * 1024) as u64),
        held.count() * 16
    );
}

/// Publishes a second large object the follower has never seen, so the seek
/// benchmark starts from a genuinely cold cache.
async fn seed_second_object(publisher: &Peer, follower: &Peer, options: &Options) -> Hash {
    let payload: Vec<u8> = (0..options.object_mib * 1024 * 1024)
        .map(|i| (i * 11 + 7) as u8)
        .collect();
    std::fs::write(publisher.space.path().join("seek.bin"), &payload).unwrap();
    publisher.node.scan_publish_push().await.unwrap();
    follower
        .node
        .sync_with_peer(&publisher.node.node_id())
        .await
        .unwrap();
    follower
        .node
        .store()
        .entry(publisher.node.origin(), "media", "seek.bin")
        .unwrap()
        .unwrap()
        .content
        .unwrap()
}

// ---- harness ---------------------------------------------------------------

struct Options {
    files: usize,
    object_mib: usize,
}

impl Options {
    fn parse() -> Options {
        let args: Vec<String> = std::env::args().collect();
        let value = |flag: &str, default: usize| {
            args.iter()
                .position(|a| a == flag)
                .and_then(|i| args.get(i + 1))
                .and_then(|v| v.parse().ok())
                .unwrap_or(default)
        };
        Options {
            files: value("--files", DEFAULT_FILES).max(1),
            object_mib: value("--object-mib", DEFAULT_OBJECT_MIB).max(1),
        }
    }
}

struct Peer {
    _data: tempfile::TempDir,
    space: tempfile::TempDir,
    node: Node,
}

impl Peer {
    async fn spawn(name: &str) -> Peer {
        let data = tempfile::tempdir().unwrap();
        let space = tempfile::tempdir().unwrap();
        let origin = OriginId::named(name, "bench.example").unwrap();
        Node::init_named_by_zone(data.path(), origin).unwrap();
        let node = Node::open(NodeConfig::loopback(data.path())).await.unwrap();
        Peer {
            _data: data,
            space,
            node,
        }
    }
}

/// Static trust is unilateral, so each node is told about the other.
fn introduce(peers: &[&Peer]) {
    for a in peers {
        for b in peers {
            if a.node.origin() == b.node.origin() {
                continue;
            }
            a.node
                .store()
                .put_binding(&Binding {
                    origin: b.node.origin().clone(),
                    node_id: b.node.node_id(),
                    source: BindingSource::Static,
                    domain: None,
                    issuer: None,
                    spaces: Vec::new(),
                    note: None,
                    added_at: 0,
                    expires_at: None,
                })
                .unwrap();
            a.node.remember_peer(&b.node.net().direct_addr()).unwrap();
        }
    }
}

/// Writes `count` small files across 100 directories.
fn write_tree(root: &Path, count: usize) {
    for dir in 0..100 {
        std::fs::create_dir_all(root.join(format!("dir{dir:02}"))).unwrap();
    }
    for i in 0..count {
        std::fs::write(
            root.join(format!("dir{:02}/file{i:06}", i % 100)),
            format!("file {i} contents").as_bytes(),
        )
        .unwrap();
    }
}

fn entries(peer: &Peer, of: &Peer, space: &str) -> Vec<synch_store::EntryRow> {
    peer.node
        .store()
        .list_entries(Some(of.node.origin()), space, "", None, None)
        .unwrap()
}

fn time(f: impl FnOnce()) -> Duration {
    let start = Instant::now();
    f();
    start.elapsed()
}

async fn time_async<F, Fut>(f: F) -> Duration
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let start = Instant::now();
    f().await;
    start.elapsed()
}

fn section(name: &str) {
    println!("{name}");
}

fn line(label: &str, value: &str) {
    println!("  {label:<34} {value}");
}

fn rate(label: &str, elapsed: Duration, count: f64, unit: &str) {
    let per_sec = count / elapsed.as_secs_f64();
    println!(
        "  {:<34} {:>12.2?}  {:>10.0} {unit}/s",
        label, elapsed, per_sec
    );
}

fn per_op(label: &str, elapsed: Duration, count: usize) {
    let each = elapsed / count as u32;
    println!(
        "  {:<34} {:>12.2?}  {:>10.0} ops/s",
        label,
        each,
        1.0 / each.as_secs_f64()
    );
}

fn throughput(label: &str, elapsed: Duration, mib: f64) {
    println!(
        "  {:<34} {:>12.2?}  {:>10.0} MiB/s",
        label,
        elapsed,
        mib / elapsed.as_secs_f64()
    );
}
