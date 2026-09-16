//! Content fetching: provider resolution, ranking, and verified range reads
//! (§6.3, §6.4).

use synch_core::{group_count, groups_for_byte_range, now_ns, ChunkRanges, Hash, OriginId};
use synch_store::{Donor, EntryRow, Proven, ProvenSubtree, VersionPolicy, VersionSet};

use crate::{
    error::{EngineError, Result},
    join::futures_join,
    node::Node,
    scanner::CloneKind,
};

/// A ranked provider candidate.
#[derive(Debug, Clone)]
pub struct Provider {
    /// The advertising origin.
    pub origin: OriginId,
    /// The device keys currently bound to it, in dial order.
    pub keys: Vec<synch_core::NodeId>,
    /// The groups it claims to hold, derived from its advertised spans.
    pub claims: ChunkRanges,
    /// Its latency EWMA in microseconds; `0` means "never measured".
    pub latency_us: i64,
    /// A per-selection random value, breaking ties between equally ranked
    /// providers so the cluster does not converge on one of them (§6.4).
    pub tiebreak: u64,
}

/// The rank an unmeasured provider sorts at: the median of the measured ones,
/// so "unknown" means unknown rather than "worse than everything measured".
fn median_latency(providers: &[Provider]) -> i64 {
    let mut measured: Vec<i64> = providers
        .iter()
        .map(|p| p.latency_us)
        .filter(|l| *l > 0)
        .collect();
    if measured.is_empty() {
        return 0;
    }
    measured.sort_unstable();
    let mid = measured.len() / 2;
    if measured.len() % 2 == 1 {
        return measured[mid];
    }
    // The midpoint of the two middle values, not one of them: landing *on* a
    // measured peer's rank makes the two tie, and the tiebreak is random, so
    // the unmeasured peer's position would flap between runs.
    measured[mid - 1] + (measured[mid] - measured[mid - 1]) / 2
}

/// What a failed dial contributes to a peer's latency EWMA.
///
/// A fixed penalty, not the elapsed time: a dial that fails *fast* — connection
/// refused, no route — would otherwise feed a small number into the average and
/// promote the peer for being quick about being useless. Worse than any real
/// latency, and smoothed by the EWMA, so one failure demotes sharply and a few
/// successes earn the rank back.
const FAILURE_PENALTY_US: i64 = 30_000_000;

/// A cheap non-cryptographic random state, seeded from the clock.
fn jitter_state() -> u64 {
    (synch_core::now_ns() as u64) ^ 0x9e37_79b9_7f4a_7c15
}

/// xorshift64*, for tiebreaks. Nothing here needs a real generator.
fn next_random(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// A byte window of an object that is present and verified locally.
///
/// What [`Node::prepare_range`] hands back so a caller can stream the
/// window out of the CAS in pieces of its own choosing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreparedRange {
    /// The object's blake3 root.
    pub root: Hash,
    /// The object's full size in bytes.
    pub size: u64,
    /// The first byte of the window.
    pub start: u64,
    /// One past the last byte of the window.
    pub end: u64,
}

impl PreparedRange {
    /// How many bytes the window covers.
    pub fn len(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }

    /// True if the window is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// What one fetch achieved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FetchReport {
    /// The groups newly verified and committed.
    pub fetched: ChunkRanges,
    /// The groups that never crossed the network: bytes a local donor already
    /// held, proven against the new root and committed
    /// (`docs/DELTA-SYNC.md` §3.4).
    ///
    /// What lets a caller say "reused 98.9 GB, fetched 1.1 GB" rather than
    /// reporting a suspiciously fast transfer.
    pub promoted: ChunkRanges,
    /// Which donor supplied which groups.
    ///
    /// The total says a fetch was cheap; the breakdown says which of this
    /// node's objects made it cheap, which is what an operator looking at a
    /// checkout that suddenly stopped reusing anything needs to see.
    pub reused: Vec<(Donor, ChunkRanges)>,
    /// How many providers were contacted, for proofs and for slices alike.
    pub providers_tried: usize,
    /// True if the whole wanted range is now present locally.
    pub complete: bool,
}

impl Node {
    /// Resolves and ranks the providers for a byte range of an object (§6.4).
    ///
    /// Ranking is by latency EWMA, then by advertised coverage, then randomly
    /// (§6.4). Span summaries are hints: a stale one costs one wasted round
    /// trip, never correctness.
    pub(crate) fn providers_for(&self, root: &Hash, start: u64, end: u64) -> Result<Vec<Provider>> {
        let now = now_ns();
        let peers = self.store().peers_seen()?;
        let mut out = Vec::new();
        for (origin, ad) in self.store().providers_for_range(root, start, end)? {
            if &origin == self.origin() {
                continue;
            }
            let keys = self.store().keys_for_origin(&origin, now)?;
            if keys.is_empty() {
                // No live binding: we could not dial them even if we wanted to.
                continue;
            }
            let claims = ChunkRanges::from_ranges(
                ad.state
                    .spans
                    .iter()
                    .map(|&(s, e)| groups_for_byte_range(s, e)),
            );
            let latency_us = keys
                .iter()
                .filter_map(|k| {
                    peers
                        .iter()
                        .find(|p| &p.node_id == k)
                        .map(|p| p.latency_ewma_us)
                })
                .filter(|l| *l > 0)
                .min()
                .unwrap_or(0);
            out.push(Provider {
                origin,
                keys,
                claims,
                latency_us,
                tiebreak: 0,
            });
        }
        // Rank by latency, then coverage, then *randomly* — which is what §6.4
        // specifies and what a deterministic tiebreak quietly undid. Ordering
        // the tail by canonical origin makes every node in the cluster choose
        // the same provider first for any object whose holders are unmeasured,
        // which is precisely the load concentration the random tiebreak exists
        // to prevent.
        //
        // An unmeasured peer sorts into the middle rather than behind every
        // measured one. `i64::MAX / 2` put a fast new local checkout behind a peer
        // measured at 400 ms, and nothing would measure it until something else
        // happened to pick it — a peer with no measurement is unknown, not slow.
        let mut rng = jitter_state();
        for provider in out.iter_mut() {
            provider.tiebreak = next_random(&mut rng);
        }
        let unmeasured = median_latency(&out);
        out.sort_by(|a, b| {
            let rank = |p: &Provider| {
                if p.latency_us == 0 {
                    unmeasured
                } else {
                    p.latency_us
                }
            };
            rank(a)
                .cmp(&rank(b))
                .then(b.claims.count().cmp(&a.claims.count()))
                .then(a.tiebreak.cmp(&b.tiebreak))
        });
        Ok(out)
    }

    /// Fetches the chunk groups covering `[start, end)` of an object.
    ///
    /// Wanted ranges are split across up to `fetch_fanout` providers; each
    /// slice is verified against the object root before any byte is committed,
    /// and verified groups survive a restart because they land in the bitmap
    /// immediately.
    pub(crate) async fn fetch_range(
        &self,
        root: &Hash,
        size: u64,
        start: u64,
        end: u64,
    ) -> Result<FetchReport> {
        let wanted = ChunkRanges::from_ranges([groups_for_byte_range(start, end)])
            .intersect(&ChunkRanges::single(0, group_count(size)));
        self.fetch_groups(root, size, &wanted).await
    }

    /// Fetches an object in full.
    pub async fn fetch_all(&self, root: &Hash, size: u64) -> Result<FetchReport> {
        let wanted = ChunkRanges::single(0, group_count(size));
        self.fetch_groups(root, size, &wanted).await
    }

    /// Fetches an object in full, offering donors it might be assembled from
    /// (`docs/DELTA-SYNC.md` §3.2).
    pub(crate) async fn fetch_all_from(
        &self,
        root: &Hash,
        size: u64,
        donors: &[Donor],
    ) -> Result<FetchReport> {
        let wanted = ChunkRanges::single(0, group_count(size));
        self.fetch_groups_from(root, size, &wanted, donors).await
    }

    /// Pins an object, fetching it first when it is not held whole (§9.2).
    ///
    /// A pin is a promise that these bytes stay available here, and a promise
    /// about bytes this node does not hold starts by getting them, or pinning
    /// metadata-only content marks nothing and says so to no one. The
    /// size travels with the entry the pin was resolved from; a bare root
    /// nobody holds even partially has no size to fetch by, and is refused
    /// rather than half-promised.
    pub async fn pin_object(&self, root: &Hash, size_hint: Option<u64>) -> Result<()> {
        let blob = {
            let store = self.store().clone();
            let root = *root;
            crate::blocking::offload(move || Ok(store.blob(&root)?)).await?
        };
        if !blob.as_ref().is_some_and(|b| b.complete || b.durable) {
            let Some(size) = blob.as_ref().map(|b| b.size).or(size_hint) else {
                return Err(EngineError::NotFound(format!(
                    "no local object with root {root}; pin it as <space>/<path> so the \
                     fetch knows the object's size"
                )));
            };
            let fetched = self.fetch_all(root, size).await?;
            if !fetched.complete {
                return Err(EngineError::NotFound(format!(
                    "no provider could supply the complete object {root}; it was not pinned"
                )));
            }
        }
        self.finalize_cloud_object(root, true).await?;
        let pinned = {
            let store = self.store().clone();
            let root = *root;
            crate::blocking::offload(move || {
                Ok(store.pin(
                    &root,
                    &synch_store::PinHolder::Operator,
                    synch_core::now_ns(),
                )?)
            })
            .await?
        };
        if !pinned {
            return Err(EngineError::NotFound(format!(
                "object {root} left the store before it could be pinned"
            )));
        }
        if self.config().cloud.is_some() {
            let node = self.clone();
            let root = *root;
            let change = crate::blocking::offload(move || node.ad_change(&root)).await?;
            if let Some(change) = change {
                self.stage([change]);
                self.flush_staged().await?;
            }
        }
        Ok(())
    }

