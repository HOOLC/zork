//! Delegated space-restricted trust, end to end over two real endpoints
//! (§3.5, §5.5). The properties worth a network for: a delegate is admitted by
//! replicated state and nothing else, sees only its spaces, cannot reach
//! content it was not delegated, and cannot publish outside its list.

use synch_core::{
    delegation_key, file_key, now_ns, ChunkRanges, Delegation, FileEntry, Hash, NodeId, SignedHead,
};
use synch_engine::Syncer;
use synch_mpt::{Scope, Trie};
use synch_store::{Slot, StoreError};

mod common;
use common::wire::{connect, connect_blob, trust as trust_static, WireNode};

/// The `d:` record an issuer publishes to delegate `subject` (§3.5).
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

/// One metadata exchange with `peer`, pulling everything this node may adopt.
async fn exchange(a: &WireNode, b: &WireNode) {
    let client = connect(a, b).await;
    Syncer::new(a.store.clone())
        .sync_with(&client)
        .await
        .unwrap();
}

/// Every node of `root`'s trie by position, walked as a full member would —
/// the ground truth a scoped delegate's view is measured against.
fn walk_all(
    store: &dyn synch_mpt::NodeStore<Error = StoreError>,
    root: Hash,
) -> Vec<(Vec<u8>, Hash)> {
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
            let bytes = synch_mpt::NodeStore::get_node(store, hash)
                .unwrap()
                .unwrap();
            synch_mpt::NodeStore::put_node(&empty, hash, &bytes).unwrap();
        }
        // Out-of-line values too: a node whose values have not arrived is
        // deferred again, so the walk would never terminate.
        for (_, hash) in &batch.values {
            let bytes = synch_mpt::NodeStore::get_value(store, hash)
                .unwrap()
                .unwrap();
            synch_mpt::NodeStore::put_value(&empty, hash, &bytes).unwrap();
        }
        walk.resume();
    }
    all
}

/// A delegate is admitted by replicated state, and sees exactly its spaces —
/// nothing is handed to it or presented by it; the record reaches the member,
/// the member admits the key, and what it reads is the projection of the
/// issuer's trie its grant covers, and of the rest not even a filename.
#[tokio::test]
async fn a_delegate_is_admitted_by_replicated_state_and_sees_only_its_spaces() {
    let issuer = WireNode::spawn(Some("nas")).await;
    let delegate = WireNode::spawn(None).await;

    // The delegate trusts the cluster by the ordinary route; the cluster comes
    // to trust it through the record below (trust is unilateral).
    trust_static(&delegate.store, &issuer.origin, &issuer.key());

    // Before the record exists the delegate's key is unknown and refused.
    assert!(!issuer
        .store
        .is_trusted_key(&delegate.key(), now_ns())
        .unwrap());

    issuer.publish(
        1,
        &[
            ("photos", "a.jpg", b"the granted bytes"),
            ("finance", "q3.pdf", b"the withheld bytes"),
        ],
        &[delegation(&delegate.key(), &["photos"])],
    );

    // Materializing the issuer's own head admitted the delegate, scoped as the record named.
    assert!(issuer
        .store
        .is_trusted_key(&delegate.key(), now_ns())
        .unwrap());
    assert_eq!(
        issuer
            .store
            .publish_scope_of_key(&delegate.key(), now_ns())
            .unwrap(),
        synch_store::PublishScope::Confined(vec!["photos".to_string()])
    );

    // The delegate syncs: learns its scope, walks under it, and promotes.
    exchange(&delegate, &issuer).await;
    assert_eq!(
        delegate.store.local_scope().unwrap(),
        Some(vec!["photos".to_string()]),
        "the delegate did not learn its scope from the exchange"
    );

    let head = delegate
        .store
        .complete_head(&issuer.origin)
        .unwrap()
        .expect("the delegate promoted the issuer's head");
    assert_eq!(head.root, issuer.root(), "same signed root, partial trie");

    // The granted space is fully readable, verified against that same root.
    let trie = Trie::new(delegate.store.as_ref());
    assert!(trie
        .get(head.root, &file_key("photos", "a.jpg").unwrap())
        .unwrap()
        .is_some());

    // And of the undelegated space: nothing — the read fails for want of the
    // subtree, not by returning an absence.
    assert!(trie
        .get(head.root, &file_key("finance", "q3.pdf").unwrap())
        .is_err());
    // The scope check agrees with what actually landed.
    let scope = Scope::of(&synch_core::scope_prefixes(&["photos".to_string()]));
    assert!(trie.is_complete_scoped(head.root, &scope).unwrap());
    assert!(!trie.is_complete(head.root).unwrap());

    // Content outside the spaces is refused even holding the root: `GetSlice`
    // is keyed by object root and carries no space, so entitlement is looked
    // up rather than read off the request (§6.4).
    let blob = connect_blob(&delegate, &issuer).await;
    let slice = blob
        .get_slice(Hash::new(b"the granted bytes"), &ChunkRanges::single(0, 1))
        .await
        .unwrap();
    assert!(
        !slice.encoded.is_empty(),
        "the granted object did not serve"
    );
    let refused = blob
        .get_slice(Hash::new(b"the withheld bytes"), &ChunkRanges::single(0, 1))
        .await;
    assert!(
        refused.is_err(),
        "a delegate was served content outside its spaces"
    );
}

