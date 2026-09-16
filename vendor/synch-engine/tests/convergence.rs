//! Randomized convergence over the §5.2 head-reconciliation rule, built
//! directly on [`Syncer`]/[`Store`] so hundreds of cases run per test: many
//! randomized deliveries of one head set to N nodes, each in its own order,
//! then gossip until it settles (DESIGN §11's `mptsync` minus the live
//! transport, partitions and interleaved publishes, which are outstanding).
//!
//! What it pins is the join: `(seq, root)` under lexicographic order is a
//! join-semilattice, so the head a node settles on is the maximum of what it
//! has seen, independent of arrival order — an acceptance rule that looks at
//! anything else leaves two honest peers on different heads forever. Two
//! harnesses: `heads_converge_…` gossips `head_floor` with fabricated roots;
//! `the_push_pull_decision_…` runs the shape `sync_with` actually runs (the
//! complete slot only) over real tries, so "converged" means servable.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use iroh_base::SecretKey;
use synch_core::{Hash, OriginId, SignedHead};
use synch_engine::reconcile::{Syncer, MAX_RETAINED_FORKS};
use synch_store::Store;

mod common;
use common::binding;

/// xorshift64*, so a case is reproducible from its seed.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below(i + 1);
            items.swap(i, j);
        }
    }
}

fn origins(names: &[&str]) -> Vec<OriginId> {
    names
        .iter()
        .map(|name| OriginId::named(name, "x.example").expect("an origin"))
        .collect()
}

/// One node: a store with the signing key bound, and a syncer over it.
struct Node {
    store: Arc<Store>,
    syncer: Syncer,
    /// Every head this node was handed directly, for the evidence invariant.
    delivered: Vec<SignedHead>,
    _dir: tempfile::TempDir,
}

fn node(key: &SecretKey, origins: &[OriginId]) -> Node {
    let dir = tempfile::tempdir().expect("a temp dir");
    let store = Arc::new(Store::open(dir.path()).expect("a store"));
    for origin in origins {
        store
            .put_binding(&binding(origin, &key.public()))
            .expect("the binding");
    }
    Node {
        syncer: Syncer::new(store.clone()),
        store,
        delivered: Vec::new(),
        _dir: dir,
    }
}

/// The greatest `(seq, root)` in a set — what every node must settle on.
fn maximum(heads: &[SignedHead]) -> (u64, Hash) {
    heads
        .iter()
        .map(|h| (h.seq, h.root))
        .max_by_key(|(seq, root)| (*seq, root.0))
        .expect("a non-empty head set")
}

/// Random rounds, then an all-pairs sweep, so what must reach everybody
/// provably does.
fn gossip<F: FnMut(usize, usize)>(rng: &mut Rng, count: usize, mut exchange: F) {
    for _ in 0..count {
        for i in 0..count {
            let mut j = rng.below(count);
            if j == i {
                j = (j + 1) % count;
            }
            exchange(i, j);
        }
    }
    for i in 0..count {
        for j in 0..count {
            if i != j {
                exchange(i, j);
            }
        }
    }
}

/// Builds the head set for one case: a few origins, a few seqs each, forks of
/// random width, and one storm well past [`MAX_RETAINED_FORKS`].
fn head_set(rng: &mut Rng, key: &SecretKey, origins: &[OriginId], seed: u64) -> Vec<SignedHead> {
    let mut heads = Vec::new();
    let mint = |origin: &OriginId, seq: u64, fork: usize| {
        let label = format!("{origin}/{seq}/{fork}/{seed}");
        SignedHead::sign(key, origin.clone(), seq, Hash::new(label.as_bytes()), 0)
    };
    for origin in origins {
        for seq in 1..=(2 + rng.below(4) as u64) {
            for fork in 0..1 + rng.below(3) {
                heads.push(mint(origin, seq, fork));
            }
        }
        // A storm at one seq: more roots than may be retained, so the eviction
        // rule runs and the acceptance rule must not.
        let storm = 1 + rng.below(3) as u64;
        for fork in 0..MAX_RETAINED_FORKS + 4 {
            heads.push(mint(origin, storm, 100 + fork));
        }
    }
    heads
}

