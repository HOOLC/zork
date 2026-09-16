//! A read scope that moves has to take the trie and the derived views with it
//! (§5.5).
//!
//! The read scope decides three things at once: what `MissingWalk` asks for,
//! what `is_complete_scoped` counts as whole, and what `materialize_diff`
//! walks. It used to be one node-wide value adopted from whichever peer spoke
//! last, and nothing re-derived the first two for a head already in the
//! complete slot — so a scope that moved left that trie permanently short, and
//! the next promotion's diff, which prunes at equal node hashes, could neither
//! reach what a narrower walk had skipped nor remove what a wider one had
//! covered.
//!
//! It is now derived per node from the `d:` record naming it, and nothing a
//! peer says is remembered. These pin what that has to mean: a widened grant
//! whose new space appears, a widened grant whose new space *changed* and once
//! wedged the origin for good, a narrowed grant that stops serving what it
//! lost, a delegate promoted to full membership, and a scope that does not move
//! whoever this node happens to talk to.

use std::sync::Arc;

use iroh_base::SecretKey;
use synch_core::{delegation_key, file_key, now_ns, Delegation, FileEntry, Hash, NodeId, OriginId};
use synch_engine::Syncer;
use synch_mpt::Trie;
use synch_net::{Net, NetOptions};
use synch_store::{Binding, BindingSource, Slot, Store};

struct Node {
    _dir: tempfile::TempDir,
    store: Arc<Store>,
    net: Net,
    secret: SecretKey,
    origin: OriginId,
}

impl Node {
    /// `named` spawns a rooted member; `None` spawns a key-identified node,
    /// which is the only shape a delegation may bind (§3.5).
    async fn spawn(named: Option<&str>) -> Node {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap());
        let secret = SecretKey::generate();
        let origin = match named {
            Some(name) => OriginId::named(name, "cluster.example").unwrap(),
            None => OriginId::Key(secret.public()),
        };
        store.set_self_origin(&origin).unwrap();
        trust_static(&store, &origin, &secret.public());
        let mut options = NetOptions::loopback();
        options.heads = Some(Arc::new(Syncer::new(store.clone())) as Arc<dyn synch_net::HeadSink>);
        let net = Net::bind(store.clone(), secret.clone(), options)
            .await
            .unwrap();
        Node {
            _dir: dir,
            store,
            net,
            secret,
            origin,
        }
    }

    fn key(&self) -> NodeId {
        self.secret.public()
    }

    fn root(&self) -> Hash {
        self.store
            .complete_head(&self.origin)
            .unwrap()
            .map(|h| h.root)
            .unwrap_or(Hash::EMPTY)
    }

    /// Publishes `files` as `(space, path, content)` plus any raw records, as
    /// one signed head — the same shape `tests/delegation.rs` publishes in.
    fn publish(&self, seq: u64, files: &[(&str, &str, &[u8])], extra: &[(Vec<u8>, Vec<u8>)]) {
        self.publish_removing(seq, files, extra, &[])
    }

    /// The same, also deleting `remove` — which is what revoking a delegation
    /// is: the `d:` key vanishes from the next root (§3.5).
    fn publish_removing(
        &self,
        seq: u64,
        files: &[(&str, &str, &[u8])],
        extra: &[(Vec<u8>, Vec<u8>)],
        remove: &[Vec<u8>],
    ) {
        let trie = Trie::new(self.store.as_ref());
        let old = self.root();
        let mut root = old;
        for key in remove {
            root = trie.remove(root, key).unwrap();
        }
        for (space, path, content) in files {
            let object = self.store.ingest_bytes(content, now_ns()).unwrap();
            let entry = FileEntry::file(content.len() as u64, 0, object, seq);
            root = trie
                .insert(
                    root,
                    &file_key(space, path).unwrap(),
                    &postcard::to_stdvec(&entry).unwrap(),
                )
                .unwrap();
            let ad = self.store.local_ad(&object).unwrap().unwrap();
            root = trie
                .insert(
                    root,
                    &synch_core::blob_key(&object),
                    &postcard::to_stdvec(&ad).unwrap(),
                )
                .unwrap();
        }
        for (key, value) in extra {
            root = trie.insert(root, key, value).unwrap();
        }
        let head =
            synch_core::SignedHead::sign(&self.secret, self.origin.clone(), seq, root, now_ns());
        self.store
            .put_head(Slot::Complete, &head, now_ns(), now_ns())
            .unwrap();
        self.store
            .transaction(|txn| txn.materialize_diff(&self.origin, old, root))
            .unwrap();
        synch_mpt::NodeStore::note_complete(self.store.as_ref(), &root).unwrap();
    }

    fn entries(&self, origin: &OriginId, space: &str) -> usize {
        self.store
            .list_entries(Some(origin), space, "", None, None)
            .unwrap()
            .len()
    }
}

