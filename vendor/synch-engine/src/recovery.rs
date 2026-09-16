//! Key-loss recovery (§3.4).
//!
//! When the operator replaces an origin's TXT record with a fresh key, the
//! cluster sees an ordinary rotation without the overlap window. The node on
//! the other end of it does not: it comes up with the same `id=` name, an empty
//! database, and no way to know whether the peers it can reach hold that
//! origin's latest history. So recovery is a distinct, explicitly driven state
//! rather than something a node does on startup:
//!
//! 1. **Detection** — a node that holds no head of its own but finds peers
//!    advertising heads for its own origin is *in recovery*, and refuses to
//!    publish. A node that silently started over at `seq = 1` would have every
//!    peer correctly reject it, and the reason would be invisible.
//! 2. **Observation** — those heads are signed by the lost key and can never be
//!    accepted (§4.4), but their existence arrives for free in every peer's
//!    `Hello` summary (§5.1). No new wire message, and no unbound signature is
//!    ever trusted.
//! 3. **Resumption** — [`Node::recover`] collects those summaries from every
//!    reachable peer for at least `recovery_quiesce`, then sets the publishing
//!    floor to `max_observed_seq + seq_gap`. The gap makes a same-seq collision
//!    with history held only by an unreachable peer improbable rather than
//!    merely unlikely.
//!
//! Recovery is operator-driven for the same reason rotation is: the node cannot
//! see the peers it cannot reach, so "how far had I got?" is a judgement made on
//! partial information. An operator knows whether the NAS holding the newest
//! history is merely asleep or genuinely gone; the node does not.

use std::{
    collections::BTreeSet,
    fmt,
    time::{Duration, Instant},
};

use synch_core::{now_ns, Hash, NodeId, OriginId};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    error::{EngineError, Result},
    node::Node,
};

/// How long `synch recover` collects peer summaries by default (§3.4).
pub(crate) const DEFAULT_RECOVERY_QUIESCE: Duration = Duration::from_secs(3600);

/// How far past this node's own next seq one recovery may lift the publishing
/// floor (§3.4).
///
/// The floor is derived from what peers *say*, is durable, only ever rises, and
/// has no lowering command — so an unauthenticated claim acted on once would
/// otherwise be able to retire an origin for good. A cluster's seqs advance by
/// one per publish, so a gap of this size already covers any real divergence
/// between what this node last held and what its peers have seen since;
/// anything past it is a claim to be clamped and logged rather than obeyed.
pub(crate) const MAX_RECOVERY_STEP: u64 = 1_000_000;

/// How far above the highest observed seq publishing resumes, by default
/// (§3.4).
pub(crate) const DEFAULT_SEQ_GAP: u64 = 1_000;

/// Where this node stands with respect to key-loss recovery (§3.4 step 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryState {
    /// This node's own origin.
    pub origin: OriginId,
    /// True if this node holds no head of its own while peers advertise heads
    /// for its origin at or above the seq it would publish at.
    pub in_recovery: bool,
    /// The highest seq any peer has advertised for our origin.
    pub observed_seq: Option<u64>,
    /// Which peer claimed that seq, when it is known (§3.4).
    ///
    /// Detection rests on peers' unauthenticated summaries — deliberately,
    /// since the true heads are signed by the lost key and cannot validate — so
    /// within the §12 trust stance any member could assert a huge seq and hold
    /// a fresh node in recovery. The attribution is what lets an operator judge
    /// the claim rather than merely obey it.
    pub observed_by: Option<NodeId>,
    /// The durable publishing floor, once `synch recover` has set one.
    pub floor: Option<u64>,
    /// The seq this node's next publish would carry.
    pub next_seq: u64,
}

/// One entry of pre-recovery history we hold and cannot reconcile (§3.4, §4.4).
///
/// A head verified while its signer was bound stays provable history even after
/// the binding is gone. When such a head sits at or above the origin's current
/// published head, the origin's new history does not supersede it: that is a
/// fork, and it is resolved by the origin's operator, never silently by the
/// protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreconciledHistory {
    /// The origin the history belongs to.
    pub origin: OriginId,
    /// The seq of the retained head.
    pub seq: u64,
    /// Its root.
    pub root: Hash,
    /// The key that signed it, which is no longer bound to the origin.
    pub signed_by: NodeId,
    /// The seq of the origin's current complete head, if we hold one.
    pub current_seq: Option<u64>,
}

