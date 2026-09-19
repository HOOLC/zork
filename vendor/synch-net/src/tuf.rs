//! TUF-driven pin refresh (docs/REKOR-ZONE-KEY.md §10).
//!
//! Sigstore rotates its tiled logs — a new shard, a new key, roughly yearly
//! — and eventually removes compromised keys from its trust root, each event
//! a client upgrade under a build-time snapshot. This module makes the pin
//! set follow Sigstore's TUF repository instead: **the client walks the
//! official repository itself and verifies what it collected here, against
//! an embedded TUF root.**
//!
//! Going to the source is the simpler arrangement, and it is available
//! because of what TUF metadata *is*: every byte chains to the root role, so
//! nothing between the repository and this module is trusted with anything —
//! the CDN, the TLS, a caching mirror. A hostile transport can deny this
//! fetch; it cannot make it mean anything.
//!
//! # The two rules that are load-bearing
//!
//! Both come straight from §10.2, and both are about availability, so
//! neither is a detail an implementation gets to soften:
//!
//! 1. **Expiry gates updates, never operation.** An unreachable repository,
//!    or stale or invalid material from one, is ignored and the current pins
//!    stand. To *change* pins the chain must be valid and unexpired; to keep
//!    working, nothing is required. No error out of this module ever fails a
//!    membership refresh.
//! 2. **Monotonicity bounds hostile mirrors.** A mirror can serve
//!    old-but-valid material, but it cannot roll a client's persisted
//!    versions back, and a freeze holds only until the served timestamp
//!    expires.
//!
//! On acceptance the pin set becomes the tlogs of the new `trusted_root` —
//! **replacing** the previous set, never unioning, so a key Sigstore removes
//! is a key clients drop.
//!
//! **That is a statement about `update`, not a revocation story, and the
//! difference matters.** Sigstore retires a shard by *window* — the shipped
//! trusted root lists `rekor.sigstore.dev`'s 2021 P-256 key with a start and
//! no end — and `tlog_keys` deliberately pins every shard the root lists,
//! retired ones included, because an archival proof from a closed shard is
//! still a proof. `validFor.end` is not enforced anywhere and cannot be:
//! nothing near a proof carries a log-attested time, `integratedTime` sits
//! outside the Merkle commitment, and a clock the reader supplies would refuse
//! exactly the archival entries the design exists to keep readable. So a shard
//! key that has ever been listed is unrevocable here, and a leaked one is
//! unrecoverable short of Sigstore removing the entry from the root outright.
//! Saying that plainly is better than a claim the shipped root contradicts.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    time::{Duration, Instant},
};

use crate::{
    pubkey::{RawKey, Scheme},
    rekor::{base64_decode, base64_encode, sha256, LogKey, LogKeys},
};

/// The target the chain has to authenticate for any of this to matter.
pub const TRUSTED_ROOT_TARGET: &str = "trusted_root.json";

/// The `signed._type` each role must declare.
const ROOT_ROLE: &str = "root";
const TIMESTAMP_ROLE: &str = "timestamp";
const SNAPSHOT_ROLE: &str = "snapshot";
const TARGETS_ROLE: &str = "targets";

/// The metadata file names the chain walks through.
const SNAPSHOT_META: &str = "snapshot.json";
const TARGETS_META: &str = "targets.json";

/// The Sigstore TUF root role this build pins — the ultimate anchor of the
/// pin set, the same standing as the ICANN trust anchor (§10.2).
///
/// `root.json` version 15, `expires` 2026-11-20T13:58:18Z, fetched
/// 2026-08-16 from `https://tuf-repo-cdn.sigstore.dev/15.root.json`. Its own
/// five root-role signatures verify under both version 14's and its own root
/// role, which is the check [`update`] applies to every later root; nothing
/// here is trusted because it was fetched, only because it is the byte
/// sequence this build ships.
///
/// Replacing it is a build, deliberately: everything else about the pin set
/// now refreshes on its own, and a root-level incident is precisely the
/// event that should still cost an upgrade (§10.4).
pub const EMBEDDED_TUF_ROOT: &str = include_str!("sigstore_tuf_root.json");

/// The Sigstore `trusted_root.json` this build boots from — the **bootstrap**
/// pin set and log directory, not the last word.
///
/// The consistent-snapshot target `6494e21e…`, its SHA-256 checked against
/// the signed `targets.json`; fetched 2026-08-16 from
/// `tuf-repo-cdn.sigstore.dev`. It is the signed artifact itself rather than
/// keys copied out of it, so which logs exist, where they are served and
/// which of them is currently in service are all read from one place — here
/// until a walk verifies, and from that walk's trusted root afterwards.
/// Nothing about a Sigstore log rotation reaches this file: only a root-level
/// incident does, and that is [`EMBEDDED_TUF_ROOT`]'s business.
pub const EMBEDDED_TRUSTED_ROOT: &str = include_str!("sigstore_trusted_root.json");

/// Why a TUF update was refused.
///
/// The variants are the failure *classes* `synch doctor` explains. None of
/// them fails a membership refresh — every one of them means "keep the
/// current pins" (§10.2) — so they exist to be *reported*, not to propagate.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TufError {
    /// The repository could not be read, or a file it served is not the JSON
    /// shape TUF metadata has.
    #[error("malformed: {0}")]
    Malformed(String),
    /// The root chain does not connect: a gap in the versions, a role the
    /// root does not define, a file claiming the wrong role.
    #[error("chain: {0}")]
    Chain(String),
    /// Too few of a role's keys signed: the material is not endorsed by the
    /// quorum the root demands.
    #[error("threshold: {0}")]
    Threshold(String),
    /// A signature does not verify over the canonical JSON of what it
    /// covers, or the key it names cannot be parsed at all.
    #[error("signature: {0}")]
    Signature(String),
    /// A role's `expires` is in the past. Gates the *update*, never the
    /// pins already in force.
    #[error("expiry: {0}")]
    Expiry(String),
    /// A version is lower than the one already accepted — the freeze/rollback
    /// attack monotonicity exists to bound.
    #[error("rollback: {0}")]
    Rollback(String),
    /// A file does not hash to what the metadata above it says it does.
    #[error("target hash: {0}")]
    TargetHash(String),
}

impl TufError {
    /// The one-word class, for logs and `synch doctor` copy.
    pub fn class(&self) -> &'static str {
        match self {
            TufError::Malformed(_) => "malformed",
            TufError::Chain(_) => "chain",
            TufError::Threshold(_) => "threshold",
            TufError::Signature(_) => "signature",
            TufError::Expiry(_) => "expiry",
            TufError::Rollback(_) => "rollback",
            TufError::TargetHash(_) => "target-hash",
        }
    }
}

/// One walk of a TUF repository: the files, verbatim, in chain order.
///
/// Collected by [`fetch_metadata`] and checked by [`update`] — the split is
/// deliberate, and the naming follows it: everything here is bytes somebody
/// else served, held together in the order the verifier reads them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TufMetadata {
    /// `root.json` at ascending versions, so a client embedded at version N
    /// can walk to the current one.
    pub roots: Vec<Vec<u8>>,
    /// `timestamp.json`, verbatim.
    pub timestamp: Vec<u8>,
    /// `snapshot.json`, verbatim.
    pub snapshot: Vec<u8>,
    /// `targets.json`, verbatim.
    pub targets: Vec<u8>,
    /// `trusted_root.json` — the target the chain authenticates.
    pub trusted_root: Vec<u8>,
}

/// The serialized format of the pin state.
///
/// The version is what makes the file's *contents* a contract rather than a
/// hint: a config value written to any other format is not read at all. A client that
/// meets one starts from the embedded bootstrap and re-learns on its next
/// walk, which costs one refresh and concedes nothing.
const STATE_FORMAT_VERSION: u64 = 4;