fn trust_static(store: &Store, origin: &OriginId, key: &NodeId) {
    store
        .put_binding(&Binding {
            origin: origin.clone(),
            node_id: *key,
            source: BindingSource::Static,
            domain: None,
            issuer: None,
            spaces: Vec::new(),
            note: None,
            added_at: 0,
            expires_at: None,
        })
        .unwrap();
}

/// The `d:` record an issuer publishes to delegate `subject` (§3.5).
fn delegation(subject: &NodeId, spaces: &[&str]) -> (Vec<u8>, Vec<u8>) {
    let record = Delegation {
        v: synch_core::RECORD_VERSION,
        spaces: spaces.iter().map(|s| s.to_string()).collect(),
        not_after: now_ns() + 86_400_000_000_000,
        note: Some("audit delegate".into()),
    };
    (
        delegation_key(subject),
        postcard::to_stdvec(&record).unwrap(),
    )
}

/// Runs `rounds` exchanges, printing each report so a failure shows whether the
/// origin is stuck or merely slow.
///
/// More than one is needed after a scope change by construction: the round that
/// carries the new grant adopts it and discards everything derived under the
/// old one, and the round after that refetches and rebuilds.
async fn rounds(syncer: &Syncer, client: &synch_net::MptClient, label: &str, rounds: usize) {
    for round in 0..rounds {
        match syncer.sync_with(client).await {
            Ok(report) => println!("{label} round {round}: {report:?}"),
            Err(e) => println!("{label} round {round}: ERROR {e}"),
        }
    }
}