/// How `synch recover` runs (§3.4 step 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryOptions {
    /// How long to keep collecting summaries before setting the floor.
    pub wait: Duration,
    /// How far above the highest observed seq the floor is set.
    pub gap: u64,
    /// How long to wait between collection rounds.
    pub poll: Duration,
}

/// What one collection round saw.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ObserveRound {
    /// Peers that answered a `Hello` exchange.
    pub reached: Vec<NodeId>,
    /// Peers that could not be reached, or failed mid-exchange.
    pub unreachable: Vec<NodeId>,
    /// The highest seq observed for our own origin after this round.
    pub observed_seq: Option<u64>,
}

/// A progress report streamed while the quiesce runs.
///
/// A one-hour wait must not look like a hung command, so every round reports
/// what it reached and how much of the wait is left (§9.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryProgress {
    /// Which collection round this is, from 1.
    pub round: usize,
    /// How long the quiesce has been running.
    pub elapsed: Duration,
    /// How much of it is left.
    pub remaining: Duration,
    /// How many peers answered this round.
    pub reached: usize,
    /// How many did not.
    pub unreachable: usize,
    /// The highest seq observed for our origin so far.
    pub observed_seq: Option<u64>,
}

impl fmt::Display for RecoveryProgress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "round {}: {} peer(s) answered, {} unreachable · highest seq seen {} · {}s elapsed, {}s left",
            self.round,
            self.reached,
            self.unreachable,
            match self.observed_seq {
                Some(seq) => seq.to_string(),
                None => "none".to_string(),
            },
            self.elapsed.as_secs(),
            self.remaining.as_secs(),
        )
    }
}

/// What `synch recover` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryReport {
    /// The origin recovered.
    pub origin: OriginId,
    /// The highest seq any reachable peer advertised for it.
    pub observed_seq: Option<u64>,
    /// The publishing floor now in force, if one was set.
    pub floor: Option<u64>,
    /// The gap applied above the highest observed seq.
    pub gap: u64,
    /// How many collection rounds ran.
    pub rounds: usize,
    /// How many distinct peers answered at least once.
    pub reached: usize,
    /// How many were never reached.
    pub unreachable: usize,
    /// How long the quiesce actually took.
    pub waited: Duration,
}

impl Node {
    /// Re-adopts a newer own-origin head retained by peers after a database
    /// restore, before any local publisher is allowed to run.
    ///
    /// Peer summaries decide only whether to ask. The full head must verify
    /// through ordinary reconciliation and its signer must be one of the
    /// device keys still present in this database; unlike key-loss recovery,
    /// no unauthenticated sequence claim is acted on.
    pub async fn readopt_self_on_startup(&self) -> Result<bool> {
        let before = {
            let node = self.clone();
            crate::blocking::offload(move || Ok(node.store().complete_head(node.origin())?)).await?
        };
        let held_keys: std::collections::HashSet<NodeId> = {
            let node = self.clone();
            crate::blocking::offload(move || {
                Ok(node
                    .store()
                    .device_keys()?
                    .into_iter()
                    .map(|key| key.node_id)
                    .collect())
            })
            .await?
        };

        for (peer, addr) in self.dial_targets().await? {
            let client = match self.net().connect_mpt(addr).await {
                Ok(client) => client,
                Err(error) => {
                    tracing::debug!(peer = %peer.fmt_short(), %error, "startup readoption peer unreachable");
                    continue;
                }
            };
            match tokio::time::timeout(
                self.config().sync_round_budget,
                self.syncer().readopt_self_with(&client, &held_keys),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => tracing::debug!(
                    peer = %peer.fmt_short(),
                    %error,
                    "startup readoption exchange failed"
                ),
                Err(_) => tracing::debug!(
                    peer = %peer.fmt_short(),
                    "startup readoption exchange exceeded its sync budget"
                ),
            }
        }

        let after = {
            let node = self.clone();
            crate::blocking::offload(move || Ok(node.store().complete_head(node.origin())?)).await?
        };
        let adopted = match (before, after) {
            (Some(before), Some(after)) => after.supersedes(Some(&(before.seq, before.root))),
            (None, Some(_)) => true,
            _ => false,
        };
        if self.config().cloud.is_some() {
            self.reconstruct_recovered_cloud_rows().await?;
        }
        Ok(adopted)
    }

