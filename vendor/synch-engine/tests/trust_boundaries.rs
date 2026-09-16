//! Containment at the trust boundaries of mptsync (§5.5, §12), over real
//! endpoints. Each test states a property the design promises.

use synch_core::{delegation_key, file_key, now_ns, Delegation, Hash, NodeId, SignedHead};
use synch_engine::{reconcile::HeadOutcome, FetchOutcome, Syncer};
use synch_mpt::{NodeStore, Scope, Trie, TrieNode};
use synch_store::Slot;

mod common;
use common::wire::{connect, trust as trust_static, WireNode};

fn delegation(subject: &NodeId, spaces: &[&str]) -> (Vec<u8>, Vec<u8>) {
    let record = Delegation {
        v: synch_core::RECORD_VERSION,
        spaces: spaces.iter().map(|s| s.to_string()).collect(),
        not_after: now_ns() + 86_400_000_000_000,
        note: Some("test delegate".into()),
    };
    (
        delegation_key(subject),
        postcard::to_stdvec(&record).unwrap(),
    )
}

/// Every node of `root`'s trie by position.
fn walk_all(store: &synch_store::Store, root: Hash) -> Vec<(Vec<u8>, Hash)> {
    let empty = synch_mpt::MemStore::new();
    let mut walk = synch_mpt::MissingWalk::new(root);
    let mut all = Vec::new();
    loop {
        let batch = walk.next_batch(&Trie::new(&empty), 512).unwrap();
        if batch.is_empty() {
            break;
        }
        for (path, hash) in &batch.nodes {
            all.push((path.clone(), *hash));
            let bytes = store.get_node(hash).unwrap().unwrap();
            empty.put_node(hash, &bytes).unwrap();
        }
        for (_, hash) in &batch.values {
            let bytes = store.get_value(hash).unwrap().unwrap();
            empty.put_value(hash, &bytes).unwrap();
        }
        walk.resume();
    }
    all
}

/// A delegate can graft a withheld subtree of the issuer's trie into its own
/// trie at an in-scope position. It knows the subtree's hash (the hash sits
/// in the branch node the signed root recomputes through) and need not hold a
/// single node of it: the full member fetching the delegate's head already
/// holds those nodes from the issuer's trie, so the walk finds them present,
/// promotes the head, and from then on serves the withheld subtree to *every*
/// delegate under the grafting origin's in-scope positions.
///
/// Closed by provenance: for a confined origin's root, a node is present only
/// if it was served as that origin's (`NodeStore::owns_node`), which bottoms
/// out in the origin itself — and the grafter never held the subtree. The
/// member's fetch asks the grafter for it, is told `missing`, and abandons the
/// head; the responder serves nothing under a confined root it does not own
/// for that origin. `Provenance.lean` is the model of both halves.
#[tokio::test]
async fn a_delegate_cannot_launder_a_withheld_subtree_through_its_own_trie() {
    let issuer = WireNode::spawn(Some("nas")).await;
    let grafter = WireNode::spawn(None).await;
    let reader = WireNode::spawn(None).await;
    trust_static(&grafter.store, &issuer.origin, &issuer.key());
    trust_static(&reader.store, &issuer.origin, &issuer.key());

    // Two withheld files, so the withheld subtree contains a Branch.
    issuer.publish(
        1,
        &[
            ("photos", "a.jpg", b"granted"),
            ("finance", "q3.pdf", b"withheld q3"),
            ("finance", "q4.pdf", b"withheld q4"),
        ],
        &[
            delegation(&grafter.key(), &["photos"]),
            delegation(&reader.key(), &["photos"]),
        ],
    );
    let issuer_root = issuer.root();
    let scope = Scope::of(&synch_core::scope_prefixes(&["photos".to_string()]));

    // The withheld Branch under `f:finance`, and where it sits: the grafter
    // learns the hash honestly from the spine, never its contents.
    let finance = synch_mpt::Nibbles::from_bytes(b"f:finance")
        .as_slice()
        .to_vec();
    let (withheld_path, withheld_branch) = walk_all(&issuer.store, issuer_root)
        .into_iter()
        .find(|(path, hash)| {
            !scope.admits_path(path)
                && path.starts_with(&finance)
                && matches!(
                    TrieNode::decode(&issuer.store.get_node(hash).unwrap().unwrap()).unwrap(),
                    TrieNode::Branch { .. }
                )
        })
        .expect("the withheld subtree holds a branch; test is vacuous");
    assert!(
        grafter.store.get_node(&withheld_branch).unwrap().is_none(),
        "the grafter must not hold the subtree it grafts"
    );

    // The graft: one hand-built extension at the root, placing the withheld
    // branch at an in-scope position of the same nibble parity as the one it
    // came from, so every key under it stays byte-aligned.
    let mut graft_at = synch_mpt::Nibbles::from_bytes(b"f:photos/q")
        .as_slice()
        .to_vec();
    if graft_at.len() % 2 != withheld_path.len() % 2 {
        graft_at.push(0x3);
    }
    let ext = TrieNode::Ext {
        prefix: synch_mpt::Nibbles::from_nibbles(&graft_at),
        child: withheld_branch,
    };
    let encoded = ext.encode();
    let graft_root = TrieNode::hash_of_encoded(&encoded).unwrap();
    grafter.store.put_node(&graft_root, &encoded).unwrap();
    let grafted = SignedHead::sign(
        &grafter.secret,
        grafter.origin.clone(),
        1,
        graft_root,
        now_ns(),
    );
    grafter
        .store
        .put_head(Slot::Complete, &grafted, now_ns(), now_ns())
        .unwrap();

    // The issuer takes the head (it verifies and the key is delegated), then
    // fetches the trie from the grafter exactly as a sync round would.
    let syncer = Syncer::new(issuer.store.clone());
    assert_eq!(
        syncer.offer_head(&grafted, now_ns()).unwrap(),
        HeadOutcome::Pending
    );
    let client = connect(&issuer, &grafter).await;
    let fetched = syncer.fetch_pending(&client, &grafter.origin).await;

    // The property: a full member must not vouch for a delegated origin's
    // trie built out of nodes that origin was never served — and no other
    // delegate may read the withheld subtree through it, by either route.
    // Every check runs before any assertion so one failure reports the
    // whole shape of the leak.
    let mut violations = Vec::new();
    let promoted = issuer
        .store
        .complete_head(&grafter.origin)
        .unwrap()
        .is_some();
    if promoted || matches!(fetched, Ok(FetchOutcome::Completed)) {
        violations.push(format!(
            "the issuer promoted a delegate's head whose trie is the issuer's own withheld \
             subtree (fetch outcome {fetched:?})"
        ));
    }

    let syncer = Syncer::new(reader.store.clone());
    let client = connect(&reader, &issuer).await;
    // Two exchanges: the first materializes the issuer's `d:` records, which
    // is what makes the grafter's head bound on the reader for the second.
    for _ in 0..2 {
        syncer.sync_with(&client).await.unwrap();
    }
    let withheld_entry = Trie::new(issuer.store.as_ref())
        .get(issuer_root, &file_key("finance", "q3.pdf").unwrap())
        .unwrap()
        .unwrap();
    let laundered = Trie::new(reader.store.as_ref())
        .get(graft_root, &file_key("photos", "q3.pdf").unwrap())
        .ok()
        .flatten();
    if laundered.as_deref() == Some(withheld_entry.as_slice()) {
        violations.push(
            "the other delegate read the withheld finance record through the grafter's trie \
             by ordinary scoped sync"
                .to_string(),
        );
    }
    let direct = client
        .get_nodes(graft_root, &[(graft_at.clone(), withheld_branch)])
        .await;
    if direct
        .map(|answer| !answer.nodes.is_empty())
        .unwrap_or(false)
    {
        violations.push(
            "GetNodes served the withheld branch at a position in a delegate-authored root"
                .to_string(),
        );
    }
    assert!(violations.is_empty(), "{}", violations.join("\n"));
}

