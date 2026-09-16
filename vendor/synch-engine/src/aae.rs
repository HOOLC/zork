//! Anti-entropy scheduling (§5.3).
//!
//! Reactive: a local publish is pushed to every trusted peer immediately —
//! the whole membership, not just current connections — which gives sub-second
//! propagation on a connected cluster. A head *received* from a peer is not
//! relayed onward; at the §12 sizes the publisher's own fan-out already reaches
//! everyone it can reach, so the pull path below is what covers a member the
//! origin cannot dial. Periodic: every `aae_interval` (±50 % jitter) a
//! bounded random sample of trusted peers gets a full `Hello` push-pull
//! exchange. Candidates are tried in sequence until one advances local state,
//! so a bad member cannot hide a healthy one without turning every round into
//! an all-to-all exchange.

use std::time::Duration;

use crate::reconcile::SyncReport;
use synch_core::{now_ns, NodeId, SignedHead};

use crate::{
    error::{EngineError, Result},
    node::Node,
};

/// The outcome of one anti-entropy round.
#[derive(Debug, Clone, Default)]
pub struct RoundReport {
    /// The peer whose exchange is reported, if any was reachable.
    pub peer: Option<NodeId>,
    /// What the exchange achieved.
    pub sync: SyncReport,
    /// Peers that could not be reached this round.
    pub unreachable: usize,
}

/// The shortest gap between two anti-entropy rounds driven by a pushed head.
///
/// Even a bounded round may dial several candidates, so answering each push
/// with its own round would let one origin publishing in a burst — an import,
/// a large rename — turn one node's publishes into a dial storm. The bell holds
/// at most one permit, so a burst arriving inside the floor costs one extra
/// round rather than one per head.
const REACTIVE_FLOOR: Duration = Duration::from_secs(2);

/// How many head pushes may be in flight at once.
///
/// The reactive path reaches the whole membership, and on a hosted replica
/// that is hundreds of peers rather than the N ≤ 100 §12 assumes. What is
/// bounded here is not the work — every peer is still pushed to — but how much
/// of it is resident at any instant: an in-flight dial holds a QUIC handshake,
/// its buffers and a descriptor, and file descriptors are what
/// `docs/CLOUD-DATAPLANE.md` §7.1 names as the practical ceiling.
///
/// Thirty-two is chosen to keep the wall time of a full fan-out inside the
/// two-second reactive floor above at ordinary dial latencies, so the bound
/// costs propagation nothing that the floor was not already spending.
const PUSH_FANOUT: usize = 32;

/// Maximum peers considered by one standing anti-entropy round.
const ANTI_ENTROPY_FANOUT: usize = 3;

/// Per-peer budget used by the standing scheduler.
///
/// Explicit `sync_with_peer` calls retain the operator's full configured
/// budget. Periodic repair needs a smaller ceiling so a responsive but broken
/// peer cannot consume minutes before the next candidate is tried.
const PERIODIC_PEER_BUDGET: Duration = Duration::from_secs(30);

impl Node {
    /// Runs one `Hello` push-pull exchange with a specific peer.
    ///
    /// Bounded as a whole, not only per exchange. `synch-net` puts a deadline on
    /// every request, but how many requests one `sync_with` issues is the
    /// peer's to choose — one `fetch_pending` per head it hands back, each
    /// looping to [`MAX_UNPRODUCTIVE_ROUNDS`], plus a pass over every pending
    /// head — so per-request deadlines compose into no bound at all. A peer
    /// answering just inside each one could monopolize a sequential scheduler.
    /// Explicit calls get the configured whole-round budget. The standing
    /// scheduler uses a smaller per-peer cap while trying its bounded fallback
    /// candidates.
    ///
    /// [`MAX_UNPRODUCTIVE_ROUNDS`]: crate::reconcile::MAX_UNPRODUCTIVE_ROUNDS
    pub async fn sync_with_peer(&self, node_id: &NodeId) -> Result<SyncReport> {
        self.sync_with_peer_budget(node_id, self.config().sync_round_budget)
            .await
    }

    async fn sync_with_peer_budget(
        &self,
        node_id: &NodeId,
        budget: Duration,
    ) -> Result<SyncReport> {
        tokio::time::timeout(budget, self.sync_with_peer_inner(node_id))
            .await
            .map_err(|_| {
                EngineError::invalid(format!(
                    "the sync round with {} outran its {budget:?} budget",
                    node_id.fmt_short()
                ))
            })?
    }