    /// Rebuilds cloud durability rows named by recovered own availability ads.
    pub(crate) async fn reconstruct_recovered_cloud_rows(&self) -> Result<()> {
        // A Litestream snapshot can predate both a recovered entry and its blob
        // row. Signed own `b:` records carry the trustworthy sizes; probing
        // with an empty range reconstructs cold rows without downloading
        // payload, including b-only pins. This finishes before maintenance can
        // retire ads or drain stale deletes.
        let advertised = {
            let node = self.clone();
            crate::blocking::offload(move || {
                let mut advertised = Vec::new();
                for root in node.store().provider_roots_for_origin(node.origin())? {
                    if let Some((_, ad)) = node
                        .store()
                        .providers(&root)?
                        .into_iter()
                        .find(|(origin, ad)| origin == node.origin() && ad.is_complete())
                    {
                        advertised.push((root, ad.size));
                    }
                }
                Ok(advertised)
            })
            .await?
        };
        for (root, size) in advertised {
            self.cas_backend()
                .ensure_ranges(root, size, synch_core::ChunkRanges::empty())
                .await?;
            let durable = {
                let node = self.clone();
                crate::blocking::offload(move || {
                    Ok(node
                        .store()
                        .blob(&root)?
                        .is_some_and(|row| row.durable && row.size == size))
                })
                .await?
            };
            if !durable {
                return Err(EngineError::NotFound(format!(
                    "recovered own ad names unavailable cloud object {root} ({size} bytes)"
                )));
            }
            let node = self.clone();
            crate::blocking::offload(move || {
                // The head has no separate pin record. A complete own b-only ad
                // is the surviving evidence of a bare pin, so recover it
                // conservatively; pinning a stale ad leaks bytes, while failing
                // to pin can delete an acknowledged durability promise.
                if !node.store().content_is_referenced(&root)? {
                    node.store().pin(
                        &root,
                        &synch_store::PinHolder::Operator,
                        synch_core::now_ns(),
                    )?;
                }
                Ok(())
            })
            .await?;
        }
        Ok(())
    }

    /// The recovery settings this node was opened with.
    pub fn recovery_options(&self) -> RecoveryOptions {
        RecoveryOptions {
            wait: self.config().recovery_quiesce,
            gap: self.config().seq_gap,
            poll: self.config().aae_interval,
        }
    }

    /// The highest seq any peer has advertised for this node's own origin.
    pub fn observed_seq(&self) -> Result<Option<u64>> {
        Ok(self
            .store()
            .observed_head(self.origin())?
            .map(|observed| observed.seq))
    }

    /// Whether this node is in key-loss recovery, and what it has seen (§3.4).
    pub fn recovery_state(&self) -> Result<RecoveryState> {
        let own_seq = self.store().complete_head(self.origin())?.map(|h| h.seq);
        let observed = self.store().observed_head(self.origin())?;
        let next_seq = self.next_seq()?;
        // Holding a head of our own settles the question: whatever peers say,
        // we have published under this origin ourselves. Otherwise a peer
        // advertising a head at or above what we would publish at means our
        // next publish would be correctly rejected — which is the state the
        // operator has to resolve.
        let in_recovery = own_seq.is_none() && observed.as_ref().is_some_and(|o| o.seq >= next_seq);
        Ok(RecoveryState {
            origin: self.origin().clone(),
            in_recovery,
            observed_seq: observed.as_ref().map(|o| o.seq),
            observed_by: observed.as_ref().and_then(|o| o.claimed_by),
            floor: self.store().publish_floor()?,
            next_seq,
        })
    }

    /// Refuses the publish if this node is in recovery (§3.4 step 1).
    ///
    /// Publishing from a wiped database would mint heads every peer rejects,
    /// with nothing on either side saying why. The error names the command that
    /// resolves it.
    ///
    /// Public so that callers which do irreversible work *before* publishing —
    /// hashing a tree, dropping a space — can refuse before doing it rather
    /// than after.
    pub fn ensure_publishable(&self) -> Result<()> {
        let state = self.recovery_state()?;
        if state.in_recovery {
            return Err(EngineError::InRecovery {
                origin: state.origin,
                observed_seq: state.observed_seq.unwrap_or_default(),
                would_publish: state.next_seq,
            });
        }
        Ok(())
    }