    /// Removes a pin and durably retires its b-only recovery record.
    pub async fn unpin_object(&self, root: &Hash) -> Result<bool> {
        let store = self.store().clone();
        let root_value = *root;
        let removed = crate::blocking::offload(move || {
            Ok(store.unpin(&root_value, &synch_store::PinHolder::Operator)?)
        })
        .await?;
        if removed && self.config().cloud.is_some() {
            let store = self.store().clone();
            let root_value = *root;
            let referenced =
                crate::blocking::offload(move || Ok(store.content_is_referenced(&root_value)?))
                    .await?;
            if !referenced {
                self.stage([(synch_core::blob_key(root), None)]);
                self.flush_staged().await?;
            }
        }
        Ok(removed)
    }

    /// Fetches specific chunk groups (§6.4).
    ///
    /// The wanted ranges are split across up to `fetch_fanout` providers and
    /// those requests run concurrently, which is what the fanout is for: three
    /// peers each serving a third of a large object beats one peer serving all
    /// of it. Failures do not end the fetch — the surviving ranges go back into
    /// the pool and the next batch of candidates is tried, so a fourth provider
    /// that holds what the first three did not is still reached.
    pub(crate) async fn fetch_groups(
        &self,
        root: &Hash,
        size: u64,
        wanted: &ChunkRanges,
    ) -> Result<FetchReport> {
        self.fetch_groups_from(root, size, wanted, &[]).await
    }

    /// Fetches specific chunk groups, offering local donors first
    /// (`docs/DELTA-SYNC.md` §3.3).
    ///
    /// Everything `Node::fetch_groups` does, with a descent in front of it:
    /// the object's tree is asked for at span granularity, compared against
    /// what the donors hold at the same offsets, and every span that turns out
    /// to be byte-identical is promoted out of local storage instead of being
    /// transferred. What is left descends to the leaf level and is compared
    /// again, and only what survives *that* reaches the network.
    ///
    /// The floor is worth stating plainly: with no donors, no provider that
    /// answers, or no span in common, this is `Node::fetch_groups` plus at
    /// most one small exchange — which is why the delta attempt needs no
    /// similarity heuristic in front of it (§3.3).
    pub async fn fetch_groups_from(
        &self,
        root: &Hash,
        size: u64,
        wanted: &ChunkRanges,
        donors: &[Donor],
    ) -> Result<FetchReport> {
        if size == 0 {
            return self.take_empty_object(root).await;
        }
        let mut report = FetchReport::default();
        if let Err(error) = self
            .cas_backend()
            .ensure_ranges(*root, size, wanted.clone())
            .await
        {
            tracing::warn!(root = %root, %error, "durable backend read fell back to peers");
        }
        let mut remaining = wanted.difference(&self.local_groups_off_runtime(root).await?);
        if remaining.is_empty() {
            report.complete = true;
            if self.config().cloud.as_ref().is_some_and(|cloud| {
                cloud.upload_policy == synch_store::cloud::CloudUploadPolicy::All
            }) {
                self.finalize_cloud_object(root, false).await?;
            }
            return Ok(report);
        }

        // Objects below `delta_min_size` skip the descent entirely: the round
        // trips cost more than the bytes they could save (§4).
        if !donors.is_empty() && size >= self.config().delta_min_size {
            self.delta_descent(root, size, &remaining, donors, &mut report)
                .await?;
            remaining = remaining.difference(&report.promoted);
            if !report.promoted.is_empty() {
                tracing::debug!(
                    root = %root,
                    promoted = report.promoted.count(),
                    remaining = remaining.count(),
                    "delta descent reused local bytes"
                );
            }
        }

        // A descent that satisfied everything leaves nobody to dial.
        let mut providers = if remaining.is_empty() {
            Vec::new()
        } else {
            self.providers_off_runtime(root, size).await?
        };
        let mut discovery_used = false;
        if providers.is_empty() && !remaining.is_empty() {
            // No local ad covers this root — a cold cache, or an origin just
            // admitted whose ads have not replicated yet. Peers may know who
            // holds it, and a hint costs at most a wasted dial because content
            // is hash-verified regardless (§5.1).
            providers = self.ask_peers_for_providers(root, size).await?;
            discovery_used = true;
        }

        let fanout = self.config().fetch_fanout.max(1);
        // A pool, not a one-shot iterator. Advancing a single iterator across
        // batches would select each provider at most once for the whole fetch,
        // so a provider that served its share and stayed healthy would never be
        // asked again: with one good provider and two ghosts in the first
        // batch, the good one serves a third, the ghosts fail, the iterator is
        // exhausted, and the fetch reports failure with the holder still
        // online. §6.4 promises the opposite — a provider that cannot help is
        // dropped and its groups are re-split across *the remainder*.
        let mut pool: Vec<Provider> = providers;
        let mut retired = std::collections::HashSet::new();
        // Providers are retired only by failing, so a round that makes no
        // progress at all must end the loop or it would spin forever.
        loop {
            if remaining.is_empty() {
                break;
            }
            if pool.is_empty() {
                // Published ads can outlive their publishers. Once every known
                // provider has failed, spend one bounded discovery walk asking
                // live peers for a replacement. Cloud replicas use aggregate
                // coverage rather than one `b:` record per object, so this is
                // how a last cloud copy is found after the original nodes die.
                if discovery_used {
                    break;
                }
                discovery_used = true;
                pool = self.ask_peers_for_providers(root, size).await?;
                pool.retain(|provider| !retired.contains(&provider.origin));
                if pool.is_empty() {
                    break;
                }
            }
            let mut chosen: Vec<Provider> = Vec::new();
            for provider in pool.iter() {
                if remaining.intersect(&provider.claims).is_empty() {
                    continue;
                }
                chosen.push(provider.clone());
                if chosen.len() >= fanout {
                    break;
                }
            }
            if chosen.is_empty() {
                break;
            }

            // Each provider gets a contiguous share *of what it claims*, taken
            // in rank order so the fastest picks first, and nothing is handed to
            // two of them.
            //
            // Assign from each provider's claimed ranges, so complementary
            // providers can cover the request without either claiming it all.
            let mut unassigned = remaining.clone();
            let mut batch: Vec<(Provider, ChunkRanges)> = Vec::new();
            let mut left = chosen.len() as u64;
            for provider in chosen {
                let mine = unassigned.intersect(&provider.claims);
                left = left.saturating_sub(1);
                if mine.is_empty() {
                    continue;
                }
                // An even split of what is still unclaimed among the providers
                // still to be offered a share, so the last one takes whatever is
                // left rather than a rounded-down fraction of it.
                let share = match left {
                    0 => mine,
                    rest => mine.take(mine.count().div_ceil(rest + 1)),
                };
                unassigned = unassigned.difference(&share);
                batch.push((provider, share));
            }
            if batch.is_empty() {
                break;
            }
            report.providers_tried += batch.len();

            let results = futures_join(batch.iter().map(|(provider, ask)| async move {
                // The accumulator travels with the request, so a provider that
                // fails on its fifth window still hands back the four before it:
                // those groups are in the bitmap, and treating them as lost had
                // the next provider asked for bytes this node already held and
                // `write_slice` re-decode them.
                let mut got = ChunkRanges::empty();
                let outcome = self.fetch_from(provider, root, size, ask, &mut got).await;
                (provider.origin.clone(), got, outcome)
            }))
            .await;
            for (origin, got, result) in results {
                remaining = remaining.difference(&got);
                report.fetched = report.fetched.union(&got);
                match result {
                    Ok(()) => {
                        if got.is_empty() {
                            // Served nothing despite claiming the range: its
                            // ads overstate what it has, so stop asking.
                            pool.retain(|p| p.origin != origin);
                            retired.insert(origin);
                        }
                    }
                    Err(e) => {
                        // A peer that cannot help is retired from the pool and
                        // whatever it did not serve stays in `remaining`, so the
                        // next batch offers it to whoever is left.
                        tracing::debug!(origin = %origin, error = %e, "provider failed");
                        pool.retain(|p| p.origin != origin);
                        retired.insert(origin);
                    }
                }
            }
            // No-progress providers were retired above. A now-empty pool gets
            // its single rediscovery chance at the top of the loop.
        }

        if !report.fetched.is_empty() || !report.promoted.is_empty() {
            // Promoted groups count as progress exactly as fetched ones do:
            // they are verified, they are held, and the point of advertising
            // them is that other replicas of the same space can then delta from
            // *this* node rather than from the origin (§3.4, §6.3).
            //
            // Publishing an updated ad is a trie write and a signed head, so
            // the milestone check and the publish it may trigger both go off
            // the runtime (§6.3).
            let node = self.clone();
            let root = *root;
            crate::blocking::offload(move || node.on_content_progress(&root).map(|_| ())).await?;
        }
        report.complete = wanted
            .difference(&self.local_groups_off_runtime(root).await?)
            .is_empty();
        if self
            .config()
            .cloud
            .as_ref()
            .is_some_and(|cloud| cloud.upload_policy == synch_store::cloud::CloudUploadPolicy::All)
        {
            self.finalize_cloud_object(root, false).await?;
        }
        Ok(report)
    }