/// A delegated origin publishing outside its spaces has its head refused whole (§3.5).
#[tokio::test]
async fn a_delegate_publishing_outside_its_spaces_is_refused() {
    let issuer = WireNode::spawn(Some("nas")).await;
    let delegate = WireNode::spawn(None).await;
    trust_static(&delegate.store, &issuer.origin, &issuer.key());

    // The delegation is the only thing admitting the delegate — static trust would make it rooted.
    issuer.publish(1, &[], &[delegation(&delegate.key(), &["photos"])]);
    assert_eq!(
        issuer
            .store
            .publish_scope(&delegate.origin, now_ns())
            .unwrap(),
        synch_store::PublishScope::Confined(vec!["photos".to_string()])
    );

    // In scope: the issuer accepts it.
    delegate.publish(1, &[("photos", "mine.jpg", b"in scope")], &[]);
    let syncer = Syncer::new(issuer.store.clone());
    let client = connect(&issuer, &delegate).await;
    syncer.sync_with(&client).await.unwrap();
    assert_eq!(
        issuer
            .store
            .complete_head(&delegate.origin)
            .unwrap()
            .map(|h| h.seq),
        Some(1)
    );

    // Out of scope: the head is refused whole, not materialized in part, so
    // the delegate stalls at the head that was legitimate.
    let legitimate = issuer
        .store
        .complete_head(&delegate.origin)
        .unwrap()
        .unwrap();
    delegate.publish(2, &[("finance", "sneaky.pdf", b"out of scope")], &[]);
    let report = syncer.sync_with(&client).await.unwrap();
    assert_eq!(
        issuer
            .store
            .complete_head(&delegate.origin)
            .unwrap()
            .map(|h| h.seq),
        Some(1),
        "a delegate published outside its spaces and the head was promoted"
    );
    // And refused means retired, not parked: its trie is wholly here, so
    // nothing else would ever clear it, and left in the pending slot it would
    // hold `head_floor` above every head this node can actually serve (§5.2).
    assert_eq!(
        issuer.store.pending_head(&delegate.origin).unwrap(),
        None,
        "a refused head kept the pending slot"
    );
    assert_eq!(
        issuer.store.head_floor(&delegate.origin).unwrap(),
        Some((legitimate.seq, legitimate.root)),
        "the floor is back at the head this node serves"
    );
    assert_eq!(
        report.heads_failed, 1,
        "the origin is reported as left behind"
    );
}