    /// Runs one collection round: a `Hello` exchange with every dialable peer,
    /// adopting nothing (§3.4 step 2).
    pub(crate) async fn observe_peers(&self) -> Result<ObserveRound> {
        let mut round = ObserveRound::default();
        for (peer, addr) in self.dial_targets().await? {
            match self.net().connect_mpt(addr).await {
                Ok(client) => match self.syncer().observe_with(&client).await {
                    Ok(_summaries) => {
                        let node = self.clone();
                        crate::blocking::offload(move || {
                            Ok(node.store().record_peer_seen(&peer, None, now_ns())?)
                        })
                        .await?;
                        round.reached.push(peer);
                    }
                    Err(e) => {
                        tracing::debug!(peer = %peer.fmt_short(), error = %e, "head exchange failed");
                        round.unreachable.push(peer);
                    }
                },
                Err(e) => {
                    tracing::debug!(peer = %peer.fmt_short(), error = %e, "peer unreachable");
                    round.unreachable.push(peer);
                }
            }
        }
        round.observed_seq = {
            let node = self.clone();
            crate::blocking::offload(move || node.observed_seq()).await?
        };
        Ok(round)
    }

    /// Collects peer summaries for the quiesce, then lifts the publishing floor
    /// above everything seen (§3.4 step 3).
    ///
    /// Progress is streamed on `progress` as each round completes, so an hour's
    /// wait is visible rather than silent. Dropping the receiving end — a CLI
    /// client that walked away — interrupts the quiesce and leaves the floor
    /// untouched: recovery is one deliberate act, not a half-applied one.
    pub async fn recover(
        &self,
        options: RecoveryOptions,
        progress: UnboundedSender<RecoveryProgress>,
    ) -> Result<RecoveryReport> {
        if options.gap == 0 {
            return Err(EngineError::invalid(
                "a recovery gap of 0 would set the floor to a seq peers have already seen: \
                 the gap is what makes a collision with history held only by an unreachable \
                 peer improbable (§3.4)",
            ));
        }
        let started = Instant::now();
        let deadline = started + options.wait;
        let mut rounds = 0usize;
        let mut reached: BTreeSet<NodeId> = BTreeSet::new();
        let mut unreachable: BTreeSet<NodeId> = BTreeSet::new();

        loop {
            let round = self.observe_peers().await?;
            rounds += 1;
            for peer in &round.reached {
                reached.insert(*peer);
                unreachable.remove(peer);
            }
            for peer in &round.unreachable {
                if !reached.contains(peer) {
                    unreachable.insert(*peer);
                }
            }

            let now = Instant::now();
            let update = RecoveryProgress {
                round: rounds,
                elapsed: now.saturating_duration_since(started),
                remaining: deadline.saturating_duration_since(now),
                reached: round.reached.len(),
                unreachable: round.unreachable.len(),
                observed_seq: round.observed_seq,
            };
            if progress.send(update).is_err() {
                return Err(EngineError::invalid(
                    "recovery was interrupted before the quiesce elapsed; \
                     the publishing floor is unchanged",
                ));
            }
            if now >= deadline {
                break;
            }
            // One sleep per round, bounded by what is left of the quiesce: the
            // wait costs a timer, never a spin.
            tokio::time::sleep(options.poll.min(deadline - now)).await;
        }

        // Everything from here is store work — the observation, this node's own
        // head, the seq it would publish at, and the durable floor write — so
        // it goes over to the blocking pool in one piece rather than four
        // acquisitions of the write connection on a runtime worker (§10).
        let node = self.clone();
        let waited = started.elapsed();
        let (reached_len, unreachable_len) = (reached.len(), unreachable.len());
        crate::blocking::offload(move || {
            node.settle_recovery_floor(options.gap, rounds, reached_len, unreachable_len, waited)
        })
        .await
    }