    /// Promotes a complete cache entry into the remote durable tier.
    pub(crate) async fn finalize_cloud_object(&self, root: &Hash, pin: bool) -> Result<()> {
        let Some(config) = self.config().cloud.as_ref() else {
            return Ok(());
        };
        let (size, durable) = {
            let (store, root) = (self.store().clone(), *root);
            crate::blocking::offload(move || {
                store
                    .blob(&root)?
                    .map(|row| (row.size, row.durable))
                    .ok_or_else(|| {
                        EngineError::NotFound(format!("no local object with root {root}"))
                    })
            })
            .await?
        };
        if pin && !durable && config.upload_policy == synch_store::cloud::CloudUploadPolicy::Own {
            return Err(EngineError::invalid(
                "cas.cloud.upload=own refuses a durability pin for peer-fetched content; use own+pinned or all",
            ));
        }
        Ok(self.cas_backend().finalize(*root, size).await?)
    }

    /// The lineage of a version: every other root that might hold bytes of it
    /// (`docs/DELTA-SYNC.md` §3.2).
    ///
    /// In the order the descent should try them: the entry's `prev` root first,
    /// because 1-step lineage (§4.2, §8) names the version this one replaced
    /// and that is the common case by a wide margin; then every other version
    /// of the same path, which is what a divergent origin or a losing version
    /// under `newest` leaves lying around.
    ///
    /// Candidates, not donors: whether this node holds any of them is
    /// [`Node::donors_for`]'s question. A checkout asks this one because it needs
    /// the names of the versions it *fails* to hold — a root missing from the
    /// CAS that the file on its disk turns out to be is a donor one `ingest`
    /// away (§3.2).
    ///
    /// The object being fetched is never its own donor: the groups it already
    /// holds are subtracted from the fetch before the descent starts.
    pub(crate) fn donor_roots(&self, selected: &EntryRow, versions: &VersionSet) -> Vec<Hash> {
        let mut roots: Vec<Hash> = Vec::new();
        let mut push = |root: Option<Hash>| {
            if let Some(root) = root {
                if selected.content != Some(root) && !roots.contains(&root) {
                    roots.push(root);
                }
            }
        };
        push(selected.prev);
        for entry in &versions.entries {
            push(entry.content);
            push(entry.prev);
        }
        roots
    }

    /// The lineage this node can actually supply bytes from.
    ///
    /// [`Node::donor_roots`] narrowed to the roots the CAS holds something of:
    /// a donor with no bytes is a wasted pass over the proof list.
    pub(crate) fn donors_for(
        &self,
        selected: &EntryRow,
        versions: &VersionSet,
    ) -> Result<Vec<Donor>> {
        let mut donors = Vec::new();
        for root in self.donor_roots(selected, versions) {
            if self.holds_any_of(&root)? {
                donors.push(Donor(root));
            }
        }
        Ok(donors)
    }

    /// True if the CAS holds any verified group of an object.
    pub(crate) fn holds_any_of(&self, root: &Hash) -> Result<bool> {
        Ok(self
            .store()
            .blob(root)?
            .is_some_and(|blob| !blob.verified_groups().is_empty()))
    }

    /// Discovers how much of an object this node can supply itself, in two
    /// rounds of proof (`docs/DELTA-SYNC.md` §3.3).
    ///
    /// Round one asks for the tree at span granularity — a 64-byte node pair per
    /// interior node above the spans, so about 381 KB for a 100 GB object — and
    /// promotes every span a donor turns out to agree with, whole. Round two
    /// asks for leaf chaining values inside the spans a donor could speak to and
    /// *disagreed* with, and promotes group by group. What survives both rounds
    /// is the delta, and it is all the caller has to fetch.
    ///
    /// Nothing here can fail the fetch. A provider that will not answer, a
    /// donor the collector took, a file rewritten end to end — each just leaves
    /// more work for the ordinary fetch path, which is exactly what would have
    /// happened without any of this.
    async fn delta_descent(
        &self,
        root: &Hash,
        size: u64,
        wanted: &ChunkRanges,
        donors: &[Donor],
        report: &mut FetchReport,
    ) -> Result<()> {
        // The whole tree is only as tall as the object: descending "to the span
        // level" of an object that *is* one span would ask for nothing at all,
        // because the root's own hash is not a chaining value anything can be
        // compared against (§2). One level below the top is the deepest cut that
        // still says something, so the level is clamped to `top - 1` — an object
        // of one span is compared at half-span granularity, one of two spans at
        // span granularity, and only an object of two groups or fewer has round
        // one land on the leaf level itself.
        let top = group_count(size)
            .next_power_of_two()
            .trailing_zeros()
            .min(63) as u8;
        let span_level = synch_core::AD_SPAN_LEVEL.min(top.saturating_sub(1));

        // Round one, at span granularity.
        let round = self
            .fetch_proofs(root, size, wanted, span_level, report)
            .await?;
        if round.is_empty() {
            return Ok(());
        }
        let leftover = self.promote_round(root, donors, round, report).await?;
        if span_level == 0 {
            return Ok(());
        }
        // Nothing in common at all, across every donor: stop here rather than
        // buy the leaf round. A same-size donor with unrelated content — a
        // re-keyed container, a rebuilt archive — passes every cheap test the
        // descent has and then matches nothing, and the leaf round over a
        // 100 GB object is ~391 MB of tree and ~750 round trips. What that
        // spend could still find is bytes that agree *inside* spans whose
        // span-level chaining values all differed, which for fixed-offset
        // groups means a run that happens to align on a group boundary within
        // an otherwise-changed span: real, but rare enough that paying 391 MB
        // for the chance is the wrong trade every time it is not found
        // (`docs/DELTA-SYNC.md` §3.3). One span in common is enough to
        // establish the donor is a relative of this object and the round is
        // worth running; zero says it is not.
        if report.promoted.is_empty() {
            tracing::debug!(
                root = %root,
                "no span in common with any donor: skipping the leaf round"
            );
            return Ok(());
        }

        // Round two, at the leaf level, inside the spans round one could not
        // settle — and inside nothing else. A span no donor can speak to has
        // nothing for a leaf comparison to compare against, and asking for its
        // tree group by group would buy a proof the size of the object to learn
        // what round one already said (§3.3).
        let unsettled = {
            let node = self.clone();
            let donors = donors.to_vec();
            let wanted = wanted.clone();
            crate::blocking::offload(move || node.unsettled_spans(&donors, &leftover, &wanted))
                .await?
        };
        let mut left = unsettled;
        while !left.is_empty() {
            // One exchange's worth at a time, because the proven subtrees of a
            // leaf-level round are one per 16 KiB group and a re-keyed 100 GB
            // container makes every span unsettled at once: held whole, that
            // list is hundreds of megabytes of a node's memory to say something
            // about bytes it is about to fetch anyway. The size of an exchange
            // is the provider's window sizer's answer, asked here rather than
            // guessed, so the two cannot disagree about where a round ends.
            let batch = synch_net::proof_window(&left, 0);
            left = left.difference(&batch);
            let round = self.fetch_proofs(root, size, &batch, 0, report).await?;
            if round.is_empty() {
                continue;
            }
            self.promote_round(root, donors, round, report).await?;
        }
        Ok(())
    }

    /// The spans a leaf-level round should look inside.
    ///
    /// A span descends only if some donor had a chaining value at that position:
    /// round one already promoted the ones that agreed, so what is left with a
    /// donor behind it is a span whose bytes moved, and the groups inside it are
    /// worth comparing one at a time. A span no donor can speak to at all — past
    /// the end of every one of them, or held by none — has nothing to be
    /// compared against, and goes to the ordinary fetch.
    ///
    /// The object's right edge is the exception, and it is bounded to one span:
    /// a subtree cut short by the end of the object is not comparable *as a
    /// subtree* in the first place (§3.3), so the absence of a donor value there
    /// says nothing, and the groups under it descend.
    ///
    /// Blocking: one outboard walk per span per donor.
    fn unsettled_spans(
        &self,
        donors: &[Donor],
        leftover: &Proven,
        wanted: &ChunkRanges,
    ) -> Result<ChunkRanges> {
        let mut out = ChunkRanges::empty();
        let mut whole: Vec<ProvenSubtree> = Vec::new();
        for subtree in &leftover.subtrees {
            if subtree.groups <= 1 {
                // Already at the leaf level: round two would ask the same
                // question again.
                continue;
            }
            if subtree.whole {
                whole.push(*subtree);
            } else {
                out = out.union(&ChunkRanges::from_ranges([subtree.range()]));
            }
        }
        if whole.is_empty() {
            return Ok(out);
        }
        let spans: Vec<(u64, u64)> = whole.iter().map(|s| (s.start, s.groups)).collect();
        let mut comparable = vec![false; whole.len()];
        for donor in donors {
            if donor.root() == leftover.root {
                continue;
            }
            // Same rule as promotion: a donor that errors has nothing to say,
            // and saying so must not fail the fetch.
            let cvs = match self.store().subtree_cvs(&donor.root(), &spans) {
                Ok(cvs) => cvs,
                Err(e) => {
                    tracing::debug!(
                        donor = %donor.root(),
                        error = %e,
                        "a donor could not be read for the leaf round"
                    );
                    continue;
                }
            };
            for (index, cv) in cvs.iter().enumerate() {
                comparable[index] |= cv.is_some();
            }
        }
        for (subtree, comparable) in whole.iter().zip(comparable) {
            if comparable {
                out = out.union(&ChunkRanges::from_ranges([subtree.range()]));
            }
        }
        // Spans are whole 16 MiB subtrees, so a span that merely *overlaps*
        // what this fetch wants would otherwise be descended in full — buying a
        // leaf proof, and a round trip, for groups already in the bitmap, which
        // `promote` then discards. Clipping costs nothing and is what makes a
        // resumed fetch's second round proportional to what is still missing.
        Ok(out.intersect(wanted))
    }

