//! Holding a whole copy of a space (`docs/REPLICATION.md`).
//!
//! A replica is one this node holds *every* version of — every
//! origin's version of every path, not the one a policy would select — fetched
//! as it appears and pinned for as long as its policy says. It materializes
//! nothing onto the filesystem and publishes nothing; the checkout half of a
//! space is independent of this one and either may be absent.
//!
//! Three tasks, deliberately separate:
//!
//! - the **sweep** ([`Node::sweep_replicas`]) reconciles what the tree
//!   references against what this node holds and wants, and is the only place
//!   that decides anything from a *listing*;
//! - the **fetch loop** ([`Node::fetch_content_wants`]) is the only one that
//!   touches the network, and so the only one that needs rate limiting;
//! - the **live path** (`Store::apply_change`, inside the transaction that
//!   flips a head) reacts to one promotion at a time and is the only one that
//!   ever has positive evidence that a root left the tree.
//!
//! Releases follow the committed materialized view: only complete heads write
//! `entries`, a pending head leaves its origin's previous complete entries in
//! place, and an origin with no complete head contributes no GC roots yet.
//! Synchronization health remains visible, but it is not a node-wide release
//! barrier.

use std::time::Duration;

use synch_core::{now_ns, Hash};
use synch_store::{PinHolder, ReplicaCoverage, ReplicaPolicy, ReplicaRow};

use crate::{
    error::{EngineError, Result},
    node::Node,
};

/// How many failed attempts turn a want from a backlog into an alarm.
///
/// Five, at the backoff below, is roughly a day of trying. A want that has
/// failed that many times is not slow: its last provider left before this node
/// reached it, and the version is most likely already gone from the cluster.
pub const UNREACHABLE_ATTEMPTS: i64 = 5;

/// The shortest wait before a failed want is tried again.
const MIN_BACKOFF: Duration = Duration::from_secs(60);

/// The longest, reached after a handful of failures.
const MAX_BACKOFF: Duration = Duration::from_secs(6 * 3600);

/// What one sweep did to one space.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Objects newly wanted.
    pub wanted: usize,
    /// Scheduled releases called off because the tree names the root again.
    pub reprieved: usize,
    /// Claims scheduled to end, because nothing names the root any more.
    pub scheduled: usize,
    /// Claims that reached their scheduled release and were dropped.
    pub released: usize,
}

/// Whether a count has moved far enough to be worth a publish: a doubling in
/// either direction, or any move off zero.
fn doubled(published: u64, current: u64) -> bool {
    match (published, current) {
        (0, 0) => false,
        (0, _) | (_, 0) => true,
        (published, current) => {
            current >= published.saturating_mul(2) || current.saturating_mul(2) <= published
        }
    }
}

/// One admitted want, with the space that wants it.
struct WantPlan {
    want: synch_store::WantRow,
    space: String,
}

/// What one pass of the fetch loop did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FetchReport {
    /// Objects fetched and taken possession of.
    pub held: usize,
    /// Objects attempted and not held.
    pub failed: usize,
    /// Bytes that crossed the network.
    pub fetched_bytes: u64,
    /// Bytes a local donor supplied instead (`docs/DELTA-SYNC.md` §3.3).
    pub reused_bytes: u64,
    /// Objects not attempted because a space is at its budget.
    pub over_budget: usize,
}

/// Whether there are no pending heads and every bound origin has a complete
/// materialized baseline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewState {
    /// Every bound origin has a complete head materialized.
    Complete,
    /// It does not, and this is why. Fetching and release sweeps continue from
    /// the materialized complete view.
    Incomplete(String),
}

impl ViewState {
    /// True if synchronization is complete.
    pub fn is_complete(&self) -> bool {
        matches!(self, ViewState::Complete)
    }

    /// The reason, for `replica ls` and `doctor`.
    pub fn reason(&self) -> Option<&str> {
        match self {
            ViewState::Complete => None,
            ViewState::Incomplete(why) => Some(why),
        }
    }
}