/// The pin set a client is running on, and where it came from (§10.2).
///
/// Persisted in the SQLite `config` row `rekor.pin_state`, global across
/// domains, so it is replicated with the rest of the serverless database.
/// Global is the point — the pin set is a property of Sigstore
/// and not of any domain being resolved, so every domain shares one floor
/// and a hostile mirror gets one client's versions to walk back, not one per
/// domain it is asked about.
///
/// The serialized format is versioned; a value this build cannot read is
/// ignored rather than trusted, which lands on the bootstrap pins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinState {
    /// The accepted `root.json`, verbatim — the next update chains from it.
    ///
    /// Read as written. The data directory is the owner's own (§9.3), so
    /// this is material this client persisted for itself, not input.
    pub root: Vec<u8>,
    /// Every root accepted *beyond* the embedded one, in ascending version
    /// order. Empty means the embedded root is still what is in force.
    ///
    /// Kept so a later [`update`] can hand the whole lineage back to a peer
    /// that needs it, and so the accepted roots survive a restart.
    pub root_chain: Vec<Vec<u8>>,
    /// Its version.
    pub root_version: u64,
    /// The accepted `timestamp.json` version.
    pub timestamp_version: u64,
    /// The accepted `snapshot.json` version.
    pub snapshot_version: u64,
    /// The accepted `targets.json` version.
    pub targets_version: u64,
    /// The accepted `targets.json`, verbatim — the role that *names* the
    /// trusted root, kept beside it so the pair can be re-checked on load.
    ///
    /// Kept because [`update`] needs the accepted version to enforce the
    /// targets rollback floor, and because a `trusted_root` with no
    /// `targets.json` beside it cannot be re-checked against a live walk.
    pub targets: Vec<u8>,
    /// The accepted `trusted_root.json`, verbatim — the pin set is derived
    /// from it rather than stored beside it, so the two cannot disagree.
    pub trusted_root: Vec<u8>,
    /// When this state was written, seconds since the epoch.
    ///
    /// A monotonic floor for the clock the expiry checks read, bounded against
    /// the real clock by the resolver so a value from the file cannot become
    /// an unbounded one (see `crate::dns`).
    pub updated_at: u64,
    /// SHA-256 of the TUF root this state was accumulated under.
    ///
    /// Not a security check — the file is this client's own (§9.3). It
    /// answers "is this state even about my repository", which a `--tuf-root`
    /// pointed somewhere new makes false. [`update`] chains from `root`
    /// rather than re-walking from the binary's anchor, so a state carried
    /// across that switch would keep extending the wrong repository's chain
    /// and never self-correct. One digest comparison ends that.
    pub anchor_digest: [u8; 32],
}

impl PinState {
    /// The state a build starts from: the embedded root, nothing accepted.
    ///
    /// `trusted_root` is empty here: a client that has never completed an
    /// update runs on [`EMBEDDED_TRUSTED_ROOT`], the bootstrap snapshot.
    pub fn embedded() -> PinState {
        PinState::anchored(EMBEDDED_TUF_ROOT.as_bytes())
    }

    /// The starting state for a caller-supplied root — what
    /// [`PinState::embedded`] is for the built-in one.
    pub fn anchored(anchor: &[u8]) -> PinState {
        PinState {
            root: anchor.to_vec(),
            root_chain: Vec::new(),
            root_version: root_version(anchor).unwrap_or(0),
            timestamp_version: 0,
            snapshot_version: 0,
            targets_version: 0,
            targets: Vec::new(),
            trusted_root: Vec::new(),
            updated_at: 0,
            anchor_digest: crate::rekor::sha256(anchor),
        }
    }

    /// The pin set this state implies (§10.2's resolution order, second
    /// step): the tlogs of the accepted trusted root, or `None` when no
    /// update has ever been accepted and the bootstrap set stands.
    pub fn log_keys(&self) -> Option<LogKeys> {
        match self.trusted_root.is_empty() {
            true => None,
            // A `None` here sends the caller back to the embedded bootstrap set,
            // so warn when an accepted trusted root cannot be parsed.
            false => match tlog_keys(&self.trusted_root) {
                Ok(keys) => Some(keys),
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "the accepted trusted root does not parse; \
                         falling back to the embedded pin set"
                    );
                    None
                }
            },
        }
    }

    /// The trusted root in force: the accepted one, or the embedded
    /// bootstrap snapshot when no update has ever been accepted.
    pub fn trusted_root_in_force(&self) -> &[u8] {
        match self.trusted_root.is_empty() {
            true => EMBEDDED_TRUSTED_ROOT.as_bytes(),
            false => &self.trusted_root,
        }
    }

    /// The transparency logs it names — where they are, not only their keys.
    ///
    /// This is what makes the endpoint follow Sigstore too: a reader asks
    /// its pin state which log to read rather than carrying a hostname a
    /// rotation will invalidate.
    pub fn tlogs(&self) -> Result<Vec<Tlog>, TufError> {
        tlogs(self.trusted_root_in_force())
    }

    /// Reads persisted state, returning `None` when the file is absent or
    /// unreadable as state.
    ///
    /// Unreadable is not an error worth failing on: the pin set falls back
    /// to the bootstrap snapshot and the next valid walk rewrites the
    /// file. A client that refused to start over a corrupt cache would be
    /// exactly the availability coupling §10.2 forbids.
    pub fn load(path: &Path) -> Option<PinState> {
        PinState::load_anchored(path, EMBEDDED_TUF_ROOT.as_bytes())
    }

    /// The same, against a caller-supplied root of trust.
    ///
    /// Production always anchors at [`EMBEDDED_TUF_ROOT`] — that is what
    /// [`PinState::load`] is. This form exists for a deployment running its
    /// own TUF repository, and for the test harness, which necessarily
    /// anchors at a root it minted.
    ///
    /// The anchor is used to answer one question — whether this state was
    /// accumulated under the repository this build is pointed at — and not
    /// to authenticate the file's contents. The data directory is the
    /// owner's own (§9.3); a writer inside it can already write bindings
    /// straight into `synch.db`, so re-deriving the pin set from the binary
    /// on every load bought nothing that access did not already concede.
    pub fn load_anchored(path: &Path, anchor: &[u8]) -> Option<PinState> {
        let text = std::fs::read_to_string(path).ok()?;
        Self::decode_anchored(&text, anchor)
    }

    /// Decodes persisted JSON against the selected TUF root anchor.
    pub(crate) fn decode_anchored(text: &str, anchor: &[u8]) -> Option<PinState> {
        let value: serde_json::Value = serde_json::from_str(text).ok()?;
        // A file this build cannot read is not an error: the pin set falls
        // back to the bootstrap snapshot and the next accepted update
        // rewrites it, which is the safe direction.
        if value["version"].as_u64() != Some(STATE_FORMAT_VERSION) {
            return None;
        }
        let blob = |key: &str| -> Option<Vec<u8>> { base64_decode(value[key].as_str()?).ok() };
        let number = |key: &str| value[key].as_u64();
        let root_chain: Vec<Vec<u8>> = value["root_chain"]
            .as_array()?
            .iter()
            .map(|entry| base64_decode(entry.as_str()?).ok())
            .collect::<Option<_>>()?;
        let stored_digest: [u8; 32] = blob("anchor_digest")?.try_into().ok()?;
        let state = PinState {
            root: blob("root")?,
            root_chain,
            root_version: number("root_version")?,
            timestamp_version: number("timestamp_version")?,
            snapshot_version: number("snapshot_version")?,
            targets_version: number("targets_version")?,
            targets: blob("targets")?,
            trusted_root: blob("trusted_root")?,
            updated_at: number("updated_at").unwrap_or(0),
            anchor_digest: stored_digest,
        };

        // The one thing the file is not taken at its word about, and it is a
        // provenance question rather than a trust one: `update` chains from
        // `state.root` rather than re-walking from the binary's anchor, so a
        // state accumulated under a different `--tuf-root` would keep
        // extending that repository's chain forever. Falling back to the
        // bootstrap costs one refresh and lands on the right repository.
        (state.anchor_digest == crate::rekor::sha256(anchor)).then_some(state)
    }

    /// Encodes the versioned persisted JSON representation.
    pub fn encode(&self) -> String {
        serde_json::json!({
            "version": STATE_FORMAT_VERSION,
            "root": base64_encode(&self.root),
            "root_chain": self
                .root_chain
                .iter()
                .map(|root| base64_encode(root))
                .collect::<Vec<_>>(),
            "root_version": self.root_version,
            "timestamp_version": self.timestamp_version,
            "snapshot_version": self.snapshot_version,
            "targets_version": self.targets_version,
            "targets": base64_encode(&self.targets),
            "trusted_root": base64_encode(&self.trusted_root),
            "updated_at": self.updated_at,
            "anchor_digest": base64_encode(&self.anchor_digest),
        })
        .to_string()
    }

    /// Writes the state at mode 0600, replacing whatever was there.
    ///
    /// The pin set is not a secret; 0600 is the same hygiene the rest of the
    /// data directory gets (§9.3), which is where the trust in these bytes
    /// comes from in the first place.
    ///
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let text = self.encode();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // A plain write leaves the state half-formed for as long as it takes,
        // and a reader catching it there gets a file that does not parse and
        // is treated as no state at all — so the sibling-and-rename ritual,
        // with the mode carried before the bytes land.
        synch_core::fs::write_atomic_owner_only(path, text.as_bytes())
    }
}