/// **F1a — a widened grant materializes the space it just gained.**
///
/// The newly granted space is untouched by the head that carries the wider
/// grant, which is the ordinary case: an operator widens a delegation and the
/// space that was just opened up has not changed. The delegate fetches the new
/// root under the wider scope, so the record *is* in its trie — but the
/// promotion diff prunes at the shared node hash, and `entries` never learns
/// about it. Nothing reports a problem. `repair rebuild-views` is the repair.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_widened_delegation_materializes_the_space_it_just_gained() {
    let _blocking = synch_core::BlockingScope::enter();
    let issuer = Node::spawn(Some("nas")).await;
    let delegate = Node::spawn(None).await;
    trust_static(&delegate.store, &issuer.origin, &issuer.key());

    issuer.publish(
        1,
        &[
            ("photos", "a.jpg", b"the granted bytes"),
            ("finance", "q3.pdf", b"the withheld bytes"),
        ],
        &[delegation(&delegate.key(), &["photos"])],
    );

    let syncer = Syncer::new(delegate.store.clone());
    let client = delegate
        .net
        .connect_mpt(issuer.net.direct_addr())
        .await
        .unwrap();
    syncer.sync_with(&client).await.unwrap();
    assert_eq!(
        delegate.store.local_scope().unwrap(),
        Some(vec!["photos".to_string()])
    );

    // The grant widens. `finance` is untouched by this head.
    issuer.publish(
        2,
        &[("photos", "b.jpg", b"another granted file")],
        &[delegation(&delegate.key(), &["photos", "finance"])],
    );
    rounds(&syncer, &client, "widen", 3).await;

    assert_eq!(
        delegate.store.local_scope().unwrap(),
        Some(vec!["finance".to_string(), "photos".to_string()]),
        "the delegate learned the wider grant"
    );
    let head = delegate
        .store
        .complete_head(&issuer.origin)
        .unwrap()
        .unwrap();
    assert_eq!(head.seq, 2, "and promoted the head that carried it");

    // The trie really does hold the newly granted record...
    let in_trie = Trie::new(delegate.store.as_ref())
        .get(head.root, &file_key("finance", "q3.pdf").unwrap())
        .unwrap();
    assert!(in_trie.is_some(), "the record reached the trie");

    // ...but the derived view — what the unified tree, checkouts and the S3
    // gateway read — is short, and nothing says so. Read before the repair
    // below, which is what makes this a materialization bug and not a fetch
    // one: `repair rebuild-views` puts the record in `entries`, and nothing in the
    // protocol ever runs it.
    let (photos, finance) = (
        delegate.entries(&issuer.origin, "photos"),
        delegate.entries(&issuer.origin, "finance"),
    );
    let rebuilt = delegate
        .store
        .rematerialize(&issuer.origin, head.root)
        .expect("a cold rebuild succeeds");
    println!(
        "before rebuild: photos {photos}, finance {finance}; \
         rebuild applied {rebuilt} changes, leaving finance {}",
        delegate.entries(&issuer.origin, "finance")
    );

    assert_eq!(photos, 2, "both granted photos are materialized");
    assert_eq!(
        finance, 1,
        "the newly granted space must be materialized without an operator"
    );
}

/// **F1b — a widened grant whose new space *changed* still promotes.**
///
/// Here the promotion diff descends into the newly granted subtree instead of
/// pruning over it, and the *old* root has no node there — it was never
/// fetched under the narrow scope. `MptError::MissingNode` is classified as an
/// origin fault, so the head is retired and put in the refusal memo, and the
/// origin is left behind on every round from then on. `repair rebuild-views`
/// cannot repair it either: the trie under the stuck complete head is itself
/// short of the new scope.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_widened_delegation_with_a_changed_space_still_promotes() {
    let _blocking = synch_core::BlockingScope::enter();
    let issuer = Node::spawn(Some("nas")).await;
    let delegate = Node::spawn(None).await;
    trust_static(&delegate.store, &issuer.origin, &issuer.key());

    issuer.publish(
        1,
        &[
            ("photos", "a.jpg", b"the granted bytes"),
            ("finance", "q3.pdf", b"the withheld bytes"),
        ],
        &[delegation(&delegate.key(), &["photos"])],
    );

    let syncer = Syncer::new(delegate.store.clone());
    let client = delegate
        .net
        .connect_mpt(issuer.net.direct_addr())
        .await
        .unwrap();
    syncer.sync_with(&client).await.unwrap();

    issuer.publish(
        2,
        &[("finance", "q4.pdf", b"a new finance file")],
        &[delegation(&delegate.key(), &["photos", "finance"])],
    );
    rounds(&syncer, &client, "widen", 3).await;

    let head = delegate.store.complete_head(&issuer.origin).unwrap();
    if let Some(h) = &head {
        let repair = delegate
            .store
            .rematerialize(&issuer.origin, h.root)
            .map(|n| n.to_string())
            .unwrap_or_else(|e| format!("ERROR {e}"));
        println!("repair rebuild-views at the stuck head: {repair}");
    }
    assert_eq!(head.map(|h| h.seq), Some(2), "the head must promote");
    assert_eq!(
        delegate.entries(&issuer.origin, "finance"),
        2,
        "both finance records must materialize"
    );
}