/// Everything `replica ls <id>` reports about one replica.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplicaStatus {
    /// The space.
    pub replica: ReplicaRow,
    /// What it holds and wants.
    pub coverage: ReplicaCoverage,
    /// When the oldest outstanding want was first wanted.
    pub oldest_want: Option<i64>,
    /// When the soonest scheduled release falls due.
    pub next_release: Option<i64>,
    /// Whether synchronization has no pending or missing baseline head.
    pub view: ViewState,
    /// Objects the tree has stopped naming that this node is holding anyway,
    /// because too few other origins advertise them (§4.3).
    pub held_back: u64,
    /// Bytes held, by the origin that published the content.
    pub by_origin: Vec<(String, u64)>,
    /// What every origin — this one included — says it holds of the space.
    pub claims: Vec<(synch_core::OriginId, synch_core::ReplicaClaim)>,
}

impl Node {
    /// Adds a durable replica role independently of any local source.
    pub fn add_replica(
        &self,
        id: &str,
        retention: ReplicaPolicy,
        grace: Option<i64>,
        budget: Option<u64>,
        checkout_path: Option<String>,
    ) -> Result<()> {
        if self.store().replica(id)?.is_some() {
            return Err(EngineError::invalid(format!(
                "replica {id} already exists; use `synch replica set {id}`"
            )));
        }
        // A delegate holds what it may read and no more. The scope decides what
        // its `entries` ever contained, so replicating outside it would be a
        // standing want for content this node can never learn the size of.
        if let Some(scope) = self.store().local_scope()? {
            if !scope.iter().any(|granted| granted == id) {
                return Err(EngineError::invalid(format!(
                    "this node's read scope does not cover {id}, so it cannot replicate it"
                )));
            }
        }
        if retention == ReplicaPolicy::Forever && grace.is_some() {
            return Err(EngineError::invalid(
                "--grace applies only to current retention",
            ));
        }
        self.store().set_config(
            "replica.release_floor",
            &self.config().replica_release_floor.to_string(),
        )?;
        let checkout_path = checkout_path
            .map(|path| self.checkout_path(id, path))
            .transpose()?;
        self.store().put_replica(&ReplicaRow {
            space: id.to_string(),
            retention,
            grace: match retention {
                ReplicaPolicy::Current => {
                    Some(grace.unwrap_or(synch_store::DEFAULT_REPLICA_GRACE_SECS))
                }
                ReplicaPolicy::Forever => None,
            },
            budget,
            checkout_path,
        })?;
        self.replica_wake().notify_one();
        Ok(())
    }

    /// Replaces a replica's configuration while leaving source state alone.
    pub fn set_replica(
        &self,
        id: &str,
        retention: Option<ReplicaPolicy>,
        grace: Option<i64>,
        budget: Option<Option<u64>>,
        checkout_path: Option<Option<String>>,
    ) -> Result<()> {
        let Some(mut replica) = self.store().replica(id)? else {
            return Err(EngineError::not_found(format!("no replica {id}")));
        };
        if let Some(retention) = retention {
            replica.retention = retention;
        }
        if replica.retention == ReplicaPolicy::Forever && grace.is_some() {
            return Err(EngineError::invalid(
                "--grace applies only to current retention",
            ));
        }
        replica.grace = match replica.retention {
            ReplicaPolicy::Current => Some(
                grace
                    .or(replica.grace)
                    .unwrap_or(synch_store::DEFAULT_REPLICA_GRACE_SECS),
            ),
            ReplicaPolicy::Forever => None,
        };
        if let Some(budget) = budget {
            replica.budget = budget;
        }
        if let Some(checkout_path) = checkout_path {
            replica.checkout_path = checkout_path
                .map(|path| self.checkout_path(id, path))
                .transpose()?;
        }
        self.store().set_config(
            "replica.release_floor",
            &self.config().replica_release_floor.to_string(),
        )?;
        self.store().put_replica(&replica)?;
        self.replica_wake().notify_one();
        Ok(())
    }

