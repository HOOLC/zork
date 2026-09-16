//! Node configuration and the tunables the design names (§5.3, §5.4, §6.3, §7.1).

use std::{path::PathBuf, time::Duration};

use synch_net::NetOptions;

use crate::error::{EngineError, Result};

/// How many distinct CAS objects a replica fetches concurrently by default.
pub const DEFAULT_REPLICA_CONCURRENCY: usize = 16;

/// Largest replica fetch concurrency accepted from a host configuration.
///
/// Each slot may fan out to several providers and the ready-candidate window is
/// a multiple of this value, so an unbounded operator value is an allocation
/// and connection storm rather than useful throughput.
pub const MAX_REPLICA_CONCURRENCY: usize = 256;

const _: () = assert!(DEFAULT_REPLICA_CONCURRENCY <= MAX_REPLICA_CONCURRENCY);

/// Where a node's data directory lives by default (§10).
pub fn default_data_dir() -> Result<PathBuf> {
    directories::ProjectDirs::from("", "", "synchronicity")
        .map(|dirs| dirs.data_dir().to_path_buf())
        .ok_or_else(|| EngineError::invalid("no platform data directory is available"))
}

/// How a node is configured.
#[derive(Debug, Clone)]
pub struct NodeConfig {
    /// The data directory holding the database and the CAS.
    pub data_dir: PathBuf,
    /// How many socket worker threads to run (`docs/SOCKETS.md` §5.1).
    ///
    /// Dedicated OS threads, because async-ebpf's `Program` is neither `Send`
    /// nor `Sync` — a guest suspends inside a signal handler — so an
    /// invocation is placed on a worker and stays there.
    ///
    /// **Zero declines the capability outright**: no pool, no SSH host key
    /// minted, and — because the socket ALPN is mounted only where a pool
    /// exists — nothing advertised for a peer to dial. That is a different
    /// statement from "run sockets with no worker to run them on", which is
    /// why zero is not rounded up to one: a host that means to serve sockets
    /// and asks for none would get a hang, while a host that means not to
    /// serve them at all had no way to say so. An embedder hosting many nodes
    /// in one process pays this per node (`docs/CLOUD-DATAPLANE.md` §4.4).
    pub socket_workers: usize,
    /// OpenDAL cloud CAS settings. `None` selects the local filesystem CAS.
    pub cloud: Option<synch_store::cloud::CloudConfig>,
    /// Who checkpoints this node's write-ahead log.
    ///
    /// The default leaves it to SQLite. An embedder replicating the database
    /// itself takes it over, so that no frame is recycled before it has been
    /// shipped (`docs/CLOUD-DATAPLANE.md` §5.3) — and then owes the WAL a
    /// checkpoint, because nothing else will run one.
    pub checkpointing: synch_store::Checkpointing,
    /// How the endpoint is bound.
    pub net: NetOptions,
    /// The base anti-entropy round interval (§5.3, default 30 s with ±50 %
    /// jitter).
    pub aae_interval: Duration,
    /// The backstop interval between checkout passes (§7.2, default 60 s with
    /// ±50 % jitter). Passes normally run because the unified tree changed
    /// and rang the bell; the interval is only for drift nobody rang about —
    /// a `chmod` moves nothing in role state, and only a pass
    /// repairs it.
    pub checkout_interval: Duration,
    /// How long a scanner rescan waits after a watcher hint (§7.1).
    pub watch_debounce: Duration,
    /// Full rescan interval (§7.1).
    pub scan_interval: Duration,
    /// How long staging must go quiet before the publisher turns a batch into
    /// one new signed root (§7.1).
    pub publish_quiesce: Duration,
    /// How many staged entries force a batch out without waiting (§7.1).
    pub publish_batch_max: usize,
    /// Minimum interval between `BlobAd` republishes for one in-flight object
    /// (§6.3).
    pub ad_update_interval: Duration,
    /// How many providers a single range fetch is split across (§6.4).
    pub fetch_fanout: usize,
    /// The backstop interval between replication sweeps (`docs/REPLICATION.md`
    /// §5, default 300 s with ±50 % jitter). Like the checkout interval, passes
    /// normally run because the unified tree changed and rang the bell.
    pub replica_interval: Duration,
    /// How many other origins must advertise a complete copy before a replica
    /// lets a stale root of its own go (`docs/REPLICATION.md` §4.3).
    ///
    /// One by default: a replica will not be the last holder to let go of
    /// something, but it does not try to enforce a cluster-wide floor either —
    /// that is the deferred half of §4.3, and the hazard is written down there.
    /// Zero disables the brake, which is the right setting for a lone replica
    /// whose peers never advertise anything it holds.
    pub replica_release_floor: i64,
    /// How many objects a replica fetches at once — concurrently, which is
    /// what makes this a rate limit rather than a batch size.
    ///
    /// Bounded because replica fetches share the endpoint with anti-entropy and
    /// foreground reads, and nothing schedules between them today (§13). The
    /// shipped default is [`DEFAULT_REPLICA_CONCURRENCY`], and the accepted
    /// range is 1 through [`MAX_REPLICA_CONCURRENCY`].
    pub replica_concurrency: usize,
    /// The smallest object a fetch will run the delta descent for
    /// (`docs/DELTA-SYNC.md` §4, default 16 MiB — one ad span).
    ///
    /// Below it the proof round trips cost more than the bytes they could
    /// save, and inline blobs (§6.2) never delta at all. Setting it to
    /// [`u64::MAX`] turns the descent off for this node, which is the escape
    /// hatch for diagnosing a fetch that is behaving strangely — code-level,
    /// like [`NodeConfig::fetch_fanout`]: no shipped binary reads it from a
    /// file.
    pub delta_min_size: u64,
    /// How long a pending head may sit with an incomplete trie before the
    /// maintenance pass abandons it (§5.2).
    ///
    /// A head reaches the pending slot by reactive push (§5.3) long before its
    /// trie does, and `head_floor` is the best of both slots — so while it sits
    /// there the node refuses a peer's older but *servable* head for that
    /// origin, and materializes nothing for it. If the publisher goes offline
    /// between the push and the fetch, nobody can serve the pending trie and
    /// nothing else can be adopted in its place. Clearing it drops the floor
    /// and head selection re-runs.
    ///
    /// Thirty anti-entropy intervals at the defaults: long enough that every
    /// peer holding the trie has had many rounds to serve it, short enough that
    /// an origin is not stranded for an afternoon.
    pub pending_head_ttl: Duration,
    /// How long an explicit anti-entropy exchange with one peer may take (§5.3).
    ///
    /// `synch-net` deadlines every individual request, but a round issues as
    /// many as the peer's answers call for — one trie fetch per head it hands
    /// back, each of those looping until it stops making progress — so the
    /// per-request deadlines bound no exchange. Explicit operator-driven syncs
    /// get this full budget; the standing periodic scheduler applies a smaller
    /// per-peer cap so one bad member cannot postpone trying the next candidate.
    ///
    /// Ten anti-entropy intervals at the defaults: far more than a real round
    /// needs even on a cold bootstrap, since a round that runs out of budget
    /// resumes from everything it already committed.
    pub sync_round_budget: Duration,
    /// How long old roots are retained (§5.4).
    pub root_retention: Duration,
    /// How long this node's own tombstones are kept before a later root drops
    /// them (§4.2, default 90 days).
    pub tombstone_ttl: Duration,
    /// How long `synch recover` collects peer summaries before it lifts the
    /// publishing floor (§3.4).
    pub recovery_quiesce: Duration,
    /// How far above the highest seq peers advertised publishing resumes after
    /// recovery (§3.4).
    pub seq_gap: u64,
    /// The human-friendly node name published in `m:self`.
    pub name: String,
    /// How the DNSSEC resolver reaches the DNS: an optional DoH endpoint and
    /// an optional root-trust-anchor override (§3.2).
    pub dns: synch_net::ResolverOptions,
}

