//! The `sync/mpt/1` ALPN: head gossip, trie node and value fetch, provider
//! hints (§5.1).
//!
//! Each request occupies one bidirectional stream and the responder dispatches
//! on its first frame. The head-gossip stream is the one exception: it carries
//! a fixed five-message push-pull exchange, so one round trip both offers what
//! we have and pulls what we lack.

use std::sync::Arc;

use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};
use synch_core::{
    now_ns, BlobAd, DeclaredScope, Hash, HeadSummary, MptMessage, NodeId, OriginId, SignedHead,
    MAX_BATCH, MAX_BATCH_PATH_BYTES, MAX_HEADS_PER_MESSAGE, MAX_PROVIDER_ADS, PROTO_VERSION,
};
use synch_mpt::{NodeStore, Trie, TrieNode};
use synch_store::Store;

use crate::{
    endpoint::{under_deadline, REQUEST_TIMEOUT},
    error::NetError,
    frame::{exchange, read_answer, read_frame, write_frame},
};

/// What the `sync/mpt/1` responder needs from the layer that reconciles heads.
///
/// The serve side has to answer `Hello` with this node's summaries, record what
/// a dialing peer advertised, and offer pushed heads for adoption. None of that
/// is networking — it is the §5.2 acceptance rule, the binding check, and the
/// promotion transaction — so it is named here as a requirement and implemented
/// where it belongs, in the engine.
///
/// The methods are synchronous and are called from the blocking pool: each one
/// walks a trie or opens a transaction.
pub trait HeadSink: Send + Sync + std::fmt::Debug + 'static {
    /// The head summaries this node advertises in `Hello` (§5.1).
    fn local_summaries(&self) -> Result<Vec<HeadSummary>, NetError>;

    /// Records what a peer advertised for this node's own origin (§3.4).
    fn observe_summaries_from(
        &self,
        peer: NodeId,
        summaries: &[HeadSummary],
        now: i64,
    ) -> Result<(), NetError>;

    /// Offers a head for adoption under the §5.2 acceptance rule.
    fn offer_head(&self, head: &SignedHead, now: i64) -> Result<(), NetError>;

    /// The full signed heads for the origins a peer asked about (§5.1).
    ///
    /// Only heads this node can back with a servable trie: what a peer does
    /// with one is fetch the trie under it from us.
    fn heads_for(&self, origins: &[OriginId]) -> Result<Vec<SignedHead>, NetError>;
}

/// How often a live session refreshes the sighting it recorded at accept.
const PEER_SEEN_REFRESH: std::time::Duration = std::time::Duration::from_secs(60);

/// How many peers the sighting throttle remembers before it drops what has
/// lapsed.
///
/// The map is keyed by device key, so it is bounded by the membership in any
/// honest cluster (§12 sizes that at N ≤ 100) — but a key with no live binding
/// never reaches the sighting closure at all, so this is a backstop against a
/// membership larger than anyone has, not against a stranger.
const MAX_TRACKED_SIGHTINGS: usize = 4096;

/// The `sync/mpt/1` protocol handler.
#[derive(Debug, Clone)]
pub(crate) struct MptProtocol {
    store: Arc<Store>,
    heads: Arc<dyn HeadSink>,
    on_unknown_key: Option<Arc<tokio::sync::Notify>>,
    /// The endpoint-wide in-flight gate, shared with every other ALPN mounted
    /// on this endpoint (`crate::serve::Inflight`).
    inflight: crate::serve::Inflight,
    /// When each peer's sighting was last written, shared across every
    /// connection this handler serves.
    last_sighting: Arc<std::sync::Mutex<std::collections::HashMap<NodeId, std::time::Instant>>>,
}

