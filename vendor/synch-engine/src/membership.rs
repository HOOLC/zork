//! DNSSEC membership domains and the `synch doctor` report (§3.2, §9.2, §12).

use std::{net::SocketAddr, time::Duration};

use synch_core::{clock_is_trusted, now_ns, NodeId, OriginId};
use synch_net::dns::{
    clamp_ttl, DialHint, MemberResolver, MemberSet, RekorPolicy, ResolverOptions, DEFAULT_DOH_URL,
    DEFAULT_TRUST_GRACE, MIN_TTL,
};
use synch_store::{Binding, BindingSource, ClockStatus, Equivocation};

/// How long one domain's membership refresh may take before the pass moves on.
///
/// `refresh_these` walks the due domains **serially**, and they are unrelated
/// to one another: a binding lapses at `ttl + DEFAULT_TRUST_GRACE` from its
/// own last good refresh, whether or not its zone was the one that stalled.
/// The floor on that is `MIN_TTL + DEFAULT_TRUST_GRACE` — sixteen minutes —
/// so the deadline has to be a small fraction of it for the queue behind a
/// slow zone to still be served.
///
/// Ninety seconds is well above what an honest `member_set` costs even under
/// `Require` (up to 18 validated lookups, each bounded by the transport's own
/// per-exchange timeout) and far below the lapse window. A zone that exceeds
/// it costs itself its own refresh, and nothing else.
const REFRESH_DEADLINE: Duration = Duration::from_secs(90);

use crate::{
    error::{EngineError, Result},
    node::Node,
    recovery::{RecoveryState, UnreconciledHistory},
};

/// The shortest gap between two *triggered* re-resolutions of one domain
/// (§3.4).
///
/// The trigger is an inbound connection from an unbound key, which a peer that
/// keeps retrying produces as fast as it can dial; the cooldown is what keeps
/// that from becoming a query flood.
pub(crate) const DNS_TRIGGER_COOLDOWN: Duration = Duration::from_secs(30);

/// The shortest the DNS loop ever sleeps between passes.
const DNS_POLL_FLOOR: Duration = Duration::from_secs(1);
/// The longest the DNS loop sleeps, so a new zone is noticed promptly even
/// though nothing was configured when the loop last looked.
const DNS_POLL_CEILING: Duration = Duration::from_secs(30);

fn cooldown_ns() -> i64 {
    DNS_TRIGGER_COOLDOWN.as_nanos().min(i64::MAX as u128) as i64
}

/// The `ip:port` an `addr=` names, or `None` when it is not one.
fn direct_address(text: &str) -> Option<SocketAddr> {
    text.parse::<SocketAddr>().ok()
}

/// The relay base URL a `relay=` names, or `None` when it is not one.
///
/// A relay is a host this node makes outbound requests to, so the scheme and
/// host are checked rather than taken on the record's word.
fn relay_url(text: &str) -> Option<iroh::RelayUrl> {
    let url = text.parse::<iroh::RelayUrl>().ok()?;
    match matches!(url.scheme(), "http" | "https") && url.host_str().is_some_and(|h| !h.is_empty())
    {
        true => Some(url),
        false => None,
    }
}

/// When one configured domain is next due for re-resolution, and when it was
/// last attempted (§3.2, §3.4).
///
/// Held in memory rather than in the database: it is a scheduling detail of a
/// running daemon, and a restart re-resolving every domain once is exactly
/// right.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DomainSchedule {
    /// When the TTL this domain's last answer carried runs out.
    pub due_at: i64,
    /// When a lookup was last attempted, successful or not.
    pub last_attempt: i64,
    /// When a lookup last succeeded, 0 for never.
    pub last_success: i64,
    /// Why the last attempt failed, cleared by the next success.
    ///
    /// "DNSSEC bogus", "no records", and "resolver down" demand different
    /// operator responses; holding the reason here is what lets `doctor` and
    /// `domain ls` say which without a trip to the daemon log.
    pub last_error: Option<String>,
    /// Device keys the last successful answer published under more than one
    /// `id=`, every binding of which was dropped (§3.2).
    ///
    /// Held rather than printed once: the scheduled refresh path has nobody to
    /// report to at the time, and the condition lasts until a zone is edited.
    /// `synch doctor` reads it from here.
    pub ambiguous: Vec<NodeId>,
    /// The origin the last successful answer bound *this node's* device key to,
    /// when that is not the origin this node publishes under (§3.2).
    ///
    /// A node publishing as `laptop@cluster.example` while the record set binds
    /// its key to `nas@cluster.example` syncs nothing and looks healthy doing
    /// it; this is what makes `doctor` able to say so.
    pub self_origin_mismatch: Option<OriginId>,
}

/// What one domain's refresh attempt came to: the refresh, or why not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainOutcome {
    /// The domain attempted.
    pub domain: String,
    /// The refresh, or the failure the operator has to read.
    pub result: std::result::Result<DomainRefresh, String>,
}

/// One configured domain's health, as `doctor` and `domain ls` report it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainHealth {
    /// The membership domain.
    pub domain: String,
    /// Live DNS bindings this domain currently vouches for.
    pub bindings: usize,
    /// The schedule state, `None` before the first attempt of this process.
    pub schedule: Option<DomainSchedule>,
}

/// What one domain refresh did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainRefresh {
    /// The domain refreshed.
    pub domain: String,
    /// Bindings written or extended.
    pub bindings: usize,
    /// Device keys that appear under more than one identity (§3.2).
    pub ambiguous: Vec<NodeId>,
    /// TXT records that could not be parsed.
    pub rejected: usize,
    /// The origin the answer binds this node's own device key to, when that is
    /// not the origin this node publishes under (§3.2).
    pub self_origin_mismatch: Option<OriginId>,
    /// How long the answer is good for.
    pub ttl: Duration,
}

/// The trust configuration a daemon's membership resolver is running with
/// (§3.2, §4.1, §10.2).
///
/// Every one of these is settable by environment variable, so what a daemon
/// enforces is not visible from its command line. `doctor` and `daemon status`
/// print this, which is what distinguishes a `require` daemon from a
/// `--rekor off` one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustConfig {
    /// Whether a validated answer additionally requires a verified
    /// transparency-log record for the zone key that signed it.
    pub rekor: RekorPolicy,
    /// The DNS-over-HTTP(S) endpoint in force.
    pub doh_url: String,
    /// Set when `--dnssec-anchor` *replaced* the ICANN root: with it, nothing
    /// signed under the real root validates.
    pub dnssec_anchor: Option<String>,
    /// Set when `--rekor-key` replaced the pinned log key set, which also turns
    /// TUF pin refresh off outright.
    pub rekor_key: Option<String>,
    /// The TUF repository the pin set follows, `None` when refresh is off.
    pub tuf_url: Option<String>,
    /// The transparency-log keys a proof is checked against right now, as the
    /// `log_id` hex a proof names.
    pub log_keys: Vec<String>,
}

/// The membership resolver a daemon holds, or why it holds none (§3.2).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ResolverStatus {
    /// No process built one: an embedded node, or a one-shot command that runs
    /// no DNS loop.
    #[default]
    Absent,
    /// Built, with the trust configuration in force.
    Ready(TrustConfig),
    /// The configured options could not be built into a resolver — a
    /// mistyped anchor path, an empty anchor file, a malformed DoH URL. No
    /// membership refresh can run at all in this state, so bindings ossify and
    /// then lapse a grace window later.
    Failed(String),
}

/// The process-wide resolver and the status `doctor` reports for it.
#[derive(Debug, Default)]
pub(crate) struct ResolverSlot {
    pub(crate) resolver: Option<std::sync::Arc<synch_net::DnssecResolver>>,
    pub(crate) status: ResolverStatus,
}