    async fn sync_with_peer_inner(&self, node_id: &NodeId) -> Result<SyncReport> {
        // The address lookup and latency record are store work and must stay
        // off the runtime worker driving the endpoint (§10).
        let addr = {
            let node = self.clone();
            let key = *node_id;
            crate::blocking::offload(move || node.peer_addr(&key)).await?
        }
        .unwrap_or_else(|| iroh::EndpointAddr::new(*node_id));
        let started = std::time::Instant::now();
        let client = self.net().connect_mpt(addr).await?;
        let report = self.syncer().sync_with(&client).await?;
        let elapsed = started.elapsed().as_micros().min(i64::MAX as u128) as i64;
        let node = self.clone();
        let key = *node_id;
        crate::blocking::offload(move || {
            Ok(node.store().record_peer_sync(&key, now_ns(), elapsed)?)
        })
        .await?;
        Ok(report)
    }

    /// [`Node::dialable_peers`] on the blocking pool.
    ///
    /// Two queries over `device_keys` and `bindings`, which is store work like
    /// any other: every async caller reaches the set this way.
    pub(crate) async fn dialable_peers_off_runtime(&self) -> Result<Vec<NodeId>> {
        let node = self.clone();
        crate::blocking::offload(move || node.dialable_peers()).await
    }

    /// Every peer this node may dial, with the last address it was seen at.
    ///
    /// One hop to the blocking pool for the whole membership rather than a
    /// `peers_seen` read per peer on the runtime worker (§10) — and one
    /// definition of "who do I dial and where", instead of the same loop
    /// written out in the push, the recovery quiesce and the rotation probe.
    pub(crate) async fn dial_targets(&self) -> Result<Vec<(NodeId, iroh::EndpointAddr)>> {
        let node = self.clone();
        crate::blocking::offload(move || {
            let peers = node.dialable_peers()?;
            let mut targets = Vec::with_capacity(peers.len());
            for peer in peers {
                let addr = node
                    .peer_addr(&peer)?
                    .unwrap_or_else(|| iroh::EndpointAddr::new(peer));
                targets.push((peer, addr));
            }
            Ok(targets)
        })
        .await
    }

    /// The trusted device keys this node may dial, excluding its own.
    pub fn dialable_peers(&self) -> Result<Vec<NodeId>> {
        // Every key this node holds, not just the active one: through a
        // rotation window the retiring key is still bound to our own origin,
        // and filtering only the active key would leave the node dialling
        // itself and reporting itself unreachable — in exactly the window where
        // an operator is watching `peer sync` and `key ls` the hardest (§3.4).
        let own: Vec<NodeId> = self
            .device_keys()?
            .into_iter()
            .map(|key| key.node_id)
            .collect();
        Ok(self
            .store()
            .trusted_keys(now_ns())?
            .into_iter()
            .filter(|k| !own.contains(k))
            .collect())
    }

    /// Runs one periodic round over a bounded random sample of trusted peers.
    ///
    /// Candidates are tried sequentially because reconciliation shares one
    /// pending slot per origin: concurrent exchanges can otherwise abandon a
    /// head while another healthy peer is fetching it. A reachable peer that
    /// only receives our state does not end the fallback; the round stops when
    /// an exchange advances local state or the sample is exhausted.
    pub async fn anti_entropy_round(&self) -> Result<RoundReport> {
        let mut peers = self.dialable_peers_off_runtime().await?;
        if peers.is_empty() {
            return Ok(RoundReport::default());
        }
        let start = (jitter_seed() % peers.len() as u64) as usize;
        peers.rotate_left(start);
        peers.truncate(ANTI_ENTROPY_FANOUT);

        let mut report = RoundReport::default();
        let budget = self.config().sync_round_budget.min(PERIODIC_PEER_BUDGET);
        for peer in peers {
            match self.sync_with_peer_budget(&peer, budget).await {
                Ok(sync) => {
                    let made_local_progress = sync.made_local_progress();
                    if report.peer.is_none() || made_local_progress {
                        report.peer = Some(peer);
                        report.sync = sync;
                    }
                    if made_local_progress {
                        return Ok(report);
                    }
                }
                Err(e) => {
                    tracing::debug!(peer = %peer.fmt_short(), error = %e, "peer unreachable");
                    report.unreachable += 1;
                }
            }
        }
        Ok(report)
    }

