//! DNSSEC-based membership discovery (§3.2).
//!
//! The resolver queries `_synchronicity.<domain> TXT` (`v=sync1 id=<label>
//! nk=<z-base-32 device key>`) and validates the chain in process — an
//! upstream resolver's AD bit is never trusted. A response that does not
//! validate is discarded and the cached member set retained until its own
//! expiry. Fail closed. Everything above the resolver — record parsing and the
//! malformed-set rules — is pure and unit-tested here without the network.

use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    path::Path,
    pin::Pin,
    time::Duration,
};

use iroh_base::PublicKey;
use synch_core::{
    origin::{normalize_domain, normalize_label},
    NodeId, OriginId,
};

use hickory_resolver::proto::{
    dnssec::{rdata::RRSIG, TrustAnchors},
    rr::{Name, Record, RecordType},
};

use crate::{
    error::NetError,
    rekor::{self, LogKeys, ProofError, VerifiedRecord, ZoneKey},
    tuf::{self, PinState, Repo, TufError, TufMetadata, TufUpdate},
};

/// The label the membership TXT records live under.
pub(crate) const TXT_PREFIX: &str = "_synchronicity";

/// The version tag every accepted record must carry.
pub(crate) const RECORD_VERSION_TAG: &str = "sync1";

/// Lower clamp on the re-resolution interval (§3.2).
pub const MIN_TTL: Duration = Duration::from_secs(60);
/// Upper clamp on the re-resolution interval (§3.2).
pub(crate) const MAX_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// The control plane's zone-key watch cadence
/// (`control-plane/src/jobs/zonekey_watch.gleam`): how long a rotated provider
/// key can sit unnoticed.
const CONTROL_PLANE_WATCH_CADENCE: Duration = Duration::from_secs(300);
/// The log round trip and the reconciler pass that follow the observation.
const CONTROL_PLANE_LOG_ROUND_TRIP: Duration = Duration::from_secs(60);
/// The TTL the control plane publishes proof records with
/// (`control-plane/src/zone/render_external.gleam`): how long a resolver may
/// still be serving the proof set from before the rotation.
const CONTROL_PLANE_PROOF_TTL: Duration = Duration::from_secs(300);

/// The longest a control plane in external mode can take to get a rotated
/// provider key onto the public record: the sum of the three delays above,
/// spelled as a sum so the arithmetic is checkable here. The terms are the
/// control plane's, transcribed and pinned by the shared fixture's `meta.txt`
/// (`regenerate_the_shared_fixture` asserts them from both suites), so the
/// relation is held across the boundary rather than by a comment. Named here
/// because this is the half that pays for it: a refresh under
/// [`RekorPolicy::Require`] fails closed for the whole window — the proof set
/// a resolver still holds covers only pre-rotation keys — and the bindings a
/// client already has must outlive it.
pub const CONTROL_PLANE_REPUBLISH_WINDOW: Duration = Duration::from_secs(
    CONTROL_PLANE_WATCH_CADENCE.as_secs()
        + CONTROL_PLANE_LOG_ROUND_TRIP.as_secs()
        + CONTROL_PLANE_PROOF_TTL.as_secs(),
);

/// Extra grace before a binding that vanished from DNS expires (§3.2).
///
/// **The grace alone has to cover [`CONTROL_PLANE_REPUBLISH_WINDOW`], with no
/// help from the TTL.** A binding expires at `<last refresh> + ttl + grace`,
/// and the refresh cadence *is* the TTL — so at the moment a provider starts
/// signing with an un-logged key the TTL term is already spent, and a grace
/// equal to the window alone still drops every DNS-sourced binding on a
/// rotation beginning just before a scheduled refresh. Fifteen minutes is the
/// window plus four minutes of headroom. The cost: a deleted record stays
/// trusted that much longer — which is why deletion is not the design's
/// revocation mechanism (§3.4 is).
pub const DEFAULT_TRUST_GRACE: Duration = Duration::from_secs(15 * 60);

/// The DoH endpoint used when none is configured.
pub const DEFAULT_DOH_URL: &str = "https://1.1.1.1/dns-query";

/// The query name for a membership domain.
pub(crate) fn query_name(domain: &str) -> String {
    format!("{TXT_PREFIX}.{domain}")
}

/// The query name a zone's key-transparency proofs live under (§3): one name
/// per zone at the apex, held between the signing zone and the domain by
/// `apex_of` — the RRSIG signer is the *bound*, not the lookup.
pub(crate) fn rekor_query_name(zone: &str) -> String {
    format!("{}.{}", rekor::REKOR_TXT_PREFIX, zone)
}

/// The label a base's control-plane attach record lives under.
pub(crate) const CP_TXT_PREFIX: &str = "_synchronicity-cp";

/// The version tag a control-plane attach record opens with.
pub(crate) const CP_RECORD_VERSION_TAG: &str = "synccp1";

/// How many attach endpoints one apex may name.
///
/// Each one costs this daemon a standing WebSocket it opens, heartbeats and
/// reconnects for the life of the process, and the number is chosen by the
/// zone rather than by the operator of the node paying for it. A fleet is a
/// primary and its nameservers; eight is well past any of them and far short
/// of a number a hostile or misconfigured zone could make expensive. The
/// control plane refuses to publish more than this
/// (`config.max_browse_endpoints`), so the two ends agree on the bound and
/// an operator hears about it at boot rather than by counting sockets.
pub(crate) const MAX_CP_ENDPOINTS: usize = 8;

/// The query name a base's control-plane attach record lives under.
///
/// One per apex, beside the transparency declaration and the proof set,
/// because it states the same kind of fact — which control plane covers this
/// base — under the same bounds.
pub(crate) fn control_plane_query_name(apex: &str) -> String {
    format!("{CP_TXT_PREFIX}.{apex}")
}

/// Where part `index` of a proof lives (§3). A proof exceeds what a managed
/// provider holds at one owner name (Cloudflare caps one name and type at
/// 8192 wire-format bytes), so parts are spread one per name: part 1 at the
/// base name — the only one derivable before reading anything — and later
/// parts at `_synchronicity-rekor-<index>`. Part 1 says how many there are.
pub(crate) fn rekor_part_query_name(zone: &str, index: usize) -> String {
    match index {
        0 | 1 => rekor_query_name(zone),
        n => format!("{}-{n}.{}", rekor::REKOR_TXT_PREFIX, zone),
    }
}

/// The candidates one refresh will actually verify, in the order they are
/// tried. Capped, because the work is not free: each `rekor::verify` walks a
/// delegation ladder of attacker-chosen RRSIGs, and how many walks one zone
/// can ask for per refresh is set by the zone. A legitimate zone serves one,
/// or two across a rollover.
fn candidates_to_verify(mut candidates: Vec<rekor::RekorProof>) -> Vec<rekor::RekorProof> {
    candidates.truncate(MAX_PROOF_CANDIDATES);
    candidates
}

/// The most reassembled proofs one refresh will verify. Sixteen names of
/// validated TXT reassemble into many candidate groups, and every candidate
/// is a full verification — a Merkle walk, a checkpoint signature, a
/// delegation ladder of RRSIGs the zone chose. A legitimate zone publishes
/// one, or two across a rollover; the rest is work a hostile domain can ask
/// for at the minimum TTL, and this is where it stops.
const MAX_PROOF_CANDIDATES: usize = 4;

/// The control-plane apex a validated membership answer names, checked at
/// both ends: between the domain being resolved and the zone that signed
/// (`<signing zone> ⊇ <apex> ⊇ <membership domain>`). The lower bound stops
/// a record redirecting a client to a control plane for a *sibling*
/// namespace, whose monitor would never watch this one; the upper stops it
/// pointing outside the zone that vouched for the answer.
///
/// **The records of one answer must agree, and one apex is all a client will
/// look under.** The apex is part of the owner name — membership lives at
/// `_synchronicity.<network>.<org>.<apex>` — so records naming two apexes
/// are records at two names, and the cases that look like they need a
/// candidate list (a decommissioned control plane's leftovers, a migration,
/// two control planes in one signing zone) all relocate the owner name with
/// the apex. A zone *hand-authored* into disagreeing is refused rather than
/// tried each way: no amount of guessing resolves it, and trying every
/// reading multiplies the lookups one refresh costs.
///
/// A record naming an *unusable* apex is still only a rejected record — the
/// rule every neighbouring reader applies, and the realistic cause is a
/// member editing their own dialing hint.
fn apex_of(domain: &str, signing_zone: &Name, records: &[String]) -> Result<Name, NetError> {
    let mut owner = Name::from_utf8(domain).map_err(|e| NetError::Dns(format!("{domain}: {e}")))?;
    owner.set_fqdn(true);
    let owner = owner.to_lowercase();

    let mut named: Option<Name> = None;
    for text in records
        .iter()
        .filter_map(|record| parse_record(record).ok())
        .filter_map(|record| record.apex)
    {
        let Ok(mut apex) = Name::from_utf8(&text) else {
            continue;
        };
        apex.set_fqdn(true);
        let apex = apex.to_lowercase();
        if !apex.zone_of(&owner) || !signing_zone.zone_of(&apex) {
            continue;
        }
        match &named {
            Some(first) if *first != apex => {
                return Err(NetError::Dns(format!(
                    "{domain}: its records name two control-plane apexes, {first} \
                     and {apex} — one answer is covered by one control plane, and \
                     which of them this is cannot be guessed"
                )))
            }
            _ => named = Some(apex),
        }
    }
    named.ok_or_else(|| {
        NetError::Dns(format!(
            "{domain}: no membership record names an apex= inside {signing_zone} \
             that contains it, so there is nowhere to look for its transparency \
             records"
        ))
    })
}

/// Clamps a TTL into the §3.2 window.
pub fn clamp_ttl(ttl: Duration) -> Duration {
    ttl.clamp(MIN_TTL, MAX_TTL)
}

/// One parsed `v=sync1` TXT record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MemberRecord {
    /// The member label, lowercased. `None` for the id-less backward-simple
    /// form, which binds `OriginId::Key(nk)`.
    pub id: Option<String>,
    /// The device key the record binds.
    pub node_key: NodeId,
    /// An optional relay dialing hint (§3.3).
    pub relay: Option<String>,
    /// An optional direct-address dialing hint (§3.3).
    pub addr: Option<String>,
    /// The control plane this record's zone belongs to, as the operator
    /// names it — where its transparency records live. A *hint about where
    /// to look*, never an authority: `apex_of` bounds it at both ends and
    /// `rekor::verify` bounds the entry found under it the same way, so a
    /// wrong value points at a name with no usable proof and fails closed.
    /// Its purpose is to let two control planes share one signing zone
    /// without sharing a record name.
    pub apex: Option<String>,
}

impl MemberRecord {
    /// The origin this record binds its key to, within `domain`.
    pub(crate) fn origin(&self, domain: &str) -> Result<OriginId, NetError> {
        match &self.id {
            Some(id) => OriginId::named(id, domain).map_err(|e| NetError::Dns(e.to_string())),
            None => Ok(OriginId::Key(self.node_key)),
        }
    }
}

/// Why a TXT record was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RecordError {
    /// The record did not start with `v=sync1`.
    #[error("not a v=sync1 record")]
    NotSync1,
    /// The record had no `nk=` field.
    #[error("record has no nk= field")]
    MissingKey,
    /// The `nk=` field was not a valid z-base-32 device key.
    #[error("invalid nk= device key: {0}")]
    BadKey(String),
    /// The `id=` field was not a valid member label.
    #[error("invalid id= label: {0}")]
    BadLabel(String),
    /// A field appeared more than once.
    #[error("duplicate field {0}=")]
    Duplicate(&'static str),
}

/// Parses one TXT record string.
///
/// Fields are whitespace-separated `key=value` pairs. `v=sync1` must come
/// first; unknown fields are ignored so the format can grow.
pub(crate) fn parse_record(text: &str) -> Result<MemberRecord, RecordError> {
    let mut fields = text.split_whitespace();
    match fields.next() {
        Some(first) if first == format!("v={RECORD_VERSION_TAG}") => {}
        _ => return Err(RecordError::NotSync1),
    }

    let mut id = None;
    let mut node_key = None;
    let mut relay = None;
    let mut addr = None;
    let mut apex = None;
    for field in fields {
        let Some((key, value)) = field.split_once('=') else {
            continue;
        };
        match key {
            "id" => {
                if id.is_some() {
                    return Err(RecordError::Duplicate("id"));
                }
                id = Some(
                    normalize_label(value).map_err(|_| RecordError::BadLabel(value.to_string()))?,
                );
            }
            "nk" => {
                if node_key.is_some() {
                    return Err(RecordError::Duplicate("nk"));
                }
                node_key = Some(
                    PublicKey::from_z32(value)
                        .map_err(|e| RecordError::BadKey(format!("{value}: {e}")))?,
                );
            }
            "relay" => relay = Some(value.to_string()),
            "addr" => addr = Some(value.to_string()),
            // Rejected on duplication, like `nk` and `id`: a record that says
            // two things about its control plane says nothing, and every other
            // field this parser turns on is read that way. (Nothing security
            // rests on it — the control plane validates hints before it renders
            // them, and `apex_of` bounds the apex at both ends regardless —
            // it is one parser reading one format one way.)
            "apex" => {
                if apex.is_some() {
                    return Err(RecordError::Duplicate("apex"));
                }
                apex = Some(value.to_ascii_lowercase());
            }
            _ => {}
        }
    }

    Ok(MemberRecord {
        id,
        node_key: node_key.ok_or(RecordError::MissingKey)?,
        relay,
        addr,
        apex,
    })
}

/// The attach endpoint one control plane publishes at its apex.
///
/// A deployment fact, never a policy: it says *this base's control plane
/// attaches here*, and nothing about which network may be browsed. Which
/// network may is decided at the endpoint itself, where a refusal is immediate
/// rather than a TTL away.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlPlaneRecord {
    /// The base URL a daemon opens its attach connection against.
    pub url: String,
}