impl TrustConfig {
    /// Summarizes the options a resolver was built from, plus the pin set that
    /// resolver actually holds.
    pub fn of(options: &ResolverOptions, log_keys: &synch_net::rekor::LogKeys) -> TrustConfig {
        TrustConfig {
            rekor: options.rekor_policy(),
            doh_url: options
                .doh_url
                .clone()
                .unwrap_or_else(|| DEFAULT_DOH_URL.to_string()),
            dnssec_anchor: options
                .trust_anchor
                .as_ref()
                .map(|p| p.display().to_string()),
            rekor_key: options.rekor_key.as_ref().map(|p| p.display().to_string()),
            tuf_url: match (options.no_tuf, &options.rekor_key) {
                (true, _) | (_, Some(_)) => None,
                (false, None) => Some(
                    options
                        .tuf_url
                        .clone()
                        .unwrap_or_else(|| synch_net::tuf::SIGSTORE_TUF_URL.to_string()),
                ),
            },
            log_keys: log_keys
                .keys()
                .iter()
                .map(|key| {
                    key.id
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>()
                })
                .collect(),
        }
    }
}

/// A `synch doctor` report.
#[derive(Debug, Clone)]
pub struct DoctorReport {
    /// This node's origin.
    pub origin: OriginId,
    /// This node's device keys and their states.
    pub device_keys: Vec<(NodeId, String)>,
    /// Every binding, live or lapsed.
    pub bindings: Vec<Binding>,
    /// Bindings whose expiry has passed.
    pub lapsed: Vec<Binding>,
    /// The complete head of every known origin, with whether we can serve it.
    pub heads: Vec<HeadStatus>,
    /// Detected same-seq forks, with both signed heads as proof (§4.4).
    pub equivocations: Vec<Equivocation>,
    /// Origins whose data this node holds without a live binding, each with
    /// how many entries it still carries.
    ///
    /// Two situations read the same way here. An origin that rotated and has
    /// not republished under a bound key yet is temporarily unverifiable
    /// (§3.4); an origin that was untrusted or dropped from the record set is
    /// permanently cut off from *future* participation while what it already
    /// published stays replicated and ages out with ordinary retention (§12).
    /// Neither removes anything from the tree, which is why the list exists.
    pub unbound_origins: Vec<(OriginId, usize)>,
    /// Whether this node is itself in key-loss recovery, and how far peers say
    /// its origin had got (§3.4).
    pub recovery: RecoveryState,
    /// Pre-recovery history we hold that the origin's current head does not
    /// supersede: the fork side of someone else's recovery (§3.4, §4.4).
    pub unreconciled: Vec<UnreconciledHistory>,
    /// Configured membership domains.
    pub domains: Vec<String>,
    /// The spaces this node may read, as last declared by a peer, or `None`
    /// for the whole keyspace (§5.5).
    ///
    /// A delegated node holds nothing outside this list and never will, so the
    /// list is the difference between "this node is broken" and "this node is
    /// working as delegated" — the same partial trie reads as either one until
    /// an operator can see the scope it was held under.
    pub local_scope: Option<Vec<String>>,
    /// Device keys a domain's answer published under more than one `id=`, whose
    /// every binding was therefore dropped (§3.2).
    ///
    /// A duplicate-`id=` zone edit is the ordinary cause, and its effect is that
    /// the member in question stops appearing in `trust ls` at all. §3.2 and
    /// `dns.rs` both say `doctor` reports the ambiguity; this is what makes that
    /// true on every refresh path rather than only on an explicit one.
    pub ambiguous: Vec<(String, NodeId)>,
    /// Domains whose answer binds this node's own device key to an origin other
    /// than the one it publishes under (§3.2).
    pub self_origin_mismatch: Vec<(String, OriginId)>,
    /// What the host clock reads and whether trust can be dated by it (§3.2).
    pub clock: ClockStatus,
    /// The trust configuration membership resolves under, or why there is none.
    pub trust: ResolverStatus,
    /// Trie and content storage counts.
    pub trie: synch_store::TrieStats,
    /// How many objects are held locally, and how many are complete.
    pub blobs: (usize, usize),
}

/// One origin's head state.
#[derive(Debug, Clone)]
pub struct HeadStatus {
    /// The origin.
    pub origin: OriginId,
    /// The complete head's seq, if any.
    pub complete_seq: Option<u64>,
    /// A pending head's seq, if a fetch is in progress.
    pub pending_seq: Option<u64>,
    /// True if we hold the full trie under the complete head and can serve it.
    pub servable: bool,
    /// True if at least one device key is currently bound to the origin.
    pub bound: bool,
    /// How many entries the origin publishes, across all spaces.
    pub entries: usize,
}

impl Node {
    /// Installs the one resolver this process refreshes membership through, or
    /// records why it has none (§3.2, §10.2).
    ///
    /// Called once, by whatever owns the DNS loop. Every later membership
    /// refresh — scheduled or asked for over the control socket — reads it back
    /// with [`Node::dns_resolver`], so the daily TUF bound and the pin state
    /// are one process's, not one request's.
    pub fn set_dns_resolver(
        &self,
        resolver: std::result::Result<std::sync::Arc<synch_net::DnssecResolver>, String>,
    ) {
        let mut slot = self.dns_resolver_slot();
        *slot = match resolver {
            Ok(resolver) => {
                let status = ResolverStatus::Ready(TrustConfig::of(
                    &self.config().dns,
                    &resolver.log_keys(),
                ));
                ResolverSlot {
                    resolver: Some(resolver),
                    status,
                }
            }
            Err(why) => ResolverSlot {
                resolver: None,
                status: ResolverStatus::Failed(why),
            },
        };
    }

    /// The resolver membership refreshes through, if this process has one.
    pub fn dns_resolver(&self) -> Option<std::sync::Arc<synch_net::DnssecResolver>> {
        self.dns_resolver_slot().resolver.clone()
    }

    /// The trust configuration membership resolves under, or why there is none.
    pub fn resolver_status(&self) -> ResolverStatus {
        self.dns_resolver_slot().status.clone()
    }

    /// The membership domain *configured* for this node, which the next start
    /// will take its name from (§3.1).
    ///
    /// This is the operator's setting, not necessarily the zone in force: an
    /// edit lands here immediately and identity is resolved once per process,
    /// so between a `synch domain set` and the next start the two differ. See
    /// `Node::resolving_domain` for what is actually being refreshed.
    pub fn domain(&self) -> Result<Option<String>> {
        Ok(self.store().membership_domain()?)
    }

    /// The zone this process resolves.
    ///
    /// The one its own name came from, where it has such a name: identity is
    /// frozen for the life of the process, so a domain set under a running
    /// daemon must not start pulling bindings from a zone this node is not yet
    /// named by, nor stop renewing the ones vouched for by the zone it is still
    /// publishing under.
    ///
    /// A node its zone does not name falls back to the configured domain — and
    /// that is not a corner case, it is what a delegate *is* (§3.5). A delegate
    /// belongs to a cluster and is named by no zone in it, so resolving only
    /// the zone in its own name left it resolving nothing and reaching named
    /// members through static bindings pinned by hand, which never expire.
    /// There is no third case: a node belongs to one cluster, so this is one
    /// zone or none, never a set.
    pub(crate) fn resolving_domain(&self) -> Option<String> {
        match self.origin().domain() {
            Some(named) => Some(named.to_string()),
            None => self.store().membership_domain().ok().flatten(),
        }
    }