    /// Collects proofs for `ranges` from whoever will serve them.
    ///
    /// Sequential across providers rather than fanned out like a fetch: a proof
    /// is a few hundred kilobytes at the span level and the round trips, not
    /// the bytes, are what it costs. A provider that answers with nothing —
    /// because it holds none of the object, or has gone away — simply hands the
    /// remainder to the next candidate.
    async fn fetch_proofs(
        &self,
        root: &Hash,
        size: u64,
        ranges: &ChunkRanges,
        level: u8,
        report: &mut FetchReport,
    ) -> Result<Proven> {
        let mut providers = self.providers_off_runtime(root, size).await?;
        if providers.is_empty() {
            providers = self.ask_peers_for_providers(root, size).await?;
        }
        let mut remaining = ranges.clone();
        let mut proven = Proven::none(*root, size);
        for provider in providers {
            if remaining.is_empty() {
                break;
            }
            let ask = remaining.intersect(&provider.claims);
            if ask.is_empty() {
                continue;
            }
            report.providers_tried += 1;
            // Accumulated here, so a descent that fails on a later window keeps
            // every subtree the earlier ones proved: a `ProvenSubtree` is the
            // only thing `promote` can act on, and the committed outboard nodes
            // cannot be read back into one.
            let mut outcome = synch_net::ProofOutcome {
                proven: Proven::none(*root, size),
                served: ChunkRanges::empty(),
            };
            let asked = self
                .proofs_from(&provider, root, size, &ask, level, &mut outcome)
                .await;
            remaining = remaining.difference(&outcome.served);
            proven.absorb(outcome.proven)?;
            if let Err(e) = asked {
                tracing::debug!(origin = %provider.origin, error = %e, "proof request failed");
            }
        }
        Ok(proven)
    }

    async fn proofs_from(
        &self,
        provider: &Provider,
        root: &Hash,
        size: u64,
        ask: &ChunkRanges,
        level: u8,
        out: &mut synch_net::ProofOutcome,
    ) -> Result<()> {
        let client = self.dial_provider(provider).await?;
        // The accumulator is ours, so a window that fails part-way through
        // leaves what was already proven with the caller rather than
        // discarding it.
        client
            .fetch_proof_into(self.cas_backend(), *root, size, ask, level, out)
            .await?;
        Ok(())
    }

    /// Dials a provider's keys in order and returns the first connection made.
    ///
    /// The one dial loop both transfer paths share, so its bookkeeping cannot
    /// diverge between them — it already had, once: the proof path recorded
    /// only successful dials, so a dead peer kept its low EWMA there forever.
    ///
    /// Timed at the dial, not around the transfer. A fetch walks the provider's
    /// whole share one window at a time, so timing it would measure *how much
    /// was asked of the peer*, not how quick the peer is — a provider that
    /// successfully served a gigabyte would record tens of seconds, worse than
    /// `FAILURE_PENALTY_US`, and so rank below a peer whose dial was refused,
    /// inverting the ranking under exactly the load it exists to spread.
    ///
    /// A failed dial has to move the EWMA, or ranking is a one-way ratchet:
    /// with latency recorded only on success, a peer that was once fast and is
    /// now a black hole keeps its low EWMA and is therefore selected first on
    /// every subsequent fetch, forever, with nothing able to demote it.
    async fn dial_provider(&self, provider: &Provider) -> Result<synch_net::BlobClient> {
        let mut last_error = None;
        for key in &provider.keys {
            let addr = match self.peer_addr_off_runtime(key).await? {
                Some(addr) => addr,
                None => iroh::EndpointAddr::new(*key),
            };
            let started = std::time::Instant::now();
            match self.net().connect_blob(addr).await {
                Ok(client) => {
                    let elapsed = started.elapsed().as_micros().min(i64::MAX as u128) as i64;
                    self.record_dial_off_runtime(key, elapsed).await;
                    return Ok(client);
                }
                Err(e) => {
                    self.record_dial_failure_off_runtime(key).await;
                    last_error = Some(e);
                }
            }
        }
        Err(match last_error {
            Some(e) => EngineError::Net(e),
            None => EngineError::not_found(format!("no dialable key for {}", provider.origin)),
        })
    }

    /// Offers every donor the subtrees a proof round established, in order, and
    /// reports what is left over.
    ///
    /// All of it is disk work — outboard lookups, run clones, payload and
    /// bitmap commits — so all of it goes to the blocking pool (§10). The
    /// subtrees a donor supplied are struck off before the next donor is asked,
    /// and what comes back is what nobody could supply.
    async fn promote_round(
        &self,
        root: &Hash,
        donors: &[Donor],
        proven: Proven,
        report: &mut FetchReport,
    ) -> Result<Proven> {
        let mut leftover = proven;
        let mut supplied = Vec::new();
        for donor in donors {
            if leftover.is_empty() {
                break;
            }
            if donor.root() == *root {
                continue;
            }
            let got = match self
                .cas_backend()
                .promote(*donor, leftover.clone(), now_ns())
                .await
            {
                Ok(got) => got,
                Err(e) => {
                    tracing::debug!(
                        root = %root,
                        donor = %donor.root(),
                        error = %e,
                        "a donor could not be promoted from: leaving its groups to the fetch"
                    );
                    continue;
                }
            };
            if got.is_empty() {
                continue;
            }
            leftover
                .subtrees
                .retain(|subtree| !got.overlaps(subtree.start, subtree.end()));
            supplied.push((*donor, got));
        }
        for (donor, got) in supplied {
            report.promoted = report.promoted.union(&got);
            match report.reused.iter_mut().find(|(had, _)| had == &donor) {
                Some((_, ranges)) => *ranges = ranges.union(&got),
                None => report.reused.push((donor, got)),
            }
        }
        Ok(leftover)
    }

    /// Produces an object of no bytes locally, rather than fetching it.
    ///
    /// A zero-length object has nothing to transfer: no bytes, and no group a
    /// provider could serve. Asking for one anyway is what breaks empty files —
    /// [`group_count`] counts an empty object as one group so that "complete"
    /// is representable, but bao encodes nothing over an empty tree, so every
    /// window comes back served-nothing, the fetch runs out of providers with
    /// that group still missing, and a checkout reports the path as `no provider
    /// could serve the content` (§6.4).
    ///
    /// Nobody has to serve it. An empty object's content is settled by its
    /// size, and its root is what BLAKE3 gives for no input — so this node can
    /// produce the object itself and get, byte for byte and hash for hash, what
    /// a provider would have sent. Ingesting it here is also what leaves the
    /// CAS row every later read goes through: `synch cat`, `get`, and adoption of
    /// an empty file all resolve through the store like any other object.
    async fn take_empty_object(&self, root: &Hash) -> Result<FetchReport> {
        let mut report = FetchReport::default();
        let held = {
            let (store, root) = (self.store().clone(), *root);
            crate::blocking::offload(move || Ok(store.blob(&root)?)).await?
        };
        if held.is_some_and(|blob| blob.complete) {
            report.complete = true;
            return Ok(report);
        }
        // An entry that declares no bytes while naming some other object is
        // inconsistent: nothing is invented for it, and the caller reports it
        // unservable exactly as it would any root nobody can supply.
        report.complete = self
            .cas_backend()
            .ingest_bytes(Vec::new(), now_ns())
            .await?
            .root
            == *root;
        Ok(report)
    }

    /// Asks trusted peers who holds an object, for roots no local ad covers
    /// (§5.1 `FindProviders`).
    ///
    /// Hints are unverified: they are fed back through the ordinary ranking so
    /// a wrong one costs a dial and nothing else, and every byte is still
    /// checked against the object root.
    ///
    /// Two things bound what one unresolvable root can cost. The walk stops at
    /// the first peer that names a provider, rather than asking everybody for
    /// hints it already has; and a root that nobody could name is remembered as
    /// a miss and left alone for a while (§6.3). Without either, a root nobody
    /// holds — an origin publishing `f:` records whose content hashes name
    /// nothing — is re-planned by every checkout pass and re-dials every peer in
    /// the cluster on each one, sequentially, so the victim's checkout can be
    /// made never to finish a pass and never to do the work it exists for.
    /// The miss expires, so a root that is published later is still picked up,
    /// and a root a local ad covers never reaches this at all.
    async fn ask_peers_for_providers(&self, root: &Hash, size: u64) -> Result<Vec<Provider>> {
        if self.provider_discovery_backed_off(root, now_ns()) {
            tracing::debug!(root = %root, "provider discovery backed off; not dialling");
            let known = self.providers_off_runtime(root, size).await?;
            if !known.is_empty() {
                // An ad reached this node some other way — head replication, or
                // another fetch's hints — so the miss is stale.
                self.clear_provider_miss(root);
            }
            return Ok(known);
        }
        let mut learned = 0;
        for (peer, addr) in self.dial_targets().await? {
            let client = match self.net().connect_mpt(addr).await {
                Ok(client) => client,
                Err(e) => {
                    tracing::debug!(peer = %peer.fmt_short(), error = %e, "peer unreachable");
                    continue;
                }
            };
            let ads = match client.find_providers(*root).await {
                Ok(ads) => ads,
                Err(e) => {
                    tracing::debug!(peer = %peer.fmt_short(), error = %e, "provider hint failed");
                    continue;
                }
            };
            // Storing a hint is a row, and the origin in one is a peer's word:
            // nothing in the answer establishes that the origin exists, and
            // nothing sweeps `blob_providers`. A hint about an origin this node
            // has no live binding for could never be dialled anyway
            // ([`Node::providers_for`] drops it), so it is not worth a row.
            //
            // The writes go to the blocking pool with every other database
            // path: this runs inside a fetch on a runtime worker, and a bounded
            // answer is still a row apiece (§10).
            let node = self.clone();
            let root = *root;
            learned += crate::blocking::offload(move || {
                let now = now_ns();
                let mut stored = 0usize;
                for (origin, ad) in ads {
                    if &origin == node.origin()
                        || node.store().keys_for_origin(&origin, now)?.is_empty()
                    {
                        continue;
                    }
                    node.store().put_provider(&root, &origin, &ad)?;
                    stored += 1;
                }
                Ok(stored)
            })
            .await?;
            // One peer that knows a holder is the answer; the rest of the
            // cluster has nothing to add that this fetch needs.
            if learned > 0 {
                break;
            }
        }
        if learned > 0 {
            tracing::debug!(hints = learned, "learned providers from peers");
        }
        let found = self.providers_off_runtime(root, size).await?;
        if found.is_empty() {
            self.note_provider_miss(root, now_ns());
        } else {
            self.clear_provider_miss(root);
        }
        Ok(found)
    }