/// One randomized case: deliver, gossip, and check what everyone settled on.
fn one_case(seed: u64) {
    let mut rng = Rng::new(seed);
    let key = SecretKey::generate();
    let origins = origins(&["nas", "laptop"]);
    let heads = head_set(&mut rng, &key, &origins, seed);
    let by_key: HashMap<(u64, [u8; 32]), SignedHead> = heads
        .iter()
        .map(|h| ((h.seq, h.root.0), h.clone()))
        .collect();

    let count = 3 + rng.below(3);
    let mut nodes: Vec<Node> = (0..count).map(|_| node(&key, &origins)).collect();

    // Message loss without a transport: every head reaches at least one node; each sees a random part.
    let mut plan: Vec<Vec<SignedHead>> = vec![Vec::new(); count];
    for head in &heads {
        plan[rng.below(count)].push(head.clone());
        for node in plan.iter_mut() {
            if rng.below(3) == 0 {
                node.push(head.clone());
            }
        }
    }
    for (node, mut for_node) in nodes.iter_mut().zip(plan) {
        rng.shuffle(&mut for_node);
        for head in for_node {
            node.syncer
                .offer_head(&head, 0)
                .expect("an offer never errors on a well-formed head");
            node.delivered.push(head);
        }
    }

    // Gossip: offer what each node holds, then the sweep that proves delivery.
    let exchange = |from: usize, to: usize| {
        for origin in &origins {
            let Some(floor) = nodes[from].store.head_floor(origin).expect("a floor") else {
                continue;
            };
            let head = by_key
                .get(&(floor.0, floor.1 .0))
                .expect("a held head is one of the set")
                .clone();
            nodes[to].syncer.offer_head(&head, 0).expect("an offer");
        }
    };
    gossip(&mut rng, count, exchange);

    for origin in &origins {
        let mine: Vec<SignedHead> = heads
            .iter()
            .filter(|h| &h.origin == origin)
            .cloned()
            .collect();
        let expected = maximum(&mine);
        for (i, node) in nodes.iter().enumerate() {
            assert_eq!(
                node.store.head_floor(origin).expect("a floor"),
                Some(expected),
                "seed {seed}: node {i} settled elsewhere for {origin}"
            );
        }
    }

    // Evidence survives the eviction that bounds it: a fork delivered to a node is still reportable (§4.4).
    for node in &nodes {
        let mut seen: HashMap<(String, u64), HashSet<[u8; 32]>> = HashMap::new();
        for head in &node.delivered {
            seen.entry((head.origin.canonical(), head.seq))
                .or_default()
                .insert(head.root.0);
        }
        let reported: HashSet<(String, u64)> = node
            .store
            .equivocations()
            .expect("the equivocations")
            .into_iter()
            .map(|e| (e.origin.canonical(), e.seq))
            .collect();
        for (at, roots) in seen {
            if roots.len() > 1 {
                assert!(
                    reported.contains(&at),
                    "seed {seed}: a fork at {at:?} was delivered and is not reported"
                );
            }
        }
    }
}

/// Every node settles on the same head, from every arrival order — the
/// property the whole of §5.2 rests on.
#[test]
fn heads_converge_whatever_order_they_arrive_in() {
    for seed in 0..10 {
        one_case(seed);
    }
}

/// The same property, gossiped the way the *protocol* gossips: `heads_for`
/// serves the complete slot only, `sync_with` pushes off `complete_head`, and
/// every root is a real trie, so "converged" means servable.
#[test]
fn the_push_pull_decision_converges_over_real_slots() {
    for seed in 0..8 {
        wire_case(seed);
    }
}