    /// Removes a replica and releases its holds, optionally preserving held
    /// roots as explicit operator pins.
    pub fn remove_replica(&self, id: &str, pin_held: bool) -> Result<()> {
        if !self
            .store()
            .remove_replica(id, pin_held, self.store().read_instant()?)?
        {
            return Err(EngineError::not_found(format!("no replica {id}")));
        }
        self.replica_wake().notify_one();
        Ok(())
    }

    /// Reconciles every replica — or one — against the materialized complete
    /// view. Pending heads neither add nor remove references until promotion.
    pub fn sweep_replicas(&self, only: Option<&str>) -> Result<Vec<(String, SweepReport)>> {
        let now = self.store().read_instant()?;
        let mut out = Vec::new();
        for space in self.store().replicas()? {
            if only.is_some_and(|id| id != space.space) {
                continue;
            }
            let holder = space.holder();
            // Reprieve before scheduling, so a root that left one path and
            // arrived at another inside one interval is never briefly marked
            // for release on the strength of the half of that the sweep saw.
            let reprieved = self.store().clear_returned_releases(&holder)?;
            let wanted = self.store().stage_space_wants(&space.space, &holder, now)?;
            let scheduled = if space.retention.releases() {
                let at = now.saturating_add(space.grace_secs().saturating_mul(1_000_000_000));
                self.store().schedule_stale_releases_above(
                    &holder,
                    at,
                    self.config().replica_release_floor,
                )?
            } else {
                0
            };
            let released = self.store().expire_pins_of(&holder, now)?;
            out.push((
                space.space.clone(),
                SweepReport {
                    wanted,
                    reprieved,
                    scheduled,
                    released,
                },
            ));
        }
        Ok(out)
    }

    /// Whether synchronization has no pending or missing baseline head.
    ///
    /// This is synchronization health for reporting. Release sweeps use the
    /// complete heads already materialized in `entries` and do not gate on it.
    ///
    /// Both questions are asked as "is there one?" rather than by listing and
    /// looking: this runs once per status poll and per `replica ls`, and a
    /// replica serving ten thousand origins made each a whole-table read plus
    /// a point read per binding (`docs/CLOUD-DATAPLANE.md` §7.1a).
    pub fn view_state(&self) -> Result<ViewState> {
        // A pending head is newer than the materialized view. The prior
        // complete entries, when any, remain the release sweep's GC roots.
        if let Some(origin) = self.store().pending_head_origin()? {
            return Ok(ViewState::Incomplete(format!(
                "{origin} has a head this node cannot materialize yet"
            )));
        }
        // A bound origin with no complete head has never been synced here, or
        // was reset. It contributes no GC roots until a head is materialized.
        if let Some(origin) = self
            .store()
            .bound_origin_without_complete_head(self.origin())?
        {
            return Ok(ViewState::Incomplete(format!(
                "{origin} is bound but has published nothing this node holds"
            )));
        }
        Ok(ViewState::Complete)
    }

    /// Fetches source repairs and replica wants, rarest first.
    ///
    /// One pass keeps at most `replica_concurrency` objects in flight. A ranked,
    /// bounded candidate window feeds those slots continuously, so one bad
    /// provider consumes one slot rather than holding the next fixed batch
    /// behind its timeout. Everything about each fetch is the ordinary §6.4
    /// path — provider fanout, delta descent against the recorded donor,
    /// resumption — so a replica gets the best case of the descent for free: it
    /// is fetching version *n+1* of a file whose version *n* it is guaranteed
    /// to hold.
    pub async fn fetch_content_wants(&self) -> Result<FetchReport> {
        self.fetch_wants(None).await
    }

    /// Fetches one named replica's wants without spending the pass on other
    /// replicas or source repairs.
    pub async fn fetch_replica_wants_for(&self, space: &str) -> Result<FetchReport> {
        let store = self.store().clone();
        let replica_space = space.to_string();
        let exists = crate::blocking::offload(move || Ok(store.replica(&replica_space)?))
            .await?
            .is_some();
        if !exists {
            return Err(EngineError::not_found(format!("no replica {space}")));
        }
        let holder = PinHolder::Replica(space.to_string());
        self.fetch_wants(Some(holder)).await
    }