    /// [`Node::providers_for`] on the blocking pool.
    ///
    /// One `blob_providers` scan plus a `peers_seen` read and a binding query
    /// per candidate — store work, and reached from inside a fetch running on a
    /// runtime worker (§10).
    async fn providers_off_runtime(&self, root: &Hash, size: u64) -> Result<Vec<Provider>> {
        let node = self.clone();
        let root = *root;
        let end = size.max(1);
        crate::blocking::offload(move || node.providers_for(&root, 0, end)).await
    }

    async fn fetch_from(
        &self,
        provider: &Provider,
        root: &Hash,
        size: u64,
        ask: &ChunkRanges,
        got: &mut ChunkRanges,
    ) -> Result<()> {
        let client = self.dial_provider(provider).await?;
        // The groups are in the bitmap whether or not a later window fails, so
        // the caller keeps them and does not ask another provider for bytes it
        // already holds.
        client
            .fetch_into(self.cas_backend(), *root, size, ask, got)
            .await?;
        Ok(())
    }

    /// [`Node::peer_addr`] on the blocking pool: it reads `peers_seen`.
    pub(crate) async fn peer_addr_off_runtime(
        &self,
        key: &synch_core::NodeId,
    ) -> Result<Option<iroh::EndpointAddr>> {
        let node = self.clone();
        let key = *key;
        crate::blocking::offload(move || node.peer_addr(&key)).await
    }

    /// Records how long a dial took, on the blocking pool.
    ///
    /// A row update and a WAL frame per dial, which is not something to do on
    /// the worker the dial itself is being driven from (§10). Ranking is
    /// advisory, so a failure to record it is logged rather than propagated:
    /// losing one measurement must not fail a fetch that otherwise worked.
    async fn record_dial_off_runtime(&self, key: &synch_core::NodeId, elapsed_us: i64) {
        let store = self.store().clone();
        let key = *key;
        let recorded: Result<()> = crate::blocking::offload(move || {
            Ok(store.record_peer_sync(&key, now_ns(), elapsed_us)?)
        })
        .await;
        if let Err(e) = recorded {
            tracing::debug!(peer = %key.fmt_short(), error = %e, "could not record dial latency");
        }
    }

    /// Records a failed dial's penalty, on the blocking pool.
    async fn record_dial_failure_off_runtime(&self, key: &synch_core::NodeId) {
        let store = self.store().clone();
        let key = *key;
        let recorded: Result<()> = crate::blocking::offload(move || {
            Ok(store.record_peer_failure(&key, now_ns(), FAILURE_PENALTY_US)?)
        })
        .await;
        if let Err(e) = recorded {
            tracing::debug!(peer = %key.fmt_short(), error = %e, "could not record dial failure");
        }
    }

    /// [`Node::local_groups`] on the blocking pool.
    pub(crate) async fn local_groups_off_runtime(&self, root: &Hash) -> Result<ChunkRanges> {
        let node = self.clone();
        let root = *root;
        crate::blocking::offload(move || node.local_groups(&root)).await
    }

    /// The groups of an object we hold and have verified.
    pub fn local_groups(&self, root: &Hash) -> Result<ChunkRanges> {
        Ok(self
            .store()
            .blob(root)?
            .map(|blob| blob.verified_groups())
            .unwrap_or_else(ChunkRanges::empty))
    }

    /// Publishes an updated `b:` advertisement if a milestone was reached
    /// (§6.3).
    pub(crate) fn on_content_progress(
        &self,
        root: &Hash,
    ) -> Result<Option<synch_core::SignedHead>> {
        if !self.ad_update_due(root)? {
            return Ok(None);
        }
        let Some(change) = self.ad_change(root)? else {
            return Ok(None);
        };
        self.publish(&[change])
    }

    /// Reads a byte range of the policy-selected version of a path, fetching
    /// whatever is missing first (§7.2, §8).
    ///
    /// Buffers the whole range: callers streaming a large object — the
    /// control server behind `synch cat --range` among them — use
    /// [`Node::prepare_range`] and then chunked
    /// [`CasBackend::read_range`](synch_store::backend::CasBackend::read_range)
    /// calls instead.
    pub async fn read_range(
        &self,
        space: &str,
        path: &str,
        policy: &VersionPolicy,
        start: u64,
        len: Option<u64>,
    ) -> Result<Vec<u8>> {
        let range = self.prepare_range(space, path, policy, start, len).await?;
        Ok(self
            .cas_backend()
            .read_range(range.root, range.start, range.end - range.start)
            .await?)
    }

    /// Refills a cold local cache from the configured durable cloud backend.
    ///
    /// A durable row remains a complete holder after scratch loss. This is the
    /// bridge from that metadata promise back to verified LocalFs cache bytes.
    pub(crate) async fn ensure_blob_cached(&self, root: &Hash, size: u64) -> Result<()> {
        Ok(self.cas_backend().ensure_cached(*root, size).await?)
    }

    /// Reads the policy-selected version of a path in full.
    pub async fn read_path(
        &self,
        space: &str,
        path: &str,
        policy: &VersionPolicy,
    ) -> Result<Vec<u8>> {
        self.read_range(space, path, policy, 0, None).await
    }

    /// Selects a version under a policy, fetches whatever of the requested
    /// range is missing, and reports where the bytes now live locally.
    ///
    /// Every byte is verified against the object's bao tree before it is
    /// committed to the CAS, so a subsequent
    /// [`Store::read_range`](synch_store::Store::read_range) over the returned
    /// window reads only verified content.
    pub async fn prepare_range(
        &self,
        space: &str,
        path: &str,
        policy: &VersionPolicy,
        start: u64,
        len: Option<u64>,
    ) -> Result<PreparedRange> {
        // Resolving the entry is a `versions_for` query and a policy decision
        // over it, and the donor lineage below is another; both are store work
        // and this runs on a runtime worker for every gateway read (§10). They
        // go over together, after the window is known, so a read costs one
        // handoff rather than three.
        let entry = {
            let node = self.clone();
            let (space, path, policy) = (space.to_string(), path.to_string(), policy.clone());
            crate::blocking::offload(move || node.resolve(&space, &path, &policy)).await?
        };
        if entry.kind == synch_core::EntryKind::Tombstone {
            return Err(EngineError::not_found(format!(
                "{} was deleted at seq {}",
                crate::tree::reference_of(policy, space, path),
                entry.seq
            )));
        }
        let root = entry
            .content
            .ok_or_else(|| EngineError::invalid("entry has no content"))?;
        let end = match len {
            Some(len) => start.saturating_add(len).min(entry.size),
            None => entry.size,
        };
        if start > entry.size {
            return Err(EngineError::invalid(format!(
                "offset {start} is past the end of a {}-byte object",
                entry.size
            )));
        }
        let wanted = ChunkRanges::from_ranges([groups_for_byte_range(start, end)])
            .intersect(&ChunkRanges::single(0, group_count(entry.size)));
        // Every read path resolved an entry to get here, which means the
        // lineage that makes delta possible is already in hand: `synch cat`, a
        // adoption and the gateway's reads all get the descent for the price of
        // one `VersionSet` lookup (§3.5).
        //
        // A *ranged* read has to earn it, though. Promotion works a span at a
        // time and the two proof rounds come before the first byte, so a small
        // cold range would pay for both and then reuse 16 MiB to answer with a
        // few hundred: below one span the descent costs more than the read
        // (§4). Whole-object reads always descend.
        let whole = start == 0 && end == entry.size;
        let donors = {
            let node = self.clone();
            let (space, path) = (space.to_string(), path.to_string());
            let entry = entry.clone();
            let worth_descending = whole || end - start >= DESCENT_MIN_RANGE;
            crate::blocking::offload(move || {
                Ok(match node.store().versions_for(&space, &path) {
                    Ok(versions) if worth_descending => node.donors_for(&entry, &versions)?,
                    Ok(_) => Vec::new(),
                    Err(e) => {
                        tracing::debug!(error = %e, "no version set for donors");
                        Vec::new()
                    }
                })
            })
            .await?
        };
        let report = self
            .fetch_groups_from(&root, entry.size, &wanted, &donors)
            .await?;
        if !report.complete {
            return Err(EngineError::not_found(format!(
                "no provider could serve bytes {start}..{end} of {root}"
            )));
        }
        Ok(PreparedRange {
            root,
            size: entry.size,
            start,
            end,
        })
    }

