//! The `sync/blob/1` ALPN: verified bao slice transfer (§6.4) and the tree
//! proofs delta sync descends with (`docs/DELTA-SYNC.md` §3.1).
//!
//! The blob ALPN carries nothing but `GetSlice`/`SliceEnd`, `GetProof`/
//! `ProofEnd`, and the slice and proof bytes themselves. Both `End` messages
//! report what the provider actually had, which is how the fetcher learns exact
//! availability — span summaries in `BlobAd` are hints, not promises.
//!
//! A proof is the same exchange as a slice with the payload left out, and it is
//! deliberately not a protocol of its own: a provider that can serve a group can
//! prove it, because the bao slice it would have sent already carries every
//! hash on that group's path to the root.

use std::sync::Arc;

use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};
use synch_core::{
    now_ns, proof_nodes_upper_bound, BlobMessage, ChunkRanges, Hash, MAX_PROOF_NODES, MAX_RANGES,
    MAX_SLICE_GROUPS,
};
use synch_store::{Proven, Store};

use crate::{
    endpoint::{under_deadline, REQUEST_TIMEOUT},
    error::NetError,
    frame::{read_answer, read_bytes, read_frame, write_bytes, write_frame},
};

impl crate::frame::Answer for BlobMessage {
    /// The blob protocol has no in-band refusal frame: a provider that cannot
    /// serve answers with empty served ranges, and one that refuses outright
    /// resets the stream. Every frame is therefore its own answer.
    fn into_refusal(self) -> Result<Self, String> {
        Ok(self)
    }
}

/// How many consecutive empty windows a provider may answer with before a
/// fetch gives up on it and lets the caller try someone else.
const MAX_BARREN_WINDOWS: u32 = 4;

/// The window-retirement rule of a windowed fetch, held once because it must
/// hold on both paths — `fetch_into` and `fetch_proof_into` each documented
/// that their copy retires exactly as the other's does.
///
/// A window is retired whether or not the provider served it, and that is
/// what puts a ceiling on the exchange: one round trip per window,
/// `ceil(ranges / window)` of them, rather than one per *group the provider
/// felt like serving*. Retiring only what came back would leave no ceiling at
/// all — a provider answering each request with one valid group is never
/// barren, so [`MAX_BARREN_WINDOWS`] never fires and the deadline is per
/// exchange: the loop would run once per group of the object, millions of
/// times for a large one, and each turn costs the victim disk work and a
/// write-connection transaction (`docs/DELTA-SYNC.md` §3.3).
///
/// Nothing honest needs a second look at a window: a partial holder answers
/// `requested ∩ held` for the whole of it in one exchange, and what it did
/// not hold this time it will not hold on the next ask either. Ranges it left
/// behind stay visible to the caller through the outcome it accumulates, so
/// another provider still gets asked.
struct WindowedWalk {
    remaining: ChunkRanges,
    barren: u32,
}

/// What retiring one window tells the fetch loop to do.
enum WalkStep {
    /// The provider served something: commit it, then take the next window.
    Commit,
    /// An empty answer, not yet enough of them to give up.
    NextWindow,
    /// [`MAX_BARREN_WINDOWS`] consecutive empty answers: a provider that
    /// claims an object and serves none of it must not hold a fetch in an
    /// unbounded walk across the whole thing; the caller has other candidates.
    GiveUp,
}

impl WindowedWalk {
    fn over(ranges: &ChunkRanges) -> WindowedWalk {
        WindowedWalk {
            remaining: ChunkRanges::from_ranges(ranges.ranges.iter().copied()),
            barren: 0,
        }
    }

    /// Retires `window` — either way — given what the provider served in it.
    fn retire(&mut self, window: &ChunkRanges, served: &ChunkRanges) -> WalkStep {
        self.remaining = self.remaining.difference(window);
        if !served.is_empty() {
            self.barren = 0;
            return WalkStep::Commit;
        }
        self.barren += 1;
        match self.barren >= MAX_BARREN_WINDOWS {
            true => WalkStep::GiveUp,
            false => WalkStep::NextWindow,
        }
    }
}