/// A scoped peer cannot reach a redacted node by claiming a position for it —
/// the sharpest form of the question. A delegate necessarily *holds* the hash
/// of every subtree withheld from it (the hash sits inside the branch the
/// signed root recomputes from), so the only barrier is position. This asks
/// for a withheld hash under an in-scope position that resolves to nothing,
/// and against a root of the caller's own choosing: the two shapes that get
/// past a naive position check.
#[tokio::test]
async fn a_withheld_node_cannot_be_reached_by_claiming_a_position_for_it() {
    let issuer = WireNode::spawn(Some("nas")).await;
    let delegate = WireNode::spawn(None).await;
    trust_static(&delegate.store, &issuer.origin, &issuer.key());

    issuer.publish(
        1,
        &[
            ("photos", "a.jpg", b"granted"),
            ("finance", "q3.pdf", b"withheld"),
            ("finance", "q4.pdf", b"withheld too"),
        ],
        &[delegation(&delegate.key(), &["photos"])],
    );
    let root = issuer.root();

    let scope = Scope::of(&synch_core::scope_prefixes(&["photos".to_string()]));
    let withheld: Vec<Hash> = walk_all(issuer.store.as_ref(), root)
        .into_iter()
        .filter(|(path, _)| !scope.admits_path(path))
        .map(|(_, hash)| hash)
        .collect();
    assert!(
        !withheld.is_empty(),
        "nothing was withheld; test is vacuous"
    );

    // An in-scope position that resolves to nothing.
    let bogus = synch_mpt::Nibbles::from_bytes(b"f:photos/\xde\xad\xbe\xef")
        .as_slice()
        .to_vec();
    assert!(
        scope.admits_path(&bogus),
        "the position must pass the scope test"
    );

    let client = connect(&delegate, &issuer).await;
    for hash in &withheld {
        let answer = client
            .get_nodes(root, &[(bogus.clone(), *hash)])
            .await
            .expect("the request itself is well formed");
        assert!(
            answer.nodes.is_empty(),
            "a withheld node was served for a position that does not hold it"
        );
        assert_eq!(answer.missing, vec![*hash]);

        // And the same hash against a root of the caller's choosing: the empty
        // path resolves to whatever root it was handed, so a root held for no head is refused outright.
        let refused = client.get_nodes(*hash, &[(Vec::new(), *hash)]).await;
        assert!(
            refused.is_err(),
            "a fabricated root let a withheld node be named"
        );
    }
}