    /// Prepares a read of an object named by its content root, with no path
    /// and no version policy involved (§8).
    ///
    /// `synch log` prints content roots and DESIGN.md §8 says reading an old
    /// version back is done by one; this is what makes that true. It is also
    /// what makes a replica's holdings reachable — an object no current entry
    /// names has no `<space>/<path>` left to ask for it by.
    ///
    /// No donors: a bare root has no entry, so it has no `prev` and no sibling
    /// versions to descend against. The read is an ordinary verified fetch.
    pub async fn prepare_root_range(
        &self,
        root: &Hash,
        start: u64,
        len: Option<u64>,
    ) -> Result<PreparedRange> {
        let size = {
            let store = self.store().clone();
            let root = *root;
            crate::blocking::offload(move || Ok(store.object_size(&root)?)).await?
        }
        .ok_or_else(|| {
            EngineError::not_found(format!(
                "nothing here knows the size of {root}: no local object, no entry naming \
                 it, and no peer advertising it"
            ))
        })?;
        if start > size {
            return Err(EngineError::invalid(format!(
                "offset {start} is past the end of a {size}-byte object"
            )));
        }
        let end = match len {
            Some(len) => start.saturating_add(len).min(size),
            None => size,
        };
        let wanted = ChunkRanges::from_ranges([groups_for_byte_range(start, end)])
            .intersect(&ChunkRanges::single(0, group_count(size)));
        let report = self.fetch_groups_from(root, size, &wanted, &[]).await?;
        if !report.complete {
            return Err(EngineError::not_found(format!(
                "no provider could serve bytes {start}..{end} of {root}"
            )));
        }
        Ok(PreparedRange {
            root: *root,
            size,
            start,
            end,
        })
    }

    /// Reads one origin's entry in full — the pinned form of
    /// [`Node::read_path`], which is what `synch adopt path` adopts from.
    pub async fn read_entry(&self, origin: &OriginId, space: &str, path: &str) -> Result<Vec<u8>> {
        self.read_path(space, path, &VersionPolicy::Origin(origin.clone()))
            .await
    }

    /// Materializes an object the CAS already holds onto the filesystem
    /// (`docs/DELTA-SYNC.md` §3.5).
    ///
    /// The one way an object becomes a file. A checkout writing its copy (§7.2),
    /// `synch adopt path` adopting a peer's version (§8), and the gateway's
    /// fetch-to-file all come through here, and all of them get the same
    /// guarantees: the target is old-or-new and never half, no staging residue
    /// is left behind on any path, and the object is never held in memory.
    ///
    /// The fetch has to have happened first — [`Node::fetch_all`] or
    /// [`Node::prepare_range`] — because this materializes what is verified and
    /// local, and refuses an object it does not hold whole rather than leaving
    /// a truncated file wearing a complete file's name.
    ///
    /// The payload is **cloned**, not copied. `FICLONE` on btrfs, XFS or
    /// bcachefs shares the CAS payload's extents with the new file: O(1),
    /// no data moved, and no second copy of the object on the disk until one of
    /// the two is written to. Where the ioctl cannot apply — the checkout is on a
    /// different filesystem from the CAS, ext4, a platform without it — the
    /// fallback is `std::fs::copy`, itself a kernel-side `copy_file_range` on
    /// Linux with no bounce through user space. Small objects live in the index
    /// rather than in a file (§6.2) and are written straight out of it.
    ///
    /// Returns which of those happened, which is what a checkout reports.
    pub(crate) async fn materialize_blob(
        &self,
        root: &Hash,
        size: u64,
        target: impl Into<std::path::PathBuf>,
    ) -> Result<CloneKind> {
        let target = target.into();
        let kind = self
            .cas_backend()
            .materialize(*root, size, target.clone())
            .await?;
        let kind = match kind {
            synch_store::backend::Materialization::Reflink => CloneKind::Reflink,
            synch_store::backend::Materialization::Copy => CloneKind::Copy,
        };
        tracing::debug!(target = %target.display(), clone = ?kind, size, "materializing an object");
        Ok(kind)
    }
}