    async fn fetch_wants(&self, only: Option<PinHolder>) -> Result<FetchReport> {
        let mut report = FetchReport::default();
        let limit = self.config().replica_concurrency.max(1);
        // Candidates are drawn per holder and then ranked together. One global
        // queue ordered by age would let a space with a large old backlog
        // starve every other replica outright, and would leave the
        // `(holder, first_wanted)` index unusable — a global `ORDER BY` over a
        // holder-leading index is a scan and a temp sort of the whole queue,
        // on the one write connection, on every pass.
        // Advanced once per pass, so which space leads the interleave moves on.
        // Without it the first `replica_concurrency` spaces in id order take
        // every slot for ever and the rest wait out their backlogs.
        let rotate = self
            .replica_rotation()
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let wants = {
            let store = self.store().clone();
            crate::blocking::offload(move || {
                let now = store.read_instant()?;
                match only {
                    Some(holder) => Ok(store.wants_to_attempt_of(
                        &holder,
                        now,
                        MIN_BACKOFF.as_nanos() as i64,
                        MAX_BACKOFF.as_nanos() as i64,
                        limit,
                    )?),
                    None => Ok(store.wants_to_attempt(
                        now,
                        MIN_BACKOFF.as_nanos() as i64,
                        MAX_BACKOFF.as_nanos() as i64,
                        limit,
                        rotate,
                    )?),
                }
            })
            .await?
        };
        // The space rows and their budgets are read once for the batch, not
        // once per object (see `budget_state`).
        let mut admitted: Vec<WantPlan> = Vec::new();
        let mut budgets: std::collections::HashMap<String, Option<(u64, u64)>> =
            std::collections::HashMap::new();
        for want in wants {
            let Some(space) = want.holder.space().map(str::to_string) else {
                // A want whose holder is not a replica's belongs to nothing
                // this loop drives. Left alone rather than deleted: it is
                // another version's row, and §3.1 keeps those.
                continue;
            };
            if matches!(want.holder, PinHolder::Source(_)) {
                admitted.push(WantPlan { want, space });
                continue;
            }
            if !budgets.contains_key(&space) {
                budgets.insert(space.clone(), self.budget_state(&space).await?);
            }
            match budgets.get_mut(&space).expect("just inserted") {
                // The space stopped being replicated between the sweep and now.
                None => {
                    let store = self.store().clone();
                    let (root, holder) = (want.root, want.holder.clone());
                    crate::blocking::offload(move || Ok(store.drop_want(&root, &holder)?)).await?;
                }
                Some((held, budget)) if held.saturating_add(want.size) > *budget => {
                    // Skipped, not stopped: a smaller want further down still
                    // fits, and rejecting one must not end the pass. Nothing is
                    // recorded against the row either — a budget is not the
                    // want's fault, and charging it an attempt would push it
                    // into the backoff and eventually into `unreachable`.
                    report.over_budget += 1;
                }
                Some((held, _)) => {
                    // Counted as though it lands, so one batch cannot overshoot
                    // a budget by the size of the whole batch.
                    *held = held.saturating_add(want.size);
                    admitted.push(WantPlan { want, space });
                }
            }
        }

        // A rolling concurrency window, because this is the network-bound step
        // and the knob is named for it. Fixed batches make one dead provider's
        // timeout a barrier in front of every later object; refilling a slot as
        // soon as its object finishes contains that failure to one slot.
        let queued: Vec<_> = admitted
            .iter()
            .enumerate()
            .map(|(i, plan)| async move {
                let held = self
                    .hold_object(
                        &plan.want.root,
                        plan.want.size,
                        plan.want.prev.as_ref(),
                        &plan.want.holder,
                    )
                    .await;
                // Completion order differs from input order. Carry the
                // index so each failure is recorded against its own want.
                (i, held)
            })
            .collect();
        let outcomes = crate::join::futures_buffered(queued, limit).await;

        for (i, outcome) in outcomes {
            let plan = &admitted[i];
            match outcome {
                Ok((fetched, reused)) => {
                    report.held += 1;
                    report.fetched_bytes += fetched;
                    report.reused_bytes += reused;
                }
                Err(e) => {
                    report.failed += 1;
                    let store = self.store().clone();
                    let (root, holder, reason) =
                        (plan.want.root, plan.want.holder.clone(), e.to_string());
                    let now = {
                        let store = self.store().clone();
                        crate::blocking::offload(move || Ok(store.read_instant()?)).await?
                    };
                    // Logged rather than propagated: one row that could not
                    // record its failure must not discard the accounting of
                    // every outcome after it.
                    let recorded = crate::blocking::offload(move || {
                        Ok(store.record_want_failure(&root, &holder, now, &reason)?)
                    })
                    .await;
                    if let Err(e) = recorded {
                        tracing::warn!(error = %e, "could not record a replica fetch failure");
                    }
                    tracing::debug!(
                        root = %plan.want.root,
                        space = %plan.space,
                        error = %e,
                        "replica fetch failed"
                    );
                }
            }
        }
        Ok(report)
    }