    /// [`Node::resolving_domains`], read off the runtime worker.
    ///
    /// For a key-identified node the answer comes from `config`, so this is a
    /// store read — and every async caller of it is on a runtime worker, which
    /// §10 aborts the process for. `run_dns` already offloads the sibling call
    /// that works out the next delay, with the same reasoning in its comment;
    /// the refreshes it then awaits did not, so any node without a membership
    /// domain in its origin died on the loop's first tick.
    pub(crate) async fn resolving_domains_off_runtime(&self) -> Vec<String> {
        let node = self.clone();
        crate::blocking::offload(move || Ok(node.resolving_domains()))
            .await
            .unwrap_or_default()
    }

    /// The resolving domain as the refresh machinery wants it: none or one.
    ///
    /// Everything below schedules, refreshes and reports per domain, which is
    /// the same work whether there is one of them or none.
    pub(crate) fn resolving_domains(&self) -> Vec<String> {
        self.resolving_domain().into_iter().collect()
    }

    /// Sets the membership domain — the zone that will name this node (§3.1).
    ///
    /// Takes effect at the next start, which is where identity is resolved.
    /// This process goes on resolving the zone its current name came from, so
    /// "one node, one zone" holds at every instant rather than only between
    /// edits; the bindings of the zone being left are dropped by the migration
    /// that adopts the new name.
    pub fn set_domain(&self, domain: &str) -> Result<()> {
        let domain = synch_core::origin::normalize_domain(domain)
            .map_err(|e| EngineError::invalid(e.to_string()))?;
        self.store().set_membership_domain(Some(&domain))?;
        Ok(())
    }

    /// Drops the membership domain. The device key names this node at the next
    /// start, and for a node that zone named, that migration is what drops the
    /// zone's bindings.
    ///
    /// A node the zone never named — a delegate (§3.5) — has no such migration
    /// coming, so its bindings are dropped here instead.
    pub fn clear_domain(&self) -> Result<bool> {
        let Some(leaving) = self.domain()? else {
            return Ok(false);
        };
        self.store().set_membership_domain(None)?;
        // The delegate expectation belonged to that zone: a node with no zone
        // is named by its key outright, and leaving the flag set would carry a
        // claim about a cluster this node has left into the next one it joins.
        self.store().set_membership_expects_name(true)?;
        // A node this zone *names* keeps them: its identity is frozen for the
        // life of the process, so it is still publishing under that name and
        // still talking to those peers (§3.1). A node it does not name was
        // already key-identified and stays that way, so nothing would ever
        // remove them and a cluster this delegate just left would stay dialable
        // until every binding expired on its own.
        if self.origin().domain() != Some(leaving.as_str()) {
            self.store().drop_dns_bindings(&leaving)?;
        }
        Ok(true)
    }

    /// Re-resolves one domain and refreshes its bindings.
    ///
    /// The whole answer is discarded unless it validates end to end; bindings
    /// that merely vanished from DNS keep their existing expiry so a
    /// propagation glitch cannot shrink the member set (§3.2).
    pub(crate) async fn refresh_domain(
        &self,
        resolver: &dyn MemberResolver,
        domain: &str,
        now: i64,
    ) -> Result<DomainRefresh> {
        let (set, ttl) = resolver.resolve_members(domain).await?;
        // Resolution is asynchronous, but applying its answer is a SQLite
        // transaction. The daemon drives this method from a Tokio worker, so
        // doing that work inline blocks the runtime (and deliberately aborts
        // debug binaries under the §10 invariant).
        let node = self.clone();
        crate::blocking::offload(move || node.apply_member_set(&set, ttl, now)).await
    }

    /// Applies an already-validated member set to the bindings table.
    ///
    /// `now` is the wall-clock reading the new expiries are dated from, taken by
    /// the caller so the §3.2 rules can be exercised without live DNS and
    /// without a real clock.
    ///
    /// A reading no trust decision can be dated by refuses the whole refresh
    /// (§3.2, and see `synch_store::clock`): extending trust to a moment this
    /// node cannot place is the one thing worse than not extending it, and the
    /// refusal is reported rather than logged — it reaches `domain refresh`,
    /// `domain ls` and `doctor` as this domain's last error.
    pub(crate) fn apply_member_set(
        &self,
        set: &MemberSet,
        ttl: Duration,
        now: i64,
    ) -> Result<DomainRefresh> {
        if !clock_is_trusted(now) {
            return Err(EngineError::invalid(format!(
                "the host clock reads {now} ns since the epoch, which cannot date a trust \
                 decision: membership is not extended and every DNS binding is treated as \
                 expired until the clock is set (static trust is unaffected)"
            )));
        }
        let now = self.store().advance_trust_floor(now)?.max(now);
        let expires_at =
            now.saturating_add((ttl + DEFAULT_TRUST_GRACE).as_nanos().min(i64::MAX as u128) as i64);
        let bindings: Vec<Binding> = set
            .bindings
            .iter()
            .map(|(origin, key)| Binding {
                origin: origin.clone(),
                node_id: *key,
                source: BindingSource::Dns,
                domain: Some(set.domain.clone()),
                issuer: None,
                spaces: Vec::new(),
                note: None,
                added_at: now,
                expires_at: Some(expires_at),
            })
            .collect();
        self.store().refresh_dns_bindings(&set.domain, &bindings)?;

        // Dialing hints from the record set (§3.3) are recorded as peer
        // addresses so the very first dial can succeed without discovery.
        //
        // A hint is dialing data attached to a *member*, so it is applied only
        // for a key that holds a live binding: a key the answer merely mentions
        // — one §3.2 dropped as ambiguous, say — is nobody this node will talk
        // to, and `record_peer_seen` replaces `last_addr` rather than adding to
        // it. And each field is read as the one thing it means, so an `addr=`
        // value cannot fall through into a relay URL this node then makes
        // outbound requests to.
        //
        // **And only when this domain is the key's sole live source.** "Holds
        // a live binding" is not enough on its own: `peers_seen.last_addr` is
        // keyed on `node_id` alone and overwrites, nothing prunes it, and no
        // successful connection ever writes a real address back — every other
        // writer passes `None`, which `COALESCE` preserves. So a second
        // configured membership domain naming a key this node already trusts
        // would repoint every future dial of it, permanently, through an
        // answer that is genuinely DNSSEC-valid for *that* domain. Gating on
        // this answer's own bindings would not help: `refresh_dns_bindings`
        // ran first and has no cross-domain conflict check, so the hostile
        // answer binds the key itself and any this-answer test passes.
        //
        // Sole-source is the test that holds, and it is the same posture §3.2
        // already takes towards an ambiguous key: when two domains both vouch
        // for one key there is no way to say which one's dialing data is
        // right, so neither is used and discovery answers instead. A key bound
        // statically is likewise not a DNS answer's to repoint.
        for (key_bytes, hints) in &set.hints {
            let Ok(key) = NodeId::from_bytes(key_bytes) else {
                continue;
            };
            if !self.store().is_trusted_key(&key, now)? {
                continue;
            }
            if !self.hint_source_is_sole(&key, &set.domain, now)? {
                tracing::debug!(
                    domain = %set.domain,
                    "a dial hint was ignored: the key it names is also live from \
                     another source, so which hint is right cannot be decided"
                );
                continue;
            }
            let mut addr = iroh::EndpointAddr::new(key);
            for hint in hints {
                match hint {
                    DialHint::Addr(text) => {
                        if let Some(socket) = direct_address(text) {
                            addr = addr.with_ip_addr(socket);
                        }
                    }
                    DialHint::Relay(text) => {
                        if let Some(url) = relay_url(text) {
                            addr = addr.with_relay_url(url);
                        }
                    }
                }
            }
            if !addr.is_empty() {
                self.remember_peer(&addr)?;
            }
        }

        // What the record set says this node's own key is: §3.2's malformed-set
        // rule returns nothing for an absent or ambiguous key, so a value here
        // that disagrees with the origin this node publishes under is a real
        // misconfiguration — one that syncs nothing while looking healthy.
        let self_origin_mismatch = set
            .self_origin(&self.node_id())
            .filter(|origin| origin != self.origin());

        Ok(DomainRefresh {
            domain: set.domain.clone(),
            bindings: bindings.len(),
            ambiguous: set.ambiguous_keys.clone(),
            rejected: set.rejected.len(),
            self_origin_mismatch,
            ttl,
        })
    }