/// What an accepted update established, for logs and `synch doctor`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TufUpdate {
    /// The state to persist and run on.
    pub state: PinState,
    /// The pin set the new trusted root names — replacing, never unioning.
    pub log_keys: LogKeys,
    /// Whether anything actually moved. Material that re-states what is
    /// already accepted is valid and boring; saying so keeps the logs quiet.
    pub changed: bool,
}

/// Verifies collected metadata and, if it is newer, returns the state to
/// adopt (§10.2).
///
/// `now` is seconds since the epoch — supplied rather than read so a
/// conformance fixture can be verified at the moment it was fetched, which
/// is the only way checked-in real metadata stays checkable.
///
/// The order is TUF's own: chain the roots, then timestamp → snapshot →
/// targets → the target itself, each step endorsed by the role the *current*
/// root names and bounded by the version the state already accepted.
pub fn update(metadata: &TufMetadata, state: &PinState, now: u64) -> Result<TufUpdate, TufError> {
    let mut trusted = Root::parse(&state.root)?;
    // Every root this update accepts is appended, so the state it produces
    // can be re-walked from the embedded root when it is next loaded.
    let mut chain = state.root_chain.clone();

    // 1. Walk the root chain. Each step must be signed by the thresholds of
    //    *both* the old root and the new one: the old root says who may
    //    succeed it, the new one proves it holds the keys it claims.
    for bytes in &metadata.roots {
        let candidate = Root::parse(bytes)?;
        if candidate.version <= trusted.version {
            // Material for a root this client already passed. Old-but-valid
            // is allowed to travel; it just does not move anything.
            continue;
        }
        if candidate.version != trusted.version + 1 {
            return Err(TufError::Chain(format!(
                "root {} follows root {}, and nothing bridges them",
                candidate.version, trusted.version
            )));
        }
        trusted.check_role(ROOT_ROLE, &candidate.meta)?;
        candidate.check_role(ROOT_ROLE, &candidate.meta)?;
        chain.push(candidate.bytes.clone());
        trusted = candidate;
    }
    if trusted.version < state.root_version {
        return Err(TufError::Rollback(format!(
            "root {} is older than the accepted root {}",
            trusted.version, state.root_version
        )));
    }
    // Only the *final* root's expiry is checked (TUF client workflow
    // §5.3.11). Intermediates in a chain are expected to be expired — the
    // real Sigstore chain has been, every time a rotation ran late.
    trusted.check_expiry(now)?;

    // 2. timestamp → snapshot → targets, each signed by the role the
    //    current root names, each no older than what is already accepted.
    let timestamp = Meta::parse(&metadata.timestamp, TIMESTAMP_ROLE)?;
    trusted.check_role(TIMESTAMP_ROLE, &timestamp)?;
    timestamp.check_expiry(now)?;
    timestamp.check_rollback(state.timestamp_version)?;

    let snapshot = Meta::parse(&metadata.snapshot, SNAPSHOT_ROLE)?;
    timestamp.check_listed(SNAPSHOT_META, &metadata.snapshot, snapshot.version)?;
    trusted.check_role(SNAPSHOT_ROLE, &snapshot)?;
    snapshot.check_expiry(now)?;
    snapshot.check_rollback(state.snapshot_version)?;

    let targets = Meta::parse(&metadata.targets, TARGETS_ROLE)?;
    snapshot.check_listed(TARGETS_META, &metadata.targets, targets.version)?;
    trusted.check_role(TARGETS_ROLE, &targets)?;
    targets.check_expiry(now)?;
    targets.check_rollback(state.targets_version)?;

    // 3. The target the whole chain exists to authenticate.
    targets.check_target(TRUSTED_ROOT_TARGET, &metadata.trusted_root)?;
    let log_keys = tlog_keys(&metadata.trusted_root)?;

    let changed = trusted.version != state.root_version
        || timestamp.version != state.timestamp_version
        || snapshot.version != state.snapshot_version
        || targets.version != state.targets_version
        || metadata.trusted_root != state.trusted_root;

    Ok(TufUpdate {
        state: PinState {
            root: trusted.bytes,
            root_chain: chain,
            root_version: trusted.version,
            timestamp_version: timestamp.version,
            snapshot_version: snapshot.version,
            targets_version: targets.version,
            targets: metadata.targets.clone(),
            trusted_root: metadata.trusted_root.clone(),
            updated_at: now,
            // Carried through: an update moves the chain forward, never the
            // repository it is a chain for.
            anchor_digest: state.anchor_digest,
        },
        log_keys,
        changed,
    })
}

/// One transparency log a Sigstore trusted root names: where it is, the key
/// its checkpoints are signed with, and the window it was in service for.
///
/// The `baseUrl` matters as much as the key. A build that pins keys but
/// hardcodes an endpoint has only moved the rotation problem: the log to
/// *read* — and, for the control plane, to *write* — is named by the same
/// signed artifact that names the key, so both follow Sigstore together or
/// neither does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tlog {
    /// Where the log is served, no trailing slash.
    pub base_url: String,
    /// The DER SubjectPublicKeyInfo of its verification key.
    pub spki: Vec<u8>,
    /// SHA-256 of that SPKI — what a proof's `log_id` names (and *not* the
    /// `logId.keyId` the trusted root carries beside it, which is the C2SP
    /// note key id; see [`crate::rekor::LogKeys`]).
    pub log_id: [u8; 32],
    /// `validFor.start`, seconds since the epoch. Absent means "always".
    pub valid_from: i64,
    /// `validFor.end`, if the log has been retired.
    pub valid_until: Option<i64>,
}

impl Tlog {
    /// Whether this log was in service at `now`.
    ///
    /// `now` is unsigned and the window bounds are signed, so the comparison
    /// is made in the *unsigned* domain and an unrepresentable bound is
    /// resolved by what it means: a negative start is a window opened before
    /// the epoch, a negative end one that closed then. A cast in either
    /// direction turns `u64::MAX` into `-1` and makes every window vacuous —
    /// the one answer a validity check must never give.
    pub(crate) fn valid_at(&self, now: u64) -> bool {
        let started = match u64::try_from(self.valid_from) {
            Ok(start) => start <= now,
            Err(_) => true,
        };
        let open = self.valid_until.is_none_or(|end| match u64::try_from(end) {
            Ok(end) => now < end,
            Err(_) => false,
        });
        started && open
    }
}

/// The transparency logs a Sigstore trusted root names, in the order it
/// lists them — only `tlogs`, the entries this design pins. Each
/// `publicKey.rawBytes` is a DER SubjectPublicKeyInfo, exactly what
/// [`LogKeys`] parses and what a proof's `log_id` is SHA-256 over.
pub fn tlogs(trusted_root: &[u8]) -> Result<Vec<Tlog>, TufError> {
    let bad = |why: String| TufError::Malformed(format!("trusted root: {why}"));
    let value: serde_json::Value =
        serde_json::from_slice(trusted_root).map_err(|e| bad(e.to_string()))?;
    let entries = value["tlogs"]
        .as_array()
        .ok_or_else(|| bad("tlogs is not an array".into()))?;
    let mut logs = Vec::with_capacity(entries.len());
    for tlog in entries {
        // **An entry this build cannot read is skipped, not fatal** — the
        // difference is the whole update story. Sigstore adds shards, and one
        // will eventually carry a shape written after this binary; refusing
        // the file for it stops pin refresh *globally and permanently* until
        // a new build ships — the "a rotation becomes a client upgrade"
        // outcome §10 exists to remove, reached with no adversary at all.
        // Skipping keeps the shards this build does understand, and the
        // empty-set refusal below is the backstop for when none is left.
        let Some(raw) = tlog["publicKey"]["rawBytes"].as_str() else {
            continue;
        };
        let Ok(spki) = base64_decode(raw) else {
            continue;
        };
        // The key has to be one this build can actually verify a checkpoint
        // with, or pinning it is pinning nothing.
        if LogKey::from_spki(&spki).is_err() {
            continue;
        }
        let Some(base_url) = tlog["baseUrl"].as_str() else {
            continue;
        };
        // A window that will not parse is not a reason to drop the log — its
        // key is still pinned — but it must not read as *currently* valid, so
        // an unparseable start is treated as "not yet" and an unparseable end
        // as "already closed".
        let when = &tlog["publicKey"]["validFor"];
        let valid_from = match when.get("start") {
            None => 0,
            Some(start) => start.as_str().and_then(parse_rfc3339).unwrap_or(i64::MAX),
        };
        let valid_until = when
            .get("end")
            .map(|end| end.as_str().and_then(parse_rfc3339).unwrap_or(0));
        logs.push(Tlog {
            base_url: base_url.trim_end_matches('/').to_string(),
            log_id: sha256(&spki),
            spki,
            valid_from,
            valid_until,
        });
    }
    if logs.is_empty() {
        // Adopting an empty pin set would silently refuse every zone from
        // then on. §10.2's whole posture is that TUF trouble is never worse
        // than not having asked, so this is trouble, not an update. This is
        // also the backstop for the skipping above: a trusted root whose every
        // entry is unreadable moves nothing, rather than moving to nothing.
        return Err(bad(
            "it names no transparency logs this build can read".into()
        ));
    }
    Ok(logs)
}