/// Why a control-plane attach record was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum CpRecordError {
    /// The record did not start with `v=synccp1`.
    #[error("not a v=synccp1 record")]
    NotSyncCp1,
    /// The record had no `url=` field.
    #[error("record has no url= field")]
    MissingUrl,
    /// The `url=` field was not an `https://` or `http://` origin.
    ///
    /// A shape refusal, not a transport policy: what makes the redirect target
    /// safe to follow is the zone-key gate above it, not the scheme — the value
    /// is one the logged, DNSSEC-validated zone chose to publish. `https://` is
    /// what a zone reachable from the open internet should publish, since the
    /// attach connection carries a device-key proof; `http://` is for endpoints
    /// whose transport is guarded another way, like a loopback testnet.
    #[error("url= must be an https:// or http:// endpoint: {0}")]
    BadUrl(String),
    /// A field appeared more than once.
    #[error("duplicate field {0}=")]
    Duplicate(&'static str),
}

/// Parses one `v=synccp1` TXT record.
///
/// Fields are whitespace-separated `key=value` pairs, `v=synccp1` first, and
/// unknown fields are ignored — the same grammar and the same growth rule as
/// the membership record beside it.
pub(crate) fn parse_control_plane_record(text: &str) -> Result<ControlPlaneRecord, CpRecordError> {
    let mut fields = text.split_whitespace();
    match fields.next() {
        Some(first) if first == format!("v={CP_RECORD_VERSION_TAG}") => {}
        _ => return Err(CpRecordError::NotSyncCp1),
    }
    let mut url = None;
    for field in fields {
        let Some((key, value)) = field.split_once('=') else {
            continue;
        };
        if key == "url" {
            if url.is_some() {
                return Err(CpRecordError::Duplicate("url"));
            }
            url = Some(value.to_string());
        }
    }
    let url = url.ok_or(CpRecordError::MissingUrl)?;
    let trimmed = url.trim_end_matches('/');
    let has_origin = ["https://", "http://"]
        .iter()
        .any(|scheme| trimmed.starts_with(scheme) && trimmed.len() > scheme.len());
    if !has_origin {
        return Err(CpRecordError::BadUrl(url));
    }
    Ok(ControlPlaneRecord {
        url: trimmed.to_string(),
    })
}

/// One dialing hint a membership record published (§3.3), with the field it
/// came from. Typed because the two fields do not mean the same thing and
/// only this parser knows which is which: `relay=` names a URL to make
/// outbound requests to, `addr=` an address to dial directly. Flattened, a
/// `relay=` value shaped like `ip:port` is indistinguishable from an `addr=`
/// value downstream — however carefully a consumer validates *shape*, it
/// cannot enforce *meaning*; the field name is the only place it exists.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum DialHint {
    /// A `relay=` value: where to reach the key through a relay.
    Relay(String),
    /// An `addr=` value: a direct address for the key.
    Addr(String),
}

impl DialHint {
    /// The value as the record spelled it, whichever field it was.
    pub fn value(&self) -> &str {
        match self {
            DialHint::Relay(text) | DialHint::Addr(text) => text,
        }
    }
}

/// The validated membership set for one domain.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemberSet {
    /// The membership domain, normalized.
    pub domain: String,
    /// The accepted `(origin, device key)` bindings.
    pub bindings: Vec<(OriginId, NodeId)>,
    /// Device keys that appear under more than one identity. Self-detection
    /// refuses to guess for these, and `synch doctor` reports the ambiguity
    /// (§3.2).
    pub ambiguous_keys: Vec<NodeId>,
    /// Records that could not be parsed, with the reason, for `synch doctor`.
    pub rejected: Vec<(String, RecordError)>,
    /// Dialing hints, by device key (§3.3).
    pub hints: BTreeMap<[u8; 32], Vec<DialHint>>,
}

impl MemberSet {
    /// Applies the §3.2 record and malformed-set rules to a batch of TXT
    /// strings that have *already* been DNSSEC-validated.
    pub fn from_records(domain: &str, records: &[String]) -> Result<MemberSet, NetError> {
        let domain = normalize_domain(domain).map_err(|e| NetError::Dns(e.to_string()))?;
        let mut parsed = Vec::new();
        let mut rejected = Vec::new();
        for text in records {
            match parse_record(text) {
                Ok(record) => parsed.push(record),
                Err(e) => rejected.push((text.clone(), e)),
            }
        }

        // Malformed-set rule: if the same nk appears under two different ids —
        // or once with and once without an id — the key is ambiguous and every
        // binding it would create is dropped.
        let mut identities: BTreeMap<[u8; 32], BTreeSet<Option<String>>> = BTreeMap::new();
        for record in &parsed {
            identities
                .entry(*record.node_key.as_bytes())
                .or_default()
                .insert(record.id.clone());
        }
        let ambiguous: BTreeSet<[u8; 32]> = identities
            .iter()
            .filter(|(_, ids)| ids.len() > 1)
            .map(|(key, _)| *key)
            .collect();

        let mut bindings = Vec::new();
        let mut hints: BTreeMap<[u8; 32], Vec<DialHint>> = BTreeMap::new();
        for record in &parsed {
            let key_bytes = *record.node_key.as_bytes();
            // Nothing is harvested from a record whose key is ambiguous. The
            // rule above says every binding such a key would create is dropped,
            // and a dialing hint is part of what the record creates: collecting
            // hints first made one added record both drop a member's binding and
            // name where to reach that member's key, which is a record that was
            // refused deciding an outbound connection.
            if ambiguous.contains(&key_bytes) {
                continue;
            }
            for hint in [
                record
                    .relay
                    .as_ref()
                    .map(|text| DialHint::Relay(text.clone())),
                record
                    .addr
                    .as_ref()
                    .map(|text| DialHint::Addr(text.clone())),
            ]
            .into_iter()
            .flatten()
            {
                hints.entry(key_bytes).or_default().push(hint);
            }
            let origin = record.origin(&domain)?;
            let binding = (origin, record.node_key);
            if !bindings.contains(&binding) {
                bindings.push(binding);
            }
        }
        bindings.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.as_bytes().cmp(b.1.as_bytes())));

        let mut ambiguous_keys: Vec<NodeId> = Vec::new();
        for key in &ambiguous {
            if let Ok(k) = PublicKey::from_bytes(key) {
                ambiguous_keys.push(k);
            }
        }

        Ok(MemberSet {
            domain,
            bindings,
            ambiguous_keys,
            rejected,
            hints,
        })
    }

    /// Every device key bound to `origin` in this set. Several keys for one
    /// origin is the rotation window (§3.4), not an error. Production reads
    /// `bindings` directly; the tests state their expectations through this.
    #[cfg(test)]
    pub(crate) fn keys_for(&self, origin: &OriginId) -> Vec<NodeId> {
        self.bindings
            .iter()
            .filter(|(o, _)| o == origin)
            .map(|(_, k)| *k)
            .collect()
    }

    /// The origin a device key resolves to, for self-detection.
    ///
    /// Returns `None` when the key is absent *or* ambiguous: §3.1 adopts a
    /// single unambiguous answer and never guesses at one.
    pub fn self_origin(&self, key: &NodeId) -> Option<OriginId> {
        if self.ambiguous_keys.contains(key) {
            return None;
        }
        let matches: Vec<&OriginId> = self
            .bindings
            .iter()
            .filter(|(_, k)| k == key)
            .map(|(o, _)| o)
            .collect();
        match matches.as_slice() {
            [only] => Some((*only).clone()),
            _ => None,
        }
    }

    /// Dialing hints published for a device key (§3.3). Production reads
    /// `hints` directly; the tests state their expectations through this.
    #[cfg(test)]
    pub(crate) fn hints_for(&self, key: &NodeId) -> &[DialHint] {
        self.hints
            .get(key.as_bytes())
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

/// Parses and normalizes a DoH endpoint URL.
///
/// `https://` or `http://`, host required, query path defaulting to
/// `/dns-query`. Plaintext concedes what UDP-53 always conceded: query
/// privacy and a denial lever. It concedes integrity only as far as the
/// validation below it is sound — and that is a real qualifier, not a
/// formality: a party who can add a record to a response is answered by
/// [`covered_by_signed_data`] and nothing else. Prefer `https://` and treat
/// the in-process validation as the last line rather than the only one.
fn doh_url(url: &str) -> Result<reqwest::Url, NetError> {
    let bad = |why: String| NetError::Dns(format!("DoH endpoint {url}: {why}"));
    let mut parsed = reqwest::Url::parse(url).map_err(|e| bad(e.to_string()))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(bad("must be an https:// or http:// URL".into()));
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err(bad("no host".into()));
    }
    if matches!(parsed.path(), "" | "/") {
        parsed.set_path("/dns-query");
    }
    Ok(parsed)
}

/// A DNSSEC-validating resolver for membership domains.
///
/// One transport, DNS-over-HTTP(S), and one validator: every answer record
/// is required to carry a *secure* DNSSEC proof computed in process, so an
/// insecure or bogus answer is discarded rather than trusted. There is no
/// UDP path and no system stub resolver in the loop at all — hickory is
/// here purely as the validation engine.
#[derive(Clone)]
pub struct DnssecResolver {
    handle: hickory_resolver::net::dnssec::DnssecDnsHandle<DohHandle>,
    /// The DNSSEC trust anchors in force — the ICANN root, or whatever
    /// `--dnssec-anchor` replaced it with. Held here as well as inside the
    /// validating handle because the DNSSEC chain a log entry carries is
    /// checked against the same anchors the live answers are.
    anchors: std::sync::Arc<TrustAnchors>,
    rekor: RekorPolicy,
    pins: std::sync::Arc<std::sync::Mutex<Pins>>,
    tuf: Option<TufSource>,
}

/// The transparency-log pin set in force, and whether TUF may move it
/// (§10.2's resolution order). An explicit `--rekor-key` file is a static,
/// different universe — TUF refresh disabled entirely, nothing walked;
/// otherwise the pins are the last TUF-verified set, else the embedded
/// bootstrap snapshot.
#[derive(Debug)]
enum Pins {
    /// `--rekor-key` named a file. Nothing refreshes this.
    Static(LogKeys),
    /// The refreshable set: what is in force, the TUF state it came from,
    /// and where that state is persisted (`None` keeps it in memory, which
    /// is what a one-shot command or a test wants).
    Tuf {
        keys: LogKeys,
        /// Boxed: a TUF state is an order of magnitude wider than the static
        /// variant, and this enum is cloned on every read of the pin set.
        state: Box<PinState>,
        path: Option<std::path::PathBuf>,
        config: Option<std::sync::Arc<synch_store::Store>>,
        /// The `root.json` a persisted state must name as the repository it
        /// was accumulated under — [`tuf::EMBEDDED_TUF_ROOT`] unless
        /// `--tuf-root` replaced it. Held so a reload cannot anchor at
        /// something the file itself supplied.
        anchor: Vec<u8>,
        /// When the repository was last walked, so a membership refresh on
        /// a short TTL is not a request to Sigstore's CDN every time it
        /// fires. Seeded from the persisted state's `updated_at`, so a
        /// restart does not reset the clock — but *not* persisted on
        /// failure, so an unreachable repository is retried next start.
        checked_at: u64,
    },
}

impl Pins {
    /// The pin set a proof is checked against right now.
    fn log_keys(&self) -> LogKeys {
        match self {
            Pins::Static(keys) => keys.clone(),
            Pins::Tuf { keys, .. } => keys.clone(),
        }
    }
}

/// Reads persisted pin state, saying so when a file is there and unusable.
///
/// Falling back to the build-time pins is the right direction — a client
/// that refused to start over a damaged cache would be the availability
/// coupling §10.2 forbids — but it discards every update ever accepted, so
/// it is not silent: "no file" is a fresh install and unremarkable; "present
/// and did not load" is a truncated write, an old format, or a `--tuf-root`
/// pointed at a different repository, and an operator has to see it.
fn load_pin_state(path: &Path, anchor: &[u8]) -> Option<PinState> {
    match PinState::load_anchored(path, anchor) {
        Some(state) => Some(state),
        None => {
            if path.exists() {
                tracing::warn!(
                    path = %path.display(),
                    "the transparency pin state was not loaded; starting from the \
                     embedded bootstrap pins and re-learning on the next walk"
                );
            }
            None
        }
    }
}

fn load_config_pin_state(store: &synch_store::Store, anchor: &[u8]) -> Option<PinState> {
    const KEY: &str = "rekor.pin_state";
    match store.config(KEY) {
        Ok(Some(text)) => match PinState::decode_anchored(&text, anchor) {
            Some(state) => Some(state),
            None => {
                tracing::warn!(
                    "the transparency pin state in SQLite was not loaded; starting from the \
                     embedded bootstrap pins and re-learning on the next walk"
                );
                None
            }
        },
        Ok(None) => None,
        Err(error) => {
            tracing::warn!(%error, "could not read transparency pin state from SQLite");
            None
        }
    }
}

/// Where the pin set is refreshed from (§10.2). `None` — `--no-tuf`, or an
/// explicit `--rekor-key` making the pin set static — is a state with no
/// repository in it at all, not a repository that answers nothing.
#[derive(Clone)]
enum TufSource {
    /// Sigstore's TUF repository, read over HTTPS. The URL is not trusted
    /// with anything — every byte fetched under it is checked against the
    /// embedded root — so this is a mirror knob, not a trust knob.
    Url(String),
    /// A repository supplied by the caller, so the walk is exercisable
    /// without egress. Tests use it; nothing else does.
    Repo(std::sync::Arc<dyn Repo + Send + Sync>),
}

impl std::fmt::Debug for DnssecResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DnssecResolver")
    }
}

/// Which apex a lookup is gated against, and where that apex comes from. A
/// membership answer carries the `apex=` field naming its own bound; a
/// record hanging off that apex carries none and must inherit the bound from
/// the answer that led to it. Naming the case means a new subordinate lookup
/// cannot quietly acquire the *first* behaviour and choose the apex it is
/// judged against.
#[derive(Debug, Clone, Copy)]
enum GateApex<'a> {
    /// Derived from this answer's own records — the membership case.
    FromRecords,
    /// Fixed by an apex a gated answer already established.
    Under(&'a Name),
}