    /// Refreshes every configured domain.
    pub async fn refresh_domains(
        &self,
        resolver: &dyn MemberResolver,
    ) -> Result<Vec<DomainOutcome>> {
        let domains = self.resolving_domains_off_runtime().await;
        self.refresh_these(resolver, &domains, now_ns()).await
    }

    /// Resolves a domain argument against the configured set (§9.2).
    ///
    /// Naming a domain this node was never told about is a typo, not a
    /// resolver problem, so it is refused before any lookup is attempted.
    pub fn configured_domain(&self, domain: &str) -> Result<String> {
        let name = synch_core::origin::normalize_domain(domain)
            .map_err(|e| EngineError::invalid(e.to_string()))?;
        if !self.resolving_domains().contains(&name) {
            return Err(EngineError::not_found(format!(
                "{name} is not a configured membership domain"
            )));
        }
        Ok(name)
    }

    /// Refreshes one configured domain by name, or every one if `domain` is
    /// `None` (`synch domain refresh [<domain>]`).
    pub async fn refresh_domains_named(
        &self,
        resolver: &dyn MemberResolver,
        domain: Option<&str>,
    ) -> Result<Vec<DomainOutcome>> {
        // The resolving domain is the origin's, held in memory, so neither
        // branch touches the store.
        let domains = match domain {
            None => self.resolving_domains_off_runtime().await,
            Some(name) => vec![self.configured_domain(name)?],
        };
        self.refresh_these(resolver, &domains, now_ns()).await
    }

    /// Re-resolves the domains whose TTL has elapsed (§3.2).
    ///
    /// This is what the daemon's DNS loop calls. A domain that has never been
    /// resolved is due immediately; otherwise it comes due one clamped TTL
    /// after the answer that produced its bindings.
    pub(crate) async fn refresh_due_domains(
        &self,
        resolver: &dyn MemberResolver,
        now: i64,
    ) -> Result<Vec<DomainOutcome>> {
        let configured = self.resolving_domains_off_runtime().await;
        let due: Vec<String> = {
            let schedule = self.dns_schedule();
            configured
                .into_iter()
                .filter(|d| schedule.get(d).is_none_or(|s| s.due_at <= now))
                .collect()
        };
        self.refresh_these(resolver, &due, now).await
    }

    /// Re-resolves every configured domain now, subject to a per-domain
    /// cooldown (§3.4).
    ///
    /// This is the unknown-key trigger: an inbound connection from a device key
    /// with no live binding is the shape a lagging rotation takes, so it earns
    /// an immediate lookup. The cooldown is what keeps a peer that keeps
    /// retrying — or a hostile one — from turning that into a query flood.
    pub(crate) async fn refresh_triggered(
        &self,
        resolver: &dyn MemberResolver,
        now: i64,
    ) -> Result<Vec<DomainOutcome>> {
        // Read before the guard is taken: the schedule lock is not `Send`, so
        // awaiting while it is held would make this future unspawnable.
        let configured = self.resolving_domains_off_runtime().await;
        let ready: Vec<String> = {
            let schedule = self.dns_schedule();
            configured
                .into_iter()
                .filter(|d| {
                    schedule
                        .get(d)
                        .is_none_or(|s| now.saturating_sub(s.last_attempt) >= cooldown_ns())
                })
                .collect()
        };
        self.refresh_these(resolver, &ready, now).await
    }

    /// How long the DNS loop should sleep before it next looks for due
    /// domains.
    pub(crate) fn next_dns_delay(&self, now: i64) -> Duration {
        let schedule = self.dns_schedule();
        let soonest = self
            .resolving_domains()
            .into_iter()
            .map(|d| schedule.get(&d).map(|s| s.due_at).unwrap_or(now))
            .min();
        drop(schedule);
        let gap = match soonest {
            // Nothing configured: wake occasionally anyway, because `domain
            // add` can land at any time and the loop is what notices.
            None => DNS_POLL_CEILING,
            Some(due) => Duration::from_nanos(due.saturating_sub(now).max(0) as u64),
        };
        gap.clamp(DNS_POLL_FLOOR, DNS_POLL_CEILING)
    }

    /// Re-resolves membership on the TTL, and on demand (§3.2, §3.4).
    ///
    /// Without this loop a DNSSEC cluster dissolves one TTL plus grace after
    /// the last manual `synch domain refresh`: `maintenance_pass` expires
    /// bindings on schedule and nothing renews them.
    pub async fn run_dns(
        &self,
        resolver: &dyn MemberResolver,
        shutdown: impl std::future::Future<Output = ()>,
    ) {
        let mut shutdown = std::pin::pin!(shutdown);
        let wake = self.dns_wake();
        loop {
            // The configured domain list is a `config` read, so working out the
            // next delay is store work like anything else (§10).
            let node = self.clone();
            let delay = crate::blocking::offload(move || Ok(node.next_dns_delay(now_ns())))
                .await
                .unwrap_or(DNS_POLL_FLOOR);
            let refreshed = tokio::select! {
                _ = &mut shutdown => return,
                _ = tokio::time::sleep(delay) => {
                    self.refresh_due_domains(resolver, now_ns()).await
                }
                _ = wake.notified() => {
                    self.refresh_triggered(resolver, now_ns()).await
                }
            };
            if let Err(e) = refreshed {
                tracing::warn!(error = %e, "membership refresh loop failed");
            }
        }
    }

    /// Refreshes the named domains, recording the schedule each answer implies.
    ///
    /// A resolver failure is not allowed to shrink the member set: the cached
    /// bindings keep their own expiry and only the retry time moves (§3.2,
    /// fail closed).
    async fn refresh_these(
        &self,
        resolver: &dyn MemberResolver,
        domains: &[String],
        now: i64,
    ) -> Result<Vec<DomainOutcome>> {
        let mut out = Vec::new();
        for domain in domains {
            // Stamped before the lookup runs, so a resolver that hangs or fails
            // cannot be retried in a tight loop.
            self.note_dns_attempt(domain, now, MIN_TTL);
            // Bounded, because this loop is serial and the domains in it are
            // unrelated. One `member_set` under `Require` issues up to 18
            // validated lookups — membership TXT, DNSKEY, and the proof
            // records — each of which recurses, and the transport's own
            // timeout applies per exchange rather than to the whole thing. A
            // zone that answers slowly enough therefore spends every other
            // domain's refresh budget, and their bindings lapse at
            // `ttl + DEFAULT_TRUST_GRACE` whether or not their own zone was
            // ever asked. `rekor.rs` names this exact failure and closes only
            // the part-count multiplier: "the threat model's attacker *is*
            // the zone, so 'it would only be hurting itself' does not hold."
            //
            // The deadline is per domain and well inside that lapse window,
            // so a hostile or merely broken zone costs itself its own refresh
            // and nothing else. Timing out is an ordinary refresh failure:
            // cached bindings keep their expiry and only the retry moves.
            let result = match tokio::time::timeout(
                REFRESH_DEADLINE,
                self.refresh_domain(resolver, domain, now),
            )
            .await
            .unwrap_or_else(|_| {
                Err(synch_net::NetError::Dns(format!(
                    "{domain}: membership refresh exceeded {}s and was abandoned so the \
                     other domains due in this pass could run; cached bindings are kept",
                    REFRESH_DEADLINE.as_secs()
                ))
                .into())
            }) {
                Ok(refresh) => {
                    self.note_dns_attempt(domain, now, refresh.ttl);
                    self.note_dns_outcome(domain, now, None);
                    self.note_dns_findings(domain, &refresh);
                    Ok(refresh)
                }
                Err(e) => {
                    tracing::warn!(domain, error = %e, "membership refresh failed; keeping cached bindings");
                    self.note_dns_outcome(domain, now, Some(e.to_string()));
                    Err(e.to_string())
                }
            };
            out.push(DomainOutcome {
                domain: domain.clone(),
                result,
            });
        }
        Ok(out)
    }