/// The log in service at `now`, the one a submission goes to and a monitor
/// reads — the latest-started of those whose window contains `now`.
///
/// Sigstore's trusted root keeps retired shards listed so old proofs stay
/// checkable, so "the current log" is a question about windows and not about
/// list order. `None` when every listed log is retired or not yet open,
/// which is a trusted root this build cannot use for anything live.
pub fn current_tlog(logs: &[Tlog], now: u64) -> Option<&Tlog> {
    logs.iter()
        .filter(|log| log.valid_at(now))
        .max_by_key(|log| log.valid_from)
}

/// The transparency-log keys a Sigstore trusted root names — every log it
/// lists, retired ones included, because a proof from a retired shard is
/// still a proof (docs/REKOR-ZONE-KEY.md §10.6).
///
/// [`current_tlog`] is the separate question of which shard is in service —
/// the only place a window decides anything: a shard is read from and
/// written to while its window is open, and its key stays pinned afterwards
/// so the archival proofs published under it keep verifying.
///
/// Each key is pinned **for the origin the same artifact names it at**: a
/// checkpoint's origin line is the log's own name for itself, and for every
/// shard a Sigstore trusted root lists that name is the host of its
/// `baseUrl` — so the trusted root carries the `(origin, key)` pairing that
/// Go's `sumdb/note` takes from a caller-supplied verifier table. Carried
/// through rather than re-derived: `Tlog` already holds it.
pub fn tlog_keys(trusted_root: &[u8]) -> Result<LogKeys, TufError> {
    let bad = |why: String| TufError::Malformed(format!("trusted root: {why}"));
    let mut keys = Vec::new();
    for log in tlogs(trusted_root)? {
        let key = crate::rekor::LogKey::from_spki(&log.spki).map_err(|e| bad(e.to_string()))?;
        keys.push(match note_origin(&log.base_url) {
            Some(origin) => key.for_origin(origin),
            None => key,
        });
    }
    Ok(LogKeys::from_keys(keys))
}

/// The checkpoint origin a log served at `base_url` signs its notes as: the
/// host, with no scheme and no path.
///
/// `None` for a URL this cannot read, which leaves the key unbound rather
/// than bound to a guess — the same posture `--rekor-key` gets.
fn note_origin(base_url: &str) -> Option<String> {
    let rest = base_url
        .strip_prefix("https://")
        .or_else(|| base_url.strip_prefix("http://"))?;
    let host = rest.split('/').next()?.trim_end_matches('.');
    match host.is_empty() {
        true => None,
        false => Some(host.to_ascii_lowercase()),
    }
}

/// The version of a `root.json`, without verifying anything about it.
fn root_version(bytes: &[u8]) -> Option<u64> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    value["signed"]["version"].as_u64()
}

// ------------------------------------------------------------- fetching

/// The official Sigstore TUF repository, the one `EMBEDDED_TUF_ROOT` anchors
/// — where both the client and the monitor read the pin set from.
pub const SIGSTORE_TUF_URL: &str = "https://tuf-repo-cdn.sigstore.dev";

/// How long a client rests between walks of the repository (§10.2). The pin
/// set moves when Sigstore opens or closes a shard — a yearly event — so a
/// day is far more often than that; and the walk is a handful of HTTPS GETs,
/// the difference between touching a CDN once a day and touching it on every
/// membership refresh.
pub(crate) const REFRESH_INTERVAL: u64 = 24 * 60 * 60;

/// How many root versions past the one already trusted [`fetch_metadata`] will
/// probe before giving up. Sigstore rotates roughly yearly; this is decades
/// of headroom and a bound on a repository that answers 200 to everything.
const ROOT_CEILING: u64 = 200;

/// The most bytes one walk may buffer, across every file it collects. A
/// per-response cap is not a bound on a walk: [`fetch_metadata`] holds every
/// root until [`update`] can chain them, and a mirror answering the whole
/// per-response allowance to every root probe turns a daily refresh into
/// more than a gigabyte resident. This aggregate is generous against the
/// real repository — `targets.json` is the only large file, at a few hundred
/// KiB, and roots are tens of KiB apiece.
pub(crate) const MAX_WALK_BYTES: usize = 8 * 1024 * 1024;

/// The longest one walk may take, end to end. A per-request timeout is not a
/// bound either: ~204 requests each stalling just inside it is hours of
/// walk, awaited inside a membership refresh. Whatever is collected by this
/// point is abandoned and the pins in force stand (§10.2).
pub(crate) const MAX_WALK_TIME: Duration = Duration::from_secs(120);

/// A TUF repository, as the one operation walking it needs. Injected rather
/// than hardwired, the same shape the control plane's fetch uses:
/// everything [`fetch_metadata`] decides is then testable without egress.
///
/// `Ok(None)` is a file the repository does not have — the end of the root
/// chain is precisely that answer — and `Err` a repository that could not be
/// reached. One ends a walk, the other abandons it.
#[allow(missing_debug_implementations)]
pub trait Repo {
    /// Fetches one path relative to the repository root.
    fn get(&self, path: &str) -> Result<Option<Vec<u8>>, String>;
}

/// How long one file of a TUF walk may take.
const TUF_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The most a single TUF file may be. Sigstore's `targets.json` is the big
/// one at a few hundred KiB; the cap exists because these are bytes from a
/// party nothing is trusted about, and a response with no bound is a reader
/// that can be exhausted.
const MAX_TUF_BYTES: usize = 8 * 1024 * 1024;

/// A TUF repository read over HTTPS: the daemon's binding refresh and the
/// monitor's discovery both walk one, so the transport — and above all its
/// byte cap — exists once rather than as two copies annotated as needing to
/// agree.
///
/// Built and used inside [`tokio::task::spawn_blocking`], which is what makes
/// a blocking client the right one: the walk is sequential — each file names
/// the next — so there is no concurrency to give up, and the JSON parsing
/// stays off the reactor. TLS is not load-bearing: every byte fetched is
/// self-authenticating and checked against [`EMBEDDED_TUF_ROOT`] before it
/// moves anything, so a hostile mirror can deny this walk and cannot make it
/// mean anything (§10.2).
#[derive(Debug)]
pub struct HttpRepo {
    base: String,
    client: reqwest::blocking::Client,
}

impl HttpRepo {
    /// A repository at `base` (e.g. [`SIGSTORE_TUF_URL`]), with requests
    /// identified by `user_agent` when one is given.
    pub fn new(base: &str, user_agent: Option<&str>) -> Result<HttpRepo, String> {
        let mut builder = reqwest::blocking::Client::builder().timeout(TUF_TIMEOUT);
        if let Some(user_agent) = user_agent {
            builder = builder.user_agent(user_agent);
        }
        Ok(HttpRepo {
            base: base.trim_end_matches('/').to_string(),
            client: builder.build().map_err(|e| e.to_string())?,
        })
    }
}

impl Repo for HttpRepo {
    fn get(&self, path: &str) -> Result<Option<Vec<u8>>, String> {
        let url = format!("{}/{path}", self.base);
        let response = self
            .client
            .get(&url)
            .send()
            .map_err(|e| format!("{url}: {e}"))?;
        match response.status().as_u16() {
            200 => {
                // Read through a `take`, never `bytes()`: a cap applied to
                // the result of `bytes()` is a bound on nothing, because the
                // allocation already happened — an endless body from a hostile
                // mirror exhausts the reader before the comparison runs. One
                // byte past the cap keeps "at the cap" and "over it"
                // distinguishable.
                use std::io::Read;
                let mut body = Vec::new();
                response
                    .take(MAX_TUF_BYTES as u64 + 1)
                    .read_to_end(&mut body)
                    .map_err(|e| format!("{url}: {e}"))?;
                if body.len() > MAX_TUF_BYTES {
                    return Err(format!("{url}: over the {MAX_TUF_BYTES}-byte cap"));
                }
                Ok(Some(body))
            }
            // The end of the root chain is a 404, and Sigstore's CDN answers
            // 403 for an object that is not there.
            403 | 404 => Ok(None),
            status => Err(format!("{url}: the repository answered {status}")),
        }
    }
}