/// A DNSSEC-validated answer that has **also** passed the §4.2 transparency
/// gate — or that is under a policy which asks for no gate at all.
///
/// Distinct from [`DnssecTxt`] because the two are different statements.
/// `DnssecTxt` says *this zone signed this*, which
/// under `RekorPolicy::Require` is not enough: the threat model's attacker is
/// a compromised parent who substitutes a DS, adds a key to the apex DNSKEY
/// RRset, and signs anything they like with a key that validates. What the
/// gate adds is that the key which signed *this RRset* is on the transparency
/// log, so using it leaves a record a monitor can see.
///
/// The private field is the mechanism, as with
/// [`chain::Authorized`](crate::chain::Authorized): only
/// `DnssecResolver::gated_txt` can build one, so a value of this type is
/// evidence the gate ran.
#[derive(Debug, Clone)]
pub(crate) struct GatedTxt {
    /// The validated answer itself.
    answer: DnssecTxt,
}

impl GatedTxt {
    /// The TXT strings, safe to act on.
    pub(crate) fn records(&self) -> &[String] {
        &self.answer.records
    }

    /// How long the answer may be believed, already held to the signature's
    /// own expiration.
    pub(crate) fn ttl(&self) -> Duration {
        self.answer.ttl
    }

    /// The zone whose RRSIG covered the answer.
    pub(crate) fn signer(&self) -> &Name {
        &self.answer.signer
    }
}

/// A DNSSEC-validated lookup result, and **nothing more than that**: the
/// signing zone signed these records and hickory's chain to the trust anchor
/// held. It says nothing about whether the key that did so is on the
/// transparency log, which under [`RekorPolicy::Require`] is the question
/// that matters — a trust decision belongs on `GatedTxt`. Named for the
/// check it *did* pass rather than for "validated", the previous name, which
/// read at every binding site as though the answer were finished being
/// checked.
#[derive(Debug, Clone)]
pub struct DnssecTxt {
    /// The TXT strings, one per record.
    pub records: Vec<String>,
    /// How long the answer may be cached: the §3.2 window, never past the
    /// point where the RRSIG covering it stops being valid.
    ///
    /// The second bound is what keeps a replay from *renewing* trust: a
    /// correctly signed answer stays signed for its whole RRSIG window —
    /// days to weeks — the proof covers the zone *key* rather than the
    /// record set, and the transport is untrusted, so a replay whose TTL was
    /// taken at face value would push a deleted member's binding out afresh
    /// on every refresh. Capped at the signature's own expiration, each
    /// replay buys less than the last and the total is bounded by a lifetime
    /// the zone signed; past it hickory refuses the answer outright. Inside
    /// the final [`MIN_TTL`] of an RRSIG's life every replay buys the same
    /// 60 s, so the last binding outlives the signature by `MIN_TTL +
    /// DEFAULT_TRUST_GRACE` — sixteen minutes past a window the zone chose
    /// in weeks, the honest bound.
    pub ttl: Duration,
    /// The zone whose RRSIG covered this answer, as the signature named it —
    /// checked to enclose the queried name before the answer was accepted
    /// (see `secure_txt`).
    pub signer: Name,
    /// The key tag that RRSIG selected. A selector, not an identity — the
    /// live key is [`Self::rrsig`] verified against the DNSKEY set.
    pub key_tag: u16,
    /// The membership RRSIG itself, kept so the live DNSKEY can be identified
    /// by actually verifying this signature rather than by 16-bit tag.
    pub rrsig: RRSIG,
    /// The TXT RRset that `rrsig` covers, for that re-verification.
    pub txt_rrset: Vec<Record>,
    /// When the signature over this answer stops being valid, seconds since
    /// the epoch — the ceiling on how long anything derived from the answer
    /// may be believed. See [`Self::ttl`].
    pub expires_at: u64,
}

/// How the resolver reaches the DNS and whom it ultimately trusts (§3.2).
///
/// The defaults — the public endpoint at [`DEFAULT_DOH_URL`], the ICANN root
/// trust anchor — are what production wants; both knobs exist for the
/// environments where they are wrong: an internal DoH endpoint (http or
/// https) for closed networks, and an internal test zone signed by its own
/// root swapping the trust anchor. Neither weakens the §3.2 stance:
/// validation happens in process against whatever anchor is in force, and
/// the endpoint is a transport, never a validator we defer to.
#[derive(Debug, Clone, Default)]
pub struct ResolverOptions {
    /// The DNS-over-HTTP(S) endpoint, [`DEFAULT_DOH_URL`] when unset:
    /// `https://` or `http://`, then `host[:port][/path]` (path defaults to
    /// `/dns-query`). A plaintext endpoint is acceptable because the
    /// transport carries nothing trusted — every answer is DNSSEC-validated
    /// in process either way, so http concedes query privacy and a denial
    /// lever, nothing about integrity.
    pub doh_url: Option<String>,
    /// A file of `DNSKEY` records (zone syntax, as `dig` prints them)
    /// *replacing* the ICANN root trust anchor, for internal deployments and
    /// tests that run their own signed root. With this set nothing signed
    /// under the real root validates any more: an override is a different
    /// universe, not an addition to this one.
    pub trust_anchor: Option<std::path::PathBuf>,
    /// Whether a validated answer additionally requires a verified
    /// transparency-log record for the zone key that signed it (§4.1).
    /// Three-state on purpose: `None` takes the default
    /// [`ResolverOptions::rekor_policy`] resolves — require, everywhere —
    /// while an explicit `--rekor` states a choice, and `off` is a choice
    /// this design wants stated, never inherited.
    pub rekor: Option<RekorPolicy>,
    /// A file of transparency-log verification key(s) *replacing* the
    /// embedded one (§4.1): PEM `PUBLIC KEY` blocks or one base64
    /// SubjectPublicKeyInfo per line; a self-hosted log lives here. Same
    /// "different universe" semantics as `trust_anchor` — and setting it
    /// also disables TUF pin refresh outright (§10.2).
    pub rekor_key: Option<std::path::PathBuf>,
    /// Where the TUF-verified pin set is persisted (§10.2); `None` keeps it
    /// in memory, which is what a one-shot command or a test wants. The file
    /// is global across domains and monotonic on purpose — the pin set
    /// belongs to Sigstore, not to any domain being resolved.
    pub rekor_state: Option<std::path::PathBuf>,
    /// SQLite config store used by a daemon so pin state rides its database
    /// replica. Takes precedence over the legacy file when both are set.
    pub rekor_config: Option<std::sync::Arc<synch_store::Store>>,
    /// The Sigstore TUF repository the pin set follows,
    /// [`tuf::SIGSTORE_TUF_URL`] when unset (§10.2). A mirror knob rather
    /// than a trust knob: whatever it names, everything fetched under it is
    /// verified against the embedded TUF root before anything moves. A
    /// deployment running its *own* Sigstore points this at its repository
    /// and `rekor_key` at its log.
    pub tuf_url: Option<String>,
    /// Turns pin refresh off, leaving the client on the pins it already has
    /// — the persisted set, else the embedded bootstrap snapshot — for a
    /// deployment that will not have its daemon reach a CDN. The cost (§10.4):
    /// the pin set stops following Sigstore, so the day a shard rotates is
    /// the day this client needs a new build.
    pub no_tuf: bool,
    /// A `root.json` *replacing* [`tuf::EMBEDDED_TUF_ROOT`] as the anchor
    /// every pin state records itself against — the same "an override is a
    /// different universe" semantics as `trust_anchor` and `rekor_key`: with
    /// this set, a persisted state chaining from the built-in Sigstore root
    /// no longer loads, and vice versa. For a deployment running its own TUF
    /// repository — the client counterpart of `CP_TUF_ROOT` — and for the
    /// test harness, which anchors at a root it minted.
    pub tuf_root: Option<std::path::PathBuf>,
}

/// Whether zone-key transparency is enforced (§4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RekorPolicy {
    /// A validated answer is discarded unless the zone key that signed it
    /// carries a verified log record. Fail closed, as §4.3 requires.
    Require,
    /// No log record is consulted. DNSSEC alone decides, as it did before
    /// this design existed.
    Off,
}

impl ResolverOptions {
    /// The policy in force, resolving the default when none was chosen.
    ///
    /// The default is `require`, everywhere: the embedded Sigstore snapshot
    /// means a stock build can always verify, and a zone key that is not on
    /// the public record is exactly what this design exists to refuse. That
    /// holds behind `trust_anchor` too — a pinned anchor closes the
    /// delegation chain to substitution, but the log requirement is about
    /// the key being *public* — and an internal deployment that wants
    /// neither says `off` in so many words, never inheriting it from an
    /// unrelated flag.
    pub fn rekor_policy(&self) -> RekorPolicy {
        self.rekor.unwrap_or(RekorPolicy::Require)
    }
}

impl DnssecResolver {
    /// Builds a resolver with every default: the public endpoint, the ICANN
    /// root trust anchor.
    pub fn with_defaults() -> Result<Self, NetError> {
        Self::with_options(&ResolverOptions::default())
    }

    /// Builds a resolver honoring [`ResolverOptions`], with in-process DNSSEC
    /// validation always on.
    pub fn with_options(options: &ResolverOptions) -> Result<Self, NetError> {
        let anchors = match &options.trust_anchor {
            None => std::sync::Arc::new(TrustAnchors::default()),
            Some(path) => {
                let anchors = TrustAnchors::from_file(path)
                    .map_err(|e| NetError::Dns(format!("trust anchor {}: {e}", path.display())))?;
                if anchors.is_empty() {
                    // An empty anchor set validates nothing, forever, quietly.
                    return Err(NetError::Dns(format!(
                        "trust anchor {}: no DNSKEY records in the file",
                        path.display()
                    )));
                }
                std::sync::Arc::new(anchors)
            }
        };
        let url = doh_url(options.doh_url.as_deref().unwrap_or(DEFAULT_DOH_URL))?;
        let handle = DohHandle::new(url)?;
        // §10.2's pin resolution order, decided once: key file (static
        // universe) ▸ persisted TUF-accepted set ▸ embedded bootstrap.
        let pins = match &options.rekor_key {
            Some(path) => {
                Pins::Static(LogKeys::from_file(path).map_err(|e| NetError::Dns(e.to_string()))?)
            }
            None => {
                // The anchor comes from the binary or an explicit override,
                // never from the state file — which makes the state's
                // recorded anchor a check on where it came from.
                let anchor = match &options.tuf_root {
                    None => tuf::EMBEDDED_TUF_ROOT.as_bytes().to_vec(),
                    Some(path) => std::fs::read(path)
                        .map_err(|e| NetError::Dns(format!("TUF root {}: {e}", path.display())))?,
                };
                let state = options
                    .rekor_config
                    .as_deref()
                    .and_then(|store| load_config_pin_state(store, &anchor))
                    .or_else(|| {
                        options
                            .rekor_state
                            .as_deref()
                            .and_then(|path| load_pin_state(path, &anchor))
                    })
                    .unwrap_or_else(|| PinState::anchored(&anchor));
                Pins::Tuf {
                    keys: state.log_keys().unwrap_or_else(LogKeys::embedded),
                    anchor,
                    // A walk is due when the last has aged out; a state
                    // never written has never walked — a fresh install
                    // refreshes at once, a restarted one does not.
                    checked_at: state.updated_at,
                    state: Box::new(state),
                    path: options.rekor_state.clone(),
                    config: options.rekor_config.clone(),
                }
            }
        };
        let tuf = match (options.no_tuf, &options.rekor_key) {
            (true, _) | (_, Some(_)) => None,
            (false, None) => Some(TufSource::Url(
                options
                    .tuf_url
                    .clone()
                    .unwrap_or_else(|| tuf::SIGSTORE_TUF_URL.to_string()),
            )),
        };
        Ok(DnssecResolver {
            handle: hickory_resolver::net::dnssec::DnssecDnsHandle::with_trust_anchor(
                handle,
                anchors.clone(),
            ),
            // The same anchor set the live validator uses, kept so that the
            // DNSSEC chain a log entry carries is checked against the trust
            // this resolver actually holds — "an override is a different
            // universe" has to hold in both directions, or a client running
            // `--dnssec-anchor` would demand a chain to the ICANN root it
            // does not trust.
            anchors,
            rekor: options.rekor_policy(),
            pins: std::sync::Arc::new(std::sync::Mutex::new(pins)),
            tuf,
        })
    }

    /// Refreshes the pin set from `repo` instead of over HTTPS.
    ///
    /// The one seam the walk needs to be exercisable without egress, so no
    /// test run ever reaches Sigstore by accident. Has no effect when the
    /// pin set is static (`--rekor-key`) or refresh is off.
    pub fn with_tuf_repo(mut self, repo: std::sync::Arc<dyn Repo + Send + Sync>) -> Self {
        self.tuf = self.tuf.map(|_| TufSource::Repo(repo));
        self
    }

    /// Whether this resolver requires zone-key transparency (§4.1).
    pub fn rekor_policy(&self) -> RekorPolicy {
        self.rekor
    }

    /// The transparency-log keys a proof is checked against right now.
    ///
    /// A snapshot rather than a handle: under [`RekorPolicy::Require`] a TUF
    /// update can replace this between refreshes, which is the whole point
    /// of §10.
    pub fn log_keys(&self) -> LogKeys {
        self.pins().log_keys()
    }

    fn pins(&self) -> std::sync::MutexGuard<'_, Pins> {
        self.pins
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Whether the last *successful* pin refresh is old enough that a failure
    /// is a standing condition rather than one bad walk.
    ///
    /// A client that has never accepted an update reads as overdue, and that is
    /// the intent: it is running on the pin set its build shipped with, which is
    /// the state a refreshable pin set exists to leave behind.
    fn pin_refresh_overdue(&self) -> bool {
        let Pins::Tuf { state, .. } = &*self.pins() else {
            return false;
        };
        let updated_at = state.updated_at;
        now_unix(updated_at)
            .is_some_and(|now| now > updated_at.saturating_add(PIN_REFRESH_STALE_AFTER))
    }

