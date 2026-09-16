//! The §5.2 reconciliation algorithm.
//!
//! ```text
//! verify sig(H) under H.signed_by; check H.signed_by is bound to O (else ignore)
//! check (H.seq, H.root) > (local.seq, local.root) lexicographically (else ignore)
//! record H as pending_head(O)                            // durable
//! frontier ← { H.root }
//! while frontier ≠ ∅:
//!     want ← { h ∈ frontier : h ∉ trie_nodes }           // structural sharing
//!     if want = ∅: break
//!     nodes ← GetNodes(want)
//!     verify each node hashes to its requested hash      // reject & disconnect
//!     store nodes; frontier ← their children ∪ value hashes
//! atomically: set complete_head(O) ← H; clear pending
//! re-materialize changed leaves from the node-level diff
//! ```

use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

use synch_core::{now_ns, DeclaredScope, Hash, HeadSummary, OriginId, SignedHead, MAX_BATCH};
use synch_mpt::{Scope, Trie, TrieNode};
use synch_store::{PublishScope, Slot, Store};

use synch_net::{HeadSink, MptClient, NetError};

use crate::error::{EngineError, Result};

/// How many distinct roots one origin may have retained at a single seq.
///
/// Two is what proves equivocation (§4.4), and the proof is the reason those
/// rows are exempt from ordinary retention. Past that the rows add no evidence,
/// so an origin signing at one seq forever would grow every peer's
/// `head_history` without bound. A little headroom over two, so a genuinely
/// confused origin is recorded rather than truncated at the minimum.
///
/// A bound on what is *retained*, never on what is accepted. Refusing a head
/// because the fork is already wide enough would make acceptance depend on
/// arrival order and so destroy the `(seq, root)` maximum §5.2 converges to: an
/// origin signing nine roots at one seq would leave two honest peers holding
/// different heads, according to which of the nine each saw first, each then
/// refusing the other's forever.
/// The cap is applied by evicting the lowest-ordered retained roots at the seq
/// instead ([`synch_store::Txn::trim_forks`]), which is the same set on every
/// peer however the roots arrived.
pub const MAX_RETAINED_FORKS: usize = 8;

/// How many fetch rounds against one peer may make no progress before the
/// pending head is abandoned and head selection re-runs (§5.2).
///
/// Per fetch, not across every advertiser: the counter lives in the loop that
/// asks, so it only ever counts against a peer that was actually asked. A
/// pending head no peer advertises at or above is never fetched at all and so
/// never counted — that case is the maintenance pass's `pending_head_ttl`
/// sweep, and the two together are what §5.2 means by no wedging.
pub const MAX_UNPRODUCTIVE_ROUNDS: u32 = 3;

/// What a promotion attempt concluded.
///
/// Four states rather than a `bool`: a trie still arriving, a memoized refusal,
/// and a head overtaken by the complete slot all mean "did not flip" but require
/// different follow-up work.
///
/// Returned rather than inferred to avoid re-reading the slot after promotion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Promotion {
    /// The pending head's trie was wholly present and the complete slot flipped.
    Flipped,
    /// A pending head stands and its trie is not all here yet: it needs a fetch.
    Waiting,
    /// The head was retired without flipping because this node's own rules
    /// refuse the promotion: the pair is in the refusal memo, the origin has
    /// no live binding here, or it is delegated and its trie publishes
    /// outside its spaces (§3.5).
    Refused,
    /// Nothing is pending now: there was no pending head, or the complete slot
    /// had overtaken the one there and it was dropped.
    ///
    /// Distinct from `Waiting`: only a pending head that still needs its trie
    /// should be reported accepted and wake the anti-entropy loop. An overtake
    /// is possible whenever local `publish` or `activate` moves the complete
    /// slot between the offer's transaction and this one — both derive
    /// their seq from the complete slot alone (§3.4).
    Idle,
}

/// What happened when a head was offered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadOutcome {
    /// The signature did not verify.
    BadSignature,
    /// The signer has no live binding to the claimed origin (§3.1).
    ///
    /// Trie heads whose signing key is not bound to the claimed origin are
    /// ignored even if relayed by a trusted peer.
    Unbound,
    /// Nothing changed for this origin.
    ///
    /// Usually because the head is not strictly greater than what we already
    /// hold. Also the answer when a head *was* adopted and then stopped being
    /// pending before the promotion looked: the complete slot had overtaken it,
    /// or another holder of the write connection promoted it in the gap. Both
    /// leave nothing to fetch, which is what this outcome tells the caller;
    /// distinguishing them would cost a second read of the complete slot on
    /// every adoption, to move a head between two counters.
    NotNewer,
    /// The head was adopted, and this node has already judged that it cannot
    /// materialize this promotion, so the head was retired again rather than
    /// re-judged (see [`Syncer::try_promote`]).
    ///
    /// Counted as a failure for this origin, because it is one: §12 requires the
    /// count of origins left behind to be in the sync report, and this is the
    /// only outcome that carries it after the first exchange.
    Refused,
    /// The head was adopted as pending and its trie must be fetched.
    Pending,
    /// The head was adopted and its trie was already present, so the complete
    /// slot flipped immediately.
    Completed,
}

impl HeadOutcome {
    /// True if the head was adopted in either slot.
    pub fn accepted(&self) -> bool {
        matches!(self, HeadOutcome::Pending | HeadOutcome::Completed)
    }
}

/// What happened when a pending head's trie was fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchOutcome {
    /// The trie is now complete and the head flipped.
    Completed,
    /// The head did not flip and there is nothing more to do with the origin
    /// this exchange.
    ///
    /// There was no pending head to fetch; or the fetch ran and the trie is
    /// still incomplete; or the slot had moved on before the promotion looked.
    /// No caller has ever needed these apart: each means this exchange is
    /// done with the origin, with nothing to report.
    NoFlip,
    /// This node's own rules refused the head, and it was retired: the pair
    /// was already in the refusal memo, so no fetch was made; or the trie
    /// arrived whole and the promotion refused it — no live binding for the
    /// origin, or a delegated origin publishing outside its spaces. The
    /// origin counts as left behind (§12), which is why this is not `NoFlip`.
    Refused,
    /// Every candidate persistently returned `missing`; the pending head was
    /// abandoned and head selection re-runs (§5.2).
    Abandoned,
}

/// Reconciliation over one node's store.
#[derive(Debug, Clone)]
pub struct Syncer {
    store: Arc<Store>,
    /// Rung when a promotion flips a head to complete: the unified tree just
    /// changed, and any checkout materializing it should look again.
    on_change: Option<Arc<tokio::sync::Notify>>,
    /// Rung by the same events as `on_change`, for the replication loop.
    ///
    /// Its own bell rather than a second waiter on `on_change`, because the
    /// bell is `notify_one`: two loops waiting on one permit means each wake
    /// reaches whichever of them happened to be waiting, and the other sleeps
    /// out its interval on a tree that changed.
    on_replica: Option<Arc<tokio::sync::Notify>>,
    /// Rung when a head is adopted as *pending*: its trie is not here and
    /// nothing in this process is going to fetch it until somebody dials.
    on_pending: Option<Arc<tokio::sync::Notify>>,
    /// Promotions this node has tried and cannot repeat, so it does not spend
    /// the diff again. Owned by [`Syncer::try_promote`], which is the only
    /// place the pair a verdict is about is known.
    refused: Arc<Mutex<HashSet<Verdict>>>,
}

/// What a promotion verdict is about: the head, and the root it would be
/// materialized *from*.
///
/// Both, because the verdict is a property of the pair. `materialize_diff`
/// prunes at equal node hashes and pays only for what differs, so a root that
/// outruns the walk's position budget from a far-behind complete slot promotes
/// cleanly once an intermediate head has landed. Keyed on the new root alone
/// this node would refuse that root for the rest of its life while every peer
/// held it — silent per-origin divergence.
type Verdict = (OriginId, u64, Hash, Hash);

/// How many refused promotions are remembered before the set is dropped
/// wholesale.
///
/// None at all in a healthy cluster. Dropped rather than evicted one at a time,
/// like the completeness memo: forgetting costs one more attempt, and no
/// correctness rests on the memory.
const MAX_REFUSED_HEADS: usize = 1024;

/// What one side brings to a `Hello`: its summaries, the scope it will serve
/// the peer, and the heads it can actually hand over.
///
/// Read in one hop off the runtime. Every field is a store read, and §10 puts
/// those on the blocking pool — the exchange itself then needs no store at all.
struct Advertisement {
    summaries: Vec<HeadSummary>,
    declared: DeclaredScope,
    servable: Vec<SignedHead>,
}