/// Walks a TUF repository, starting the root chain at `from_root` — the
/// version the caller already trusts. **This verifies nothing.** It follows
/// the consistent-snapshot naming so the right files are collected —
/// timestamp names the snapshot version, the snapshot names the targets
/// version, the targets name the target's digest — and hands the result to
/// [`update`], where every signature, expiry and rollback bound is checked.
/// Fetching over a hostile transport is therefore a denial, not a
/// vulnerability: the bytes are self-authenticating, so a tampering mirror
/// produces material that fails verification and the current pins stand.
pub fn fetch_metadata(repo: &dyn Repo, from_root: u64) -> Result<TufMetadata, TufError> {
    let mut budget = Budget::new();
    let get = |budget: &mut Budget, path: &str| -> Result<Option<Vec<u8>>, TufError> {
        budget.check()?;
        let bytes = repo.get(path).map_err(TufError::Malformed)?;
        budget.spend(bytes.as_ref().map_or(0, Vec::len))?;
        Ok(bytes)
    };
    let fetch = |budget: &mut Budget, path: &str| -> Result<Vec<u8>, TufError> {
        get(budget, path)?.ok_or_else(|| TufError::Chain(format!("the repository has no {path}")))
    };

    // The root chain, from the version already trusted up to whatever the
    // repository last published; the walk ends at the first version the
    // repository does not have — TUF's way of saying "this is current".
    let mut roots = Vec::new();
    for version in from_root..from_root.saturating_add(ROOT_CEILING) {
        match get(&mut budget, &format!("{version}.root.json"))? {
            None => break,
            Some(bytes) => roots.push(bytes),
        }
    }
    if roots.is_empty() {
        return Err(TufError::Chain(format!(
            "the repository has no {from_root}.root.json, the root this client trusts"
        )));
    }

    let timestamp = fetch(&mut budget, "timestamp.json")?;
    let snapshot_version = meta_version(&timestamp, SNAPSHOT_META)?;
    let snapshot = fetch(&mut budget, &format!("{snapshot_version}.{SNAPSHOT_META}"))?;
    let targets_version = meta_version(&snapshot, TARGETS_META)?;
    let targets = fetch(&mut budget, &format!("{targets_version}.{TARGETS_META}"))?;

    // The one target the whole chain exists to carry, named by its digest.
    // `update` re-derives that digest from the bytes, so a repository that
    // serves something else here fails verification rather than this fetch.
    let digest = target_digest(&targets, TRUSTED_ROOT_TARGET)?;
    let trusted_root = fetch(
        &mut budget,
        &format!("targets/{digest}.{TRUSTED_ROOT_TARGET}"),
    )?;

    Ok(TufMetadata {
        roots,
        timestamp,
        snapshot,
        targets,
        trusted_root,
    })
}

/// What one walk of a repository is allowed to spend: bytes buffered and time
/// elapsed, both across the whole walk rather than per response.
struct Budget {
    started: Instant,
    spent: usize,
}

impl Budget {
    fn new() -> Budget {
        Budget {
            started: Instant::now(),
            spent: 0,
        }
    }

    /// Whether the walk may make another request.
    fn check(&self) -> Result<(), TufError> {
        match self.started.elapsed() > MAX_WALK_TIME {
            false => Ok(()),
            true => Err(TufError::Malformed(format!(
                "the walk took longer than {} seconds; the current pins stand",
                MAX_WALK_TIME.as_secs()
            ))),
        }
    }

    /// Charges a response against the byte budget.
    fn spend(&mut self, bytes: usize) -> Result<(), TufError> {
        self.spent = self.spent.saturating_add(bytes);
        match self.spent <= MAX_WALK_BYTES {
            true => Ok(()),
            false => Err(TufError::Malformed(format!(
                "the walk served more than the {MAX_WALK_BYTES}-byte ceiling; \
                 the current pins stand"
            ))),
        }
    }
}

/// The version a role lists for the file below it, unverified — enough to
/// name the next file to fetch.
fn meta_version(bytes: &[u8], file: &str) -> Result<u64, TufError> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|e| TufError::Malformed(format!("{file}: {e}")))?;
    value["signed"]["meta"][file]["version"]
        .as_u64()
        .ok_or_else(|| TufError::Malformed(format!("the metadata does not list {file}")))
}

/// The SHA-256 `targets.json` names for a target, unverified — enough to
/// name the consistent-snapshot file to fetch.
fn target_digest(targets: &[u8], name: &str) -> Result<String, TufError> {
    let value: serde_json::Value = serde_json::from_slice(targets)
        .map_err(|e| TufError::Malformed(format!("{TARGETS_META}: {e}")))?;
    let digest = value["signed"]["targets"][name]["hashes"]["sha256"]
        .as_str()
        .ok_or_else(|| {
            TufError::Malformed(format!("{TARGETS_META} names no {name} with a sha256"))
        })?;
    match digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        true => Ok(digest.to_ascii_lowercase()),
        false => Err(TufError::Malformed(format!(
            "{TARGETS_META} gives {name} a digest that is not a SHA-256"
        ))),
    }
}

// ---------------------------------------------------------------- metadata

/// One parsed TUF metadata file: the `signed` object, the canonical bytes
/// its signatures cover, and the signatures themselves.
#[derive(Debug, Clone)]
struct Meta {
    role: &'static str,
    version: u64,
    expires: i64,
    signed: serde_json::Value,
    /// The canonical JSON of `signed` — what every signature is over.
    canonical: Vec<u8>,
    /// `(keyid, signature)`, in the order the file lists them.
    signatures: Vec<(String, Vec<u8>)>,
}

impl Meta {
    /// Parses a metadata file and checks it declares the role it was fetched
    /// as. A snapshot served where the targets belong is a chain error, not
    /// a signature one, and saying so is how `synch doctor` stays useful.
    fn parse(bytes: &[u8], role: &'static str) -> Result<Meta, TufError> {
        let bad = |why: String| TufError::Malformed(format!("{role}.json: {why}"));
        let value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|e| bad(e.to_string()))?;
        let signed = value
            .get("signed")
            .cloned()
            .ok_or_else(|| bad("no signed object".into()))?;
        let declared = signed["_type"]
            .as_str()
            .ok_or_else(|| bad("signed._type is not a string".into()))?;
        if declared != role {
            return Err(TufError::Chain(format!(
                "a file served as {role}.json declares itself {declared}"
            )));
        }
        // spec_version is `MAJOR.MINOR.FIX`; a major bump is a format this
        // build does not claim to read.
        let spec = signed["spec_version"]
            .as_str()
            .ok_or_else(|| bad("no spec_version".into()))?;
        if !spec.starts_with("1.") {
            return Err(bad(format!("spec version {spec} is not 1.x")));
        }
        let version = signed["version"]
            .as_u64()
            .ok_or_else(|| bad("version is not a whole number".into()))?;
        let expires = parse_rfc3339(
            signed["expires"]
                .as_str()
                .ok_or_else(|| bad("expires is not a string".into()))?,
        )
        .ok_or_else(|| bad("expires is not an RFC 3339 timestamp".into()))?;
        let mut signatures = Vec::new();
        for entry in value["signatures"]
            .as_array()
            .ok_or_else(|| bad("signatures is not an array".into()))?
        {
            let keyid = entry["keyid"]
                .as_str()
                .ok_or_else(|| bad("a signature has no keyid".into()))?;
            let sig = hex_decode(
                entry["sig"]
                    .as_str()
                    .ok_or_else(|| bad("a signature has no sig".into()))?,
            )
            .ok_or_else(|| bad("a signature is not hex".into()))?;
            signatures.push((keyid.to_string(), sig));
        }
        let canonical = canonical_json(&signed).map_err(bad)?;
        Ok(Meta {
            role,
            version,
            expires,
            signed,
            canonical,
            signatures,
        })
    }

    /// Whether this role is still valid at `now`.
    ///
    /// `now` is unsigned and `expires` is signed, so the comparison happens in
    /// the unsigned domain: a timestamp before the epoch is expired, and a
    /// `now` past `i64::MAX` is not quietly reinterpreted as a negative number
    /// that every expiry clears. Expiry is the only bound on how old the
    /// metadata a hostile mirror may serve is, so a comparison that can be
    /// made vacuous is not a bound at all.
    fn check_expiry(&self, now: u64) -> Result<(), TufError> {
        match u64::try_from(self.expires).is_ok_and(|expires| expires > now) {
            true => Ok(()),
            false => Err(TufError::Expiry(format!(
                "{}.json version {} expired at {}",
                self.role, self.version, self.expires
            ))),
        }
    }

    fn check_rollback(&self, accepted: u64) -> Result<(), TufError> {
        match self.version >= accepted {
            true => Ok(()),
            false => Err(TufError::Rollback(format!(
                "{}.json version {} is older than the accepted {accepted}",
                self.role, self.version
            ))),
        }
    }

    /// Checks a file this one lists in `meta`: version exactly, hashes and
    /// length when they are given.
    ///
    /// Sigstore's timestamp lists `snapshot.json` by version alone, and its
    /// snapshot does the same for `targets.json` — hashes are optional in
    /// the TUF spec and this repository omits them for the files that change
    /// every publish. Version equality is what still binds them.
    fn check_listed(&self, file: &str, bytes: &[u8], version: u64) -> Result<(), TufError> {
        let entry = &self.signed["meta"][file];
        let listed = entry["version"]
            .as_u64()
            .ok_or_else(|| TufError::Chain(format!("{}.json does not list {file}", self.role)))?;
        if listed != version {
            return Err(TufError::Rollback(format!(
                "{}.json names {file} version {listed}, the walk collected {version}",
                self.role
            )));
        }
        check_hashes(file, entry, bytes)
    }

    /// Checks the target file the chain exists to authenticate.
    ///
    /// Unlike `meta`, a target entry's `hashes` are not optional: without
    /// them nothing in the chain says anything about these bytes.
    fn check_target(&self, name: &str, bytes: &[u8]) -> Result<(), TufError> {
        let entry = &self.signed["targets"][name];
        if entry.is_null() {
            return Err(TufError::TargetHash(format!(
                "targets.json version {} names no {name}",
                self.version
            )));
        }
        if entry["hashes"]["sha256"].as_str().is_none() {
            return Err(TufError::TargetHash(format!(
                "targets.json gives no sha256 for {name}"
            )));
        }
        check_hashes(name, entry, bytes)
    }
}