    /// Resolves `_synchronicity.<domain> TXT`, discarding anything that does
    /// not validate — and **stopping there**. The name is deliberately
    /// unpleasant: what comes back is DNSSEC-valid and nothing else — *the
    /// zone* said this, never that a *logged* zone key did — and under
    /// [`RekorPolicy::Require`] those two differ by precisely the attacker
    /// the transparency log exists to catch: a compromised parent who
    /// substitutes a DS, adds a key to the apex DNSKEY RRset, and signs
    /// whatever they like with a key that validates.
    ///
    /// **No trust decision may be made from this.** `gated_txt` is the one
    /// way to get an answer that may be acted on, and it returns a
    /// `GatedTxt` so that "has this been gated?" is a question the type
    /// system answers. No production caller reaches this at all; it exists
    /// so `synch doctor` and the tests can see the answer a resolver took
    /// *before* anything judged it — a diagnostic, not an input.
    pub async fn lookup_txt_ungated(&self, domain: &str) -> Result<DnssecTxt, NetError> {
        let domain = normalize_domain(domain).map_err(|e| NetError::Dns(e.to_string()))?;
        let name = query_name(&domain);
        let response = self.lookup(&name, RecordType::TXT).await?;
        self.validated_txt(&name, &response.answers)
    }

    /// Fetches one TXT RRset and takes it all the way to trustworthy: DNSSEC
    /// validation, then the §4.2 transparency gate against an apex.
    ///
    /// **This is the only way to obtain a [`GatedTxt`], and every trust
    /// decision in this module is made from one** — the whole point of the
    /// separate type.
    ///
    /// The TUF refresh happens here rather than at each site; its young-walk
    /// check makes the second call in a pass a no-op, so routing a
    /// subordinate lookup through the full gate costs a policy comparison.
    async fn gated_txt(
        &self,
        domain: &str,
        name: &str,
        apex: GateApex<'_>,
    ) -> Result<GatedTxt, NetError> {
        let response = self.lookup(name, RecordType::TXT).await?;
        let validated = self.validated_txt(name, &response.answers)?;
        if self.rekor != RekorPolicy::Require {
            // Off and Prefer make no demand of the signer, so there is no apex
            // to derive and no proof to read. The answer is DNSSEC-validated
            // and that is the whole of what this policy asked for.
            return Ok(GatedTxt { answer: validated });
        }
        let apex = match apex {
            // A membership answer names its own apex, and `apex_of` holds it
            // between the signing zone and the domain. Refusing an answer that
            // names none or names two happens there, so what comes back is a
            // single bounded name rather than a claim about a loop.
            GateApex::FromRecords => apex_of(domain, &validated.signer, &validated.records)?,
            // A record hanging off an apex a gated answer already established.
            // Its own RRset carries no `apex=` — only the membership record
            // does — so the bound comes from the answer that led here.
            GateApex::Under(apex) => apex.clone(),
        };
        // The pins are refreshed *before* the proof is verified, so a proof
        // from a shard Sigstore added since this build shipped verifies in the
        // same refresh that learned about it (§10.2) — and never fatally to the
        // refresh, because a client that cannot read Sigstore degrades to a
        // frozen pin set rather than a failed cluster.
        match self.refresh_tuf().await {
            Ok(Some(update)) if update.changed => tracing::info!(
                root = update.state.root_version,
                timestamp = update.state.timestamp_version,
                logs = update.log_keys.keys().len(),
                "transparency-log pin set updated from Sigstore's TUF repository"
            ),
            Ok(_) => {}
            Err(e) if self.pin_refresh_overdue() => tracing::warn!(
                error = %e,
                stale_after = PIN_REFRESH_STALE_AFTER,
                "Sigstore's TUF repository has not updated the pin set in far \
                 longer than the refresh interval; the pins in force are frozen"
            ),
            Err(e) => tracing::debug!(
                error = %e,
                "Sigstore's TUF repository did not update the pin set; the current pins stand"
            ),
        }
        self.verify_zone_key(domain, &apex, &validated).await?;
        Ok(GatedTxt { answer: validated })
    }

    /// Applies the §3.2 acceptance rules to one answer, holding the TTL it
    /// yields to the signature's own expiration.
    fn validated_txt(
        &self,
        name: &str,
        answers: &[hickory_resolver::proto::rr::Record],
    ) -> Result<DnssecTxt, NetError> {
        secure_txt(name, answers, now_unix(0))
    }

    /// Resolves and applies the §3.2 rules in one step.
    ///
    /// Under [`RekorPolicy::Require`] this is where §4.2's three validated
    /// lookups happen, over the one DoH transport: the membership TXT, the
    /// DNSKEY at the zone its RRSIG names, and the proof record beside it.
    /// A refused proof refuses the whole answer — the caller keeps its
    /// cached member set until its own expiry, exactly as for a bogus chain.
    pub async fn member_set(&self, domain: &str) -> Result<(MemberSet, Duration), NetError> {
        let domain = normalize_domain(domain).map_err(|e| NetError::Dns(e.to_string()))?;
        let name = query_name(&domain);
        // Fetch, validate and gate in one step (§4.2). The membership answer
        // names the apex it is judged against, which is why this leg is
        // `FromRecords` and the attach record's is not.
        let gated = self
            .gated_txt(&domain, &name, GateApex::FromRecords)
            .await?;
        let set = MemberSet::from_records(&domain, gated.records())?;
        Ok((set, gated.ttl()))
    }

    /// Resolves the control plane a membership domain's base attaches to.
    ///
    /// Two validated lookups and no configuration: the membership answer for
    /// `domain` names the apex, `apex_of` holds that apex between the signing
    /// zone and the domain exactly as the transparency lookup does, and the
    /// attach record is read at `_synchronicity-cp.<apex>`. Every step is
    /// DNSSEC-validated fail-closed, so a stripped, spoofed or absent answer
    /// yields no endpoint rather than a redirected one.
    ///
    /// The TTL is the shorter of the two answers': the endpoint is only as
    /// believable as the apex that led to it.
    ///
    /// **Every usable record, not the first one.** A control plane is a
    /// fleet, and the registry of attached daemons is one node's memory: a
    /// node nobody holds a tunnel to can answer no browse question however
    /// faithfully its database replicated. So the apex names each node with
    /// a record of its own, and the caller opens a tunnel to each. Reading
    /// them as several records rather than as several `url=` fields in one
    /// is what lets a zone add a node without breaking a daemon built before
    /// it: an older client takes the first record it can parse and reaches
    /// one node of the fleet, where a second `url=` would be a duplicate
    /// field it refuses outright.
    pub async fn control_plane(
        &self,
        domain: &str,
    ) -> Result<(Vec<ControlPlaneRecord>, Duration), NetError> {
        let domain = normalize_domain(domain).map_err(|e| NetError::Dns(e.to_string()))?;
        let name = query_name(&domain);
        // The membership answer yields the apex the attach record hangs off,
        // so it gets the same gate as `member_set`: under `RekorPolicy::Require`
        // the zone key that signed it must be on the transparency log — what
        // makes the `apex=` field believable, and safe to look one label
        // under. It says nothing about the attach record itself, which is a
        // second RRset with a signer of its own.
        let membership = self
            .gated_txt(&domain, &name, GateApex::FromRecords)
            .await?;
        // The apex the attach record hangs off — always needed here (unlike
        // in `member_set`), because a control plane offering cloud attach
        // always publishes it.
        let apex = apex_of(&domain, membership.signer(), membership.records())?;
        let cp_name = control_plane_query_name(apex.to_string().trim_end_matches('.'));
        // **The gate again, on this answer's own signer** — the whole reason
        // both legs go through one function. Gating the membership answer
        // says nothing about who signed *this* record, and they are two RRsets
        // a zone can sign with two different keys. The threat model's
        // attacker is precisely a party who can add a DNSKEY to the apex RRset
        // — a compromised or coerced parent substituting a DS — and such a key
        // validates everything it signs: they serve the operator's genuine
        // membership RRset (public data, so the gate above passes on the
        // logged key) and sign only this record with the unlogged one, and
        // the daemon attaches to their control plane with no monitor seeing
        // anything. `Under(&apex)` rather than `FromRecords`: this RRset
        // carries no `apex=` field, and letting it name its own bound would
        // hand the record under audit the choice of what it is judged
        // against. The cost is one DNSKEY lookup and proof set per attach
        // session; a zone signing both answers with one key — every ordinary
        // deployment — reads the same proof twice and passes twice, and the
        // TUF walk inside the second call is a no-op.
        let validated = self
            .gated_txt(&domain, &cp_name, GateApex::Under(&apex))
            .await?;
        // One unreadable record must not sink a readable one — the same rule
        // the proof set applies: a control plane mid-upgrade can leave an
        // old-format record beside a current one. `url=` is checked only for
        // shape (an `https://` or `http://` origin, see
        // `parse_control_plane_record`) and is otherwise an opaque redirect
        // target: acceptable *because* the zone key that published it is
        // gated directly above. On `https://`,
        // WebPKI TLS on the WSS connection sits on top; on `http://` the
        // zone key is the whole of it.
        let mut refusal = None;
        let mut endpoints: Vec<ControlPlaneRecord> = Vec::new();
        for record in validated.records() {
            match parse_control_plane_record(record) {
                Ok(record) => {
                    if !endpoints.contains(&record) {
                        endpoints.push(record);
                    }
                }
                Err(e) => refusal = Some(e),
            }
        }
        if endpoints.is_empty() {
            return Err(match refusal {
                Some(e) => {
                    NetError::Dns(format!("{cp_name}: no usable control-plane record ({e})"))
                }
                None => NetError::Dns(format!("{cp_name}: no control-plane record")),
            });
        }
        // An RRset arrives in whatever order the wire happened to carry it,
        // and a caller that opens one tunnel per endpoint must not open a
        // different *set* on each refresh — so the order is the zone's data
        // rather than the answer's arrival, and the cap cuts a fixed
        // prefix instead of an arbitrary one.
        endpoints.sort_by(|a, b| a.url.cmp(&b.url));
        if endpoints.len() > MAX_CP_ENDPOINTS {
            tracing::warn!(
                name = %cp_name,
                named = endpoints.len(),
                cap = MAX_CP_ENDPOINTS,
                "the apex names more control-plane endpoints than a daemon will attach to; \
                 the rest are ignored"
            );
            endpoints.truncate(MAX_CP_ENDPOINTS);
        }
        Ok((endpoints, validated.ttl().min(membership.ttl())))
    }

    /// Walks Sigstore's TUF repository and adopts what it served if it is
    /// newer (§10.2). `Ok(None)` is the ordinary case: refresh is off, or
    /// the last walk is still young. An `Err` names the class the chain
    /// broke in for `synch doctor`; the refresh pipeline logs it and carries
    /// on, because nothing about a TUF repository may fail a membership
    /// refresh. Under `--rekor-key` or `--no-tuf` this does nothing at all:
    /// a static universe is static in both directions.
    pub async fn refresh_tuf(&self) -> Result<Option<TufUpdate>, NetError> {
        let Some(source) = self.tuf.clone() else {
            return Ok(None);
        };
        // Two decisions under the lock and nothing else: whether a walk is
        // due and which root version it starts from. The walk itself is
        // seconds of network; holding the mutex across it would serialize
        // every membership refresh behind a CDN.
        let (now, from_root) = {
            let mut pins = self.pins();
            let Pins::Tuf {
                state, checked_at, ..
            } = &mut *pins
            else {
                return Ok(None);
            };
            // No trustworthy clock, no refresh: expiry is the only bound on
            // how old served metadata may be, and the pins in force are the
            // safe place to stay.
            let Some(now) = now_unix(state.updated_at) else {
                return Err(tuf_error(
                    &source,
                    TufError::Expiry(
                        "the system clock is unreadable, so no expiry could be \
                         checked; the current pins stand"
                            .into(),
                    ),
                ));
            };
            // Due on the interval, and also when the stamp is ahead of the
            // clock (see `refresh_due` for why that can happen).
            if !refresh_due(*checked_at, now) {
                return Ok(None);
            }
            // Stamped before the walk, so a slow or down repository costs
            // one attempt a day, not one per refresh.
            *checked_at = now;
            (now, state.root_version)
        };
        let metadata = self.walk_tuf(&source, from_root).await?;

        // Persistence is read off the blocking pool with no pin mutex held:
        // SQLite may wait behind any daemon transaction, and no runtime worker
        // or concurrent resolver should wait with it.
        let (memory, path, config, anchor) = {
            let pins = self.pins();
            let Pins::Tuf {
                state,
                path,
                config,
                anchor,
                ..
            } = &*pins
            else {
                return Ok(None);
            };
            (
                (**state).clone(),
                path.clone(),
                config.clone(),
                anchor.clone(),
            )
        };
        let persisted = {
            let path = path.clone();
            let config = config.clone();
            let anchor = anchor.clone();
            crate::blocking::offload(move || {
                Ok(match config.as_deref() {
                    Some(store) => load_config_pin_state(store, &anchor),
                    None => path
                        .as_deref()
                        .and_then(|path| load_pin_state(path, &anchor)),
                })
            })
            .await?
        };
        // Whichever of the two is further along, whole: a state is a
        // coherent set, so taking the newer *state* is right and the newer
        // of each field would not be. Read against this resolver's anchor,
        // exactly as at startup: a state accumulated under some other TUF
        // repository is not this resolver's to adopt, however far along.
        let current = match persisted {
            Some(stored) if dominates(&stored, &memory) => stored,
            _ => memory,
        };
        let update = tuf::update(&metadata, &current, now).map_err(|e| tuf_error(&source, e))?;
        let persisted_state = update.state.clone();
        let persisted = crate::blocking::offload(move || {
            if let Some(store) = config {
                store.set_config("rekor.pin_state", &persisted_state.encode())?;
            } else if let Some(path) = path.as_deref() {
                persisted_state.save(path).map_err(|error| {
                    NetError::Dns(format!("pin state {}: {error}", path.display()))
                })?;
            }
            Ok(())
        })
        .await;
        if let Err(error) = persisted {
            // A pin set that cannot be persisted is still adopted for this
            // process and re-learned next time.
            tracing::warn!(%error, "could not persist the TUF pin state");
        }
        let mut pins = self.pins();
        let Pins::Tuf { keys, state, .. } = &mut *pins else {
            return Ok(None);
        };
        *keys = update.log_keys.clone();
        **state = update.state.clone();
        Ok(Some(update))
    }