/// The largest prefix of `remaining` whose proof fits one exchange.
///
/// Sized by [`proof_nodes_upper_bound`], so a provider holding everything
/// asked for still comes in under [`MAX_PROOF_NODES`] and never truncates.
/// Ranges are taken whole where they fit and split where they do not, and
/// the count is clamped to [`MAX_RANGES`] so the set operations under it
/// stay cheap on both sides (§12).
///
/// Public because a caller walking a large region in rounds has to cut it the
/// same way: what fits depends on the level and on how fragmented the ranges
/// are — a contiguous run costs one node per subtree plus a root path, a
/// scattered set costs a root path each — so any second answer to "how much
/// per round?" would disagree with this one.
pub fn proof_window(remaining: &ChunkRanges, level: u8) -> ChunkRanges {
    let mut taken: Vec<synch_core::GroupRange> = Vec::new();
    for range in remaining.ranges.iter().take(MAX_RANGES) {
        let candidate = ChunkRanges::from_ranges(taken.iter().copied().chain([*range]));
        if proof_nodes_upper_bound(&candidate, level) <= MAX_PROOF_NODES {
            taken.push(*range);
            continue;
        }
        // This range does not fit whole. Take as much of its head as does,
        // which is at worst nothing — in which case the window is what we have.
        let mut lo = range.start;
        let mut hi = range.end;
        while lo < hi {
            let mid = lo + (hi - lo).div_ceil(2);
            let probe = ChunkRanges::from_ranges(
                taken
                    .iter()
                    .copied()
                    .chain([synch_core::GroupRange::new(range.start, mid)]),
            );
            if proof_nodes_upper_bound(&probe, level) <= MAX_PROOF_NODES {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        if lo > range.start {
            taken.push(synch_core::GroupRange::new(range.start, lo));
        }
        break;
    }
    if taken.is_empty() {
        // Even one group does not fit the budget, which only happens for a
        // degenerate level; ask for a single group so the walk still advances.
        if let Some(first) = remaining.ranges.first() {
            taken.push(synch_core::GroupRange::new(first.start, first.start + 1));
        }
    }
    ChunkRanges::from_ranges(taken)
}

/// Validates what a provider says it served, before any set operation reads it.
///
/// Two bounds, and the request itself supplies both:
///
/// - **Range count.** The provider side rejects a request past [`MAX_RANGES`]
///   because the set operations under it are quadratic in the number of ranges
///   and the asker would not be paying for them (§12). The same holds in
///   reverse: `served` is decoded from a frame, so a provider could answer with
///   a million singleton ranges, and the requester intersects it on a runtime
///   worker.
/// - **Containment.** A provider can only have served what was asked for.
///   Anything outside the request is at best noise the requester would union
///   into its progress, and at worst a claim carried straight into
///   `write_slice` — so the slice path and the proof path both intersect it
///   away here, rather than one of them.
fn check_served(served: ChunkRanges, requested: &ChunkRanges) -> Result<ChunkRanges, NetError> {
    if served.range_count() > MAX_RANGES {
        return Err(NetError::Unexpected(format!(
            "provider claims {} served ranges, past the {MAX_RANGES} limit",
            served.range_count()
        )));
    }
    Ok(served.intersect(requested))
}

/// The `sync/blob/1` protocol handler.
#[derive(Debug, Clone)]
pub(crate) struct BlobProtocol {
    store: Arc<Store>,
    backend: Arc<dyn synch_store::backend::CasBackend>,
    on_unknown_key: Option<Arc<tokio::sync::Notify>>,
    /// The endpoint-wide in-flight gate, shared with every other ALPN mounted
    /// on this endpoint (`crate::serve::Inflight`).
    inflight: crate::serve::Inflight,
}

impl BlobProtocol {
    /// Builds a handler over a store.
    pub(crate) fn new(
        store: Arc<Store>,
        backend: Arc<dyn synch_store::backend::CasBackend>,
    ) -> Self {
        BlobProtocol {
            store,
            backend,
            on_unknown_key: None,
            inflight: None,
        }
    }

    /// Gates this handler on the endpoint-wide in-flight semaphore.
    pub(crate) fn inflight(mut self, gate: crate::serve::Inflight) -> Self {
        self.inflight = gate;
        self
    }

    /// Rings `wake` whenever a connection is refused for an unknown key (§3.4).
    pub(crate) fn on_unknown_key(mut self, wake: Option<Arc<tokio::sync::Notify>>) -> Self {
        self.on_unknown_key = wake;
        self
    }
}

impl ProtocolHandler for BlobProtocol {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let handler = self.clone();
        crate::serve::serve_connection(
            &self.store.clone(),
            connection,
            self.on_unknown_key.as_ref(),
            &self.inflight,
            |_| std::future::ready(()),
            move |peer, mut send, mut recv| {
                let handler = handler.clone();
                async move {
                    if let Err(e) = handler.handle_stream(peer, &mut send, &mut recv).await {
                        tracing::debug!(error = %e, "blob stream ended");
                    }
                    let _ = send.finish();
                }
            },
        )
        .await
    }
}

impl BlobProtocol {
    /// Refuses an object a delegated peer has no granted path to (§3.5).
    ///
    /// `GetSlice` is keyed by object root and carries no space, so
    /// entitlement to the bytes has to be looked up: does any entry in one of
    /// this peer's granted spaces name this content?
    ///
    /// A rooted peer is unrestricted, but finding that out still costs a
    /// bindings read, and this runs once per slice — thousands of times across
    /// one large object. The cheap half of the answer is asked first: a store
    /// holding no delegation at all cannot have a scoped peer, which is the
    /// state of every cluster that does not use the feature.
    async fn check_content_scope(
        &self,
        peer: synch_core::NodeId,
        root: synch_core::Hash,
    ) -> Result<(), NetError> {
        let store = self.store.clone();
        let permitted = crate::blocking::offload(move || {
            if !store.has_delegations()? {
                return Ok(true);
            }
            let (scope, origins) =
                store.publish_scope_of_key_with_origins(&peer, synch_core::now_ns())?;
            let spaces = match scope {
                // A peer with no live binding gets nothing. The dial gate has
                // already refused it, so this is the second lock on the same
                // door rather than the only one — but it is the lock that would
                // matter if a binding lapsed mid-session.
                synch_store::PublishScope::Untrusted => return Ok(false),
                synch_store::PublishScope::Unrestricted => return Ok(true),
                synch_store::PublishScope::Confined(spaces) => spaces,
            };
            Ok(store.content_in_spaces(&root, &spaces, &origins)?)
        })
        .await?;
        match permitted {
            true => Ok(()),
            false => {
                tracing::warn!(
                    peer = %peer.fmt_short(),
                    "refusing content outside the peer's delegated spaces"
                );
                Err(NetError::Unexpected(
                    "requested an object outside this peer's scope".to_string(),
                ))
            }
        }
    }

    async fn handle_stream(
        &self,
        peer: synch_core::NodeId,
        send: &mut iroh::endpoint::SendStream,
        recv: &mut iroh::endpoint::RecvStream,
    ) -> Result<(), NetError> {
        match read_frame::<BlobMessage>(recv).await? {
            BlobMessage::GetSlice { root, ranges } => {
                self.check_content_scope(peer, root).await?;
                // The range set arrives straight off the wire, unnormalized and
                // unbounded: the set operations below it are quadratic in the
                // number of ranges, so a request made of a million singleton
                // ranges costs the provider far more than it costs the asker
                // (§12). Bound it, and normalize before anything else reads it.
                if ranges.range_count() > MAX_RANGES {
                    return Err(NetError::Unexpected(format!(
                        "slice request of {} ranges exceeds the {MAX_RANGES} limit",
                        ranges.range_count()
                    )));
                }
                let ranges = ChunkRanges::from_ranges(ranges.ranges.iter().copied());
                // A provider serves the intersection of what was asked for and
                // what it verifiably holds. Encoding validates the local copy
                // against the root, so a corrupted payload fails here rather
                // than being served.
                //
                // It reads the payload and its outboard off disk to do it, and
                // a window is up to `MAX_SLICE_GROUPS` — so it runs on the
                // blocking pool. Serving one peer's large object must not stop
                // this node's connection tasks from polling (§10).
                let backend = self.backend.clone();
                let (encoded, served) = match backend.encode_slice(root, ranges).await {
                    Ok(pair) => pair,
                    Err(synch_store::StoreError::MissingBlob(_)) => {
                        (Vec::new(), ChunkRanges::empty())
                    }
                    Err(e) => return Err(e.into()),
                };
                write_bytes(send, &encoded).await?;
                write_frame(send, &BlobMessage::SliceEnd { served }).await?;
                Ok(())
            }
            BlobMessage::GetProof {
                root,
                ranges,
                level,
            } => {
                self.check_content_scope(peer, root).await?;
                // Bounded before anything reads it, for the same reason a slice
                // request is: the set operations under it are quadratic in the
                // number of ranges (§12).
                if ranges.range_count() > MAX_RANGES {
                    return Err(NetError::Unexpected(format!(
                        "proof request of {} ranges exceeds the {MAX_RANGES} limit",
                        ranges.range_count()
                    )));
                }
                let ranges = ChunkRanges::from_ranges(ranges.ranges.iter().copied());
                // Cheaper than a slice — a proof of a 16 MiB span is 32 bytes —
                // but it still walks a tree and reads an outboard off disk, and
                // the leaf-level round over a large edit walks a lot of one. It
                // goes to the blocking pool with everything else (§10).
                let backend = self.backend.clone();
                let (encoded, served) = match backend
                    .encode_proof(root, ranges, level, MAX_PROOF_NODES)
                    .await
                {
                    Ok(pair) => pair,
                    Err(synch_store::StoreError::MissingBlob(_)) => {
                        (Vec::new(), ChunkRanges::empty())
                    }
                    Err(e) => return Err(e.into()),
                };
                write_bytes(send, &encoded).await?;
                write_frame(send, &BlobMessage::ProofEnd { served }).await?;
                Ok(())
            }
            BlobMessage::SliceEnd { .. } | BlobMessage::ProofEnd { .. } => Err(
                NetError::Unexpected("an End message is a response, not a request".into()),
            ),
        }
    }
}

/// A client for the `sync/blob/1` ALPN, over one established connection.
#[derive(Debug, Clone)]
pub struct BlobClient {
    connection: Connection,
    /// How long any one exchange on this connection may wait for its answer.
    deadline: std::time::Duration,
}

/// A received slice, together with what the provider actually served.
#[derive(Debug, Clone)]
pub struct Slice {
    /// The bao-encoded slice bytes.
    pub encoded: Vec<u8>,
    /// The ranges the provider had, which is what the encoding covers.
    pub served: ChunkRanges,
}

/// A received proof, together with what the provider actually served.
#[derive(Debug, Clone)]
pub struct Proof {
    /// The pre-order node pairs.
    pub encoded: Vec<u8>,
    /// The ranges the proof covers.
    pub served: ChunkRanges,
}

/// What a run of proof exchanges established (`docs/DELTA-SYNC.md` §3.3).
#[derive(Debug, Clone)]
pub struct ProofOutcome {
    /// The subtrees whose chaining values are now proven against the root, in
    /// tree order — one per group at level 0, one per span higher up, with the
    /// root they were chained to.
    pub proven: Proven,
    /// The ranges they cover, which is what the requester can stop asking for.
    pub served: ChunkRanges,
}

impl BlobClient {
    /// Wraps an established `sync/blob/1` connection.
    pub fn new(connection: Connection) -> Self {
        BlobClient {
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

    /// The peer's device key.
    pub fn remote_id(&self) -> synch_core::NodeId {
        self.connection.remote_id()
    }

    /// Requests a verified slice.
    pub async fn get_slice(&self, root: Hash, ranges: &ChunkRanges) -> Result<Slice, NetError> {
        under_deadline(self.deadline, "a slice request", async {
            let request = BlobMessage::GetSlice {
                root,
                ranges: ranges.clone(),
            };
            let (encoded, end) = self.body_and_end(&request).await?;
            let served = match end {
                BlobMessage::SliceEnd { served } => check_served(served, ranges)?,
                _ => return Err(NetError::Unexpected("expected SliceEnd".into())),
            };
            Ok(Slice { encoded, served })
        })
        .await
    }

    /// Requests the tree over a range, without its bytes.
    pub(crate) async fn get_proof(
        &self,
        root: Hash,
        ranges: &ChunkRanges,
        level: u8,
    ) -> Result<Proof, NetError> {
        under_deadline(self.deadline, "a proof request", async {
            let request = BlobMessage::GetProof {
                root,
                ranges: ranges.clone(),
                level,
            };
            let (encoded, end) = self.body_and_end(&request).await?;
            let served = match end {
                BlobMessage::ProofEnd { served } => check_served(served, ranges)?,
                _ => return Err(NetError::Unexpected("expected ProofEnd".into())),
            };
            Ok(Proof { encoded, served })
        })
        .await
    }

    /// Runs one blob exchange: the request, then the raw body and its End
    /// frame — the answer shape both blob requests share.
    async fn body_and_end(
        &self,
        request: &BlobMessage,
    ) -> Result<(Vec<u8>, BlobMessage), NetError> {
        let mut recv = crate::frame::request(&self.connection, request).await?;
        let encoded = read_bytes(&mut recv).await?;
        let end = read_answer::<BlobMessage>(&mut recv).await?;
        Ok((encoded, end))
    }

    /// Requests the tree over a range and commits it to the local CAS,
    /// verifying every node against the object root before anything is stored.
    ///
    /// The counterpart of [`BlobClient::fetch_into`] for the descent that comes
    /// *before* a fetch (`docs/DELTA-SYNC.md` §3.3): it answers "what does this
    /// object's tree look like here?", and the answer is what lets the caller
    /// discover that most of the object is already on this disk under another
    /// name. A provider serves one window of nodes per exchange, so a large
    /// range is walked window by window; each is verified and committed as it
    /// arrives, so an interrupted descent keeps what it proved.
    ///
    /// Accumulates into `out`, which the caller owns, and reports how the
    /// descent ended.
    ///
    /// The accumulator is caller-owned so a failure part-way through preserves
    /// every `ProvenSubtree` already received for `Store::promote`.
    pub async fn fetch_proof_into(
        &self,
        backend: &Arc<dyn synch_store::backend::CasBackend>,
        root: Hash,
        size: u64,
        ranges: &ChunkRanges,
        level: u8,
        out: &mut ProofOutcome,
    ) -> Result<(), NetError> {
        let mut walk = WindowedWalk::over(ranges);
        while !walk.remaining.is_empty() {
            // The window is the *requester's* to choose, and it is chosen so
            // the provider never has to truncate.
            //
            // Leaving it to the provider — offering the whole remainder and
            // letting `ProofEnd` report how much came back — makes an honest
            // short answer indistinguishable from a provider dribbling one
            // node per round trip, and forces the provider to discard a
            // truncated walk and redo it over the ranges that fit so both
            // sides agree node for node. All of that assumes the split is
            // unpredictable. It is not: the cost of a walk is bounded
            // by its ranges and level, and while the provider walks
            // `requested ∩ what it holds` — which we cannot know — a subset
            // never costs more than the whole. Sizing the window to fit
            // assuming a full holder therefore fits for every holder.
            let window = proof_window(&walk.remaining, level);
            let proof = self.get_proof(root, &window, level).await?;
            // Already clamped to the window by `check_served`.
            let served = proof.served.clone();
            match walk.retire(&window, &served) {
                WalkStep::GiveUp => break,
                WalkStep::NextWindow => continue,
                WalkStep::Commit => {}
            }
            let backend = backend.clone();
            let encoded = proof.encoded;
            let for_store = served.clone();
            // The fold goes over with the write it belongs to. `absorb`
            // deduplicates against everything proven so far, which for a
            // multi-window object is real CPU on whatever thread runs it —
            // and it touches no store connection, so leaving it out here put
            // it somewhere §10's checker cannot see.
            let mut carried = std::mem::replace(&mut out.proven, Proven::none(root, size));
            let proven = backend
                .write_proof(root, size, for_store, level, encoded, now_ns())
                .await?;
            out.proven = crate::blocking::offload(move || {
                carried.absorb(proven)?;
                Ok(carried)
            })
            .await?;
            out.served = out.served.union(&served);
        }
        Ok(())
    }

    /// Requests a slice and commits it to the local CAS, verifying every group
    /// against the object root before anything is stored.
    ///
    /// A provider serves at most [`MAX_SLICE_GROUPS`] groups per exchange, so
    /// anything larger is walked one window at a time until the request is
    /// covered — which is what lets an object bigger than one frame transfer at
    /// all. Each window is committed as it arrives, so an interrupted fetch
    /// keeps everything it verified.
    /// Accumulates into `got`, which the caller owns, so a failure part-way
    /// through does not lose the windows already committed: the groups are in the
    /// bitmap either way, and a caller that had to rediscover that asked another
    /// provider for bytes this node already held and re-decoded them.
    pub async fn fetch_into(
        &self,
        backend: &Arc<dyn synch_store::backend::CasBackend>,
        root: Hash,
        size: u64,
        ranges: &ChunkRanges,
        got: &mut ChunkRanges,
    ) -> Result<(), NetError> {
        let mut walk = WindowedWalk::over(ranges);
        while !walk.remaining.is_empty() {
            let window = walk.remaining.take(MAX_SLICE_GROUPS);
            let slice = self.get_slice(root, &window).await?;
            match walk.retire(&window, &slice.served) {
                WalkStep::GiveUp => break,
                WalkStep::NextWindow => continue,
                WalkStep::Commit => {}
            }
            // Committing a window decodes it against the object root and
            // writes both the sparse payload and its outboard, then fsyncs
            // them before the bitmap advances — the heaviest disk work a fetch
            // does, and it happens once per window. Off the runtime it goes.
            let backend = backend.clone();
            let served = slice.served.clone();
            let encoded = slice.encoded;
            let written = backend
                .write_slice(root, size, served, encoded, now_ns())
                .await?;
            *got = got.union(&written.groups);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{bare_endpoint, test_store, StalledPeer};
    use synch_core::{GroupRange, AD_SPAN_LEVEL, ALPN_BLOB, CHUNK_GROUP_SIZE, MAX_PROOF_NODES};

    /// A peer that keeps the session open and answers nothing fails the
    /// request instead of holding the fetch forever.
    ///
    /// `STREAM_TIMEOUT` bounds what this node does for a peer; the deadline
    /// here bounds what a peer can do to this node, and the windowed fetches
    /// above apply it once per window so a long walk is never cut short for
    /// making steady progress.
    #[tokio::test]
    async fn a_peer_that_answers_nothing_fails_a_slice_and_a_proof() {
        let peer = StalledPeer::bind(ALPN_BLOB).await;
        let dialer = bare_endpoint(ALPN_BLOB).await;
        let connection = dialer.connect(peer.addr.clone(), ALPN_BLOB).await.unwrap();
        let client =
            BlobClient::new(connection).with_deadline(std::time::Duration::from_millis(100));
        let patience = std::time::Duration::from_secs(10);
        let root = Hash::new(b"object");
        let ranges = ChunkRanges::single(0, 4);

        let slice = tokio::time::timeout(patience, client.get_slice(root, &ranges))
            .await
            .expect("a slice request must not hang")
            .expect_err("a stalled peer serves no slice");
        assert!(slice.to_string().contains("went unanswered"), "{slice}");
        let proof = tokio::time::timeout(patience, client.get_proof(root, &ranges, 0))
            .await
            .expect("a proof request must not hang")
            .expect_err("a stalled peer serves no proof");
        assert!(proof.to_string().contains("went unanswered"), "{proof}");

        dialer.close().await;
        peer.shutdown().await;
    }

    /// A provider dribbling one group per answer cannot hold a descent open.
    ///
    /// Retiring only what came back would let a provider serving a valid
    /// proof of a single group per exchange reset the barren counter every
    /// time: `MAX_BARREN_WINDOWS` never fires, and the deadline is per
    /// exchange — one round trip per group of the object, each costing the
    /// victim an outboard write, an fsync and an immediate transaction on its
    /// one write connection. So the whole window is retired either way, on
    /// this path as on the slice path (`docs/DELTA-SYNC.md` §3.3).
    #[tokio::test]
    async fn a_provider_serving_one_group_at_a_time_cannot_stretch_a_descent() {
        let (_provider_dir, provider_store) = test_store();
        // Sixty-four groups, all of which one proof window covers, so the
        // ceiling under test is the only thing that can end the loop.
        let size = 64 * CHUNK_GROUP_SIZE;
        let bytes: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
        let root = provider_store.ingest_bytes(&bytes, now_ns()).unwrap();
        let groups = synch_core::group_count(size);
        assert_eq!(
            proof_window(&ChunkRanges::single(0, groups), 0).count(),
            groups
        );

        // A peer that answers every proof request with the lowest group asked
        // for, and nothing else. Every answer is valid, so nothing else about
        // it looks hostile.
        let endpoint = bare_endpoint(ALPN_BLOB).await;
        let addr = crate::testing::direct_addr(&endpoint);
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let serving = endpoint.clone();
        let counter = asked.clone();
        let store_for_peer = provider_store.clone();
        let peer = tokio::spawn(async move {
            while let Some(incoming) = serving.accept().await {
                let Ok(connection) = incoming.await else {
                    continue;
                };
                while let Ok((mut send, mut recv)) = connection.accept_bi().await {
                    let Ok(request) = read_frame::<BlobMessage>(&mut recv).await else {
                        break;
                    };
                    let BlobMessage::GetProof {
                        root,
                        ranges,
                        level,
                    } = request
                    else {
                        break;
                    };
                    counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let first = ranges.ranges.first().copied().expect("a non-empty request");
                    let one = ChunkRanges::single(first.start, first.start + 1);
                    let (encoded, served) = store_for_peer
                        .encode_proof(&root, &one, level, MAX_PROOF_NODES)
                        .expect("the provider holds the object");
                    write_bytes(&mut send, &encoded).await.unwrap();
                    write_frame(&mut send, &BlobMessage::ProofEnd { served })
                        .await
                        .unwrap();
                    let _ = send.finish();
                }
            }
        });

        let (_fetcher_dir, fetcher) = test_store();
        let fetcher_backend: Arc<dyn synch_store::backend::CasBackend> =
            Arc::new(synch_store::backend::LocalFs::new(fetcher));
        let dialer = bare_endpoint(ALPN_BLOB).await;
        let connection = dialer.connect(addr, ALPN_BLOB).await.unwrap();
        let client = BlobClient::new(connection);
        let mut outcome = ProofOutcome {
            proven: Proven::none(root, size),
            served: ChunkRanges::empty(),
        };
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            client.fetch_proof_into(
                &fetcher_backend,
                root,
                size,
                &ChunkRanges::single(0, groups),
                0,
                &mut outcome,
            ),
        )
        .await
        .expect("the descent must not run for one round trip per group")
        .unwrap();

        // One window was asked for, and one window is what the exchange cost —
        // whatever the provider chose to put in it.
        assert_eq!(asked.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(
            outcome.served.count(),
            1,
            "and what it served is what we got"
        );

        peer.abort();
        dialer.close().await;
        endpoint.close().await;
    }

    /// The requester sizes each window so the provider never truncates.
    ///
    /// A predictable split is what bounds a descent at `ceil(ranges / window)`
    /// exchanges without either side having to signal or compensate for a walk
    /// that overran the frame.
    #[test]
    fn a_proof_window_always_fits_one_exchange() {
        let groups_in = |bytes: u64| bytes / CHUNK_GROUP_SIZE;
        let hundred_gb = ChunkRanges::single(0, groups_in(100_000_000_000));

        // The span-level round of a 100 GB object fits in one exchange whole —
        // that is the property the whole descent rests on.
        let span = proof_window(&hundred_gb, AD_SPAN_LEVEL);
        assert_eq!(span, hundred_gb, "a span round must not be split");
        assert!(proof_nodes_upper_bound(&span, AD_SPAN_LEVEL) <= MAX_PROOF_NODES);

        // The leaf round of the same object does not, so it is split — and each
        // window still fits.
        let leaf = proof_window(&hundred_gb, 0);
        assert!(!leaf.is_empty() && leaf != hundred_gb);
        assert!(proof_nodes_upper_bound(&leaf, 0) <= MAX_PROOF_NODES);

        // Fragmentation costs a root path per range, and the window shrinks to
        // suit rather than overrunning the budget.
        let scattered =
            ChunkRanges::from_ranges((0..1000u64).map(|i| GroupRange::new(i * 1024, i * 1024 + 1)));
        let window = proof_window(&scattered, 0);
        assert!(!window.is_empty());
        assert!(proof_nodes_upper_bound(&window, 0) <= MAX_PROOF_NODES);

        // Degenerate inputs still advance rather than returning nothing.
        assert!(proof_window(&ChunkRanges::empty(), 0).is_empty());
        assert!(!proof_window(&ChunkRanges::single(0, 1), 63).is_empty());
    }
}