/// A delegate cannot authorize anything with its own trie: its root lands in
/// `head_history` as soon as signature and binding verify, so given one of its
/// own roots it could name any hash it has heard of under an in-scope position
/// and be handed the node back — and by publishing an in-scope entry naming a
/// withheld object it could read that object's row back as its own title
/// (`GetSlice` carries no space; entitlement is looked up by which spaces name
/// the object). Both must refuse.
#[tokio::test]
async fn a_delegate_cannot_authorize_with_its_own_root_or_name() {
    let issuer = WireNode::spawn(Some("nas")).await;
    let delegate = WireNode::spawn(None).await;
    // Only the delegate trusts the issuer statically; the issuer learns the
    // delegate from its own published `d:` record (static trust would make it rooted).
    trust_static(&delegate.store, &issuer.origin, &issuer.key());

    let withheld = b"the withheld bytes".to_vec();
    issuer.publish(
        1,
        &[
            ("photos", "a.jpg", b"granted"),
            ("finance", "q3.pdf", withheld.as_slice()),
        ],
        &[delegation(&delegate.key(), &["photos"])],
    );

    // A node of the issuer's trie the delegate is not entitled to — a hash it
    // knows honestly, from a branch the signed root recomputes through.
    let scope = Scope::of(&synch_core::scope_prefixes(&["photos".to_string()]));
    let withheld_node = walk_all(issuer.store.as_ref(), issuer.root())
        .into_iter()
        .find(|(path, _)| !scope.admits_path(path))
        .map(|(_, hash)| hash)
        .expect("the issuer withholds something; test is vacuous");

    // The delegate signs a root of its own — an entirely in-scope key, so the
    // publish itself is legitimate — and the issuer records it. Asked against
    // that root, an in-scope position must not authorize anything.
    let mine = delegate.publish(1, &[("photos", "mine.jpg", b"my own bytes")], &[]);
    issuer
        .store
        .put_head(Slot::Complete, &mine, now_ns(), now_ns())
        .unwrap();
    assert!(
        issuer.store.is_head_root(&mine.root, &[]).unwrap(),
        "the issuer really did record the delegate's root"
    );
    let client = connect(&delegate, &issuer).await;
    let refused = client
        .get_nodes(
            mine.root,
            &[(
                synch_mpt::Nibbles::from_bytes(b"f:photos/")
                    .as_slice()
                    .to_vec(),
                withheld_node,
            )],
        )
        .await;
    assert!(
        refused.is_err(),
        "a delegate's own root authorized a position in someone else's trie"
    );

    // Nor does an in-scope entry naming a withheld object grant it: the issuer
    // holds the row (a delegate published it), and `GetSlice` must still refuse.
    let withheld_root = Hash::new(&withheld);
    issuer
        .store
        .put_entry(
            &delegate.origin,
            "photos",
            "decoy.bin",
            &FileEntry::file(withheld.len() as u64, 0, withheld_root, 1),
        )
        .unwrap();
    let blob = connect_blob(&delegate, &issuer).await;
    let refused = blob
        .get_slice(withheld_root, &ChunkRanges::single(0, 1))
        .await;
    assert!(
        refused.is_err(),
        "a delegate's own entry granted it content outside its spaces"
    );
    // And the issuer's own grant still works: the refusal is about who
    // published the row, not about the lookup being broken.
    let granted = blob
        .get_slice(Hash::new(b"granted"), &ChunkRanges::single(0, 1))
        .await
        .unwrap();
    assert!(
        !granted.encoded.is_empty(),
        "the granted object stopped serving"
    );
}

/// A delegate is never shown a node whose key material runs out of its scope:
/// the trie compresses, so a node at a position the spine legitimately admits
/// can still spell an undelegated space's name in its extension prefix, or
/// complete a whole out-of-scope key in its leaf. Both are refused as a
/// boundary — not an absence, or the walk would retry until its head was
/// abandoned.
#[tokio::test]
async fn a_compressed_node_spanning_out_of_scope_is_a_boundary_not_an_absence() {
    let issuer = WireNode::spawn(Some("nas")).await;
    let delegate = WireNode::spawn(None).await;
    trust_static(&delegate.store, &issuer.origin, &issuer.key());

    // The issuer holds no `photos`, so every spine node below `f:` compresses toward a space never granted.
    issuer.publish(
        1,
        &[("finance", "q3.pdf", b"withheld")],
        &[delegation(&delegate.key(), &["photos"])],
    );

    let syncer = Syncer::new(delegate.store.clone());
    let client = connect(&delegate, &issuer).await;
    syncer.sync_with(&client).await.unwrap();

    // The delegate converged rather than wedging, and holds nothing of the
    // space it was not granted — not even the name.
    let head = delegate
        .store
        .complete_head(&issuer.origin)
        .unwrap()
        .expect("the delegate promoted the issuer's head");
    assert_eq!(head.root, issuer.root());
    assert!(delegate
        .store
        .list_entries(Some(&issuer.origin), "finance", "", None, None)
        .unwrap()
        .is_empty());
    // A second pass is a no-op: the boundary was remembered, not re-asked.
    syncer.sync_with(&client).await.unwrap();
    assert!(delegate
        .store
        .pending_head(&issuer.origin)
        .unwrap()
        .is_none());
}