    /// Pushes a head to every trusted peer (§5.3, reactive path).
    ///
    /// Concurrently, because each push is bounded by a dial timeout and a
    /// request deadline: sequentially those add up across the membership and a
    /// publish waits for all of them before it returns, while run together one
    /// slow peer costs one deadline rather than delaying every peer behind it.
    ///
    /// Bounded concurrently, though, at `PUSH_FANOUT`. Every peer at once is
    /// the shape §5.3 was written for at the N ≤ 100 sizes §12 assumes, and it
    /// is a dial storm at the sizes a hosted replica sees: an in-flight dial
    /// holds a QUIC handshake, its buffers and a file descriptor, and this is
    /// reached from the standing replica loop through
    /// [`Node::publish_material_claims`] whenever coverage moves. The buffer
    /// admits the next peer the instant one finishes, so the total is still one
    /// deadline plus the queue rather than a deadline per peer.
    pub async fn push_head(&self, head: &SignedHead) -> Result<usize> {
        let targets = self.dial_targets().await?;
        let pushes: Vec<_> = targets
            .into_iter()
            .map(|(peer, addr)| async move {
                match self.net().connect_mpt(addr).await {
                    Ok(client) => match client.push_head(head).await {
                        Ok(()) => true,
                        Err(e) => {
                            tracing::debug!(peer = %peer.fmt_short(), error = %e, "head push failed");
                            false
                        }
                    },
                    Err(e) => {
                        tracing::debug!(peer = %peer.fmt_short(), error = %e, "peer unreachable");
                        false
                    }
                }
            })
            .collect();
        let results = crate::join::futures_buffered(pushes, PUSH_FANOUT).await;
        Ok(results.into_iter().filter(|pushed| *pushed).count())
    }

    /// Scans, publishes, and pushes the resulting head in one step.
    ///
    /// The scan stages into the publisher and then flushes it, so the result is
    /// one batch and one head — including anything a watcher-triggered rescan
    /// had already staged — and the head is out before this returns.
    pub async fn scan_publish_push(&self) -> Result<Option<SignedHead>> {
        self.scan_and_stage_async().await?;
        self.flush_staged().await
    }

    /// The next anti-entropy delay: the configured interval with ±50 % jitter.
    pub(crate) fn next_aae_delay(&self) -> Duration {
        jittered(self.config().aae_interval)
    }

    /// Runs the periodic anti-entropy loop until `shutdown` resolves.
    ///
    /// Woken by the clock or by a head landing in the pending slot, whichever
    /// comes first. The second is what makes reactive push mean anything: a
    /// pushed head names a root this node has never seen, and adopting it into
    /// the pending slot is not convergence — nothing a reader looks at moves
    /// until the trie under it is fetched and the head promotes. Without this
    /// arm that fetch waited for the next jittered interval, so §5.3's
    /// sub-second propagation delivered a pointer and the data followed up to
    /// 45 s later.
    pub async fn run_anti_entropy(&self, shutdown: impl std::future::Future<Output = ()>) {
        let mut shutdown = std::pin::pin!(shutdown);
        let pending = self.pending_wake();
        let mut last = tokio::time::Instant::now() - REACTIVE_FLOOR;
        loop {
            let delay = self.next_aae_delay();
            let reason = tokio::select! {
                _ = &mut shutdown => return,
                _ = tokio::time::sleep(delay) => "the interval",
                _ = pending.notified() => "a pushed head needing its trie",
            };
            // A bell that rings inside the floor is answered late rather than
            // dropped: an origin publishing in a burst pushes once per head,
            // and each round may dial several candidates. The interval arm is
            // never delayed by this, because it is already longer.
            let since = last.elapsed();
            if since < REACTIVE_FLOOR {
                tokio::select! {
                    _ = &mut shutdown => return,
                    _ = tokio::time::sleep(REACTIVE_FLOOR - since) => {}
                }
            }
            tracing::trace!(reason, "anti-entropy round");
            last = tokio::time::Instant::now();
            if let Err(e) = self.anti_entropy_round().await {
                tracing::warn!(error = %e, "anti-entropy round failed");
            }
        }
    }

    /// Runs the periodic maintenance loop: GC, binding expiry, and tombstone
    /// expiry (§5.4, §3.2, §4.2).
    pub async fn run_maintenance(&self, shutdown: impl std::future::Future<Output = ()>) {
        let mut shutdown = std::pin::pin!(shutdown);
        loop {
            tokio::select! {
                _ = &mut shutdown => return,
                // On the blocking pool: a GC pass sweeps the trie and unlinks
                // every unreferenced payload, which is proportional to what
                // the store has accumulated since the last one (§5.4).
                _ = tokio::time::sleep(Duration::from_secs(300)) => {
                    let node = self.clone();
                    let blocking = node.clone();
                    let pass = crate::blocking::offload(move || blocking.maintenance_pass()).await;
                    if let Err(e) = pass {
                        tracing::warn!(error = %e, "maintenance pass failed");
                    }
                    match node.cas_backend().maintain(now_ns()).await {
                        Ok(report)
                            if report.local_orphans > 0
                                || report.cache_entries_evicted > 0 =>
                        {
                            tracing::info!(
                                local_orphans = report.local_orphans,
                                cache_entries_evicted = report.cache_entries_evicted,
                                cache_bytes_evicted = report.cache_bytes_evicted,
                                "CAS backend maintenance completed"
                            );
                        }
                        Ok(_) => {}
                        Err(e) => tracing::warn!(error = %e, "CAS backend maintenance failed"),
                    }
                }
            }
        }
    }