/// The smallest range a cold read runs the delta descent for
/// (`docs/DELTA-SYNC.md` §4).
///
/// Delta promotes at span granularity, and the two proof rounds are round trips
/// before the first byte: a one-byte `synch cat --range` of an object nobody
/// here holds would pay both of them and then promote up to 16 MiB to answer
/// with 1 byte. A whole-object fetch always descends — that is the case delta
/// exists for — and a ranged one only when the range is worth a span.
const DESCENT_MIN_RANGE: u64 = synch_core::AD_SPAN_GRANULARITY;
#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::node;

    #[tokio::test]
    async fn a_cloud_pin_finalizes_cache_before_promising_it() {
        let data = tempfile::tempdir().unwrap();
        Node::init(data.path(), None).unwrap();
        let mut config = crate::config::NodeConfig::loopback(data.path());
        config.cloud = Some(synch_store::cloud::CloudConfig {
            service: synch_store::cloud::CloudService::Memory,
            options: Default::default(),
            scratch_dir: data.path().join("cloud-scratch"),
            io_timeout: std::time::Duration::from_secs(5),
            upload_policy: synch_store::cloud::CloudUploadPolicy::OwnPinned,
            cache_bytes: Some(512 * 1024 * 1024),
        });
        let node = Node::open(config).await.unwrap();
        let payload = vec![41u8; 100_000];
        let root = node.store().ingest_bytes(&payload, now_ns()).unwrap();
        let before = node.store().blob(&root).unwrap().unwrap();
        assert!(before.complete);
        assert!(!before.durable);
        assert_eq!(
            node.ad_change(&root).unwrap(),
            Some((synch_core::blob_key(&root), None))
        );

        node.pin_object(&root, Some(payload.len() as u64))
            .await
            .unwrap();
        let after = node.store().blob(&root).unwrap().unwrap();
        assert!(after.pinned);
        assert!(after.durable);
        assert!(node
            .store()
            .providers(&root)
            .unwrap()
            .into_iter()
            .any(|(origin, ad)| origin == *node.origin() && ad.is_complete()));
        assert!(node.unpin_object(&root).await.unwrap());
        assert!(!node.store().blob(&root).unwrap().unwrap().pinned);
        assert!(!node
            .store()
            .providers(&root)
            .unwrap()
            .into_iter()
            .any(|(origin, _)| origin == *node.origin()));
        node.reconstruct_recovered_cloud_rows().await.unwrap();
        assert!(!node.store().blob(&root).unwrap().unwrap().pinned);
        assert_eq!(
            node.cas_backend()
                .read_range(root, 17, 901 - 17)
                .await
                .unwrap(),
            payload[17..901]
        );
        node.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_pin_refuses_an_incomplete_fetch_and_own_accepts_an_existing_durable_object() {
        let partial_data = tempfile::tempdir().unwrap();
        Node::init(partial_data.path(), None).unwrap();
        let mut partial_config = crate::config::NodeConfig::loopback(partial_data.path());
        partial_config.cloud = Some(synch_store::cloud::CloudConfig {
            service: synch_store::cloud::CloudService::Memory,
            options: Default::default(),
            scratch_dir: partial_data.path().join("cloud-scratch"),
            io_timeout: std::time::Duration::from_secs(5),
            upload_policy: synch_store::cloud::CloudUploadPolicy::OwnPinned,
            cache_bytes: Some(512 * 1024 * 1024),
        });
        let partial_node = Node::open(partial_config).await.unwrap();
        let payload = vec![0x51; 100_000];
        let provider_dir = tempfile::tempdir().unwrap();
        let provider = synch_store::Store::open(provider_dir.path()).unwrap();
        let root = provider.ingest_bytes(&payload, now_ns()).unwrap();
        let wanted = ChunkRanges::single(0, 1);
        let (encoded, served) = provider.encode_slice(&root, &wanted).unwrap();
        partial_node
            .cas_backend()
            .write_slice(root, payload.len() as u64, served, encoded, now_ns())
            .await
            .unwrap();
        assert!(partial_node
            .pin_object(&root, Some(payload.len() as u64))
            .await
            .is_err());
        assert!(!partial_node.store().blob(&root).unwrap().unwrap().pinned);
        partial_node.shutdown().await.unwrap();

        let own_data = tempfile::tempdir().unwrap();
        Node::init(own_data.path(), None).unwrap();
        let mut own_config = crate::config::NodeConfig::loopback(own_data.path());
        own_config.cloud = Some(synch_store::cloud::CloudConfig {
            service: synch_store::cloud::CloudService::Memory,
            options: Default::default(),
            scratch_dir: own_data.path().join("cloud-scratch"),
            io_timeout: std::time::Duration::from_secs(5),
            upload_policy: synch_store::cloud::CloudUploadPolicy::Own,
            cache_bytes: Some(512 * 1024 * 1024),
        });
        let own_node = Node::open(own_config).await.unwrap();
        let durable = own_node
            .cas_backend()
            .ingest_bytes(payload, now_ns())
            .await
            .unwrap();
        own_node
            .pin_object(&durable.root, Some(durable.size))
            .await
            .unwrap();
        let row = own_node.store().blob(&durable.root).unwrap().unwrap();
        assert!(row.durable && row.pinned);
        own_node.shutdown().await.unwrap();
    }
    use synch_core::{BlobAd, FileEntry, GroupRange};
    use synch_store::{Binding, BindingSource};

    fn peer_addr(endpoint: &iroh::Endpoint) -> iroh::EndpointAddr {
        iroh::EndpointAddr::from_parts(
            endpoint.id(),
            endpoint
                .bound_sockets()
                .into_iter()
                .map(iroh::TransportAddr::Ip),
        )
    }

    /// Binds `key` to `origin` as a static trust: the shape every test
    /// partner is introduced with.
    fn bind(node: &Node, origin: &OriginId, key: synch_core::NodeId) {
        node.store()
            .put_binding(&Binding {
                origin: origin.clone(),
                node_id: key,
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

    fn trust(node: &Node, name: &str) -> (OriginId, synch_core::NodeId) {
        let key = iroh_base::SecretKey::generate().public();
        let origin = OriginId::named(name, "x.example").unwrap();
        bind(node, &origin, key);
        (origin, key)
    }

    fn link(node: &Node, peer: &Node, origin: &OriginId) {
        bind(node, origin, peer.node_id());
        node.remember_peer(&peer.net().direct_addr()).unwrap();
    }

    /// A peer answering `FindProviders` with a canned ad per root, counting asks.
    struct CountingPeer {
        origin: OriginId,
        key: synch_core::NodeId,
        hits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        endpoint: iroh::Endpoint,
    }

    impl CountingPeer {
        /// Binds one that answers `known` with an ad for `holder`, every other root with nothing.
        async fn bind(name: &str, known: Hash, holder: OriginId) -> CountingPeer {
            let secret = iroh_base::SecretKey::generate();
            let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
                .secret_key(secret.clone())
                .relay_mode(iroh::endpoint::RelayMode::Disabled)
                .clear_address_lookup()
                .clear_ip_transports()
                .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())
                .unwrap()
                .alpns(vec![synch_core::ALPN_MPT.to_vec()])
                .bind()
                .await
                .unwrap();
            let (serving, hits) = (
                endpoint.clone(),
                std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            );
            let counter = hits.clone();
            tokio::spawn(async move {
                while let Some(incoming) = serving.accept().await {
                    let Ok(connection) = incoming.await else {
                        continue;
                    };
                    while let Ok((mut send, mut recv)) = connection.accept_bi().await {
                        let Ok(synch_core::MptMessage::FindProviders { object_root }) =
                            synch_net::frame::read_frame(&mut recv).await
                        else {
                            break;
                        };
                        counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let ads = if object_root == known {
                            vec![(holder.clone(), BlobAd::complete(1000))]
                        } else {
                            Vec::new()
                        };
                        let _ = synch_net::frame::write_frame(
                            &mut send,
                            &synch_core::MptMessage::Providers { ads },
                        )
                        .await;
                        let _ = send.finish();
                    }
                }
            });
            CountingPeer {
                origin: OriginId::named(name, "x.example").unwrap(),
                key: secret.public(),
                hits,
                endpoint,
            }
        }

        fn asked(&self) -> usize {
            self.hits.load(std::sync::atomic::Ordering::Relaxed)
        }

        /// Makes the node trust this peer and know where to reach it.
        fn known_to(&self, node: &Node) {
            bind(node, &self.origin, self.key);
            let addr = peer_addr(&self.endpoint);
            node.store()
                .record_peer_seen(&self.key, Some(&crate::node::encode_addr(&addr)), now_ns())
                .unwrap();
        }
    }

    /// Discovery stops at the first answer and backs off after a fruitless
    /// round — or every checkout pass would re-dial the cluster (§6.3).
    #[tokio::test]
    async fn provider_discovery_stops_early_and_then_backs_off() {
        let (_d, node) = node().await;
        let held = Hash::new(b"an object somebody holds");
        let unheld = Hash::new(b"an object nobody holds");
        let (holder, _) = trust(&node, "holder");
        let first = CountingPeer::bind("peer-a", held, holder.clone()).await;
        let second = CountingPeer::bind("peer-b", held, holder.clone()).await;
        first.known_to(&node);
        second.known_to(&node);

        // One peer names a holder, so the rest of the cluster is not asked.
        let found = node.ask_peers_for_providers(&held, 1000).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].origin, holder);
        assert_eq!(
            first.asked() + second.asked(),
            1,
            "the walk stops at the first peer that answers"
        );
        assert!(!node.provider_discovery_backed_off(&held, now_ns()));

        // A root nobody can name costs one full round; the miss then dials nobody.
        let before = first.asked() + second.asked();
        assert!(node
            .ask_peers_for_providers(&unheld, 1000)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            first.asked() + second.asked() - before,
            2,
            "every peer is asked when none of them knows"
        );
        assert!(node.provider_discovery_backed_off(&unheld, now_ns()));
        assert!(node
            .ask_peers_for_providers(&unheld, 1000)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            first.asked() + second.asked() - before,
            2,
            "and asking again dials nobody while the miss is warm"
        );

        // The miss is a delay, not a verdict: it lets go on its own, and a held root clears it.
        let past_it = now_ns() + 2 * crate::node::PROVIDER_MISS_MAX_BACKOFF.as_nanos() as i64;
        assert!(!node.provider_discovery_backed_off(&unheld, past_it));
        node.store()
            .put_provider(&unheld, &holder, &BlobAd::complete(1000))
            .unwrap();
        assert_eq!(
            node.ask_peers_for_providers(&unheld, 1000)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(!node.provider_discovery_backed_off(&unheld, now_ns()));
    }

    /// Stale ads for vanished publishers must not suppress discovery of a live
    /// replica that holds the last copy but advertises only aggregate coverage.
    #[tokio::test]
    async fn a_fetch_rediscovers_after_every_known_provider_fails() {
        let (_fetcher_dir, fetcher) = node().await;
        let (_provider_dir, provider) = node().await;
        let provider_origin = provider.origin().clone();
        let fetcher_origin = fetcher.origin().clone();
        link(&fetcher, &provider, &provider_origin);
        link(&provider, &fetcher, &fetcher_origin);

        let payload = b"the cloud replica has the last copy".to_vec();
        let root = provider.store().ingest_bytes(&payload, now_ns()).unwrap();

        // It is known as an object provider, but speaks no blob protocol and
        // names no replacement. The real holder is a peer, not a provider row.
        let ghost =
            CountingPeer::bind("vanished", Hash::new(b"another root"), provider_origin).await;
        ghost.known_to(&fetcher);
        fetcher
            .store()
            .put_provider(
                &root,
                &ghost.origin,
                &BlobAd::complete(payload.len() as u64),
            )
            .unwrap();

        let wanted = ChunkRanges::single(0, group_count(payload.len() as u64));
        let report = fetcher
            .fetch_groups_from(&root, payload.len() as u64, &wanted, &[])
            .await
            .unwrap();
        assert!(report.complete);
        assert!(report.providers_tried >= 2);
        assert_eq!(
            fetcher
                .cas_backend()
                .read_range(root, 0, payload.len() as u64)
                .await
                .unwrap(),
            payload
        );

        ghost.endpoint.close().await;
        fetcher.shutdown().await.unwrap();
        provider.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn providers_are_ranked_by_latency() {
        let (_d, node) = node().await;
        let root = Hash::new(b"object");
        let (fast, fast_key) = trust(&node, "fast");
        let (slow, slow_key) = trust(&node, "slow");
        let (unknown, _) = trust(&node, "unknown");
        for origin in [&fast, &slow, &unknown] {
            node.store()
                .put_provider(&root, origin, &BlobAd::complete(1000))
                .unwrap();
        }
        node.store().record_peer_sync(&fast_key, 0, 1_000).unwrap();
        node.store()
            .record_peer_sync(&slow_key, 0, 500_000)
            .unwrap();

        let ranked = node.providers_for(&root, 0, 1000).unwrap();
        // A never-measured peer ranks at the median, not behind all: "unknown" is not "slow".
        assert_eq!(
            ranked.iter().map(|p| &p.origin).collect::<Vec<_>>(),
            vec![&fast, &unknown, &slow]
        );
    }

    /// No providers, never invented content: a locally complete object needs
    /// no fetch, nobody-serves reports incomplete, an empty object none at all.
    #[tokio::test]
    async fn a_fetch_with_no_providers_reports_the_shape_exactly() {
        let (_d, node) = node().await;
        let payload = vec![3u8; 100_000];
        let root = node.store().ingest_bytes(&payload, now_ns()).unwrap();
        let report = node.fetch_all(&root, payload.len() as u64).await.unwrap();
        assert!(report.complete);
        assert_eq!(report.providers_tried, 0);
        assert!(report.fetched.is_empty());

        let report = node
            .fetch_all(&Hash::new(b"nobody has this"), 100_000)
            .await
            .unwrap();
        assert!(!report.complete);
        assert_eq!(report.providers_tried, 0);

        // An empty object published by a peer and never held here: no CAS row,
        // no provider ad, and — being offline — nobody to ask either.
        let (peer, _) = trust(&node, "nas");
        let empty = Hash::new(b"");
        node.store()
            .put_entry(&peer, "s", "empty.txt", &FileEntry::file(0, 0, empty, 1))
            .unwrap();
        let report = node.fetch_all(&empty, 0).await.unwrap();
        assert!(report.complete, "{report:?}");
        assert_eq!(report.providers_tried, 0, "nobody should have been asked");
        // An entry claiming no bytes while naming some other object is not completed by invention.
        assert!(
            !node
                .fetch_all(&Hash::new(b"not the empty object"), 0)
                .await
                .unwrap()
                .complete
        );
    }

    /// Donors are the versions this node can supply bytes from, in the order
    /// `docs/DELTA-SYNC.md` §3.2 wants them tried.
    #[tokio::test]
    async fn donors_are_the_lineage_this_node_can_still_read() {
        let (_d, node) = node().await;
        let (peer, _) = trust(&node, "nas");
        let (rival, _) = trust(&node, "laptop");

        let previous = node
            .store()
            .ingest_bytes(&vec![1u8; 100_000], now_ns())
            .unwrap();
        let rival_root = node
            .store()
            .ingest_bytes(&vec![2u8; 100_000], now_ns())
            .unwrap();
        let mut selected = FileEntry::file(100_000, 5, Hash::new(b"the version being fetched"), 2);
        selected.prev = Some(previous);
        node.store()
            .put_entry(&peer, "s", "disk.img", &selected)
            .unwrap();
        // A version nobody has bytes of: named in the tree, useless as a donor, left out.
        let mut theirs = FileEntry::file(100_000, 4, rival_root, 1);
        theirs.prev = Some(Hash::new(b"a version this node never fetched"));
        node.store()
            .put_entry(&rival, "s", "disk.img", &theirs)
            .unwrap();

        let versions = node.store().versions_for("s", "disk.img").unwrap();
        let entry = versions
            .entries
            .iter()
            .find(|e| e.origin == peer)
            .unwrap()
            .clone();
        let donors = node.donors_for(&entry, &versions).unwrap();
        assert_eq!(
            donors,
            vec![Donor(previous), Donor(rival_root)],
            "the replaced version first, then the other version of the path"
        );
        assert!(
            !donors.contains(&Donor(Hash::new(b"the version being fetched"))),
            "the object being fetched is never its own donor"
        );
    }

    /// Round two looks inside spans a donor can speak to and nowhere else
    /// (`docs/DELTA-SYNC.md` §3.3): a span past every donor's end has nothing
    /// to compare against.
    #[tokio::test]
    async fn the_leaf_round_descends_only_where_a_donor_can_answer() {
        let (_d, node) = node().await;
        const GROUP: usize = 16 * 1024;
        // A donor of eight groups; the new version is nineteen groups and a bit,
        // so spans 8..16 are past its end and the right edge is no whole subtree.
        let payload: Vec<u8> = (0..8 * GROUP).map(|i| (i * 13 % 251) as u8).collect();
        let donor = node.store().ingest_bytes(&payload, now_ns()).unwrap();
        let span = |start: u64, groups: u64, whole: bool| ProvenSubtree {
            start,
            groups,
            cv: synch_core::Cv([0u8; 32]),
            whole,
        };
        let round_one = Proven {
            root: Hash::new(b"the version being fetched"),
            size: (19 * GROUP + 700) as u64,
            subtrees: vec![
                span(0, 4, true),
                span(4, 4, true),
                span(8, 4, true),
                span(12, 4, true),
                span(16, 4, false),
            ],
        };

        let all = ChunkRanges::single(0, 20);
        assert_eq!(
            node.unsettled_spans(&[Donor(donor)], &round_one, &all)
                .unwrap(),
            ChunkRanges::from_ranges([GroupRange::new(0, 8), GroupRange::new(16, 20)]),
            "only the spans the donor reaches, plus the object's right edge"
        );
        // With no donor, round two has only the right edge to look inside.
        assert_eq!(
            node.unsettled_spans(&[], &round_one, &all).unwrap(),
            ChunkRanges::single(16, 20)
        );
        // Spans are whole subtrees; the result is clipped to what was asked for.
        assert_eq!(
            node.unsettled_spans(&[Donor(donor)], &round_one, &ChunkRanges::single(5, 18))
                .unwrap(),
            ChunkRanges::from_ranges([GroupRange::new(5, 8), GroupRange::new(16, 18)])
        );
    }

    #[tokio::test]
    async fn a_fetch_keeps_going_past_the_first_fanout_candidates() {
        // §6.4: stopping after `fetch_fanout` candidates strands a fetch whose
        // fourth-ranked provider is the one that can serve it.
        let (_d, node) = node().await;
        let payload = vec![9u8; 100_000];
        let root = Hash::new(&payload);
        let size = payload.len() as u64;

        // Three undialable providers, all ranked ahead of the fourth by measured latency.
        for (i, name) in ["ghost-a", "ghost-b", "ghost-c"].iter().enumerate() {
            let (origin, key) = trust(&node, name);
            node.store()
                .put_provider(&root, &origin, &BlobAd::complete(size))
                .unwrap();
            node.store()
                .record_peer_sync(&key, 0, (i as i64 + 1) * 10)
                .unwrap();
        }

        // The fourth is a real node that holds the bytes.
        let holder_origin = OriginId::named("holder", "x.example").unwrap();
        let (_holder_dir, holder) = crate::testkit::node_as(&holder_origin).await;
        assert_eq!(
            holder.store().ingest_bytes(&payload, now_ns()).unwrap(),
            root
        );
        link(&node, &holder, &holder_origin);
        link(&holder, &node, node.origin());
        node.store()
            .put_provider(&root, &holder_origin, &BlobAd::complete(size))
            .unwrap();
        // Measured, slower than every ghost, so it deterministically ranks
        // last — stated rather than inherited (§6.4's tiebreak is random).
        node.store()
            .record_peer_sync(&holder.node_id(), 0, 100_000)
            .unwrap();

        let ranked = node.providers_for(&root, 0, size).unwrap();
        assert_eq!(ranked.len(), 4);
        assert_eq!(
            ranked[3].origin, holder_origin,
            "the one that works is ranked last"
        );

        let report = node.fetch_all(&root, size).await.unwrap();
        assert!(report.complete, "{report:?}");
        assert!(
            report.providers_tried > node.config().fetch_fanout,
            "the fetch must look past its first batch: {report:?}"
        );
        assert_eq!(node.store().read_all(&root).unwrap(), payload);
    }

    /// Complementary halves between two providers can serve an object.
    #[tokio::test]
    async fn complementary_holders_are_asked_for_what_they_hold() {
        let (_d, node) = node().await;
        // Four ad spans, so each half is a whole number, surviving `coalesce_spans`' rounding.
        let size = 4 * synch_core::AD_SPAN_GRANULARITY;
        let payload: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
        let half = size / 2;

        // A source of verified slices, so each holder gets exactly its half.
        let source_dir = tempfile::tempdir().unwrap();
        let source = synch_store::Store::open(source_dir.path()).unwrap();
        let root = source.ingest_bytes(&payload, now_ns()).unwrap();

        // The tempdirs outlive the loop: dropping one deletes a running store.
        let mut dirs = Vec::new();
        let mut holders = Vec::new();
        for (name, spans, latency) in [
            // The fast one holds the *second* half, the positional share it
            // would have been handed is the one it cannot serve.
            ("tail", vec![(half, size)], 10i64),
            ("head", vec![(0, half)], 1_000i64),
        ] {
            let origin = OriginId::named(name, "x.example").unwrap();
            let (dir, holder) = crate::testkit::node_as(&origin).await;
            dirs.push(dir);

            let mut want = ChunkRanges::from_ranges(
                spans
                    .iter()
                    .map(|&(s, e)| synch_core::groups_for_byte_range(s, e)),
            );
            while !want.is_empty() {
                let (encoded, served) = source.encode_slice(&root, &want).unwrap();
                holder
                    .store()
                    .write_slice(&root, size, &served, &encoded, now_ns())
                    .unwrap();
                want = want.difference(&served);
            }
            assert!(!holder.store().blob(&root).unwrap().unwrap().complete);

            link(&node, &holder, &origin);
            link(&holder, &node, node.origin());
            let ad = holder.store().local_ad(&root).unwrap().unwrap();
            assert!(!ad.is_complete(), "{:?}", ad.state.spans);
            node.store().put_provider(&root, &origin, &ad).unwrap();
            node.store()
                .record_peer_sync(&holder.node_id(), 0, latency)
                .unwrap();
            holders.push(holder);
        }

        let ranked = node.providers_for(&root, 0, size).unwrap();
        assert_eq!(ranked.len(), 2);
        assert!(
            ranked[0].claims.intersect(&ranked[1].claims).is_empty(),
            "the two claims are disjoint: {:?} vs {:?}",
            ranked[0].claims,
            ranked[1].claims
        );

        let report = node.fetch_all(&root, size).await.unwrap();
        assert!(report.complete, "{report:?}");
        assert_eq!(node.store().read_all(&root).unwrap(), payload);
    }

    #[tokio::test]
    async fn partial_ads_narrow_what_we_ask_for() {
        let (_d, node) = node().await;
        let g = synch_core::AD_SPAN_GRANULARITY;
        let root = Hash::new(b"object");
        let (origin, _) = trust(&node, "partial");
        node.store()
            .put_provider(&root, &origin, &BlobAd::partial(4 * g, [(0, g)]))
            .unwrap();
        // Self-ads and unbound origins are filtered out of the provider pool.
        node.store()
            .put_provider(&root, node.origin(), &BlobAd::complete(10))
            .unwrap();
        node.store()
            .put_provider(
                &root,
                &OriginId::named("stranger", "x.example").unwrap(),
                &BlobAd::complete(10),
            )
            .unwrap();

        // The head of the object is claimed, so the provider is offered for
        // it; the tail is not, so the provider is not offered for it at all.
        let head = node.providers_for(&root, 0, 100).unwrap();
        assert_eq!(head.len(), 1);
        assert!(head[0].claims.contains(0));
        assert!(node.providers_for(&root, 3 * g, 4 * g).unwrap().is_empty());
        assert_eq!(
            node.providers_for(&root, 0, 10).unwrap().len(),
            1,
            "only the bound, non-self provider remains"
        );
    }
}