impl MptProtocol {
    /// Builds a handler over a store and the reconciler that owns head state.
    pub(crate) fn new(store: Arc<Store>, heads: Arc<dyn HeadSink>) -> Self {
        MptProtocol {
            store,
            heads,
            on_unknown_key: None,
            inflight: None,
            last_sighting: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// Gates this handler on the endpoint-wide in-flight semaphore.
    pub(crate) fn inflight(mut self, gate: crate::serve::Inflight) -> Self {
        self.inflight = gate;
        self
    }

    /// Rings `wake` whenever a connection is refused for an unknown key — a
    /// peer whose key this node has not resolved yet (the far side of a key
    /// rotation, typically) arrives exactly this way, and §3.4 makes that
    /// refusal a trigger for an immediate DNS re-resolution.
    pub(crate) fn on_unknown_key(mut self, wake: Option<Arc<tokio::sync::Notify>>) -> Self {
        self.on_unknown_key = wake;
        self
    }

    fn store(&self) -> &Arc<Store> {
        &self.store
    }
}

impl ProtocolHandler for MptProtocol {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        // A session outlives the request that opened it, so "last seen" cannot
        // be recorded only at accept: a peer syncing steadily over one
        // connection for an hour would read as an hour absent in `synch
        // peers`. Refreshed as requests arrive, at most once an interval —
        // the sighting is for an operator's eyes, not worth a write per
        // stream.
        //
        // The throttle is shared across connections, not captured per `accept`,
        // and the write goes to the blocking pool: it is a row update and a
        // WAL frame on the store's one write connection, and a per-connection
        // throttle throttles nothing — N sessions bought N writes for the
        // price of N handshakes.
        let store = self.store().clone();
        let last = self.last_sighting.clone();
        let sighting = move |peer: NodeId| {
            let store = store.clone();
            let last = last.clone();
            async move {
                {
                    let mut seen = last.lock().expect("the sighting lock");
                    if seen
                        .get(&peer)
                        .is_some_and(|at: &std::time::Instant| at.elapsed() < PEER_SEEN_REFRESH)
                    {
                        return;
                    }
                    // Stamped before the write, so concurrent streams do not
                    // each queue one while the first is still in flight.
                    if seen.len() >= MAX_TRACKED_SIGHTINGS {
                        seen.retain(|_, at| at.elapsed() < PEER_SEEN_REFRESH);
                    }
                    seen.insert(peer, std::time::Instant::now());
                }
                let recorded: Result<(), NetError> = crate::blocking::offload(move || {
                    Ok(store.record_peer_seen(&peer, None, now_ns())?)
                })
                .await;
                if let Err(e) = recorded {
                    tracing::debug!(peer = %peer.fmt_short(), error = %e, "could not record a sighting");
                }
            }
        };

        let handler = self.clone();
        crate::serve::serve_connection(
            &self.store.clone(),
            connection,
            self.on_unknown_key.as_ref(),
            &self.inflight,
            sighting,
            move |peer, mut send, mut recv| {
                let handler = handler.clone();
                async move {
                    if let Err(e) = handler.handle_stream(peer, &mut send, &mut recv).await {
                        tracing::debug!(peer = %peer.fmt_short(), error = %e, "mpt stream ended");
                        // The peer is told what went wrong rather than left to
                        // read a closed stream.
                        let _ = write_frame(
                            &mut send,
                            &MptMessage::Error {
                                reason: e.to_string(),
                            },
                        )
                        .await;
                    }
                    let _ = send.finish();
                }
            },
        )
        .await
    }
}

impl MptProtocol {
    async fn handle_stream(
        &self,
        peer: NodeId,
        send: &mut iroh::endpoint::SendStream,
        recv: &mut iroh::endpoint::RecvStream,
    ) -> Result<(), NetError> {
        let request: MptMessage = read_frame(recv).await?;
        match request {
            MptMessage::Hello { proto, heads, .. } => {
                if proto != PROTO_VERSION {
                    return Err(NetError::Unexpected(format!(
                        "unsupported protocol version {proto}"
                    )));
                }
                check_heads(heads.len(), "a Hello summary list")?;
                // A dialing peer's summaries are as good an observation as
                // the ones we collect by dialing out, and a node in recovery
                // is more likely to be called than to be calling (§3.4).
                // Summarizing means asking the trie whether we hold each root
                // whole — a walk on the first ask, memoized after — so the
                // pair runs on the blocking pool (§5.1).
                let sink = self.heads.clone();
                let store = self.store().clone();
                let (ours, scope) = crate::blocking::offload(move || {
                    sink.observe_summaries_from(peer, &heads, now_ns())?;
                    let summaries = sink.local_summaries()?;
                    // What this node will serve that peer, so a delegated one
                    // can learn the scope it is about to walk under (§5.5).
                    // The three-valued shape is the declaration, not a
                    // narrowing of it: collapsing `Untrusted` and
                    // `Unrestricted` into one "nothing" would tell a peer
                    // whose grant was revoked the same thing as one promoted
                    // to a full member, and the reader could not tell the two
                    // apart.
                    let scope = match store.publish_scope_of_key(&peer, now_ns())? {
                        synch_store::PublishScope::Untrusted => DeclaredScope::Untrusted,
                        synch_store::PublishScope::Unrestricted => DeclaredScope::Unrestricted,
                        synch_store::PublishScope::Confined(spaces) => {
                            DeclaredScope::Confined(spaces)
                        }
                    };
                    Ok((summaries, scope))
                })
                .await?;
                write_frame(
                    send,
                    &MptMessage::Hello {
                        proto: PROTO_VERSION,
                        heads: ours,
                        scope,
                    },
                )
                .await?;

                // The peer pushes what it has that we lack, then asks for what
                // we have that it lacks.
                match read_frame::<MptMessage>(recv).await? {
                    MptMessage::Heads { heads } => {
                        check_heads(heads.len(), "a Heads push")?;
                        // Each offer verifies a signature, records history,
                        // and may promote the head — which walks the trie and
                        // re-materializes the changed leaves in one
                        // transaction (§5.2). Containment is the sink's: only
                        // it can tell "this origin published something we
                        // cannot apply" from "our disk is full", and an error
                        // reaching here is the second kind. One origin must
                        // not stop an exchange that still owes this peer an
                        // answer to its `HeadsWant`; a local fault does.
                        let sink = self.heads.clone();
                        crate::blocking::offload(move || {
                            for head in heads {
                                sink.offer_head(&head, now_ns())?;
                            }
                            Ok(())
                        })
                        .await?;
                    }
                    other => return Err(unexpected("Heads", &other)),
                }
                match read_frame::<MptMessage>(recv).await? {
                    MptMessage::HeadsWant { origins } => {
                        check_heads(origins.len(), "a HeadsWant list")?;
                        // A database query per origin, so it goes to the
                        // blocking pool like every other head operation (§5.1).
                        let sink = self.heads.clone();
                        let heads =
                            crate::blocking::offload(move || sink.heads_for(&origins)).await?;
                        write_frame(send, &MptMessage::Heads { heads }).await?;
                    }
                    other => return Err(unexpected("HeadsWant", &other)),
                }
                Ok(())
            }
            MptMessage::HeadPush { head } => {
                let sink = self.heads.clone();
                let pushed = head.clone();
                crate::blocking::offload(move || sink.offer_head(&pushed, now_ns())).await?;
                tracing::debug!(origin = %head.origin, "head pushed to us");
                // An empty Heads is the smallest well-typed ack in the schema.
                write_frame(send, &MptMessage::Heads { heads: Vec::new() }).await?;
                Ok(())
            }
            // Both batch reads run on the blocking pool: `MAX_BATCH` row reads
            // out of SQLite is a bounded amount of work, but not a small one,
            // and a cold store answers them from disk.
            MptMessage::GetNodes { root, wants } => {
                check_wants(&wants)?;
                let store = self.store().clone();
                let (nodes, missing, redacted) = crate::blocking::offload(move || {
                    let scope = store.scope_for_key(&peer, now_ns())?;
                    let admitted = admit(&store, peer, root, &wants)?;
                    let vouch = Vouch::for_root(&store, root)?;
                    let mut answer = Answer::new();
                    let mut missing = Distinct::default();
                    let mut redacted = Distinct::default();
                    // Every position is judged, and only the *payload* is
                    // deduplicated: a scoped peer may name one node at two
                    // spine positions and be entitled to it at just one of
                    // them, so a verdict reached about the first position
                    // must not be the answer for the second — in either
                    // order, which is why the payload check comes *after* the
                    // scope judgement and a node can come back both served
                    // and redacted. `admit` ran first — the request is
                    // authorized by position and only then deduplicated by
                    // what those positions resolved to.
                    for (at, (path, claimed)) in admitted.into_iter().zip(wants.iter()) {
                        // A position holding nothing is reported against the
                        // hash the caller named, so an honest walk sees the
                        // ordinary `missing` it already handles.
                        let Some(hash) = at else {
                            missing.push(*claimed);
                            continue;
                        };
                        let Some(data) = store.get_node(&hash)? else {
                            missing.push(hash);
                            continue;
                        };
                        // Held is not the same as vouched for: under a
                        // confined origin's root only a node this store was
                        // served as that origin's goes out, to any peer.
                        if !vouch.covers(&store, &hash)? {
                            missing.push(hash);
                            continue;
                        }
                        // Position admits the node; what the node *reveals* may
                        // still run out of scope. A compressed node carries key
                        // material of its own — an extension's prefix, a leaf's
                        // remaining key and value — and one sitting on the spine
                        // can describe a key range the peer was never granted.
                        // LEAN-MODEL: mpt-serve-node (ScopedSync.ServeNode)
                        // `ScopedSync.ServeNode`/`Redacts`;
                        // `served_reveals_within_scope` is the privacy claim.
                        if !scope.is_full()
                            && !TrieNode::decode(&data)
                                .map(|node| scope.admits_node(path, &node))
                                .unwrap_or(false)
                        {
                            redacted.push(hash);
                            continue;
                        }
                        if answer.served(&hash) {
                            continue;
                        }
                        // A short answer is an ordinary answer: the requester's
                        // walk defers everything it asked for and re-offers what
                        // did not come back (`MissingWalk::resume`).
                        if !answer.push(hash, data) {
                            break;
                        }
                    }
                    Ok((
                        answer.into_payloads(),
                        missing.into_vec(),
                        redacted.into_vec(),
                    ))
                })
                .await?;
                write_frame(
                    send,
                    &MptMessage::Nodes {
                        nodes,
                        missing,
                        redacted,
                    },
                )
                .await?;
                Ok(())
            }
            MptMessage::GetValues { root, wants } => {
                check_wants(&wants)?;
                // Bounded in bytes as well as in count, and *here*. A value is
                // arbitrary bytes, so the count cap alone is not a cost cap.
                // `MAX_TRIE_VALUE_LEN` is the enforced bound, and the budget
                // below is what keeps even a full batch of them inside a frame.
                let store = self.store().clone();
                let (values, missing) = crate::blocking::offload(move || {
                    // A value is authorized by the position of the node that
                    // holds it: resolve that node, and serve the value only if
                    // the node genuinely carries it. Without the second half a
                    // scoped peer could name an in-scope node and any value
                    // hash it liked.
                    //
                    // For an unscoped peer there is nothing to authorize — any
                    // payload this store holds may go — so it is answered by
                    // hash exactly as it always was, and the descent that finds
                    // the holder is not paid for at all.
                    let scope = store.scope_for_key(&peer, now_ns())?;
                    let holders = match scope.is_full() {
                        true => None,
                        false => Some(admit(&store, peer, root, &wants)?),
                    };
                    let vouch = Vouch::for_root(&store, root)?;
                    let mut answer = Answer::new();
                    let mut missing = Distinct::default();
                    for (i, wanted) in wants.iter().enumerate() {
                        if answer.served(&wanted.1) {
                            continue;
                        }
                        if let Some(holders) = &holders {
                            // The holder must be vouched for as well as
                            // admitted: a value under a confined origin's root
                            // travels only with the node that carries it.
                            let vouched = match holders[i] {
                                Some(h) => vouch.covers(&store, &h)?,
                                None => false,
                            };
                            let carried = vouched
                                && match holders[i].map(|h| store.get_node(&h)).transpose()? {
                                    Some(Some(data)) => TrieNode::decode(&data)
                                        .map(|node| {
                                            // Coverage, not just position — the
                                            // same second half `GetNodes`
                                            // applies. A node sitting at an
                                            // in-scope position can still
                                            // describe a key that runs out of
                                            // scope: a `Leaf` spells the rest
                                            // of its key, and that key's value
                                            // is the payload being asked for.
                                            // Checking only the position here
                                            // let one handler redact a node the
                                            // other served the contents of, for
                                            // the price of knowing its value
                                            // hash.
                                            // LEAN-MODEL: mpt-serve-value (ScopedSync.ServeValue)
                                            // `ScopedSync.ServeValue`.
                                            node.value_hashes().contains(&wanted.1)
                                                && scope.admits_node(&wanted.0, &node)
                                        })
                                        .unwrap_or(false),
                                    _ => false,
                                };
                            if !carried {
                                missing.push(wanted.1);
                                continue;
                            }
                        }
                        match store.get_value(&wanted.1)? {
                            Some(data) => {
                                if !answer.push(wanted.1, data) {
                                    break;
                                }
                            }
                            None => missing.push(wanted.1),
                        }
                    }
                    Ok((answer.into_payloads(), missing.into_vec()))
                })
                .await?;
                write_frame(send, &MptMessage::Values { values, missing }).await?;
                Ok(())
            }
            MptMessage::FindProviders { object_root } => {
                // Hints are unverified — content is hash-verified regardless,
                // so a wrong hint only wastes a dial (§5.1) — and bounded, so
                // one small request cannot buy the asker an unbounded table of
                // rows to write. The bound is applied by the query and by the
                // decode of each row's spans, not by the `truncate` below: a
                // cap after the work is not a cap on the work (§12).
                //
                // On the blocking pool with every other database read. Run on
                // a runtime worker it would take the single global connection
                // mutex there, so its cost would be borne by every other
                // connection and timer in the process.
                let store = self.store().clone();
                let mut ads = crate::blocking::offload(move || {
                    let own = store.self_origin()?.zip(store.local_ad(&object_root)?);
                    let mut ads = store.providers(&object_root)?;
                    if let Some((origin, ad)) = own {
                        // A cloud replica may hold an object durably without
                        // publishing a per-object `b:` record in its aggregate
                        // replica head. Name this responder directly so a cold
                        // peer can recover bytes after every original publisher
                        // has disappeared. Prefer the fresh local fact over a
                        // possibly stale materialized ad for the same origin.
                        ads.retain(|(candidate, _)| candidate != &origin);
                        ads.insert(0, (origin, ad));
                    }
                    Ok(ads)
                })
                .await?;
                ads.truncate(MAX_PROVIDER_ADS);
                write_frame(send, &MptMessage::Providers { ads }).await?;
                Ok(())
            }
            MptMessage::GetBindings { origin } => {
                // What this peer currently holds bound, live keys only — a
                // lapsed binding is exactly what the asker wants to know is
                // gone (§3.4). Informational within the trusted cluster: the
                // caller is already an authorized member (§3.2, §12).
                let store = self.store().clone();
                let asked = origin.clone();
                let keys =
                    crate::blocking::offload(move || Ok(store.keys_for_origin(&asked, now_ns())?))
                        .await?;
                write_frame(send, &MptMessage::BindingsFor { origin, keys }).await?;
                Ok(())
            }
            other => Err(unexpected("a request", &other)),
        }
    }
}

/// How many payload bytes one `Nodes` or `Values` answer may carry.
///
/// The request is capped at [`MAX_BATCH`] hashes; the answer it draws was capped
/// only by [`MAX_FRAME_LEN`](synch_core::MAX_FRAME_LEN), and discovered to
/// overrun it only *after* `write_frame` had serialized the whole message —
/// which is to say after the responder had already allocated it twice. Applied
/// while the answer is assembled, it is a bound on the work as well as on the
/// wire (§12).
///
/// Half a frame, so the postcard framing and the `missing` list have room and a
/// short answer is never produced for lack of a few hundred bytes.
const ANSWER_BYTE_BUDGET: usize = synch_core::MAX_FRAME_LEN / 2;

/// Assembles one bounded batch answer: at most [`ANSWER_BYTE_BUDGET`] payload
/// bytes, one payload per distinct hash.
///
/// The one holder of the two rules `Nodes` and `Values` answers share — and
/// must share, because each is a §12 bound the other restating is a second
/// place to lose it:
///
/// - **One payload per distinct hash.** A requester may only take one —
///   `take_served` refuses a repeated payload as a protocol violation and ends
///   the exchange — so serving a hash named twice literally would make this
///   node look hostile for a fault on the asking side. Deduplicating also
///   stops a repeated hash from turning one bounded batch into [`MAX_BATCH`]
///   copies of the same payload. Only the payload is deduplicated, not the
///   judgement: a scoped walk legitimately names one node at two spine
///   positions (§5.5), and the position refused must not answer for the
///   position admitted.
/// - **One payload always goes, whatever its size.** A stored payload larger
///   than the whole budget predates the ceiling, and answering nothing would
///   stall the requester's walk forever. It is the whole answer, though:
///   anything after it would push the frame past `MAX_FRAME_LEN`, and then the
///   requester gets an error instead of the payload, every round, without ever
///   advancing `unproductive`.
struct Answer {
    budget: usize,
    answered: std::collections::HashSet<Hash>,
    payloads: Vec<(Hash, Vec<u8>)>,
}

impl Answer {
    fn new() -> Answer {
        Answer {
            budget: ANSWER_BYTE_BUDGET,
            answered: std::collections::HashSet::new(),
            payloads: Vec::new(),
        }
    }