impl Syncer {
    /// Binds a syncer to a store.
    pub fn new(store: Arc<Store>) -> Self {
        Syncer {
            store,
            on_change: None,
            on_replica: None,
            on_pending: None,
            refused: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Whether this exact promotion has already been tried and failed.
    fn is_refused(&self, key: &Verdict) -> bool {
        self.refused
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(key)
    }

    /// Remembers that this promotion failed on the origin's own data.
    ///
    /// In memory rather than in the schema: the verdict is about what *this
    /// build* can decode, so an upgraded node should re-attempt it, and a restart
    /// is the cheapest expression of that.
    fn refuse(&self, key: Verdict) {
        let mut refused = self
            .refused
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if refused.len() >= MAX_REFUSED_HEADS {
            refused.clear();
        }
        refused.insert(key);
    }

    /// Rings `wake` whenever a head flips to complete (§5.2).
    ///
    /// Every merge path ends in [`Syncer::try_promote`] — the Hello exchange
    /// in either direction, a pushed head whose trie was already here, a
    /// pending head's completed fetch — so this one bell covers all of them.
    pub(crate) fn on_replica(mut self, wake: Arc<tokio::sync::Notify>) -> Self {
        self.on_replica = Some(wake);
        self
    }

    /// The bell rung when a promotion flips a head to complete.
    pub(crate) fn on_change(mut self, wake: Arc<tokio::sync::Notify>) -> Self {
        self.on_change = Some(wake);
        self
    }

    /// Rings `wake` whenever a head is adopted as pending (§5.3).
    ///
    /// A head that arrives by push names a root this node has never seen, by
    /// construction — that is what makes it worth pushing. The serve side
    /// adopts it into the pending slot and there the matter rested: nothing
    /// scheduled the fetch, so the trie under it arrived only when this node's
    /// own anti-entropy round next happened to dial a peer advertising it
    /// complete, one jittered interval later. What propagated in the sub-second
    /// §5.3 claims for reactive push was a pointer that no reading surface —
    /// `entries`, checkouts, the S3 gateway — looks at, because all of them sit
    /// behind promotion.
    pub(crate) fn on_pending(mut self, wake: Arc<tokio::sync::Notify>) -> Self {
        self.on_pending = Some(wake);
        self
    }

    /// The store this syncer reconciles into.
    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    /// The head summaries this node advertises in `Hello` (§5.1).
    ///
    /// `complete` means "I hold the full trie under this root and can serve
    /// it"; a signed head alone proves nothing about that, so the flag is
    /// computed from the local trie, never assumed.
    pub fn local_summaries(&self) -> Result<Vec<HeadSummary>> {
        let trie = Trie::new(self.store.as_ref());
        let now = now_ns();
        let mut out = Vec::new();
        for stored in self.store.all_heads(Slot::Complete)? {
            let head = stored.head;
            // With provenance for a confined origin (§5.5): a trie of that
            // origin's this node holds whole but was not served as its is not
            // one it can vouch for, and `complete` is the vouching.
            let owner = self.store.provenance_owner(&head.origin, now)?;
            let complete =
                trie.is_complete_scoped_for(owner.as_ref(), head.root, &Scope::full())?;
            out.push(HeadSummary {
                origin: head.origin,
                seq: head.seq,
                root: head.root,
                complete,
            });
        }
        // A pending head is advertised too — as strictly not complete — so a
        // peer learns the newer head exists without being told we can serve it.
        //
        // It is advertised *alongside* the complete head, never in place of it.
        // Collapsing the two slots into one summary per origin erased the fact
        // that we hold the older root whole, and two things read that fact: our
        // own push decision, which only pushes a head whose order key matches
        // the summary it just advertised, and every peer's servability filter,
        // which needs `complete` set to fetch trie nodes from us. A node with
        // any pending head therefore stopped propagating that origin entirely
        // and dropped out of the provider set for a root it could serve.
        for stored in self.store.all_heads(Slot::Pending)? {
            let head = stored.head;
            let already = out
                .iter()
                .any(|s| s.origin == head.origin && (s.seq, s.root.0) == (head.seq, head.root.0));
            if already {
                continue;
            }
            out.push(HeadSummary {
                origin: head.origin,
                seq: head.seq,
                root: head.root,
                complete: false,
            });
        }
        out.sort_by(|a, b| {
            a.origin
                .cmp(&b.origin)
                .then(a.order_key().cmp(&b.order_key()))
        });
        // The wire caps a head-carrying message at `MAX_HEADS_PER_MESSAGE`, and
        // the responder and the dialer both refuse one that overruns it — so a
        // node that built a longer list than it is allowed to send would fail
        // every exchange in both directions, permanently, with nothing to
        // repair it. Heads are never deleted when trust is removed, so the list
        // only grows. §12 sizes membership two orders of magnitude below the
        // cap, so this trims nothing in any real cluster; it is here so the
        // request this node makes is always one it is allowed to make.
        if out.len() > synch_core::MAX_HEADS_PER_MESSAGE {
            tracing::warn!(
                summaries = out.len(),
                cap = synch_core::MAX_HEADS_PER_MESSAGE,
                "more origins than one Hello can carry: advertising the lowest-sorting prefix"
            );
            out.truncate(synch_core::MAX_HEADS_PER_MESSAGE);
        }
        Ok(out)
    }

    /// Records what a peer advertised for *this node's own* origin (§3.4),
    /// and which peer made the claim.
    ///
    /// A node that lost its key and its database holds no head of its own, and
    /// the heads its peers still hold for it are signed by the lost key: no
    /// longer bound, so they can never be accepted as heads (§4.4). Their
    /// existence is what recovery reads, and it is already in every `Hello`
    /// summary — no new wire message, and no unbound signature is trusted here.
    /// Summaries for other origins are ignored: for those, the ordinary
    /// acceptance rule is both sufficient and stricter. Detection rests on
    /// unauthenticated summaries, so the attribution is what lets an operator
    /// judge a claim that holds a node in recovery.
    ///
    /// Returns the highest seq now observed for our origin.
    pub fn observe_summaries_from(
        &self,
        claimed_by: Option<synch_core::NodeId>,
        summaries: &[HeadSummary],
        now: i64,
    ) -> Result<Option<u64>> {
        let Some(own) = self.store.self_origin()? else {
            return Ok(None);
        };
        // The greatest summary for our origin, and only that one. What the row
        // records is a maximum — `record_observed_head` keeps the higher of what
        // is stored and what arrives — so offering it each of a message's
        // summaries in turn wrote the same answer many times over.
        //
        // That was the cost, not the semantics. A `Hello` may carry
        // `MAX_HEADS_PER_MESSAGE` summaries, summaries are unauthenticated by
        // design (§3.4), and our own origin is public — so a peer could set
        // every one of them to it and buy thousands of autocommit writes on the
        // store's single write connection for one message, repeatable per
        // stream. Picking the maximum first makes it one.
        // A seq the store cannot represent is dropped here rather than carried
        // to `record_observed_head`, which refuses it — correctly, because
        // clamping would silently invert every ordering over the column. But a
        // refusal there is an `Err`, and both callers propagate it: the serve
        // side aborts the stream before it has read the peer's push or answered
        // its want, and the dial side aborts before the adoption loop. A
        // summary is an unauthenticated *claim* (§3.4), so an unusable one has
        // to cost the claim and not the exchange — §12's containment rule, the
        // same one `offer_head` and the pending sweep hold to.
        let representable = |s: &&HeadSummary| {
            if s.seq > i64::MAX as u64 {
                tracing::warn!(
                    origin = %own,
                    seq = s.seq,
                    peer = claimed_by.map(|k| k.fmt_short().to_string()).unwrap_or_default(),
                    "a peer claims a head for our own origin at an unrepresentable seq; ignored"
                );
                return false;
            }
            true
        };
        let best = summaries
            .iter()
            .filter(|s| s.origin == own)
            .filter(representable)
            .max_by_key(|s| s.order_key());
        if let Some(summary) = best {
            if self.store.record_observed_head(
                &own,
                summary.seq,
                &summary.root,
                summary.complete,
                claimed_by.as_ref(),
                now,
            )? {
                tracing::info!(
                    origin = %own,
                    seq = summary.seq,
                    peer = claimed_by.map(|k| k.fmt_short().to_string()).unwrap_or_default(),
                    "a peer advertises a head for our own origin"
                );
            }
        }
        Ok(self.store.observed_head(&own)?.map(|o| o.seq))
    }

    /// The full signed heads for the origins a peer asked about (§5.1).
    ///
    /// Only heads this node can back with a *servable trie*: what a peer does
    /// with one is fetch the trie under it from us, so handing over a head
    /// whose trie is not here costs the puller the whole
    /// [`MAX_UNPRODUCTIVE_ROUNDS`] escape and ends with the head abandoned.
    ///
    /// The complete slot is not that claim. A slot says this node promoted the
    /// head; whether the trie under it is whole is what
    /// [`Syncer::local_summaries`] answers with `complete` in the same
    /// exchange, and a delegate's slot holds every foreign origin over a
    /// partial trie by construction. Memoized per root.
    pub fn heads_for(&self, origins: &[OriginId]) -> Result<Vec<SignedHead>> {
        let trie = Trie::new(self.store.as_ref());
        let now = now_ns();
        let mut out = Vec::new();
        for origin in origins {
            let Some(stored) = self.store.head(origin, Slot::Complete)? else {
                continue;
            };
            let owner = self.store.provenance_owner(origin, now)?;
            if !trie.is_complete_scoped_for(owner.as_ref(), stored.head.root, &Scope::full())? {
                tracing::debug!(
                    origin = %origin,
                    "not serving a head whose trie this node does not hold whole"
                );
                continue;
            }
            out.push(stored.head);
        }
        Ok(out)
    }

    /// Offers a head for adoption, applying the full §5.2 acceptance rule.
    pub fn offer_head(&self, head: &SignedHead, now: i64) -> Result<HeadOutcome> {
        // 1. The signature must verify under the key that claims to have made it.
        if head.verify_signature().is_err() {
            return Ok(HeadOutcome::BadSignature);
        }
        // 2. That key must be bound to the claimed origin, right now.
        if !self.store.is_bound(&head.origin, &head.signed_by, now)? {
            return Ok(HeadOutcome::Unbound);
        }
        // The ordering check and the write that acts on it are one transaction.
        // Read-then-write across two lock acquisitions would let two concurrent
        // offers — one per peer connection, all on the blocking pool — both
        // read the same floor, both decide they supersede it, and both write
        // the pending slot, so the lower one clobbers the higher and the higher
        // survives only in `head_history` with nothing to re-drive it.
        // LEAN-MODEL: mpt-offer-pending (MptGc.OfferPending)
        // `MptGc.OfferPending` models the history row and pending slot written
        // here together; history is what places this root in trie GC retention.
        let outcome = self.store.transaction(|txn| -> Result<HeadOutcome> {
            // Verified heads are provable history and fork evidence even
            // when they lose the ordering comparison, so they are retained
            // either way (§4.4).
            txn.record_history(head, now)?;

            // 3. (seq, root) must be strictly greater, lexicographically.
            //    Strictly greater on seq alone would not converge: two
            //    peers receiving different same-seq heads in different
            //    orders would diverge permanently.
            //
            //    Nothing else may stand in front of this comparison. It is
            //    the join that makes head selection order-independent, so a
            //    head that beats the floor reaches the pending slot however
            //    many roots the origin has already signed at this seq — the
            //    width of the fork is a storage question, answered below,
            //    after the ordering has been settled.
            let floor = txn.head_floor(&head.origin)?;
            // Acceptance deliberately does not consult the refusal memo: a
            // verdict is about a *promotion*, which is a pair of roots, and
            // only `try_promote` knows both. A head this node has already
            // failed on is adopted here and retired there, which costs two
            // indexed writes rather than the diff.
            // LEAN-MODEL: mpt-head-adopt (Convergence.adopt)
            // `Convergence.adopt`; `select_eq_of_mem_iff` is why the head a
            // node ends up with depends on which heads it heard, not their
            // order — it needs the total order the NB above insists on.
            let outcome = if head.supersedes(floor.as_ref()) {
                txn.put_head(Slot::Pending, head, now, now)?;
                HeadOutcome::Pending
            } else {
                // LEAN-MODEL: mpt-retain-only (MptGc.Retain)
                // `MptGc.Retain`: the history row above keeps this root in
                // the GC mark set even though no slot ever points at it.
                HeadOutcome::NotNewer
            };

            // Same-seq forks are exempt from `root_retention` until the
            // origin publishes past the forked seq, so a member signing an
            // unlimited number of roots at one seq would otherwise buy
            // permanent growth on every peer, surviving even `trust rm`.
            // Two roots prove the equivocation; past the cap the *lowest*
            // roots at the seq are evicted, which leaves every peer holding
            // the same [`MAX_RETAINED_FORKS`] greatest roots whatever order
            // they arrived in, and never touches the row a slot points at.
            let evicted = txn.trim_forks(&head.origin, head.seq, MAX_RETAINED_FORKS)?;
            if evicted > 0 {
                tracing::warn!(
                    origin = %head.origin,
                    seq = head.seq,
                    evicted,
                    "same-seq forks past the cap evicted: equivocation is already proven"
                );
            }
            Ok(outcome)
        })?;
        if !outcome.accepted() {
            return Ok(outcome);
        }
        match self.try_promote(&head.origin, now)? {
            Promotion::Flipped => Ok(HeadOutcome::Completed),
            // Rung here, not before the promotion. A refused head is adopted and
            // retired in the same breath — adopting it is what makes it beat the
            // floor again — so ringing on adoption woke the anti-entropy loop for
            // a head that no longer exists. `notify_one` keeps a permit and the
            // loop is not parked during a round, so the permit was always waiting
            // when the round ended: one unapplicable head held by one peer pinned
            // the node to back-to-back rounds, forever, at the `REACTIVE_FLOOR`
            // rather than the interval. It also bought a pointless extra round
            // for every head whose trie was already here.
            Promotion::Waiting => {
                if let Some(wake) = &self.on_pending {
                    // One permit no matter how often this rings, exactly as the
                    // promotion bell does — a wake landing mid-round must not be
                    // lost, and the loop is parked on this only between rounds
                    // (§5.3).
                    wake.notify_one();
                }
                Ok(HeadOutcome::Pending)
            }
            Promotion::Refused => Ok(HeadOutcome::Refused),
            // Adopted, and then dropped again by the promotion because the
            // complete slot had overtaken it. Nothing is pending, so this is not
            // an acceptance and there is nothing to fetch.
            Promotion::Idle => Ok(HeadOutcome::NotNewer),
        }
    }

    /// Flips the pending head to complete if its whole trie is present,
    /// re-materializing the derived views from the node-level diff.
    ///
    /// The flip and the materialization are one SQLite transaction (§5.2,
    /// §10): a crash between them would leave `entries` — what the unified
    /// tree, checkouts, and `synch-s3` serve from — missing a promoted head's
    /// delta, with nothing left to say so.
    ///
    /// An origin fault is condemned here, because this is the only place that
    /// knows which head was judged. Both callers arrive with a head from an
    /// earlier snapshot — `offer_head` has committed and released the connection,
    /// and the maintenance sweep is a promotion diff per origin behind its
    /// `all_heads` list — while `HeadPush` writes this slot from the blocking
    /// pool throughout. A caller condemning "its" head therefore retired, and
    /// permanently refused, whatever happened to be in the slot, and left the
    /// head that actually failed unrecorded.
    pub fn try_promote(&self, origin: &OriginId, now: i64) -> Result<Promotion> {
        let read_scope = self.store.local_trie_scope()?;
        let publish_scope = self.store.publish_scope(origin, now)?;
        let owner = self.store.provenance_owner(origin, now)?;
        // What the transaction judged, for the fault arm: it rolls back, so the
        // head cannot be recovered from the slot afterwards.
        let judged: std::cell::RefCell<Option<Verdict>> = std::cell::RefCell::new(None);
        // LEAN-MODEL: mpt-promote (MptGc.Promote)
        // `Safety` pairs `MptGc.Promote` with content materialization: the
        // completeness check, slot flip and derived views share this commit.
        // LEAN-MODEL: cas-remote-promotion (Cas.ReplicaPromote)
        // LEAN-MODEL: cas-ordinary-promotion (Cas.OrdinaryPromote)
        // `Cas.OrdinaryPromote`/`ReplicaPromote` model the entry plus the
        // pin-or-want decision made by `materialize_diff` below.
        let promoted = self.store.transaction(|txn| -> Result<Promotion> {
            let Some(pending) = txn.head(origin, Slot::Pending)? else {
                return Ok(Promotion::Idle);
            };
            // The displaced root, the verdict key and the memo check all come
            // before the completeness walk, because the walk is the most likely
            // thing here to raise and `judged` has to be set before anything
            // that can: `is_complete` descends a peer's node graph and refuses a
            // structurally invalid node, which `is_origin_fault` classifies as
            // the origin's fault — and with `judged` still unset the fault arm
            // retired nothing and remembered nothing, so the head kept the slot
            // until the sweep took it and the next exchange re-adopted it. That
            // is the cycle the memo exists to break, and it was worse than
            // before the memo existed. None of this work depends on
            // completeness, and doing the cheap checks first is free.
            let displaced = txn.complete_head(origin)?;
            let old_root = displaced
                .as_ref()
                .map(|h| h.root)
                .unwrap_or(synch_core::Hash::EMPTY);
            let key: Verdict = (
                pending.head.origin.clone(),
                pending.head.seq,
                pending.head.root,
                old_root,
            );
            // Tried before, from this same root, and failed. Retire it without
            // paying the diff again — leaving it in the slot would hold
            // `head_floor` above everything this node can serve.
            if self.is_refused(&key) {
                txn.clear_head(origin, Slot::Pending)?;
                return Ok(Promotion::Refused);
            }
            *judged.borrow_mut() = Some(key);
            // The pending head must actually beat the complete one, rather than
            // rest on "pending is always greater", an invariant `offer_head`
            // maintains and two other writers do not: `publish` and the key
            // rotation in `activate` both derive their seq from the *complete*
            // slot alone and write it directly, never consulting pending. So a
            // peer relaying an older head of our own origin — signed by a key
            // of ours that is still bound, which is exactly the §3.4 recovery
            // shape — can sit in the pending slot while a local publish moves
            // the complete slot past it, and taking that invariant on trust
            // would install the lesser head and roll `entries` back to it.
            let floor = displaced.as_ref().map(|h| (h.seq, h.root));
            if !pending.head.supersedes(floor.as_ref()) {
                tracing::debug!(
                    origin = %origin,
                    pending = pending.head.seq,
                    complete = displaced.as_ref().map(|h| h.seq).unwrap_or(0),
                    "dropping a pending head the complete slot has overtaken"
                );
                // LEAN-MODEL: mpt-drop-pending (MptGc.DropPending)
                // `MptGc.DropPending` is every clearing of the pending slot
                // that does not flip it: this one, the refusal above,
                // `sweep_pending_heads`, and a read-scope change. The root
                // stays retained through `head_history` until pruned.
                txn.clear_head(origin, Slot::Pending)?;
                return Ok(Promotion::Idle);
            }
            // Only walk after the cheap `(seq, root)` supersession check. This
            // avoids advertising, checking, and fetching a pending root that the
            // complete slot has already overtaken. On a restore-from-backup,
            // where that root shares nothing with the
            // current one, the last of those is a whole cold trie transfer thrown
            // away.
            let trie = Trie::new(txn);
            // Completeness is a property of a root *and* a scope: what has to
            // be present is what this node may read, not what the origin
            // published (§5.5).
            // And with provenance for a confined origin: what has to be
            // present is what this node was served as that origin's, not
            // what it happens to hold from anyone's trie (§5.5).
            // LEAN-MODEL: mpt-complete-owned-promote (Provenance.confined_head_vouched)
            // `Provenance.confined_head_vouched`: a member vouches for a
            // confined origin's head only if every node under it is one that
            // origin legitimately held.
            if !trie.is_complete_scoped_for(owner.as_ref(), pending.head.root, &read_scope)? {
                return Ok(Promotion::Waiting);
            }
            // A delegated origin's trie must hold nothing outside the spaces
            // it was delegated, and a head whose trie does is refused whole
            // rather than materialized in part (§3.5): a filtered subset would
            // leave this node's contents disagreeing with the root the origin
            // signed, and every peer filtering independently would disagree
            // with every other. The origin stalls, `doctor` names the key, and
            // no other origin is affected.
            //
            // Below `supersedes` for the reason the walk above is: this
            // descends the trie too, and a head the complete slot has already
            // overtaken should pay for neither.
            //
            // Cheap despite reading like a scan: the check descends only where
            // the boundary is unresolved, so it visits the spine and stops.
            //
            // Either refusal *retires* the head rather than leaving it to
            // wait. Its trie is wholly here, so nothing else ever would: the
            // sweep's staleness rule steps over a complete trie, and §5.2 is
            // explicit that a head whose promotion this node's own rules
            // refuse never keeps the slot — left in place it holds
            // `head_floor` above every lesser head this node could serve, for
            // as long as the origin does not publish past it. The verdict is
            // not memoized: it is about the grant, which can move, and
            // re-reaching it costs a spine walk, not a diff.
            match &publish_scope {
                // An origin this node holds no live binding for publishes
                // nothing it will promote. Not merely a scope question: were
                // this to fall through as "unrestricted", revoking a
                // delegation would *promote* the head its scope had been
                // refusing, which is the opposite of what revoking is for.
                // Nothing is lost by dropping it: a head re-offered once the
                // binding is back is re-adopted over a trie already here.
                PublishScope::Untrusted => {
                    tracing::debug!(
                        origin = %origin,
                        seq = pending.head.seq,
                        "retiring a pending head: no live binding for this origin"
                    );
                    txn.clear_head(origin, Slot::Pending)?;
                    return Ok(Promotion::Refused);
                }
                PublishScope::Unrestricted => {}
                PublishScope::Confined(spaces) => {
                    let scope = Scope::of(&synch_core::publish_prefixes(spaces));
                    if let Some(key) = trie.first_key_outside(pending.head.root, &scope)? {
                        tracing::warn!(
                            origin = %origin,
                            seq = pending.head.seq,
                            key = %synch_mpt::Nibbles::from_nibbles(&key)
                                .to_bytes()
                                .map(|b| String::from_utf8_lossy(&b).into_owned())
                                .unwrap_or_else(|| "<partial>".to_string()),
                            "refusing a delegated origin's head: it publishes outside its spaces"
                        );
                        txn.clear_head(origin, Slot::Pending)?;
                        return Ok(Promotion::Refused);
                    }
                }
            }
            // The displaced head is already retained: `put_head` recorded its
            // signature when it took the slot. Recording it again here would be
            // a second rule writing the same row, kept honest only by
            // `INSERT OR IGNORE` (§10, v11).
            // LEAN-MODEL: mpt-supersede (MptGc.Supersede)
            // The displaced root is no longer active or materialized; it
            // stays retained through `head_history` until pruned.
            txn.put_head(Slot::Complete, &pending.head, pending.received_at, now)?;
            txn.clear_head(origin, Slot::Pending)?;
            // LEAN-MODEL: mpt-materialize-scoped (Convergence.ScopedView)
            // `Convergence.ScopedView`: what this derives is a function of the
            // root and the read scope alone (`scoped_view_deterministic`), and
            // every admitted key is readable here (`admitted_key_readable`).
            txn.materialize_diff(origin, old_root, pending.head.root)?;
            Ok(Promotion::Flipped)
        });
        let promoted = match promoted {
            Ok(promoted) => promoted,
            Err(e) if is_origin_fault(&e) => {
                if let Some(key) = judged.into_inner() {
                    // Retire it, and do not try this pair again. The retire
                    // stops it holding the floor; the memo stops the
                    // adopt/diff/fail cycle repeating once per exchange.
                    let (_, seq, root, _) = key.clone();
                    self.refuse(key);
                    self.store
                        .clear_head_at(origin, Slot::Pending, seq, &root)?;
                    tracing::warn!(
                        origin = %origin,
                        seq,
                        error = %e,
                        "origin left behind: a head this node cannot materialize \
                         does not hold the floor"
                    );
                }
                return Err(e);
            }
            Err(e) => return Err(e),
        };
        if promoted == Promotion::Flipped {
            tracing::debug!(origin = %origin, "head flipped to complete");
            // One permit no matter how often this rings: passes coalesce,
            // and a wake landing mid-pass is not lost.
            for wake in [&self.on_change, &self.on_replica].into_iter().flatten() {
                wake.notify_one();
            }
        }
        Ok(promoted)
    }

    /// Fetches the pending head's trie from `client`, verifying every node
    /// against the hash it was requested by.
    ///
    /// Nodes are content-addressed, so `client` need not be the origin, nor
    /// even the peer that told us about the head: any peer advertising a
    /// complete head for the origin at or above this seq will do.
    pub async fn fetch_pending(
        &self,
        client: &MptClient,
        origin: &OriginId,
    ) -> Result<FetchOutcome> {
        let Some(pending) = ({
            let store = self.store.clone();
            let origin = origin.clone();
            crate::blocking::offload(move || Ok(store.pending_head(&origin)?)).await?
        }) else {
            return Ok(FetchOutcome::NoFlip);
        };
        // What this origin's trie looked like when we last held all of it. Every
        // subtree the new root shares with it is already here, so the walk can
        // skip it outright and descend only what changed (§5.2) — which is the
        // difference between an incremental sync costing the change and costing
        // the whole tree. Only a root we have *established* is complete will do:
        // that check is memoized, so it is a lookup after the first time.
        //
        // Establishing it the first time is a full walk, so it goes off the
        // runtime — as does every batch of the walk below, and every batch of
        // nodes and values committed between the round trips (§10).
        //
        // The scope comes back with it: everything below is confined to what
        // this node may read (§5.5) — for a rooted member the whole keyspace
        // and the walk is unchanged, for a delegated one a stop at the boundary
        // rather than a request it would be refused — and reading it is a store
        // read, which belongs on the same hop rather than on the runtime.
        let (reference, scope, held, owner) = {
            let store = self.store.clone();
            let origin = origin.clone();
            crate::blocking::offload(move || {
                let trie = Trie::new(store.as_ref());
                let scope = store.local_trie_scope()?;
                let held = store.complete_head(&origin)?;
                // Whose provenance the walk carries: a confined origin's trie
                // is fetched as *its*, so a node this store already holds from
                // another origin's trie is asked for again, and the reference
                // must be complete with the same provenance (§5.5).
                let owner = store.provenance_owner(&origin, now_ns())?;
                let reference = match &held {
                    // "Held whole" means held whole *within this scope*: the
                    // walk never commits part of a subtree it is inside, so
                    // every boundary it holds is a scope edge and pruning
                    // against it stays sound.
                    // LEAN-MODEL: mpt-fetch-reference (ScopedSync.prune_sound_paired)
                    // `ScopedSync.prune_sound_paired`: the reference's
                    // `CompleteWithin` premise is established here, over the
                    // same provenance the walk below reads presence with.
                    Some(head)
                        if trie.is_complete_scoped_for(owner.as_ref(), head.root, &scope)? =>
                    {
                        Some(head.root)
                    }
                    _ => None,
                };
                Ok((reference, scope, held, owner))
            })
            .await?
        };
        // The same memo `try_promote` keeps, consulted before the walk. A
        // trie this build has refused a node of is refused again on every
        // fetch, so retrying costs the walk and the batch each exchange, from
        // every peer, forever, with nothing ever banked — while the head is
        // re-adopted each time because retiring it drops the floor back to
        // what this node can serve. Keyed with the complete root like a
        // promotion verdict, which is conservative: the refusal is about the
        // trie, and re-trying it once the held root moves costs one fetch.
        let verdict: Verdict = (
            origin.clone(),
            pending.seq,
            pending.root,
            held.as_ref()
                .map(|h| h.root)
                .unwrap_or(synch_core::Hash::EMPTY),
        );
        if self.is_refused(&verdict) {
            let store = self.store.clone();
            let (origin, seq, root) = (origin.clone(), pending.seq, pending.root);
            crate::blocking::offload(move || {
                Ok(store.clear_head_at(&origin, Slot::Pending, seq, &root)?)
            })
            .await?;
            return Ok(FetchOutcome::Refused);
        }
        let mut walk = synch_mpt::MissingWalk::for_origin(
            owner.clone(),
            reference,
            pending.root,
            scope.clone(),
        );
        let mut unproductive = 0u32;
        loop {
            // One walk across the whole fetch, resumed rather than restarted:
            // beginning again at the root for every batch makes a cold fetch
            // re-descend everything it has already pulled, once per batch. The
            // walk travels into the blocking pool and back so its position
            // survives each round trip.
            let store = self.store.clone();
            let (missing, returned) = crate::blocking::offload(move || {
                let trie = Trie::new(store.as_ref());
                let missing = walk.next_batch(&trie, MAX_BATCH)?;
                Ok((missing, walk))
            })
            .await?;
            walk = returned;
            if missing.is_empty() {
                if walk.is_exhausted() {
                    break;
                }
                walk.resume();
                continue;
            }

            let mut learned = 0usize;
            if !missing.nodes.is_empty() {
                let response = client.get_nodes(pending.root, &missing.nodes).await?;
                let store = self.store.clone();
                let requested: Vec<Hash> = missing.nodes.iter().map(|(_, hash)| *hash).collect();
                // A boundary the peer reported is recorded before anything
                // else, and counts as progress: the walk then treats it as
                // satisfied rather than absent, so the fetch converges instead
                // of retrying to the §5.2 abandonment clause (§5.5).
                //
                // Recorded against the *positions* this round asked the hash
                // at, never against the bare hash — the same rule
                // `take_served` applies to `nodes` and `values`, and for the
                // same reason. Unfiltered, a peer that answers every request
                // with one arbitrary hash in `redacted` resets `unproductive`
                // every round, so the abandonment clause never fires and the
                // fetch never ends. And a refusal is about where a node sits:
                // one node can stand at two spine positions and be refused at
                // only one of them, so the responder judges every position and
                // a hash that came back served, or absent somewhere, was not
                // refused outright and is not a boundary at all this round.
                // Only spine positions are recorded, because those are the only
                // ones the walk consults the memo for.
                let served: std::collections::HashSet<Hash> =
                    response.nodes.iter().map(|(hash, _)| *hash).collect();
                let absent: std::collections::HashSet<Hash> =
                    response.missing.iter().copied().collect();
                let refused: std::collections::HashSet<Hash> = response
                    .redacted
                    .iter()
                    .copied()
                    .filter(|hash| !served.contains(hash) && !absent.contains(hash))
                    .collect();
                let boundary: Vec<(Vec<u8>, Hash)> = missing
                    .nodes
                    .iter()
                    .filter(|(path, hash)| refused.contains(hash) && !scope.contains_subtree(path))
                    .cloned()
                    .collect();
                learned += boundary
                    .iter()
                    .map(|(_, hash)| *hash)
                    .collect::<std::collections::HashSet<Hash>>()
                    .len();
                if !boundary.is_empty() {
                    let store = self.store.clone();
                    crate::blocking::offload(move || {
                        // One transaction, for the reason the node batch below
                        // is one: a row per autocommit statement is a write
                        // connection and a WAL frame per boundary.
                        store.transaction(|txn| -> Result<()> {
                            // LEAN-MODEL: mpt-learn-scoped (ScopedSync.Learn)
                            // `ScopedSync.Learn`: nodes, values and refusals
                            // enter a delegate's store from the responder
                            // alone; `reachable_confined` is what that buys.
                            for (path, hash) in &boundary {
                                synch_mpt::NodeStore::note_redacted(txn, hash, path)?;
                            }
                            Ok(())
                        })
                    })
                    .await?;
                }
                let owner = owner.clone();
                let stored = crate::blocking::offload(move || {
                    // One transaction per batch, not one per node. Written
                    // through the `Store`, each `put_node` is a bare `execute`
                    // in autocommit — its own transaction, its own acquisition
                    // of the one write connection, its own WAL frame — so a
                    // `MAX_BATCH` response cost up to 256 of them, and a cold
                    // bootstrap of an n-node trie cost n. Measured at 3.8x on
                    // 10 240 puts. The batch is also atomic this way, which is
                    // what §10 asks of a multi-step write; nothing is lost by a
                    // rollback either, since trie nodes are content-addressed
                    // and simply re-fetched.
                    // LEAN-MODEL: mpt-fetch-batch (MptGc.LearnBatch)
                    // `MptGc.LearnBatch` abstracts the transaction that makes a
                    // connected verified batch visible; only the last may close
                    // the root and make `complete` true.
                    store.transaction(|txn| {
                        take_served(
                            &requested,
                            &response.nodes,
                            "node",
                            verify_node,
                            |hash, bytes| {
                                synch_mpt::NodeStore::put_node(txn, hash, bytes)?;
                                // Served under this origin's root by a peer
                                // vouching for it: that is what provenance
                                // records, in the same transaction as the
                                // node (§5.5).
                                // LEAN-MODEL: mpt-learn-owned (Provenance.learn)
                                // `Provenance.Step.learn` writes `held` and
                                // `owned` together.
                                if let Some(origin) = &owner {
                                    synch_mpt::NodeStore::note_owned(txn, origin, hash)?;
                                }
                                Ok(true)
                            },
                        )
                    })
                })
                .await;
                learned += match stored {
                    Ok(stored) => stored,
                    // The origin published a node this build refuses (§12).
                    // The batch rolled back, and the head is retired by name
                    // here rather than left for the sweep: it holds
                    // `head_floor` above everything this node can serve, and
                    // its trie can never complete — the refused node is never
                    // stored, so every later fetch would meet it again. The
                    // verdict is remembered so the next exchange does not
                    // re-adopt and re-fetch it, and the fault propagates so
                    // this exchange counts the origin as left behind and
                    // carries on with every other.
                    Err(e) if is_origin_fault(&e) => {
                        tracing::warn!(
                            origin = %origin,
                            seq = pending.seq,
                            error = %e,
                            "abandoning pending head: the origin published a trie node this node refuses"
                        );
                        self.refuse(verdict.clone());
                        let store = self.store.clone();
                        let (origin, seq, root) = (origin.clone(), pending.seq, pending.root);
                        crate::blocking::offload(move || {
                            Ok(store.clear_head_at(&origin, Slot::Pending, seq, &root)?)
                        })
                        .await?;
                        return Err(e);
                    }
                    Err(e) => return Err(e),
                };
            }
            if !missing.values.is_empty() {
                let response = client.get_values(pending.root, &missing.values).await?;
                let store = self.store.clone();
                let requested: Vec<Hash> = missing.values.iter().map(|(_, hash)| *hash).collect();
                learned += crate::blocking::offload(move || {
                    // One transaction per batch, as for nodes above.
                    store.transaction(|txn| {
                        take_served(
                            &requested,
                            &response.values,
                            "value",
                            verify_value,
                            |hash, bytes| {
                                // Two bounds on what an origin may put in a
                                // value, and both are refusals of *that origin's*
                                // data rather than of the peer relaying it — the
                                // serving peer sent exactly what it was asked for
                                // (§12).
                                //
                                // A value small enough to be inline must *be*
                                // inline. `ValueRef::for_value` makes that true of
                                // everything this node builds, and nothing made it
                                // true of what arrives: `check_invariants` rejects
                                // an oversized inline value and had no rule the
                                // other way, because the payload is not in the
                                // node. This is the first place both are in hand.
                                // Left unchecked it is a second root for the same
                                // key/value map — the thing structural sharing and
                                // the reference-pruning walk rest on not happening
                                // — plus an extra round trip and an extra
                                // `trie_values` row per leaf, at the publisher's
                                // choosing.
                                //
                                // And a value has an upper bound at last
                                // (`MAX_TRIE_VALUE_LEN`): the key side was bounded
                                // three ways and this side by the frame alone, at
                                // 16 MiB each with no limit on how many, which is
                                // what let one small trie cost every peer
                                // gigabytes to serve and terabytes to materialize.
                                //
                                // *Refused*, not raised. Returning an error here
                                // rolled the whole batch back — losing the
                                // legitimate values in it — and propagated out of
                                // `fetch_pending` through `?`, so `learned == 0`
                                // was never reached, `unproductive` never advanced,
                                // and the `MAX_UNPRODUCTIVE_ROUNDS` escape the
                                // comment claimed could not fire for this fault at
                                // all: the head sat holding `head_floor` until the
                                // `pending_head_ttl` sweep took it, thirty times
                                // longer. Skipping the value leaves the walk asking
                                // for it and the counter counting, which is what
                                // the rule was always meant to do.
                                if bytes.len() <= synch_core::INLINE_VALUE_MAX {
                                    tracing::warn!(
                                        %hash,
                                        len = bytes.len(),
                                        ceiling = synch_core::INLINE_VALUE_MAX,
                                        "refusing an out-of-line value small enough to be inline"
                                    );
                                    return Ok(false);
                                }
                                if bytes.len() > synch_core::MAX_TRIE_VALUE_LEN {
                                    tracing::warn!(
                                        %hash,
                                        len = bytes.len(),
                                        ceiling = synch_core::MAX_TRIE_VALUE_LEN,
                                        "refusing a trie value past the size ceiling"
                                    );
                                    return Ok(false);
                                }
                                synch_mpt::NodeStore::put_value(txn, hash, bytes)?;
                                Ok(true)
                            },
                        )
                    })
                })
                .await?;
            }

            // LEAN-MODEL: mpt-fetch-progress (Convergence.FetchStep)
            // `Convergence.FetchStep`: a productive round learns an item the
            // finite trie bounds, so the fetch terminates
            // (`fetch_terminates`); a round that can learn nothing more from a
            // peer holding the head has a complete trie (`stuck_complete`).
            if learned == 0 {
                unproductive += 1;
                if unproductive >= MAX_UNPRODUCTIVE_ROUNDS {
                    // No wedging on unservable heads: abandon the pending head
                    // and let head selection re-run. Structural sharing makes
                    // the restart cost proportional to what actually changed.
                    tracing::warn!(
                        origin = %origin,
                        seq = pending.seq,
                        "abandoning pending head: providers persistently missing nodes"
                    );
                    // The head this fetch judged, not whatever is in the slot
                    // now: a `HeadPush` accepted while we were between round
                    // trips would otherwise be deleted by a verdict that was
                    // never about it.
                    let store = self.store.clone();
                    let (origin, seq, root) = (origin.clone(), pending.seq, pending.root);
                    let dropped = crate::blocking::offload(move || {
                        Ok(store.clear_head_at(&origin, Slot::Pending, seq, &root)?)
                    })
                    .await?;
                    if !dropped {
                        tracing::debug!(
                            "a newer head arrived while this one was being fetched; \
                             leaving the slot to it"
                        );
                    }
                    return Ok(FetchOutcome::Abandoned);
                }
            } else {
                unproductive = 0;
                // Progress restarts the slot's staleness clock. The clock is on
                // the slot rather than on the head occupying it (see
                // `put_head_in`), so without this a trie that legitimately
                // takes longer than `pending_head_ttl` to fetch would be swept
                // out from under the fetch that is filling it.
                //
                // Named, because this fetch has been working on `pending.root`
                // since before the first round trip and the slot may have moved
                // on since: stamping whatever is there would let progress on
                // this root hold the sweep off a head nobody can serve.
                let store = self.store.clone();
                let touched = origin.clone();
                let (seq, root) = (pending.seq, pending.root);
                crate::blocking::offload(move || {
                    Ok(store.touch_pending_at(&touched, seq, &root, now_ns())?)
                })
                .await?;
            }
            // Everything just stored goes back on the frontier, so the nodes
            // that arrived expand into their own children.
            walk.resume();
        }

        // The walk drained with nothing missing, which *is* the answer to "do I
        // hold all of this?" — so record it rather than let the promotion below
        // and the next `Hello` each rediscover it by walking the trie again.
        // The promotion that follows re-materializes every changed leaf in one
        // transaction, so the pair stays off the runtime like the rest.
        let store = self.store.clone();
        let syncer = self.clone();
        let origin = origin.clone();
        let promoted = crate::blocking::offload(move || {
            // LEAN-MODEL: mpt-complete-memo (ScopedSync.prune_sound)
            // `ScopedSync.prune_sound`: a walk that pruned against the
            // reference above still establishes `CompleteWithin` for this root.
            synch_mpt::NodeStore::note_complete(
                store.as_ref(),
                &scope.memo_key_for(owner.as_ref(), pending.root),
            )?;
            syncer.try_promote(&origin, now_ns())
        })
        .await?;
        Ok(match promoted {
            Promotion::Flipped => FetchOutcome::Completed,
            Promotion::Refused => FetchOutcome::Refused,
            Promotion::Waiting | Promotion::Idle => FetchOutcome::NoFlip,
        })
    }

    /// Runs a `Hello` exchange that pushes and pulls nothing, purely to read
    /// the peer's summaries (§3.4 step 2).
    ///
    /// This is what the recovery quiesce collects with. It is the ordinary
    /// exchange with an empty decision, so a recovering node learns how far
    /// peers say its origin had got without adopting anything.
    pub(crate) async fn observe_with(&self, client: &MptClient) -> Result<Vec<HeadSummary>> {
        let ours = self.summaries_off_runtime(client.remote_id()).await?;
        let exchange = client
            .head_exchange(ours.summaries, ours.declared, |_theirs| {
                (Vec::new(), Vec::new())
            })
            .await?;
        let syncer = self.clone();
        let peer = client.remote_id();
        let summaries = exchange.summaries.clone();
        crate::blocking::offload(move || {
            syncer.observe_summaries_from(Some(peer), &summaries, now_ns())
        })
        .await?;
        Ok(exchange.summaries)
    }

    /// Fetches and adopts a peer-retained head for this node's own origin,
    /// provided it was signed by a device key this database still holds.
    ///
    /// This is restore readoption, not key-loss recovery: a Litestream restore
    /// may be behind while retaining every signing key, so the peer's full head
    /// is independently verifiable and can safely replace the restored one.
    pub(crate) async fn readopt_self_with(
        &self,
        client: &MptClient,
        held_keys: &std::collections::HashSet<synch_core::NodeId>,
    ) -> Result<()> {
        let Advertisement {
            summaries: ours,
            declared,
            ..
        } = self.advertisement_off_runtime(client.remote_id()).await?;
        let own = {
            let store = self.store.clone();
            crate::blocking::offload(move || {
                store
                    .self_origin()?
                    .ok_or_else(|| EngineError::invalid("the node has no self origin"))
            })
            .await?
        };
        let wanted = own.clone();
        let exchange = client
            .head_exchange(ours, declared, move |summaries| {
                let peer_has_own = summaries.iter().any(|summary| summary.origin == wanted);
                (
                    Vec::new(),
                    peer_has_own.then_some(wanted.clone()).into_iter().collect(),
                )
            })
            .await?;

        {
            let syncer = self.clone();
            let scope = exchange.scope.clone();
            let summaries = exchange.summaries.clone();
            let peer = client.remote_id();
            crate::blocking::offload(move || syncer.adopt_scope(peer, &scope, &summaries)).await?;
        }
        {
            let syncer = self.clone();
            let summaries = exchange.summaries.clone();
            let peer = client.remote_id();
            crate::blocking::offload(move || {
                syncer.observe_summaries_from(Some(peer), &summaries, now_ns())
            })
            .await?;
        }

        for head in exchange.received {
            if head.origin != own || !held_keys.contains(&head.signed_by) {
                continue;
            }
            match self.offer_head_off_runtime(&head).await? {
                HeadOutcome::Pending => {
                    let _ = self.fetch_pending(client, &own).await?;
                }
                HeadOutcome::Completed
                | HeadOutcome::NotNewer
                | HeadOutcome::BadSignature
                | HeadOutcome::Unbound
                | HeadOutcome::Refused => {}
            }
        }
        Ok(())
    }

    /// [`Syncer::offer_head`] on the blocking pool.
    async fn offer_head_off_runtime(&self, head: &SignedHead) -> Result<HeadOutcome> {
        let syncer = self.clone();
        let head = head.clone();
        crate::blocking::offload(move || syncer.offer_head(&head, now_ns())).await
    }

    /// [`Syncer::local_summaries`] on the blocking pool.
    ///
    /// Summarizing asks the trie whether each advertised root is held whole,
    /// which is a walk the first time it is asked of a root (§5.1) — not
    /// something to do on a runtime worker.
    async fn summaries_off_runtime(&self, peer: synch_core::NodeId) -> Result<Advertisement> {
        let syncer = self.clone();
        crate::blocking::offload(move || {
            Ok(Advertisement {
                summaries: syncer.local_summaries()?,
                declared: syncer.declared_scope(peer)?,
                servable: Vec::new(),
            })
        })
        .await
    }

    /// What this node advertises, and the heads it can actually hand over.
    ///
    /// Read together because the push decision needs both. This performs one
    /// walk per unproven root and one query per origin off the runtime worker;
    /// the scope declaration travels with them.
    async fn advertisement_off_runtime(&self, peer: synch_core::NodeId) -> Result<Advertisement> {
        let syncer = self.clone();
        crate::blocking::offload(move || {
            let summaries = syncer.local_summaries()?;
            let declared = syncer.declared_scope(peer)?;
            // Filtered by the summaries this same exchange carries, not by the
            // complete slot: a head is worth pushing only if this node can
            // serve the trie under it. Pushing one that cannot be served lands
            // it in the receiver's pending slot, pinning `head_floor` until it
            // times out — the same cost `heads_for` stopped paying, reached by
            // the other message (§5.1).
            let servable = syncer
                .store
                .all_heads(Slot::Complete)?
                .into_iter()
                .map(|stored| stored.head)
                .filter(|head| {
                    summaries.iter().any(|s| {
                        s.origin == head.origin
                            && s.root == head.root
                            && s.seq == head.seq
                            && s.complete
                    })
                })
                .collect();
            Ok(Advertisement {
                summaries,
                declared,
                servable,
            })
        })
        .await
    }

    /// What this node will serve the peer it is dialing (§5.5).
    ///
    /// A dialer declares the scope of the peer it is calling, exactly as a
    /// responder does, so the direction of the dial makes no difference to what
    /// either side may read.
    ///
    /// A store read, so it is only ever called from a blocking scope.
    ///
    /// A declaration is not a grant — every responder enforces its own scope on
    /// every request — so its only effect is on how far the *reader* narrows
    /// itself. The three-valued shape is deliberate: a peer whose binding has
    /// momentarily lapsed must not be told the same thing as one promoted to a
    /// full member, because the reader decides what to do with its own grant
    /// from exactly that difference.
    fn declared_scope(&self, peer: synch_core::NodeId) -> Result<DeclaredScope> {
        Ok(match self.store.publish_scope_of_key(&peer, now_ns())? {
            synch_store::PublishScope::Untrusted => DeclaredScope::Untrusted,
            synch_store::PublishScope::Unrestricted => DeclaredScope::Unrestricted,
            synch_store::PublishScope::Confined(spaces) => DeclaredScope::Confined(spaces),
        })
    }

    /// Adopts the scope a peer declared for us, clearing anything memoized
    /// against the old one.
    ///
    /// Narrowing or widening, a change makes every completeness answer
    /// computed under the previous scope an answer to a different question, so
    /// the memo goes with it. It is keyed by scope as well as root, so nothing
    /// stale can be read back — this only avoids a table of answers nobody
    /// will ask for again.
    ///
    /// A declaration is only ever a bootstrap: once this node's own grant is
    /// materialized — its `d:` record, which the walk under any scope always
    /// includes — the read scope is that grant, and nothing a peer says is
    /// remembered (§5.5). A declaration can therefore never widen what this
    /// node may read:
    ///
    /// - a live grant makes the grant authoritative, whatever the peer says:
    ///   a peer one head behind the `d:` record, or one whose binding for this
    ///   node has lapsed, sees no delegation and would otherwise widen the
    ///   delegate back to the whole keyspace;
    /// - with no grant, a `Confined` declaration is the bootstrap — the only
    ///   legitimate way to learn the grant, since the trie it lives in cannot
    ///   be read before the scope is known;
    /// - `Unrestricted` with no grant is a promotion to a full member — but
    ///   only one this node can vouch for itself (a rooted binding for its
    ///   own key in a foreign origin, the shape a promotion's zone record
    ///   leaves): any operator's local `trust add` produces the same wire
    ///   value, and a lapsed grant must not be widened back by it;
    /// - `Untrusted` with no grant collapses a dead grant (revocation) to the
    ///   empty scope — `m:self` and the `d:` namespace, no file data — and
    ///   leaves a fresh node's default `None` alone.
    fn adopt_scope(
        &self,
        peer: synch_core::NodeId,
        declared: &DeclaredScope,
        summaries: &[synch_core::HeadSummary],
    ) -> Result<()> {
        // And only a peer that holds the trie the grant is published in. A
        // delegation is a record in the issuing origin's trie, so a peer that
        // cannot serve that trie has not read it and will answer from its own
        // local `trust add` instead — two honest peers then tell this node two
        // different things, and with one node-wide scope it flips between them
        // once per round, discarding and refetching every foreign origin each
        // time (§5.5).
        //
        // This is the enforceable half of "a delegate syncs only with full
        // members of its cluster": membership is holding the cluster's tries,
        // not sharing a domain suffix, and `complete` is where a peer says so.
        // A node no delegation names skips the check, which is what lets one
        // bootstrap.
        let issuers = self.store.own_issuers(now_ns())?;
        if !issuers.is_empty()
            && !issuers
                .iter()
                .any(|issuer| summaries.iter().any(|s| &s.origin == issuer && s.complete))
        {
            tracing::debug!(
                peer = %peer.fmt_short(),
                "ignoring a read scope declared by a peer that does not hold this node's \
                 issuer's trie"
            );
            return Ok(());
        }
        // Only a peer this node holds a *rooted* binding for may narrow it.
        //
        // The declaration drives more than what this node asks for: it decides
        // what counts as a complete trie and what gets materialized. A delegate
        // that could set it would be able to stop an ordinary member
        // replicating spaces it has every right to — and a member never holds a
        // rooted binding for a delegate, which is exactly the distinction that
        // closes it. A delegate bootstrapping holds a static or DNS binding for
        // the peers it dials, so it still learns its own scope.
        let rooted = self
            .store
            .live_bindings(now_ns())?
            .into_iter()
            .any(|b| b.node_id == peer && b.is_rooted());
        if !rooted {
            if *declared != DeclaredScope::Untrusted {
                tracing::debug!(
                    peer = %peer.fmt_short(),
                    "ignoring a read scope declared by a peer this node has no rooted binding for"
                );
            }
            return Ok(());
        }
        // The effective scope, from the node's own grant and the peer's
        // declaration — the declaration can only narrow, never widen:
        // `own_grant` is the `d:` record this node materialized itself, so it
        // is the freshest truth it holds, and a declaration from a peer whose
        // view is stale (or hostile) must not reach past it.
        let own = self.store.own_grant(now_ns())?;
        let current = self.store.local_scope()?;
        // A promotion this node can vouch for itself: a rooted binding for
        // its own key in a foreign origin, the shape a promotion's zone
        // record leaves in this node's resolver.
        let promoted = self.store.own_rooted_in_foreign_origin(now_ns())?;
        let effective: Option<Vec<String>> = match (own, declared) {
            // The grant is authoritative; a declaration cannot widen it.
            (Some(grant), _) => Some(grant),
            // Bootstrap: no grant materialized yet, and a peer that holds one
            // tells us what the walk is about to find in the trie.
            (None, DeclaredScope::Confined(spaces)) if current.is_none() => Some(spaces.clone()),
            // A dead grant must not be re-adopted from a stale declaration;
            // only a materialized re-grant (a fresh `d:` record, which the
            // walk always includes) may widen the collapsed scope.
            (None, DeclaredScope::Confined(_)) => return Ok(()),
            // Promotion: the peer holds a rooted binding for this node. Only
            // a binding this node itself materialized may widen it — any
            // operator's local `trust add` produces the same wire value, and
            // a delegate whose grant lapsed must not be widened back to the
            // whole keyspace by one stale rooted view (§5.5).
            (None, DeclaredScope::Unrestricted) if promoted => None,
            // Not promoted: the no-binding case. A scope that was confined
            // collapses to the empty scope — `m:self` and the `d:` namespace,
            // no file data — while a fresh node's default `None` stays put:
            // a fresh node is not a delegate until it has replicated the
            // trie the granting record lives in, and a key-identified node
            // may equally be an ordinary peer replicating a rooted member in
            // full (the bootstrap, §5.5).
            (None, DeclaredScope::Unrestricted)
                if current.as_ref().is_some_and(|s| !s.is_empty()) =>
            {
                Some(Vec::new())
            }
            (None, DeclaredScope::Unrestricted) => return Ok(()),
            // Revocation: the peer holds no binding at all.
            (None, DeclaredScope::Untrusted) if current.as_ref().is_some_and(|s| !s.is_empty()) => {
                Some(Vec::new())
            }
            (None, DeclaredScope::Untrusted) => return Ok(()),
        };
        // The one path that moves the scope, and destructive by design (§5.5):
        // everything derived under the old one answers a question nobody is
        // asking any more, and no diff reconciles it.
        if self.store.set_read_scope(effective.as_deref())? {
            tracing::info!(
                spaces = ?effective,
                "the read scope moved: every foreign origin will be refetched and rebuilt under it"
            );
            for wake in [&self.on_change, &self.on_replica].into_iter().flatten() {
                wake.notify_one();
            }
        }
        Ok(())
    }

    /// Runs one full `Hello` push-pull exchange with a peer, then fetches
    /// whatever it advertised that we do not have (§5.2, §5.3).
    pub async fn sync_with(&self, client: &MptClient) -> Result<SyncReport> {
        // A delegate pulls metadata only from a full member of its own cluster
        // (§5.5). The dial refuses this too; this is the check that decides,
        // because a session can outlive the binding it was opened under and an
        // inbound connection never passed through the dial at all.
        {
            let store = self.store.clone();
            let peer = client.remote_id();
            if let Some(reason) =
                crate::blocking::offload(move || Ok(store.refuse_metadata_sync(&peer, now_ns())?))
                    .await?
            {
                tracing::debug!(peer = %peer.fmt_short(), %reason, "declining a metadata exchange");
                return Ok(SyncReport::default());
            }
        }

        // Bring summaries and their servable heads over together in one hop to
        // the blocking pool. The decision below needs only those heads (§10).
        let Advertisement {
            summaries: ours,
            declared,
            servable,
        } = self.advertisement_off_runtime(client.remote_id()).await?;

        let mut report = SyncReport::default();
        let theirs = client
            .head_exchange(ours.clone(), declared, |theirs| {
                // Both slots may be advertised per origin, so the comparison is
                // against the best summary either side has for it, never the
                // first one that happens to match. Indexed once rather than
                // re-scanned per origin: the scan was quadratic in the number
                // of summaries, on both sides of the decision.
                let theirs_best = best_summaries(theirs);

                // Push: the servable head we hold, whenever it beats theirs.
                // Keyed off the complete slot directly rather than off whichever
                // summary was advertised: what we can hand over is exactly what
                // the complete slot holds.
                let push: Vec<SignedHead> = servable
                    .iter()
                    .filter(|head| {
                        let mine = (head.seq, head.root.0);
                        theirs_best
                            .get(&head.origin)
                            .is_none_or(|peer| mine > *peer)
                    })
                    .cloned()
                    .collect();
                (push, wanted_origins(theirs, &ours))
            })
            .await?;

        // What the peer says it will serve us. Adopted before anything is
        // fetched, so the very first walk of this session is already confined
        // to it — which is how a delegated node comes to be able to read the
        // trie its own scope is published in (§5.5).
        {
            let syncer = self.clone();
            let scope = theirs.scope.clone();
            let peer = client.remote_id();
            let seen = theirs.summaries.clone();
            crate::blocking::offload(move || syncer.adopt_scope(peer, &scope, &seen)).await?;
        }

        // Every exchange is also an observation of what peers hold for our own
        // origin, which is what `synch recover` reads (§3.4).
        {
            let syncer = self.clone();
            let peer = client.remote_id();
            let summaries = theirs.summaries.clone();
            crate::blocking::offload(move || {
                syncer.observe_summaries_from(Some(peer), &summaries, now_ns())
            })
            .await?;
        }

        report.heads_pushed = theirs.pushed;
        for head in theirs.received {
            // Adoption may promote the head, which walks the trie and
            // re-materializes every changed leaf in one transaction (§5.2).
            let outcome = match self.offer_head_off_runtime(&head).await {
                Ok(outcome) => outcome,
                Err(e) if is_origin_fault(&e) => {
                    contain(&head.origin, &e, &mut report);
                    continue;
                }
                Err(e) => return Err(e),
            };
            match outcome {
                // §12: the count of origins left behind belongs in the sync
                // report, and after the first exchange this is the only outcome
                // that carries it — the fault propagates once, and every later
                // offer is answered from the memo without re-deriving it.
                HeadOutcome::Refused => report.left_behind(&head.origin),
                HeadOutcome::Pending => {
                    report.heads_accepted += 1;
                    // Only from a peer that says it can serve the trie. The
                    // pending-slot pass below has always applied this guard;
                    // without it here, a head handed over by a peer that had
                    // just advertised `complete: false` was fetched from it
                    // anyway (§5.1).
                    if !serves_trie(&theirs.summaries, &head.origin, head.seq, head.root.0) {
                        continue;
                    }
                    match self.fetch_pending(client, &head.origin).await {
                        Ok(FetchOutcome::Completed) => report.tries_completed += 1,
                        Ok(FetchOutcome::Abandoned) => report.heads_abandoned += 1,
                        Ok(FetchOutcome::Refused) => report.left_behind(&head.origin),
                        Ok(FetchOutcome::NoFlip) => {}
                        Err(e) if is_origin_fault(&e) => contain(&head.origin, &e, &mut report),
                        Err(e) => return Err(e),
                    }
                }
                HeadOutcome::Completed => {
                    report.heads_accepted += 1;
                    report.tries_completed += 1;
                }
                HeadOutcome::BadSignature | HeadOutcome::Unbound => report.heads_rejected += 1,
                HeadOutcome::NotNewer => {}
            }
        }

        // A head can arrive by reactive push (§5.3) long before its trie does.
        // Such a head sits in the pending slot and is *not* newer than what we
        // hold, so the exchange above will not have asked for it — but §5.2
        // says its nodes may be fetched from any peer advertising a complete
        // head for that origin at or above its seq. Do exactly that here, which
        // is what turns "I heard about it" into "I can serve it".
        let pending_heads = {
            let store = self.store.clone();
            crate::blocking::offload(move || Ok(store.all_heads(Slot::Pending)?)).await?
        };
        for stored in pending_heads {
            let pending = stored.head;
            if !serves_trie(
                &theirs.summaries,
                &pending.origin,
                pending.seq,
                pending.root.0,
            ) {
                continue;
            }
            match self.fetch_pending(client, &pending.origin).await {
                Ok(FetchOutcome::Completed) => report.tries_completed += 1,
                Ok(FetchOutcome::Abandoned) => report.heads_abandoned += 1,
                Ok(FetchOutcome::Refused) => report.left_behind(&pending.origin),
                Ok(FetchOutcome::NoFlip) => {}
                Err(e) if is_origin_fault(&e) => contain(&pending.origin, &e, &mut report),
                Err(e) => return Err(e),
            }
        }
        Ok(report)
    }
}

/// True if `summaries` says the peer can serve the trie under this head (§5.1).
///
/// A signed head proves the origin published it; it says nothing about whether
/// the peer offering it holds the trie. `complete` is the claim that matters,
/// and it has to be checked against a seq at or above the one being fetched —
/// an older complete root cannot serve a newer head's nodes.
fn serves_trie(
    summaries: &[synch_core::HeadSummary],
    origin: &OriginId,
    seq: u64,
    root: [u8; 32],
) -> bool {
    summaries.iter().any(|summary| {
        &summary.origin == origin && summary.complete && summary.order_key() >= (seq, root)
    })
}

/// Verifies one batch of what a peer served and commits it, refusing anything
/// that was not asked for.
///
/// Three things are checked and all three are containment: a payload has to hash
/// to the hash it was requested by, it has to be one of the hashes this walk
/// asked for, and it may appear only once. Without the second, a peer answering
/// every request with `missing` plus one self-consistent pair of its own counts
/// as progress on every round — the unproductive counter never fires, the fetch
/// loop never ends, and the junk lands in the trie tables.
///
/// The third is what bounds the answer's *size*. A request is capped at
/// [`MAX_BATCH`] hashes, but the response it draws is capped only by the frame
/// length: a peer could answer 256 wanted hashes with a 16 MiB frame repeating
/// one of them, every entry passing both other checks, and each repeat costs an
/// autocommit `INSERT OR IGNORE` on the store's single write connection — so a
/// cheap request would buy six figures of serialized statements, blocking every
/// other database user in the process. Counting repeats as `learned` would also
/// defeat the [`MAX_UNPRODUCTIVE_ROUNDS`] escape, since a peer serving one real
/// node and 10^5 copies of it makes progress forever.
///
/// Nodes and values differ only in how a payload is verified, where it is
/// stored and which error names it, so the checks live here rather than in two
/// loops that have to be kept in step. `verify` says whether a payload is the
/// one asked for and, when it is not acceptable, *whose* fault that is: a
/// `NetError` is the peer's and ends the exchange, an origin fault is
/// contained to the origin ([`is_origin_fault`]).
///
/// The third check is now a backstop rather than the bound it was: `Nodes.nodes`
/// and `Values.values` are capped at [`MAX_BATCH`] *while decoding*, so a
/// response cannot carry more entries than the request carried hashes. It stays
/// because the containment set is what enforces it either way, and because a
/// repeat must not count as progress even if one arrives.
///
/// Returns how many were stored.
fn take_served(
    requested: &[synch_core::Hash],
    served: &[(synch_core::Hash, Vec<u8>)],
    what: &str,
    verify: impl Fn(&synch_core::Hash, &[u8]) -> Result<()>,
    put: impl Fn(&synch_core::Hash, &[u8]) -> Result<bool>,
) -> Result<usize> {
    // A wanted hash can be asked for once and so may be answered once. The set
    // is built from the request, never from the response, so the peer cannot
    // grow it.
    let mut outstanding: std::collections::HashSet<synch_core::Hash> =
        requested.iter().copied().collect();
    let mut stored = 0usize;
    for (hash, bytes) in served {
        // `remove` is the containment check and the repeat check at once: a
        // hash that was never asked for is not in the set, and one already
        // served has been taken out of it.
        if !outstanding.remove(hash) {
            return Err(EngineError::Net(NetError::Unexpected(format!(
                "peer served unrequested or repeated trie {what} {hash}"
            ))));
        }
        // A malicious or corrupt peer can withhold, never inject.
        verify(hash, bytes)?;
        // `put` decides whether the payload is one this node will keep: a rule
        // about what the *origin* published — a value small enough to be inline,
        // or one past the size ceiling — refuses the payload without failing the
        // batch. Refused payloads are not progress, so the unproductive counter
        // still runs and the head is retired by the §5.2 rule rather than by the
        // TTL sweep half an hour later.
        if put(hash, bytes)? {
            stored += 1;
        }
    }
    Ok(stored)
}

/// Whether served node bytes are the node they were requested as, and whose
/// fault it is when they are not.
///
/// Two failures look alike and must not be treated alike. Bytes that hash to
/// nothing wanted are the *peer's*: a corrupt or hostile relay, which ends the
/// exchange. Bytes that hash to the requested hash under their own kind's tag
/// but that this build refuses — a non-canonical encoding, a key run past
/// `MAX_KEY_LEN`, a broken structural invariant — are the *origin's*: the hash
/// covers the raw bytes, so no relay could have produced them, and §12 says a
/// record this node cannot apply fails its own origin and no other. Reporting
/// the second as the first aborted every exchange with every peer serving
/// that origin, at whichever origin sorted after it, and left the head pending
/// for the sweep to retire and the next exchange to re-adopt.
fn verify_node(expected: &synch_core::Hash, bytes: &[u8]) -> Result<()> {
    match TrieNode::hash_of_encoded(bytes) {
        Ok(hash) if &hash == expected => Ok(()),
        Err(refused) if TrieNode::hashes_to(expected, bytes) => Err(EngineError::Mpt(refused)),
        Ok(_) | Err(_) => Err(EngineError::Net(NetError::NodeHashMismatch {
            expected: *expected,
        })),
    }
}

/// Whether served value bytes are the payload they were requested as.
///
/// A value has no shape to refuse at this point: the bounds on what an origin
/// may put in one are applied by the `put` that follows, which refuses the
/// payload without failing the batch.
fn verify_value(expected: &synch_core::Hash, bytes: &[u8]) -> Result<()> {
    if &synch_core::Hash::new(bytes) == expected {
        return Ok(());
    }
    Err(EngineError::Net(NetError::ValueHashMismatch {
        expected: *expected,
    }))
}

/// The serve side's view of the reconciler (§5.2).
///
/// `synch-net` answers `Hello` and `HeadPush` by calling through this, so the
/// acceptance rule, the binding check and the promotion transaction stay here —
/// in the layer that owns head state — while the networking crate keeps only
/// the framing and the streams. It is also what lets one `Syncer` serve both
/// directions: the same object the node dials with is handed to the endpoint,
/// so a head flipping to complete rings the one bell either way, instead of a
/// `Notify` being threaded through the endpoint constructor to connect two
/// syncers that never knew about each other.
impl HeadSink for Syncer {
    fn local_summaries(&self) -> std::result::Result<Vec<HeadSummary>, NetError> {
        Syncer::local_summaries(self).map_err(to_net)
    }

    fn observe_summaries_from(
        &self,
        peer: synch_core::NodeId,
        summaries: &[HeadSummary],
        now: i64,
    ) -> std::result::Result<(), NetError> {
        Syncer::observe_summaries_from(self, Some(peer), summaries, now)
            .map(|_| ())
            .map_err(to_net)
    }

    /// Contained here, not at the wire.
    ///
    /// §12's rule is that a record this node cannot apply fails *its own
    /// origin* and no other, and `to_net` flattens every engine failure into
    /// one `NetError::Unexpected(String)` — so a caller on the far side of this
    /// seam cannot tell an undecodable `f:` record from `SQLITE_BUSY`. Classify
    /// here, where `try_promote` has retired any head it could judge. A fault
    /// raised earlier leaves the slot to the maintenance sweep.
    fn offer_head(&self, head: &SignedHead, now: i64) -> std::result::Result<(), NetError> {
        match Syncer::offer_head(self, head, now) {
            Ok(_) => Ok(()),
            Err(e) if is_origin_fault(&e) => {
                tracing::warn!(
                    origin = %head.origin,
                    seq = head.seq,
                    error = %e,
                    "origin left behind: its pushed head could not be applied"
                );
                Ok(())
            }
            Err(e) => Err(to_net(e)),
        }
    }

    fn heads_for(&self, origins: &[OriginId]) -> std::result::Result<Vec<SignedHead>, NetError> {
        Syncer::heads_for(self, origins).map_err(to_net)
    }
}

/// Renders an engine failure for the wire.
///
/// The seam is one-way by design: the engine names domain failures in its own
/// error type, and what crosses back into `synch-net` is a protocol-level
/// description of one. A `NetError` variant per storage fault is what would
/// make the transport enum a domain taxonomy.
fn to_net(error: EngineError) -> NetError {
    match error {
        EngineError::Net(e) => e,
        other => NetError::Unexpected(other.to_string()),
    }
}

/// The greatest `(seq, root)` each origin appears at in a summary set.
///
/// Both slots may be advertised per origin, so every comparison in an exchange is
/// against the best summary a side has for it. Indexed once rather than rescanned
/// per origin, which was quadratic in the number of summaries.
fn best_summaries(set: &[HeadSummary]) -> std::collections::HashMap<OriginId, (u64, [u8; 32])> {
    let mut best = std::collections::HashMap::new();
    for summary in set {
        let key = summary.order_key();
        best.entry(summary.origin.clone())
            .and_modify(|held| {
                if key > *held {
                    *held = key;
                }
            })
            .or_insert(key);
    }
    best
}

/// The origins to ask a peer for: those whose best summary beats our best.
///
/// Over the *best* summary each side has for an origin, never the first one that
/// happens to match. A peer advertises both slots per origin, and its complete
/// slot can be the higher of the two — `publish` and `activate` take
/// `next_own_seq` and write the complete slot without consulting pending, which
/// is the §3.4 recovery shape §5.2 names. Walking the raw summaries and skipping
/// origins already seen compared whichever the peer listed first and discarded
/// the rest, so the pull decision depended on an order nothing on the wire
/// constrains, and this node could miss a head strictly newer than its own. It
/// was safe only by accident: `local_summaries` sorts ascending, so the complete
/// summary is usually second and its being discarded usually did not matter.
///
/// Invisible in a symmetric cluster, because the peer's own round pushes what we
/// failed to pull; visible in exactly the topology the pull exists for, where we
/// can dial the peer and it cannot dial back (§5.3).
///
/// Sorted, so what a peer receives does not depend on hash iteration order.
fn wanted_origins(theirs: &[HeadSummary], ours: &[HeadSummary]) -> Vec<OriginId> {
    let ours_best = best_summaries(ours);
    let mut want: Vec<OriginId> = best_summaries(theirs)
        .into_iter()
        .filter(|(origin, theirs)| ours_best.get(origin).is_none_or(|mine| theirs > mine))
        .map(|(origin, _)| origin)
        .collect();
    want.sort();
    want
}

/// True if a failure is about *one origin's* replicated data rather than about
/// this node, the peer, or the connection.
///
/// Three kinds of failure reach this, and only one of them is an origin's.
///
/// - **An origin's**: a record that will not decode, a node that breaks a
///   structural invariant, a value at a depth no key reaches. Durable,
///   reproduced on every exchange that reaches it, and *contained* — one member
///   publishing something this node cannot apply must not stop it converging
///   with every other origin the same peer serves (§12).
/// - **The peer's**: a protocol violation or a broken stream. Ends the exchange.
/// - **Ours**: `SQLITE_BUSY` from another process, a full disk, an I/O error.
///   These propagate so an operator can see them.
pub(crate) fn is_origin_fault(error: &EngineError) -> bool {
    match error {
        EngineError::Store(e) => is_origin_store_fault(e),
        EngineError::Mpt(e) => is_origin_mpt_fault(e),
        _ => false,
    }
}

/// Whether a store failure is about replicated data rather than about this
/// node's own disk or database.
///
/// `Invalid` is "a caller supplied an argument the store refuses", and on the
/// paths this guards the argument is always something a peer or an origin
/// supplied: a summary claiming a seq past the representable range, a head whose
/// `(origin, seq, root)` already retains a different signature. The store's other
/// `Invalid`s — a schema stamp this build cannot read, a migration that failed —
/// belong to `Store::open` and are unreachable from an exchange.
fn is_origin_store_fault(error: &synch_store::StoreError) -> bool {
    match error {
        // What a peer published, read back and refused.
        synch_store::StoreError::Decode(_)
        | synch_store::StoreError::Column { .. }
        | synch_store::StoreError::Invalid(_) => true,
        synch_store::StoreError::Mpt(e) => is_origin_mpt_fault(e),
        // Ours: the database, the filesystem, or an object this node holds.
        _ => false,
    }
}

/// Whether a trie failure is about the structure a peer served rather than
/// about the store under it.
fn is_origin_mpt_fault(error: &synch_mpt::MptError) -> bool {
    !matches!(
        error,
        synch_mpt::MptError::Store(_) | synch_mpt::MptError::WalkStopped
    )
}

/// Logs a contained per-origin failure and records it in the report.
///
/// One origin publishing something this node cannot materialize must not stop
/// it from converging with every *other* origin the same peer serves: the
/// failing head keeps its slot, the exchange carries on, and the count says
/// plainly that something was left behind (§5.2).
fn contain(origin: &OriginId, error: &EngineError, report: &mut SyncReport) {
    tracing::warn!(
        origin = %origin,
        error = %error,
        "origin left behind: its published data could not be applied"
    );
    report.left_behind(origin);
}

/// What one exchange achieved, for logging and tests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Heads we pushed to the peer.
    pub heads_pushed: usize,
    /// Heads the peer sent that we adopted.
    pub heads_accepted: usize,
    /// Heads the peer sent that failed verification or the binding check.
    pub heads_rejected: usize,
    /// Tries that became complete during this exchange.
    pub tries_completed: usize,
    /// Pending heads abandoned because nobody could serve their nodes.
    pub heads_abandoned: usize,
    /// Origins skipped because their own published data could not be applied.
    pub heads_failed: usize,
    /// Which origins `heads_failed` counts, so counting one twice cannot inflate
    /// it. Not part of the rendered report.
    failed_origins: std::collections::HashSet<OriginId>,
}

impl SyncReport {
    /// Whether this exchange completed locally visible state.
    ///
    /// Pushing heads advances the peer, accepting a pending head only records a
    /// pointer, and abandoning or refusing one merely unwedges the slot. None
    /// stops periodic fallback to another candidate; only a completed trie has
    /// moved what local readers can observe.
    pub(crate) fn made_local_progress(&self) -> bool {
        self.tries_completed > 0
    }