    /// One maintenance pass, exposed so tests and `synch doctor` can drive it.
    ///
    /// The order is the one §5.4 implies and it is not arbitrary. History is
    /// pruned to `root_retention` *first*, because the trie mark set is
    /// "complete + pending heads + remaining history roots" — marking from
    /// every root ever recorded means nothing is ever swept, which is exactly
    /// the state this pass exists to avoid. Then the trie sweep runs, and then
    /// content: an object drops out of `referenced_content` only once the
    /// entries naming it are gone, which is what the trie sweep produces.
    ///
    /// Tombstones past `tombstone_ttl` are *staged* here rather than published:
    /// the publisher turns them into one head like any other batch (§4.2).
    pub fn maintenance_pass(&self) -> Result<synch_store::GcStats> {
        let now = now_ns();
        // The pass that reaps expiring trust is also the one that records how
        // far the clock has got: a persisted floor is what stops a backwards
        // step from handing back trust that already lapsed (§3.2,
        // `synch_store::clock`). A reading that can date nothing advances
        // nothing and reaps nothing, and says so — membership stops being
        // extended, which is loud in `doctor` rather than silent here.
        if synch_core::clock_is_trusted(now) {
            contained(
                "advancing the trust floor",
                self.store().advance_trust_floor(now),
            );
        } else {
            tracing::warn!(
                reading = now,
                "the host clock cannot date a trust decision: no dns binding is honored and \
                 none is expired until it is set (see `synch doctor`)"
            );
        }
        if let Some(expired) = contained("expiring bindings", self.store().expire_bindings(now)) {
            if expired > 0 {
                tracing::info!(expired, "dns bindings lapsed");
            }
        }
        // A read scope that no grant stands behind is collapsed here rather
        // than at adoption: the delegate it names is cut off at the
        // connection gate the moment its binding dies, so no peer's
        // declaration ever reaches it again (§3.2, §5.5). The clock is the
        // only thing that can drive the derived views away — this is the
        // same destructive move `adopt_scope` makes for a moved grant,
        // applied to the one that expired.
        contained(
            "collapsing a grantless read scope",
            self.store().collapse_grantless_scope(now),
        );
        contained("expiring tombstones", self.expire_tombstones());
        // The catch-all for claims a replication sweep will not visit again:
        // a space removed with its pins kept still has releases that were
        // scheduled before it went, and nothing else would ever run them
        // (`docs/REPLICATION.md` §3.1).
        // The store's reading, not the bare `now` this pass carries: releases
        // are scheduled against `read_instant` and this is the second thing
        // that runs them, so the two must date by the same clock or the
        // catch-all and the sweep disagree about when a grace window ended.
        let releasing = self
            .store()
            .read_instant()
            .and_then(|reading| self.store().expire_pins(reading))
            .map_err(EngineError::from);
        if let Some(expired) = contained("expiring pins", releasing) {
            if expired > 0 {
                tracing::info!(expired, "scheduled releases fell due");
            }
        }
        self.sweep_pending_heads(now);
        // Ads for objects content GC has since dropped. Staged before the
        // content sweep below rather than after, so a root that goes this pass
        // is retired next pass rather than lingering an interval — and so the
        // ad and the payload never disagree in the direction that has peers
        // dialling us for bytes we no longer have (§6.3).
        contained("reconciling ads", self.reconcile_ads());

        let retention = self
            .config()
            .root_retention
            .as_nanos()
            .min(i64::MAX as u128) as i64;
        let before = now.saturating_sub(retention);
        let mut pruned = 0;
        // Only the origins with a row old enough to be taken. Pruning opens an
        // immediate transaction per origin, so asking every origin in the
        // history cost one write-lock acquisition each to conclude there was
        // nothing to do — five minutely, ahead of every other tenant's store
        // work on a shard (`docs/CLOUD-DATAPLANE.md` §7.1a). The filter is
        // exact rather than a heuristic: every deletion below requires
        // `recorded_at < before`, so an origin with no such row has nothing
        // this pass could take.
        for origin in self.store().history_origins_before(before)? {
            // Per origin, so one origin's history cannot stop another's from
            // being pruned — and so the trie sweep below still runs.
            if let Some(n) = contained(
                "pruning history",
                self.store().prune_history_before(&origin, before),
            ) {
                pruned += n;
            }
        }
        if pruned > 0 {
            tracing::info!(pruned, "old roots dropped out of retention");
        }
        let stats = self.store().gc(before)?;
        if stats.nodes > 0 || stats.values > 0 || stats.blobs > 0 || stats.orphans > 0 {
            tracing::info!(
                nodes = stats.nodes,
                values = stats.values,
                blobs = stats.blobs,
                orphans = stats.orphans,
                "garbage collected"
            );
        }
        Ok(stats)
    }