    /// Fetches one object and takes possession of it for a holder.
    ///
    /// Finalize, *then* pin. On a cloud backend a pin is a promise about
    /// durable storage rather than about a scratch cache — `pin_object` already
    /// works this way, and `docs/SERVERLESS.md` §6.3 makes cache-only content
    /// evictable by design — so a claim written before the object is durable is
    /// a promise about bytes the backend is entitled to drop.
    async fn hold_object(
        &self,
        root: &Hash,
        size: u64,
        prev: Option<&Hash>,
        holder: &PinHolder,
    ) -> Result<(u64, u64)> {
        // A donor with no bytes here is a wasted pass over the proof list,
        // which is what `donors_for` filters for on the read path. The check
        // reads the blob row, so it goes over the blocking pool rather than
        // running on the worker polling this future (§10) — the guard aborts
        // the process for it, and a test that has entered a `BlockingScope`
        // will not notice.
        let donors: Vec<synch_store::Donor> = match prev.copied() {
            None => Vec::new(),
            Some(prev) => {
                let node = self.clone();
                crate::blocking::offload(move || {
                    Ok(match node.holds_any_of(&prev)? {
                        true => vec![synch_store::Donor(prev)],
                        false => Vec::new(),
                    })
                })
                .await?
            }
        };
        let fetched = self.fetch_all_from(root, size, &donors).await?;
        if !fetched.complete {
            return Err(EngineError::not_found(format!(
                "no provider could serve the complete object {root}"
            )));
        }
        self.finalize_cloud_object(root, true).await?;
        let store = self.store().clone();
        let (root, holder) = (*root, holder.clone());
        let now = now_ns();
        let held =
            crate::blocking::offload(move || Ok(store.take_possession(&root, &holder, now)?))
                .await?;
        if !held {
            return Err(EngineError::not_found(format!(
                "object {root} left the store before it could be held"
            )));
        }
        Ok((
            bytes_of(&fetched.fetched, size),
            bytes_of(&fetched.promoted, size),
        ))
    }

    /// A space's `(bytes held, ceiling)`, or `None` when it is not replicated.
    ///
    /// A missing ceiling means no ceiling; zero admits no non-empty object.
    /// Reaching a ceiling stops fetching and never shortens a release: a
    /// replica that let go of its grace window because a disk filled up would
    /// drop the recovery story exactly when nobody was watching (§3.8).
    ///
    /// Read once per space per pass, not once per object: `replica_coverage`
    /// aggregates over every pin the holder has, so asking it per candidate
    /// turns a budgeted replica — the large ones, which are what budgets are
    /// for — into a scan of its own pins for every object it fetches.
    async fn budget_state(&self, space: &str) -> Result<Option<(u64, u64)>> {
        let store = self.store().clone();
        let space = space.to_string();
        crate::blocking::offload(move || {
            let Some(row) = store.replica(&space)? else {
                return Ok(None);
            };
            let Some(budget) = row.budget else {
                return Ok(Some((0, u64::MAX)));
            };
            let coverage = store.replica_coverage(&row.holder(), UNREACHABLE_ATTEMPTS)?;
            Ok(Some((coverage.held_bytes, budget)))
        })
        .await
    }