    /// The tail of [`Node::recover`]: decide the floor from what was observed
    /// and record it.
    ///
    /// Split out so the whole decision runs in one hop to the blocking pool,
    /// and so the early returns stay readable.
    fn settle_recovery_floor(
        &self,
        gap: u64,
        rounds: usize,
        reached: usize,
        unreachable: usize,
        waited: std::time::Duration,
    ) -> Result<RecoveryReport> {
        let observed = self.store().observed_head(self.origin())?;
        let mut report = RecoveryReport {
            origin: self.origin().clone(),
            observed_seq: observed.as_ref().map(|o| o.seq),
            floor: self.store().publish_floor()?,
            gap,
            rounds,
            reached,
            unreachable,
            waited,
        };
        let Some(observed) = observed else {
            // Nothing to resume from: no peer claims this origin ever
            // published. A genuinely fresh node is exactly this case, and it
            // must keep starting at seq 1.
            return Ok(report);
        };

        // A node holding its own current head is not resuming from loss, and
        // peers echoing our published history back is not history to leap
        // over: re-running recover after a successful recovery would otherwise
        // burn another gap's worth of seqs every time. Only an observation
        // beyond our own head says some peer holds history we lost.
        if let Some(own) = self.store().complete_head(self.origin())? {
            if own.seq >= observed.seq {
                tracing::info!(
                    origin = %self.origin(),
                    own = own.seq,
                    observed = observed.seq,
                    "peers advertise nothing beyond our own head; the floor stays put"
                );
                return Ok(report);
            }
        }

        // Never below what this node would publish anyway, and never below a
        // floor already in force.
        //
        // And never further above it than a plausible history could reach. The
        // observed seq is an *unauthenticated* summary — §3.4 accepts that any
        // member can assert a huge one and hold a fresh node in recovery, since
        // an operator reads the claim and its attribution and judges it. What
        // that reasoning does not cover is this floor, which is durable, only
        // ever rises, and has no command to lower it: a single absurd claim,
        // acted on once by the operator running the documented remedy, would
        // retire the origin permanently. Capping the *step* leaves the
        // transient half exactly as §3.4 describes it while keeping one bad
        // claim from being unrecoverable — a genuinely higher peer seq is
        // reached in as many recoveries as it takes, each one visible.
        let next = self.next_seq()?;
        let ceiling = next.saturating_add(MAX_RECOVERY_STEP);
        if observed.seq > ceiling {
            tracing::warn!(
                origin = %self.origin(),
                observed = observed.seq,
                ceiling,
                claimed_by = observed
                    .claimed_by
                    .map(|k| k.fmt_short().to_string())
                    .unwrap_or_default(),
                "a peer advertises a seq no plausible history reaches: clamping the floor"
            );
        }
        let floor = observed.seq.min(ceiling).saturating_add(gap).max(next);
        let floor = self.store().raise_publish_floor(floor)?;
        report.floor = Some(floor);
        tracing::info!(
            origin = %self.origin(),
            observed = observed.seq,
            floor,
            "recovery complete: publishing resumes above every seq peers advertised"
        );
        Ok(report)
    }

