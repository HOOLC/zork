//! The embeddable node: identity, spaces, publishing, and peering.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use iroh::EndpointAddr;
use iroh_base::SecretKey;
use synch_core::{
    blob_key, file_key, manifest_key, now_ns, parse_file_key, validate_space, BlobAd, Delegation,
    EntryKind, Hash, NodeId, NodeManifest, OriginId, SignedHead, SpaceInfo, SOFTWARE,
};
use synch_mpt::Trie;
use synch_net::Net;

use crate::reconcile::Syncer;
use synch_store::{Binding, BindingSource, KeyState, Slot, SourceKind, Store};

use crate::{
    config::NodeConfig,
    error::{EngineError, Result},
    publisher::Publisher,
};

/// The binding by which a node holds its own name.
///
/// Without it a node could not verify its own heads after a restart: every head
/// check goes through the bindings table (§3.1), including one it signed itself.
fn self_binding(origin: &OriginId, node_id: NodeId, now: i64) -> Binding {
    Binding {
        origin: origin.clone(),
        node_id,
        source: BindingSource::Static,
        domain: None,
        issuer: None,
        spaces: Vec::new(),
        note: Some("self".into()),
        added_at: now,
        expires_at: None,
    }
}

/// A staged trie change: a key, and its new value or `None` to remove it.
pub(crate) type StagedChange = (Vec<u8>, Option<Vec<u8>>);

/// A running node.
///
/// This is the whole embeddable API: any Rust application can hold one of these
/// and get a full participant in the cluster.
#[derive(Debug, Clone)]
pub struct Node {
    inner: Arc<NodeInner>,
}

/// A non-owning handle onto a [`Node`]. See [`Node::downgrade`].
#[derive(Debug, Clone, Default)]
pub(crate) struct WeakNode(std::sync::Weak<NodeInner>);

impl WeakNode {
    /// The node, if it is still open.
    pub(crate) fn upgrade(&self) -> Option<Node> {
        self.0.upgrade().map(|inner| Node { inner })
    }
}

#[derive(Debug)]
struct NodeInner {
    store: Arc<Store>,
    /// Object-safe CAS semantics used by engine and network call sites.
    cas: Arc<dyn synch_store::backend::CasBackend>,
    /// The endpoint under the currently active device key: what this node
    /// dials from and what peers reach first.
    net: std::sync::RwLock<Net>,
    /// Endpoints kept live for keys that are on their way out, so both keys
    /// serve concurrently through a rotation's overlap window (§3.4).
    retiring: std::sync::Mutex<Vec<Net>>,
    syncer: Syncer,
    origin: OriginId,
    /// The signing key. Swapped only by `synch key activate` (§3.4); a node
    /// never switches keys on its own.
    secret: std::sync::RwLock<SecretKey>,
    config: NodeConfig,
    /// The batch between staging and one signed root (§7.1).
    publisher: Publisher,
    ad_clock: std::sync::Mutex<std::collections::HashMap<Hash, i64>>,
    /// Content roots that provider discovery has failed to resolve, and when
    /// each may be asked about again (§6.3).
    ///
    /// Discovery walks every dialable peer, and it is entered whenever no local
    /// ad covers a root — so a root nobody holds is re-planned by every checkout
    /// pass and re-dials the whole cluster, forever. An origin publishing `f:`
    /// records whose content hashes name nothing therefore starves the victim's
    /// counterpart of the passes that would have done real work, and `trust rm` does
    /// not clear it because what has already been published is retained for
    /// `root_retention`. The entries expire, so a root that later becomes
    /// available is picked up.
    provider_misses: std::sync::Mutex<std::collections::HashMap<Hash, ProviderMiss>>,
    /// What the running checkout passes believe about the file at each target
    /// path, and the stat that belief is anchored to — so a quiet pass can
    /// skip re-hashing every file it has already written or read
    /// (`docs/DELTA-SYNC.md` §3.5).
    checkout_writes: std::sync::Mutex<std::collections::HashMap<PathBuf, CheckoutWrite>>,
    /// Socket program bytes, shared across the admissions of one content root.
    program_bytes: Arc<crate::sockets::ProgramBytesCache>,
    /// The socket worker pool, or `None` where this build has no eBPF runtime
    /// (`docs/SOCKETS.md` §5.1).
    ///
    /// Started with the node rather than lazily on the first connection: it
    /// installs process-wide SIGUSR1 and SIGSEGV handlers, and that is a thing
    /// to do while starting up rather than while serving.
    sockets: Option<crate::sockets::SocketPool>,
    /// Serializes socket authorization changes against the final admission
    /// check. Slow reads and declaration hooks stay outside it; only the
    /// transition that makes an invocation live shares this gate with arm,
    /// disarm, redeclare, removal, and quarantine.
    socket_authorization: std::sync::RwLock<()>,
    /// When each configured membership domain is next due for re-resolution,
    /// and when it was last attempted (§3.2, §3.4).
    dns: std::sync::Mutex<std::collections::HashMap<String, crate::membership::DomainSchedule>>,
    /// Rung when an inbound connection is refused for an unknown device key,
    /// which §3.4 makes a trigger for an immediate DNS re-resolution.
    dns_wake: Arc<tokio::sync::Notify>,
    /// The one resolver every membership refresh in this process goes through —
    /// the scheduled loop and every control request alike — or why there is
    /// none.
    ///
    /// One, not one per request: the resolver holds when it last walked the TUF
    /// repository, and that bound is what keeps a Sigstore outage costing one
    /// attempt a day instead of one per command (§10.2). And a process that
    /// could not build one refreshes nothing at all, which is a state `doctor`
    /// and `daemon status` have to be able to name.
    dns_resolver: std::sync::Mutex<crate::membership::ResolverSlot>,
    /// Rung when the unified tree may have changed — an accepted head flipped
    /// complete, a local publish landed, a checkout was added — so standing
    /// checkout loop materializes it without waiting out its interval (§7.2).
    checkout_wake: Arc<tokio::sync::Notify>,
    replica_wake: Arc<tokio::sync::Notify>,
    replica_rotation: Arc<std::sync::atomic::AtomicUsize>,
    /// Rung when a head lands in the pending slot: its trie has to be fetched
    /// and only an anti-entropy round does that.
    pending_wake: Arc<tokio::sync::Notify>,
    /// Serializes checkout passes, whether the standing loop or `synch replica
    /// sync` asked: two passes over one root would plan against each other's
    /// half-written state.
    checkout_lock: tokio::sync::Mutex<()>,
    /// Serializes socket tree-write commits (`docs/TREE-WRITES.md` §5.3): a
    /// conditional commit's check and the staging that follows it must not
    /// interleave with another socket writer's commit of the same path. The
    /// scanner does not take it — a concurrent local edit races a socket
    /// commit exactly as it races an S3 `PUT`, and that race is documented
    /// rather than closed.
    tree_write_lock: tokio::sync::Mutex<()>,
    /// Rung when a space is added or removed, so the watcher re-registers
    /// without waiting for the next filesystem hint (§7.1).
    spaces_changed: Arc<tokio::sync::Notify>,
    /// What the cloud-attach task has achieved per endpoint of per
    /// membership domain.
    ///
    /// In memory and nowhere else: a stored table of live connections is a
    /// lie the moment the process dies, and `synch control-plane status` is asking
    /// what this daemon is doing now.
    cloud: std::sync::Mutex<
        std::collections::HashMap<crate::cloud::CloudKey, crate::cloud::CloudDomainStatus>,
    >,
}

/// What checkout passes believe about the file at one target, and the stat
/// that belief is anchored to.
///
/// See [`Node::note_checkout_write`] for why a pass remembers anything at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckoutWrite {
    /// The content root the file is believed to be.
    pub(crate) content: Hash,
    /// The file's length when recorded.
    pub(crate) size: u64,
    /// The file's stored mtime when recorded.
    pub(crate) mtime_ns: i64,
    /// The file's platform identity when recorded (dev+inode on unix).
    pub(crate) file_id: Option<Vec<u8>>,
    /// When the record was taken: the racy-window anchor.
    pub(crate) recorded_at: i64,
}

impl CheckoutWrite {
    /// A record for the file now at `target`, believed to be `content`: the
    /// stat the belief is anchored to, taken just after the write or hash
    /// that established it. `None` if the file is already gone, in which case
    /// there is nothing to anchor.
    pub(crate) fn of(target: &Path, content: Hash) -> Option<CheckoutWrite> {
        let stat = std::fs::metadata(target).ok().filter(|m| m.is_file())?;
        Some(CheckoutWrite {
            content,
            size: stat.len(),
            mtime_ns: crate::scanner::mtime_nanos(&stat),
            file_id: crate::scanner::file_identity(&stat),
            recorded_at: synch_core::now_ns(),
        })
    }
}

/// A content root discovery could not resolve, and how long to leave it alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProviderMiss {
    /// When this root may be asked about again, in unix nanoseconds.
    pub(crate) until: i64,
    /// How many discovery rounds have come back with nothing, which is what
    /// the backoff doubles on.
    pub(crate) misses: u32,
}

/// How long a root nobody could name a provider for is left alone before the
/// next attempt, doubling per failure up to [`PROVIDER_MISS_MAX_BACKOFF`].
pub(crate) const PROVIDER_MISS_BACKOFF: std::time::Duration = std::time::Duration::from_secs(60);

/// The ceiling on that backoff. Bounded rather than permanent: a root nobody
/// holds today may be published tomorrow, and a negative cache that never
/// lets go is a worse fault than the polling it replaced.
pub(crate) const PROVIDER_MISS_MAX_BACKOFF: std::time::Duration =
    std::time::Duration::from_secs(3600);

/// How many roots the negative cache remembers.
///
/// It is fed by what other origins publish, so it needs a bound of its own; the
/// entry nearest to expiring is the one dropped, since it is the one whose
/// suppression is worth least.
pub(crate) const MAX_PROVIDER_MISSES: usize = 4096;

/// What `init` created.
#[derive(Debug, Clone)]
pub struct InitReport {
    /// The node's identity, when the device key settles it — `None` for a node
    /// whose zone has yet to name it (§3.1).
    pub origin: Option<OriginId>,
    /// The generated device key.
    pub node_id: NodeId,
    /// The membership domain that will name this node, if any.
    pub domain: Option<String>,
    /// The data directory.
    pub data_dir: PathBuf,
}

impl Node {
    /// Creates a device key and database in `data_dir`.
    ///
    /// With no `--domain`, the device key is the identity (§3.1) —
    /// self-certifying but not rotatable, and settled here. With one, the zone
    /// names this node: nothing is settled yet, the origin is left unset, and
    /// the daemon resolves it at startup once a record binds this key.
    pub fn init(data_dir: impl AsRef<Path>, domain: Option<&str>) -> Result<InitReport> {
        let data_dir = data_dir.as_ref().to_path_buf();
        let store = Store::open(&data_dir)?;
        if store.self_origin()?.is_some() || store.membership_domain()?.is_some() {
            return Err(EngineError::invalid(format!(
                "{} is already initialized",
                data_dir.display()
            )));
        }
        let domain = domain
            .map(synch_core::origin::normalize_domain)
            .transpose()
            .map_err(|e| EngineError::invalid(e.to_string()))?;
        let secret = SecretKey::generate();
        let node_id = secret.public();
        store.add_device_key(&secret, KeyState::Active, now_ns())?;
        let origin = match &domain {
            Some(domain) => {
                store.set_membership_domain(Some(domain))?;
                None
            }
            None => {
                let origin = OriginId::Key(node_id);
                store.set_self_origin(&origin)?;
                // A node always holds its own origin: without this binding it
                // could not verify its own heads after a restart. A node whose
                // name is still to come gets this when it adopts one.
                store.put_binding(&self_binding(&origin, node_id, now_ns()))?;
                Some(origin)
            }
        };
        Ok(InitReport {
            origin,
            node_id,
            domain,
            data_dir,
        })
    }