impl NodeConfig {
    /// Builds a configuration rooted at `data_dir` with the design defaults.
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        NodeConfig {
            data_dir: data_dir.into(),
            socket_workers: std::thread::available_parallelism()
                .map(|n| n.get().min(4))
                .unwrap_or(1),
            cloud: None,
            checkpointing: synch_store::Checkpointing::default(),
            net: NetOptions::default(),
            aae_interval: Duration::from_secs(30),
            checkout_interval: Duration::from_secs(60),
            watch_debounce: Duration::from_millis(500),
            scan_interval: Duration::from_secs(3600),
            publish_quiesce: crate::publisher::DEFAULT_PUBLISH_QUIESCE,
            publish_batch_max: crate::publisher::DEFAULT_PUBLISH_BATCH_MAX,
            ad_update_interval: Duration::from_secs(60),
            fetch_fanout: 3,
            replica_interval: Duration::from_secs(300),
            replica_concurrency: DEFAULT_REPLICA_CONCURRENCY,
            replica_release_floor: 1,
            delta_min_size: synch_core::AD_SPAN_GRANULARITY,
            pending_head_ttl: Duration::from_secs(900),
            sync_round_budget: Duration::from_secs(300),
            root_retention: Duration::from_secs(7 * 24 * 3600),
            tombstone_ttl: Duration::from_secs(90 * 24 * 3600),
            recovery_quiesce: crate::recovery::DEFAULT_RECOVERY_QUIESCE,
            seq_gap: crate::recovery::DEFAULT_SEQ_GAP,
            name: hostname(),
            dns: synch_net::ResolverOptions::default(),
        }
    }

    /// Builds a configuration for a loopback-only, fully offline node.
    pub fn loopback(data_dir: impl Into<PathBuf>) -> Self {
        NodeConfig {
            net: NetOptions::loopback(),
            ..NodeConfig::new(data_dir)
        }
    }
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "synchronicity".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_node_fetches_sixteen_replica_objects_by_default() {
        assert_eq!(
            NodeConfig::new("unused").replica_concurrency,
            DEFAULT_REPLICA_CONCURRENCY
        );
        assert_eq!(DEFAULT_REPLICA_CONCURRENCY, 16);
    }
}