/// **F1c — a delegate promoted to a full member replicates everything.**
///
/// Promotion is revoking the delegation, not merely rooting the key: a node is
/// a delegate exactly while some origin's trie names it in a `d:` record, which
/// is the cluster-visible statement of its role. Rooting it without revoking
/// leaves a contradictory config, and under a derived scope the record wins —
/// which is the point, since it is the thing every other member reads too.
///
/// Two things have to happen and only one of them is carried by a head. The
/// revocation is: the promotion of that publish removes the binding. The
/// widening is not — no head follows it — so the maintenance pass is what
/// notices, and this asserts that it does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delegate_promoted_to_a_full_member_replicates_everything() {
    let _blocking = synch_core::BlockingScope::enter();
    let issuer = Node::spawn(Some("nas")).await;
    let delegate = Node::spawn(None).await;
    trust_static(&delegate.store, &issuer.origin, &issuer.key());

    issuer.publish(
        1,
        &[
            ("photos", "a.jpg", b"the granted bytes"),
            ("finance", "q3.pdf", b"the withheld bytes"),
        ],
        &[delegation(&delegate.key(), &["photos"])],
    );

    let syncer = Syncer::new(delegate.store.clone());
    let client = delegate
        .net
        .connect_mpt(issuer.net.direct_addr())
        .await
        .unwrap();
    syncer.sync_with(&client).await.unwrap();
    assert_eq!(
        delegate.store.local_scope().unwrap(),
        Some(vec!["photos".to_string()])
    );

    // The operator promotes the delegate: a rooted binding for its key, and
    // the delegation revoked in the same head. The delegate's own resolver
    // materializes the promotion the same way — a rooted binding for its own
    // key in the issuer's origin — which is the local evidence a later
    // `Unrestricted` declaration is checked against (§5.5).
    trust_static(&issuer.store, &delegate.origin, &delegate.key());
    trust_static(&delegate.store, &issuer.origin, &delegate.key());
    issuer.publish_removing(
        2,
        &[("photos", "b.jpg", b"another file")],
        &[],
        &[delegation_key(&delegate.key())],
    );
    assert_eq!(
        issuer
            .store
            .publish_scope_of_key(&delegate.key(), now_ns())
            .unwrap(),
        synch_store::PublishScope::Unrestricted
    );
    rounds(&syncer, &client, "promote", 3).await;

    assert_eq!(
        delegate.store.local_scope().unwrap(),
        None,
        "the delegate is a full member now"
    );

    assert_eq!(
        delegate.entries(&issuer.origin, "finance"),
        1,
        "a full member replicates every space"
    );
}

/// **F1d — a narrowed grant drops the revoked space from the derived views.**
///
/// The mirror image of F1a: nothing re-derives `entries` for a scope that
/// shrank, so the delegate goes on listing and serving the space it lost.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_narrowed_delegation_drops_what_it_no_longer_covers() {
    let _blocking = synch_core::BlockingScope::enter();
    let issuer = Node::spawn(Some("nas")).await;
    let delegate = Node::spawn(None).await;
    trust_static(&delegate.store, &issuer.origin, &issuer.key());

    issuer.publish(
        1,
        &[("photos", "a.jpg", b"one"), ("finance", "q3.pdf", b"two")],
        &[delegation(&delegate.key(), &["photos", "finance"])],
    );

    let syncer = Syncer::new(delegate.store.clone());
    let client = delegate
        .net
        .connect_mpt(issuer.net.direct_addr())
        .await
        .unwrap();
    syncer.sync_with(&client).await.unwrap();
    assert_eq!(delegate.entries(&issuer.origin, "finance"), 1);

    issuer.publish(
        2,
        &[("photos", "b.jpg", b"three")],
        &[delegation(&delegate.key(), &["photos"])],
    );
    rounds(&syncer, &client, "narrow", 3).await;

    assert_eq!(
        delegate.store.local_scope().unwrap(),
        Some(vec!["photos".to_string()])
    );
    assert_eq!(
        delegate.entries(&issuer.origin, "finance"),
        0,
        "the revoked space must leave the derived views"
    );
}