    /// Collects the metadata chain from the repository, off the reactor.
    ///
    /// The walk is a handful of sequential HTTPS GETs ending in a few hundred
    /// KB of `targets.json`; [`tuf::fetch_metadata`] is the one
    /// implementation, which the monitor also runs. `spawn_blocking` is what
    /// lets that stay true without a second, async transcription of a walk
    /// whose every step is load-bearing.
    async fn walk_tuf(&self, source: &TufSource, from_root: u64) -> Result<TufMetadata, NetError> {
        let owned = source.clone();
        // The shared handoff rather than a raw `spawn_blocking`: `offload`
        // enters the `BlockingScope` that marks this thread as allowed to
        // block, which is what `assert_off_runtime` checks in debug builds.
        let walked = crate::blocking::offload(move || {
            Ok(match &owned {
                TufSource::Repo(repo) => tuf::fetch_metadata(&**repo, from_root),
                TufSource::Url(url) => tuf::HttpRepo::new(url, None)
                    .map_err(|e| TufError::Malformed(format!("TUF client: {e}")))
                    .and_then(|repo| tuf::fetch_metadata(&repo, from_root)),
            })
        })
        .await?;
        walked.map_err(|e| tuf_error(source, e))
    }

    /// Verifies that the zone key which signed an answer is on the public
    /// record (§4.2). Two more validated lookups, then no network at all.
    pub(crate) async fn verify_zone_key(
        &self,
        domain: &str,
        apex: &Name,
        membership: &DnssecTxt,
    ) -> Result<VerifiedRecord, NetError> {
        let signing_zone = &membership.signer;
        let zone_text = signing_zone.to_string();
        let apex_text = apex.to_string();
        let key_tag = membership.rrsig.input().key_tag;
        let dnskey_rdata = self
            .zone_dnskey(signing_zone, &membership.rrsig, &membership.txt_rrset)
            .await?;

        // Under the **apex**, which the membership answer named: every
        // record a control plane owns hangs off its own apex, so two of
        // them in one signing zone never share a name.
        let apex_label = apex_text.trim_end_matches('.');
        let name = rekor_query_name(apex_label);
        let response = self.lookup(&name, RecordType::TXT).await?;
        let absent = || NetError::RekorAbsent {
            name: name.clone(),
            key_tag,
        };
        // A name that does not exist and one with no proof for this key tag
        // are the same fact to a client: never logged, as far as the zone
        // will say.
        let records = match self.validated_txt(&name, &response.answers) {
            Ok(validated) => validated.records,
            Err(NetError::Dns(_)) if response.answers.is_empty() => return Err(absent()),
            Err(e) => return Err(e),
        };

        // One unreadable record must not sink a readable one: during a
        // rollover the set holds a record per key, and refusing the whole
        // set strands a client that had the proof it needed next to the one
        // it did not. The set is DNSSEC-validated, so skipping what this
        // build cannot read is compatibility, not injection; a malformed
        // record is still reported when *nothing* matched, so "gibberish"
        // stays distinguishable from "nothing for this key" in `synch
        // doctor`. A proof spans several names: part 1 is the only name
        // derivable from the answer and says how many parts there are; the
        // rest are fetched by index, bounded by what part 1 claims and the
        // format's 255-part ceiling — a lying record cannot turn one refresh
        // into a scan.
        let mut records = records;
        let wanted = rekor::parts_claimed(&records);
        for index in 2..=wanted {
            let part = rekor_part_query_name(apex_label, index);
            // A part that does not resolve leaves the set incomplete, which
            // `proofs_from_txt` reports as the missing-part refusal it is.
            if let Ok(answer) = self.lookup(&part, RecordType::TXT).await {
                if let Ok(validated) = self.validated_txt(&part, &answer.answers) {
                    records.extend(validated.records);
                }
            }
        }

        let mut candidates = Vec::new();
        let mut malformed: Option<rekor::ProofError> = None;
        for reassembled in rekor::proofs_from_txt(&records) {
            match reassembled {
                Ok(candidate) => candidates.push(candidate),
                Err(e) => malformed = Some(e),
            }
        }
        if candidates.is_empty() {
            // The same rule as above: one unreadable record must not sink a
            // readable one, and a malformed record is reported when *nothing*
            // matched, so "gibberish" stays distinguishable from "nothing
            // for this key".
            return Err(match malformed {
                Some(e) => NetError::RekorMalformed {
                    name: name.clone(),
                    reason: e.to_string(),
                },
                None => absent(),
            });
        }

        let key = ZoneKey {
            domain,
            signing_zone: &zone_text,
            key_tag,
            dnskey_rdata: &dnskey_rdata,
        };
        // Every candidate, not just the last: a record's subject is a key
        // set with no selector on the wire, and a zone can legitimately
        // serve more than one record; membership in a verified set decides.
        let candidates = candidates_to_verify(candidates);
        let mut last = None;
        for candidate in &candidates {
            match rekor::verify(candidate, &key, &self.log_keys(), &self.anchors) {
                Ok(verified) => return Ok(verified),
                Err(e) => last = Some(e),
            }
        }
        Err(rekor_error(
            &name,
            last.expect("a non-empty candidate list yields an error"),
        ))
    }

    /// The DNSKEY rdata that actually verifies the membership RRSIG.
    ///
    /// A 16-bit key tag is not an identity. After looking the DNSKEY set up
    /// at the signing zone, the live key is the one `signing_key_rdata`
    /// can re-verify the RRSIG under.
    async fn zone_dnskey(
        &self,
        signing_zone: &Name,
        rrsig: &RRSIG,
        txt_rrset: &[Record],
    ) -> Result<Vec<u8>, NetError> {
        let name = signing_zone.to_string();
        let response = self.lookup(&name, RecordType::DNSKEY).await?;
        signing_key_rdata(signing_zone, rrsig, txt_rrset, &response.answers)
    }

    /// One validated lookup over the single transport.
    async fn lookup(
        &self,
        name: &str,
        record_type: RecordType,
    ) -> Result<hickory_resolver::proto::op::DnsResponse, NetError> {
        use futures_core::Stream;
        use hickory_resolver::{net::xfer::DnsHandle, proto::op::Query};

        let mut qname = Name::from_utf8(name).map_err(|e| NetError::Dns(format!("{name}: {e}")))?;
        qname.set_fqdn(true);
        let query = Query::query(qname, record_type);
        let mut stream = self.handle.lookup(query, Default::default());
        std::future::poll_fn(|cx| Pin::new(&mut stream).poll_next(cx))
            .await
            .ok_or_else(|| NetError::Dns(format!("{name}: the endpoint sent no response")))?
            .map_err(|e| NetError::Dns(format!("{name}: {e}")))
    }
}

/// The DNSKEY rdata that actually verifies `rrsig` over `txt_rrset`.
///
/// A 16-bit key tag is not an identity: after a DS/provider compromise an
/// attacker can publish a new ZSK with the same tag as a previously logged
/// key, sign membership with it, and keep serving the old proof. The live
/// key is the one whose public key verifies the RRSIG, not the first Secure
/// DNSKEY sharing its tag. This also bounds hickory's choice of *which*
/// RRSIG it verified under: the transport can steer the choice among equally
/// valid signatures and cannot steer it onto a key that signed nothing.
///
/// `verify_rrsig`'s name argument is the TXT owner — the RRset being
/// checked — not the signing zone (the DNSKEY owner, RFC 4035 §5.3.1).
fn signing_key_rdata(
    signing_zone: &Name,
    rrsig: &RRSIG,
    txt_rrset: &[Record],
    dnskey_answers: &[Record],
) -> Result<Vec<u8>, NetError> {
    use hickory_resolver::proto::{
        dnssec::{rdata::DNSSECRData, Verifier},
        rr::{DNSClass, RData, RecordType},
    };

    let key_tag = rrsig.input().key_tag;
    let zone_name = signing_zone.to_string();

    // Do not trust Proof::Secure alone on the DNSKEY set: hickory 0.26 will
    // chase whatever signer_name an RRSIG carries, so an off-path DNSKEY
    // RRSIG must not authorize anything here.
    let dnskey_rrsig_encloses = dnskey_answers.iter().any(|record| {
        if !covered_by_signed_data(record, signing_zone) {
            return false;
        }
        let RData::DNSSEC(DNSSECRData::RRSIG(sig)) = &record.data else {
            return false;
        };
        sig.input().type_covered == RecordType::DNSKEY
            && sig.input().signer_name.to_lowercase().zone_of(signing_zone)
    });
    if !dnskey_rrsig_encloses {
        return Err(NetError::Dns(format!(
            "{zone_name}: no DNSSEC-secure DNSKEY RRSIG whose signer contains \
             the zone — a zone may only sign for names it holds \
             (RFC 4035 §5.3.1)"
        )));
    }

    let owner = txt_rrset
        .iter()
        .find(|record| matches!(record.data, RData::TXT(_)))
        .map(|record| &record.name)
        .ok_or_else(|| {
            NetError::Dns(format!(
                "{zone_name}: membership RRset has no TXT records to verify \
                 against the signing key"
            ))
        })?;

    let mut matched: Option<Vec<u8>> = None;
    for record in dnskey_answers {
        if !covered_by_signed_data(record, signing_zone) {
            continue;
        }
        let RData::DNSSEC(DNSSECRData::DNSKEY(dnskey)) = &record.data else {
            continue;
        };
        if dnskey.calculate_key_tag().ok() != Some(key_tag) {
            continue;
        }
        if dnskey
            .verify_rrsig(owner, DNSClass::IN, rrsig, txt_rrset.iter())
            .is_err()
        {
            continue;
        }
        let rdata = crate::chain::dnskey_rdata(dnskey);
        if let Some(existing) = &matched {
            if existing != &rdata {
                return Err(NetError::Dns(format!(
                    "{zone_name}: two DNSKEYs with key tag {key_tag} verify \
                     the membership RRSIG; refusing to guess which one signed it"
                )));
            }
            continue;
        }
        matched = Some(rdata);
    }
    matched.ok_or_else(|| {
        NetError::Dns(format!(
            "{zone_name}: no DNSSEC-secure DNSKEY with key tag {key_tag} \
             verifies the membership RRSIG"
        ))
    })
}

/// Lifts a verification failure into the error class `synch doctor` explains.
fn rekor_error(name: &str, error: ProofError) -> NetError {
    let name = name.to_string();
    match error {
        ProofError::Malformed(reason) => NetError::RekorMalformed { name, reason },
        ProofError::Attribution(reason) => NetError::RekorAttribution { name, reason },
        ProofError::Binding(reason) => NetError::RekorBinding { name, reason },
        ProofError::Inclusion(reason) => NetError::RekorInclusion { name, reason },
        ProofError::Checkpoint(reason) => NetError::RekorCheckpoint { name, reason },
        ProofError::UnknownLog(reason) => NetError::RekorUnknownLog { name, reason },
        ProofError::Chain(reason) => NetError::RekorChain { name, reason },
    }
}

/// Lifts a TUF failure into the error class `synch doctor` explains, naming
/// the repository it was walking.
///
/// None of these ever reaches a caller of [`DnssecResolver::member_set`]:
/// §10.2 is explicit that TUF trouble is never worse than not having asked.
/// They exist so the refresh can say which way it broke.
fn tuf_error(source: &TufSource, error: TufError) -> NetError {
    NetError::Tuf {
        repository: match source {
            TufSource::Url(url) => url.clone(),
            TufSource::Repo(_) => "the supplied TUF repository".to_string(),
        },
        class: error.class(),
        reason: error.to_string(),
    }
}

/// A pin state's versions: the root, then each role below it.
fn versions(state: &PinState) -> [u64; 4] {
    [
        state.root_version,
        state.timestamp_version,
        state.snapshot_version,
        state.targets_version,
    ]
}

/// Whether `a` is at least as far along as `b` in **every** role.
///
/// The four versions are independently-monotone counters, so "further along"
/// is componentwise dominance, not the lexicographic order a tuple
/// comparison gives: a state ahead on `root` but behind on
/// timestamp/snapshot/targets would read as newer and adopting it would
/// drop those roles' rollback floors — the whole thing the floors exist to
/// prevent. When neither dominates, keep what is in memory rather than pick
/// a tie-break that means nothing.
fn dominates(a: &PinState, b: &PinState) -> bool {
    versions(a)
        .iter()
        .zip(versions(b).iter())
        .all(|(a, b)| a >= b)
}

/// Whether a TUF walk is due, given when the last one was stamped. Ordinarily
/// just the interval. The other arm is the one worth naming: a `checked_at`
/// **ahead of the clock** is not a recent check, it is an impossible one, and
/// it makes the walk due rather than postponing it — the stamp is seeded from
/// `PinState::updated_at` and the state file is read as written (§10.2), so a
/// corrupt or hand-edited file would otherwise put the next refresh past any
/// instant the clock can reach, freezing the pin set silently. (The same
/// freeze is what [`MAX_CLOCK_FLOOR_LEAD`] bounds inside [`now_unix`]; this
/// arm closes it here.) The caller stamps `now` immediately afterwards, so a
/// file claiming the future costs one walk and repairs itself.
fn refresh_due(checked_at: u64, now: u64) -> bool {
    checked_at > now || now >= checked_at.saturating_add(tuf::REFRESH_INTERVAL)
}

/// How far ahead of the system clock a persisted floor may be before the
/// floor is the thing that is wrong. A floor exists to stop the clock going
/// *backwards*; a day of tolerance covers every legitimate lead (an update
/// accepted on a slightly fast clock, an NTP correction between refreshes),
/// and a floor of 2100 would otherwise make every refresh interval
/// unreachable and every TUF expiry vacuous, silently, forever.
const MAX_CLOCK_FLOOR_LEAD: u64 = 24 * 60 * 60;

/// How long a pin refresh may keep failing before a failure is worth an
/// operator's attention. A single failed walk is ordinary (debug); a week of
/// them means the pin set is frozen, which refuses every proof from the day
/// Sigstore opens a shard the client has never heard of. "Much more than the
/// interval" is what makes the two distinguishable.
const PIN_REFRESH_STALE_AFTER: u64 = 7 * tuf::REFRESH_INTERVAL;