    /// Whether `hash`'s payload is already in the answer.
    fn served(&self, hash: &Hash) -> bool {
        self.answered.contains(hash)
    }

    /// Adds one payload under the budget; `false` once the answer is full and
    /// the assembling loop should stop.
    fn push(&mut self, hash: Hash, data: Vec<u8>) -> bool {
        self.answered.insert(hash);
        match self.budget.checked_sub(data.len()) {
            Some(left) => {
                self.budget = left;
                self.payloads.push((hash, data));
                true
            }
            None if self.payloads.is_empty() => {
                self.payloads.push((hash, data));
                false
            }
            None => false,
        }
    }

    fn into_payloads(self) -> Vec<(Hash, Vec<u8>)> {
        self.payloads
    }
}

/// A `missing` or `redacted` list that names each hash once, in first-seen
/// order, however many positions resolved to it.
#[derive(Default)]
struct Distinct {
    seen: std::collections::HashSet<Hash>,
    order: Vec<Hash>,
}

impl Distinct {
    fn push(&mut self, hash: Hash) {
        if self.seen.insert(hash) {
            self.order.push(hash);
        }
    }

    fn into_vec(self) -> Vec<Hash> {
        self.order
    }
}

/// Whose trie a request walks, and what each of those origins demands before
/// this store vouches for a node under it (§5.5).
///
/// A root this store holds a head for belongs to one origin — in principle to
/// several, if two origins signed identical tries — and for a confined one a
/// node is served only if this store was served it as that origin's
/// (`NodeStore::owns_node`). Holding the node is not enough: a member holds
/// every node of the issuer's trie, and a delegate can place any withheld
/// subtree's hash in its own trie. Enforced for every peer, scoped or not — a
/// full member that was handed the node under the grafting root would record
/// provenance for it and complete the head, and the leak would only have moved
/// one hop. A root this store holds no head for demands nothing: `admit` has
/// already refused it for a scoped peer, and an unscoped peer may have any
/// node this store holds.
// LEAN-MODEL: mpt-serve-vouched (Provenance.Vouched)
// `Provenance.Vouched`; `Provenance.serve_legit` is why what a responder hands
// out under any root is legitimately the reader's to hold.
struct Vouch {
    owners: Vec<Option<OriginId>>,
}

impl Vouch {
    fn for_root(store: &Store, root: Hash) -> Result<Vouch, NetError> {
        let now = now_ns();
        let mut owners = Vec::new();
        for origin in store.head_root_origins(&root)? {
            owners.push(store.provenance_owner(&origin, now)?);
        }
        Ok(Vouch { owners })
    }