    /// Records that this origin was left behind, once however often it is said.
    ///
    /// The number is rendered as "N origin(s) left behind", and two paths reach
    /// this verdict for the same origin in one exchange: the fault propagates
    /// from the first offer of a head, and every later offer of it is answered
    /// from the refusal memo. Nothing dedups the head list a peer sends, so a
    /// repeated head counted its origin once per copy.
    fn left_behind(&mut self, origin: &OriginId) {
        if self.failed_origins.insert(origin.clone()) {
            self.heads_failed += 1;
        }
    }
}
#[cfg(test)]
mod tests {
    use iroh_base::SecretKey;
    use synch_core::{file_key, FileEntry, Hash, OriginId, SignedHead};
    use synch_store::{Binding, BindingSource};

    use super::*;

    pub(super) fn setup() -> (tempfile::TempDir, Arc<Store>, SecretKey, OriginId) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap());
        let key = SecretKey::generate();
        let origin = OriginId::named("nas", "x.example").unwrap();
        store
            .put_binding(&Binding {
                origin: origin.clone(),
                node_id: key.public(),
                source: BindingSource::Static,
                domain: None,
                issuer: None,
                spaces: Vec::new(),
                note: None,
                added_at: 0,
                expires_at: None,
            })
            .unwrap();
        (dir, store, key, origin)
    }

    fn publish(store: &Store, files: &[&str]) -> Hash {
        let trie = Trie::new(store);
        let mut root = Hash::EMPTY;
        for path in files {
            let entry = FileEntry::file(7, 0, Hash::new(path.as_bytes()), 1);
            root = trie
                .insert(
                    root,
                    &file_key("s", path).unwrap(),
                    &postcard::to_stdvec(&entry).unwrap(),
                )
                .unwrap();
        }
        root
    }

    /// DoS bound: an origin flooding one seq is evicted at the cap, never
    /// refused — the greatest roots keep the slot, whatever arrived first.
    #[test]
    fn same_seq_forks_are_evicted_at_the_cap_and_never_refused() {
        let (_d, store, key, origin) = setup();
        let syncer = Syncer::new(store.clone());
        // Ascending roots, past the cap: each one supersedes the last.
        for i in 1..=MAX_RETAINED_FORKS as u8 + 4 {
            let head = SignedHead::sign(&key, origin.clone(), 1, Hash([i; 32]), 0);
            assert!(
                syncer.offer_head(&head, 0).unwrap().accepted(),
                "fork {i} is evidence and is taken"
            );
        }
        assert_eq!(store.fork_width(&origin, 1).unwrap(), MAX_RETAINED_FORKS);
        let roots: Vec<u8> = store
            .head_history(&origin)
            .unwrap()
            .iter()
            .filter(|h| h.seq == 1)
            .map(|h| h.root.0[0])
            .collect();
        assert_eq!(*roots.iter().min().unwrap(), MAX_RETAINED_FORKS as u8 - 3);
        assert_eq!(
            store.head_floor(&origin).unwrap().unwrap().1,
            Hash([MAX_RETAINED_FORKS as u8 + 4; 32]),
            "and the greatest root of all holds the slot"
        );
        // The evidence a fork exists is still there.
        assert_eq!(store.equivocations().unwrap().len(), 1);

        // A head at a later seq is the origin moving on, and is taken normally.
        let next = SignedHead::sign(&key, origin, 2, Hash([1u8; 32]), 0);
        assert!(syncer.offer_head(&next, 0).unwrap().accepted());
    }

    /// §5.2 containment: only requested hashes, each once, hash-verified —
    /// the injection and amplification bounds on trie fetch.
    #[test]
    fn a_peer_may_not_answer_with_what_was_not_asked_for() {
        let wanted = Hash::new(b"wanted");
        let junk = b"nobody asked for this".to_vec();
        let unrequested = Hash::new(&junk);
        let stored = std::cell::RefCell::new(Vec::new());
        let take = |requested: &[Hash], served: &[(Hash, Vec<u8>)]| {
            take_served(requested, served, "value", verify_value, |hash, _| {
                stored.borrow_mut().push(*hash);
                Ok(true)
            })
        };

        let err = take(&[wanted], &[(unrequested, junk.clone())])
            .expect_err("an unrequested value is refused");
        assert!(err.to_string().contains("unrequested"), "{err}");
        assert!(stored.borrow().is_empty(), "and nothing was written");

        // What was asked for is taken, and a payload that does not hash to the
        // hash it was requested by is still refused on its own terms.
        assert_eq!(take(&[unrequested], &[(unrequested, junk)]).unwrap(), 1);
        assert!(matches!(
            take(
                &[unrequested],
                &[(unrequested, b"different bytes".to_vec())]
            ),
            Err(EngineError::Net(NetError::ValueHashMismatch { .. }))
        ));
        assert_eq!(*stored.borrow(), vec![unrequested]);

        // One wanted node repeated would pass containment and cost an insert
        // apiece; the repeat is refused, and counting it as progress would
        // defeat `MAX_UNPRODUCTIVE_ROUNDS`.
        stored.borrow_mut().clear();
        let dup = b"a node that really was asked for".to_vec();
        let dup_hash = Hash::new(&dup);
        let err = take(&[dup_hash], &[(dup_hash, dup.clone()), (dup_hash, dup)])
            .expect_err("a repeat is refused");
        assert!(err.to_string().contains("repeated"), "{err}");
        assert_eq!(
            *stored.borrow(),
            vec![dup_hash],
            "taken before the repeat was seen"
        );

        stored.borrow_mut().clear();
        let other = b"a second wanted node".to_vec();
        let other_hash = Hash::new(&other);
        assert_eq!(
            take(
                &[dup_hash, other_hash],
                &[
                    (dup_hash, b"a node that really was asked for".to_vec()),
                    (other_hash, other),
                ],
            )
            .unwrap(),
            2
        );
    }

    /// §12: served bytes that hash to what was asked for but break a
    /// structural invariant are the origin's fault; bytes that hash to
    /// nothing wanted are the peer's. The two must classify apart, because one
    /// is contained to an origin and the other ends the exchange.
    #[test]
    fn a_refused_node_shape_is_the_origins_fault_and_wrong_bytes_are_the_peers() {
        let (value, _) = synch_mpt::ValueRef::for_value(b"x");
        let leaf = synch_mpt::TrieNode::Leaf {
            key_rest: synch_mpt::Nibbles::from_nibbles(&[1, 2, 3]),
            value,
        };
        let mut children = [None; 16];
        children[6] = Some(leaf.hash());
        let lonely = synch_mpt::TrieNode::Branch {
            children,
            value: None,
        };
        let (bytes, hash) = (lonely.encode(), lonely.hash());
        assert!(synch_mpt::TrieNode::hash_of_encoded(&bytes).is_err());

        let err = verify_node(&hash, &bytes).expect_err("an under-occupied branch is refused");
        assert!(is_origin_fault(&err), "{err}");
        let err = verify_node(&Hash::new(b"something else"), &bytes)
            .expect_err("bytes that hash to nothing wanted are refused");
        assert!(!is_origin_fault(&err), "{err}");
        assert!(matches!(
            err,
            EngineError::Net(NetError::NodeHashMismatch { .. })
        ));
        let good = leaf.encode();
        verify_node(&leaf.hash(), &good).expect("a canonical node verifies");
        assert!(!is_origin_fault(
            &verify_node(&hash, &good).expect_err("the wrong node is the peer's")
        ));
    }

    #[test]
    fn a_forged_signature_is_rejected() {
        let (_d, store, key, origin) = setup();
        let syncer = Syncer::new(store.clone());
        let mut head = SignedHead::sign(&key, origin.clone(), 1, Hash::EMPTY, 0);
        head.seq = 99;
        assert_eq!(
            syncer.offer_head(&head, 0).unwrap(),
            HeadOutcome::BadSignature
        );
        assert_eq!(store.complete_head(&origin).unwrap(), None);
        assert!(store.head_history(&origin).unwrap().is_empty());

        // §3.2: a valid signature from a key that is not bound to the origin
        // is refused even if relayed by a trusted peer.
        let stranger = SecretKey::generate();
        let relayed = SignedHead::sign(&stranger, origin.clone(), 1, Hash::EMPTY, 0);
        relayed.verify_signature().unwrap();
        assert_eq!(
            syncer.offer_head(&relayed, 0).unwrap(),
            HeadOutcome::Unbound
        );
        assert_eq!(store.complete_head(&origin).unwrap(), None);
    }

    #[test]
    fn an_expired_binding_no_longer_admits_heads() {
        // The instants are real ones: an expiry is compared against a clock,
        // and a reading below `MIN_TRUSTED_NS` dates nothing (§3.2).
        let before = synch_core::MIN_TRUSTED_NS + 50;
        let expiry = synch_core::MIN_TRUSTED_NS + 100;
        let after = synch_core::MIN_TRUSTED_NS + 200;
        let (_d, store, _k, origin) = setup();
        let rotated = SecretKey::generate();
        store
            .put_binding(&Binding {
                origin: origin.clone(),
                node_id: rotated.public(),
                source: BindingSource::Dns,
                domain: Some("x.example".into()),
                issuer: None,
                spaces: Vec::new(),
                note: None,
                added_at: 0,
                expires_at: Some(expiry),
            })
            .unwrap();
        let syncer = Syncer::new(store.clone());
        let head = SignedHead::sign(&rotated, origin.clone(), 1, Hash::EMPTY, 0);
        assert_eq!(
            syncer.offer_head(&head, before).unwrap(),
            HeadOutcome::Completed
        );
        let later = SignedHead::sign(&rotated, origin, 2, Hash::new(b"x"), 0);
        assert_eq!(
            syncer.offer_head(&later, after).unwrap(),
            HeadOutcome::Unbound
        );
    }

    /// Two-slot advertising: a pending head is advertised *alongside* the
    /// servable complete head, never in its place — a propagation-killing shape.
    #[test]
    fn summaries_report_completeness_honestly() {
        let (_d, store, key, origin) = setup();
        let syncer = Syncer::new(store.clone());
        let root = publish(&store, &["a"]);
        syncer
            .offer_head(&SignedHead::sign(&key, origin.clone(), 1, root, 0), 0)
            .unwrap();
        let summaries = syncer.local_summaries().unwrap();
        assert_eq!(summaries.len(), 1);
        assert!(summaries[0].complete);

        syncer
            .offer_head(
                &SignedHead::sign(&key, origin, 2, Hash::new(b"unknown"), 0),
                0,
            )
            .unwrap();
        let summaries = syncer.local_summaries().unwrap();
        assert_eq!(summaries.len(), 2);
        let complete: Vec<_> = summaries.iter().filter(|s| s.complete).collect();
        assert_eq!(complete.len(), 1, "the servable root is still advertised");
        assert_eq!(complete[0].seq, 1);
        assert_eq!(complete[0].root, root);
        let pending: Vec<_> = summaries.iter().filter(|s| !s.complete).collect();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].seq, 2);
    }

    /// §5.2: the flip and the materialization are one transaction — a record
    /// the materializer cannot decode stands in for the crash.
    #[test]
    fn a_promotion_that_fails_to_materialize_does_not_flip_the_head() {
        let (_d, store, key, origin) = setup();
        let syncer = Syncer::new(store.clone());

        let good = publish(&store, &["a"]);
        let complete = SignedHead::sign(&key, origin.clone(), 1, good, 0);
        assert!(matches!(
            syncer.offer_head(&complete, 0).unwrap(),
            HeadOutcome::Completed
        ));

        // A pending head whose trie is fully present but carries a well-formed
        // `f:` key with a value that is not a `FileEntry`.
        let trie = Trie::new(store.as_ref());
        let poisoned = trie
            .insert(good, &file_key("s", "poisoned").unwrap(), &[0xffu8; 8])
            .unwrap();
        let pending = SignedHead::sign(&key, origin.clone(), 2, poisoned, 0);
        store
            .put_head(synch_store::Slot::Pending, &pending, 0, 0)
            .unwrap();

        let err = syncer.try_promote(&origin, 0).unwrap_err().to_string();
        assert!(err.contains("corrupt record"), "{err}");

        // What must not have happened is the flip: the complete slot still
        // names the good root (the poisoned root *is* in `head_history` — the
        // signature is there by construction — so that table is no oracle).
        let complete = store.complete_head(&origin).unwrap().unwrap();
        assert_eq!(complete.seq, 1);
        assert_eq!(complete.root, good);
        assert_ne!(complete.root, poisoned);
        // And the head that failed no longer holds the slot, and the good
        // head's entry is still materialized.
        assert_eq!(store.pending_head(&origin).unwrap(), None);
        assert!(store.entry(&origin, "s", "poisoned").unwrap().is_none());
        assert!(store.entry(&origin, "s", "a").unwrap().is_some());
    }
}