    /// Whether `domain` is the only live source vouching for `key`.
    ///
    /// True when every live binding for the key is a DNS binding from this
    /// same domain. A live binding from another domain, or a static one, makes
    /// it false: the key is not this answer's alone to supply dialing data
    /// for.
    fn hint_source_is_sole(&self, key: &NodeId, domain: &str, now: i64) -> Result<bool> {
        let bindings = self.store().bindings_for_key(key)?;
        let clock = self.store().trust_instant(now)?;
        let mut live = bindings.iter().filter(|b| b.is_live(clock)).peekable();
        if live.peek().is_none() {
            return Ok(false);
        }
        Ok(live.all(|b| b.source == BindingSource::Dns && b.domain.as_deref() == Some(domain)))
    }

    fn note_dns_attempt(&self, domain: &str, now: i64, ttl: Duration) {
        let due_at = now.saturating_add(clamp_ttl(ttl).as_nanos().min(i64::MAX as u128) as i64);
        let mut schedule = self.dns_schedule();
        let entry = schedule.entry(domain.to_string()).or_default();
        entry.last_attempt = now;
        entry.due_at = due_at;
    }

    fn note_dns_outcome(&self, domain: &str, now: i64, error: Option<String>) {
        let mut schedule = self.dns_schedule();
        let entry = schedule.entry(domain.to_string()).or_default();
        match error {
            None => {
                entry.last_success = now;
                entry.last_error = None;
            }
            Some(error) => entry.last_error = Some(error),
        }
    }

    /// Records what a successful answer was wrong about, for `doctor`.
    ///
    /// The scheduled path has nobody to stream progress lines to, and an
    /// ambiguity or a self-origin mismatch persists until a zone is edited — so
    /// the finding has to be held rather than printed once and dropped.
    fn note_dns_findings(&self, domain: &str, refresh: &DomainRefresh) {
        if !refresh.ambiguous.is_empty() {
            tracing::warn!(
                domain,
                keys = refresh.ambiguous.len(),
                "device keys published under more than one id bound nothing; see `synch doctor`"
            );
        }
        if let Some(origin) = &refresh.self_origin_mismatch {
            tracing::warn!(
                domain,
                %origin,
                "this node's device key is published under a different origin than it publishes as"
            );
        }
        let mut schedule = self.dns_schedule();
        let entry = schedule.entry(domain.to_string()).or_default();
        entry.ambiguous = refresh.ambiguous.clone();
        entry.self_origin_mismatch = refresh.self_origin_mismatch.clone();
    }

    /// The membership domain's health: bindings held, schedule, last error.
    ///
    /// The last error is what keeps a failing domain from reading like a
    /// healthy one in `doctor`.
    pub fn domain_health(&self) -> Result<Vec<DomainHealth>> {
        let bindings = self.store().bindings()?;
        let schedule = self.dns_schedule().clone();
        Ok(self
            .resolving_domains()
            .into_iter()
            .map(|domain| {
                let held = bindings
                    .iter()
                    .filter(|b| {
                        b.source == synch_store::BindingSource::Dns
                            && b.domain.as_deref() == Some(&domain)
                    })
                    .count();
                DomainHealth {
                    schedule: schedule.get(&domain).cloned(),
                    domain,
                    bindings: held,
                }
            })
            .collect())
    }