    /// Decides what becomes of every pending head: promote it, abandon it, or
    /// leave it to the fetch that is still working on it (§5.2).
    ///
    /// One pass over the slots rather than two, and — the part that matters —
    /// **contained per origin**. Every step here can fail on data one origin
    /// published: `try_promote` ends in `materialize_diff`, which raises on a
    /// record that will not decode, and `is_complete` raises on a node graph
    /// the structural guard refuses. Propagating either aborted the whole
    /// maintenance pass, so nothing after it ran — not the abandonment sweep,
    /// not ad retirement, not history pruning, and not `gc()`. A single member
    /// publishing one undecodable `f:` value therefore disabled garbage
    /// collection on every peer that adopted the head, permanently and
    /// silently, with a `warn!` every five minutes as the only trace. That is
    /// the opposite of what §12 promises: "a record this node cannot apply
    /// fails its own origin and no other".
    ///
    /// Three outcomes per head:
    ///
    /// - **Promoted**, when its trie is wholly here. `try_promote` otherwise
    ///   runs only from an accepted offer and from the end of a successful
    ///   fetch, and neither covers a crash between the last committed batch of
    ///   trie nodes and the promotion that would have followed.
    /// - **Abandoned**, when nobody has served it past `pending_head_ttl`, or
    ///   when promoting it *failed*. The second case is new and it is the
    ///   escape a poisoned head needs: `head_floor` is the best of both slots,
    ///   so a pending head holds the floor above every servable head for its
    ///   origin, and a head whose trie is complete but whose promotion cannot
    ///   succeed is stepped over by the TTL rule below for exactly the reason
    ///   that rule exists — it looks like it is one promotion away. It is not,
    ///   and it would hold that origin hostage forever.
    /// - **Left alone**, when a fetch is still working on it.
    fn sweep_pending_heads(&self, now: i64) {
        let ttl = self
            .config()
            .pending_head_ttl
            .as_nanos()
            .min(i64::MAX as u128) as i64;
        let before = now.saturating_sub(ttl);
        let syncer = self.syncer();
        let trie = synch_mpt::Trie::new(self.store().as_ref());

        let Some(pending) = contained(
            "listing pending heads",
            self.store().all_heads(synch_store::Slot::Pending),
        ) else {
            return;
        };

        let (mut promoted, mut abandoned) = (0usize, 0usize);
        for stored in pending {
            let origin = &stored.head.origin;
            let outcome = syncer.try_promote(origin, now);
            let poisoned = match outcome {
                Ok(crate::reconcile::Promotion::Flipped) => {
                    promoted += 1;
                    continue;
                }
                // `try_promote` retired it on this call, so it counts — and
                // there is nothing left for the compare-and-clear below to
                // delete, which is why this returns early rather than falling
                // through to it.
                Ok(crate::reconcile::Promotion::Refused) => {
                    abandoned += 1;
                    continue;
                }
                // Both mean this pass has no verdict to record: the head
                // either still needs its trie, or is already gone.
                Ok(crate::reconcile::Promotion::Waiting)
                | Ok(crate::reconcile::Promotion::Idle) => false,
                // Only a fault in what the *origin* published condemns its head;
                // local database and I/O failures remain operational errors.
                // `try_promote` has already retired the head it judged and
                // recorded the verdict if it was a permanent one — and it is the
                // only party that knows *which* head that was, since the slot
                // can move under this loop between the `all_heads` snapshot and
                // the promotion. Naming `stored.head` here condemned whatever
                // this pass happened to list rather than what actually failed.
                Err(e) if crate::reconcile::is_origin_fault(&e) => {
                    // Warn, not debug. `try_promote` warns too when it got far
                    // enough to name the head — but when it did not, this is the
                    // only line an operator gets for a head that is about to be
                    // permanently abandoned.
                    tracing::warn!(
                        origin = %origin,
                        error = %e,
                        "a pending head for this origin could not be materialized"
                    );
                    true
                }
                Err(e) => {
                    tracing::warn!(
                        origin = %origin,
                        seq = stored.head.seq,
                        error = %e,
                        "could not decide a pending head this pass; leaving it where it is"
                    );
                    continue;
                }
            };

            // Scoped, like promotion: on a node reading under a scope the
            // unscoped answer is false by construction for every foreign
            // origin, so a pending head that failed promotion for any unrelated
            // reason would read as stale and be abandoned once its TTL passed
            // (§5.5). A scope this node cannot read is treated as the whole
            // keyspace, which is the conservative reading: it makes a head look
            // less complete, never more.
            //
            // `is_complete_scoped` walks a peer's node graph, so it can raise on
            // its own — but only in cases `try_promote` has already met: it
            // walks the same root over the transaction just above, against the
            // same rows and the same memo, so a graph this raises on made
            // `outcome` an `Err` and `poisoned` is already true. What is left is
            // the divergence the two walks *can* have, which is a transient
            // store failure on the second one, and the answer to that is to
            // leave the head where it is for a pass rather than to condemn it:
            // reading it as incomplete would make a `SQLITE_BUSY` past the TTL
            // abandon a head whose trie is wholly here.
            //
            // Only walked when the head is not already condemned: for a poisoned
            // head the answer is discarded, and walking anyway both wasted a
            // full trie walk and let the "leaving it for a pass" branch log a
            // reprieve for a head this pass is about to abandon.
            let scope = self
                .store()
                .local_trie_scope()
                .unwrap_or_else(|_| synch_mpt::Scope::full());
            // With the provenance promotion demands (§5.5), read
            // conservatively on a store error for the same reason as the scope.
            let owner = self
                .store()
                .provenance_owner(origin, now)
                .unwrap_or_else(|_| Some(origin.clone()));
            let stale = !poisoned
                && stored.received_at <= before
                && !match trie.is_complete_scoped_for(owner.as_ref(), stored.head.root, &scope) {
                    Ok(complete) => complete,
                    Err(e) => {
                        tracing::warn!(
                            origin = %origin,
                            error = %e,
                            "cannot decide whether a pending trie is here; leaving it for a pass"
                        );
                        true
                    }
                };
            if !poisoned && !stale {
                continue;
            }
            // Both cases fall through to the compare-and-clear below, the
            // poisoned one included even though `try_promote` will usually have
            // retired it already: it *compares*, so a head already gone reports
            // `dropped = false`, while a head `try_promote` never reached still
            // gets an exit. That second case is why this cannot be an early
            // `continue` — a fault raised before `try_promote` has read the head
            // it would condemn leaves nothing to retire it, and the slot then
            // holds `head_floor` above everything this node can serve.
            //
            // `abandoned` counts heads this pass retired, by whichever hand did
            // it — the same convention the `Refused` arm above uses. Counting
            // only what this statement deleted would report 0 for a pass whose
            // own `try_promote` did the retiring, which is most of them.
            if !poisoned {
                tracing::warn!(
                    origin = %origin,
                    seq = stored.head.seq,
                    "abandoning a pending head no peer will serve"
                );
            }
            // The head this pass judged, named explicitly. Between the snapshot
            // above and here the sweep ran a promotion transaction and a trie
            // walk, and a `HeadPush` accepted on the blocking pool in that
            // window would otherwise be deleted by a verdict reached about a
            // different head — and condemned by *its* `received_at`.
            let cleared = contained(
                "clearing a pending head",
                self.store().clear_head_at(
                    origin,
                    synch_store::Slot::Pending,
                    stored.head.seq,
                    &stored.head.root,
                ),
            );
            if cleared.is_some_and(|dropped| dropped) || poisoned {
                abandoned += 1;
            }
        }
        if promoted > 0 {
            tracing::info!(promoted, "pending heads whose tries were already here");
        }
        if abandoned > 0 {
            tracing::info!(abandoned, "pending heads this pass dropped");
        }
    }
}