/// Seconds since the epoch, for the expiry checks every TUF role carries,
/// floored by the last state this client accepted and bounded against the
/// clock. Three properties, each one a way the pin set can be frozen:
///
/// - **An unreadable clock is `None`, not zero.** Every expiry check is
///   `expires > now`, so at zero nothing has ever expired and expiry — the
///   only bound on how old served metadata may be — stops being a bound; a
///   refresh with no trustworthy clock does not run.
/// - **A clock moved backwards is floored** by `updated_at`: a bad NTP step
///   or a dead RTC coming up at the epoch cannot reopen the same window.
/// - **And the floor is bounded by the clock in turn.** The floor is read
///   from a file, so a value far in the future makes the refresh interval
///   unreachable *and* every expiry pass, silently. Past the tolerance the
///   clock wins and the floor is reported — neither a corrupt file nor a
///   clock that was very wrong is something to keep obeying.
fn now_unix(floor: u64) -> Option<u64> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    if floor > now.saturating_add(MAX_CLOCK_FLOOR_LEAD) {
        tracing::warn!(
            floor,
            now,
            "the persisted TUF timestamp is further ahead of this clock than any \
             correction explains; using the clock, and the pin refresh will \
             re-establish the floor"
        );
        return Some(now);
    }
    Some(now.max(floor))
}

/// How long one DoH exchange may take end to end.
const DOH_TIMEOUT: Duration = Duration::from_secs(10);

/// The most of a DoH response body we will buffer. A DNS message is
/// length-prefixed by 16 bits, so 65535 bytes is the whole of any legitimate
/// answer; the transport is untrusted, so a body without end is a
/// memory-exhaustion lever — we read up to this bound and refuse the rest.
/// Denial, which http already concedes, never escalates to unbounded
/// allocation.
const MAX_DOH_RESPONSE: usize = 64 * 1024;

/// An RFC 8484 DNS-over-HTTP(S) client — the only transport. Queries are
/// POSTed in wire format over reqwest, and every response goes through the
/// [`DnssecDnsHandle`] wrapped around this: hickory reduced to in-process
/// DNSSEC validation. The endpoint hostname resolves through the operating
/// system once per connection — name-to-address plumbing for the endpoint
/// itself, not part of the membership trust path.
#[derive(Clone)]
struct DohHandle {
    client: reqwest::Client,
    url: reqwest::Url,
}

impl DohHandle {
    fn new(url: reqwest::Url) -> Result<Self, NetError> {
        // Environment proxies are honored, as reqwest does by default: on
        // networks where a proxy is the only road out, refusing it strands
        // exactly the deployments DoH exists for.
        let client = reqwest::Client::builder()
            .timeout(DOH_TIMEOUT)
            .build()
            .map_err(|e| NetError::Dns(format!("DoH client: {e}")))?;
        Ok(DohHandle { client, url })
    }

    /// POSTs one wire-format query, returning the wire-format answer.
    async fn exchange(&self, body: Vec<u8>) -> Result<Vec<u8>, std::io::Error> {
        let response = self
            .client
            .post(self.url.clone())
            .header("content-type", "application/dns-message")
            .header("accept", "application/dns-message")
            .body(body)
            .send()
            .await
            .map_err(|e| std::io::Error::other(format!("{}: {e}", self.url)))?;
        if !response.status().is_success() {
            return Err(std::io::Error::other(format!(
                "{} answered {}",
                self.url,
                response.status()
            )));
        }
        // A `Content-Length` past the cap is refused up front, but the
        // header is a hint an attacker can omit or lie about — the streaming
        // loop below is the real bound.
        if response
            .content_length()
            .is_some_and(|n| n > MAX_DOH_RESPONSE as u64)
        {
            return Err(std::io::Error::other(format!(
                "{}: response exceeds the {MAX_DOH_RESPONSE}-byte DoH ceiling",
                self.url
            )));
        }
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| std::io::Error::other(format!("{}: {e}", self.url)))?
        {
            if bytes.len() + chunk.len() > MAX_DOH_RESPONSE {
                return Err(std::io::Error::other(format!(
                    "{}: response exceeds the {MAX_DOH_RESPONSE}-byte DoH ceiling",
                    self.url
                )));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
}

/// What one DoH exchange resolves to.
type ExchangeResult =
    Result<hickory_resolver::proto::op::DnsResponse, hickory_resolver::net::NetError>;

/// A one-shot response stream, which is all one HTTP exchange produces.
struct OnceResponse {
    future: Option<Pin<Box<dyn Future<Output = ExchangeResult> + Send>>>,
}

impl futures_core::Stream for OnceResponse {
    type Item = Result<hickory_resolver::proto::op::DnsResponse, hickory_resolver::net::NetError>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        match self.future.as_mut() {
            None => std::task::Poll::Ready(None),
            Some(future) => match future.as_mut().poll(cx) {
                std::task::Poll::Pending => std::task::Poll::Pending,
                std::task::Poll::Ready(result) => {
                    self.future = None;
                    std::task::Poll::Ready(Some(result))
                }
            },
        }
    }
}

impl hickory_resolver::net::xfer::DnsHandle for DohHandle {
    type Response = OnceResponse;
    type Runtime = hickory_resolver::net::runtime::TokioRuntimeProvider;

    fn send(&self, request: hickory_resolver::proto::op::DnsRequest) -> Self::Response {
        let handle = self.clone();
        OnceResponse {
            future: Some(Box::pin(async move {
                let body = request
                    .to_vec()
                    .map_err(hickory_resolver::net::NetError::from)?;
                let answer = handle
                    .exchange(body)
                    .await
                    .map_err(hickory_resolver::net::NetError::from)?;
                let mut response = hickory_resolver::proto::op::DnsResponse::from_buffer(answer)
                    .map_err(hickory_resolver::net::NetError::from)?;
                strip_off_path_rrsigs(&mut response);
                Ok(response)
            })),
        }
    }
}

/// Drops every RRSIG whose signer does not enclose the owner name.
///
/// RFC 4035 §5.3.1: the Signer's Name MUST be the zone that contains the
/// RRset. hickory 0.26 does not enforce this (TODO on that rule) and DoH is
/// an untrusted transport, so an off-path signature is removed before the
/// validator can be sent chasing a DNSKEY at a name with no business signing
/// this RRset. Parent signing a child DS is fine (`com.` encloses
/// `example.com.`); `attacker.com` signing `example.com` DS/TXT/DNSKEY is
/// not.
///
/// # What this does not defend against
///
/// **An injected RRSIG that names the real zone is a one-packet denial of the
/// lookup, and this filter cannot stop it.** Such a record passes the
/// enclosure test by construction — the whole shape of the injection — and
/// hickory races one future per RRSIG with `select_ok`, where a *failed*
/// verification resolves `Ok(None)` and counts as the winner, producing
/// `Proof::Bogus` for the whole RRset. The junk RRSIG shares the real one's
/// DNSKEY lookup, so record order — the transport's choice — decides. The
/// consequence is availability only, fail-closed: cached bindings stand
/// until grace runs out, nothing false is accepted. It is a bug in the
/// dependency's signature-selection logic, not fixable from this side; this
/// filter closes only the off-path half.
///
/// The other half stays open, and saying so is the point: which RRSIG
/// hickory reports as the signer **does** decide which zone key must carry a
/// transparency proof, so during an RFC 6781 double-signature rollover a
/// transport can pick which of the zone's live keys the requirement lands on
/// (see the note in `secure_txt`). It cannot be steered onto a key that did
/// not sign, so the failure is availability, not acceptance: under `Require`
/// the un-logged key fails closed for `CONTROL_PLANE_REPUBLISH_WINDOW` —
/// exactly the state where only one live key has a published proof.
fn strip_off_path_rrsigs(response: &mut hickory_resolver::proto::op::DnsResponse) {
    drop_off_path_rrsigs(&mut response.answers);
    drop_off_path_rrsigs(&mut response.authorities);
    drop_off_path_rrsigs(&mut response.additionals);
}

fn drop_off_path_rrsigs(records: &mut Vec<Record>) {
    records.retain(rrsig_signer_encloses_owner);
}

fn rrsig_signer_encloses_owner(record: &Record) -> bool {
    use hickory_resolver::proto::{dnssec::rdata::DNSSECRData, rr::RData};
    match &record.data {
        RData::DNSSEC(DNSSECRData::RRSIG(rrsig)) => rrsig
            .input()
            .signer_name
            .to_lowercase()
            .zone_of(&record.name),
        _ => true,
    }
}

/// Whether a record is one the RRSIG that validated its group actually
/// **covers** — the question `Proof::Secure` looks like it answers and does
/// not. hickory groups an answer into RRsets by `(name, record_type)` and
/// nothing else (`DnssecDnsHandle::verify_rrsets`), then stamps the verdict
/// onto *every* record in the group, while the signed-data construction
/// filters by `(dns_class, type_covered, name)` with class hard-coded to IN.
/// A record of another class is dropped from the bytes the signature is
/// checked over — the honest RRSIG still verifies — and comes back marked
/// `Secure` anyway: a class-CH record spliced into a DoH response arrives
/// "DNSSEC-validated" while signed by nobody. On the membership path that is
/// a full read/write binding for a key the zone never published, surviving
/// `--rekor require` because the proof covers the zone *key*.
///
/// The rule, and the reason this is a named predicate rather than three more
/// `&&`s at four call sites: **the set of records accepted must be exactly
/// the set the verifier canonicalizes** — `(name, type, class)`. Matching it
/// by construction keeps the next unmodelled dimension from being the next
/// vulnerability.
fn covered_by_signed_data(record: &Record, owner: &Name) -> bool {
    use hickory_resolver::proto::rr::DNSClass;
    record.name == *owner && record.dns_class == DNSClass::IN && record.proof.is_secure()
}

/// Applies the fail-closed §3.2 record checks to one answer set.
///
/// Shared by both backends so the acceptance rule cannot drift between
/// transports: every record must carry a *secure* proof, and one unvalidated
/// record poisons the whole answer.
fn secure_txt(
    name: &str,
    answers: &[hickory_resolver::proto::rr::Record],
    now: Option<u64>,
) -> Result<DnssecTxt, NetError> {
    use hickory_resolver::proto::{
        dnssec::rdata::DNSSECRData,
        rr::{DNSClass, RData, RecordType},
    };

    let mut qname = hickory_resolver::proto::rr::Name::from_utf8(name)
        .map_err(|e| NetError::Dns(format!("{name}: {e}")))?;
    qname.set_fqdn(true);

    // Step one, and it has to be first: *who* signed this. hickory marks
    // exactly one RRSIG per RRset as the one it verified under (the rest come
    // back `Indeterminate`), so at most one candidate exists here. Which one
    // follows the order the untrusted transport chose, so it is steerable
    // among *equally valid* signatures — deciding which key the proof
    // requirement lands on during an RFC 6781 double-signature rollover —
    // but never onto a key that did not sign: `signing_key_rdata` re-verifies
    // the signature against the DNSKEY set before anything is bound.
    let signed_by = answers.iter().find_map(|record| {
        if !covered_by_signed_data(record, &qname) {
            return None;
        }
        let RData::DNSSEC(DNSSECRData::RRSIG(rrsig)) = &record.data else {
            return None;
        };
        (rrsig.input().type_covered == RecordType::TXT).then(|| rrsig.clone())
    });
    let Some(rrsig) = signed_by else {
        return Err(NetError::Dns(format!(
            "{name}: the validated answer carries no RRSIG naming its signer"
        )));
    };
    let key_tag = rrsig.input().key_tag;
    let signer = rrsig.input().signer_name.to_lowercase();

    // **The check hickory does not make.** RFC 4035 §5.3.1: the signer's
    // name MUST be the zone that contains the RRset. hickory 0.26 skips it
    // in two places, both marked TODO — so `Proof::Secure` means only "some
    // key, at some name the answer chose, signed this", and an attacker
    // holding *any* DNSSEC-signed zone can sign an RRset owned by somebody
    // else's name and have it validate. Owner-name filtering does not close
    // it: the forged RRset is owned by the queried name, the whole point of
    // the forgery; the signer must enclose the name it signed for, and that
    // is what this asserts. The general defense is the transport filter
    // (`strip_off_path_rrsigs`) for DS and DNSKEY; this check remains so a
    // membership or proof TXT cannot name a signing zone that does not hold
    // it — that name is what Rekor binds to.
    if !signer.zone_of(&qname) {
        return Err(NetError::Dns(format!(
            "{name}: the answer is signed by {signer}, which does not contain \
             the name it answers for — a zone may only sign for names it holds \
             (RFC 4035 §5.3.1)"
        )));
    }

    let mut records = Vec::new();
    let mut txt_rrset = Vec::new();
    let mut ttl = MAX_TTL;
    for record in answers {
        // DNSSEC proves an RRset is signed — it does not bind the answer to
        // the question: a validly signed TXT from an attacker-controlled
        // zone would bind attacker keys into the member set. Only records
        // owned by the queried name count.
        if record.name != qname {
            continue;
        }
        // And of the class the signature covers — see `covered_by_signed_data`:
        // a record of another class is not signed, though hickory stamps the
        // whole group `Secure`; accepting it here is a membership forgery.
        if record.dns_class != DNSClass::IN {
            continue;
        }
        // Only the records this answer is *made of* need proofs: RRSIGs come
        // back `Indeterminate` except the one verified under, so demanding
        // proofs on those refuses every answer during an RFC 6781
        // double-signature rollover — a one-packet DoS to anyone who can
        // *add* a record to a response.
        if !matches!(record.data, RData::TXT(_)) {
            continue;
        }
        if !record.proof.is_secure() {
            // Fail closed: one unvalidated record poisons the answer.
            return Err(NetError::Dns(format!(
                "{name}: answer is not DNSSEC-secure (proof: {:?})",
                record.proof
            )));
        }
        ttl = ttl.min(Duration::from_secs(u64::from(record.ttl)));
        let RData::TXT(txt) = &record.data else {
            unreachable!("filtered to TXT above")
        };
        // A TXT record is a sequence of character-strings; join them.
        let joined: String = txt
            .txt_data
            .iter()
            .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
            .collect();
        records.push(joined);
        txt_rrset.push(record.clone());
    }
    if records.is_empty() {
        return Err(NetError::Dns(format!("{name}: no TXT records")));
    }

    // The signature's own expiration is the ceiling on everything derived
    // from this answer (see `DnssecTxt::ttl`), read from the RRSIG hickory
    // verified under.
    let expires_at = u64::from(rrsig.input().sig_expiration.get());
    let ttl = match now {
        // No readable clock: the §3.2 window is the only bound available —
        // hickory has already refused expired signatures, so this is the
        // absence of a second fence, not a hole.
        None => clamp_ttl(ttl),
        Some(now) => clamp_ttl(ttl.min(Duration::from_secs(expires_at.saturating_sub(now)))),
    };

    Ok(DnssecTxt {
        records,
        ttl,
        signer,
        key_tag,
        rrsig,
        txt_rrset,
        expires_at,
    })
}

/// A future returning one domain's validated member set.
pub type MemberSetFuture<'a> =
    Pin<Box<dyn Future<Output = Result<(MemberSet, Duration), NetError>> + Send + 'a>>;

/// What a membership refresh resolves through (§3.2).
///
/// [`DnssecResolver`] is the production implementation and the only one a
/// running node uses. The trait exists so that the daemon's TTL-driven refresh
/// loop — the thing that keeps a DNSSEC cluster from dissolving one TTL after
/// the last `synch domain refresh` — can be driven and asserted on without a
/// live, signed zone.
///
/// Boxed rather than `async fn` in trait so the refresh loop can hold a
/// `&dyn MemberResolver` and stay object-safe.
pub trait MemberResolver: std::fmt::Debug + Send + Sync {
    /// Resolves one domain's member set and how long the answer is good for.
    fn resolve_members<'a>(&'a self, domain: &'a str) -> MemberSetFuture<'a>;
}

impl MemberResolver for DnssecResolver {
    fn resolve_members<'a>(&'a self, domain: &'a str) -> MemberSetFuture<'a> {
        Box::pin(self.member_set(domain))
    }
}

#[cfg(test)]
mod tests {
    use hickory_resolver::proto::{
        dnssec::{
            crypto::EcdsaSigningKey,
            rdata::{DNSSECRData, DNSKEY, RRSIG},
            Algorithm, DnssecSigner, Proof, SigningKey,
        },
        op::{DnsResponse, Message, OpCode},
        rr::{rdata::TXT, DNSClass, Name, RData, Record, RecordSet, RecordType},
    };
    use iroh_base::SecretKey;