/// Compares a file against the `hashes`/`length` an entry carries, when it
/// carries them.
fn check_hashes(file: &str, entry: &serde_json::Value, bytes: &[u8]) -> Result<(), TufError> {
    if let Some(length) = entry["length"].as_u64() {
        if length != bytes.len() as u64 {
            return Err(TufError::TargetHash(format!(
                "{file} is {} bytes, the metadata says {length}",
                bytes.len()
            )));
        }
    }
    if let Some(expected) = entry["hashes"]["sha256"].as_str() {
        let actual = hex::encode(sha256(bytes));
        if !expected.eq_ignore_ascii_case(&actual) {
            return Err(TufError::TargetHash(format!(
                "{file} hashes to {actual}, the metadata says {expected}"
            )));
        }
    }
    Ok(())
}

/// A parsed `root.json`: the metadata plus the role and key tables that make
/// it the thing every other file is checked against.
#[derive(Debug, Clone)]
struct Root {
    version: u64,
    meta: Meta,
    bytes: Vec<u8>,
    /// The role table: role name → (key ids, threshold).
    roles: BTreeMap<String, (Vec<String>, u64)>,
    /// The key table, by the id the roles and signatures name.
    keys: BTreeMap<String, TufKey>,
}

impl Root {
    fn parse(bytes: &[u8]) -> Result<Root, TufError> {
        let meta = Meta::parse(bytes, ROOT_ROLE)?;
        let bad = |why: String| TufError::Malformed(format!("root.json: {why}"));
        let mut roles = BTreeMap::new();
        for (name, role) in meta.signed["roles"]
            .as_object()
            .ok_or_else(|| bad("roles is not an object".into()))?
        {
            let threshold = role["threshold"]
                .as_u64()
                .ok_or_else(|| bad(format!("role {name} has no threshold")))?;
            if threshold == 0 {
                // A zero threshold is a role anything satisfies.
                return Err(bad(format!("role {name} has threshold 0")));
            }
            let keyids = role["keyids"]
                .as_array()
                .ok_or_else(|| bad(format!("role {name} has no keyids")))?
                .iter()
                .filter_map(|id| id.as_str().map(str::to_string))
                .collect();
            roles.insert(name.clone(), (keyids, threshold));
        }
        let mut keys = BTreeMap::new();
        for (id, key) in meta.signed["keys"]
            .as_object()
            .ok_or_else(|| bad("keys is not an object".into()))?
        {
            // A key this build cannot use is not an error here: it becomes a
            // threshold failure only if a role actually needs it.
            if let Ok(parsed) = TufKey::parse(key) {
                keys.insert(id.clone(), parsed);
            }
        }
        Ok(Root {
            version: meta.version,
            bytes: bytes.to_vec(),
            roles,
            keys,
            meta,
        })
    }

    fn check_expiry(&self, now: u64) -> Result<(), TufError> {
        self.meta.check_expiry(now)
    }

    /// Checks that `meta` carries signatures from at least `threshold`
    /// distinct keys of this root's `role` — distinct is doing work: one
    /// key's signature repeated five times must not satisfy a threshold of
    /// three.
    fn check_role(&self, role: &str, meta: &Meta) -> Result<(), TufError> {
        let (keyids, threshold) = self.roles.get(role).ok_or_else(|| {
            TufError::Chain(format!("root {} defines no {role} role", self.version))
        })?;
        let authorized: BTreeSet<&String> = keyids.iter().collect();
        let mut signed: BTreeSet<&str> = BTreeSet::new();
        // Every keyid a verification was *attempted* for, not the ones that
        // verified: skipping on `signed` alone short-circuits a repeated
        // keyid only once it has succeeded, so a file repeating one
        // authorized keyid with a failing signature paid for a fresh P-256
        // verification per copy — and the signature list is bounded only by
        // the document, so one hostile response bought tens of thousands of
        // them. A keyid gets one attempt either way; a repeated keyid is
        // malformed input, and no valid file needs a second try.
        let mut tried: BTreeSet<&str> = BTreeSet::new();
        for (keyid, signature) in &meta.signatures {
            if !authorized.contains(keyid) || !tried.insert(keyid.as_str()) {
                continue;
            }
            let Some(key) = self.keys.get(keyid) else {
                continue;
            };
            if key.verify(&meta.canonical, signature).is_ok() {
                signed.insert(keyid);
            }
        }
        match signed.len() as u64 >= *threshold {
            true => Ok(()),
            false => Err(TufError::Threshold(format!(
                "{}.json version {} carries {} of the {threshold} {role} signatures root {} requires",
                meta.role,
                meta.version,
                signed.len(),
                self.version
            ))),
        }
    }
}

// -------------------------------------------------------------------- keys

/// One key from a root's key table: a [`RawKey`] and nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TufKey(RawKey);

impl TufKey {
    /// Parses one entry of `signed.keys`.
    ///
    /// Dispatch is on `scheme`, never `keytype`: Sigstore's roots write the
    /// same P-256 key as keytype `ecdsa-sha2-nistp256` up to version 8 and
    /// `ecdsa` from version 9, while the scheme stayed
    /// `ecdsa-sha2-nistp256` throughout. `keyval.public` is PEM
    /// SubjectPublicKeyInfo in every root from version 5 on, and hex-encoded
    /// raw key material before that.
    fn parse(key: &serde_json::Value) -> Result<TufKey, TufError> {
        let bad = |why: &str| TufError::Signature(format!("key: {why}"));
        let scheme = match key["scheme"].as_str() {
            Some("ecdsa-sha2-nistp256") => Scheme::EcdsaP256Sha256,
            Some("ed25519") => Scheme::Ed25519,
            Some(other) => return Err(bad(&format!("unsupported scheme {other}"))),
            None => return Err(bad("no scheme")),
        };
        let public = key["keyval"]["public"]
            .as_str()
            .ok_or_else(|| bad("no keyval.public"))?;
        let key = match public.contains("-----BEGIN") {
            true => RawKey::from_spki_as(&pem_body(public)?, scheme).ok_or_else(|| {
                TufError::Signature("a key is not the SubjectPublicKeyInfo its scheme names".into())
            })?,
            false => RawKey::from_raw(
                &hex_decode(public.trim()).ok_or_else(|| bad("keyval.public is not hex"))?,
                scheme,
            )
            .ok_or_else(|| {
                TufError::Signature("a key is not the raw material its scheme names".into())
            })?,
        };
        Ok(TufKey(key))
    }