#[cfg(test)]
mod containment_tests {
    use synch_core::{file_key, FileEntry, Hash, OriginId, SignedHead};
    use synch_store::Slot;

    use super::tests::setup;
    use super::*;

    fn pending_received_at(store: &Store, origin: &OriginId) -> i64 {
        store
            .head(origin, Slot::Pending)
            .unwrap()
            .unwrap()
            .received_at
    }

    /// A poisoned head must not pin `head_floor` above servable heads; the
    /// verdict is remembered (re-offer Refused from the memo, a different
    /// root judged on its merits), the evidence stays in `head_history`.
    ///
    /// Two ways to poison a trie, one verdict: a non-`FileEntry` value, and a
    /// node the walk refuses (§4.3) — the walk fires before the promotion
    /// diff, so the verdict key is built before it.
    #[test]
    fn a_promotion_judged_unpromotable_holds_no_floor_and_is_not_attempted_again() {
        let (_d, store, key, origin) = setup();
        let syncer = Syncer::new(store.clone());
        let trie = Trie::new(store.as_ref());

        // A canonical trie whose one `f:` value is not a `FileEntry`.
        let poisoned = trie
            .insert(Hash::EMPTY, &file_key("s", "a.txt").unwrap(), &[0xffu8; 8])
            .unwrap();
        let head = SignedHead::sign(&key, origin.clone(), 5, poisoned, 0);
        let err = syncer
            .offer_head(&head, 0)
            .expect_err("the record cannot be materialized");
        assert!(err.to_string().contains("corrupt record"), "{err}");

        assert_eq!(store.pending_head(&origin).unwrap(), None);
        assert_eq!(store.complete_head(&origin).unwrap(), None);
        assert_eq!(store.head_floor(&origin).unwrap(), None);
        assert_eq!(store.head_history(&origin).unwrap().len(), 1);

        // Offered again — which is what every later exchange with a peer
        // holding it does: no error surfaces, the outcome is still counted
        // against the origin, and it does not keep the floor.
        assert_eq!(
            syncer.offer_head(&head, 0).unwrap(),
            HeadOutcome::Refused,
            "the promotion is not re-attempted, and the origin is still counted"
        );
        assert_eq!(store.head_floor(&origin).unwrap(), None);

        // The same verdict by the other route: a trie that is structurally
        // invalid — an extension whose child is a leaf, which no canonical
        // trie contains (§4.3). Stored through `put_node` directly, because
        // this is what a peer serving a hand-built graph looks like.
        let (value, _) = synch_mpt::ValueRef::for_value(&[7u8; 4]);
        let child = synch_mpt::TrieNode::Leaf {
            key_rest: synch_mpt::Nibbles::from_nibbles(&[1, 2]),
            value,
        };
        let child_hash = child.hash();
        let structural = synch_mpt::TrieNode::Ext {
            prefix: synch_mpt::Nibbles::from_nibbles(&[4]),
            child: child_hash,
        };
        let structural_root = structural.hash();
        synch_mpt::NodeStore::put_node(store.as_ref(), &child_hash, &child.encode()).unwrap();
        synch_mpt::NodeStore::put_node(store.as_ref(), &structural_root, &structural.encode())
            .unwrap();
        let bad = SignedHead::sign(&key, origin.clone(), 7, structural_root, 0);
        let err = syncer
            .offer_head(&bad, 0)
            .expect_err("a non-canonical node is the origin's fault");
        assert!(is_origin_fault(&err), "{err}");
        assert_eq!(store.pending_head(&origin).unwrap(), None);
        assert_eq!(
            store.head_floor(&origin).unwrap(),
            None,
            "retired: the floor is back to what this node can serve"
        );

        // A good root this node *can* serve is adoptable again — a lesser
        // one, which the held floor would have refused...
        let good_value = postcard::to_stdvec(&FileEntry::file(1, 0, Hash::new(b"c"), 1)).unwrap();
        let good = trie
            .insert(Hash::EMPTY, &file_key("s", "a.txt").unwrap(), &good_value)
            .unwrap();
        let lesser = SignedHead::sign(&key, origin.clone(), 4, good, 0);
        assert_eq!(
            syncer.offer_head(&lesser, 0).unwrap(),
            HeadOutcome::Completed,
            "a lesser servable head must be adoptable again"
        );

        // ...and a later one is the origin moving on, taken normally.
        let later = SignedHead::sign(&key, origin, 6, good, 0);
        assert_eq!(
            syncer.offer_head(&later, 0).unwrap(),
            HeadOutcome::Completed
        );
    }