    /// What `replica ls <id>` reports.
    pub fn replica_status(&self, id: &str) -> Result<ReplicaStatus> {
        let Some(space) = self.store().replica(id)? else {
            return Err(EngineError::not_found(format!("no replica {id}")));
        };
        let holder = space.holder();
        Ok(ReplicaStatus {
            coverage: self
                .store()
                .replica_coverage(&holder, UNREACHABLE_ATTEMPTS)?,
            oldest_want: self.store().oldest_want(&holder, UNREACHABLE_ATTEMPTS)?,
            next_release: self.store().next_release(&holder)?,
            view: self.view_state()?,
            // Only meaningful where the policy releases at all. Under
            // Under `forever` nothing is ever let go, so "too few peers advertise
            // these to let them go" would imply a release that peers could
            // unblock — and none is waiting on them.
            held_back: match space.retention.releases() {
                true => self
                    .store()
                    .held_back_by_replication_floor(&holder, self.config().replica_release_floor)?,
                false => 0,
            },
            by_origin: self.store().held_bytes_by_origin(&holder)?,
            claims: self.replica_claims_on(id)?,
            replica: space,
        })
    }

    /// The `r:<space>` records this node should be publishing (§4.1).
    ///
    /// Staged like `m:space/<id>` and the manifest, through the ordinary
    /// publisher, so a claim costs no head of its own. A space that stopped
    /// being replicated yields a removal, because a claim left standing over a
    /// space this node no longer holds is the one kind of lie this record can
    /// tell that nobody could check.
    pub(crate) fn replica_claim_changes(&self) -> Result<Vec<crate::node::StagedChange>> {
        let mut out = Vec::new();
        let mut claimed = std::collections::HashSet::new();
        for space in self.store().replicas()? {
            let policy = space.retention;
            let coverage = self
                .store()
                .replica_coverage(&space.holder(), UNREACHABLE_ATTEMPTS)?;
            let claim = synch_core::ReplicaClaim {
                v: synch_core::RECORD_VERSION,
                // The oldest claim this node holds for the space is when it
                // started holding it. Better than a stored timestamp, which
                // would be one more thing to keep in agreement with the pins.
                since_ns: self.store().oldest_pin(&space.holder())?.unwrap_or(0),
                policy: policy.render().to_string(),
                grace_secs: match policy.releases() {
                    true => space.grace_secs(),
                    false => 0,
                },
                objects: coverage.held,
                bytes: coverage.held_bytes,
                // Holding nothing and wanting nothing is what a space looks
                // like before its first sweep, and claiming full coverage there
                // says the opposite of the truth to the one report an operator
                // reads to find out whether coverage exists.
                complete: coverage.wanted == 0 && coverage.held > 0,
            };
            let bytes = synch_core::record::encode(&claim)?;
            out.push((synch_core::replica_claim_key(&space.space)?, Some(bytes)));
            claimed.insert(space.space.clone());
        }
        // Withdraw a claim over a space this node has stopped replicating. The
        // published set is the authority on what to withdraw, since the
        // configuration no longer remembers the space at all.
        for space in self.published_claim_spaces()? {
            if !claimed.contains(&space) {
                out.push((synch_core::replica_claim_key(&space)?, None));
            }
        }
        Ok(out)
    }