/// **F1e — the scope does not depend on which peer spoke last.**
///
/// A delegate hears a different declaration from every peer that sees it
/// differently, and each is honest about what *that* peer will serve. Two
/// laptops in the same position, fed the same two peers in opposite orders,
/// must hold the same rows — and no round with either peer may move them.
///
/// What settles it is that nothing a peer says is remembered: the scope is the
/// grant in the `d:` record naming this node, read out of the trie it already
/// replicates, so the answer is the same before and after any exchange with
/// anybody. A declaration only ever narrows the walk.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delegates_scope_is_the_same_whatever_order_it_meets_its_peers_in() {
    let _blocking = synch_core::BlockingScope::enter();
    let work = Node::spawn(Some("work")).await;
    let home = Node::spawn(Some("home")).await;

    work.publish(
        1,
        &[
            ("reports", "q3.pdf", b"delegated"),
            ("secrets", "keys.txt", b"withheld"),
        ],
        &[],
    );
    home.publish(1, &[("family", "a.jpg", b"pictures")], &[]);

    // Two laptops in the same position, differing only in the order they meet
    // their peers in — which is what §5.3's random peer choice decides.
    let mut settled = Vec::new();
    for order in [[0usize, 1, 0, 1], [1, 0, 1, 0]] {
        let laptop = Node::spawn(None).await;
        trust_static(&laptop.store, &work.origin, &work.key());
        trust_static(&laptop.store, &home.origin, &home.key());
        trust_static(&home.store, &laptop.origin, &laptop.key());
        work.publish(
            work.store
                .complete_head(&work.origin)
                .unwrap()
                .map(|h| h.seq + 1)
                .unwrap_or(1),
            &[],
            &[delegation(&laptop.key(), &["reports"])],
        );

        let syncer = Syncer::new(laptop.store.clone());
        let to_work = laptop
            .net
            .connect_mpt(work.net.direct_addr())
            .await
            .unwrap();
        let to_home = laptop
            .net
            .connect_mpt(home.net.direct_addr())
            .await
            .unwrap();

        let mut seen = Vec::new();
        for &which in &order {
            let (peer, label) = match which {
                0 => (&to_work, "work"),
                _ => (&to_home, "home"),
            };
            rounds(&syncer, peer, label, 1).await;
            let scope = laptop.store.local_scope().unwrap();
            println!("  [{order:?}] scope now {scope:?}");
            seen.push(scope);
        }
        // The first round is the bootstrap — a delegate holds no grant until it
        // has replicated the trie the granting record lives in, so `None` there
        // is the honest answer and not a scope. From the moment it is known,
        // nothing a peer says may move it: it is read out of the trie, and the
        // trie is the same either way.
        assert!(
            seen[1..].iter().all(|scope| scope == &seen[1]),
            "a peer's declaration moved this node's scope once it knew it: {seen:?}"
        );
        assert!(
            seen[1].is_some(),
            "the grant must be known after the first round: {seen:?}"
        );
        settled.push((
            laptop.store.local_scope().unwrap(),
            laptop.entries(&work.origin, "reports"),
            laptop.entries(&work.origin, "secrets"),
            laptop.entries(&home.origin, "family"),
        ));
    }
    assert_eq!(
        settled[0], settled[1],
        "two nodes fed the same peers in opposite orders must agree: {settled:?}"
    );
}