    fn covers(&self, store: &Store, hash: &Hash) -> Result<bool, NetError> {
        if self.owners.is_empty() {
            return Ok(true);
        }
        for owner in &self.owners {
            match owner {
                None => return Ok(true),
                Some(origin) => {
                    if NodeStore::owns_node(store, origin, hash)? {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }
}

/// Resolves a batch of claimed positions and returns what stands at each,
/// refusing the whole request if any position lies outside the peer's scope.
///
/// This is where a scoped peer's view is enforced (§5.5). The peer says where it
/// believes a node sits; this descends from a root *this* node holds and
/// reports what is really there, so a fabricated root fails at the first step
/// and a lie about the position simply resolves to whatever is genuinely at
/// the path named — which is in scope by construction.
///
/// An out-of-scope position is refused rather than answered `missing`, because
/// it is not a race: an honest peer prunes its own frontier at the boundary
/// and never asks. A request that crosses it is a probe, and saying so is
/// worth more than quietly returning nothing.
// LEAN-MODEL: mpt-serve-admit (ScopedSync.Admit)
// `ScopedSync.Admit`; `admit_ignores_claim` and `admit_unique` are "a hash
// cannot be authorized; a position can".
fn admit(
    store: &Store,
    peer: NodeId,
    root: Hash,
    wants: &[(Vec<u8>, Hash)],
) -> Result<Vec<Option<Hash>>, NetError> {
    let (scope, origins) = store.scope_for_key_with_origins(&peer, now_ns())?;
    if scope.is_full() {
        // Nothing to authorize: an unscoped peer may have any node this store
        // holds, so the request is answered by hash exactly as it always was
        // and the position it carried is not consulted.
        return Ok(wants.iter().map(|(_, claimed)| Some(*claimed)).collect());
    }
    // The position is only meaningful relative to a trie this node vouches
    // for: given a root of the caller's choosing, the empty path resolves to
    // that root itself and every position below it is whatever the caller put
    // there — so authorization by position would authorize nothing at all.
    // The peer's own roots are excluded for the same reason: a delegate signs
    // and publishes its own trie, which this node records once the signature
    // and binding verify, so a root of the caller's choosing is exactly what
    // a peer's own head is — and with one it could read every withheld
    // subtree, one level at a time.
    if !store.is_head_root(&root, &origins)? {
        tracing::warn!(
            peer = %peer.fmt_short(),
            "refusing a trie request against a root this node holds no head for"
        );
        return Err(NetError::Unexpected(
            "requested positions against a root this node holds no head for".to_string(),
        ));
    }
    // Refuse an out-of-scope position without failing the whole batch. Scope
    // may differ briefly while a widened delegation replicates; the caller
    // already represents the refused position as missing.
    let mut refused = 0usize;
    let paths: Vec<Vec<u8>> = wants.iter().map(|(path, _)| path.clone()).collect();
    let admitted: Vec<bool> = paths
        .iter()
        .map(|path| {
            let ok = scope.admits_path(path);
            refused += usize::from(!ok);
            ok
        })
        .collect();
    if refused > 0 {
        tracing::warn!(
            peer = %peer.fmt_short(),
            refused,
            of = wants.len(),
            "refusing trie positions outside the peer's scope"
        );
    }
    // For a scoped peer the position is the *only* authorization, so what is
    // served is what the descent found and never what the request claimed.
    //
    // The distinction is the whole of the boundary. A delegate necessarily
    // holds the hash of every subtree withheld from it — the hash is inside the
    // branch node that makes the signed root recompute — so falling back to the
    // claimed hash where a position resolves to nothing would hand over any of
    // them for the price of naming an in-scope position that happens to be
    // empty. A position that resolves to nothing holds nothing, and that is the
    // answer.
    let resolved = Trie::new(store).resolve_paths(root, &paths)?;
    Ok(resolved
        .into_iter()
        .zip(admitted)
        .map(|(at, ok)| ok.then_some(at).flatten())
        .collect())
}

/// Bounds one positioned batch on both axes.
///
/// The count is capped while decoding, like every other sequence on this wire.
/// The *paths* are not, and they are the axis that began mattering when these
/// requests started carrying positions: a responder descends every path it is
/// handed, so [`MAX_BATCH`] maximal ones would be megabytes of request for
/// megabytes of walking (§12). A real walk's batch shares nearly all of its
/// prefixes and comes nowhere near this.
fn check_wants(wants: &[(Vec<u8>, Hash)]) -> Result<(), NetError> {
    check_batch(wants.len())?;
    let bytes: usize = wants.iter().map(|(path, _)| path.len()).sum();
    if bytes > MAX_BATCH_PATH_BYTES {
        return Err(NetError::Unexpected(format!(
            "batch carries {bytes} path bytes, over the {MAX_BATCH_PATH_BYTES} limit"
        )));
    }
    Ok(())
}

/// A backstop behind the decode-time bound.
///
/// `MptMessage` refuses an over-long field while deserializing, which is what
/// actually caps the cost — this cannot normally fire. It is kept so the
/// responder still states its own contract, and so removing a `#[serde(...)]`
/// attribute does not silently remove the cap with it.
fn check_batch(len: usize) -> Result<(), NetError> {
    if len > MAX_BATCH {
        return Err(NetError::Unexpected(format!(
            "batch of {len} exceeds the {MAX_BATCH} limit"
        )));
    }
    Ok(())
}

/// Bounds a head-carrying message, which `MAX_BATCH` does not cover.
///
/// `GetNodes`/`GetValues` are capped at [`MAX_BATCH`] hashes because a cheap
/// request must not buy expensive work (§12). The head messages need a cap of
/// their own and are the more expensive of the two: bounded only by
/// `MAX_FRAME_LEN` (16 MiB), one `Heads` frame carries on the order of 110 000
/// `SignedHead`s, and each one costs an Ed25519 verification *and* a
/// `head_history` insert — the insert running before the ordering check, so
/// heads that lose the comparison are persisted too. `HeadsWant` is the same
/// shape with a database query per origin. Seconds of CPU and hundreds of
/// thousands of autocommit statements, for 16 MB of upload, repeatable per
/// stream.
///
/// The bound is generous next to any real cluster: §12 sizes membership at
/// N ≤ 100 origins, so a legitimate exchange names tens of heads, not
/// thousands.
/// A backstop behind the decode-time bound, as [`check_batch`] is.
fn check_heads(len: usize, what: &str) -> Result<(), NetError> {
    if len > MAX_HEADS_PER_MESSAGE {
        return Err(NetError::Unexpected(format!(
            "{what} of {len} exceeds the {MAX_HEADS_PER_MESSAGE} limit"
        )));
    }
    Ok(())
}

fn unexpected(wanted: &str, got: &MptMessage) -> NetError {
    NetError::Unexpected(format!("expected {wanted}, got {}", message_name(got)))
}

impl crate::frame::Answer for MptMessage {
    fn into_refusal(self) -> Result<Self, String> {
        match self {
            MptMessage::Error { reason } => Err(reason),
            other => Ok(other),
        }
    }
}

fn message_name(msg: &MptMessage) -> &'static str {
    match msg {
        MptMessage::Hello { .. } => "Hello",
        MptMessage::HeadsWant { .. } => "HeadsWant",
        MptMessage::Heads { .. } => "Heads",
        MptMessage::HeadPush { .. } => "HeadPush",
        MptMessage::GetNodes { .. } => "GetNodes",
        MptMessage::Nodes { .. } => "Nodes",
        MptMessage::GetValues { .. } => "GetValues",
        MptMessage::Values { .. } => "Values",
        MptMessage::FindProviders { .. } => "FindProviders",
        MptMessage::Providers { .. } => "Providers",
        MptMessage::Error { .. } => "Error",
        MptMessage::GetBindings { .. } => "GetBindings",
        MptMessage::BindingsFor { .. } => "BindingsFor",
    }
}

/// A `Nodes` response.
#[derive(Debug, Clone, Default)]
pub struct NodesResponse {
    /// The served nodes.
    pub nodes: Vec<(Hash, Vec<u8>)>,
    /// The hashes the responder did not have.
    pub missing: Vec<Hash>,
    /// The hashes the responder holds and may not show (§5.5).
    ///
    /// A boundary rather than an absence: there is nothing here for this node
    /// ever, so asking again is pointless and treating it as absent would have
    /// the walk retry until its head was abandoned.
    pub redacted: Vec<Hash>,
}

/// A `Values` response.
#[derive(Debug, Clone, Default)]
pub struct ValuesResponse {
    /// The served values.
    pub values: Vec<(Hash, Vec<u8>)>,
    /// The hashes the responder did not have.
    pub missing: Vec<Hash>,
}

/// The outcome of a head-gossip exchange.
#[derive(Debug, Clone, Default)]
pub struct HeadExchange {
    /// The peer's advertised summaries.
    pub summaries: Vec<HeadSummary>,
    /// How many of our heads we pushed.
    pub pushed: usize,
    /// The signed heads the peer sent in response to our want list.
    pub received: Vec<SignedHead>,
    /// What the peer says it will serve us (§5.5).
    pub scope: DeclaredScope,
}

/// A client for the `sync/mpt/1` ALPN, over one established connection.
#[derive(Debug, Clone)]
pub struct MptClient {
    connection: Connection,
    /// How long any one exchange on this connection may wait for its answer.
    deadline: std::time::Duration,
}

impl MptClient {
    /// Wraps an established `sync/mpt/1` connection.
    pub fn new(connection: Connection) -> Self {
        MptClient {
            connection,
            deadline: REQUEST_TIMEOUT,
        }
    }

    /// The same client under a deadline of the caller's choosing, for tests
    /// that need a stall to be reported in milliseconds rather than minutes.
    #[cfg(test)]
    pub(crate) fn with_deadline(mut self, deadline: std::time::Duration) -> Self {
        self.deadline = deadline;
        self
    }

    /// The underlying connection.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// The peer's device key, cryptographically established by the handshake.
    pub fn remote_id(&self) -> synch_core::NodeId {
        self.connection.remote_id()
    }

    /// Runs the five-message head-gossip exchange.
    ///
    /// `decide` is handed the peer's summaries and returns `(heads to push,
    /// origins to pull)`.
    pub async fn head_exchange<F>(
        &self,
        ours: Vec<HeadSummary>,
        declared: DeclaredScope,
        decide: F,
    ) -> Result<HeadExchange, NetError>
    where
        F: FnOnce(&[HeadSummary]) -> (Vec<SignedHead>, Vec<OriginId>),
    {
        under_deadline(self.deadline, "a head exchange", async {
            let (mut send, mut recv) = self.connection.open_bi().await?;
            write_frame(
                &mut send,
                &MptMessage::Hello {
                    proto: PROTO_VERSION,
                    heads: ours,
                    scope: declared,
                },
            )
            .await?;

            let (summaries, scope) = match read_answer::<MptMessage>(&mut recv).await? {
                MptMessage::Hello {
                    proto,
                    heads,
                    scope,
                } => {
                    if proto != PROTO_VERSION {
                        return Err(NetError::Unexpected(format!(
                            "unsupported protocol version {proto}"
                        )));
                    }
                    check_heads(heads.len(), "a Hello summary list")?;
                    (heads, scope)
                }
                other => return Err(unexpected("Hello", &other)),
            };

            let (push, want) = decide(&summaries);
            let pushed = push.len();
            write_frame(&mut send, &MptMessage::Heads { heads: push }).await?;
            write_frame(&mut send, &MptMessage::HeadsWant { origins: want }).await?;

            let received = match read_answer::<MptMessage>(&mut recv).await? {
                MptMessage::Heads { heads } => {
                    // The answer to our own `HeadsWant` is bounded like every
                    // other head-carrying message, and it is the easiest one to
                    // overlook: the responder caps its `Hello`, its `Heads`
                    // push and its `HeadsWant`, and this side caps the `Hello`
                    // it reads back — but a peer could answer a want-list of
                    // one origin with a full frame of heads, and `sync_with`
                    // offers every one of them, at an Ed25519 verification and
                    // a `head_history` insert apiece. Same rule as `Providers`
                    // on this side.
                    check_heads(heads.len(), "a Heads answer")?;
                    heads
                }
                other => return Err(unexpected("Heads", &other)),
            };
            let _ = send.finish();
            Ok(HeadExchange {
                summaries,
                pushed,
                received,
                scope,
            })
        })
        .await
    }

    /// Pushes a head reactively (§5.3).
    pub async fn push_head(&self, head: &SignedHead) -> Result<(), NetError> {
        under_deadline(self.deadline, "a head push", async {
            match exchange(
                &self.connection,
                &MptMessage::HeadPush { head: head.clone() },
            )
            .await?
            {
                MptMessage::Heads { .. } => Ok(()),
                other => Err(unexpected("an acknowledgement", &other)),
            }
        })
        .await
    }

    /// Fetches trie nodes by hash.
    ///
    /// At most [`MAX_BATCH`] hashes: the responder refuses a longer request
    /// outright, so a caller that oversteps loses the whole batch rather than
    /// the tail of it. `MissingWalk::next_batch` is the only caller and stops at
    /// the cap.
    pub async fn get_nodes(
        &self,
        root: Hash,
        wants: &[(Vec<u8>, Hash)],
    ) -> Result<NodesResponse, NetError> {
        debug_assert!(wants.len() <= MAX_BATCH, "a batch past the responder's cap");
        let batch: Vec<(Vec<u8>, Hash)> = wants.to_vec();
        under_deadline(self.deadline, "a trie node request", async {
            match exchange(
                &self.connection,
                &MptMessage::GetNodes { root, wants: batch },
            )
            .await?
            {
                MptMessage::Nodes {
                    nodes,
                    missing,
                    redacted,
                } => Ok(NodesResponse {
                    nodes,
                    missing,
                    redacted,
                }),
                other => Err(unexpected("Nodes", &other)),
            }
        })
        .await
    }

    /// Fetches out-of-line trie values by hash.
    ///
    /// At most [`MAX_BATCH`] hashes, exactly as [`MptClient::get_nodes`].
    pub async fn get_values(
        &self,
        root: Hash,
        wants: &[(Vec<u8>, Hash)],
    ) -> Result<ValuesResponse, NetError> {
        debug_assert!(wants.len() <= MAX_BATCH, "a batch past the responder's cap");
        let batch: Vec<(Vec<u8>, Hash)> = wants.to_vec();
        under_deadline(self.deadline, "a trie value request", async {
            match exchange(
                &self.connection,
                &MptMessage::GetValues { root, wants: batch },
            )
            .await?
            {
                MptMessage::Values { values, missing } => Ok(ValuesResponse { values, missing }),
                other => Err(unexpected("Values", &other)),
            }
        })
        .await
    }

    /// Asks a peer who advertises an object, for bootstrapping a cold cache
    /// that holds no `b:` records for it yet (§6.3).
    pub async fn find_providers(
        &self,
        object_root: Hash,
    ) -> Result<Vec<(OriginId, BlobAd)>, NetError> {
        under_deadline(self.deadline, "a provider hint request", async {
            match exchange(&self.connection, &MptMessage::FindProviders { object_root }).await? {
                MptMessage::Providers { ads } => {
                    if ads.len() > MAX_PROVIDER_ADS {
                        return Err(NetError::Unexpected(format!(
                            "a Providers answer of {} exceeds the {MAX_PROVIDER_ADS} limit",
                            ads.len()
                        )));
                    }
                    Ok(ads)
                }
                other => Err(unexpected("Providers", &other)),
            }
        })
        .await
    }

    /// Asks the peer which device keys it currently holds bound for an origin
    /// (§5.1).
    ///
    /// This is how `synch key ls` answers "have my peers picked up the new
    /// binding yet?" — the judgement §3.4 says a rotation's switch-over needs
    /// and that a node cannot make from its own view of DNS.
    pub async fn get_bindings(&self, origin: &OriginId) -> Result<Vec<NodeId>, NetError> {
        under_deadline(self.deadline, "a binding request", async {
            let request = MptMessage::GetBindings {
                origin: origin.clone(),
            };
            match exchange(&self.connection, &request).await? {
                MptMessage::BindingsFor {
                    origin: answered,
                    keys,
                } if &answered == origin => {
                    // Bounded like every other list off the wire. A `NodeId`
                    // decodes through `VerifyingKey::from_bytes`, an Edwards
                    // point decompression apiece, so an unbounded answer buys
                    // seconds of curve arithmetic for one small request — the
                    // same shape `Providers` is capped against just below.
                    check_heads(keys.len(), "a BindingsFor answer")?;
                    Ok(keys)
                }
                MptMessage::BindingsFor {
                    origin: answered, ..
                } => Err(NetError::Unexpected(format!(
                    "asked about {origin}, answered about {answered}"
                ))),
                other => Err(unexpected("BindingsFor", &other)),
            }
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{bare_endpoint, test_store, trusting_pair, StalledPeer};
    use synch_core::{BlobAd, ALPN_MPT};

    /// How long a test waits before calling a request hung rather than slow.
    const PATIENCE: std::time::Duration = std::time::Duration::from_secs(10);

    /// A sink that contains one origin's heads, the way a *local* fault does,
    /// and takes every other.
    ///
    /// Containment is the sink's: only it can tell "we cannot apply this
    /// origin" from "our disk is full", so the wire relays its verdict and
    /// the exchange goes on.
    #[derive(Debug)]
    struct Picky {
        refuse: OriginId,
        offered: std::sync::Mutex<Vec<OriginId>>,
        serving: std::collections::HashMap<OriginId, SignedHead>,
    }

    impl HeadSink for Picky {
        fn local_summaries(&self) -> Result<Vec<HeadSummary>, NetError> {
            Ok(Vec::new())
        }

        fn observe_summaries_from(
            &self,
            _peer: NodeId,
            _summaries: &[HeadSummary],
            _now: i64,
        ) -> Result<(), NetError> {
            Ok(())
        }

        fn offer_head(&self, head: &SignedHead, _now: i64) -> Result<(), NetError> {
            self.offered
                .lock()
                .expect("the lock")
                .push(head.origin.clone());
            if head.origin == self.refuse {
                // An origin fault, contained by the sink: the exchange goes on.
                return Ok(());
            }
            Ok(())
        }

        fn heads_for(&self, origins: &[OriginId]) -> Result<Vec<SignedHead>, NetError> {
            Ok(origins
                .iter()
                .filter_map(|origin| self.serving.get(origin).cloned())
                .collect())
        }
    }

    /// Every client method carries its own deadline: a peer that keeps the
    /// session open and answers nothing fails the request instead of holding
    /// the caller forever, putting a stalled peer on the same footing as an
    /// unreachable one.
    #[tokio::test]
    async fn a_peer_that_answers_nothing_fails_every_request() {
        macro_rules! stalled {
            ($name:literal, $call:expr) => {
                (
                    $name,
                    tokio::time::timeout(PATIENCE, $call)
                        .await
                        .expect(concat!($name, " must not hang"))
                        .map(|_| ()),
                )
            };
        }
        let peer = StalledPeer::bind(ALPN_MPT).await;
        let dialer = bare_endpoint(ALPN_MPT).await;
        let connection = dialer.connect(peer.addr.clone(), ALPN_MPT).await.unwrap();
        let client =
            MptClient::new(connection).with_deadline(std::time::Duration::from_millis(100));
        let origin = OriginId::named("stalled", "x.example").unwrap();
        let stalled: Vec<(&str, Result<(), NetError>)> = vec![
            stalled!(
                "get_nodes",
                client.get_nodes(Hash::EMPTY, &[(Vec::new(), Hash::new(b"n"))])
            ),
            stalled!(
                "get_values",
                client.get_values(Hash::EMPTY, &[(Vec::new(), Hash::new(b"v"))])
            ),
            stalled!("find_providers", client.find_providers(Hash::new(b"o"))),
            stalled!("get_bindings", client.get_bindings(&origin)),
            stalled!(
                "head_exchange",
                client.head_exchange(Vec::new(), DeclaredScope::Untrusted, |_| (
                    Vec::new(),
                    Vec::new()
                ))
            ),
        ];
        for (what, outcome) in stalled {
            let err = outcome.expect_err(what);
            assert!(err.to_string().contains("went unanswered"), "{what}: {err}");
        }

        dialer.close().await;
        peer.shutdown().await;
    }

    /// The provider handler does not run SQLite on a runtime worker.
    ///
    /// `FindProviders` takes the single global connection mutex and decodes
    /// every row's spans under it; run inline on a busy store it would stall
    /// every timer and connection the worker also drives (§10, §12). Measured
    /// by the runtime's own clock.
    #[tokio::test]
    async fn find_providers_never_blocks_the_runtime_on_the_store() {
        let (_dir, store) = test_store();
        let root = Hash::new(b"an object someone advertises");
        store
            .put_provider(
                &root,
                &OriginId::named("holder", "x.example").unwrap(),
                &BlobAd::complete(1000),
            )
            .unwrap();
        let (server, client, _client_dir) =
            trusting_pair(store.clone(), crate::endpoint::NetOptions::loopback()).await;
        let mpt = client.connect_mpt(server.direct_addr()).await.unwrap();

        // The header of a `FindProviders`, and nothing else yet: the server has
        // accepted the stream, checked the binding, and is waiting on the body.
        let body = postcard::to_stdvec(&MptMessage::FindProviders { object_root: root }).unwrap();
        let (mut send, mut recv) = mpt.connection().open_bi().await.unwrap();
        send.write_all(&(body.len() as u32).to_le_bytes())
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Now a writer takes the one connection, as a publish or a GC pass does.
        const HELD: std::time::Duration = std::time::Duration::from_millis(2_000);
        const PATIENCE: std::time::Duration = std::time::Duration::from_millis(400);
        let busy = store.clone();
        let holding = std::thread::spawn(move || {
            busy.transaction(|_txn| -> Result<(), synch_store::StoreError> {
                std::thread::sleep(HELD);
                Ok(())
            })
            .unwrap();
        });
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        send.write_all(&body).await.unwrap();
        let started = std::time::Instant::now();
        let answer = tokio::time::timeout(PATIENCE, crate::frame::read_bytes(&mut recv)).await;
        assert!(
            answer.is_err(),
            "the answer waits on the store, which is busy"
        );
        assert!(
            started.elapsed() < HELD / 2,
            "the runtime kept its own timers running: {:?}",
            started.elapsed()
        );

        holding.join().unwrap();
        // And once the store is free the answer comes back.
        let ads = tokio::time::timeout(HELD, mpt.find_providers(root))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ads.len(), 1);
        client.shutdown().await.unwrap();
        server.shutdown().await.unwrap();
    }

    /// A holder names itself even when no published `b:` record materialized a
    /// provider row for it. Cloud replicas deliberately aggregate their object
    /// coverage, so this is the route by which a cold node finds the last copy.
    #[tokio::test]
    async fn find_providers_names_the_responder_when_it_holds_the_object() {
        let (_dir, store) = test_store();
        let origin = OriginId::named("cloud-1", "x.example").unwrap();
        store.set_self_origin(&origin).unwrap();
        let payload = b"the last copy is in cloud storage";
        let root = store.ingest_bytes(payload, now_ns()).unwrap();
        assert!(store.providers(&root).unwrap().is_empty());

        let (server, client, _client_dir) =
            trusting_pair(store.clone(), crate::endpoint::NetOptions::loopback()).await;
        let mpt = client.connect_mpt(server.direct_addr()).await.unwrap();

        let ads = mpt.find_providers(root).await.unwrap();
        assert_eq!(ads, vec![(origin, BlobAd::complete(payload.len() as u64))]);

        client.shutdown().await.unwrap();
        server.shutdown().await.unwrap();
    }

    /// A sink that contains an origin fault leaves the exchange free to
    /// finish: every offered head reaches the sink, and the want is still
    /// answered.
    #[tokio::test]
    async fn a_contained_origin_does_not_stop_a_hello_exchange() {
        let (_dir, store) = test_store();
        let signer = iroh_base::SecretKey::generate();
        let bad = OriginId::named("bad", "x.example").unwrap();
        let good = OriginId::named("good", "x.example").unwrap();
        let served = OriginId::named("served", "x.example").unwrap();

        // A head the server can hand back when the exchange gets that far.
        let servable = SignedHead::sign(&signer, served.clone(), 3, Hash::new(b"root"), 0);
        let sink = std::sync::Arc::new(Picky {
            refuse: bad.clone(),
            offered: std::sync::Mutex::new(Vec::new()),
            serving: [(served.clone(), servable.clone())].into_iter().collect(),
        });
        let options = crate::endpoint::NetOptions {
            heads: Some(sink.clone() as std::sync::Arc<dyn HeadSink>),
            ..crate::endpoint::NetOptions::loopback()
        };
        let (server, client, _client_dir) = trusting_pair(store.clone(), options).await;

        let pushed = vec![
            SignedHead::sign(&signer, bad.clone(), 1, Hash::new(b"a"), 0),
            SignedHead::sign(&signer, good.clone(), 1, Hash::new(b"b"), 0),
        ];
        let exchange = client
            .connect_mpt(server.direct_addr())
            .await
            .unwrap()
            .head_exchange(Vec::new(), DeclaredScope::Untrusted, move |_| {
                (pushed, vec![served.clone()])
            })
            .await
            .expect("the exchange completes");

        assert_eq!(
            *sink.offered.lock().unwrap(),
            vec![bad, good],
            "every offered head reaches the sink"
        );
        assert_eq!(
            exchange.received,
            vec![servable],
            "and the want list is still answered"
        );

        client.shutdown().await.unwrap();
        server.shutdown().await.unwrap();
    }

    /// A sink that reports how many requests were ever being served at once.
    ///
    /// The measurement has to be taken inside the handler's own blocking
    /// closure, because that is the resource under discussion: every request
    /// on both ALPNs offloads its store work onto the process's one blocking
    /// pool, and "how many at once" is exactly how much of that pool one
    /// endpoint's peers can hold.
    #[derive(Debug, Default)]
    struct Counting {
        now: std::sync::atomic::AtomicUsize,
        peak: std::sync::atomic::AtomicUsize,
    }

    impl HeadSink for Counting {
        fn local_summaries(&self) -> Result<Vec<HeadSummary>, NetError> {
            use std::sync::atomic::Ordering;
            let now = self.now.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            // Long enough that four requests issued together overlap on any
            // machine, and short enough not to slow the suite down.
            std::thread::sleep(std::time::Duration::from_millis(150));
            self.now.fetch_sub(1, Ordering::SeqCst);
            Ok(Vec::new())
        }

        fn observe_summaries_from(
            &self,
            _peer: NodeId,
            _summaries: &[HeadSummary],
            _now: i64,
        ) -> Result<(), NetError> {
            Ok(())
        }

        fn offer_head(&self, _head: &SignedHead, _now: i64) -> Result<(), NetError> {
            Ok(())
        }

        fn heads_for(&self, _origins: &[OriginId]) -> Result<Vec<SignedHead>, NetError> {
            Ok(Vec::new())
        }
    }

    /// An embedder can cap what one endpoint's peers hold at once, and without
    /// the cap they hold as much as they ask for.
    ///
    /// This is the bound the multi-tenant data plane rests on
    /// (`docs/CLOUD-DATAPLANE.md` §9): one process, one node per hosted
    /// network, one blocking pool between all of them. DESIGN §12 declines
    /// per-peer limits because in a single cluster a peer that can send a
    /// request is a member and abuse is a membership problem — but org A's
    /// membership is not org B's to curate, so an embedder serving both asks
    /// for a ceiling and gets containment that does not depend on anybody's
    /// good behaviour.
    ///
    /// Both halves are asserted, because the cap proves nothing on its own: a
    /// test that only showed "at most one at a time" would pass just as
    /// happily against a responder that never overlapped anything.
    #[tokio::test]
    async fn an_endpoint_wide_cap_bounds_what_one_peer_can_hold() {
        async fn peak_under(limit: Option<usize>) -> usize {
            let (_dir, store) = test_store();
            let sink = std::sync::Arc::new(Counting::default());
            let options = crate::endpoint::NetOptions {
                heads: Some(sink.clone() as std::sync::Arc<dyn HeadSink>),
                max_inflight_requests: limit,
                ..crate::endpoint::NetOptions::loopback()
            };
            let (server, client, _client_dir) = trusting_pair(store, options).await;
            let mpt = client.connect_mpt(server.direct_addr()).await.unwrap();

            // Four at once on one connection, so the only thing that can hold
            // them apart is the endpoint-wide gate: the per-connection cap is
            // eight and never bites here.
            let exchanges: Vec<_> = (0..4)
                .map(|_| {
                    let mpt = mpt.clone();
                    tokio::spawn(async move {
                        let _ = mpt
                            .head_exchange(Vec::new(), DeclaredScope::Untrusted, |_| {
                                (Vec::new(), Vec::new())
                            })
                            .await;
                    })
                })
                .collect();
            for task in exchanges {
                let _ = task.await;
            }
            client.shutdown().await.unwrap();
            server.shutdown().await.unwrap();
            sink.peak.load(std::sync::atomic::Ordering::SeqCst)
        }

        assert!(
            peak_under(None).await > 1,
            "without a ceiling a peer's pipelined requests are served together — \
             which is the behaviour a single-cluster daemon wants, and the \
             behaviour a shared process cannot afford"
        );
        assert_eq!(
            peak_under(Some(1)).await,
            1,
            "with a ceiling of one, one is all a peer can ever hold"
        );
    }

    /// A stream that stalls mid-message does not stop the connection serving
    /// the next request: one task per stream under a semaphore and a deadline
    /// keeps the session usable either way.
    #[tokio::test]
    async fn a_stalled_stream_does_not_hold_the_connection() {
        let (_dir, store) = test_store();
        let (server, client, _client_dir) =
            trusting_pair(store.clone(), crate::endpoint::NetOptions::loopback()).await;
        let mpt = client.connect_mpt(server.direct_addr()).await.unwrap();

        // A frame header promising bytes that never arrive: the responder has
        // accepted the stream and is waiting on the body.
        let (mut stalled, _recv) = mpt.connection().open_bi().await.unwrap();
        stalled.write_all(&64u32.to_le_bytes()).await.unwrap();

        // The next request on the same connection is answered regardless.
        let origin = OriginId::named("nas", "x.example").unwrap();
        let keys = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            mpt.get_bindings(&origin),
        )
        .await
        .expect("a stalled stream must not hold the connection")
        .unwrap();
        assert!(keys.is_empty());

        client.shutdown().await.unwrap();
        server.shutdown().await.unwrap();
    }
    /// A batch answer is bounded in bytes, not only in hashes.
    ///
    /// Two halves. A full legal batch of ceiling-sized values fits one frame and
    /// is served whole — that is what `MAX_TRIE_VALUE_LEN` buys. And a store that
    /// holds something larger than the ceiling, as one written before the ceiling
    /// existed does, still cannot be made to build an answer past the frame: the
    /// budget is applied while the answer is assembled rather than discovered by
    /// `write_frame` after the whole message has been serialized. A short answer
    /// is ordinary — the requester's walk defers what it asked for and re-offers
    /// whatever did not come back.
    #[tokio::test]
    async fn a_values_answer_is_bounded_in_bytes() {
        // Values are authorized by the position of the node carrying them
        // (§5.5), so the request has to name real positions in a real trie —
        // and the wants are produced the way a requester produces them, by
        // walking a store that holds the nodes and not yet the payloads.
        let (_dir, store) = test_store();
        let payload = |len: usize, tag: u64| {
            let mut bytes = vec![0u8; len];
            bytes[..8].copy_from_slice(&tag.to_le_bytes());
            bytes
        };

        // A full batch at the value ceiling, published the ordinary way.
        let mut ceiling_root = Hash::EMPTY;
        for i in 0..MAX_BATCH as u64 {
            ceiling_root = Trie::new(store.as_ref())
                .insert(
                    ceiling_root,
                    &synch_core::file_key("s", &i.to_string()).unwrap(),
                    &payload(synch_core::MAX_TRIE_VALUE_LEN, i),
                )
                .unwrap();
        }

        // And three values from before the ceiling, each half the budget. No
        // `insert` will make one now, which is the point: the nodes are built
        // by hand, exactly as a store written before `MAX_TRIE_VALUE_LEN`
        // existed still holds them.
        let hand_built = |sizes: &[usize], tag: u64| -> Hash {
            let mut children = [None; 16];
            for (i, slot) in children.iter_mut().take(sizes.len()).enumerate() {
                let bytes = payload(sizes[i], tag + i as u64);
                let value = Hash::new(&bytes);
                synch_mpt::NodeStore::put_value(store.as_ref(), &value, &bytes).unwrap();
                let leaf = synch_mpt::TrieNode::Leaf {
                    // Odd, so the branch nibble above it makes a whole number
                    // of bytes and the position names a key that could exist.
                    key_rest: synch_mpt::Nibbles::from_nibbles(&[1, 2, 3]),
                    value: synch_mpt::ValueRef::Hash(value),
                };
                let encoded = leaf.encode();
                let hash = Hash::new(&encoded);
                synch_mpt::NodeStore::put_node(store.as_ref(), &hash, &encoded).unwrap();
                *slot = Some(hash);
            }
            let root_node = synch_mpt::TrieNode::Branch {
                children,
                value: None,
            };
            let encoded = root_node.encode();
            let root = Hash::new(&encoded);
            synch_mpt::NodeStore::put_node(store.as_ref(), &root, &encoded).unwrap();
            root
        };
        let oversized_root = hand_built(&[ANSWER_BYTE_BUDGET / 2; 3], 1_000);
        // And one larger than the whole budget, sitting ahead of three more.
        let crowded_root = hand_built(
            &[
                ANSWER_BYTE_BUDGET + 1,
                ANSWER_BYTE_BUDGET / 2,
                ANSWER_BYTE_BUDGET / 2,
                ANSWER_BYTE_BUDGET / 2,
            ],
            2_000,
        );

        // The positions a requester would name: every node, none of the values.
        let positions = |root: Hash| -> (tempfile::TempDir, Vec<(Vec<u8>, Hash)>) {
            let (dir, bare) = test_store();
            let reachable = Trie::new(store.as_ref()).reachable(root).unwrap();
            for node in &reachable.nodes {
                let bytes = synch_mpt::NodeStore::get_node(store.as_ref(), node)
                    .unwrap()
                    .unwrap();
                synch_mpt::NodeStore::put_node(bare.as_ref(), node, &bytes).unwrap();
            }
            let missing = synch_mpt::MissingWalk::new(root)
                .next_batch(&Trie::new(bare.as_ref()), MAX_BATCH)
                .unwrap();
            assert!(missing.nodes.is_empty(), "every node was copied across");
            (dir, missing.values)
        };
        let (_at_dir, at_ceiling) = positions(ceiling_root);
        let (_over_dir, oversized) = positions(oversized_root);
        let (_crowded_dir, crowded) = positions(crowded_root);
        assert_eq!(at_ceiling.len(), MAX_BATCH);
        assert_eq!(oversized.len(), 3);
        assert_eq!(crowded.len(), 4);

        let (server, client, _client_dir) =
            trusting_pair(store.clone(), crate::endpoint::NetOptions::loopback()).await;
        let mpt = client.connect_mpt(server.direct_addr()).await.unwrap();

        let answer = mpt.get_values(ceiling_root, &at_ceiling).await.unwrap();
        assert_eq!(
            answer.values.len(),
            MAX_BATCH,
            "a full batch at the ceiling is served whole"
        );
        assert!(answer.missing.is_empty());

        let answer = mpt.get_values(oversized_root, &oversized).await.unwrap();
        let bytes: usize = answer.values.iter().map(|(_, v)| v.len()).sum();
        assert!(bytes <= ANSWER_BYTE_BUDGET, "{bytes} bytes served");
        assert!(!answer.values.is_empty(), "and it is not empty");
        assert!(
            answer.values.len() < oversized.len(),
            "the budget bit: {} of {}",
            answer.values.len(),
            oversized.len()
        );
        let asked: Vec<Hash> = oversized.iter().map(|(_, hash)| *hash).collect();
        for (hash, payload) in &answer.values {
            assert_eq!(&Hash::new(payload), hash);
            assert!(asked.contains(hash));
        }

        // A payload larger than the whole budget, asked for ahead of others.
        // It is served — answering nothing would stall the walk forever — but
        // it is the *whole* answer. Letting the next value follow it into the
        // frame is how the budget stops bounding anything: the assembled
        // message goes past `MAX_FRAME_LEN`, `write_frame` refuses it after
        // serializing, and the requester gets an error rather than a short
        // answer, every round, with nothing to advance `unproductive`.
        // Largest first: the walk orders wants by position, and the case is
        // about what follows an over-budget payload into the same frame.
        let mut crowded = crowded;
        crowded.sort_by_key(|(_, hash)| {
            std::cmp::Reverse(
                synch_mpt::NodeStore::get_value(store.as_ref(), hash)
                    .unwrap()
                    .unwrap()
                    .len(),
            )
        });
        let answer = mpt.get_values(crowded_root, &crowded).await.unwrap();
        assert_eq!(
            answer.values.len(),
            1,
            "an over-budget payload is the whole answer"
        );
        assert_eq!(answer.values[0].1.len(), ANSWER_BYTE_BUDGET + 1);

        client.shutdown().await.unwrap();
        server.shutdown().await.unwrap();
    }
}