/// Runs one maintenance step, reporting a failure rather than propagating it.
///
/// The pass is a sequence of independent sweeps over shared state, and its
/// later steps — ad retirement, history pruning, the trie and content sweeps —
/// are the ones that reclaim disk. Letting an earlier step's failure abort the
/// pass means a fault in *one* origin's data stops garbage collection for the
/// whole node, on every pass, forever. `gc_orphans` already documents that
/// hazard for itself ("`maintenance_pass` would report failure forever and no
/// orphan would be swept again"); this applies the same rule to the steps
/// ahead of it.
fn contained<T, E: std::fmt::Display>(what: &str, result: std::result::Result<T, E>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            tracing::warn!(step = what, %error, "a maintenance step failed; the pass continues");
            None
        }
    }
}

/// Applies ±50 % jitter to a duration.
///
/// Jitter is what keeps a cluster from synchronizing its rounds into a
/// thundering herd.
pub(crate) fn jittered(base: Duration) -> Duration {
    let base_ms = base.as_millis().max(1) as u64;
    let spread = base_ms; // ±50 % is a full base-width window centered on base
    let offset = jitter_seed() % spread.max(1);
    Duration::from_millis(base_ms / 2 + offset)
}

/// Applies +0–50 % jitter to a duration — never shorter than `base`.
///
/// The variant for backoff, where `base` is a floor: the centered [`jittered`]
/// can halve it, and a reconnect delay below the minimum backoff defeats the
/// backoff. The two used to share one name across this crate with different
/// semantics, which is a trap for whoever reaches for the wrong one.
pub(crate) fn jittered_floor(base: Duration) -> Duration {
    let span_ms = (base.as_millis() as u64) / 2;
    if span_ms == 0 {
        return base;
    }
    base + Duration::from_millis(jitter_seed() % span_ms)
}