/// **F1f — an unattended round through a more generous peer changes nothing.**
///
/// The laptop is a delegate of `work`, and `home` — which also replicates
/// `work` — holds a rooted binding for the laptop, so `home` will serve it the
/// whole keyspace. An ordinary anti-entropy round that happens to pick `home`
/// used to promote `work`'s head under that wider scope and leave the origin
/// behind for good, past returning to `work`.
///
/// Now the declaration cannot reach the promotion at all: the laptop
/// materializes its own grant whoever it heard from, and `scope_meet` keeps
/// the walk from pulling the rest in the first place.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_round_through_a_wider_peer_keeps_the_delegating_origin_replicating() {
    let _blocking = synch_core::BlockingScope::enter();
    let work = Node::spawn(Some("work")).await;
    let home = Node::spawn(Some("home")).await;
    let laptop = Node::spawn(None).await;

    trust_static(&laptop.store, &work.origin, &work.key());
    trust_static(&laptop.store, &home.origin, &home.key());
    trust_static(&home.store, &laptop.origin, &laptop.key());
    trust_static(&home.store, &work.origin, &work.key());
    trust_static(&work.store, &home.origin, &home.key());

    work.publish(
        1,
        &[
            ("reports", "q3.pdf", b"delegated"),
            ("secrets", "keys.txt", b"withheld"),
        ],
        &[delegation(&laptop.key(), &["reports"])],
    );

    // Home replicates work in full.
    let home_syncer = Syncer::new(home.store.clone());
    let home_to_work = home.net.connect_mpt(work.net.direct_addr()).await.unwrap();
    home_syncer.sync_with(&home_to_work).await.unwrap();
    assert_eq!(
        home.store
            .complete_head(&work.origin)
            .unwrap()
            .unwrap()
            .root,
        work.root()
    );

    // The laptop replicates work under its grant.
    let syncer = Syncer::new(laptop.store.clone());
    let to_work = laptop
        .net
        .connect_mpt(work.net.direct_addr())
        .await
        .unwrap();
    syncer.sync_with(&to_work).await.unwrap();
    assert_eq!(
        laptop.store.local_scope().unwrap(),
        Some(vec!["reports".to_string()])
    );

    // Work publishes again; home picks it up.
    work.publish(2, &[("reports", "q4.pdf", b"more")], &[]);
    home_syncer.sync_with(&home_to_work).await.unwrap();

    // The laptop's next rounds happen to pick `home`, which declares the
    // same confined scope the issuer does (it replicated the `d:` record)
    // — and which a wider or emptier declaration could not move either.
    let to_home = laptop
        .net
        .connect_mpt(home.net.direct_addr())
        .await
        .unwrap();
    rounds(&syncer, &to_home, "home", 4).await;
    // And going back to the delegating origin does not repair it.
    rounds(&syncer, &to_work, "work", 3).await;

    let head = laptop.store.complete_head(&work.origin).unwrap();
    assert_eq!(
        head.map(|h| h.seq),
        Some(2),
        "the delegating origin must keep replicating"
    );
    assert_eq!(
        laptop.entries(&work.origin, "reports"),
        2,
        "and its new record must materialize"
    );
}