    use super::*;

    fn key() -> NodeId {
        SecretKey::generate().public()
    }

    #[test]
    fn resolver_loads_rekor_pin_state_from_sqlite_config() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(synch_store::Store::open(dir.path()).unwrap());
        let mut state = PinState::anchored(tuf::EMBEDDED_TUF_ROOT.as_bytes());
        state.updated_at = 123;
        store
            .set_config("rekor.pin_state", &state.encode())
            .unwrap();
        let resolver = DnssecResolver::with_options(&ResolverOptions {
            rekor_config: Some(store),
            no_tuf: true,
            ..ResolverOptions::default()
        })
        .unwrap();
        let pins = resolver.pins();
        let Pins::Tuf {
            state, checked_at, ..
        } = &*pins
        else {
            panic!("default resolver pins must be TUF-backed");
        };
        assert_eq!(state.updated_at, 123);
        assert_eq!(*checked_at, 123);
    }

    fn record(id: Option<&str>, key: &NodeId) -> String {
        match id {
            Some(id) => format!("v=sync1 id={id} nk={}", key.to_z32()),
            None => format!("v=sync1 nk={}", key.to_z32()),
        }
    }

    #[test]
    fn doh_urls_normalize() {
        for (input, expected) in [
            ("https://1.1.1.1", "https://1.1.1.1/dns-query"),
            ("http://[::1]:8053", "http://[::1]:8053/dns-query"),
            (
                "http://10.0.0.53:8053/resolve",
                "http://10.0.0.53:8053/resolve",
            ),
        ] {
            assert_eq!(doh_url(input).unwrap().as_str(), expected);
        }
        // Only http(s) is a DoH transport; everything else is refused.
        let err = doh_url("ftp://1.1.1.1/dns-query").unwrap_err();
        assert!(err.to_string().contains("https:// or http://"), "{err}");
        assert!(doh_url("not a url").is_err());
    }

    #[test]
    fn resolver_options_build_and_fail_closed() {
        // A missing or empty trust anchor is refused by name: an anchor set
        // with no keys would validate nothing, forever, quietly.
        let err = DnssecResolver::with_options(&ResolverOptions {
            no_tuf: true,
            trust_anchor: Some("/does/not/exist.key".into()),
            ..Default::default()
        })
        .unwrap_err();
        assert!(err.to_string().contains("trust anchor"), "{err}");

        let empty = tempfile::NamedTempFile::new().unwrap();
        let err = DnssecResolver::with_options(&ResolverOptions {
            no_tuf: true,
            trust_anchor: Some(empty.path().into()),
            ..Default::default()
        })
        .unwrap_err();
        assert!(err.to_string().contains("no DNSKEY records"), "{err}");
    }

    #[test]
    fn parse_record_accepts_and_refuses_malformed() {
        let k = key();
        let parsed = parse_record(&record(Some("nas"), &k)).unwrap();
        assert_eq!(parsed.id.as_deref(), Some("nas"));
        assert_eq!(parsed.node_key, k);
        assert_eq!(
            parsed.origin("cluster.example.com").unwrap(),
            OriginId::named("nas", "cluster.example.com").unwrap()
        );
        // §3.2: an idless record binds the key itself, non-rotatable.
        let idless = parse_record(&record(None, &k)).unwrap();
        assert_eq!(idless.id, None);
        assert_eq!(idless.origin("x.example").unwrap(), OriginId::Key(k));
        // Unknown fields are tolerated (forward compatibility), and labels
        // are case-insensitive.
        assert!(parse_record(&format!("v=sync1 id=nas nk={} future=whatever", k.to_z32())).is_ok());
        assert_eq!(
            parse_record(&format!("v=sync1 id=NAS nk={}", k.to_z32()))
                .unwrap()
                .id
                .as_deref(),
            Some("nas")
        );
        assert_eq!(
            parse_record("v=sync2 id=nas nk=x").unwrap_err(),
            RecordError::NotSync1
        );
        assert_eq!(
            parse_record("v=sync1 id=nas").unwrap_err(),
            RecordError::MissingKey
        );
        assert!(matches!(
            parse_record("v=sync1 id=nas nk=notakey").unwrap_err(),
            RecordError::BadKey(_)
        ));
        assert!(matches!(
            parse_record(&format!("v=sync1 id=BAD_LABEL nk={}", k.to_z32())).unwrap_err(),
            RecordError::BadLabel(_)
        ));
        assert_eq!(
            parse_record(&format!("v=sync1 id=a id=b nk={}", k.to_z32())).unwrap_err(),
            RecordError::Duplicate("id")
        );
        // A duplicated apex is rejected like a duplicated nk=; one apex is
        // normalized to lowercase.
        assert_eq!(
            parse_record(&format!(
                "v=sync1 id=nas nk={} apex=a.example apex=b.example",
                k.to_z32()
            ))
            .unwrap_err(),
            RecordError::Duplicate("apex")
        );
        assert_eq!(
            parse_record(&format!("v=sync1 id=nas nk={} apex=A.Example", k.to_z32()))
                .unwrap()
                .apex
                .as_deref(),
            Some("a.example")
        );
    }

    #[test]
    fn builds_a_member_set() {
        let nas = key();
        let laptop = key();
        let records = vec![record(Some("nas"), &nas), record(Some("laptop"), &laptop)];
        let set = MemberSet::from_records("Cluster.Example.COM.", &records).unwrap();
        assert_eq!(set.domain, "cluster.example.com");
        assert_eq!(set.bindings.len(), 2);
        assert_eq!(
            set.keys_for(&OriginId::named("nas", "cluster.example.com").unwrap()),
            vec![nas]
        );
        assert!(set.ambiguous_keys.is_empty() && set.rejected.is_empty());

        // §3.2: one id, two keys is a rotation window — both bound, and each
        // key keeps one identity for self-detection.
        let old = key();
        let new = key();
        let set = MemberSet::from_records(
            "x.example",
            &[record(Some("nas"), &old), record(Some("nas"), &new)],
        )
        .unwrap();
        let origin = OriginId::named("nas", "x.example").unwrap();
        let mut keys = set.keys_for(&origin);
        keys.sort_by_key(|k| *k.as_bytes());
        let mut expected = vec![old, new];
        expected.sort_by_key(|k| *k.as_bytes());
        assert_eq!(keys, expected);
        assert_eq!(set.self_origin(&old), Some(origin.clone()));
        assert_eq!(set.self_origin(&new), Some(origin));

        // Identical records collapse; unparseable records are reported, not
        // fatal; and an empty set is not an error (bindings just expire).
        let set = MemberSet::from_records(
            "x.example",
            &[
                record(Some("nas"), &nas),
                record(Some("nas"), &nas),
                "v=sync1 id=broken".into(),
            ],
        )
        .unwrap();
        assert_eq!(set.bindings.len(), 1);
        assert_eq!(set.rejected.len(), 1);
        let empty = MemberSet::from_records("x.example", &[]).unwrap();
        assert!(empty.bindings.is_empty() && empty.rejected.is_empty());
    }

    #[test]
    fn one_key_under_two_ids_is_ambiguous() {
        // §3.2: self-detection refuses to guess, and the bindings an
        // ambiguous key would create are dropped — its hints with them.
        let k = key();
        let other = key();
        let set = MemberSet::from_records(
            "x.example",
            &[
                record(Some("nas"), &k),
                record(Some("laptop"), &k),
                record(Some("vps"), &other),
            ],
        )
        .unwrap();
        assert_eq!(set.ambiguous_keys, vec![k]);
        assert_eq!(set.self_origin(&k), None);
        assert_eq!(set.bindings.len(), 1);
        assert_eq!(
            set.bindings[0].0,
            OriginId::named("vps", "x.example").unwrap()
        );

        // No binding, no hints: harvesting one would let a single added
        // record both evict a member and name where to reach its key.
        let set = MemberSet::from_records(
            "x.example",
            &[
                format!("v=sync1 id=nas nk={} addr=10.0.0.1:4433", k.to_z32()),
                format!(
                    "v=sync1 id=attacker nk={} relay=https://relay.attacker.example",
                    k.to_z32()
                ),
            ],
        )
        .unwrap();
        assert_eq!(set.ambiguous_keys, vec![k]);
        assert!(set.hints_for(&k).is_empty());

        let set = MemberSet::from_records("x.example", &[record(Some("nas"), &key())]).unwrap();
        assert_eq!(set.self_origin(&key()), None);
    }

    #[test]
    fn hints_are_collected_per_key_and_keep_the_field_they_came_from() {
        let k = key();
        let records = vec![format!(
            "v=sync1 id=nas nk={} addr=10.0.0.1:4433 relay=https://relay.example",
            k.to_z32()
        )];
        let set = MemberSet::from_records("x.example", &records).unwrap();
        assert_eq!(
            set.hints_for(&k),
            &[
                DialHint::Relay("https://relay.example".into()),
                DialHint::Addr("10.0.0.1:4433".into())
            ]
        );
        assert!(set.hints_for(&key()).is_empty());

        // The typed distinction survives a value whose *shape* belongs to the
        // other field: a consumer matching on shape alone would dial a relay
        // value as an address.
        let set = MemberSet::from_records(
            "x.example",
            &[format!(
                "v=sync1 id=nas nk={} relay=10.0.0.9:4433",
                k.to_z32()
            )],
        )
        .unwrap();
        assert_eq!(
            set.hints_for(&k),
            &[DialHint::Relay("10.0.0.9:4433".into())],
            "a relay= value is a relay however it is spelled"
        );
    }

    /// The client's half of the timing relation in `zone/render_external.ttl_proof`:
    /// the grace alone must cover the republish window, because the TTL term
    /// cancels in the survival margin — a grace equal to the window still
    /// drops every DNS-sourced binding on an ordinary rotation.
    #[test]
    fn bindings_outlive_a_provider_rotation() {
        assert!(
            DEFAULT_TRUST_GRACE > CONTROL_PLANE_REPUBLISH_WINDOW,
            "a client would drop DNS-sourced members before the control plane \
             could re-publish: {DEFAULT_TRUST_GRACE:?} <= {CONTROL_PLANE_REPUBLISH_WINDOW:?}"
        );
        // With real headroom, not the zero margin an equality gives: the
        // three delays the window sums are the control plane's.
        assert!(
            DEFAULT_TRUST_GRACE >= CONTROL_PLANE_REPUBLISH_WINDOW + Duration::from_secs(120),
            "the margin over the republish window is too thin to absorb any \
             drift in the control plane's three delays"
        );
    }

    #[test]
    fn attach_records_parse_and_refuse() {
        assert_eq!(
            parse_control_plane_record("v=synccp1 url=https://sync.example").unwrap(),
            ControlPlaneRecord {
                url: "https://sync.example".into()
            }
        );
        // A trailing slash is the same endpoint, normalized so the signing
        // context cannot disagree about the URL.
        assert_eq!(
            parse_control_plane_record("v=synccp1 url=https://sync.example/ future=field")
                .unwrap()
                .url,
            "https://sync.example"
        );
        assert_eq!(
            parse_control_plane_record("v=sync1 nk=x").unwrap_err(),
            CpRecordError::NotSyncCp1
        );
        assert_eq!(
            parse_control_plane_record("v=synccp1").unwrap_err(),
            CpRecordError::MissingUrl
        );
        assert_eq!(
            parse_control_plane_record("v=synccp1 url=a url=b").unwrap_err(),
            CpRecordError::Duplicate("url")
        );
        // What guards the redirect target is the zone-key gate, not the
        // scheme; anything that is not an origin is refused.
        assert_eq!(
            parse_control_plane_record("v=synccp1 url=http://127.0.0.1:8510").unwrap(),
            ControlPlaneRecord {
                url: "http://127.0.0.1:8510".into()
            }
        );
        for bad in ["https://", "http://", "sync.example"] {
            assert!(
                matches!(
                    parse_control_plane_record(&format!("v=synccp1 url={bad}")),
                    Err(CpRecordError::BadUrl(_))
                ),
                "{bad}"
            );
        }
    }