    /// The claim changes worth a publish of their own.
    ///
    /// Counts move on every object a bootstrap holds, and a head per fetched
    /// object would drown the cluster in publishes to say "still going". So the
    /// standing loop publishes only when something *material* changed: the
    /// policy, the grace window, whether the space is covered, a claim
    /// appearing or disappearing — or the object count doubling or halving.
    ///
    /// That last one is what keeps the number usable without making it chatty.
    /// Leaving counts to ride along with whatever the node publishes next reads
    /// as thrift and is a bad trade: a dedicated replica publishes nothing of
    /// its own, so its claim would sit at the zero it was created with while it
    /// held terabytes, and an operator reading "all of 0 objects held" would
    /// reasonably conclude the thing was broken. Doubling bounds the publishes
    /// to a logarithmic number per convergence and keeps the claim within a
    /// factor of two, which is what a coverage claim is for: it describes
    /// another node's disk at a moment that node chose (§4.2), and no reader
    /// may act on it beyond ordering its own work.
    pub(crate) fn material_claim_changes(&self) -> Result<Vec<crate::node::StagedChange>> {
        let mut out = Vec::new();
        for change in self.replica_claim_changes()? {
            let space = synch_core::parse_replica_claim_key(&change.0)?;
            let published = self.replica_claim_of(self.origin(), &space)?;
            let material = match (&change.1, &published) {
                // A claim appearing, or being withdrawn: always material.
                (Some(_), None) | (None, Some(_)) => true,
                (None, None) => false,
                (Some(bytes), Some(published)) => {
                    let claim: synch_core::ReplicaClaim = synch_core::record::decode(bytes)?;
                    claim.policy != published.policy
                        || claim.grace_secs != published.grace_secs
                        || claim.complete != published.complete
                        || doubled(published.objects, claim.objects)
                }
            };
            if material {
                out.push(change);
            }
        }
        Ok(out)
    }

    /// The spaces this node's own published trie carries a claim for.
    fn published_claim_spaces(&self) -> Result<Vec<String>> {
        let Some(head) = self.store().complete_head(self.origin())? else {
            return Ok(Vec::new());
        };
        let trie = synch_mpt::Trie::new(self.store().as_ref());
        let mut out = Vec::new();
        for (key, _) in trie.scan(
            head.root,
            &[synch_core::record::PREFIX_REPLICA, b':'],
            None,
            None,
        )? {
            if let Ok(space) = synch_core::parse_replica_claim_key(&key) {
                out.push(space);
            }
        }
        Ok(out)
    }

    /// What one origin says it holds of a space, if it says anything.
    ///
    /// Rendered as a claim wherever it is shown. It is a member's assertion
    /// about its own disk, and this node has no way to check it — §4.2.
    pub(crate) fn replica_claim_of(
        &self,
        origin: &synch_core::OriginId,
        space: &str,
    ) -> Result<Option<synch_core::ReplicaClaim>> {
        let Some(head) = self.store().complete_head(origin)? else {
            return Ok(None);
        };
        let trie = synch_mpt::Trie::new(self.store().as_ref());
        let Some(bytes) = trie.get(head.root, &synch_core::replica_claim_key(space)?)? else {
            return Ok(None);
        };
        let claim: synch_core::ReplicaClaim = synch_core::record::decode(&bytes)?;
        // A record from a future schema is refused rather than half-read, for
        // the reason `f:` and `b:` refuse one: postcard ignores trailing bytes,
        // so a v2 claim decodes as a v1 claim with the new field silently
        // missing.
        match synch_core::record::is_supported_version(claim.v) {
            true => Ok(Some(claim)),
            false => Ok(None),
        }
    }

    /// Every origin's claim on a space, for `replica ls <id>`.
    pub(crate) fn replica_claims_on(
        &self,
        space: &str,
    ) -> Result<Vec<(synch_core::OriginId, synch_core::ReplicaClaim)>> {
        // Every origin whose trie this node holds, not every origin with an
        // entry: a dedicated replica publishes no entries anywhere, so an
        // `entries`-derived candidate list cannot see the one node shape this
        // report exists for.
        let mut out = Vec::new();
        for origin in self.store().origins_with_complete_heads()? {
            if let Some(claim) = self.replica_claim_of(&origin, space)? {
                out.push((origin, claim));
            }
        }
        Ok(out)
    }