    /// Verifies one signature over the canonical bytes, under the shared
    /// double-encoding rule ([`RawKey::verifies`]): Sigstore's TUF signatures
    /// are DER, and the fixed-width verifier refuses those outright.
    fn verify(&self, message: &[u8], sig: &[u8]) -> Result<(), TufError> {
        match self.0.verifies(message, sig) {
            true => Ok(()),
            false => Err(TufError::Signature("a signature does not verify".into())),
        }
    }
}

/// The base64 body of a PEM block, whatever its label.
fn pem_body(pem: &str) -> Result<Vec<u8>, TufError> {
    let body: String = pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect::<Vec<_>>()
        .join("");
    base64_decode(body.trim()).map_err(|_| TufError::Signature("a PEM key is not base64".into()))
}

/// The key id TUF derives for a key object: SHA-256 over its canonical JSON.
///
/// Informational, not a lookup path. Sigstore's roots key their table by ids
/// that agree with this for every key in every root but one — root 11 kept a
/// key's id while editing a `x-tuf-on-ci-online-uri` member inside it — so
/// this is how a fixture test says the two normally agree.
#[cfg(any(test, feature = "sim"))]
pub(crate) fn key_id(key: &serde_json::Value) -> Result<String, TufError> {
    Ok(hex::encode(sha256(
        &canonical_json(key).map_err(TufError::Malformed)?,
    )))
}

// -------------------------------------------------------- canonical JSON

/// Renders a value as OLPC canonical JSON — the form TUF signatures cover.
///
/// The rules are few and every one is load-bearing: object members sorted by
/// key, no whitespace anywhere, strings escaping only `"` and `\` (control
/// characters travel raw), integers only. The conformance fixture verifies it
/// against the real repository's bytes rather than a hand-written example.
pub(crate) fn canonical_json(value: &serde_json::Value) -> Result<Vec<u8>, String> {
    let mut out = String::new();
    write_canonical(value, &mut out)?;
    Ok(out.into_bytes())
}