    /// Builds the `synch doctor` report.
    pub fn doctor(&self) -> Result<DoctorReport> {
        let now = now_ns();
        let clock = self.store().clock_status(now)?;
        let (ambiguous, self_origin_mismatch) = {
            let schedule = self.dns_schedule();
            let mut ambiguous = Vec::new();
            let mut mismatch = Vec::new();
            for (domain, state) in schedule.iter() {
                for key in &state.ambiguous {
                    ambiguous.push((domain.clone(), *key));
                }
                if let Some(origin) = &state.self_origin_mismatch {
                    mismatch.push((domain.clone(), origin.clone()));
                }
            }
            ambiguous.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.as_bytes().cmp(b.1.as_bytes())));
            mismatch.sort_by(|a, b| a.0.cmp(&b.0));
            (ambiguous, mismatch)
        };
        let bindings = self.store().bindings()?;
        // Lapsed by the cascade, not by the date alone: a delegated binding
        // whose issuer's own rooted binding is gone has lapsed too, and
        // reporting it as live is precisely the invisible half of the hole
        // §3.5's cascade exists to close.
        //
        // This carries the trust floor with it rather than dropping it:
        // `live_bindings` opens by flooring `now` through `trust_instant`, so
        // the report still agrees with `is_bound` and `keys_for_origin` on a
        // clock that stepped backwards — one report contradicting itself is
        // worse than either answer.
        let live: std::collections::HashSet<(String, Vec<u8>, &str)> = self
            .store()
            .live_bindings(now)?
            .into_iter()
            .map(|b| {
                (
                    b.origin.canonical(),
                    b.node_id.as_bytes().to_vec(),
                    b.source.as_str(),
                )
            })
            .collect();
        let lapsed: Vec<Binding> = bindings
            .iter()
            .filter(|b| {
                !live.contains(&(
                    b.origin.canonical(),
                    b.node_id.as_bytes().to_vec(),
                    b.source.as_str(),
                ))
            })
            .cloned()
            .collect();

        let mut heads = Vec::new();
        let mut unbound_origins = Vec::new();
        let mut origins: Vec<OriginId> = self
            .store()
            .all_heads(synch_store::Slot::Complete)?
            .into_iter()
            .map(|s| s.head.origin)
            .collect();
        for stored in self.store().all_heads(synch_store::Slot::Pending)? {
            if !origins.contains(&stored.head.origin) {
                origins.push(stored.head.origin);
            }
        }
        for origin in self.store().trusted_origins(now)? {
            if !origins.contains(&origin) {
                origins.push(origin);
            }
        }
        // An origin whose binding has gone keeps its entries — nothing
        // cascades a deletion through everyone's tries — so it has to be
        // listed even when no head slot survives for it (§12).
        for origin in self.store().entry_origins()? {
            if !origins.contains(&origin) {
                origins.push(origin);
            }
        }
        origins.sort();

        let trie = synch_mpt::Trie::new(self.store().as_ref());
        let mut unreconciled = Vec::new();
        for origin in origins {
            unreconciled.extend(self.unreconciled_history(&origin)?);
            let complete = self.store().complete_head(&origin)?;
            let pending = self.store().pending_head(&origin)?;
            // Scoped, like promotion and like the summaries this node
            // advertises (§5.5): under a read scope the unscoped answer is
            // false by construction for every foreign origin, so an unscoped
            // check here would report a delegate holding exactly what it was
            // granted as PARTIAL on every line — the one report an operator
            // consults to tell a broken node from a confined one.
            //
            // `materialization_scope` rather than the read scope flat, because
            // this node's *own* trie is one it built and therefore holds
            // whole; judging it by the grant would call a genuinely partial
            // local trie servable.
            let servable = match &complete {
                Some(head) => trie.is_complete_scoped_for(
                    self.store().provenance_owner(&origin, now)?.as_ref(),
                    head.root,
                    &self.store().materialization_scope(&origin)?,
                )?,
                None => false,
            };
            let bound = !self.store().keys_for_origin(&origin, now)?.is_empty();
            let mut entries = 0;
            for space in self.store().known_spaces()? {
                entries += self
                    .store()
                    .list_entries(Some(&origin), &space, "", None, None)?
                    .len();
            }
            if !bound && (complete.is_some() || entries > 0) {
                unbound_origins.push((origin.clone(), entries));
            }
            heads.push(HeadStatus {
                origin,
                complete_seq: complete.map(|h| h.seq),
                pending_seq: pending.map(|h| h.seq),
                servable,
                bound,
                entries,
            });
        }

        // Only the completeness flag and the count are read, so this takes the
        // projection rather than every inline payload in the store.
        let blobs = self.store().blob_candidates()?;
        let complete_blobs = blobs.iter().filter(|b| b.complete).count();
        Ok(DoctorReport {
            origin: self.origin().clone(),
            device_keys: self
                .store()
                .device_keys()?
                .into_iter()
                .map(|k| (k.node_id, k.state.as_str().to_string()))
                .collect(),
            bindings,
            lapsed,
            heads,
            equivocations: self.store().equivocations()?,
            unbound_origins,
            recovery: self.recovery_state()?,
            unreconciled,
            domains: self.resolving_domains(),
            local_scope: self.store().local_scope()?,
            ambiguous,
            self_origin_mismatch,
            clock,
            trust: self.resolver_status(),
            trie: self.store().trie_stats()?,
            blobs: (blobs.len(), complete_blobs),
        })
    }

    /// Rebuilds `entries` and `blob_providers` from the authoritative trie
    /// (`synch repair rebuild-views`, §10).
    ///
    /// Per origin, and one origin's failure does not stop the others. Each
    /// `rematerialize` is its own transaction, so a failure rolls back and is
    /// reported for only that origin while the remaining origins continue.
    pub fn rebuild_views(&self) -> Result<usize> {
        let mut total = 0;
        let mut failed = Vec::new();
        // `complete_slot_roots`, not `all_heads`: a rebuild needs the origin and the
        // root and nothing else, and `all_heads` skips a row whose signature will
        // not parse — which would have this report success having quietly not
        // rebuilt that origin at all.
        let complete = self.store().complete_slot_roots()?;
        // A row that will not read is a named failure, not a silent skip and not
        // a reason to rebuild nothing.
        failed.extend(complete.unreadable);
        for (origin, root) in complete.roots {
            match self.store().rematerialize(&origin, root) {
                Ok(n) => total += n,
                Err(e) => {
                    tracing::warn!(
                        origin = %origin,
                        error = %e,
                        "this origin's views could not be rebuilt; the others were"
                    );
                    failed.push(origin.canonical());
                }
            }
        }
        if !failed.is_empty() {
            return Err(EngineError::invalid(format!(
                "rebuilt every origin except {}; see the log for why",
                failed.join(", ")
            )));
        }
        Ok(total)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NodeConfig;
    use iroh_base::SecretKey;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex, PoisonError,
    };

    /// A node named by `cluster.example`, the zone it refreshes (§3.1).
    async fn node() -> (tempfile::TempDir, Node) {
        let dir = tempfile::tempdir().unwrap();
        Node::init_named_by_zone(
            dir.path(),
            OriginId::named("self", "cluster.example").unwrap(),
        )
        .unwrap();
        let node = Node::open(NodeConfig::loopback(dir.path())).await.unwrap();
        (dir, node)
    }

    /// The DNS record line for one `(id, key)`.
    fn rec(id: &str, key: &NodeId) -> String {
        format!("v=sync1 id={id} nk={}", key.to_z32())
    }

    /// §3.2: a node resolves the zone it *belongs to*, whether or not that
    /// zone names it — which is the whole route a delegate has to its cluster.
    ///
    /// A delegated node is named by no zone (§3.5). Resolving only the zone in
    /// its own name left it resolving nothing, so it could reach a member
    /// publishing under a name only through a hand-pinned static binding
    /// (`trust add --as`) that never expired and shadowed the record it named.
    /// The configured domain is the fallback, and there is no third case: a
    /// node belongs to one cluster, so this is one zone or none.
    #[tokio::test]
    async fn a_node_resolves_its_zone_even_when_that_zone_does_not_name_it() {
        // A delegate: identified by its own key, belonging to a zone that
        // publishes no record for it. `init` with no domain is exactly that
        // shape, and identity is frozen for the process, so this cannot be
        // reached by clearing a named node's domain (§3.1).
        let dir = tempfile::tempdir().unwrap();
        Node::init(dir.path(), None).unwrap();
        let node = Node::open(NodeConfig::loopback(dir.path())).await.unwrap();
        let _d = dir;
        assert!(node.origin().domain().is_none());
        assert!(node.resolving_domains().is_empty());

        node.set_domain("cluster.example").unwrap();
        assert_eq!(
            node.resolving_domains(),
            ["cluster.example"],
            "a node its zone does not name still resolves that zone"
        );

        let nas = SecretKey::generate().public();
        let set = MemberSet::from_records("cluster.example", &[rec("nas", &nas)]).unwrap();
        node.apply_member_set(&set, Duration::from_secs(300), now_ns())
            .unwrap();

        let origin = OriginId::named("nas", "cluster.example").unwrap();
        let now = now_ns();
        assert!(node.store().is_bound(&origin, &nas, now).unwrap());
        // And it expires, which is what a `--as` binding never did.
        let binding = node
            .store()
            .bindings_for_origin(&origin)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(binding.source, synch_store::BindingSource::Dns);
        assert!(binding.expires_at.is_some());

        // Leaving the zone takes its bindings with it, rather than leaving a
        // cluster this node just left dialable until their grace runs out.
        assert!(node.clear_domain().unwrap());
        assert!(node.resolving_domains().is_empty());
        assert!(!node.store().is_bound(&origin, &nas, now).unwrap());
    }

    #[tokio::test]
    async fn applying_a_member_set_writes_dns_bindings() {
        // §3.2: records bind, addr hints become dialable, expiry is ttl + grace.
        let (_d, node) = node().await;
        let nas = SecretKey::generate().public();
        let laptop = SecretKey::generate().public();
        let set = MemberSet::from_records(
            "cluster.example",
            &[
                format!("{} addr=127.0.0.1:5555", rec("nas", &nas)),
                rec("laptop", &laptop),
            ],
        )
        .unwrap();
        let refresh = node
            .apply_member_set(&set, Duration::from_secs(300), now_ns())
            .unwrap();
        assert_eq!(refresh.bindings, 2);
        assert!(refresh.ambiguous.is_empty());

        let origin = OriginId::named("nas", "cluster.example").unwrap();
        let now = now_ns();
        assert!(node.store().is_bound(&origin, &nas, now).unwrap());
        assert_eq!(node.peer_addr(&nas).unwrap().unwrap().ip_addrs().count(), 1);

        // Clearing the domain leaves the bindings until the next start (§3.1).
        node.clear_domain().unwrap();
        assert!(node.store().is_bound(&origin, &nas, now).unwrap());
        assert!(node
            .store()
            .is_bound(node.origin(), &node.node_id(), now)
            .unwrap());

        let far_future = now + Duration::from_secs(3600).as_nanos() as i64;
        assert!(!node.store().is_bound(&origin, &nas, far_future).unwrap());
    }

    #[tokio::test]
    async fn ambiguity_reaches_doctor_from_the_scheduled_path() {
        // §3.2/§3.4: scheduled-path findings are held for `doctor` to say —
        // ambiguous key and self-origin mismatch alike.
        let (_d, node) = node().await;
        let key = SecretKey::generate().public();
        let resolver = FakeResolver::new(
            vec![
                rec("nas", &key),
                rec("laptop", &key),
                rec("nas", &node.node_id()),
            ],
            MIN_TTL,
            None,
        );
        node.set_domain("cluster.example").unwrap();
        node.refresh_due_domains(&resolver, now_ns()).await.unwrap();
        assert!(!node.store().is_trusted_key(&key, now_ns()).unwrap());

        let report = node.doctor().unwrap();
        assert_eq!(report.ambiguous, vec![("cluster.example".to_string(), key)]);
        assert_eq!(
            report.self_origin_mismatch,
            vec![(
                "cluster.example".to_string(),
                OriginId::named("nas", "cluster.example").unwrap()
            )]
        );
    }

    /// `peers_seen.last_addr` overwrites and nothing prunes it, so a hint
    /// that lands is permanent; the `id=`-less record is the same gate (§3.2).
    #[tokio::test]
    async fn a_second_domain_cannot_repoint_a_key_the_first_one_vouches_for() {
        let shared = SecretKey::generate().public();
        let now = now_ns();

        for record in [
            rec("nas", &shared),
            format!("v=sync1 nk={}", shared.to_z32()),
        ] {
            let (_d, node) = node().await;
            // Domain A vouches for the key and supplies its address.
            node.apply_member_set(
                &MemberSet::from_records("a.example", &[format!("{record} addr=192.0.2.7:4433")])
                    .unwrap(),
                Duration::from_secs(300),
                now,
            )
            .unwrap();
            let first = node.peer_addr(&shared).unwrap().expect("A's hint applies");
            assert_eq!(first.ip_addrs().count(), 1);

            // Domain B names the same key and points it elsewhere.
            node.apply_member_set(
                &MemberSet::from_records(
                    "b.example",
                    &[format!(
                        "{record} addr=198.51.100.66:9999 relay=https://attacker.example"
                    )],
                )
                .unwrap(),
                Duration::from_secs(300),
                now,
            )
            .unwrap();

            // Untouched: two domains vouch for the key now, so neither's
            // dialing data is used.
            let after = node.peer_addr(&shared).unwrap().expect("A's hint stands");
            assert_eq!(
                after.ip_addrs().collect::<Vec<_>>(),
                first.ip_addrs().collect::<Vec<_>>(),
                "a second domain must not repoint a key the first one vouches for"
            );
            assert_eq!(
                after.relay_urls().count(),
                0,
                "and must not add a relay this node would then dial through"
            );

            // The two domains are two bindings now — what the gate needs.
            let mut domains: Vec<String> = node
                .store()
                .bindings_for_key(&shared)
                .unwrap()
                .iter()
                .filter_map(|b| b.domain.clone())
                .collect();
            domains.sort();
            assert_eq!(domains, ["a.example", "b.example"]);
        }
    }

    /// A `relay=` can never be supplied by `addr=`, and vice versa.
    #[test]
    fn a_dialing_hint_means_one_thing() {
        assert!(direct_address("192.0.2.7:4433").is_some());
        assert!(relay_url("https://relay.example./").is_some());
        assert!(relay_url("http://relay.example:8080").is_some());
        // Neither field accepts the other's shape.
        assert!(direct_address("https://relay.example./").is_none());
        assert!(relay_url("192.0.2.7:4433").is_none());
        // Un-dialable schemes, hostless URLs, bare hosts, port-less addresses.
        for bad in [
            "ftp://relay.example",
            "https://",
            "relay.example",
            "192.0.2.7",
            "",
        ] {
            assert!(direct_address(bad).is_none(), "{bad} is not an address");
            assert!(relay_url(bad).is_none(), "{bad} is not a relay");
        }
    }

    #[tokio::test]
    async fn a_clock_that_dates_nothing_refuses_to_extend_membership() {
        // Every expiry check is `now < expires_at`, so an undatable clock
        // honors every binding forever; refuse to extend trust, loudly (§3.2).
        let (_d, node) = node().await;
        let nas = SecretKey::generate().public();
        let set = MemberSet::from_records("cluster.example", &[rec("nas", &nas)]).unwrap();
        let refused = node
            .apply_member_set(&set, Duration::from_secs(300), 0)
            .expect_err("an undatable instant cannot extend trust");
        assert!(
            refused.to_string().contains("clock"),
            "the operator has to be told why: {refused}"
        );
        let origin = OriginId::named("nas", "cluster.example").unwrap();
        assert!(!node.store().is_bound(&origin, &nas, 0).unwrap());
        assert!(!node.store().is_trusted_key(&nas, 0).unwrap());

        // And the refusal is what `doctor` reports, not a silence.
        let report = node.doctor().unwrap();
        assert!(report.clock.trusted, "the test host's clock is fine");
        assert!(!node.store().clock_status(0).unwrap().trusted);
    }

    /// Judged against the whole keyspace, every foreign head on a confined
    /// node reads PARTIAL by construction; this node's own trie is the
    /// exception — it built that one, so it is judged whole (§5.5).
    #[tokio::test]
    async fn doctor_reports_the_basics_and_the_read_scope() {
        let (_d, node) = node().await;
        assert!(
            node.doctor().unwrap().local_scope.is_none(),
            "an undelegated node reads the whole keyspace"
        );
        let space = tempfile::tempdir().unwrap();
        node.add_filesystem_source("media", space.path()).unwrap();
        std::fs::write(space.path().join("a.txt"), b"hello").unwrap();
        node.scan_and_publish().unwrap();

        let report = node.doctor().unwrap();
        assert_eq!(report.origin, *node.origin());
        assert_eq!(report.heads.len(), 1);
        assert_eq!(report.heads[0].complete_seq, Some(1));
        assert!(report.heads[0].servable && report.heads[0].bound);
        assert_eq!(report.heads[0].entries, 1);
        assert!(report.equivocations.is_empty() && report.unbound_origins.is_empty());
        assert!(report.trie.nodes > 0);
        assert_eq!(report.blobs, (1, 1));

        // What a peer's `Hello` would have left behind on a delegated node.
        node.store()
            .set_read_scope(Some(&["photos".to_string()]))
            .unwrap();
        let report = node.doctor().unwrap();
        assert_eq!(report.local_scope, Some(vec!["photos".to_string()]));
        assert!(
            report.heads[0].servable,
            "this node's own head is held whole whatever it was granted to read"
        );
        node.shutdown().await.unwrap();
    }

    /// §12: a trust-withdrawn origin keeps its head and entries; doctor flags
    /// it until it republishes under a bound key.
    #[tokio::test]
    async fn doctor_surfaces_unbound_origins() {
        let (_d, node) = node().await;
        let peer = SecretKey::generate();
        let origin = OriginId::named("nas", "x.example").unwrap();
        node.store()
            .put_head(
                synch_store::Slot::Complete,
                &synch_core::SignedHead::sign(
                    &peer,
                    origin.clone(),
                    5,
                    synch_core::Hash([2u8; 32]),
                    0,
                ),
                0,
                0,
            )
            .unwrap();
        let report = node.doctor().unwrap();
        assert!(report.unbound_origins.iter().any(|(o, _)| o == &origin));
    }

    /// A resolver that answers from a fixed record set and counts its calls.
    #[derive(Debug)]
    struct FakeResolver {
        records: Mutex<Vec<String>>,
        ttl: Duration,
        calls: AtomicUsize,
        failing: AtomicBool,
        stall: Option<(String, Duration)>,
    }

    impl FakeResolver {
        fn new(
            records: Vec<String>,
            ttl: Duration,
            stall: Option<(String, Duration)>,
        ) -> FakeResolver {
            FakeResolver {
                records: Mutex::new(records),
                ttl,
                calls: AtomicUsize::new(0),
                failing: AtomicBool::new(false),
                stall,
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        fn fail(&self, failing: bool) {
            self.failing.store(failing, Ordering::SeqCst);
        }
    }

    impl MemberResolver for FakeResolver {
        fn resolve_members<'a>(&'a self, domain: &'a str) -> synch_net::dns::MemberSetFuture<'a> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let failing = self.failing.load(Ordering::SeqCst);
            let records = self
                .records
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            let ttl = self.ttl;
            let stall = self.stall.clone();
            Box::pin(async move {
                if let Some((dom, d)) = stall {
                    if domain == dom {
                        tokio::time::sleep(d).await;
                    }
                }
                if failing {
                    return Err(synch_net::NetError::Dns("resolver is down".into()));
                }
                Ok((MemberSet::from_records(domain, &records)?, ttl))
            })
        }
    }

    #[tokio::test]
    async fn bindings_survive_past_their_ttl_while_the_resolver_answers() {
        // §3.2: a failing resolver keeps the cached bindings, never shrinking the set.
        let (_d, node) = node().await;
        let nas = SecretKey::generate().public();
        let resolver = FakeResolver::new(vec![rec("nas", &nas)], MIN_TTL, None);
        node.set_domain("cluster.example").unwrap();

        let start = now_ns();
        let origin = OriginId::named("nas", "cluster.example").unwrap();
        assert_eq!(
            node.refresh_due_domains(&resolver, start)
                .await
                .unwrap()
                .len(),
            1,
            "a domain that has never resolved is due at once"
        );
        assert!(node.store().is_bound(&origin, &nas, start).unwrap());
        assert!(node
            .refresh_due_domains(&resolver, start + 1_000)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(resolver.calls(), 1);

        // TTL ticks over while the resolver is down: the failure is an outcome
        // and a health entry, and the cached binding keeps its own expiry.
        let ttl = MIN_TTL.as_nanos() as i64;
        resolver.fail(true);
        let later = start + ttl + 1;
        let outcomes = node.refresh_due_domains(&resolver, later).await.unwrap();
        assert_eq!(outcomes.len(), 1);
        assert!(outcomes[0].result.is_err(), "{outcomes:?}");
        let health = node.domain_health().unwrap();
        let schedule = health[0].schedule.as_ref().unwrap();
        assert!(schedule.last_error.is_some(), "{schedule:?}");
        assert!(schedule.last_success > 0, "the first refresh succeeded");
        assert!(
            node.store().is_bound(&origin, &nas, later).unwrap(),
            "the cached binding keeps its own expiry"
        );
        assert_eq!(resolver.calls(), 2);
        assert!(node
            .refresh_due_domains(&resolver, later + 1)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            resolver.calls(),
            2,
            "the retry waits out a clamped TTL rather than spinning"
        );

        // Recovered, the next due refresh renews past the maintenance sweep.
        resolver.fail(false);
        let again = later + ttl;
        assert_eq!(
            node.refresh_due_domains(&resolver, again)
                .await
                .unwrap()
                .len(),
            1
        );
        let horizon = again + 1;
        assert!(node.store().is_bound(&origin, &nas, horizon).unwrap());
        node.maintenance_pass().unwrap();
        assert!(node.store().is_bound(&origin, &nas, horizon).unwrap());
    }

    #[tokio::test]
    async fn the_unknown_key_trigger_fires_once_per_cooldown() {
        // §3.4: an unknown-key inbound triggers immediate re-resolution,
        // rate-limited against a peer that keeps retrying.
        let (_d, node) = node().await;
        let nas = SecretKey::generate().public();
        let resolver = FakeResolver::new(vec![rec("nas", &nas)], Duration::from_secs(3600), None);
        node.set_domain("cluster.example").unwrap();

        let now = now_ns();
        assert_eq!(
            node.refresh_triggered(&resolver, now).await.unwrap().len(),
            1
        );
        assert_eq!(resolver.calls(), 1);
        // Every further trigger inside the cooldown is dropped.
        for _ in 0..2 {
            assert!(node
                .refresh_triggered(&resolver, now)
                .await
                .unwrap()
                .is_empty());
        }
        assert_eq!(resolver.calls(), 1);

        // Past the cooldown it fires again, even though the TTL has hours left.
        let past = now + DNS_TRIGGER_COOLDOWN.as_nanos() as i64 + 1;
        assert_eq!(
            node.refresh_triggered(&resolver, past).await.unwrap().len(),
            1
        );
        assert_eq!(resolver.calls(), 2);
    }

    #[tokio::test]
    async fn the_dns_loop_stops_on_shutdown_and_refreshes_on_the_bell() {
        let (_d, node) = node().await;
        let nas = SecretKey::generate().public();
        let resolver = Arc::new(FakeResolver::new(
            vec![rec("nas", &nas)],
            Duration::from_secs(3600),
            None,
        ));
        node.set_domain("cluster.example").unwrap();

        let (tx, rx) = tokio::sync::oneshot::channel();
        let node_task = node.clone();
        let resolver_task = resolver.clone();
        let handle = tokio::spawn(async move {
            node_task
                .run_dns(resolver_task.as_ref(), async {
                    let _ = rx.await;
                })
                .await;
        });

        // The first pass is due immediately; give it a generous window.
        assert!(
            crate::testkit::eventually(|| resolver.calls() > 0).await,
            "the loop resolves what is due"
        );
        let origin = OriginId::named("nas", "cluster.example").unwrap();
        assert!(node.store().is_bound(&origin, &nas, now_ns()).unwrap());

        tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(10), handle)
            .await
            .expect("the loop must stop promptly")
            .unwrap();
    }

    #[tokio::test]
    async fn one_stalling_domain_does_not_spend_the_whole_pass() {
        let (_d, node) = node().await;
        let nas = SecretKey::generate().public();
        let resolver = FakeResolver::new(
            vec![rec("nas", &nas)],
            MIN_TTL,
            Some(("slow.example".to_string(), Duration::from_secs(86_400))),
        );
        let started = tokio::time::Instant::now();
        let outcomes = node
            .refresh_these(
                &resolver,
                &["slow.example".to_string(), "fast.example".to_string()],
                now_ns(),
            )
            .await
            .expect("the pass itself must not fail");

        // The stalling domain is reported as a failure and named.
        assert_eq!(outcomes.len(), 2);
        let slow = outcomes
            .iter()
            .find(|o| o.domain == "slow.example")
            .unwrap();
        let why = slow.result.as_ref().expect_err("it never answered");
        assert!(
            why.contains("slow.example") && why.contains("abandoned"),
            "{why}"
        );

        // The domain behind it in the queue was still resolved, and the pass
        // ended on the deadline, not on the stall.
        let fast = outcomes
            .iter()
            .find(|o| o.domain == "fast.example")
            .unwrap();
        assert!(fast.result.is_ok(), "{:?}", fast.result);
        assert!(started.elapsed() < REFRESH_DEADLINE * 2);
    }
}