    /// Test support: initializes a node already named by its zone.
    ///
    /// Production has one path to a name — the zone answers, and
    /// [`settle_identity`](Self::settle_identity) adopts what it says (§3.1).
    /// A test that needs a named node without standing up a signed zone gets
    /// the same end state through the same migration, as though that answer had
    /// arrived. There is deliberately no non-test way to name a node by hand.
    #[doc(hidden)]
    pub fn init_named_by_zone(data_dir: impl AsRef<Path>, origin: OriginId) -> Result<InitReport> {
        let domain = origin
            .domain()
            .ok_or_else(|| EngineError::invalid("a zone names a Named origin"))?
            .to_string();
        let report = Self::init(&data_dir, Some(&domain))?;
        let store = Store::open(data_dir.as_ref())?;
        Self::migrate_identity(&store, None, &origin, report.node_id, &domain)?;
        Ok(InitReport {
            origin: Some(origin),
            ..report
        })
    }

    /// Adopts `origin` as this node's name, migrating everything keyed by the
    /// old one (§3.1).
    ///
    /// One transaction, as §10 requires of every multi-step state change. As
    /// separate autocommit writes, a crash after the head slots are cleared but
    /// before the views are would leave `entries` rows for an origin with no
    /// head in either slot — and nothing removes those: `rebuild_views`
    /// iterates the complete slots, so an origin with neither is never visited.
    /// The unified tree reads `entries` regardless of heads, so every path
    /// would stay duplicated under both names, in every checkout, permanently.
    ///
    /// Blobs stay: they are content-addressed, and the next scan republishes
    /// them under the new name. `head_history` stays too, so heads signed under
    /// the old name survive as the fork evidence §4.4 makes of them.
    fn migrate_identity(
        store: &Store,
        previous: Option<&OriginId>,
        adopted: &OriginId,
        node_id: NodeId,
        domain: &str,
    ) -> Result<()> {
        let previous = previous.cloned();
        let adopted = adopted.clone();
        let domain = domain.to_string();
        let now = now_ns();
        store.transaction(|txn| -> Result<()> {
            txn.set_self_origin(&adopted)?;
            txn.put_binding(&self_binding(&adopted, node_id, now))?;
            // A first name is an adoption too, and the one an operator most
            // often wants to see dated.
            txn.record_identity_adoption(previous.as_ref(), &adopted, &node_id, &domain, now)?;
            // The zone being left has no authority behind its bindings any
            // more; nothing else would drop them before their own expiry.
            txn.delete_dns_bindings_other_than(&domain)?;
            if let Some(previous) = &previous {
                txn.remove_binding(previous, &node_id, BindingSource::Static)?;
                // A rename revokes what the old name vouched for (§3.5). The
                // cascade already stops honoring these the moment the previous
                // origin's own binding goes, one line above — so the choice
                // here is not whether trust ends but whether the rows that
                // recorded it are left behind, pointing at an origin that no
                // longer exists and that nothing will ever publish a delta for.
                //
                // Said out loud, because it is the one consequence of a rename
                // an operator does not go looking for: the files come back
                // under the new name and the delegates simply stop.
                let revoked = txn.delete_delegations_by(previous)?;
                if !revoked.is_empty() {
                    tracing::warn!(
                        previous = %previous,
                        adopted = %adopted,
                        count = revoked.len(),
                        subjects = revoked
                            .iter()
                            .map(|k| k.fmt_short().to_string())
                            .collect::<Vec<_>>()
                            .join(", "),
                        "adopting a new name revoked the delegations the old one had issued; \
                         re-issue them with `synch delegate add` if they are still wanted"
                    );
                }
                // Drop the old name's view so the unified tree does not keep a
                // second copy of every path under it.
                txn.clear_head(previous, Slot::Complete)?;
                txn.clear_head(previous, Slot::Pending)?;
                txn.delete_origin_entries(previous)?;
                txn.delete_origin_providers(previous)?;
                txn.clear_observed_head(previous)?;
                // The floor was a promise about seqs under the old name; the
                // new one has no history for it to bound (§3.4).
                txn.clear_publish_floor()?;
            }
            Ok(())
        })?;
        Ok(())
    }

    /// Settles what this node is called, before anything is bound (§3.1).
    ///
    /// With no membership domain the device key is the identity and there is
    /// nothing to ask. With one, the zone is asked exactly once and the answer
    /// is frozen for the lifetime of the process — which is what lets a changed
    /// name be adopted here, with no daemon to stop.
    ///
    /// The two ways to get no name apart are the whole point of the shape:
    /// a validated answer that does not name this key is a *withdrawal* and
    /// leaves an already-named node alone, while no validated answer at all
    /// says nothing about anything and must not cost a node its name either.
    /// Only a node with no usable name is left [`Unidentified`], and a name
    /// issued by a zone this node no longer resolves is not a usable one.
    ///
    /// [`Unidentified`]: EngineError::Unidentified
    async fn settle_identity(
        store: &Arc<Store>,
        node_id: NodeId,
        resolver: Option<&dyn synch_net::MemberResolver>,
    ) -> Result<OriginId> {
        // Store reads go to the blocking pool, and the resolution between them
        // does not (§10) — so the two are separate offloads with the await in
        // the middle rather than one closure holding a connection across it.
        let read = {
            let store = store.clone();
            crate::blocking::offload(move || Ok((store.membership_domain()?, store.self_origin()?)))
                .await?
        };
        let (domain, stored) = read;

        let Some(domain) = domain else {
            // No zone: the key is the name, and adopting it is a migration
            // like any other when the node was called something else.
            let adopted = OriginId::Key(node_id);
            if stored.as_ref() != Some(&adopted) {
                let store = store.clone();
                let adopted = adopted.clone();
                crate::blocking::offload(move || {
                    Self::migrate_identity(&store, stored.as_ref(), &adopted, node_id, "")
                })
                .await?;
            }
            return Ok(adopted);
        };

        // A name from a zone that is no longer this node's is not a fallback:
        // nothing currently names it. `stored` is kept alongside, because what
        // this node holds is what a migration has to clean up.
        let usable = stored
            .clone()
            .filter(|p| p.domain() == Some(domain.as_str()));

        // `answered` is what separates a delegate from a broken start. A zone
        // that replies and does not name this key has *said* this node is not
        // one of its members, which is exactly what a delegated node is (§3.5):
        // it belongs to the cluster, takes its name from no zone in it, and
        // still resolves that zone for everyone else's bindings. A zone that
        // could not be asked has said nothing at all, and starting unnamed on
        // the strength of a DNS failure would trade this node's published
        // identity for a key origin on a transient error.
        //
        // Both used to arrive below as `None`, so the first left the daemon
        // waiting on the reduced socket for a record that was never coming.
        let (resolved, answered) = match resolver {
            Some(resolver) => match resolver.resolve_members(&domain).await {
                Ok((set, _ttl)) => {
                    let found = set.self_origin(&node_id).filter(|found| {
                        // A record with no `id=` binds `Key(nk)`. Taking that
                        // would trade a rotatable identity for a fixed one on
                        // the strength of a missing field (§3.1).
                        let named = found.domain().is_some();
                        if !named {
                            tracing::warn!(
                                domain,
                                key = %node_id.to_z32(),
                                "this node's key is published without an id=; not adopting it"
                            );
                        }
                        named
                    });
                    (found, true)
                }
                Err(e) => {
                    tracing::warn!(domain, error = %e, "could not resolve the membership domain");
                    (None, false)
                }
            },
            // No resolver is configured only where no domain is, and that case
            // returned above — so reaching here is a resolver that could not be
            // built, which is not an answer either.
            None => (None, false),
        };

        // Named by no zone, the zone said so, and the operator said to expect
        // that: a delegate. It keeps the key that identifies it, and
        // `resolving_domain` goes on returning the configured domain so its
        // cluster's members are still resolved.
        //
        // The operator's word is what makes this safe. On a first start a
        // delegate and a member whose record has not propagated give DNS the
        // same answer, so inferring it would either leave every delegate
        // waiting for a record that is never published, or let a member publish
        // under a key origin on a propagation lag — and then migrate away from it, leaving that origin in every
        // peer's view until it is swept.
        let expects_name = {
            let store = store.clone();
            crate::blocking::offload(move || Ok(store.membership_expects_name()?)).await?
        };
        if resolved.is_none() && answered && usable.is_none() && !expects_name {
            tracing::info!(
                domain,
                key = %node_id.to_z32(),
                "this zone does not name this node; running key-identified, and still \
                 resolving that zone for its members"
            );
            let adopted = OriginId::Key(node_id);
            if stored.as_ref() != Some(&adopted) {
                let store = store.clone();
                let adopted = adopted.clone();
                let domain = domain.clone();
                crate::blocking::offload(move || {
                    Self::migrate_identity(&store, stored.as_ref(), &adopted, node_id, &domain)
                })
                .await?;
            }
            return Ok(adopted);
        }

        match resolved {
            Some(adopted) => {
                // `usable` decides whether an old name may be *kept*; what is
                // migrated away from is whatever this node actually holds. A
                // name from a replaced zone — and a key identity, which names
                // no domain at all — is exactly the state that has to be
                // cleaned up, so filtering here would skip the migration in
                // the two cases that need it most.
                if stored.as_ref() != Some(&adopted) {
                    let store = store.clone();
                    let adopted = adopted.clone();
                    let domain = domain.clone();
                    crate::blocking::offload(move || {
                        Self::migrate_identity(&store, stored.as_ref(), &adopted, node_id, &domain)
                    })
                    .await?;
                }
                Ok(adopted)
            }
            None => usable.ok_or(EngineError::Unidentified {
                domain,
                node_id: Box::new(node_id),
            }),
        }
    }