/// One randomized case over the real push/pull decision.
fn wire_case(seed: u64) {
    use synch_core::{file_key, FileEntry, HeadSummary};
    use synch_mpt::Trie;

    let mut rng = Rng::new(seed);
    let key = SecretKey::generate();
    let origins = origins(&["nas", "laptop", "vps"]);

    let count = 3 + rng.below(3);
    let nodes: Vec<Node> = (0..count).map(|_| node(&key, &origins)).collect();

    // A published head is a head whose trie exists, built in the publisher's
    // store and nowhere else — peers must pull the nodes to serve it on.
    let entry = |n: u64| {
        postcard::to_stdvec(&FileEntry::file(n, 0, Hash::new(&n.to_le_bytes()), 1)).expect("encode")
    };
    let mut published: Vec<SignedHead> = Vec::new();
    for (i, origin) in origins.iter().enumerate() {
        // One publisher per origin, a couple of successive heads each, so the
        // pull side chooses the greater and the push side stops offering the lesser.
        let at = i % count;
        let mut root = Hash::EMPTY;
        for seq in 1..=(1 + rng.below(3) as u64) {
            let key_bytes = file_key("media", &format!("{origin}-{seq}.bin")).expect("a key");
            let value = entry(seq);
            root = Trie::new(nodes[at].store.as_ref())
                .insert(root, &key_bytes, &value)
                .expect("the insert");
            let head = SignedHead::sign(&key, origin.clone(), seq, root, 0);
            nodes[at]
                .store
                .put_head(synch_store::Slot::Complete, &head, 0, 0)
                .expect("the slot");
            nodes[at]
                .store
                .transaction(|txn| txn.materialize_diff(origin, Hash::EMPTY, root))
                .expect("the views");
            published.push(head);
        }
    }

    // One exchange, in the shape `sync_with` runs it: both sides advertise
    // `local_summaries`, each decides what to push and want, and the wanted
    // origins are answered out of `heads_for` — the complete slot only.
    let exchange = |from: usize, to: usize| {
        let ours = nodes[from].syncer.local_summaries().expect("summaries");
        let theirs = nodes[to].syncer.local_summaries().expect("summaries");
        let best = |set: &[HeadSummary], origin: &OriginId| {
            set.iter()
                .filter(|s| &s.origin == origin)
                .map(|s| s.order_key())
                .max()
        };

        // Push: every complete head that beats what the peer advertised.
        for stored in nodes[from]
            .store
            .all_heads(synch_store::Slot::Complete)
            .expect("the slots")
        {
            let head = stored.head;
            if best(&theirs, &head.origin).is_none_or(|peer| (head.seq, head.root.0) > peer) {
                // Applying it needs the trie: content-addressed nodes, from any peer (§5.2).
                copy_trie(&nodes[from], &nodes[to], head.root);
                let _ = nodes[to].syncer.offer_head(&head, 0);
            }
        }
        // Pull: every origin the peer is ahead on, answered from its complete slot alone.
        let want: Vec<OriginId> = theirs
            .iter()
            .filter(|s| best(&ours, &s.origin).is_none_or(|mine| s.order_key() > mine))
            .map(|s| s.origin.clone())
            .collect();
        for head in nodes[to].syncer.heads_for(&want).expect("heads_for") {
            copy_trie(&nodes[to], &nodes[from], head.root);
            let _ = nodes[from].syncer.offer_head(&head, 0);
        }
    };

    gossip(&mut rng, count, exchange);

    for origin in &origins {
        let expected = published
            .iter()
            .filter(|p| &p.origin == origin)
            .map(|p| (p.seq, p.root))
            .max_by_key(|(seq, root)| (*seq, root.0))
            .expect("every origin published");
        for (i, holder) in nodes.iter().enumerate() {
            let complete = holder
                .store
                .complete_head(origin)
                .expect("the slot")
                .unwrap_or_else(|| panic!("seed {seed}: node {i} holds no head for {origin}"));
            assert_eq!(
                (complete.seq, complete.root),
                expected,
                "seed {seed}: node {i} settled elsewhere for {origin}"
            );
            // Converged means servable, not merely known: the trie is here.
            assert!(
                synch_mpt::Trie::new(holder.store.as_ref())
                    .is_complete(complete.root)
                    .expect("completeness"),
                "seed {seed}: node {i} holds a head for {origin} it cannot serve"
            );
            // The derived view agrees with the head — what the unified tree,
            // checkouts and the gateway read (one key per head, so seq is leaf count).
            assert_eq!(
                holder
                    .store
                    .list_entries(Some(origin), "media", "", None, None)
                    .expect("the entries")
                    .len(),
                complete.seq as usize,
                "seed {seed}: node {i}'s entries do not match the head it holds"
            );
        }
    }
}

/// Copies every trie node and value under `root` between stores — `GetNodes`/
/// `GetValues` without a transport; the walk itself is covered over real
/// endpoints in `two_nodes.rs`.
fn copy_trie(from: &Node, to: &Node, root: Hash) {
    use synch_mpt::NodeStore;
    let reachable = synch_mpt::Trie::new(from.store.as_ref())
        .reachable(root)
        .expect("the reachable set");
    for hash in reachable.nodes {
        if let Some(bytes) = NodeStore::get_node(from.store.as_ref(), &hash).expect("a node") {
            NodeStore::put_node(to.store.as_ref(), &hash, &bytes).expect("the put");
        }
    }
    for hash in reachable.values {
        if let Some(bytes) = NodeStore::get_value(from.store.as_ref(), &hash).expect("a value") {
            NodeStore::put_value(to.store.as_ref(), &hash, &bytes).expect("the put");
        }
    }
}
