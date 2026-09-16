//! What a node hands over on `HeadsWant` has to be a head it can serve the
//! trie of (§5.1).
//!
//! `Syncer::heads_for` used to answer out of the complete *slot*, which is not
//! the same question: a slot says this node promoted the head, and whether the
//! trie under it is whole is what `local_summaries` answers with `complete` in
//! the same exchange. A delegate's complete slot holds every foreign origin
//! over a *partial* trie by construction, so the two disagreed — and the
//! puller believed the head rather than the summary, because `sync_with`'s
//! adoption loop fetched from whoever handed one over without the
//! `summary.complete` guard its own pending-slot pass applied forty lines
//! later. Every member pulling from a delegate burned the whole
//! `MAX_UNPRODUCTIVE_ROUNDS` escape per origin, per exchange, to learn what
//! the summary in that same exchange had already told it.

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

    fn publish(&self, seq: u64, files: &[(&str, &str, &[u8])], extra: &[(Vec<u8>, Vec<u8>)]) {
        let trie = Trie::new(self.store.as_ref());
        let old = self.root();
        let mut root = old;
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

/// A delegate holds a foreign origin's head in its complete slot over a trie
/// it holds only the granted part of, and advertises `complete: false` to say
/// so. It hands the head over on `HeadsWant` anyway, so a member pulling from
/// it adopts a head nobody in the exchange can serve, spends
/// `MAX_UNPRODUCTIVE_ROUNDS` of `GetNodes` discovering that, and abandons it —
/// every round it happens to pick the delegate, for every foreign origin the
/// delegate carries.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delegate_does_not_hand_over_the_heads_it_cannot_serve() {
    let _blocking = synch_core::BlockingScope::enter();
    let issuer = Node::spawn(Some("nas")).await;
    let delegate = Node::spawn(None).await;
    let member = Node::spawn(Some("laptop")).await;

    trust_static(&delegate.store, &issuer.origin, &issuer.key());
    // The member trusts both the issuer and the delegate, and the delegate
    // trusts the member: an ordinary rooted pair.
    trust_static(&member.store, &issuer.origin, &issuer.key());
    trust_static(&member.store, &delegate.origin, &delegate.key());
    trust_static(&delegate.store, &member.origin, &member.key());

    issuer.publish(
        1,
        &[
            ("photos", "a.jpg", b"granted"),
            ("finance", "q3.pdf", b"withheld"),
        ],
        &[delegation(&delegate.key(), &["photos"])],
    );

    // The delegate syncs and promotes the issuer's head over a partial trie.
    let delegate_syncer = Syncer::new(delegate.store.clone());
    let to_issuer = delegate
        .net
        .connect_mpt(issuer.net.direct_addr())
        .await
        .unwrap();
    delegate_syncer.sync_with(&to_issuer).await.unwrap();
    assert_eq!(
        delegate
            .store
            .complete_head(&issuer.origin)
            .unwrap()
            .expect("the delegate promoted the head")
            .root,
        issuer.root()
    );

    // What the delegate says about that head, and what it hands over, disagree.
    let summaries = delegate_syncer.local_summaries().unwrap();
    let advertised = summaries
        .iter()
        .find(|s| s.origin == issuer.origin)
        .expect("the delegate advertises the origin");
    let served = delegate_syncer
        .heads_for(std::slice::from_ref(&issuer.origin))
        .unwrap();
    println!(
        "delegate advertises complete = {}, hands over {} head(s)",
        advertised.complete,
        served.len()
    );

    // And the member pulling from it pays for the disagreement.
    let member_syncer = Syncer::new(member.store.clone());
    let to_delegate = member
        .net
        .connect_mpt(delegate.net.direct_addr())
        .await
        .unwrap();
    let report = member_syncer.sync_with(&to_delegate).await.unwrap();
    println!("member pulling from the delegate: {report:?}");

    assert!(
        !advertised.complete && served.is_empty(),
        "what a node advertises and what it serves must agree"
    );
    assert_eq!(
        report.heads_abandoned, 0,
        "a peer that cannot serve a trie must not hand out its head"
    );
}

/// The other half of the same rule, from the puller's side (§5.5): a delegate
/// must not pull metadata from another delegate at all.
///
/// `heads_for` makes the delegate stop *offering* what it cannot serve. This is
/// what stops a delegate asking in the first place — and it is the constraint
/// that makes the read scope derivable from local state, because every peer a
/// delegate can reach serves the whole of its grant. A full member of some
/// other cluster is refused for the same reason: it has no trie of this
/// delegate's cluster to serve.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delegate_pulls_metadata_only_from_a_full_member_of_its_cluster() {
    let _blocking = synch_core::BlockingScope::enter();
    let issuer = Node::spawn(Some("nas")).await;
    let sibling = Node::spawn(None).await;
    let delegate = Node::spawn(None).await;

    trust_static(&delegate.store, &issuer.origin, &issuer.key());
    trust_static(&delegate.store, &sibling.origin, &sibling.key());
    trust_static(&sibling.store, &delegate.origin, &delegate.key());
    trust_static(&sibling.store, &issuer.origin, &issuer.key());

    issuer.publish(
        1,
        &[
            ("photos", "a.jpg", b"granted"),
            ("finance", "q3.pdf", b"withheld"),
        ],
        &[
            delegation(&delegate.key(), &["photos"]),
            delegation(&sibling.key(), &["photos"]),
        ],
    );

    // The delegate bootstraps from its issuer, which is a full member.
    let syncer = Syncer::new(delegate.store.clone());
    let to_issuer = delegate
        .net
        .connect_mpt(issuer.net.direct_addr())
        .await
        .unwrap();
    syncer.sync_with(&to_issuer).await.unwrap();
    assert_eq!(
        delegate.store.local_scope().unwrap(),
        Some(vec!["photos".to_string()]),
        "the delegate read its own grant out of the trie it just fetched"
    );

    // Now that it knows it is a delegate, the sibling delegate is refused —
    // both at the dial and, for a session opened before the grant landed, at
    // the exchange itself.
    let refused = delegate.net.connect_mpt(sibling.net.direct_addr()).await;
    assert!(
        matches!(refused, Err(synch_net::NetError::Untrusted(_))),
        "a delegate must not dial another delegate for metadata: {refused:?}"
    );

    // A full member of the delegate's own cluster stays reachable.
    let member = Node::spawn(Some("desktop")).await;
    trust_static(&delegate.store, &member.origin, &member.key());
    trust_static(&member.store, &delegate.origin, &delegate.key());
    assert!(
        delegate
            .net
            .connect_mpt(member.net.direct_addr())
            .await
            .is_ok(),
        "a full member of its own cluster is exactly who it should be pulling from"
    );

    // And a full member of a different cluster is not.
    let stranger = Node::spawn(Some("laptop")).await;
    let other = OriginId::named("laptop", "other.example").unwrap();
    delegate
        .store
        .put_binding(&Binding {
            origin: other,
            node_id: stranger.key(),
            source: BindingSource::Static,
            domain: None,
            issuer: None,
            spaces: Vec::new(),
            note: None,
            added_at: 0,
            expires_at: None,
        })
        .unwrap();
    let refused = delegate.net.connect_mpt(stranger.net.direct_addr()).await;
    assert!(
        matches!(refused, Err(synch_net::NetError::Untrusted(_))),
        "a full member of another cluster has nothing of this one to serve: {refused:?}"
    );
}