    /// Pre-recovery history we hold that the current head does not supersede
    /// (§3.4, §4.4).
    ///
    /// A head signed by a key that is no longer bound to its origin, at or
    /// above that origin's current published head, is history the origin's new
    /// key has not spoken for. On a peer that was partitioned through someone
    /// else's recovery, that is the fork evidence — retained *with* its
    /// signature, so it is provable rather than merely asserted.
    pub(crate) fn unreconciled_history(
        &self,
        origin: &OriginId,
    ) -> Result<Vec<UnreconciledHistory>> {
        let now = now_ns();
        let current_seq = self.store().complete_head(origin)?.map(|h| h.seq);
        let mut out = Vec::new();
        for head in self.store().head_history(origin)? {
            if head.seq < current_seq.unwrap_or(0) {
                continue;
            }
            if self.store().is_bound(origin, &head.signed_by, now)? {
                continue;
            }
            out.push(UnreconciledHistory {
                origin: origin.clone(),
                seq: head.seq,
                root: head.root,
                signed_by: head.signed_by,
                current_seq,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::node_as;

    /// One staged file entry, encoded the way the scanner encodes them.
    fn staged_file(node: &Node) -> crate::node::StagedChange {
        node.store()
            .put_source("s", synch_store::SourceKind::Api, None)
            .unwrap();
        let root = node.store().ingest_bytes(b"payload", 0).unwrap();
        let entry = synch_core::FileEntry::file(7, 0, root, 1);
        (
            synch_core::file_key("s", "a.txt").unwrap(),
            Some(postcard::to_stdvec(&entry).unwrap()),
        )
    }

    fn nas() -> OriginId {
        OriginId::named("nas", "cluster.example").unwrap()
    }

    /// A `--wait 0` options literal, the common case.
    fn quick(gap: u64) -> RecoveryOptions {
        RecoveryOptions {
            wait: Duration::ZERO,
            gap,
            poll: Duration::from_secs(30),
        }
    }

    /// Records one peer summary for our own origin, as an `observe` round
    /// would; `claimed_by` names the peer the operator is hearing it from.
    fn observe(node: &Node, seq: u64, claimed_by: Option<&iroh_base::PublicKey>) {
        node.store()
            .record_observed_head(
                node.origin(),
                seq,
                &Hash([1u8; 32]),
                true,
                claimed_by,
                now_ns(),
            )
            .unwrap();
    }

    /// A node holding a head of its own is not in recovery, whatever peers
    /// advertise: it has published itself (§3.4).
    #[tokio::test]
    async fn holding_our_own_head_settles_the_question() {
        let (_d, node) = node_as(&nas()).await;
        node.publish(&[staged_file(&node)]).unwrap().unwrap();
        observe(&node, 100, None);
        assert!(!node.recovery_state().unwrap().in_recovery);
        node.ensure_publishable().unwrap();
        node.shutdown().await.unwrap();
    }

    /// `--wait 0` collects one round and returns; the floor lands above
    /// everything seen (§3.4). A node holding its own head ignores the echo
    /// of its published history, but never real evidence.
    #[tokio::test]
    async fn recover_sets_the_floor_above_every_observation() {
        // A headless node: the floor lands above everything seen.
        let (_d, node) = node_as(&nas()).await;

        // A node no peer knows: no floor, and seq 1 is left alone.
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let report = node.recover(quick(1_000), tx).await.unwrap();
        assert_eq!(report.observed_seq, None);
        assert_eq!(report.floor, None);
        assert_eq!(node.next_seq().unwrap(), 1);

        // The gap is not an optimization to remove: it exists to prevent a
        // collision at the highest advertised seq.
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let err = node
            .recover(
                RecoveryOptions {
                    gap: 0,
                    ..quick(1_000)
                },
                tx,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("gap"), "{err}");

        observe(&node, 100, None);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let report = node.recover(quick(1_000), tx).await.unwrap();
        assert_eq!(report.rounds, 1);
        assert_eq!(report.observed_seq, Some(100));
        assert_eq!(report.floor, Some(1_100));
        assert_eq!(rx.try_recv().unwrap().round, 1);

        assert_eq!(node.next_seq().unwrap(), 1_100);
        assert!(!node.recovery_state().unwrap().in_recovery);
        node.ensure_publishable().unwrap();

        // An observation below the floor is no return to recovery: the floor
        // already clears it.
        observe(&node, 200, None);
        assert!(!node.recovery_state().unwrap().in_recovery);

        // One above it is: publishing at the floor would now collide.
        observe(&node, 5_000, None);
        assert!(node.recovery_state().unwrap().in_recovery);
        node.shutdown().await.unwrap();

        // Holding our own head, the echo of our published history leaves the
        // floor alone; an accidental re-run would otherwise burn another gap.
        let (_d, node) = node_as(&nas()).await;
        node.publish(&[staged_file(&node)]).unwrap().unwrap();
        let own = node.store().complete_head(node.origin()).unwrap().unwrap();
        node.store()
            .record_observed_head(node.origin(), own.seq, &own.root, true, None, now_ns())
            .unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let report = node.recover(quick(1_000), tx).await.unwrap();
        assert_eq!(report.floor, None, "{report:?}");
        assert_eq!(node.next_seq().unwrap(), own.seq + 1);

        // Genuinely newer history still raises the floor: only the echo is
        // ignored, not real evidence.
        node.store()
            .record_observed_head(
                node.origin(),
                own.seq + 50,
                &Hash([9u8; 32]),
                true,
                None,
                now_ns(),
            )
            .unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let report = node.recover(quick(1_000), tx).await.unwrap();
        assert_eq!(report.floor, Some(own.seq + 50 + 1_000));
        node.shutdown().await.unwrap();
    }

    /// A node nobody has ever heard of is not in recovery, and publishes at
    /// seq 1; a peer advertising a *different* origin says nothing about ours.
    #[tokio::test]
    async fn a_fresh_node_is_not_in_recovery() {
        let (_d, node) = node_as(&nas()).await;
        let state = node.recovery_state().unwrap();
        assert!(!state.in_recovery);
        assert_eq!(state.observed_seq, None);
        assert_eq!(state.next_seq, 1);
        node.ensure_publishable().unwrap();

        node.store()
            .record_observed_head(
                &OriginId::named("laptop", "cluster.example").unwrap(),
                900,
                &Hash([3u8; 32]),
                true,
                None,
                now_ns(),
            )
            .unwrap();
        assert!(!node.recovery_state().unwrap().in_recovery);
        node.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_dropped_progress_receiver_interrupts_the_quiesce() {
        let (_d, node) = node_as(&nas()).await;
        observe(&node, 100, None);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        drop(rx);
        let err = node
            .recover(
                RecoveryOptions {
                    wait: Duration::from_secs(3_600),
                    poll: Duration::from_millis(10),
                    ..quick(1_000)
                },
                tx,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("interrupted"), "{err}");
        assert_eq!(node.store().publish_floor().unwrap(), None);
        assert!(node.recovery_state().unwrap().in_recovery);
        node.shutdown().await.unwrap();
    }
}
