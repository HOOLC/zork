//! A low-level in-process node: a store, a bound signing key, and a real iroh
//! endpoint speaking the metadata and blob protocols — the harness for tests
//! that drive the wire directly (delegation, two_nodes) instead of the `Node` API.
#![allow(dead_code)]

use std::sync::Arc;

use iroh_base::SecretKey;
use synch_core::{file_key, now_ns, FileEntry, Hash, NodeId, OriginId, SignedHead};
use synch_engine::Syncer;
use synch_mpt::Trie;
use synch_net::{Net, NetOptions};
use synch_store::{Binding, BindingSource, Slot, Store};

pub(crate) struct WireNode {
    pub _dir: tempfile::TempDir,
    pub store: Arc<Store>,
    pub cas: Arc<dyn synch_store::backend::CasBackend>,
    pub net: Net,
    pub secret: SecretKey,
    pub origin: OriginId,
}

impl WireNode {
    /// `named` spawns a rooted member; `None` spawns a key-identified node,
    /// which is the only shape a delegation may bind (§2).
    pub(crate) async fn spawn(named: Option<&str>) -> WireNode {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap());
        let secret = SecretKey::generate();
        let origin = match named {
            Some(name) => OriginId::named(name, "cluster.example").unwrap(),
            None => OriginId::Key(secret.public()),
        };
        store.set_self_origin(&origin).unwrap();
        // A node always trusts itself, so its own key is bound first.
        trust(&store, &origin, &secret.public());
        // The endpoint reconciles through a head sink — the §5.2 acceptance
        // rule — and dials with the same object it serves through.
        let mut options = NetOptions::loopback();
        let cas: Arc<dyn synch_store::backend::CasBackend> =
            Arc::new(synch_store::backend::LocalFs::new(store.clone()));
        options.cas = Some(cas.clone());
        options.heads = Some(Arc::new(Syncer::new(store.clone())) as Arc<dyn synch_net::HeadSink>);
        let net = Net::bind(store.clone(), secret.clone(), options)
            .await
            .unwrap();
        WireNode {
            _dir: dir,
            store,
            cas,
            net,
            secret,
            origin,
        }
    }

    pub(crate) fn key(&self) -> NodeId {
        self.secret.public()
    }

    /// The root this node holds complete for its own origin, or `EMPTY`.
    pub(crate) fn root(&self) -> Hash {
        self.store
            .complete_head(&self.origin)
            .unwrap()
            .map(|h| h.root)
            .unwrap_or(Hash::EMPTY)
    }

    /// Publishes `files` as `(space, path, content)` plus raw records, as a new signed head.
    pub(crate) fn publish(
        &self,
        seq: u64,
        files: &[(&str, &str, &[u8])],
        extra: &[(Vec<u8>, Vec<u8>)],
    ) -> SignedHead {
        let trie = Trie::new(self.store.as_ref());
        // One transaction, as every production writer of the complete slot does it (§5.2).
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
        let head = SignedHead::sign(&self.secret, self.origin.clone(), seq, root, now_ns());
        self.store
            .transaction(|txn| -> Result<(), synch_store::StoreError> {
                txn.put_head(Slot::Complete, &head, now_ns(), now_ns())?;
                txn.materialize_diff(&self.origin, old, root)?;
                Ok(())
            })
            .unwrap();
        head
    }
}

/// A metadata client from `a` to `b`, for tests that drive the wire directly.
pub(crate) async fn connect(a: &WireNode, b: &WireNode) -> synch_net::MptClient {
    a.net.connect_mpt(b.net.direct_addr()).await.unwrap()
}

/// A blob client from `a` to `b`, for tests that drive `GetSlice` (§6.4).
pub(crate) async fn connect_blob(a: &WireNode, b: &WireNode) -> synch_net::BlobClient {
    a.net.connect_blob(b.net.direct_addr()).await.unwrap()
}

/// A static binding of `origin` to `key`, as an operator would configure one.
pub(crate) fn trust(store: &Store, origin: &OriginId, key: &NodeId) {
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

/// Shuts every endpoint down, so a test can tear down in one line.
pub(crate) async fn shutdown_all(nodes: &[&WireNode]) {
    for node in nodes {
        node.net.shutdown().await.unwrap();
    }
}

/// Every node in `nodes` trusts every other node's origin and key.
pub(crate) fn trust_all(nodes: &[&WireNode]) {
    for a in nodes {
        for b in nodes {
            trust(&a.store, &b.origin, &b.key());
        }
    }
}