    /// The fetch bell (§5.3): a pending head rung mid-round — when the loop
    /// is dialling peers rather than parked — must still wake a listener that
    /// parks afterwards, and a refused head must not ring at all: a Notify
    /// keeps one permit per ring, so either fault would fail a check.
    #[tokio::test]
    async fn the_fetch_bell_keeps_mid_round_rings_and_stays_silent_for_refusals() {
        let (_d, store, key, origin) = setup();
        let wake = Arc::new(tokio::sync::Notify::new());
        let syncer = Syncer::new(store.clone()).on_pending(wake.clone());

        // Rung with no listener parked, which is the mid-round case.
        let head = SignedHead::sign(&key, origin.clone(), 1, Hash([1u8; 32]), 0);
        assert_eq!(syncer.offer_head(&head, 0).unwrap(), HeadOutcome::Pending);
        // A listener arriving afterwards still has to see it.
        tokio::time::timeout(std::time::Duration::from_secs(5), wake.notified())
            .await
            .expect("a wake that landed while the loop was busy must still be there");

        // A poisoned head rings nothing: neither the fault on arrival nor the
        // memo refusal.
        let trie = Trie::new(store.as_ref());
        let poisoned = trie
            .insert(Hash::EMPTY, &file_key("s", "a.txt").unwrap(), &[0xffu8; 8])
            .unwrap();
        let bad = SignedHead::sign(&key, origin.clone(), 5, poisoned, 0);
        syncer
            .offer_head(&bad, 0)
            .expect_err("the record cannot be materialized");
        assert_eq!(
            syncer.offer_head(&bad, 0).unwrap(),
            HeadOutcome::Refused,
            "answered from the memo"
        );
        for _ in 0..2 {
            tokio::time::timeout(std::time::Duration::from_millis(200), wake.notified())
                .await
                .expect_err("a retired or refused head must not ring the fetch bell");
        }
    }