fn write_canonical(value: &serde_json::Value, out: &mut String) -> Result<(), String> {
    match value {
        serde_json::Value::Null => out.push_str("null"),
        serde_json::Value::Bool(true) => out.push_str("true"),
        serde_json::Value::Bool(false) => out.push_str("false"),
        serde_json::Value::Number(number) => match number.as_i64() {
            Some(n) => out.push_str(&n.to_string()),
            // A float has no canonical rendering, so canonical JSON has no
            // floats. Refusing is the only honest answer.
            None => match number.as_u64() {
                Some(n) => out.push_str(&n.to_string()),
                None => return Err(format!("{number} is not a whole number")),
            },
        },
        serde_json::Value::String(text) => write_canonical_string(text, out),
        serde_json::Value::Array(items) => {
            out.push('[');
            for (at, item) in items.iter().enumerate() {
                if at > 0 {
                    out.push(',');
                }
                write_canonical(item, out)?;
            }
            out.push(']');
        }
        serde_json::Value::Object(members) => {
            // serde_json's map is already ordered by key; sorting again
            // costs nothing and means the ordering is this function's
            // promise rather than a dependency's default.
            let mut keys: Vec<&String> = members.keys().collect();
            keys.sort();
            out.push('{');
            for (at, key) in keys.into_iter().enumerate() {
                if at > 0 {
                    out.push(',');
                }
                write_canonical_string(key, out);
                out.push(':');
                write_canonical(&members[key], out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

/// A canonical-JSON string: quotes and backslashes escaped, nothing else.
fn write_canonical_string(text: &str, out: &mut String) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
}

// ------------------------------------------------------------------ time

/// Parses the RFC 3339 timestamps TUF metadata carries, as seconds since the
/// epoch.
///
/// Narrow on purpose, and wider than the current repository needs: Sigstore's
/// roots have written `2023-04-18T18:13:43Z`, `2022-05-11T19:09:02.663975009Z`
/// and `2021-12-18T13:28:12.99008-06:00` at different times, so fractional
/// seconds and numeric offsets both have to parse or old chains stop walking.
fn parse_rfc3339(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    // Every separator is checked, not only the ones in the date: a timestamp
    // whose punctuation is not RFC 3339's is a string this parser has no
    // business reading fields out of by offset.
    if bytes.len() < 19
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let number = |from: usize, to: usize| text.get(from..to)?.parse::<i64>().ok();
    let year = number(0, 4)?;
    let month = number(5, 7)?;
    let day = number(8, 10)?;
    let hour = number(11, 13)?;
    let minute = number(14, 16)?;
    let second = number(17, 19)?;
    // 60 is a leap second, which RFC 3339 §5.6 admits; 61 is not a time.
    if !(1..=12).contains(&month) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    // The day is checked against the month it is in, not against 31. The
    // conversion below rolls an overlong day forward — `2026-02-31` becomes
    // March 3rd — and every field this parser reads is an *expiry*, so
    // accepting one silently extended the metadata's life by the overflow.
    // Small, but it is the one direction an expiry must never move on its own.
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let last = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => 28 + i64::from(leap),
    };
    if !(1..=last).contains(&day) {
        return None;
    }
    // Whatever follows the seconds is a fraction, a zone, or both.
    let rest = &text[19..];
    let rest = match rest.strip_prefix('.') {
        Some(fraction) => {
            &fraction[fraction
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(fraction.len())..]
        }
        None => rest,
    };
    let offset = match rest.as_bytes() {
        [b'Z'] | [b'z'] | [] => 0,
        [sign @ (b'+' | b'-'), ..] if rest.len() == 6 && rest.as_bytes()[3] == b':' => {
            let hours: i64 = rest[1..3].parse().ok()?;
            let minutes: i64 = rest[4..6].parse().ok()?;
            let magnitude = hours * 3600 + minutes * 60;
            match sign {
                b'-' => -magnitude,
                _ => magnitude,
            }
        }
        _ => return None,
    };
    Some(
        synch_core::civil::days_from_civil(year, month, day) * 86_400
            + hour * 3600
            + minute * 60
            + second
            - offset,
    )
}

// ------------------------------------------------------------- encodings

/// Lowercase or uppercase hex, as TUF writes signatures and digests.
fn hex_decode(text: &str) -> Option<Vec<u8>> {
    hex::decode(text).ok()
}

#[cfg(test)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_json_is_canonical_json() {
        let value: serde_json::Value = serde_json::from_str(
            r#"{ "b": 1, "a": [true, false, null], "c": "quote \" slash \\ tab \t" }"#,
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(canonical_json(&value).unwrap()).unwrap(),
            // Canonical JSON escapes only the quote and the backslash, so a
            // raw tab inside a string survives — where implementations diverge.
            "{\"a\":[true,false,null],\"b\":1,\"c\":\"quote \\\" slash \\\\ tab \t\"}"
        );
        // Floats have no canonical rendering, so they are refused.
        assert!(canonical_json(&serde_json::json!({ "x": 1.5 })).is_err());
    }

    #[test]
    fn rfc3339_parses_every_shape_the_repository_has_written() {
        // 1970-01-01T00:00:00Z is the epoch, by construction.
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2026-11-20T13:58:18Z"), Some(1_795_183_098));
        // Fractional seconds are dropped, not misread as anything.
        assert_eq!(
            parse_rfc3339("2022-05-11T19:09:02.663975009Z"),
            parse_rfc3339("2022-05-11T19:09:02Z")
        );
        // A numeric offset moves the instant, and both signs work.
        assert_eq!(
            parse_rfc3339("2021-12-18T13:28:12.99008-06:00"),
            parse_rfc3339("2021-12-18T19:28:12Z")
        );
        assert_eq!(
            parse_rfc3339("2021-12-18T13:28:12+02:00"),
            parse_rfc3339("2021-12-18T11:28:12Z")
        );
        // A leap second is a time RFC 3339 §5.6 admits.
        assert_eq!(
            parse_rfc3339("2016-12-31T23:59:60Z"),
            Some(parse_rfc3339("2016-12-31T23:59:59Z").unwrap() + 1)
        );
        // The fields are read out by offset, so every separator is checked,
        // not only the ones in the date.
        for broken in [
            "",
            "2026-13-20T13:58:18Z",
            "2026-11-20T13:58:18+0200",
            "2026-11-20T13.58:18Z",
            "2026-11-20T13:58:61Z",
            // A day that does not exist in the month it names: the conversion
            // rolls one forward — Feb 31st becomes March 3rd — and every
            // field read is an expiry, so accepting one silently extended the
            // metadata's life by the overflow.
            "2026-02-31T00:00:00Z",
            "2026-01-00T00:00:00Z",
            // 2026 is not a leap year.
            "2026-02-29T00:00:00Z",
        ] {
            assert_eq!(parse_rfc3339(broken), None, "{broken:?} must not parse");
        }
        // The leap days that do exist still parse.
        for real in [
            "2024-02-29T00:00:00Z",
            "2000-02-29T00:00:00Z",
            "2026-01-31T00:00:00Z",
            "2026-04-30T00:00:00Z",
        ] {
            assert!(parse_rfc3339(real).is_some(), "{real:?} must parse");
        }
    }

    #[test]
    fn a_state_file_round_trips_and_is_the_owners_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("rekor-pins.json");
        let state = PinState {
            updated_at: 1_700_000_000,
            ..PinState::embedded()
        };
        state.save(&path).unwrap();
        assert_eq!(PinState::load(&path).as_ref(), Some(&state));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // The temporary never shared a name with anybody else's: two writers
        // renaming over the real one is the case the rename dance prevents.
        let siblings: Vec<String> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(siblings, vec!["rekor-pins.json".to_string()]);
    }

    /// One entry this build cannot read must not cost the ones it can:
    /// Sigstore will eventually publish a shard in a shape written after
    /// this binary, and refusing the whole root for it would stop pin
    /// refresh globally — "a rotation becomes a client upgrade" (§10).
    #[test]
    fn an_unreadable_shard_is_skipped_and_the_readable_ones_survive() {
        // An empty pin set is never adopted, and neither is garbage.
        assert!(matches!(
            tlog_keys(br#"{"tlogs":[]}"#),
            Err(TufError::Malformed(_))
        ));
        assert!(matches!(tlogs(b"not json"), Err(TufError::Malformed(_))));
        let p384 = "MHYwEAYHKoZIzj0CAQYFK4EEACIDYgAEnl6ZQFT3z9Xk3gGmNCEnhZAcuP0Ib3Yl                 Cn0nOxKMOxYOs+7t1EytzHnjvUvJcVZLzGGyEXFYVCPmVXOImk7VkRz0hkK+9tJm                 ovNXeqXHtNc4DmMfDsJrbYbHNGiBTsMD";
        let root = serde_json::json!({"tlogs": [
            // A curve this build does not verify with.
            tlog_entry(Some("https://p384.example"), p384, serde_json::json!("2021-01-12T11:53:27Z"), None),
            // A window a future encoder spelled as a number.
            tlog_entry(Some("https://odd.example"), P256, serde_json::json!(1610452407), None),
            // No baseUrl at all.
            tlog_entry(None, P256, serde_json::json!("2021-01-12T11:53:27Z"), None),
            // And one this build reads perfectly well.
            tlog_entry(Some("https://good.example"), P256, serde_json::json!("2021-01-12T11:53:27Z"), None),
        ]})
        .to_string();

        let logs = tlogs(root.as_bytes()).expect("the readable shard survives");
        assert_eq!(logs.len(), 2, "{logs:?}");
        assert!(logs
            .iter()
            .any(|log| log.base_url == "https://good.example"));
        // The one with an unparseable window is *pinned* — a proof under its
        // key still verifies — but reads as not yet in service.
        let odd = logs
            .iter()
            .find(|log| log.base_url == "https://odd.example")
            .expect("its key is still pinned");
        assert!(!odd.valid_at(1_800_000_000));
        assert!(tlog_keys(root.as_bytes()).is_ok());
    }

    const P256: &str = "MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAE2G2Y+2tabdTV5BcGiBIx0a9fAFwrkBbmLSGtks4L3qX6yYY0zufBnhC8Ur/iy55GhWP/9A/bY2LhC30M9+RYtw==";
    const ED25519: &str = "MCowBQYDK2VwAyEAt8rlp1knGwjfbcXAYPYAkn0XiLz1x8O4t0YkEhie244=";

    /// One `{"tlogs": [...]}` entry, in the shapes these tests exercise.
    fn tlog_entry(
        base_url: Option<&str>,
        spki_b64: &str,
        start: serde_json::Value,
        end: Option<&str>,
    ) -> serde_json::Value {
        let mut valid_for = serde_json::json!({ "start": start });
        if let Some(end) = end {
            valid_for["end"] = serde_json::json!(end);
        }
        let mut entry =
            serde_json::json!({ "publicKey": { "rawBytes": spki_b64, "validFor": valid_for } });
        if let Some(base_url) = base_url {
            entry["baseUrl"] = serde_json::json!(base_url);
        }
        entry
    }

    /// A trusted root with three shards: one closed, one open, one not yet.
    fn three_shards() -> Vec<u8> {
        serde_json::json!({"tlogs": [
            tlog_entry(Some("https://retired.example/"), P256, serde_json::json!("2021-01-12T11:53:27Z"), Some("2025-09-23T00:00:00Z")),
            tlog_entry(Some("https://open.example"), ED25519, serde_json::json!("2025-09-23T00:00:00Z"), None),
            tlog_entry(Some("https://next.example"), ED25519, serde_json::json!("2030-01-01T00:00:00Z"), None),
        ]})
        .to_string()
        .into_bytes()
    }

    #[test]
    fn the_current_log_is_the_one_whose_window_is_open_now() {
        let logs = tlogs(&three_shards()).unwrap();
        // 2026-08-16: the middle shard.
        let open = current_tlog(&logs, 1_786_854_774).expect("a log in service");
        assert_eq!(open.base_url, "https://open.example");
        // 2023: the first, before the middle one opened.
        assert_eq!(
            current_tlog(&logs, 1_690_000_000).unwrap().base_url,
            "https://retired.example",
            "a trailing slash is not part of a base URL"
        );
        // 2035: the last, once the others have been superseded.
        assert_eq!(
            current_tlog(&logs, 2_050_000_000).unwrap().base_url,
            "https://next.example"
        );
        // Before any of them opened, nothing is in service — a trusted root
        // to report on, not one to guess a hostname from.
        assert!(current_tlog(&logs, 0).is_none());

        // Every shard stays pinned regardless: a proof from the retired one
        // is still a proof, and its checkpoint still has to verify.
        assert_eq!(tlog_keys(&three_shards()).unwrap().keys().len(), 3);
    }

    #[test]
    fn the_embedded_trusted_root_names_a_log_in_service() {
        let logs = tlogs(EMBEDDED_TRUSTED_ROOT.as_bytes()).expect("the embedded trusted root");
        // The bootstrap has to be usable on its own: a build whose embedded
        // artifact names no open shard cannot make a first request at all.
        let open = current_tlog(&logs, 1_786_854_774).expect("an open shard in the embedded root");
        assert!(open.base_url.starts_with("https://"));
        // And the bootstrap pin set is exactly what that artifact names — the
        // only guard against the two embedded artifacts drifting apart.
        assert_eq!(
            crate::rekor::LogKeys::embedded(),
            tlog_keys(EMBEDDED_TRUSTED_ROOT.as_bytes()).unwrap()
        );
    }

    /// One walk has an aggregate byte budget and a total deadline, not only
    /// per-response caps: a mirror answering the whole per-response allowance
    /// to every root probe turns a daily refresh into a gigabyte resident,
    /// and ~204 requests stalling just inside a per-request timeout is hours
    /// of walk — `update` would have bailed after two roots.
    #[test]
    fn a_walk_that_serves_too_many_bytes_is_abandoned() {
        /// A repository that answers a megabyte of nothing to every request.
        struct Greedy;
        impl Repo for Greedy {
            fn get(&self, _path: &str) -> Result<Option<Vec<u8>>, String> {
                Ok(Some(vec![b'x'; 1024 * 1024]))
            }
        }
        let error = fetch_metadata(&Greedy, 1).expect_err("a greedy mirror is abandoned");
        assert!(
            matches!(&error, TufError::Malformed(why) if why.contains("ceiling")),
            "{error}"
        );
        // Well short of what `ROOT_CEILING` responses would have cost.
        assert!(MAX_WALK_BYTES < ROOT_CEILING as usize * 1024 * 1024);

        // And the same walk has a total deadline: a walk past it stops, and
        // the current pins stand.
        let overrun = Budget {
            started: Instant::now()
                .checked_sub(MAX_WALK_TIME * 2)
                .expect("a monotonic clock with some history"),
            spent: 0,
        };
        let error = overrun.check().expect_err("a walk past its deadline stops");
        assert!(
            matches!(&error, TufError::Malformed(why) if why.contains("longer than")),
            "{error}"
        );
    }
}