/// A node that hashes correctly but breaks a structural invariant is the
/// *origin's* fault (§12) and is contained to that origin: the exchange goes
/// on, the origin is reported as left behind, and the head does not keep the
/// pending slot. Reported as a hash mismatch — a peer fault — it aborted the
/// whole exchange, left the head pending, and repeated on every exchange with
/// every peer that served it.
#[tokio::test]
async fn a_non_canonical_node_fails_its_origin_and_not_the_exchange() {
    let a = WireNode::spawn(Some("a")).await;
    let b = WireNode::spawn(Some("b")).await;
    common::wire::trust_all(&[&a, &b]);

    // `a` publishes a root that hashes fine and decodes fine but is
    // non-canonical: a branch with a single occupant.
    let (value, _) = synch_mpt::ValueRef::for_value(b"x");
    let leaf = TrieNode::Leaf {
        key_rest: synch_mpt::Nibbles::from_nibbles(&[1, 2, 3]),
        value,
    };
    let leaf_hash = leaf.hash();
    let mut children = [None; 16];
    children[6] = Some(leaf_hash);
    let branch = TrieNode::Branch {
        children,
        value: None,
    };
    let encoded = branch.encode();
    let root = branch.hash();
    assert!(TrieNode::hash_of_encoded(&encoded).is_err());
    a.store.put_node(&leaf_hash, &leaf.encode()).unwrap();
    a.store.put_node(&root, &encoded).unwrap();
    a.store.note_complete(&root).unwrap();
    let head = SignedHead::sign(&a.secret, a.origin.clone(), 1, root, now_ns());
    a.store
        .put_head(Slot::Complete, &head, now_ns(), now_ns())
        .unwrap();

    let syncer = Syncer::new(b.store.clone());
    let client = connect(&b, &a).await;
    let report = syncer.sync_with(&client).await;
    assert!(
        report.is_ok(),
        "one origin's malformed node aborted the whole exchange: {report:?}"
    );
    assert_eq!(
        report.unwrap().heads_failed,
        1,
        "the origin must be reported as left behind"
    );
    assert_eq!(
        b.store.pending_head(&a.origin).unwrap(),
        None,
        "the unpromotable head must not keep the pending slot"
    );
}