    /// The pending slot ages, not the head occupying it: taking `received_at`
    /// from each new head reset the sweep clock, so a fast publisher outran
    /// the TTL forever.
    #[test]
    fn a_newer_pending_head_inherits_the_slots_clock() {
        let (_d, store, key, origin) = setup();
        let first = SignedHead::sign(&key, origin.clone(), 1, Hash([1u8; 32]), 0);
        let second = SignedHead::sign(&key, origin.clone(), 2, Hash([2u8; 32]), 0);

        store.put_head(Slot::Pending, &first, 100, 100).unwrap();
        store
            .put_head(Slot::Pending, &second, 9_000, 9_000)
            .unwrap();
        let stored = store.head(&origin, Slot::Pending).unwrap().unwrap();
        assert_eq!(stored.head.seq, 2, "the newer head took the slot");
        assert_eq!(stored.received_at, 100);

        // A fetch that commits something restarts it, so a trie that genuinely
        // takes longer than the TTL to arrive is not swept mid-transfer.
        assert!(
            !store
                .touch_pending_at(&origin, 1, &Hash([1u8; 32]), 9_500)
                .unwrap(),
            "a fetch of a head the slot has moved past must not restart the clock"
        );
        let received = pending_received_at(&store, &origin);
        assert_eq!(received, 100);
        assert!(store
            .touch_pending_at(&origin, 2, &Hash([2u8; 32]), 9_500)
            .unwrap());
        let received = pending_received_at(&store, &origin);
        assert_eq!(received, 9_500);

        // An empty slot starts its own clock.
        assert!(store
            .clear_head_at(&origin, Slot::Pending, 2, &Hash([2u8; 32]))
            .unwrap());
        store
            .put_head(Slot::Pending, &first, 20_000, 20_000)
            .unwrap();
        let received = pending_received_at(&store, &origin);
        assert_eq!(received, 20_000);
        let other = OriginId::named("other", "x.example").unwrap();
        assert!(
            !store
                .touch_pending_at(&other, 1, &Hash([1u8; 32]), 1)
                .unwrap(),
            "and there is nothing to touch for an origin with no pending head"
        );
    }

    /// Our own next seq counts the pending slot and the retained history, not
    /// just the complete slot — the §3.4 recovery shape a restored backup meets.
    #[test]
    fn the_next_own_seq_clears_the_pending_slot_and_the_history() {
        let (_d, store, key, origin) = setup();
        store.set_self_origin(&origin).unwrap();
        assert_eq!(store.next_own_seq(&origin).unwrap(), 1);

        // A peer relays one of our own heads back, at a seq the complete slot
        // knows nothing about.
        let relayed = SignedHead::sign(&key, origin.clone(), 9, Hash([9u8; 32]), 0);
        assert_eq!(
            Syncer::new(store.clone()).offer_head(&relayed, 0).unwrap(),
            HeadOutcome::Pending
        );
        assert_eq!(store.complete_head(&origin).unwrap(), None);
        assert_eq!(store.next_own_seq(&origin).unwrap(), 10);

        // Even once the pending slot is cleared, the retained history still does.
        store
            .clear_head_at(&origin, Slot::Pending, 9, &Hash([9u8; 32]))
            .unwrap();
        assert_eq!(store.next_own_seq(&origin).unwrap(), 10);
    }
}