    /// Runs the standing replication loop until `shutdown` resolves.
    ///
    /// A sweep runs on every wake — a head flipping complete, a local publish,
    /// a space newly replicated — and once before the first wait, so a node
    /// restarted with a backlog starts working through it rather than waiting
    /// out an interval. The fetch loop runs after each sweep and then keeps
    /// running while it is making progress: one pass keeps at most
    /// `replica_concurrency` objects in flight over a bounded candidate window,
    /// and a cold replica has millions.
    pub async fn run_replicas(&self, shutdown: impl std::future::Future<Output = ()>) {
        crate::aae::run_standing(
            shutdown,
            self.replica_wake(),
            self.config().replica_interval,
            || self.replica_pass_logged(),
        )
        .await
    }

    /// One sweep and as much fetching as it turns up, logged rather than
    /// streamed: the standing loop has no client on the other end.
    async fn replica_pass_logged(&self) {
        let node = self.clone();
        let swept = crate::blocking::offload(move || node.sweep_replicas(None)).await;
        match swept {
            Ok(reports) => {
                for (space, report) in reports {
                    if report != SweepReport::default() {
                        tracing::info!(
                            space = %space,
                            wanted = report.wanted,
                            reprieved = report.reprieved,
                            scheduled = report.scheduled,
                            released = report.released,
                            "replica sweep"
                        );
                    }
                }
            }
            Err(e) => tracing::warn!(error = %e, "replica sweep failed"),
        }
        // Keep fetching while a pass is doing something. A pass that holds
        // nothing has either drained the queue or hit every backoff, and either
        // way the next wake is soon enough.
        loop {
            match self.fetch_content_wants().await {
                Ok(report) if report.held > 0 => {
                    tracing::info!(
                        held = report.held,
                        failed = report.failed,
                        fetched_bytes = report.fetched_bytes,
                        reused_bytes = report.reused_bytes,
                        over_budget = report.over_budget,
                        "replica fetch pass"
                    );
                }
                Ok(_) => break,
                Err(e) => {
                    tracing::warn!(error = %e, "replica fetch pass failed");
                    break;
                }
            }
        }
        self.publish_material_claims().await;
    }

    /// Publishes a coverage claim when one materially changed (§4.1).
    pub async fn publish_material_claims(&self) {
        let node = self.clone();
        let changes = match crate::blocking::offload(move || node.material_claim_changes()).await {
            Ok(changes) if !changes.is_empty() => changes,
            Ok(_) => return,
            Err(e) => {
                tracing::warn!(error = %e, "could not compute replication claims");
                return;
            }
        };
        // A node that cannot publish still replicates; it simply says nothing
        // about it. Refusing the whole pass over an unpublishable claim would
        // stop it holding content over a record nobody needs.
        //
        // Off the runtime worker: the check reads this origin's heads, and §10
        // aborts the process for a store read on a worker thread.
        let node = self.clone();
        let publishable =
            crate::blocking::offload(move || Ok(node.ensure_publishable().is_ok())).await;
        match publishable {
            Ok(true) => {}
            Ok(false) => {
                tracing::debug!("not publishing replication claims: this node cannot publish");
                return;
            }
            Err(e) => {
                tracing::warn!(error = %e, "could not check whether claims may be published");
                return;
            }
        }
        self.stage(changes);
        if let Err(e) = self.flush_staged().await {
            tracing::warn!(error = %e, "could not publish replication claims");
        }
    }
}

/// Bytes a set of chunk groups covers, clamped to the object.
///
/// Counted in bytes rather than groups so the tail group of a 100-byte file
/// does not report 16 KiB.
fn bytes_of(groups: &synch_core::ChunkRanges, size: u64) -> u64 {
    groups
        .ranges
        .iter()
        .map(|r| {
            let end = r.end.saturating_mul(synch_core::CHUNK_GROUP_SIZE).min(size);
            end.saturating_sub(r.start.saturating_mul(synch_core::CHUNK_GROUP_SIZE))
        })
        .sum()
}