/// Runs a standing pass-per-wake loop until `shutdown` resolves.
///
/// The standing replica workers run one pass before the first wait, so a node
/// restarted with a backlog starts working
/// through it rather than waiting out an interval; then one pass per ring of
/// `wake` or per jittered `interval`, whichever comes first — the interval
/// being the backstop for drift nobody rang a bell about.
pub(crate) async fn run_standing<F, Fut>(
    shutdown: impl std::future::Future<Output = ()>,
    wake: std::sync::Arc<tokio::sync::Notify>,
    interval: Duration,
    mut pass: F,
) where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let mut shutdown = std::pin::pin!(shutdown);
    loop {
        pass().await;
        tokio::select! {
            _ = &mut shutdown => return,
            _ = wake.notified() => {}
            _ = tokio::time::sleep(jittered(interval)) => {}
        }
    }
}

/// A cheap non-cryptographic source of jitter, seeded from the clock.
fn jitter_seed() -> u64 {
    let mut state = (now_ns() as u64) ^ 0x9e37_79b9_7f4a_7c15;
    state ^= state << 13;
    state ^= state >> 7;
    state ^= state << 17;
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{node, node_with};
    use synch_store::{Binding, BindingSource};

    /// One trusted binding, the literal that recurs through the sweep tests.
    fn bind(
        node: &Node,
        name: &str,
        key: &iroh_base::PublicKey,
        source: BindingSource,
        expires_at: Option<i64>,
    ) {
        node.store()
            .put_binding(&Binding {
                origin: synch_core::OriginId::named(name, "x.example").unwrap(),
                node_id: *key,
                source,
                domain: Some("x.example".into()),
                issuer: None,
                spaces: Vec::new(),
                note: None,
                added_at: 0,
                expires_at,
            })
            .unwrap();
    }

    #[tokio::test]
    async fn maintenance_expires_bindings_and_runs_gc() {
        let (_d, node) = node().await;
        let key = iroh_base::SecretKey::generate().public();
        bind(&node, "gone", &key, BindingSource::Dns, Some(1));
        node.maintenance_pass().unwrap();
        assert_eq!(
            node.store().bindings().unwrap().len(),
            1,
            "only self remains"
        );
        node.shutdown().await.unwrap();
    }
    /// A pending head nobody can serve stops holding an origin hostage.
    ///
    /// `head_floor` is the best of both slots, so a head pushed reactively by a
    /// publisher that then goes offline sits in the pending slot refusing every
    /// older head a peer could actually serve. Past the TTL the maintenance
    /// pass drops it and head selection re-runs.
    #[tokio::test]
    async fn a_pending_head_nobody_serves_is_abandoned_by_maintenance() {
        use synch_core::{file_key, FileEntry, Hash, SignedHead};
        use synch_store::Slot;

        let (_d, node) = node_with(|config| {
            config.pending_head_ttl = Duration::from_secs(60);
        })
        .await;

        let key = iroh_base::SecretKey::generate();
        let origin = synch_core::OriginId::named("nas", "x.example").unwrap();
        bind(&node, "nas", &key.public(), BindingSource::Static, None);

        // A root this node holds nothing of, pushed long enough ago that the
        // TTL has run out.
        let ttl_ns = 60 * 1_000_000_000i64;
        let now = now_ns();
        let stranded = SignedHead::sign(&key, origin.clone(), 5, Hash::new(b"unserved"), 0);
        node.store()
            .put_head(Slot::Pending, &stranded, now - 2 * ttl_ns, 0)
            .unwrap();

        // A fresh pending head for another origin is left where it is.
        let fresh_origin = synch_core::OriginId::named("laptop", "x.example").unwrap();
        bind(&node, "laptop", &key.public(), BindingSource::Static, None);
        let fresh = SignedHead::sign(&key, fresh_origin.clone(), 1, Hash::new(b"recent"), 0);
        node.store()
            .put_head(Slot::Pending, &fresh, now, 0)
            .unwrap();

        // And an old pending head whose trie *is* here: the sweep steps over
        // it, and the promotion pass performs that promotion.
        let held_origin = synch_core::OriginId::named("vps", "x.example").unwrap();
        bind(&node, "vps", &key.public(), BindingSource::Static, None);
        let trie = synch_mpt::Trie::new(node.store().as_ref());
        let root = trie
            .insert(
                Hash::EMPTY,
                &file_key("s", "a.txt").unwrap(),
                &postcard::to_stdvec(&FileEntry::file(1, 0, Hash::new(b"c"), 1)).unwrap(),
            )
            .unwrap();
        let held = SignedHead::sign(&key, held_origin.clone(), 1, root, 0);
        node.store()
            .put_head(Slot::Pending, &held, now - 2 * ttl_ns, 0)
            .unwrap();

        node.maintenance_pass().unwrap();
        assert_eq!(node.store().pending_head(&origin).unwrap(), None);
        assert_eq!(
            node.store().pending_head(&fresh_origin).unwrap(),
            Some(fresh)
        );
        assert_eq!(
            node.store().pending_head(&held_origin).unwrap(),
            None,
            "a pending head whose trie is here is promoted, not left sitting"
        );
        assert_eq!(
            node.store().complete_head(&held_origin).unwrap(),
            Some(held),
            "and what it is promoted into is the complete slot"
        );
        node.shutdown().await.unwrap();
    }

    /// §12: one origin's undecodable `f:` record fails its own origin and
    /// nothing else. The head also stops holding `head_floor`: its trie is
    /// complete, so the TTL rule would otherwise treat it as promotable.
    #[tokio::test]
    async fn a_head_that_cannot_be_materialized_does_not_stop_the_pass() {
        use synch_core::{file_key, Hash, SignedHead};
        use synch_store::Slot;

        let (_d, node) = node().await;
        let key = iroh_base::SecretKey::generate();
        let origin = synch_core::OriginId::named("rogue", "x.example").unwrap();
        bind(&node, "rogue", &key.public(), BindingSource::Static, None);

        // A canonical one-leaf trie whose `f:` record will not decode.
        let trie = synch_mpt::Trie::new(node.store().as_ref());
        let root = trie
            .insert(Hash::EMPTY, &file_key("s", "a.txt").unwrap(), &[0xffu8; 8])
            .unwrap();
        assert!(trie.is_complete(root).unwrap(), "the trie is wholly here");
        let head = SignedHead::sign(&key, origin.clone(), 1, root, 0);
        node.store()
            .put_head(Slot::Pending, &head, now_ns(), 0)
            .unwrap();

        // Junk in the trie tables that only a sweep removes.
        let junk = Hash::new(b"junk");
        synch_mpt::NodeStore::put_value(node.store().as_ref(), &junk, b"junk").unwrap();

        node.maintenance_pass()
            .expect("one origin's undecodable record must not fail the pass");

        // The sweep ran.
        assert!(
            synch_mpt::NodeStore::get_value(node.store().as_ref(), &junk)
                .unwrap()
                .is_none(),
            "gc did not run"
        );
        // And the head stopped holding the origin hostage.
        assert_eq!(
            node.store().pending_head(&origin).unwrap(),
            None,
            "a head whose promotion cannot succeed must not hold head_floor"
        );
        assert_eq!(node.store().complete_head(&origin).unwrap(), None);

        // With the floor dropped, an older head a peer can actually serve is
        // adoptable again.
        let servable = SignedHead::sign(&key, origin.clone(), 1, Hash::EMPTY, 0);
        assert!(crate::Syncer::new(node.store().clone())
            .offer_head(&servable, now_ns())
            .unwrap()
            .accepted());

        node.shutdown().await.unwrap();
    }
}