/// Withdrawing a delegation is deleting a trie key, and it propagates as any
/// deletion does (§6).
#[tokio::test]
async fn revocation_is_deletion_and_cuts_the_delegate_off() {
    let issuer = WireNode::spawn(Some("nas")).await;
    let delegate = WireNode::spawn(None).await;
    trust_static(&delegate.store, &issuer.origin, &issuer.key());

    issuer.publish(
        1,
        &[("photos", "a.jpg", b"bytes")],
        &[delegation(&delegate.key(), &["photos"])],
    );
    assert!(issuer
        .store
        .is_trusted_key(&delegate.key(), now_ns())
        .unwrap());

    // Remove the key from the trie and publish — no revocation state, no
    // tombstone; the key is simply gone from the new root.
    let trie = Trie::new(issuer.store.as_ref());
    let old = issuer.root();
    let root = trie.remove(old, &delegation_key(&delegate.key())).unwrap();
    let head = SignedHead::sign(&issuer.secret, issuer.origin.clone(), 2, root, now_ns());
    issuer
        .store
        .put_head(Slot::Complete, &head, now_ns(), now_ns())
        .unwrap();
    issuer
        .store
        .transaction(|txn| txn.materialize_diff(&issuer.origin, old, root))
        .unwrap();

    assert!(
        !issuer
            .store
            .is_trusted_key(&delegate.key(), now_ns())
            .unwrap(),
        "the delegation outlived its record"
    );
}

/// `GetValues` refuses on coverage, not only on position: the two handlers
/// must draw the same boundary. `GetNodes` applies the position check *and*
/// `Scope::admits_node`, because a node at an admitted position can still
/// describe a key that runs out of scope — a leaf spells the rest of its key,
/// and that key's value is the record.
#[tokio::test]
async fn a_value_is_refused_by_the_coverage_of_the_node_that_holds_it() {
    let issuer = WireNode::spawn(Some("nas")).await;
    let delegate = WireNode::spawn(None).await;
    trust_static(&delegate.store, &issuer.origin, &issuer.key());

    // A withheld space's manifest record (§5.5), padded past
    // `INLINE_VALUE_MAX` so the value sits out of line and is fetched by
    // `GetValues` rather than carried inside its node.
    let info = synch_core::SpaceInfo {
        v: synch_core::RECORD_VERSION,
        description: "w".repeat(synch_core::INLINE_VALUE_MAX * 4),
        entry_count: 9,
    };
    let record = postcard::to_stdvec(&info).unwrap();
    // Only the withheld space's manifest is published: with nothing else under
    // `m:` the trie collapses, and the leaf carrying the record sits on the
    // very spine the grant's own `m:` keys run through.
    issuer.publish(
        1,
        &[],
        &[
            delegation(&delegate.key(), &["photos"]),
            (
                synch_core::space_info_key("finance").unwrap(),
                record.clone(),
            ),
        ],
    );
    let root = issuer.root();

    // The node that carries the withheld payload, at a position the delegate's
    // scope admits: the spine. That pairing is the whole attack — an admitted
    // position holding a node whose coverage is not admitted.
    let scope = Scope::of(&synch_core::scope_prefixes(&["photos".to_string()]));
    let withheld = Hash::new(&record);
    let (path, _) = walk_all(issuer.store.as_ref(), root)
        .into_iter()
        .find(|(path, hash)| {
            scope.admits_path(path)
                && synch_mpt::NodeStore::get_node(issuer.store.as_ref(), hash)
                    .unwrap()
                    .map(|bytes| {
                        synch_mpt::TrieNode::decode(&bytes)
                            .map(|n| n.value_hashes().contains(&withheld))
                            .unwrap_or(false)
                    })
                    .unwrap_or(false)
        })
        .expect("the withheld value sits at no admitted position; test is vacuous");

    let client = connect(&delegate, &issuer).await;
    let answer = client
        .get_values(root, &[(path, withheld)])
        .await
        .expect("the request itself is well formed");
    assert!(
        answer.values.is_empty(),
        "a withheld record was served by a handler that checked only the position"
    );
    assert_eq!(answer.missing, vec![withheld]);
}