/// **F1g — a round through a peer that never saw the grant does not widen.**
///
/// The peer holds the delegating origin's trie complete at a head from
/// *before* the `d:` record existed, and a rooted binding for the laptop —
/// so it declares `Unrestricted`, the old encoding's `None`, which the
/// laptop used to adopt wholesale as "the whole keyspace". The round still
/// passes the issuer-trie guard (the peer does advertise a complete head of
/// the issuer), so only the grant-derived scope can stop the widening.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_round_through_a_peer_that_never_saw_the_grant_does_not_widen() {
    let _blocking = synch_core::BlockingScope::enter();
    let work = Node::spawn(Some("work")).await;
    let stale = Node::spawn(Some("stale")).await;
    let laptop = Node::spawn(None).await;

    trust_static(&laptop.store, &work.origin, &work.key());
    trust_static(&laptop.store, &stale.origin, &stale.key());
    trust_static(&stale.store, &work.origin, &work.key());
    // The reciprocal bindings the connection gate needs: stale may dial work
    // (it replicates it in full) and the laptop may dial stale.
    trust_static(&work.store, &stale.origin, &stale.key());
    trust_static(&stale.store, &laptop.origin, &laptop.key());

    // Work publishes before delegating; the stale peer replicates that head.
    work.publish(
        1,
        &[
            ("reports", "q3.pdf", b"delegated"),
            ("secrets", "keys.txt", b"withheld"),
        ],
        &[],
    );
    let stale_syncer = Syncer::new(stale.store.clone());
    let stale_to_work = stale.net.connect_mpt(work.net.direct_addr()).await.unwrap();
    stale_syncer.sync_with(&stale_to_work).await.unwrap();

    // The grant arrives in the next head; the laptop replicates under it.
    work.publish(
        2,
        &[("reports", "q4.pdf", b"more")],
        &[delegation(&laptop.key(), &["reports"])],
    );
    let syncer = Syncer::new(laptop.store.clone());
    let to_work = laptop
        .net
        .connect_mpt(work.net.direct_addr())
        .await
        .unwrap();
    syncer.sync_with(&to_work).await.unwrap();
    assert_eq!(
        laptop.store.local_scope().unwrap(),
        Some(vec!["reports".to_string()])
    );

    // The laptop's next rounds happen to pick the stale peer, which still
    // holds only the pre-delegation head and declares `Unrestricted`.
    let to_stale = laptop
        .net
        .connect_mpt(stale.net.direct_addr())
        .await
        .unwrap();
    rounds(&syncer, &to_stale, "stale", 3).await;

    assert_eq!(
        laptop.store.local_scope().unwrap(),
        Some(vec!["reports".to_string()]),
        "a peer that never saw the grant must not widen the delegate"
    );
    assert_eq!(
        laptop.entries(&work.origin, "reports"),
        2,
        "the granted records stay materialized"
    );
    assert_eq!(
        laptop.entries(&work.origin, "secrets"),
        0,
        "and the withheld ones stay out"
    );
}

/// **F1h — a delegation whose grant expired collapses the scope to nothing.**
///
/// The moment the `d:` record's `not_after` passes, both sides' bindings
/// lapse: the issuer stops serving the delegate at the connection gate, so
/// no peer's declaration ever reaches it again — the maintenance pass is
/// the only thing that can drive the derived views away. The collapsed
/// scope is the empty one (`m:self` and the `d:` namespace, no file data);
/// a grant can only come back through a fresh materialized `d:` record.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_expired_delegation_collapses_the_read_scope_to_nothing() {
    let _blocking = synch_core::BlockingScope::enter();
    let issuer = Node::spawn(Some("nas")).await;
    let delegate = Node::spawn(None).await;
    trust_static(&delegate.store, &issuer.origin, &issuer.key());

    // A grant that expires three seconds from now: long enough for the
    // bootstrap sync, short enough to lapse inside the test.
    let record = synch_core::Delegation {
        v: synch_core::RECORD_VERSION,
        spaces: vec!["photos".to_string()],
        not_after: now_ns() + 3_000_000_000,
        note: None,
    };
    let extra = vec![(
        delegation_key(&delegate.key()),
        postcard::to_stdvec(&record).unwrap(),
    )];
    issuer.publish(1, &[("photos", "a.jpg", b"the granted bytes")], &extra);

    let syncer = Syncer::new(delegate.store.clone());
    let client = delegate
        .net
        .connect_mpt(issuer.net.direct_addr())
        .await
        .unwrap();
    syncer.sync_with(&client).await.unwrap();
    assert_eq!(
        delegate.store.local_scope().unwrap(),
        Some(vec!["photos".to_string()])
    );

    // The grant lapses. The issuer's binding lapsed with it, so the delegate
    // is cut off at the connection gate and never hears from a peer again;
    // the maintenance pass collapses the grantless scope.
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    let collapsed = delegate.store.collapse_grantless_scope(now_ns()).unwrap();
    assert!(collapsed, "the grantless scope must move");

    assert_eq!(
        delegate.store.local_scope().unwrap(),
        Some(Vec::<String>::new()),
        "an expired delegation collapses the read scope to the empty one"
    );
    assert_eq!(
        delegate.entries(&issuer.origin, "photos"),
        0,
        "the expired spaces leave the derived views"
    );
}