    /// One answer names one control-plane apex, bounded at both ends, and
    /// two apexes in one answer is refused rather than tried each way.
    #[test]
    fn one_answer_names_one_apex_and_disagreement_is_refused() {
        let k = key();
        let domain = "net.org.cp.example.com";
        let signing_zone = Name::from_utf8("example.com.").unwrap();
        let named = |apex: &str| format!("v=sync1 id=nas nk={} apex={apex}", k.to_z32());
        let live = named("cp.example.com");
        let other = named("org.cp.example.com");
        let expected = Name::from_utf8("cp.example.com.").unwrap();

        assert_eq!(
            apex_of(domain, &signing_zone, std::slice::from_ref(&live)).unwrap(),
            expected
        );

        // Two usable apexes in one answer is a refusal.
        let records = vec![live.clone(), other.clone()];
        let error = apex_of(domain, &signing_zone, &records)
            .expect_err("an answer naming two apexes must be refused");
        assert!(
            error.to_string().contains("two control-plane apexes"),
            "refused for the wrong reason: {error}"
        );

        // An apex outside the signing zone, one not containing the domain,
        // or one that is not a name, is a rejected record — a member editing
        // their own dialing hint is the realistic cause.
        let outside = named("cp.other.example");
        let sibling = named("sibling.example.com");
        let unparseable = named("cp..example.com");
        assert_eq!(
            apex_of(
                domain,
                &signing_zone,
                &[live, outside.clone(), sibling.clone(), unparseable.clone()]
            )
            .unwrap(),
            expected
        );

        // No usable apex at all is a refusal naming what is missing.
        for records in [vec![], vec![outside, sibling, unparseable]] {
            let error =
                apex_of(domain, &signing_zone, &records).expect_err("no usable apex is a refusal");
            assert!(error.to_string().contains("no membership record names"));
        }
    }

    /// The persisted clock floor cannot run away from the real clock.
    ///
    /// `updated_at` is read from a file and floors every TUF expiry check, so
    /// an unbounded floor is a silent, permanent freeze — past the refresh
    /// interval the walk never runs again, nothing is logged at any level,
    /// and one update taken while the clock is briefly wrong-forward burns
    /// the value in for good.
    #[test]
    fn the_clock_floor_is_bounded_by_the_clock() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(now_unix(0), Some(now));
        assert_eq!(now_unix(now - 1_000), Some(now));
        let slightly = now + MAX_CLOCK_FLOOR_LEAD / 2;
        assert_eq!(now_unix(slightly), Some(slightly));
        for absurd in [now + MAX_CLOCK_FLOOR_LEAD * 2, 4_102_444_800, u64::MAX] {
            assert_eq!(now_unix(absurd), Some(now), "floor {absurd}");
            // And the walk really does become due — asked of `refresh_due`,
            // the thing that decides it (an earlier form compared the clamped
            // clock against `absurd + interval` and passed while frozen).
            assert!(
                refresh_due(absurd, now_unix(absurd).unwrap()),
                "a walk must still become due under floor {absurd}"
            );
        }
        // The gate: a stamp ahead of the clock — an impossible one — is due
        // rather than postponed, so a single integer in `rekor.pin_state`
        // cannot stop every pin refresh for good, silently, across restarts.
        let t = 1_800_000_000;
        assert!(!refresh_due(t, t) && !refresh_due(t - 1, t));
        assert!(!refresh_due(t - (tuf::REFRESH_INTERVAL - 1), t));
        assert!(refresh_due(t - tuf::REFRESH_INTERVAL, t) && refresh_due(0, t));
        assert!(refresh_due(t + 1, t) && refresh_due(t + tuf::REFRESH_INTERVAL * 100, t));
        assert!(refresh_due(4_102_444_800, t) && refresh_due(u64::MAX, t));
    }

    #[test]
    fn signing_key_rdata_picks_the_key_that_verifies_not_the_first_same_tag() {
        let zone = Name::from_utf8("example.").unwrap();
        let (real_key, real_signer) = p256_at(&zone);
        let tag = real_key.calculate_key_tag().expect("tag");
        let (decoy_key, _decoy_signer) = colliding_p256(&zone, tag);

        let owner = Name::from_utf8("_synchronicity.example.").unwrap();
        let txt_rrset = signed_txt(&owner, "v=sync1 id=nas", &real_signer);
        let rrsig = txt_rrsig(&txt_rrset);

        // Decoy first: a first-match tag lookup would return the wrong key.
        let dnskeys = vec![
            secure_dnskey(&zone, decoy_key.clone()),
            secure_dnskey(&zone, real_key.clone()),
            secure_dnskey_rrsig(&zone, &[decoy_key.clone(), real_key.clone()], &real_signer),
        ];
        assert_eq!(
            signing_key_rdata(&zone, &rrsig, &txt_rrset, &dnskeys).unwrap(),
            crate::chain::dnskey_rdata(&real_key)
        );

        // With no key that verifies the membership RRSIG, the answer is
        // refused even though every key is secure.
        let only_decoy = vec![
            secure_dnskey(&zone, decoy_key.clone()),
            secure_dnskey_rrsig(&zone, std::slice::from_ref(&decoy_key), &real_signer),
        ];
        let err = signing_key_rdata(&zone, &rrsig, &txt_rrset, &only_decoy).unwrap_err();
        assert!(
            err.to_string().contains("verifies the membership RRSIG"),
            "{err}"
        );
    }

    /// The proofs one refresh will verify are bounded by this build, not by
    /// the zone.
    #[test]
    fn the_proofs_one_refresh_verifies_are_capped() {
        // What a zone can put at one name is not bounded here; the work one
        // refresh does is.

        let candidates: Vec<rekor::RekorProof> = (0u64..32)
            .map(|n| rekor::RekorProof {
                log_id: [7u8; 32],
                log_index: n,
                statement: b"{}".to_vec(),
                canonicalized_body: b"{}".to_vec(),
                checkpoint: "log.example\n1\nAAAA\n\n\u{2014} log.example AAAAAAAA\n"
                    .as_bytes()
                    .to_vec(),
                inclusion_path: Vec::new(),
            })
            .collect();
        assert!(
            candidates.len() > MAX_PROOF_CANDIDATES,
            "the zone can offer more candidates than this build will walk"
        );
        assert_eq!(
            candidates_to_verify(candidates).len(),
            MAX_PROOF_CANDIDATES,
            "how much verification one refresh does is this build's decision"
        );
    }

    /// A replay cannot renew trust past the signature's own lifetime.
    ///
    /// `secure_txt` keeps no state across refreshes and the proof covers the
    /// zone key rather than the record set, so a signed answer replays as a
    /// unit for its whole RRSIG window; taking the record TTL at face value
    /// would let each replay push a binding out afresh. The signature's own
    /// expiration is the ceiling.
    #[test]
    fn an_answers_ttl_never_outlives_the_signature_over_it() {
        let zone = Name::from_utf8("example.").unwrap();
        let (_, signer) = p256_at(&zone);
        let owner = Name::from_utf8("_synchronicity.example.").unwrap();
        let mut answers = signed_txt(&owner, "v=sync1 id=nas nk=x", &signer);
        for record in &mut answers {
            record.proof = Proof::Secure;
            record.ttl = 3600;
        }
        let name = "_synchronicity.example.";
        let expires_at = u64::from(txt_rrsig(&answers).input().sig_expiration.get());

        // Well inside the window: the record's own TTL decides, clamped.
        let early = expires_at - 4 * 3600;
        let validated = secure_txt(name, &answers, Some(early)).expect("a signed answer");
        assert_eq!(validated.expires_at, expires_at);
        assert_eq!(validated.ttl, Duration::from_secs(3600));

        // Near the end of it the signature decides: a replay accepted here
        // buys minutes rather than the hour the record claims.
        let late = expires_at - 90;
        let validated = secure_txt(name, &answers, Some(late)).expect("a signed answer");
        assert_eq!(validated.ttl, Duration::from_secs(90));

        // Past the end the floor takes over (hickory has already refused the
        // answer by then; this branch exists so the arithmetic cannot
        // underflow), and the clamp's other bound: no TTL exceeds the cap.
        assert_eq!(
            secure_txt(name, &answers, Some(expires_at + 1))
                .expect("a signed answer")
                .ttl,
            MIN_TTL
        );
        assert_eq!(clamp_ttl(Duration::from_secs(999_999)), MAX_TTL);
    }

    #[test]
    fn off_path_rrsigs_are_stripped_from_every_section() {
        let child = Name::from_utf8("cluster.example.").unwrap();
        let parent = Name::from_utf8("example.").unwrap();
        let attacker = Name::from_utf8("attacker.example.").unwrap();
        let owner = Name::from_utf8("_synchronicity.cluster.example.").unwrap();

        let on_path = signed_rrsig(&owner, &child);
        let parent_ds = signed_rrsig(&child, &parent);
        let off_path = signed_rrsig(&owner, &attacker);
        let txt = Record::from_rdata(owner, 300, RData::TXT(TXT::new(vec!["v=sync1".into()])));

        let mut message = Message::response(1, OpCode::Query);
        message.add_answer(on_path.clone());
        message.add_answer(off_path.clone());
        message.add_answer(txt.clone());
        message.add_authority(parent_ds);
        message.add_authority(off_path.clone());
        message.add_additional(off_path);
        message.add_additional(on_path);

        let mut response = DnsResponse::from_message(message).expect("response");
        strip_off_path_rrsigs(&mut response);

        let is_rrsig =
            |record: &Record| matches!(record.data, RData::DNSSEC(DNSSECRData::RRSIG(_)));
        assert_eq!(response.answers.iter().filter(|r| is_rrsig(r)).count(), 1);
        assert!(response.answers.iter().any(|r| r.data == txt.data));
        assert_eq!(response.authorities.len(), 1);
        assert_eq!(response.additionals.len(), 1);
        // What survived every section encloses its owner — RFC 4035 §5.3.1,
        // which the resolver library skips.
        assert!(response.answers.iter().all(rrsig_signer_encloses_owner));
        assert!(response.authorities.iter().all(rrsig_signer_encloses_owner));
        assert!(response.additionals.iter().all(rrsig_signer_encloses_owner));
    }

    fn p256_at(origin: &Name) -> (DNSKEY, DnssecSigner) {
        let algorithm = Algorithm::ECDSAP256SHA256;
        let pkcs8 = EcdsaSigningKey::generate_pkcs8(algorithm).expect("keygen");
        let key = EcdsaSigningKey::from_pkcs8(&pkcs8, algorithm).expect("key load");
        let public = key.to_public_key().expect("public key");
        let dnskey = DNSKEY::from_key(&public);
        let signer = DnssecSigner::new(
            dnskey.clone(),
            Box::new(key),
            origin.clone(),
            std::time::Duration::from_secs(86_400),
        );
        (dnskey, signer)
    }

    fn colliding_p256(origin: &Name, tag: u16) -> (DNSKEY, DnssecSigner) {
        // A key tag is 16 bits; 4,000,000 draws leave a 1-in-21 geometric
        // tail, so the loop is long on purpose.
        for _ in 0..4_000_000 {
            let pair = p256_at(origin);
            if pair.0.calculate_key_tag().ok() == Some(tag) {
                return pair;
            }
        }
        panic!("no P-256 key with tag {tag} in 4_000_000 draws");
    }

    fn sign_rrset(set: &RecordSet, signer: &DnssecSigner) -> RRSIG {
        RRSIG::from_rrset(
            set,
            DNSClass::IN,
            time::OffsetDateTime::now_utc() - time::Duration::hours(1),
            signer,
        )
        .expect("sign rrset")
    }

    fn signed_txt(owner: &Name, text: &str, signer: &DnssecSigner) -> Vec<Record> {
        let mut set = RecordSet::new(owner.clone(), RecordType::TXT, 0);
        set.insert(
            Record::from_rdata(
                owner.clone(),
                300,
                RData::TXT(TXT::new(vec![text.to_string()])),
            ),
            0,
        );
        let rrsig = sign_rrset(&set, signer);
        set.insert_rrsig(Record::from_rdata(
            owner.clone(),
            300,
            RData::DNSSEC(DNSSECRData::RRSIG(rrsig)),
        ));
        set.records(true).cloned().collect()
    }

    fn txt_rrsig(records: &[Record]) -> RRSIG {
        for record in records {
            if let RData::DNSSEC(DNSSECRData::RRSIG(rrsig)) = &record.data {
                return rrsig.clone();
            }
        }
        panic!("signed set has no RRSIG");
    }

    fn secure_dnskey(zone: &Name, dnskey: DNSKEY) -> Record {
        let mut record = Record::from_rdata(
            zone.clone(),
            300,
            RData::DNSSEC(DNSSECRData::DNSKEY(dnskey)),
        );
        record.proof = Proof::Secure;
        record
    }

    fn secure_dnskey_rrsig(zone: &Name, keys: &[DNSKEY], signer: &DnssecSigner) -> Record {
        let mut set = RecordSet::new(zone.clone(), RecordType::DNSKEY, 0);
        for key in keys {
            set.insert(
                Record::from_rdata(
                    zone.clone(),
                    300,
                    RData::DNSSEC(DNSSECRData::DNSKEY(key.clone())),
                ),
                0,
            );
        }
        let rrsig = sign_rrset(&set, signer);
        let mut record =
            Record::from_rdata(zone.clone(), 300, RData::DNSSEC(DNSSECRData::RRSIG(rrsig)));
        record.proof = Proof::Secure;
        record
    }

    fn signed_rrsig(owner: &Name, signer_name: &Name) -> Record {
        let (_, signer) = p256_at(signer_name);
        signed_txt(owner, "x", &signer)
            .into_iter()
            .find(|record| matches!(record.data, RData::DNSSEC(DNSSECRData::RRSIG(_))))
            .expect("rrsig")
    }
}