    /// Opens an initialized data directory and binds the endpoint.
    pub async fn open(mut config: NodeConfig) -> Result<Node> {
        if !(1..=crate::MAX_REPLICA_CONCURRENCY).contains(&config.replica_concurrency) {
            return Err(EngineError::invalid(format!(
                "replica_concurrency must be between 1 and {}, got {}",
                crate::MAX_REPLICA_CONCURRENCY,
                config.replica_concurrency
            )));
        }
        let cloud_cas = config
            .cloud
            .as_ref()
            .map(synch_store::cloud::CloudStore::open)
            .transpose()?;
        // Opening runs migrations and hardens file permissions, both of which
        // are filesystem work, so it goes to the blocking pool like everything
        // else that touches the disk.
        let data_dir = config.data_dir.clone();
        let desired_backend = config
            .cloud
            .as_ref()
            .map(|cloud| cloud.service.as_str())
            .unwrap_or("local")
            .to_string();
        let cloud_settings = config
            .cloud
            .as_ref()
            .map(persisted_cloud_settings)
            .unwrap_or_default();
        let cloud_namespace: Vec<(String, Option<String>)> = cloud_settings
            .iter()
            .filter(|(key, _)| !key.ends_with(".cache_bytes") && !key.ends_with(".upload"))
            .cloned()
            .collect();
        let store_options = synch_store::StoreOptions {
            checkpointing: config.checkpointing,
        };
        let opened = crate::blocking::offload(move || {
            let store = Store::open_with(&data_dir, store_options)?;
            if desired_backend != "local" {
                let path_spaces: Vec<String> = store
                    .sources()?
                    .into_iter()
                    .filter(|space| space.local_path.is_some())
                    .map(|space| space.space)
                    .collect();
                if !path_spaces.is_empty() {
                    return Err(EngineError::invalid(format!(
                        "cloud CAS requires API sources; filesystem source(s): {}",
                        path_spaces.join(", ")
                    )));
                }
            }
            match store.config("cas.backend")? {
                Some(stored) if stored != desired_backend => {
                    return Err(EngineError::invalid(format!(
                        "this node uses the {stored} CAS backend, not {desired_backend}; run \
                         `synch cas migrate --to {desired_backend}` instead of flipping the flag"
                    )))
                }
                Some(_) => {}
                None if desired_backend != "local" && !store.blob_candidates()?.is_empty() => {
                    return Err(EngineError::invalid(format!(
                        "this node already has local CAS content; migrate it before selecting \
                         the {desired_backend} backend"
                    )))
                }
                None => store.set_config("cas.backend", &desired_backend)?,
            }
            let mut namespace_was_stored = false;
            for (key, _) in &cloud_namespace {
                namespace_was_stored |= store.config(key)?.is_some();
            }
            if namespace_was_stored {
                for (key, desired) in &cloud_namespace {
                    if store.config(key)? != *desired {
                        return Err(EngineError::invalid(format!(
                            "cloud CAS setting {key} changed; run `synch cas migrate --to \
                             {desired_backend}` instead of pointing this node at another namespace"
                        )));
                    }
                }
            }
            for (key, value) in cloud_settings {
                match value {
                    Some(value) => store.set_config(&key, &value)?,
                    None => store.clear_config(&key)?,
                }
            }
            // No device key at all is an uninitialized directory; keys but
            // none active is a rotation that got halfway (§3.4), and the two
            // want different words.
            let secret = match store.active_device_key()? {
                Some(key) => key.secret,
                None if store.device_keys()?.is_empty() => return Err(EngineError::NotInitialized),
                None => return Err(EngineError::NoActiveKey),
            };
            Ok((Arc::new(store), secret))
        })
        .await?;
        let (store, secret) = opened;
        store.set_remote_cas(config.cloud.is_some());
        let legacy_pin_state = config.data_dir.join("rekor-pins.json");
        let pin_store = store.clone();
        crate::blocking::offload(move || {
            if legacy_pin_state.exists() {
                if pin_store.config("rekor.pin_state")?.is_none() {
                    let text = std::fs::read_to_string(&legacy_pin_state)?;
                    pin_store.set_config("rekor.pin_state", &text)?;
                }
                // The database is authoritative after the row commits. Keeping
                // a second writable copy would let the two monotonic floors
                // drift and makes the next cold restore choose ambiguously.
                std::fs::remove_file(&legacy_pin_state)?;
            }
            Ok(())
        })
        .await?;
        config.dns.rekor_state = None;
        config.dns.rekor_config = Some(store.clone());
        // Before the endpoint, before any loop: what this node is called, and
        // the migration if the zone has changed its mind (§3.1).
        //
        // The resolver is built only for a node that has a zone to ask, and a
        // failure to build one is raised rather than swallowed. Swallowing it
        // makes a mistyped `--dnssec-anchor` indistinguishable from a zone
        // that has not published a record yet, and the daemon then waits
        // forever telling the operator to publish a record they already
        // published. It is also the same stance the standing refresh takes
        // (`build_resolver`): options that yield no resolver are a refusal to
        // start, not a degraded mode.
        let resolver = {
            let dns = config.dns.clone();
            let store = store.clone();
            crate::blocking::offload(move || match store.membership_domain()? {
                None => Ok(None),
                Some(_) => Ok(Some(synch_net::DnssecResolver::with_options(&dns)?)),
            })
            .await?
        };
        let origin = Self::settle_identity(
            &store,
            secret.public(),
            resolver
                .as_ref()
                .map(|r| r as &dyn synch_net::MemberResolver),
        )
        .await?;
        // Every endpoint this node ever binds — this one, and the second one a
        // key activation brings up — rings the same bell, because the refusal
        // that matters can arrive at either (§3.4).
        let dns_wake = Arc::new(tokio::sync::Notify::new());
        config.net.on_unknown_key = Some(dns_wake.clone());
        // Every head that flips to complete — dialed out or pushed in — rings
        // the checkout bell. One syncer does both: it is handed to the endpoint
        // as the head sink the serve side reconciles through, and it is the
        // same object this node's own rounds dial with.
        let checkout_wake = Arc::new(tokio::sync::Notify::new());
        let replica_wake = Arc::new(tokio::sync::Notify::new());
        // And every head adopted as *pending* rings the anti-entropy loop: its
        // trie is not here, and until somebody dials for it the head is a
        // pointer no reading surface follows (§5.3).
        let pending_wake = Arc::new(tokio::sync::Notify::new());
        let syncer = Syncer::new(store.clone())
            .on_change(checkout_wake.clone())
            .on_replica(replica_wake.clone())
            .on_pending(pending_wake.clone());
        config.net.heads = Some(Arc::new(syncer.clone()) as Arc<dyn synch_net::HeadSink>);
        let cas: Arc<dyn synch_store::backend::CasBackend> = match (cloud_cas, &config.cloud) {
            (Some(objects), Some(cloud)) => Arc::new(
                synch_store::backend::Cloud::open(
                    store.clone(),
                    objects,
                    cloud.upload_policy,
                    cloud.cache_bytes,
                )
                .await?,
            ),
            _ => Arc::new(synch_store::backend::LocalFs::new(store.clone())),
        };
        config.net.cas = Some(cas.clone());
        // Mounted only where there is a runtime to serve it: a peer's dial then
        // fails at ALPN negotiation rather than after a handshake and a
        // refusal, and a build that cannot run a program never advertises that
        // it can (`docs/SOCKETS.md` §5.1).
        // Zero workers is the same statement made by the host rather than by
        // the build, and it is answered here rather than inside the pool so
        // that declining costs nothing at all: no threads, and no SSH host key
        // minted — generating one is a durable write, and a node that will
        // never answer an SSH socket has no business having one
        // (`config::NodeConfig`).
        let socket_workers = config.socket_workers;
        #[cfg(all(
            any(target_os = "linux", target_os = "macos", target_os = "openbsd"),
            any(target_arch = "x86_64", target_arch = "aarch64")
        ))]
        let socket_pool = if socket_workers == 0 {
            None
        } else {
            const SSH_HOST_KEY_CONFIG: &str = "ssh.host_key.ed25519";
            let key_store = store.clone();
            let host_key =
                crate::blocking::offload(move || match key_store.config(SSH_HOST_KEY_CONFIG)? {
                    Some(encoded) => Ok(synch_sock::SshHostKey::from_openssh(&encoded)
                        .map_err(|error| EngineError::invalid(error.to_string()))?),
                    None => {
                        let key = synch_sock::SshHostKey::generate()
                            .map_err(|error| EngineError::invalid(error.to_string()))?;
                        let encoded = key
                            .to_openssh()
                            .map_err(|error| EngineError::invalid(error.to_string()))?;
                        key_store.set_config(SSH_HOST_KEY_CONFIG, &encoded)?;
                        Ok(key)
                    }
                })
                .await?;
            tracing::info!(
                fingerprint = %host_key.fingerprint(),
                "SSH socket host key ready"
            );
            crate::sockets::SocketPool::start_with_ssh_host_key(
                socket_workers,
                crate::sockets::default_limits(),
                host_key,
            )
        };
        #[cfg(not(all(
            any(target_os = "linux", target_os = "macos", target_os = "openbsd"),
            any(target_arch = "x86_64", target_arch = "aarch64")
        )))]
        let socket_pool = if socket_workers == 0 {
            None
        } else {
            crate::sockets::SocketPool::start(socket_workers, crate::sockets::default_limits())
        };
        let dispatch = crate::sockets::SocketDispatch::new();
        if socket_pool.is_some() {
            config.net.sockets = Some(Arc::new(dispatch.clone()));
        }

        // A batch that was still buffered when the process died was never
        // published, and the scanner would skip those files forever (§7.1).
        // Do this before binding the endpoint: after `Net::bind` returns there
        // must be no await between a live endpoint and the Node that owns it.
        let reindexed = {
            let store = store.clone();
            let origin = origin.clone();
            crate::blocking::offload(move || {
                crate::scanner::reconcile_local_files_in(&store, &origin)
            })
            .await?
        };
        if reindexed > 0 {
            tracing::info!(
                paths = reindexed,
                "re-indexing paths whose staged changes never reached a root"
            );
        }
        let net = Net::bind(store.clone(), secret.clone(), config.net.clone()).await?;
        let publisher = Publisher::new(config.publish_quiesce, config.publish_batch_max);
        let node = Node {
            inner: Arc::new(NodeInner {
                store,
                cas,
                net: std::sync::RwLock::new(net),
                retiring: std::sync::Mutex::new(Vec::new()),
                syncer,
                origin,
                secret: std::sync::RwLock::new(secret),
                config,
                publisher,
                ad_clock: std::sync::Mutex::new(Default::default()),
                provider_misses: std::sync::Mutex::new(Default::default()),
                checkout_writes: std::sync::Mutex::new(Default::default()),
                program_bytes: crate::sockets::ProgramBytesCache::new(),
                sockets: socket_pool,
                socket_authorization: std::sync::RwLock::new(()),
                dns: std::sync::Mutex::new(Default::default()),
                dns_resolver: std::sync::Mutex::new(Default::default()),
                dns_wake,
                checkout_wake,
                replica_wake,
                replica_rotation: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                pending_wake,
                checkout_lock: tokio::sync::Mutex::new(()),
                tree_write_lock: tokio::sync::Mutex::new(()),
                spaces_changed: Arc::new(tokio::sync::Notify::new()),
                cloud: std::sync::Mutex::new(Default::default()),
            }),
        };
        // The handler was mounted on the endpoint before the node existed;
        // this is where it learns what it dispatches to. Done before anything
        // else that can await, so the window in which a connection finds it
        // unbound is as short as construction allows.
        dispatch.bind(&node);
        Ok(node)
    }

    /// A non-owning handle onto this node.
    ///
    /// What the socket dispatcher holds. It cannot hold a `Node`: the node owns
    /// the endpoint, the endpoint's router owns the socket protocol handler,
    /// and the handler owns the dispatcher — so a strong reference there is a
    /// cycle, and every node ever opened would stay alive with its database
    /// open. That is not a leak you notice until something reopens the same
    /// data directory.
    pub(crate) fn downgrade(&self) -> WeakNode {
        WeakNode(Arc::downgrade(&self.inner))
    }

    /// The metadata and content store.
    pub fn store(&self) -> &Arc<Store> {
        &self.inner.store
    }

    /// The configured async CAS backend.
    pub fn cas_backend(&self) -> &Arc<dyn synch_store::backend::CasBackend> {
        &self.inner.cas
    }

    /// The socket worker pool, or `None` where this build serves no sockets.
    pub(crate) fn socket_workers(&self) -> Option<&crate::sockets::SocketPool> {
        self.inner.sockets.as_ref()
    }

    /// Whether the socket pool is at its daemon-wide bound (`docs/SOCKETS.md`
    /// §10).
    pub(crate) fn socket_pool_full(&self) -> bool {
        self.inner.sockets.as_ref().is_some_and(|pool| pool.full())
    }

    /// Claims a place in the socket-program cache for `root`: the cached
    /// bytes if they are already loaded, a seat on an in-flight load if one
    /// is under way, or the loader role if this admission reads the CAS.
    pub(crate) fn socket_program_load(&self, root: &Hash) -> crate::sockets::ProgramLoad {
        self.inner.program_bytes.begin_load(root)
    }

    /// The limits every socket invocation on this node runs under.
    pub(crate) fn socket_limits(&self) -> synch_sock::Limits {
        match &self.inner.sockets {
            Some(pool) => pool.limits(),
            None => crate::sockets::default_limits(),
        }
    }

    /// The next invocation id, as `synch socket ps` prints it.
    pub(crate) fn next_socket_id(&self) -> u64 {
        self.inner
            .sockets
            .as_ref()
            .map(|p| p.next_id())
            .unwrap_or(0)
    }

    /// Takes a concurrency slot for one invocation, or reports the socket full.
    #[allow(
        clippy::too_many_arguments,
        reason = "a pass-through to the registry, whose arguments are the facts                   a live-invocation entry is made of"
    )]
    pub(crate) fn reserve_socket_slot(
        &self,
        id: u64,
        socket: &str,
        peer: &str,
        peer_key: synch_core::NodeId,
        program: synch_core::Hash,
        max_streams: usize,
    ) -> Option<synch_sock::SlotGuard> {
        self.inner
            .sockets
            .as_ref()?
            .reserve(id, socket, peer, peer_key, program, max_streams)
    }

    /// Drops everything one socket's map held.
    pub(crate) fn clear_socket_map(&self, socket: &str) {
        if let Some(pool) = &self.inner.sockets {
            pool.clear_map(socket);
        }
    }

    /// The batch between staging and one signed root (§7.1).
    pub fn publisher(&self) -> &Publisher {
        &self.inner.publisher
    }

    /// The endpoint under the active device key.
    ///
    /// Returned by value because a rotation can replace it (§3.4): holding a
    /// borrow across an `activate` would pin the old endpoint.
    pub fn net(&self) -> Net {
        self.inner
            .net
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The addresses of the endpoints still serving for keys that are being
    /// retired (§3.4).
    pub fn retiring_endpoints(&self) -> Vec<EndpointAddr> {
        self.retiring_nets().iter().map(Net::addr).collect()
    }

    /// The endpoints still serving for keys that are being retired (§3.4).
    pub(crate) fn retiring_nets(&self) -> Vec<Net> {
        self.inner
            .retiring
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The active signing key.
    pub(crate) fn secret(&self) -> SecretKey {
        self.inner
            .secret
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Replaces the active key and its endpoint, demoting the previous
    /// endpoint to a serving-only one for the overlap window (§3.4).
    pub(crate) fn swap_active_endpoint(&self, secret: SecretKey, net: Net) {
        let previous = {
            let mut slot = self
                .inner
                .net
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::replace(&mut *slot, net)
        };
        *self
            .inner
            .secret
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = secret;
        self.inner
            .retiring
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(previous);
    }

    /// Removes a retiring endpoint, returning it so the caller can shut it
    /// down outside the lock.
    pub(crate) fn take_retiring_endpoint(&self, node_id: &NodeId) -> Option<Net> {
        let mut retiring = self
            .inner
            .retiring
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let index = retiring.iter().position(|net| &net.id() == node_id)?;
        Some(retiring.remove(index))
    }

    /// The reconciler.
    pub fn syncer(&self) -> &Syncer {
        &self.inner.syncer
    }

    /// This node's stable identity.
    pub fn origin(&self) -> &OriginId {
        &self.inner.origin
    }

    /// This node's active device key.
    pub fn node_id(&self) -> NodeId {
        self.secret().public()
    }

    /// The configuration this node was opened with.
    pub fn config(&self) -> &NodeConfig {
        &self.inner.config
    }

    /// Serializes socket tree-write commits (`docs/TREE-WRITES.md` §5.3).
    pub(crate) fn tree_write_lock(&self) -> &tokio::sync::Mutex<()> {
        &self.inner.tree_write_lock
    }

    /// Holds socket authorization stable while an admission becomes live.
    pub(crate) fn socket_authorization_read(&self) -> std::sync::RwLockReadGuard<'_, ()> {
        self.inner
            .socket_authorization
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Excludes final admission while local authorization changes.
    pub(crate) fn socket_authorization_write(&self) -> std::sync::RwLockWriteGuard<'_, ()> {
        self.inner
            .socket_authorization
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Shuts every endpoint this node holds down cleanly.
    pub async fn shutdown(&self) -> Result<()> {
        let retiring = self.retiring_nets();
        let active = self.net();

        // First close the admission gate on every key this node still serves.
        // Existing connections remain alive so their workers can report a
        // clean `Closed{Shutdown}` on the control stream.
        for net in retiring.iter().chain(std::iter::once(&active)) {
            net.stop_socket_admission();
        }
        if let Some(pool) = &self.inner.sockets {
            pool.shutdown().await;
        }
        // Worker replies are delivered by protocol tasks outside the pool.
        // Do not tear their endpoint out from underneath the final frame.
        for net in retiring.iter().chain(std::iter::once(&active)) {
            net.drain_socket_streams().await;
        }

        for net in retiring {
            if let Err(e) = net.shutdown().await {
                tracing::warn!(error = %e, "a retiring endpoint did not shut down cleanly");
            }
        }
        let network = active.shutdown().await;
        network?;
        Ok(())
    }

    // ---- membership -------------------------------------------------------

    /// Trusts a device key (§3.2).
    ///
    /// The key is the identity. A name belongs to the zone that issues it, so
    /// there is no way to attach one here: a hand-made binding never expires,
    /// and one carrying a name would outlive the record it shadowed — dropping
    /// a member from the zone would stop being how a member is dropped.
    pub fn trust_add(&self, node_id: NodeId, note: Option<&str>) -> Result<OriginId> {
        let origin = OriginId::Key(node_id);
        self.store().put_binding(&Binding {
            origin: origin.clone(),
            node_id,
            source: BindingSource::Static,
            domain: None,
            issuer: None,
            spaces: Vec::new(),
            note: note.map(str::to_string),
            added_at: now_ns(),
            expires_at: None,
        })?;
        Ok(origin)
    }

    /// Delegates a device key into the cluster, confined to `spaces` (§3.5).
    ///
    /// Returns the staged `d:` record for the caller to publish. The
    /// delegation is not a credential and nothing is handed to the subject:
    /// it becomes real when this record reaches the members, which the
    /// ordinary reactive push does within a round.
    ///
    /// Refused unless this node is itself rooted — static or DNS. That is the
    /// one-level rule seen from the issuing side, and it is courtesy rather
    /// than enforcement: no other node reads a delegation from an origin
    /// without a live rooted binding, so a delegate publishing one would
    /// merely be ignored. Failing here says so at the point an operator can
    /// still do something about it.
    pub fn delegate_add(
        &self,
        subject: NodeId,
        spaces: &[String],
        not_after: i64,
        note: Option<&str>,
    ) -> Result<StagedChange> {
        if subject == self.node_id() {
            return Err(EngineError::invalid(
                "a node cannot delegate to its own device key",
            ));
        }
        if self.is_delegated()? {
            return Err(EngineError::invalid(
                "this node is itself delegated, and a delegation may not be delegated onward; \
                 no member would read the record",
            ));
        }
        // `*` is a legal space id — nothing in `validate_space` forbids it —
        // so a delegation naming it would quietly grant a space called `*`
        // rather than every space. A closed list is the only thing a
        // delegation can be, and a user reaching for a wildcard has to be told
        // that rather than handed an empty grant.
        if spaces.iter().any(|s| s == "*") {
            return Err(EngineError::invalid(
                "a delegation names spaces explicitly; there is no wildcard",
            ));
        }
        let delegation = Delegation {
            v: synch_core::RECORD_VERSION,
            spaces: spaces.to_vec(),
            not_after,
            note: note.map(str::to_string),
        };
        if !delegation.is_well_formed() {
            return Err(EngineError::invalid(format!(
                "a delegation names between 1 and {} distinct valid spaces",
                synch_core::MAX_DELEGATION_SPACES
            )));
        }
        if not_after <= now_ns() {
            return Err(EngineError::invalid(
                "a delegation must expire in the future",
            ));
        }
        let bytes = synch_core::record::encode(&delegation)?;
        Ok((synch_core::delegation_key(&subject), Some(bytes)))
    }

    /// Withdraws a delegation this node issued (§3.5).
    ///
    /// Revocation is deletion: the `d:` key vanishes from the next root, and
    /// tries being replicated whole means the diff surfaces the removal
    /// everywhere it reaches — including to a peer partitioned for years
    /// (§4.2). There is no revocation state to retain and nothing to expire.
    pub fn delegate_remove(&self, subject: &NodeId) -> Result<StagedChange> {
        let key = synch_core::delegation_key(subject);
        let root = self.current_root()?;
        let trie = Trie::new(self.store().as_ref());
        if trie.get(root, &key)?.is_none() {
            return Err(EngineError::NotFound(format!(
                "this node has not delegated {}",
                subject.fmt_short()
            )));
        }
        Ok((key, None))
    }

    /// Every delegation this node currently honors, whoever issued it.
    ///
    /// After replication every member holds all of them, which is the
    /// transitive-trust concession made legible: an operator can see from any
    /// node exactly who was admitted, by whom, and to what.
    pub fn delegations(&self) -> Result<Vec<Binding>> {
        Ok(self.store().all_delegations()?)
    }

    /// True if this node is itself in the cluster on a delegation (§3.5).
    ///
    /// Asked of the two facts a node can actually observe about itself: that
    /// some origin has delegated its device key, and that a peer has declared
    /// a read scope for it. A node's *own* binding says nothing — every node
    /// statically trusts itself at `init`, so "am I rooted?" reads true
    /// everywhere and would answer this question wrongly for exactly the nodes
    /// it is asked about.
    ///
    /// Courtesy, not enforcement. The rule that matters is the one every other
    /// node applies without asking: a `d:` record is read only from an origin
    /// holding a live *rooted* binding there, so a delegate's records are
    /// honored by nobody whatever it publishes. This is what makes the command
    /// say so instead of reporting a success that means nothing.
    pub(crate) fn is_delegated(&self) -> Result<bool> {
        // A live delegation of this node's own key is the authoritative
        // answer, because it came out of a signed trie. The adopted read scope
        // is only the bootstrap — what a delegate has before it has replicated
        // the record naming it — so it answers only when there is no record to
        // read, and stops answering the moment one says otherwise.
        let own = self.node_id();
        if self
            .store()
            .delegations(now_ns())?
            .into_iter()
            .any(|b| b.node_id == own)
        {
            return Ok(true);
        }
        Ok(self.store().has_delegations()? && self.store().local_scope()?.is_some())
    }

    /// Records a peer's address so it can be dialed later.
    pub fn remember_peer(&self, addr: &EndpointAddr) -> Result<()> {
        let encoded = encode_addr(addr);
        self.store()
            .record_peer_seen(&addr.id, Some(&encoded), now_ns())?;
        Ok(())
    }

    /// The address recorded for a peer, if any.
    pub fn peer_addr(&self, node_id: &NodeId) -> Result<Option<EndpointAddr>> {
        Ok(self
            .store()
            .peers_seen()?
            .into_iter()
            .find(|p| &p.node_id == node_id)
            .and_then(|p| p.last_addr)
            .and_then(|bytes| decode_addr(*node_id, &bytes)))
    }

    // ---- sources ----------------------------------------------------------

    /// Registers a local directory as a space (§4.1).
    ///
    /// Source roots may not overlap a replica checkout, which is what makes the
    /// "no echo" guarantee structural rather than conventional (§7.2).
    pub fn add_filesystem_source(&self, id: &str, path: impl AsRef<Path>) -> Result<()> {
        if self.cas_backend().remote_upload_parts() {
            return Err(EngineError::invalid(
                "a cloud-CAS node may only add API sources; use `synch source add <id> --api`",
            ));
        }
        validate_space(id)?;
        let path = canonical_dir(path.as_ref())?;
        for replica in self.store().replicas()? {
            if let Some(checkout) = replica.checkout_path {
                if paths_overlap(&path, &stored_root(&checkout)) {
                    return Err(EngineError::invalid(format!(
                        "source root {} overlaps replica checkout {}",
                        path.display(),
                        checkout
                    )));
                }
            }
        }
        for space in self.store().sources()? {
            let Some(local_path) = space.local_path.as_deref() else {
                if space.space == id {
                    return Err(EngineError::invalid(format!(
                        "source {id} is API-only; remove it before attaching a local directory"
                    )));
                }
                continue;
            };
            if space.space != id && paths_overlap(&path, &stored_root(local_path)) {
                return Err(EngineError::invalid(format!(
                    "source root {} overlaps source {}",
                    path.display(),
                    space.space
                )));
            }
            // Re-pointing an existing space at a different directory is
            // refused, because what it actually does is publish a mass
            // deletion. `put_space` upserts `local_path` and clears neither
            // `local_files` nor `entries`, so the next scan walks the new root,
            // finds none of the old paths, and tombstones every one of them —
            // then every checkout following the view deletes its copy. The operator
            // asking for this usually means "re-sync this space from my peers",
            // which is the exact opposite.
            //
            // `scan_space_with_ingest` already refuses a root that is missing or is not a
            // directory, with the same reasoning in its comment; this is the
            // sibling case it does not test, where the root is present, is a
            // directory, and is simply somewhere else.
            if space.space == id {
                let current = stored_root(local_path);
                if current != path {
                    return Err(EngineError::invalid(format!(
                        "source {id} is already rooted at {}. Re-pointing it at {} would publish a \
                         deletion for every path under the old root: remove the source first if \
                         that is what you want, or move the directory into place instead",
                        current.display(),
                        path.display()
                    )));
                }
            }
        }
        self.store()
            .put_source(id, SourceKind::Filesystem, Some(&path.to_string_lossy()))?;
        self.spaces_changed();
        Ok(())
    }

    /// Registers an API-only publisher (`docs/SERVERLESS.md` §10).
    pub fn add_api_source(&self, id: &str) -> Result<()> {
        validate_space(id)?;
        if let Some(source) = self.store().source(id)? {
            return match source.local_path {
                None => Ok(()),
                Some(path) => Err(EngineError::invalid(format!(
                    "source {id} is already rooted at {path}; remove it before making it API-only"
                ))),
            };
        }
        self.store().put_source(id, SourceKind::Api, None)?;
        self.spaces_changed();
        Ok(())
    }

    /// Plans removal of a source's published entries.
    ///
    /// Staging the removal is half of a publish, so it takes the same recovery
    /// gate (§3.4): a node that cannot publish must not drop the source either,
    /// or the unpublish would be lost with it.
    pub fn source_removal(&self, id: &str) -> Result<Vec<StagedChange>> {
        // "removed ghost and unpublished 0 record(s)" for a space that never
        // existed is a lie with a friendly face.
        let Some(_source) = self.store().source(id)? else {
            return Err(EngineError::NotFound(format!("no source {id}")));
        };
        // A space this node only replicates has nothing of its own under the
        // prefix, so the scan below would stage nothing and the outcome would
        // be right by accident. That is not good enough for the one command
        // here that can publish a mass deletion: it takes the publishing gate,
        // and a node that cannot publish must not be stopped from giving up a
        // space it never published into (`docs/REPLICATION.md` §3.2).
        // Or has published: a record advertised under an earlier answer to that
        // predicate must still be retractable, or `source rm` leaves it behind.
        self.ensure_publishable()?;
        let mut staged = Vec::new();
        let root = self.current_root()?;
        let trie = Trie::new(self.store().as_ref());
        let prefix = synch_core::space_prefix(id)?;
        for (key, _) in trie.scan(root, &prefix, None, None)? {
            staged.push((key, None));
        }
        staged.push(self.space_info_removal(id)?);
        Ok(staged)
    }

    /// Commits local source-role removal after its trie removal has published.
    /// Keeping this second prevents a failed publish from leaving live own
    /// entries behind with no source role able to retract them.
    pub fn finish_source_removal(&self, id: &str) -> Result<()> {
        if !self.store().remove_source(id)? {
            return Err(EngineError::NotFound(format!("no source {id}")));
        }
        for path in self.store().local_files(id)? {
            self.store().remove_local_file(id, &path)?;
        }
        self.spaces_changed();
        Ok(())
    }

    /// Tells the watcher that the set of spaces changed (§7.1).
    pub(crate) fn spaces_changed(&self) {
        self.inner.spaces_changed.notify_waiters();
    }

    /// The bell the watcher waits on for space additions and removals.
    pub(crate) fn spaces_changed_signal(&self) -> Arc<tokio::sync::Notify> {
        self.inner.spaces_changed.clone()
    }

    /// The bell an unknown-key refusal rings, which the DNS refresh loop waits
    /// on (§3.4).
    pub(crate) fn dns_wake(&self) -> Arc<tokio::sync::Notify> {
        self.inner.dns_wake.clone()
    }

    /// The bell that wakes standing checkout reconciliation (§7.2).
    pub(crate) fn checkout_wake(&self) -> Arc<tokio::sync::Notify> {
        self.inner.checkout_wake.clone()
    }

    /// The bell a replication sweep waits on (`docs/REPLICATION.md` §3.4).
    ///
    /// Its own rather than shared with checkouts: the two react to the same
    /// events but at different costs, and a checkout pass over an unchanged tree
    /// must not drag a sweep of four million entries along with it.
    pub(crate) fn replica_wake(&self) -> Arc<tokio::sync::Notify> {
        self.inner.replica_wake.clone()
    }

    /// Which replica leads the next fetch batch (`docs/REPLICATION.md`
    /// §3.3). In memory only: fairness across a restart is not worth a write.
    pub(crate) fn replica_rotation(&self) -> Arc<std::sync::atomic::AtomicUsize> {
        self.inner.replica_rotation.clone()
    }

    /// The bell a head landing in the pending slot rings (§5.3).
    pub(crate) fn pending_wake(&self) -> Arc<tokio::sync::Notify> {
        self.inner.pending_wake.clone()
    }

    /// Serializes one materialization pass against every other on this node:
    /// a checkout pass (§7.2) and a `synch adopt tree` of a space (adopt_tree.rs) alike.
    pub(crate) async fn lock_materialization(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.inner.checkout_lock.lock().await
    }

    /// The resolver slot every membership refresh in this process reads from.
    pub(crate) fn dns_resolver_slot(
        &self,
    ) -> std::sync::MutexGuard<'_, crate::membership::ResolverSlot> {
        self.inner
            .dns_resolver
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// What the cloud-attach task has achieved per endpoint of per
    /// membership domain.
    pub(crate) fn cloud_slot(
        &self,
    ) -> std::sync::MutexGuard<
        '_,
        std::collections::HashMap<crate::cloud::CloudKey, crate::cloud::CloudDomainStatus>,
    > {
        self.inner
            .cloud
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The per-domain re-resolution schedule (§3.2).
    pub(crate) fn dns_schedule(
        &self,
    ) -> std::sync::MutexGuard<
        '_,
        std::collections::HashMap<String, crate::membership::DomainSchedule>,
    > {
        self.inner
            .dns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    // ---- publishing -------------------------------------------------------

    /// The root of this node's own current head.
    pub fn current_root(&self) -> Result<Hash> {
        Ok(self
            .store()
            .complete_head(self.origin())?
            .map(|h| h.root)
            .unwrap_or(Hash::EMPTY))
    }

    /// This node's own current signed head, if it has published anything.
    pub fn own_head(&self) -> Result<Option<SignedHead>> {
        Ok(self.store().complete_head(self.origin())?)
    }

    /// The seq this node's next publish will carry.
    ///
    /// Normally one past the current head, or 1 for a node that has never
    /// published. A node that came back from key loss also carries a durable
    /// publishing floor (§3.4), and the floor only ever raises the seq: it is
    /// what keeps a recovered node above the history its peers still hold,
    /// across restarts.
    pub fn next_seq(&self) -> Result<u64> {
        Ok(self.store().next_own_seq(self.origin())?)
    }

    /// Applies staged changes as one new signed root (§7.1).
    ///
    /// One save in an editor costs one head; a 100k-file initial index costs a
    /// handful, because the batch becomes a single root.
    ///
    /// Refuses while this node is in key-loss recovery (§3.4): the head it
    /// would mint carries a seq every peer rejects.
    /// One SQLite transaction, as §10 requires: "trie writes, head, history,
    /// and materialization commit together or not at all". Without that, a
    /// crash between any two of those steps leaves a head whose trie is
    /// half-written, or a head slot the derived views do not agree with.
    pub fn publish(&self, staged: &[StagedChange]) -> Result<Option<SignedHead>> {
        if staged.is_empty() {
            return Ok(None);
        }
        self.ensure_publishable()?;
        let secret = self.secret();
        let origin = self.origin().clone();
        let now = now_ns();

        let head = self
            .store()
            // LEAN-MODEL: cas-source-publish (Cas.SourcePublish)
            // `Safety.sourcePublish` pairs `Cas.SourcePublish` with the trie
            // transition below: durable check, pin, entry and head share commit.
            // LEAN-MODEL: mpt-own-publish (MptGc.OwnPublish)
            // `MptGc.OwnPublish` models the trie/head/materialized side of this
            // same transaction; it is complete because this node built it.
            .transaction(|txn| -> Result<Option<SignedHead>> {
                // Read the head we are about to displace inside the transaction:
                // the root we build on and the seq we build past have to come from
                // the same snapshot the flip is written against.
                let previous = txn.complete_head(&origin)?;
                let old_root = previous.as_ref().map(|h| h.root).unwrap_or(Hash::EMPTY);

                let trie = Trie::new(txn);
                let mut root = old_root;
                let mut changed_spaces = std::collections::HashSet::new();
                let mut source_ads = std::collections::HashMap::new();
                for (key, value) in staged {
                    if let Ok((space, _path)) = parse_file_key(key) {
                        changed_spaces.insert(space.to_string());
                        if let Some(bytes) = value {
                            let entry = crate::scanner::decode_entry(bytes)?;
                            if matches!(entry.kind, EntryKind::File | EntryKind::Socket) {
                                let content = entry.content.ok_or_else(|| {
                                    EngineError::invalid(format!(
                                        "live own entry in {space} has no content root"
                                    ))
                                })?;
                                txn.hold_source_blob(&space, &content, entry.size, now)?;
                                source_ads.insert(content, entry.size);
                            }
                        }
                    }
                    root = match value {
                        Some(v) => trie.insert(root, key, v)?,
                        None => trie.remove(root, key)?,
                    };
                }
                // Publication owns the invariant: callers cannot accidentally
                // publish an own live file without also advertising the
                // complete durable content that the source hold just proved.
                // LEAN-MODEL: cas-publication-contract (Publication.publication_contract)
                // `Publication.publication_contract` is what this transaction
                // promises along every execution: for as long as the tree
                // names the content, its holder pins it, it is available, and
                // its size is the one recorded here.
                for (content, size) in source_ads {
                    let ad = synch_core::record::encode(&BlobAd::complete(size))?;
                    root = trie.insert(root, &blob_key(&content), &ad)?;
                }
                if root == old_root {
                    return Ok(None);
                }

                // Read from the same snapshot the flip is written against, and
                // from *every* record of what this origin has already signed —
                // both slots and the retained history, not the complete slot
                // alone ([`Txn::next_own_seq`]).
                let seq = txn.next_own_seq(&origin)?;
                let head = SignedHead::sign(&secret, origin.clone(), seq, root, now);
                // No explicit history writes: `put_head` records the signature
                // it is pointing at, and the head being displaced recorded its
                // own when it took the slot (§10, v11).
                txn.put_head(Slot::Complete, &head, now, now)?;
                txn.materialize_diff(&origin, old_root, root)?;
                for space in changed_spaces {
                    txn.reconcile_source_holds(&origin, &space)?;
                }
                Ok(Some(head))
            })?;

        if let Some(head) = &head {
            // This node just built the whole trie under that root, so it holds
            // it whole by construction. Recording that here is what keeps the
            // first `Hello` after every publish from proving it again by
            // walking the entire trie (§5.1).
            synch_mpt::NodeStore::note_complete(self.store().as_ref(), &head.root)?;
            tracing::info!(
                seq = head.seq,
                changes = staged.len(),
                "published a new root"
            );
        }
        Ok(head)
    }

    /// Builds the `m:self` manifest record for this node (§4.2).
    ///
    /// Node-wide facts only. What this node advertises about each space is a
    /// record of its own under `m:space/<id>`, because a leaf value cannot be
    /// partly redacted and a single manifest listing every space would be
    /// unshowable to a delegate (§5.5).
    pub(crate) fn manifest_change(&self) -> Result<StagedChange> {
        let manifest = NodeManifest {
            v: synch_core::RECORD_VERSION,
            name: self.inner.config.name.clone(),
            software: SOFTWARE.to_string(),
        };
        let bytes = synch_core::record::encode(&manifest)?;
        Ok((manifest_key(), Some(bytes)))
    }

    /// Builds the `m:space/<id>` records for this node's spaces (§4.2, §5.5).
    pub(crate) fn space_info_changes(&self) -> Result<Vec<StagedChange>> {
        let mut out = Vec::new();
        for source in self.store().sources()? {
            let entry_count = self.store().count_entries(self.origin(), &source.space)?;
            let info = SpaceInfo {
                v: synch_core::RECORD_VERSION,
                // Local paths are host-private implementation details and are
                // never meaningful to another member of the cluster.
                description: String::new(),
                entry_count,
            };
            let bytes = synch_core::record::encode(&info)?;
            out.push((synch_core::space_info_key(&source.space)?, Some(bytes)));
        }
        Ok(out)
    }

    /// The tombstone that removes one space's advertised record.
    pub(crate) fn space_info_removal(&self, space: &str) -> Result<StagedChange> {
        Ok((synch_core::space_info_key(space)?, None))
    }

    /// Reads what an origin publishes about one space (§4.2, §5.5).
    ///
    /// `None` for a space the origin does not advertise. Reading under a scope
    /// this space falls outside of is a different answer: the subtree was never
    /// served, so the lookup fails rather than reporting an absence — which is
    /// the distinction a scoped node has to keep (§5.5).
    pub fn space_info_of(&self, origin: &OriginId, space: &str) -> Result<Option<SpaceInfo>> {
        let Some(head) = self.store().complete_head(origin)? else {
            return Ok(None);
        };
        let trie = Trie::new(self.store().as_ref());
        let Some(bytes) = trie.get(head.root, &synch_core::space_info_key(space)?)? else {
            return Ok(None);
        };
        synch_core::record::decode(&bytes)
            .map(Some)
            .map_err(EngineError::from)
    }

    /// Reads an origin's published manifest.
    #[cfg(test)]
    pub(crate) fn manifest_of(&self, origin: &OriginId) -> Result<Option<NodeManifest>> {
        let Some(head) = self.store().complete_head(origin)? else {
            return Ok(None);
        };
        let trie = Trie::new(self.store().as_ref());
        let Some(bytes) = trie.get(head.root, &manifest_key())? else {
            return Ok(None);
        };
        synch_core::record::decode(&bytes)
            .map(Some)
            .map_err(EngineError::from)
    }

    // ---- blob advertisements ---------------------------------------------

    /// The `b:` record for a locally held object, if we hold any of it.
    pub(crate) fn ad_change(&self, root: &Hash) -> Result<Option<StagedChange>> {
        if self.cas_backend().remote_upload_parts()
            && self
                .store()
                .blob(root)?
                .is_some_and(|row| row.complete && !row.durable)
        {
            // A complete cloud ad survives in a signed head, so recovery must
            // be able to treat it as a durability promise. Cache-only objects
            // may advertise partial progress, but completion under `own` /
            // `own+pinned` retires that transient ad instead of making an
            // ambiguous promise the next SQLite restore cannot interpret.
            return Ok(Some((blob_key(root), None)));
        }
        let Some(ad) = self.store().local_ad(root)? else {
            return Ok(None);
        };
        let bytes = synch_core::record::encode(&ad)?;
        Ok(Some((blob_key(root), Some(bytes))))
    }

    /// Records what a checkout pass believes about the file at `target`, and
    /// the stat that belief is anchored to (`docs/DELTA-SYNC.md` §3.5).
    ///
    /// The belief comes from one of two moments: the pass wrote the file
    /// itself (a successful write is trusted, the same at-rest posture the
    /// CAS takes toward its own payloads, §2.1), or the pass found the file
    /// on disk and hashed it to the selected root. Either way the record lets
    /// later passes answer "is this already the selected version?" with a
    /// stat instead of a hash, so a quiet pass costs the tree's syscalls, not
    /// its bytes. The stat is the evidence the scanner trusts for the node's
    /// own files — length, stored mtime, platform identity, past the racy
    /// window — with a stronger anchor than the scanner's: not a
    /// peer-published mtime that happens to match, but a write or hash this
    /// process performed itself.
    ///
    /// Two limits, both deliberate:
    ///
    /// - **In memory, per process.** Nothing durable ever calls a file good,
    ///   so there is no stale verdict to clear. The price is paid on restart:
    ///   every checkout's first pass hashes the whole tree once, and that pass
    ///   doubles as the checkout's only scrub.
    /// - **A stat that never moved hides what lies beneath it.** A same-size
    ///   rewrite that restores length, mtime, and identity — and bytes that
    ///   rot at rest behind an unmoved stat, including a CAS payload already
    ///   rotted before a pass wrote from it — are invisible until the next
    ///   restart's hash. That is the filesystem-integrity domain, and §2.1
    ///   delegates it there.
    pub(crate) fn note_checkout_write(&self, target: &Path, write: CheckoutWrite) {
        self.checkout_writes().insert(target.to_path_buf(), write);
    }

    /// What passes believe about `target`, if this process believes anything.
    pub(crate) fn checkout_write_was(&self, target: &Path) -> Option<CheckoutWrite> {
        self.checkout_writes().get(target).cloned()
    }

    /// Forgets what was believed about `target` — called when the file leaves
    /// the checkout, when the file is gone or the wrong length, and when a
    /// fresh write or hash re-anchors the belief.
    pub(crate) fn forget_checkout_write(&self, target: &Path) {
        self.checkout_writes().remove(target);
    }

    fn checkout_writes(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<PathBuf, CheckoutWrite>> {
        self.inner
            .checkout_writes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Whether an object's advertisement is due for a milestone update (§6.3).
    ///
    /// Ads are published on first ingest and on completion, and otherwise at
    /// most once per `ad_update_interval` per object while a download is in
    /// flight — never per chunk.
    pub(crate) fn ad_update_due(&self, root: &Hash) -> Result<bool> {
        let Some(blob) = self.store().blob(root)? else {
            return Ok(false);
        };
        let interval = self.inner.config.ad_update_interval.as_nanos() as i64;
        let mut clock = self
            .inner
            .ad_clock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = now_ns();
        match clock.get(root) {
            // First sighting, or the object just completed: always a milestone.
            None => {
                clock.insert(*root, now);
                Ok(true)
            }
            Some(_) if blob.complete => {
                clock.insert(*root, now);
                Ok(true)
            }
            Some(last) if now - last >= interval => {
                clock.insert(*root, now);
                Ok(true)
            }
            Some(_) => Ok(false),
        }
    }

    /// True if provider discovery for this root is still backed off (§6.3).
    ///
    /// Asked before dialling anybody. Callers reach discovery only when no
    /// local ad covers the root, so a root that becomes available through
    /// ordinary head replication is fetched without ever consulting this.
    pub(crate) fn provider_discovery_backed_off(&self, root: &Hash, now: i64) -> bool {
        self.provider_misses()
            .get(root)
            .is_some_and(|miss| now < miss.until)
    }

    /// Records that discovery found nobody for this root, doubling the wait.
    pub(crate) fn note_provider_miss(&self, root: &Hash, now: i64) {
        let mut misses = self.provider_misses();
        let previous = misses.get(root).map(|m| m.misses).unwrap_or(0);
        let backoff = PROVIDER_MISS_BACKOFF
            .saturating_mul(1u32 << previous.min(16))
            .min(PROVIDER_MISS_MAX_BACKOFF);
        let entry = ProviderMiss {
            until: now.saturating_add(backoff.as_nanos() as i64),
            misses: previous.saturating_add(1),
        };
        if misses.len() >= MAX_PROVIDER_MISSES && !misses.contains_key(root) {
            misses.retain(|_, miss| now < miss.until);
            if misses.len() >= MAX_PROVIDER_MISSES {
                if let Some(soonest) = misses
                    .iter()
                    .min_by_key(|(_, miss)| miss.until)
                    .map(|(root, _)| *root)
                {
                    misses.remove(&soonest);
                }
            }
        }
        misses.insert(*root, entry);
    }

    /// Forgets a miss, because somebody turned out to hold the root after all.
    pub(crate) fn clear_provider_miss(&self, root: &Hash) {
        self.provider_misses().remove(root);
    }

    fn provider_misses(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<Hash, ProviderMiss>> {
        self.inner
            .provider_misses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The published `b:` ad we currently advertise for an object.
    pub fn published_ad(&self, root: &Hash) -> Result<Option<BlobAd>> {
        let head_root = self.current_root()?;
        let trie = Trie::new(self.store().as_ref());
        let Some(bytes) = trie.get(head_root, &blob_key(root))? else {
            return Ok(None);
        };
        synch_core::record::decode(&bytes)
            .map(Some)
            .map_err(EngineError::from)
    }

    // ---- entry helpers ----------------------------------------------------

    /// The trie key for a path in one of this node's spaces.
    pub fn key_for(&self, space: &str, path: &str) -> Result<Vec<u8>> {
        Ok(file_key(space, path)?)
    }
}

fn canonical_dir(path: &Path) -> Result<PathBuf> {
    // A bare os error with no path reads as a daemon fault; naming the path
    // and the operation makes "Permission denied" point at the right thing.
    std::fs::create_dir_all(path)
        .map_err(|e| EngineError::invalid(format!("could not create {}: {e}", path.display())))?;
    std::fs::canonicalize(path)
        .map_err(|e| EngineError::invalid(format!("could not resolve {}: {e}", path.display())))
}

/// True if either path contains the other.
pub(crate) fn paths_overlap(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

/// Resolves a stored source or checkout root for comparison against a freshly
/// canonicalized path.
///
/// Both registration paths canonicalize before storing, but a stored root can
/// still be non-canonical: it may predate that, or a symlink may have appeared
/// along it since. Comparing a canonical path against a raw one silently
/// misses overlaps — on macOS every temp path under `/var` resolves to
/// `/private/var`, so the guard passed a directory it should have refused.
/// Falls back to the raw value when the directory no longer exists.
pub(crate) fn stored_root(path: &str) -> PathBuf {
    let raw = PathBuf::from(path);
    std::fs::canonicalize(&raw).unwrap_or(raw)
}

fn persisted_cloud_settings(
    cloud: &synch_store::cloud::CloudConfig,
) -> Vec<(String, Option<String>)> {
    let prefix = format!("cas.cloud.{}", cloud.service.as_str());
    let admitted: &[&str] = match cloud.service {
        synch_store::cloud::CloudService::S3 => &[
            "root",
            "bucket",
            "region",
            "endpoint",
            "enable_virtual_host_style",
        ],
        synch_store::cloud::CloudService::Gcs => &["root", "bucket", "endpoint"],
        synch_store::cloud::CloudService::Azblob => {
            &["root", "container", "endpoint", "account_name"]
        }
        synch_store::cloud::CloudService::Memory => &["root"],
    };
    let mut settings: Vec<(String, Option<String>)> = admitted
        .iter()
        .map(|name| {
            (
                format!("{prefix}.{name}"),
                cloud.options.get(*name).cloned(),
            )
        })
        .collect();
    settings.push((
        format!("{prefix}.cache_bytes"),
        cloud.cache_bytes.map(|bytes| bytes.to_string()),
    ));
    settings.push((
        format!("{prefix}.upload"),
        Some(cloud.upload_policy.as_str().to_string()),
    ));
    settings
}

/// Encodes an endpoint address for the `peers_seen.last_addr` column.
pub fn encode_addr(addr: &EndpointAddr) -> Vec<u8> {
    let parts: Vec<String> = addr
        .ip_addrs()
        .map(|a| format!("ip:{a}"))
        .chain(addr.relay_urls().map(|u| format!("relay:{u}")))
        .collect();
    parts.join("\n").into_bytes()
}

/// Decodes an endpoint address from the `peers_seen.last_addr` column.
pub(crate) fn decode_addr(id: NodeId, bytes: &[u8]) -> Option<EndpointAddr> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut addr = EndpointAddr::new(id);
    for part in text.split('\n').filter(|p| !p.is_empty()) {
        if let Some(ip) = part.strip_prefix("ip:") {
            if let Ok(socket) = ip.parse() {
                addr = addr.with_ip_addr(socket);
            }
        } else if let Some(url) = part.strip_prefix("relay:") {
            if let Ok(url) = url.parse() {
                addr = addr.with_relay_url(url);
            }
        }
    }
    Some(addr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{node, node_with};

    fn node_dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[tokio::test]
    async fn replica_concurrency_must_stay_inside_the_engine_bound() {
        for value in [0, crate::MAX_REPLICA_CONCURRENCY + 1] {
            let dir = node_dir();
            let mut config = NodeConfig::loopback(dir.path());
            config.replica_concurrency = value;
            let error = Node::open(config).await.unwrap_err().to_string();
            assert!(error.contains("replica_concurrency"), "{error}");
            assert!(error.contains("between 1 and"), "{error}");
        }
    }

    #[tokio::test]
    async fn init_identity_follows_the_zone() {
        // No domain: the key is the identity, opening before init fails
        // clearly, and re-init is refused.
        let dir = node_dir();
        let err = Node::open(NodeConfig::loopback(dir.path()))
            .await
            .unwrap_err();
        assert!(matches!(err, EngineError::NotInitialized));
        let report = Node::init(dir.path(), None).unwrap();
        assert_eq!(report.origin, Some(OriginId::Key(report.node_id)));
        assert_eq!(report.domain, None);
        assert!(Node::init(dir.path(), None).is_err());
        let node = Node::open(NodeConfig::loopback(dir.path())).await.unwrap();
        assert_eq!(node.origin(), &OriginId::Key(report.node_id));
        assert_eq!(node.node_id(), report.node_id);
        // Live self binding: the node can verify its own heads.
        assert!(node
            .store()
            .is_bound(node.origin(), &node.node_id(), now_ns())
            .unwrap());

        // A domain that has not named it yet: no origin, name normalized.
        let dir = node_dir();
        let report = Node::init(dir.path(), Some("Cluster.Example.")).unwrap();
        assert_eq!(report.origin, None);
        assert_eq!(report.domain.as_deref(), Some("cluster.example"));
        let store = Arc::new(Store::open(dir.path()).unwrap());
        assert_eq!(store.self_origin().unwrap(), None);

        node.shutdown().await.unwrap();
    }

    /// A zone's answer migrates identity-bound state and clears the old floor.
    #[test]
    fn adopting_a_name_migrates_everything_keyed_by_the_old_one() {
        let dir = node_dir();
        let report = Node::init(dir.path(), None).unwrap();
        let previous = OriginId::Key(report.node_id);
        let store = Store::open(dir.path()).unwrap();
        store.raise_publish_floor(500).unwrap();

        let named = OriginId::named("orb", "cluster.example").unwrap();
        Node::migrate_identity(
            &store,
            Some(&previous),
            &named,
            report.node_id,
            "cluster.example",
        )
        .unwrap();

        assert_eq!(store.self_origin().unwrap(), Some(named.clone()));
        assert!(store.is_bound(&named, &report.node_id, now_ns()).unwrap());
        assert!(!store
            .is_bound(&previous, &report.node_id, now_ns())
            .unwrap());
        assert!(store.complete_head(&previous).unwrap().is_none());
        // The floor bounded seqs under a name nobody holds any more.
        assert_eq!(store.publish_floor().unwrap(), None);

        let history = store.identity_history().unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].previous, Some(previous));
        assert_eq!(history[0].adopted, named);
        assert_eq!(history[0].domain, "cluster.example");
    }

    /// A rename revokes what the old name vouched for, and the rows are *gone*,
    /// not debris: `d:` records live only in the trie the migration drops (§3.5).
    #[tokio::test]
    async fn adopting_a_name_revokes_the_delegations_the_old_name_issued() {
        let dir = node_dir();
        let named = OriginId::named("nas", "cluster.example").unwrap();
        let report = Node::init(dir.path(), None).unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap());

        let subject = {
            let node = Node::open(NodeConfig::loopback(dir.path())).await.unwrap();
            let subject = iroh_base::SecretKey::generate().public();
            let change = node
                .delegate_add(
                    subject,
                    &["photos".to_string()],
                    now_ns() + 86_400_000_000_000,
                    None,
                )
                .unwrap();
            node.publish(&[change]).unwrap().unwrap();
            assert!(store.is_trusted_key(&subject, now_ns()).unwrap());
            assert_eq!(store.all_delegations().unwrap().len(), 1);
            node.shutdown().await.unwrap();
            subject
        };
        let previous = store.self_origin().unwrap().unwrap();

        Node::migrate_identity(
            &store,
            Some(&previous),
            &named,
            report.node_id,
            "cluster.example",
        )
        .unwrap();

        assert!(
            !store.is_trusted_key(&subject, now_ns()).unwrap(),
            "the delegate is no longer admitted"
        );
        assert!(
            store.all_delegations().unwrap().is_empty(),
            "the rows went with the name that issued them"
        );
    }

    /// A zone that answers and does not name this node is a *delegate*, not a
    /// failure: it keeps its key identity and goes on resolving the zone.
    ///
    /// This is the case §3.5 is built on — a delegated node belongs to a
    /// cluster and is named by no zone in it — and `settle_identity` used to
    /// leave it waiting on the reduced socket for a record that was never
    /// coming, because "the zone says you are not in it" and "I could not ask
    /// the zone" both arrived here as `None`. Only the second is an error.
    #[tokio::test]
    async fn a_zone_that_does_not_name_this_node_leaves_it_key_identified() {
        #[derive(Debug)]
        struct Answers(Vec<String>);
        impl synch_net::MemberResolver for Answers {
            fn resolve_members<'a>(
                &'a self,
                domain: &'a str,
            ) -> synch_net::dns::MemberSetFuture<'a> {
                let records = self.0.clone();
                Box::pin(async move {
                    Ok((
                        synch_net::MemberSet::from_records(domain, &records)
                            .map_err(|e| synch_net::NetError::Dns(e.to_string()))?,
                        std::time::Duration::from_secs(300),
                    ))
                })
            }
        }

        let dir = node_dir();
        let report = Node::init(dir.path(), None).unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap());
        // The operator joins the cluster's zone as a delegate: this node
        // belongs to it and expects no record naming itself. Without that word
        // from the operator, a zone that answers and does not name this key is
        // `Unidentified`, and the daemon waits on the reduced socket.
        store
            .set_membership_domain(Some("cluster.example"))
            .unwrap();
        store.set_membership_expects_name(false).unwrap();
        let other = iroh_base::SecretKey::generate().public();
        let zone = Answers(vec![format!("v=sync1 id=nas nk={}", other.to_z32())]);

        let settled = Node::settle_identity(&store, report.node_id, Some(&zone))
            .await
            .expect("a zone that does not name this node is not a failure to start");
        assert_eq!(
            settled,
            OriginId::Key(report.node_id),
            "a delegate keeps the key that identifies it"
        );
    }

    /// A node that starts as a delegate and is *later named* by its zone
    /// adopts the name through the ordinary identity migration.
    ///
    /// The interesting half is what the migration does *not* touch: a `d:`
    /// record naming this node's key lives in the issuing origin's trie, and a
    /// rename here cannot reach it. So a node the zone has just promoted stays
    /// confined to its grant until the issuer revokes that record — which is
    /// the same rule as everywhere else (a delegation outranks a rooted
    /// binding, §3.5) and is worth pinning, because an operator who adds a DNS
    /// record expecting a promotion will not otherwise see why nothing changed.
    #[tokio::test]
    async fn a_delegate_later_named_by_its_zone_adopts_the_name() {
        #[derive(Debug)]
        struct Answers(std::sync::Mutex<Vec<String>>);
        impl synch_net::MemberResolver for Answers {
            fn resolve_members<'a>(
                &'a self,
                domain: &'a str,
            ) -> synch_net::dns::MemberSetFuture<'a> {
                let records = self.0.lock().unwrap().clone();
                Box::pin(async move {
                    Ok((
                        synch_net::MemberSet::from_records(domain, &records)
                            .map_err(|e| synch_net::NetError::Dns(e.to_string()))?,
                        std::time::Duration::from_secs(300),
                    ))
                })
            }
        }

        let dir = node_dir();
        let report = Node::init(dir.path(), None).unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap());
        store
            .set_membership_domain(Some("cluster.example"))
            .unwrap();
        store.set_membership_expects_name(false).unwrap();

        // First start: the zone names other members, not this key.
        let other = iroh_base::SecretKey::generate().public();
        let zone = Answers(std::sync::Mutex::new(vec![format!(
            "v=sync1 id=nas nk={}",
            other.to_z32()
        )]));
        let first = Node::settle_identity(&store, report.node_id, Some(&zone))
            .await
            .unwrap();
        assert_eq!(first, OriginId::Key(report.node_id));

        // The issuer delegates `photos` to this key, as a real cluster would.
        let issuer = OriginId::named("nas", "cluster.example").unwrap();
        store
            .put_binding(&synch_store::Binding {
                origin: OriginId::Key(report.node_id),
                node_id: report.node_id,
                source: BindingSource::Delegated,
                domain: None,
                issuer: Some(issuer.clone()),
                spaces: vec!["photos".to_string()],
                note: None,
                added_at: 0,
                expires_at: None,
            })
            .unwrap();

        // The operator now publishes a record for this key.
        zone.0
            .lock()
            .unwrap()
            .push(format!("v=sync1 id=laptop nk={}", report.node_id.to_z32()));
        let named = Node::settle_identity(&store, report.node_id, Some(&zone))
            .await
            .unwrap();
        assert_eq!(
            named,
            OriginId::named("laptop", "cluster.example").unwrap(),
            "the zone naming this key promotes it at the next start"
        );
        assert_eq!(store.self_origin().unwrap(), Some(named.clone()));
        // The old key identity is migrated away from, not left beside it.
        assert_eq!(
            store.identity_history().unwrap().last().unwrap().adopted,
            named
        );
        assert!(store
            .complete_head(&OriginId::Key(report.node_id))
            .unwrap()
            .is_none());
    }

    /// A node that expects to be named and is not is still left `Unidentified`
    /// — the daemon comes up on the reduced socket and waits — even though the
    /// zone answered (§3.1).
    ///
    /// This is the half that makes the delegate opt-in worth having. On a first
    /// start "the zone does not name me" and "my record has not propagated yet"
    /// are the same answer, so without the operator's word a member joining
    /// during a propagation lag would silently publish under a key origin and
    /// migrate away from it later — leaving that origin in every peer's view.
    #[tokio::test]
    async fn a_node_that_expects_a_name_and_has_none_waits_for_one() {
        #[derive(Debug)]
        struct WithoutUs(String);
        impl synch_net::MemberResolver for WithoutUs {
            fn resolve_members<'a>(
                &'a self,
                domain: &'a str,
            ) -> synch_net::dns::MemberSetFuture<'a> {
                let records = vec![self.0.clone()];
                Box::pin(async move {
                    Ok((
                        synch_net::MemberSet::from_records(domain, &records)
                            .map_err(|e| synch_net::NetError::Dns(e.to_string()))?,
                        std::time::Duration::from_secs(300),
                    ))
                })
            }
        }

        let dir = node_dir();
        let report = Node::init(dir.path(), None).unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap());
        // The default: this node expects its zone to name it.
        store
            .set_membership_domain(Some("cluster.example"))
            .unwrap();
        assert!(store.membership_expects_name().unwrap());

        let other = iroh_base::SecretKey::generate().public();
        let zone = WithoutUs(format!("v=sync1 id=nas nk={}", other.to_z32()));
        let err = Node::settle_identity(&store, report.node_id, Some(&zone))
            .await
            .expect_err("a member whose record is missing must not take a key identity");
        assert!(matches!(err, EngineError::Unidentified { .. }), "{err:?}");
        // And the message says how to proceed either way.
        let said = err.to_string();
        assert!(said.contains("--delegate"), "{said}");
    }

    /// A node the zone *has* named, whose record then goes missing from an
    /// otherwise-valid answer, keeps its name (§3.1).
    ///
    /// This is a withdrawal, and a withdrawal must not cost a running member
    /// its identity: propagation lags, an operator edits the zone in flight,
    /// an answer arrives partial. The node goes on publishing under the name it
    /// already holds. The delegate branch is guarded on having *no* usable
    /// name for exactly this reason.
    #[tokio::test]
    async fn a_named_node_whose_record_goes_missing_keeps_its_name() {
        #[derive(Debug)]
        struct WithoutUs(String);
        impl synch_net::MemberResolver for WithoutUs {
            fn resolve_members<'a>(
                &'a self,
                domain: &'a str,
            ) -> synch_net::dns::MemberSetFuture<'a> {
                let records = vec![self.0.clone()];
                Box::pin(async move {
                    Ok((
                        synch_net::MemberSet::from_records(domain, &records)
                            .map_err(|e| synch_net::NetError::Dns(e.to_string()))?,
                        std::time::Duration::from_secs(300),
                    ))
                })
            }
        }

        let dir = node_dir();
        let named = OriginId::named("laptop", "cluster.example").unwrap();
        let report = Node::init_named_by_zone(dir.path(), named.clone()).unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap());

        // The zone answers, and this node is not in the answer.
        let other = iroh_base::SecretKey::generate().public();
        let zone = WithoutUs(format!("v=sync1 id=nas nk={}", other.to_z32()));
        let settled = Node::settle_identity(&store, report.node_id, Some(&zone))
            .await
            .expect("a withdrawal does not unname a node that already has a name");
        assert_eq!(
            settled, named,
            "a member whose record is momentarily absent keeps publishing under its name"
        );
    }

    /// The other half: a zone this node *cannot reach* still leaves it
    /// unnamed, because "not in the zone" and "could not ask" must not look
    /// alike (§3.1).
    #[tokio::test]
    async fn a_zone_that_cannot_be_reached_still_leaves_the_node_unnamed() {
        #[derive(Debug)]
        struct Unreachable;
        impl synch_net::MemberResolver for Unreachable {
            fn resolve_members<'a>(
                &'a self,
                _domain: &'a str,
            ) -> synch_net::dns::MemberSetFuture<'a> {
                Box::pin(async move { Err(synch_net::NetError::Dns("nxdomain".into())) })
            }
        }

        let dir = node_dir();
        let report = Node::init(dir.path(), None).unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap());
        store.set_membership_domain(Some("typo.example")).unwrap();

        let err = Node::settle_identity(&store, report.node_id, Some(&Unreachable))
            .await
            .expect_err("a zone that cannot be asked is not a delegation");
        assert!(matches!(err, EngineError::Unidentified { .. }), "{err:?}");
    }

    /// `settle_identity` answers from the membership domain alone: a cleared
    /// domain migrates back to the device key, a replaced one leaves the node
    /// unidentified (§3.1).
    #[tokio::test]
    async fn settle_identity_follows_the_membership_domain() {
        // Cleared: the device key names the node again; everything keyed by
        // the old name goes with the migration.
        let dir = node_dir();
        let named = OriginId::named("nas", "cluster.example").unwrap();
        let report = Node::init_named_by_zone(dir.path(), named.clone()).unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap());
        store.set_membership_domain(None).unwrap();

        let settled = Node::settle_identity(&store, report.node_id, None)
            .await
            .unwrap();
        assert_eq!(settled, OriginId::Key(report.node_id));
        assert!(store.complete_head(&named).unwrap().is_none());
        assert_eq!(
            store.identity_history().unwrap().last().unwrap().adopted,
            settled
        );

        // Replaced: nothing names it, so it waits rather than signing under
        // it.
        let dir = node_dir();
        let report = Node::init_named_by_zone(dir.path(), named.clone()).unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap());
        store.set_membership_domain(Some("other.example")).unwrap();
        let err = Node::settle_identity(&store, report.node_id, None)
            .await
            .unwrap_err();
        let EngineError::Unidentified { domain, node_id } = &err else {
            panic!("{err:?}");
        };
        assert_eq!(domain, "other.example");
        assert_eq!(**node_id, report.node_id);
        assert!(err.to_string().contains(&report.node_id.to_z32()));
    }

    #[tokio::test]
    async fn manifests_round_trip_through_the_trie() {
        let (_d, node) = node().await;
        let space = tempfile::tempdir().unwrap();
        node.add_filesystem_source("media", space.path()).unwrap();
        let mut staged = vec![node.manifest_change().unwrap()];
        staged.extend(node.space_info_changes().unwrap());
        node.publish(&staged).unwrap().unwrap();

        let manifest = node.manifest_of(node.origin()).unwrap().unwrap();
        assert_eq!(manifest.software, SOFTWARE);
        // Space info is the node's own record, shown to a delegated peer and
        // withheld from one that is not (§5.5).
        let info = node.space_info_of(node.origin(), "media").unwrap().unwrap();
        assert_eq!(info.description, "");
        assert!(node
            .space_info_of(node.origin(), "absent")
            .unwrap()
            .is_none());
        node.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn sources_may_not_overlap_replica_checkouts() {
        let (_d, node) = node().await;
        let shared = tempfile::tempdir().unwrap();
        node.store()
            .put_replica(&synch_store::ReplicaRow {
                space: "media".into(),
                retention: synch_store::ReplicaPolicy::Current,
                grace: Some(60),
                budget: None,
                checkout_path: Some(shared.path().to_string_lossy().into_owned()),
            })
            .unwrap();
        let err = node
            .add_filesystem_source("media", shared.path())
            .unwrap_err();
        assert!(err.to_string().contains("overlaps replica checkout"));

        // And a nested subdirectory is caught too, so "no echo" is structural.
        let nested = shared.path().join("sub");
        assert!(node.add_filesystem_source("nested", &nested).is_err());

        // Spaces may not overlap each other either; re-adding the same space
        // id at the same path is a legal update.
        let a = tempfile::tempdir().unwrap();
        node.add_filesystem_source("a", a.path()).unwrap();
        assert!(node
            .add_filesystem_source("b", a.path().join("sub"))
            .is_err());
        node.add_filesystem_source("a", a.path()).unwrap();

        node.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_cloud_namespace_change_requires_cas_migration() {
        let data = tempfile::tempdir().unwrap();
        Node::init(data.path(), None).unwrap();
        let configured = |root: &str| {
            let mut config = NodeConfig::loopback(data.path());
            config.cloud = Some(synch_store::cloud::CloudConfig {
                service: synch_store::cloud::CloudService::Memory,
                options: [("root".to_string(), root.to_string())]
                    .into_iter()
                    .collect(),
                scratch_dir: data.path().join("cloud-scratch"),
                io_timeout: std::time::Duration::from_secs(5),
                upload_policy: synch_store::cloud::CloudUploadPolicy::OwnPinned,
                cache_bytes: Some(512 * 1024 * 1024),
            });
            config
        };
        let node = Node::open(configured("/one/")).await.unwrap();
        node.shutdown().await.unwrap();
        let error = Node::open(configured("/two/")).await.unwrap_err();
        assert!(error.to_string().contains("synch cas migrate"), "{error}");
    }

    #[tokio::test]
    async fn a_cloud_node_refuses_an_existing_filesystem_source() {
        let data = tempfile::tempdir().unwrap();
        let checkout = tempfile::tempdir().unwrap();
        Node::init(data.path(), None).unwrap();
        Store::open(data.path())
            .unwrap()
            .put_source(
                "media",
                synch_store::SourceKind::Filesystem,
                Some(&checkout.path().to_string_lossy()),
            )
            .unwrap();
        let mut config = NodeConfig::loopback(data.path());
        config.cloud = Some(synch_store::cloud::CloudConfig {
            service: synch_store::cloud::CloudService::Memory,
            options: Default::default(),
            scratch_dir: data.path().join("cloud-scratch"),
            io_timeout: std::time::Duration::from_secs(5),
            upload_policy: synch_store::cloud::CloudUploadPolicy::OwnPinned,
            cache_bytes: Some(512 * 1024 * 1024),
        });
        let error = Node::open(config).await.unwrap_err();
        assert!(error.to_string().contains("filesystem source"), "{error}");
    }

    #[tokio::test]
    async fn legacy_rekor_pin_state_moves_once_into_sqlite() {
        let data = tempfile::tempdir().unwrap();
        Node::init(data.path(), None).unwrap();
        let legacy = data.path().join("rekor-pins.json");
        std::fs::write(&legacy, r#"{"generation":1}"#).unwrap();
        let node = Node::open(NodeConfig::loopback(data.path())).await.unwrap();
        assert_eq!(
            node.store().config("rekor.pin_state").unwrap().as_deref(),
            Some(r#"{"generation":1}"#)
        );
        assert!(!legacy.exists());
        node.shutdown().await.unwrap();

        // A stale legacy file can reappear after restoring mixed volumes; the
        // SQLite floor wins and the second writable copy is removed.
        std::fs::write(&legacy, r#"{"generation":0}"#).unwrap();
        let node = Node::open(NodeConfig::loopback(data.path())).await.unwrap();
        assert_eq!(
            node.store().config("rekor.pin_state").unwrap().as_deref(),
            Some(r#"{"generation":1}"#)
        );
        assert!(!legacy.exists());
        node.shutdown().await.unwrap();
    }

    /// Static trust binds the key and only the key: names are the zone's to
    /// issue (§3.2).
    #[tokio::test]
    async fn trust_add_binds_the_key_as_the_identity() {
        let (_d, node) = node().await;
        let peer = SecretKey::generate().public();

        let origin = node.trust_add(peer, Some("laptop")).unwrap();
        assert_eq!(origin, OriginId::Key(peer));
        assert!(node.store().is_trusted_key(&peer, now_ns()).unwrap());
        node.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_publish_that_fails_halfway_leaves_nothing_behind() {
        // §10: trie, head, history and materialization commit together or not
        // at all; an undecodable record fails the last of those steps.
        let (_d, node) = node().await;
        let space = tempfile::tempdir().unwrap();
        node.add_filesystem_source("media", space.path()).unwrap();
        std::fs::write(space.path().join("a.txt"), b"hello").unwrap();
        let (_, head) = node.scan_and_publish().unwrap();
        let before = head.unwrap();
        let entries_before = node
            .store()
            .list_entries(Some(node.origin()), "media", "", None, None)
            .unwrap();

        // A well-formed `f:` key whose value no `FileEntry` decodes from.
        let poison = vec![(
            file_key("media", "poisoned").unwrap(),
            Some(vec![0xffu8; 8]),
        )];
        let err = node.publish(&poison).unwrap_err().to_string();
        assert!(err.contains("record:"), "{err}");

        // Nothing moved: not the head, not the history, not the views, not the
        // trie.
        assert_eq!(node.own_head().unwrap().unwrap(), before);
        assert_eq!(node.current_root().unwrap(), before.root);
        assert_eq!(node.store().head_history(node.origin()).unwrap().len(), 1);
        assert_eq!(
            node.store()
                .list_entries(Some(node.origin()), "media", "", None, None)
                .unwrap(),
            entries_before
        );
        assert!(node
            .store()
            .entry(node.origin(), "media", "poisoned")
            .unwrap()
            .is_none());

        // And a publish after it still works, from the head that survived.
        std::fs::write(space.path().join("b.txt"), b"hello").unwrap();
        let (_, after) = node.scan_and_publish().unwrap();
        assert_eq!(after.unwrap().seq, before.seq + 1);
        node.shutdown().await.unwrap();
    }

    /// Zero socket workers declines the capability rather than starving it:
    /// no pool, and — the part a durable write makes visible — no SSH host key
    /// minted for a node that will never answer an SSH socket. A multi-tenant
    /// host pays both of these per tenant (`docs/CLOUD-DATAPLANE.md` §4.4).
    #[tokio::test]
    async fn declining_sockets_starts_no_pool_and_mints_no_host_key() {
        let (_d, node) = node_with(|config| config.socket_workers = 0).await;
        assert!(node.socket_workers().is_none());
        assert_eq!(node.store().config("ssh.host_key.ed25519").unwrap(), None);
        node.shutdown().await.unwrap();
    }

    /// And the default still serves them, so the switch is the host's alone.
    #[cfg(all(
        any(target_os = "linux", target_os = "macos", target_os = "openbsd"),
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))]
    #[tokio::test]
    async fn the_default_still_serves_sockets() {
        let (_d, node) = node_with(|config| config.socket_workers = 1).await;
        assert!(node.socket_workers().is_some());
        assert!(node
            .store()
            .config("ssh.host_key.ed25519")
            .unwrap()
            .is_some());
        node.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn own_file_publication_requires_and_advertises_durable_content() {
        let (_d, node) = node().await;
        node.add_api_source("media").unwrap();
        let absent = Hash::new(b"not in the cas");
        let entry = synch_core::FileEntry::file(14, now_ns(), absent, 1);
        let staged = vec![(
            file_key("media", "a.txt").unwrap(),
            Some(synch_core::record::encode(&entry).unwrap()),
        )];
        let error = node.publish(&staged).unwrap_err().to_string();
        assert!(
            error.contains("complete durable content is not present"),
            "{error}"
        );
        assert!(node.own_head().unwrap().is_none());

        let root = node
            .store()
            .ingest_bytes(b"durable bytes", now_ns())
            .unwrap();
        let entry = synch_core::FileEntry::file(13, now_ns(), root, 1);
        let staged = vec![(
            file_key("media", "a.txt").unwrap(),
            Some(synch_core::record::encode(&entry).unwrap()),
        )];
        node.publish(&staged).unwrap();
        assert_eq!(
            node.published_ad(&root).unwrap(),
            Some(BlobAd::complete(13))
        );
        assert!(node.store().pins().unwrap().iter().any(|pin| {
            pin.root == root && pin.holder == synch_store::PinHolder::Source("media".into())
        }));
        node.shutdown().await.unwrap();
    }
}
