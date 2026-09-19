//! Zone-key transparency: the offline half (docs/REKOR-ZONE-KEY.md).
//!
//! DNSSEC answers *is this key authorized for this zone?* by delegation, and
//! every link in that delegation is an institution that can be compromised
//! or compelled: a substituted DS names an attacker's key, the attacker
//! signs a perfectly valid zone, and nothing in DNSSEC makes the substitution
//! visible. Requiring the zone key to appear in a public, append-only log
//! does not prevent the substitution — it makes it *public*: an attacker
//! must either log their key under the operator's apex, where a monitor sees
//! it, or fail validation here.
//!
//! Everything in this module is pure and offline. A proof arrives inside the
//! zone (a TXT record at `_synchronicity-rekor.<apex>`), so the client never
//! talks to Rekor: the proof verifies against the DNSKEY the chain already
//! validated and against a *pinned* log key, never against where it came
//! from. Fail closed — every check refuses rather than degrades, and the
//! caller keeps its cached member set (§4.3).
//!
//! What an entry claims is an authorized **key set**: the apex DNSKEY RRset
//! its embedded chain proves. The entry's own signature is *attribution* —
//! it names whoever built the entry — and authorizes nothing. Authorization
//! is the chain, and only the chain: an attacker able to forge a chain for a
//! rogue key holds that key and could sign anything it liked, so a
//! possession requirement would add no security while making a
//! provider-held zone key (Cloudflare's, Bunny's) impossible to log at all.
//! See docs/EXTERNAL-DNS-PROVIDER.md.
//!
//! # Wire format
//!
//! `RekorProof` v4, big-endian throughout:
//!
//! ```text
//! u8       version            = 4
//! u8[32]   log_id               SHA-256 of the log's DER SubjectPublicKeyInfo
//! u64      log_index
//! u16+[]   statement            the in-toto Statement, byte-exact (PAE preimage)
//! u16+[]   canonicalized_body   the Rekor entry body, verbatim (leaf preimage)
//! u16+[]   checkpoint           signed note: origin, tree size, root hash, sigs
//! u8+[32]* inclusion_path       Merkle audit path, leaf to root
//! ```
//!
//! There is no key-tag selector: a record's subject is a set, so a client
//! tries each record the zone serves and membership decides.
//!
//! # What this format pins — and why the verifier is a *certificate*
//!
//! A proof is only checkable if both halves agree byte for byte on what was
//! hashed, so this record carries the exact two byte strings the public log
//! itself commits to:
//!
//! 1. **The log entry** is a `hashedrekord` v0.0.2 body. Rekor v2 accepts no
//!    other entry type — `internal/server/service.go` rejects everything with
//!    "invalid type, must be hashedrekord", and the deprecated DSSE type only
//!    ever stored a `payloadHash` — so a DSSE-signed Statement is logged as a
//!    `hashedrekord` over the DSSE **PAE**: `data.digest = SHA-256(PAE)`,
//!    `signature.content` = the ECDSA-P256 signature over that digest (DER,
//!    not raw). The log returns these bytes as `canonicalizedBody` and this
//!    record carries them **verbatim** — nothing here re-canonicalizes JSON,
//!    because the log already did and the leaf commits to its output.
//! 2. **The verifier is an `x509Certificate`, never a raw public key.**
//!    A raw-key entry is *apex-anonymous*: its leaf holds
//!    a digest, a signature and 91 bytes of SubjectPublicKeyInfo, and nothing
//!    that names a zone. Nobody could monitor a zone for newly published
//!    keys, which makes the transparency claim hollow — the threat model has
//!    a compromised upstream DNS provider in it, so DNS-served state cannot
//!    be the monitoring channel. Rekor's `Verifier` is a oneof of exactly two
//!    arms, and it performs **no** certificate validation on the second one
//!    (`pkg/verifier/certificate`: parse, take the public key, stop), copying
//!    the certificate DER verbatim into the canonicalized body. So a
//!    self-signed certificate carrying the apex as a `dNSName` SAN puts the
//!    zone name, in the clear, inside the Merkle leaf — where a monitor
//!    walking the log's tiles can index it (docs/REKOR-ZONE-KEY.md §5.5).
//! 3. **The Merkle leaf** is `SHA-256(0x00 || canonicalized_body)`, and an
//!    interior node is `SHA-256(0x01 || left || right)` — RFC 6962 §2.1.
//!
//! The Statement travels alongside the body (not inside it) because the body
//! commits only to the PAE *digest*; the client re-derives that digest from
//! the Statement bytes and refuses the proof if they disagree. None of this
//! is negotiable per deployment — a v4 of the record format is how it changes.

use std::path::Path;

use aws_lc_rs::{digest, signature};
use hickory_resolver::proto::dnssec::TrustAnchors;

use crate::{
    chain::{self, ChainError},
    pubkey::{RawKey, Scheme, P256_SPKI_PREFIX},
    x509::Certificate,
};

/// The only `RekorProof` version this build accepts: the entry authorizes
/// the apex DNSKEY RRset its chain proves, the entry signature is
/// attribution by the certificate's own key, and the wire carries no
/// key-tag selector. Any other version byte is refused as malformed.
pub(crate) const PROOF_VERSION: u8 = 4;

/// The token every chunk of a proof record starts with.
pub(crate) const PROOF_TXT_PREFIX: &str = "sync1p";

/// The most base64url characters one record carries.
///
/// Chosen against the tightest provider limit rather than against DNS:
/// Cloudflare refuses a TXT record past 4096 **wire-format** bytes, which
/// counts the one-byte length prefix each ≤255-byte character-string adds.
/// At this size a record is ~2 KB of payload plus a ~22-byte header and
/// ~8 prefixes, so it clears that ceiling with room for a header that grows.
pub const PROOF_CHUNK_CHARS: usize = 2000;

/// The label the proof records live under, one below the zone apex.
pub const REKOR_TXT_PREFIX: &str = "_synchronicity-rekor";

/// The DSSE payload type of an in-toto Statement.
pub const DSSE_PAYLOAD_TYPE: &str = "application/vnd.in-toto+json";

/// The only entry kind Rekor v2 accepts, and the only one this design logs.
pub(crate) const HASHEDREKORD_KIND: &str = "hashedrekord";

/// The entry API version the body must declare.
pub(crate) const HASHEDREKORD_API_VERSION: &str = "0.0.2";

/// The `hashedrekord` v0.0.2 digest algorithm name the body carries.
pub(crate) const HASHEDREKORD_DIGEST_ALGORITHM: &str = "SHA2_256";

/// The `hashedrekord` v0.0.2 verifier key-details tag for a P-256 key.
pub(crate) const HASHEDREKORD_KEY_DETAILS: &str = "PKIX_ECDSA_P256_SHA_256";

/// The in-toto Statement type the entry must declare.
pub(crate) const STATEMENT_TYPE: &str = "https://in-toto.io/Statement/v1";

/// The predicate type carrying the zone-key claim.
///
/// v2 is the key-set claim: the subject is the apex DNSKEY RRset the chain
/// proves, and the DSSE signer is whoever published the entry — attribution,
/// not possession.
pub(crate) const PREDICATE_TYPE: &str = "https://synchronicity.sh/zone-key/v2";

/// Why a zone-key transparency record was refused.
///
/// The variants are the failure *classes* `synch doctor` explains: an absent
/// record on a not-yet-upgraded control plane reads differently from a
/// binding mismatch, which is an alarm.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProofError {
    /// The record could not be decoded as a v4 `RekorProof`.
    #[error("malformed proof: {0}")]
    Malformed(String),
    /// The entry signature does not verify under the certificate's own key:
    /// the entry misattributes itself, so nothing about it can be believed.
    #[error("attribution: {0}")]
    Attribution(String),
    /// The Statement does not describe the key and zone that were observed.
    #[error("binding: {0}")]
    Binding(String),
    /// The entry is not in the tree the checkpoint commits to.
    #[error("inclusion: {0}")]
    Inclusion(String),
    /// The checkpoint is not signed by the log it claims to come from.
    #[error("checkpoint: {0}")]
    Checkpoint(String),
    /// No pinned key matches the proof's `log_id`: the entry lives in a log
    /// this client was never told to trust.
    #[error("unknown log: {0}")]
    UnknownLog(String),
    /// The entry carries no DNSSEC chain, or one that does not establish
    /// that this key was ever authorized for this zone.
    ///
    /// A client already knows the answer — it validated the delegation
    /// natively. It enforces this **on behalf of monitors**: an entry whose
    /// chain is absent or broken is one a monitor files as noise, so a client
    /// that accepted it would hand an attacker a key that works against
    /// victims *and* raises no alarm (§5.5, the tier-A/tier-B invariant).
    #[error("chain: {0}")]
    Chain(String),
}

/// One decoded zone-key transparency record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RekorProof {
    /// SHA-256 of the log's DER SubjectPublicKeyInfo; selects the pinned key.
    pub log_id: [u8; 32],
    /// The entry's index in the log, needed to walk the audit path.
    pub log_index: u64,
    /// The in-toto Statement, byte-exact — the DSSE PAE preimage the entry's
    /// digest commits to.
    pub statement: Vec<u8>,
    /// The Rekor `hashedrekord` entry body, exactly as the log returned it in
    /// `canonicalizedBody` — the Merkle leaf preimage. The entry signature
    /// and the signer's *certificate* live inside these bytes, and the
    /// certificate is what carries the apex and the monitors' evidence.
    pub canonicalized_body: Vec<u8>,
    /// The signed note the log published, verbatim.
    pub checkpoint: Vec<u8>,
    /// The Merkle audit path, leaf to root.
    pub inclusion_path: Vec<[u8; 32]>,
}

impl RekorProof {
    /// Encodes the record in the wire format above.
    ///
    /// `None` for a record that does not fit it — a blob past 65535 bytes or
    /// an audit path past 255 hops. Refusing beats emitting *something*: this
    /// format exists so two implementations agree byte for byte, and a
    /// truncated or wrapped length is two implementations disagreeing about
    /// the failure as well as about the record. Both sides refuse.
    ///
    /// **Publisher-side, and not compiled into the client.** The control plane
    /// writes these records; a resolving client only ever decodes them. This
    /// half exists as the second independent encoder the cross-validation
    /// fixtures are held still by, so it lives behind the same gate as the rest
    /// of the harness (see [`crate::sim`]).
    #[cfg(any(test, feature = "sim"))]
    pub fn encode(&self) -> Option<Vec<u8>> {
        for blob in [&self.statement, &self.canonicalized_body, &self.checkpoint] {
            u16::try_from(blob.len()).ok()?;
        }
        u8::try_from(self.inclusion_path.len()).ok()?;
        Some(self.encode_unchecked())
    }

    /// The encoder proper, for a record already known to fit.
    #[cfg(any(test, feature = "sim"))]
    fn encode_unchecked(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            43 + self.statement.len()
                + self.canonicalized_body.len()
                + self.checkpoint.len()
                + 32 * self.inclusion_path.len(),
        );
        out.push(PROOF_VERSION);
        out.extend_from_slice(&self.log_id);
        out.extend_from_slice(&self.log_index.to_be_bytes());
        for blob in [&self.statement, &self.canonicalized_body, &self.checkpoint] {
            let len = u16::try_from(blob.len()).expect("checked by encode");
            out.extend_from_slice(&len.to_be_bytes());
            out.extend_from_slice(blob);
        }
        out.push(u8::try_from(self.inclusion_path.len()).expect("checked by encode"));
        for hash in &self.inclusion_path {
            out.extend_from_slice(hash);
        }
        out
    }

    /// Decodes a v4 record, refusing anything with bytes left over — a
    /// record that decodes two ways is a record an attacker can steer.
    pub fn decode(bytes: &[u8]) -> Result<RekorProof, ProofError> {
        let mut reader = Reader::new(bytes);
        let version = reader.u8("version")?;
        if version != PROOF_VERSION {
            return Err(ProofError::Malformed(format!(
                "version {version} is not {PROOF_VERSION}"
            )));
        }
        let log_id = reader.array32("log id")?;
        let log_index = reader.u64("log index")?;
        let statement = reader.blob16("statement")?.to_vec();
        let canonicalized_body = reader.blob16("canonicalized body")?.to_vec();
        let checkpoint = reader.blob16("checkpoint")?.to_vec();
        let hops = reader.u8("inclusion path length")?;
        let mut inclusion_path = Vec::with_capacity(usize::from(hops));
        for _ in 0..hops {
            inclusion_path.push(reader.array32("inclusion path hash")?);
        }
        reader.finish()?;
        Ok(RekorProof {
            log_id,
            log_index,
            statement,
            canonicalized_body,
            checkpoint,
            inclusion_path,
        })
    }

    /// Renders the record as the TXT payloads a zone serves for it, or
    /// `None` for a record too large for the wire format itself (see
    /// [`RekorProof::encode`]).
    ///
    /// A proof does not fit in one record. Managed DNS providers cap a TXT
    /// record well below what a full proof needs — Cloudflare refuses
    /// anything past 4096 wire-format bytes, and an ICANN-rooted proof is
    /// about twice that — so the payload is split across several records at
    /// the same owner name, each carrying a header that says where it
    /// belongs:
    ///
    /// ```text
    /// sync1p <group> <index>/<total> <base64url chunk>
    /// ```
    ///
    /// `group` is the first four bytes of the SHA-256 of the encoded proof,
    /// in hex. It ties one proof's chunks together where several proofs
    /// share a name (a rollover serves two), and it is *checked* after
    /// reassembly, so chunks of different proofs cannot be spliced into
    /// something that decodes.
    ///
    /// Publisher-side, like [`RekorProof::encode`], and behind the same gate.
    #[cfg(any(test, feature = "sim"))]
    pub fn to_txt(&self) -> Option<Vec<String>> {
        let encoded = self.encode()?;
        let group = hex_lower(&sha256(&encoded)[..4]);
        let payload = base64url_encode(&encoded);
        let chunks: Vec<&str> = payload
            .as_bytes()
            .chunks(PROOF_CHUNK_CHARS)
            .map(|c| std::str::from_utf8(c).expect("base64url is ASCII"))
            .collect();
        let total = chunks.len();
        // The header carries a one-byte index and total, so a proof that
        // needed more records than that could not name its own pieces.
        u8::try_from(total).ok()?;
        Some(
            chunks
                .iter()
                .enumerate()
                .map(|(i, chunk)| format!("{PROOF_TXT_PREFIX} {group} {}/{total} {chunk}", i + 1))
                .collect(),
        )
    }

    /// The RFC 6962 leaf hash of this entry: over the log's own body bytes.
    ///
    /// This is the whole point of the format — the leaf commits to the exact
    /// `canonicalizedBody` the log returned, so an inclusion walk under this
    /// hash reaches a root the real Sigstore computed over the same bytes.
    pub fn leaf_hash(&self) -> [u8; 32] {
        leaf_hash(&self.canonicalized_body)
    }
}

/// Reassembles every complete proof served at one owner name.
///
/// The records at `_synchronicity-rekor.<zone>` are a bag: chunks of one
/// proof, chunks of another during a rollover, and — because the set is
/// whatever the zone published — possibly records this build cannot read at
/// all. Chunks are grouped by their `group` field, and a group yields a
/// proof only when every index it claims is present, the pieces concatenate
/// to valid base64url, and the digest of the result is the group it said it
/// was. Anything else is reported rather than silently dropped, so "the zone
/// published gibberish" stays distinguishable from "the zone published
/// nothing".
///
/// One malformed record never sinks a readable one: each group is decided on
/// its own, which is what lets a mid-rollover zone serve a record this build
/// does not understand beside the one it needs.
///
/// **A record added at an index the zone already published is a
/// contradiction, and the group is refused.** Records claiming different
/// `total`s are still separate readings — a rollover legitimately serves a
/// five-part set beside a nine-part one — but within one reading each index
/// arrives once or not at all.
///
/// This is a deliberate narrowing from treating duplicates as alternatives and
/// trying every combination. That is a product over the per-index counts, it
/// needs a cap to be safe, and the cap is what an injector then aims at —
/// past it the group is refused whole, honest assembly included. It never
/// bought a surviving answer against the only party who can reach these
/// records: every one of them arrives DNSSEC-validated, so duplicating an
/// index takes the zone's signing key, and whoever holds that can delete the
/// records instead. Availability against the zone's own signer is not
/// defensible here and is not claimed (§4.3); *authorization* is, and the
/// group digest and the chain walk hold it whatever the records look like.
pub(crate) fn proofs_from_txt(records: &[String]) -> Vec<Result<RekorProof, ProofError>> {
    use std::collections::BTreeMap;

    // group -> the (index, total, chunk) readings its records support.
    let mut groups: BTreeMap<String, Vec<(usize, usize, String)>> = BTreeMap::new();
    let mut junk: Vec<ProofError> = Vec::new();
    for record in records {
        match parse_chunk(record) {
            Ok((group, index, total, chunk)) => {
                groups.entry(group).or_default().push((index, total, chunk));
            }
            Err(e) => junk.push(e),
        }
    }

    let mut out = Vec::new();
    for (group, parts) in groups {
        out.push(assemble_group(&group, &parts));
    }
    // Only worth reporting when nothing reassembled: a zone mid-upgrade may
    // legitimately serve a record beside the ones that work.
    if out.is_empty() {
        out.extend(junk.into_iter().map(Err));
    }
    out
}

/// Reassembles one group: one reading per part count its records claim.
///
/// **One chunk per index, and a duplicated index is a contradiction rather
/// than a candidate.** The alternative — duplicates as alternatives, trying
/// every combination — is a product over the per-index counts, needs a cap,
/// and the cap is what an injector then aims at; past it the group is refused
/// whole, honest assembly and all. It bought nothing to be worth that: every
/// record arrives DNSSEC-validated, so the only party who can duplicate an
/// index is the zone's own signer, who can delete the records or refuse the
/// name instead — availability against the zone is not defensible, and
/// *authorization* is what the group digest and the chain walk hold.
///
/// So the rule is the one a self-contradicting answer gets everywhere else:
/// read it one way, refuse when the records do not agree on what that way is.
/// Distinct claimed totals are still separate readings — a sum over the
/// records, not a product — which is what lets a `1/5` set and a `1/9` set
/// coexist at one name during a rollover.
fn assemble_group(group: &str, parts: &[(usize, usize, String)]) -> Result<RekorProof, ProofError> {
    use std::collections::BTreeMap;

    // Indexed once, by `(total, index)`. The obvious shape — rescanning
    // `parts` for every index of every claimed total — is quadratic in the
    // record count, and the records are attacker-chosen: sixteen names of
    // validated TXT hold ~27,000 minimal records, and one group spread across
    // totals 1..=255 costs about a second of CPU inside one `poll`, with no
    // await for a timeout to fire at. The candidate cap runs after this, so
    // it does not bound it.
    //
    // A BTreeMap rather than a HashMap: the keys are the zone's, and iteration
    // order is not something an answer should get to choose.
    let mut by_total: BTreeMap<usize, BTreeMap<usize, &str>> = BTreeMap::new();
    let mut duplicated: BTreeMap<usize, usize> = BTreeMap::new();
    for (index, total, chunk) in parts {
        if by_total
            .entry(*total)
            .or_default()
            .insert(*index, chunk.as_str())
            .is_some()
        {
            duplicated.entry(*total).or_insert(*index);
        }
    }

    let mut last: Option<ProofError> = None;
    // Smallest count first: a complete honest set is tried before any larger
    // count an added record invented.
    for (total, chunks) in &by_total {
        let total = *total;
        // Only records that agree on the count take part in its reading. A
        // record claiming some other total is a different reading, not a hole
        // in this one.
        let mut payload = String::new();
        let mut broken = None;
        for index in 1..=total {
            if duplicated.get(&total).is_some_and(|first| *first == index) {
                broken = Some(ProofError::Malformed(format!(
                    "proof {group} is served in {total} part(s) and part {index} \
                     arrived more than once, so the zone does not agree with \
                     itself about what it published"
                )));
                break;
            }
            match chunks.get(&index).copied() {
                Some(chunk) => payload.push_str(chunk),
                None => {
                    broken = Some(ProofError::Malformed(format!(
                        "proof {group} is served in {total} part(s) and part {index} \
                         did not arrive"
                    )));
                    break;
                }
            }
        }
        if let Some(e) = broken {
            last = Some(e);
            continue;
        }
        match decode_group(group, &payload) {
            Ok(proof) => return Ok(proof),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| {
        ProofError::Malformed(format!("proof {group} is served in no parts at all"))
    }))
}

/// Which part a record is, for a publisher deciding where to put it.
///
/// Publisher-side: a client derives the name it asks for from the index it
/// wants, never the other way round.
#[cfg(any(test, feature = "sim"))]
pub(crate) fn part_index_of(record: &str) -> Option<usize> {
    parse_chunk(record).ok().map(|(_, index, _, _)| index)
}

/// The most parts a client will gather to reassemble one proof. An
/// ICANN-rooted proof is ~6.1 KB encoded — five records at
/// [`PROOF_CHUNK_CHARS`] apiece (docs/REKOR-ZONE-KEY.md §3) — and sixteen
/// covers that with room for a chain that grows, while the work a single
/// record can demand stays bounded at fifteen extra lookups instead of the
/// format's 254.
pub const MAX_PROOF_PARTS: usize = 16;

/// The largest part count any record at this name claims, capped at
/// [`MAX_PROOF_PARTS`] — what tells a client how many more names to ask
/// for. Read from already-DNSSEC-validated records, so it is the zone's own
/// statement, but still only a hint about *work*, never about truth: a
/// wrong count yields a set that fails to reassemble — a refusal, not an
/// acceptance.
///
/// The cap is what keeps "a hint about work" from being a hint about *how
/// much*: the format allows a `total` of 255, and one record spelling
/// `1/255` would cost every resolving client 254 round trips before a byte
/// was verified — a hostile or mistaken zone stalling every other domain's
/// refresh past the grace window, and the threat model's attacker *is* the
/// zone, so "it would only be hurting itself" does not hold.
pub fn parts_claimed(records: &[String]) -> usize {
    records
        .iter()
        .filter_map(|record| parse_chunk(record).ok())
        .map(|(_, _, total, _)| total)
        .max()
        .unwrap_or(1)
        .min(MAX_PROOF_PARTS)
}

/// One record, as `sync1p <group> <index>/<total> <chunk>`.
fn parse_chunk(record: &str) -> Result<(String, usize, usize, String), ProofError> {
    let malformed = |why: &str| ProofError::Malformed(format!("proof record: {why}"));
    let mut fields = record.split_whitespace();
    match fields.next() {
        Some(PROOF_TXT_PREFIX) => {}
        _ => return Err(malformed("not a sync1p record")),
    }
    let group = fields.next().ok_or_else(|| malformed("no group"))?;
    if group.len() != 8 || !group.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(malformed("the group is not eight hex digits"));
    }
    let counter = fields.next().ok_or_else(|| malformed("no index"))?;
    let (index, total) = counter
        .split_once('/')
        .ok_or_else(|| malformed("the index is not <index>/<total>"))?;
    let index: usize = index
        .parse()
        .map_err(|_| malformed("the index is not a number"))?;
    let total: usize = total
        .parse()
        .map_err(|_| malformed("the total is not a number"))?;
    if index == 0 || total == 0 || index > total || total > 255 {
        return Err(malformed("the index is outside 1..=total"));
    }
    let chunk = fields.next().ok_or_else(|| malformed("no payload"))?;
    if fields.next().is_some() {
        return Err(malformed("trailing fields"));
    }
    Ok((group.to_ascii_lowercase(), index, total, chunk.to_string()))
}

/// Decodes a reassembled payload and holds it to the group it claimed.
fn decode_group(group: &str, payload: &str) -> Result<RekorProof, ProofError> {
    let bytes = base64url_decode(payload)?;
    let digest = hex_lower(&sha256(&bytes)[..4]);
    if digest != group {
        return Err(ProofError::Malformed(format!(
            "proof {group} reassembled to something whose digest is {digest}: \
             its parts are not all from one record"
        )));
    }
    RekorProof::decode(&bytes)
}

/// The zone key an answer was actually signed by: what the proof must bind.
///
/// The apex and key tag come from the RRSIG the validator already accepted,
/// and the rdata from the DNSKEY RRset at that apex — so this is an
/// observation of the chain, never something the proof gets to assert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZoneKey<'a> {
    /// The membership domain being resolved. The apex an entry claims has to
    /// contain it: an entry for a *sibling* namespace must not authorize
    /// keys used to answer for this one, or an attacker could point the
    /// requirement at a name the operator's monitor never watches.
    pub domain: &'a str,
    /// The zone whose RRSIG signed the answer — the bottom of the entry's
    /// ladder, and the zone whose key set the chain proves. Not necessarily
    /// the apex: a control plane at `sync.example.com` served out of the
    /// `example.com` zone is signed by `example.com`.
    pub signing_zone: &'a str,
    /// The key tag the RRSIG selected.
    pub key_tag: u16,
    /// The DNSKEY rdata: flags, protocol, algorithm, public key.
    pub dnskey_rdata: &'a [u8],
}

/// The raw 64-byte uncompressed P-256 point inside a DER
/// SubjectPublicKeyInfo, or `None` for any other key type.
///
/// Used on the entry *signer's* key — the certificate's own — which this
/// design requires to be P-256 (the `hashedrekord` verifier's
/// `PKIX_ECDSA_P256_SHA_256`). The zone keys the entry authorizes are not
/// constrained by algorithm at all: they are matched by rdata membership in
/// the chain-proven RRset.
pub(crate) fn p256_point(spki: &[u8]) -> Option<&[u8]> {
    let point = spki.get(27..)?;
    if point.len() != 64 || p256_spki(point) != spki {
        return None;
    }
    Some(point)
}

/// What a verified record establishes, for logs and `synch doctor`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedRecord {
    /// The entry's index in the log.
    pub log_index: u64,
    /// The tree size the checkpoint commits to.
    pub tree_size: u64,
    /// The checkpoint's origin line — which log, in its own words.
    pub origin: String,
    /// `create`, `rollover` or `retire`.
    pub action: String,
}

/// Verifies a proof completely, offline (§4.2).
///
/// The chain, in the order the validated algorithm runs it: the log vouches
/// for a leaf (checkpoint, inclusion) whose body names this zone in a
/// certificate (apex binding) carrying a DNSSEC chain that proves an
/// authorized key set (authorization), whose own key signed the entry
/// (attribution), whose digest is the DSSE PAE of this Statement, and whose
/// Statement describes exactly that proven set — which the key that signed
/// this answer is a member of (key binding). Any single failure refuses the
/// whole record — no partial credit.
///
/// There is deliberately no possession check: an attacker able to forge an
/// authorized chain for a rogue key holds that key and could sign possession
/// too, so requiring the *zone* key's signature would add nothing — while
/// making a provider-held key (a zone hosted and signed by Cloudflare or
/// Bunny) impossible to log. Authorization is the chain.
///
/// # The chain extension, and why the client verifies a chain it does not need
///
/// The client validated this zone's delegation natively, all the way to its
/// trust anchor, before reaching this function — but *the client enforces
/// whatever property makes an entry discoverable, or an attacker simply
/// omits it*. **The chain is required, and verified cryptographically**: its
/// absence would *silence* a monitor — a chainless entry is tier B, recorded
/// and not reported — handing an attacker a key that works against victims
/// and rings no bell. So the client refuses it, which makes "client-accepted"
/// imply "tier A" (docs/REKOR-ZONE-KEY.md §5.5). RRSIG validity *windows*
/// are deliberately not checked here; see [`crate::chain`] for the reasons.
///
/// **Unknown extensions are ignored, and that is load-bearing.** Nothing here
/// refuses a certificate for carrying an extension this build has no name
/// for — the conformance fixture carries some — and it still verifies.
///
/// A `retire` entry is refused. The action is the last thing `check_binds`
/// tests, so a retire carrying a chain is chain-checked first
/// and refused on its action, while the chainless retire the publish side may
/// emit (a retired zone may have no DS left) is refused as `Chain` several
/// steps earlier. Either way retirement is a monitor breadcrumb (§2), never
/// authorization — accepting one would reopen the hole the chain requirement
/// closes.
pub fn verify(
    proof: &RekorProof,
    key: &ZoneKey<'_>,
    logs: &LogKeys,
    anchors: &TrustAnchors,
) -> Result<VerifiedRecord, ProofError> {
    let log_key = logs.find(&proof.log_id).ok_or_else(|| {
        ProofError::UnknownLog(match logs.is_empty() {
            // Naming the real reason: an empty pin set is a build with no
            // log key in it, not a proof from the wrong log.
            true => "no log key is pinned in this build — name one with \
                     --rekor-key, or run with --rekor off"
                .to_string(),
            false => format!("no pinned key with id {}", hex_lower(&proof.log_id)),
        })
    })?;
    let checkpoint = Checkpoint::parse(&proof.checkpoint)?;

    // Inclusion: the entry's body is a leaf of the tree the checkpoint
    // commits to. The leaf is over the log's own `canonicalizedBody`, so
    // this walk reaches a root the log computed over the same bytes.
    verify_inclusion(
        proof.log_index,
        checkpoint.tree_size,
        proof.leaf_hash(),
        &proof.inclusion_path,
        checkpoint.root_hash,
    )?;

    // The log vouches: the checkpoint carries the pinned key's signature.
    checkpoint.verify_signature(log_key)?;

    // The body the leaf committed to: a hashedrekord over a PAE digest,
    // carrying the entry signature and the certificate that names the signer.
    let body = HashedRekordBody::parse(&proof.canonicalized_body)?;

    // Who the certificate says it is about — the parsed apex. Read through
    // `chain::identify`, the same reader the monitor uses, so no spelling of
    // a name can mean one thing here and another there.
    let claimed_apex = chain::identify(&body.certificate).map_err(chain_error)?;

    // Apex binding. The apex is the control plane's *name*, and the entry is
    // the only thing that says what it is — so it is not taken on trust but
    // pinned between two names the client already knows, and it has to sit
    // in between:
    //
    //   <signing zone>  ⊇  <claimed apex>  ⊇  <membership domain>
    //
    // The lower bound is what stops an entry for a sibling namespace from
    // authorizing keys used to answer here: a monitor watching this domain's
    // delegation path would never look at `other.example.com`, so an entry
    // naming it must not be usable against `sync.example.com`. The upper
    // bound is checked inside the chain walk, where the ladder's own bottom
    // link is the signing zone — see `chain::validate`.
    let domain = chain::parse_name(key.domain).map_err(chain_error)?;
    if !claimed_apex.zone_of(&domain) {
        return Err(ProofError::Binding(format!(
            "the entry's certificate names {claimed_apex}, which does not contain \
             the membership domain {domain} it would be authorizing",
        )));
    }

    // Authorization: the entry carries a chain that proves, to anyone
    // reading the log and nothing else, which keys were delegated for this
    // zone. `chain::authorize` is the *only* way to ask, and it is the same
    // call the monitor makes — neither side gets to choose the apex it feeds
    // the walk. See `chain::authorize` for the break that rule exists to
    // prevent. What comes back is the proven key set, and it decides the key
    // binding below; a chainless entry proves nothing and is refused.
    let authorized = chain::authorize(&body.certificate, anchors).map_err(chain_error)?;

    // And the ladder's bottom is the zone that actually signed this answer.
    // Without this an entry could carry a valid chain for some *other* zone
    // that happens to enclose the apex, and the keys it proves would be that
    // zone's rather than the one the client is talking to.
    let observed_signer = chain::parse_name(key.signing_zone).map_err(chain_error)?;
    if authorized.signing_zone != observed_signer {
        return Err(ProofError::Binding(format!(
            "the entry's chain is signed by {}, the answer was signed by {observed_signer}",
            authorized.signing_zone
        )));
    }

    // Attribution: the entry signature verifies under the certificate's own
    // key — the entry is what its signer made, whoever that is. Rekor signs
    // the hashedrekord's `data.digest` as a prehash — which, because that
    // digest *is* SHA-256(PAE), is the same signature as ECDSA-SHA256 over
    // the PAE itself. Verifying it over the PAE is how aws-lc-rs is asked
    // the question. Rekor entry signatures are ASN.1/DER, not the raw r||s
    // of DNSSEC.
    let pae = pae(DSSE_PAYLOAD_TYPE, &proof.statement);
    let signer = p256_point(&body.certificate.spki).ok_or_else(|| {
        ProofError::Attribution("the certificate's key is not a P-256 SubjectPublicKeyInfo".into())
    })?;
    verify_ecdsa_p256_asn1(signer, &pae, &body.signature).map_err(|_| {
        ProofError::Attribution("the entry signature is not the certificate's key's".into())
    })?;

    // The entry commits to the DSSE PAE of *this* Statement — the body holds
    // only its digest, so the Statement bytes cannot be swapped under it.
    if body.digest != sha256(&pae) {
        return Err(ProofError::Binding(
            "the logged entry's digest is not this statement's DSSE PAE".into(),
        ));
    }

    // Statement and key binding: the Statement describes exactly the proven
    // set, and the key that signed this answer is a member of it.
    let statement = ZoneKeyStatement::parse(&proof.statement)?;
    statement.check_binds(&claimed_apex, key, &authorized.proven_keys)?;

    Ok(VerifiedRecord {
        log_index: proof.log_index,
        tree_size: checkpoint.tree_size,
        origin: checkpoint.origin,
        action: statement.action,
    })
}

/// Lifts a chain failure into the proof's own error class.
fn chain_error(error: ChainError) -> ProofError {
    ProofError::Chain(error.to_string())
}

/// The fields a `hashedrekord` v0.0.2 body carries that a proof turns on.
///
/// Rekor v2 accepts no other entry type; a DSSE-signed Statement is logged as
/// a `hashedrekord` over the DSSE PAE (docs/REKOR-ZONE-KEY.md §2). The body is
/// the log's own `canonicalizedBody`, verified here by re-deriving nothing —
/// the digest, signature and certificate are read out and checked against what
/// the DNSSEC chain independently observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashedRekordBody {
    /// The SHA-256 the entry commits to — must equal `SHA-256(PAE)`.
    pub digest: Vec<u8>,
    /// The entry signature, DER/ASN.1 ECDSA over the PAE.
    pub signature: Vec<u8>,
    /// The signer's certificate: the apex-carrying key envelope, parsed.
    pub certificate: Certificate,
    /// The certificate's DER, verbatim — what a monitor re-reads and what
    /// anyone auditing the log entry sees.
    pub certificate_der: Vec<u8>,
}

impl HashedRekordBody {
    /// Parses the body, refusing anything whose known members are the wrong
    /// shape and asserting every tag this design logs. Unknown members are
    /// tolerated so the entry format can grow.
    ///
    /// The `publicKey` arm of Rekor's verifier oneof is **not** handled, at
    /// all: an entry whose verifier is a bare key names no apex anywhere in
    /// its leaf, so no monitor could ever have seen it. There is no branch
    /// to reach and no fallback.
    pub fn parse(bytes: &[u8]) -> Result<HashedRekordBody, ProofError> {
        let bad = |why: String| ProofError::Malformed(format!("entry body: {why}"));
        let value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|e| bad(e.to_string()))?;
        let text = |v: &serde_json::Value, what: &str| -> Result<String, ProofError> {
            v.as_str()
                .map(str::to_string)
                .ok_or_else(|| bad(format!("{what} is not a string")))
        };
        let b64 = |v: &serde_json::Value, what: &str| -> Result<Vec<u8>, ProofError> {
            base64_decode(&text(v, what)?).map_err(|_| bad(format!("{what} is not base64")))
        };

        let kind = text(&value["kind"], "kind")?;
        let api_version = text(&value["apiVersion"], "apiVersion")?;
        if kind != HASHEDREKORD_KIND || api_version != HASHEDREKORD_API_VERSION {
            return Err(ProofError::Binding(format!(
                "the entry is {kind} {api_version}, not \
                 {HASHEDREKORD_KIND} {HASHEDREKORD_API_VERSION}"
            )));
        }
        let spec = &value["spec"]["hashedRekordV002"];
        if spec.is_null() {
            return Err(bad("not a hashedrekord v0.0.2 entry".into()));
        }

        let data = &spec["data"];
        let algorithm = text(&data["algorithm"], "data.algorithm")?;
        if algorithm != HASHEDREKORD_DIGEST_ALGORITHM {
            return Err(ProofError::Binding(format!(
                "entry digest algorithm {algorithm} is not {HASHEDREKORD_DIGEST_ALGORITHM}"
            )));
        }
        let signature = &spec["signature"];
        let verifier = &signature["verifier"];
        let key_details = text(&verifier["keyDetails"], "verifier.keyDetails")?;
        if key_details != HASHEDREKORD_KEY_DETAILS {
            return Err(ProofError::Binding(format!(
                "entry key details {key_details} are not {HASHEDREKORD_KEY_DETAILS}"
            )));
        }
        let certificate_der = match verifier["x509Certificate"]["rawBytes"].is_string() {
            true => b64(
                &verifier["x509Certificate"]["rawBytes"],
                "verifier.x509Certificate.rawBytes",
            )?,
            false => {
                return Err(ProofError::Binding(
                    "the entry's verifier is not an x509Certificate, so its leaf names \
                     no zone and no monitor could ever have seen it"
                        .into(),
                ))
            }
        };
        let certificate = Certificate::parse(&certificate_der).map_err(|e| bad(e.to_string()))?;
        Ok(HashedRekordBody {
            digest: b64(&data["digest"], "data.digest")?,
            signature: b64(&signature["content"], "signature.content")?,
            certificate,
            certificate_der,
        })
    }
}

/// One key of a Statement's claimed set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementKey {
    /// The DNSKEY key tag (RFC 4034 App. B, over the whole rdata).
    pub key_tag: u16,
    /// The DNSSEC algorithm number.
    pub algorithm: u8,
    /// The DNSKEY flags.
    pub flags: u16,
    /// Lowercase hex SHA-256 of the DNSKEY rdata.
    pub sha256: String,
}

/// The in-toto Statement a zone-key entry carries (§2).
///
/// The subject is a **key set** — the apex DNSKEY RRset the entry's chain
/// proves — rendered as one in-toto subject per key, each named by the apex
/// and identified by the SHA-256 of its DNSKEY rdata. The predicate repeats
/// the set with the tag, algorithm and flags a human or a monitor wants at a
/// glance; the two lists must agree entry for entry, in order.
///
/// Built here as well as parsed: the canonical rendering is the thing both
/// halves of the system have to agree on, so it lives with the parser that
/// depends on it rather than in whichever tool happened to need it first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneKeyStatement {
    /// The zone this key set is claimed for.
    pub apex: String,
    /// The claimed keys, in canonical order: ascending key tag, ties broken
    /// by the hex digest. Both renderers sort the same way, so one set has
    /// exactly one rendering.
    pub keys: Vec<StatementKey>,
    /// `create`, `rollover` or `retire`.
    pub action: String,
}

impl ZoneKeyStatement {
    /// The Statement for a key set, from the DNSKEY rdatas themselves —
    /// tag, algorithm, flags and digest are all derived, and the canonical
    /// order is applied here.
    ///
    /// A DNSKEY rdata is `flags(2) protocol(1) algorithm(1) key`, so anything
    /// under four bytes has no complete header to read. **Such an rdata
    /// renders as `flags: 0, algorithm: 0` — both zero together, never one
    /// real value beside one invented one** — and the Gleam publisher renders
    /// it the same way: deriving each field on its own would let a three-byte
    /// rdata come out as `flags: 258, algorithm: 0` here and all-zero there,
    /// and the Statement is what the two sides have to agree on byte for
    /// byte. Nothing legitimate reaches it either way — the chain walk's
    /// rdatas come out of parsed DNSKEY records, never shorter than their
    /// fixed header.
    pub fn for_keys(apex: &str, rdatas: &[Vec<u8>], action: &str) -> ZoneKeyStatement {
        let mut keys: Vec<StatementKey> = rdatas
            .iter()
            .map(|rdata| StatementKey {
                key_tag: chain::key_tag(rdata),
                algorithm: match rdata.len() >= 4 {
                    true => rdata[3],
                    false => 0,
                },
                flags: match rdata.len() >= 4 {
                    true => u16::from_be_bytes([rdata[0], rdata[1]]),
                    false => 0,
                },
                sha256: hex_lower(&sha256(rdata)),
            })
            .collect();
        keys.sort_by(|a, b| (a.key_tag, &a.sha256).cmp(&(b.key_tag, &b.sha256)));
        ZoneKeyStatement {
            apex: apex.to_string(),
            keys,
            action: action.to_string(),
        }
    }

    /// Renders the canonical Statement bytes.
    ///
    /// Field order is fixed and there is no whitespace: the signature and
    /// the leaf hash are over these exact bytes, so "equivalent JSON" is not
    /// equivalent at all.
    ///
    /// Publisher-side: a client parses a Statement out of a proof and never
    /// writes one, so this is behind the harness gate (see [`crate::sim`]).
    #[cfg(any(test, feature = "sim"))]
    pub fn to_json(&self) -> Vec<u8> {
        let mut out = String::new();
        out.push_str("{\"_type\":");
        json_string(&mut out, STATEMENT_TYPE);
        out.push_str(",\"subject\":[");
        for (index, key) in self.keys.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            out.push_str("{\"name\":");
            json_string(&mut out, &self.apex);
            out.push_str(",\"digest\":{\"sha256\":");
            json_string(&mut out, &key.sha256);
            out.push_str("}}");
        }
        out.push_str("],\"predicateType\":");
        json_string(&mut out, PREDICATE_TYPE);
        out.push_str(",\"predicate\":{\"apex\":");
        json_string(&mut out, &self.apex);
        out.push_str(",\"keys\":[");
        for (index, key) in self.keys.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            out.push_str("{\"keyTag\":");
            out.push_str(&key.key_tag.to_string());
            out.push_str(",\"algorithm\":");
            out.push_str(&key.algorithm.to_string());
            out.push_str(",\"flags\":");
            out.push_str(&key.flags.to_string());
            out.push_str(",\"sha256\":");
            json_string(&mut out, &key.sha256);
            out.push('}');
        }
        out.push_str("],\"action\":");
        json_string(&mut out, &self.action);
        out.push_str("}}");
        out.into_bytes()
    }

    /// Parses a Statement, accepting unknown members so the predicate can
    /// grow, and refusing anything whose known members are the wrong shape.
    ///
    /// The subject list and the predicate's key list must agree — same
    /// length, same digests, same order — or the statement is describing two
    /// different sets at once and cannot be believed about either.
    pub fn parse(bytes: &[u8]) -> Result<ZoneKeyStatement, ProofError> {
        let bad = |why: String| ProofError::Malformed(format!("statement: {why}"));
        let value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|e| bad(e.to_string()))?;
        let text = |v: &serde_json::Value, what: &str| -> Result<String, ProofError> {
            v.as_str()
                .map(str::to_string)
                .ok_or_else(|| bad(format!("{what} is not a string")))
        };
        let number = |v: &serde_json::Value, what: &str| -> Result<u64, ProofError> {
            v.as_u64()
                .ok_or_else(|| bad(format!("{what} is not a whole number")))
        };

        let statement_type = text(&value["_type"], "_type")?;
        if statement_type != STATEMENT_TYPE {
            return Err(ProofError::Binding(format!(
                "statement type {statement_type} is not {STATEMENT_TYPE}"
            )));
        }
        let predicate_type = text(&value["predicateType"], "predicateType")?;
        if predicate_type != PREDICATE_TYPE {
            return Err(ProofError::Binding(format!(
                "predicate type {predicate_type} is not {PREDICATE_TYPE}"
            )));
        }
        let subjects = value["subject"]
            .as_array()
            .ok_or_else(|| bad("subject is not an array".into()))?;
        if subjects.is_empty() {
            return Err(bad("a zone-key entry claims at least one key".into()));
        }
        let predicate = &value["predicate"];
        let apex = text(&predicate["apex"], "apex")?;
        let listed = predicate["keys"]
            .as_array()
            .ok_or_else(|| bad("predicate keys is not an array".into()))?;
        if listed.len() != subjects.len() {
            return Err(bad(format!(
                "{} subject(s) but {} predicate key(s)",
                subjects.len(),
                listed.len()
            )));
        }
        let mut keys = Vec::with_capacity(listed.len());
        for (subject, entry) in subjects.iter().zip(listed) {
            let subject_name = text(&subject["name"], "subject name")?;
            if subject_name != apex {
                return Err(bad(format!(
                    "a subject names {subject_name}, the predicate names {apex}"
                )));
            }
            let subject_sha256 = text(&subject["digest"]["sha256"], "subject sha256 digest")?;
            let sha256 = text(&entry["sha256"], "key sha256")?;
            if !subject_sha256.eq_ignore_ascii_case(&sha256) {
                return Err(bad("a subject digest and its predicate key disagree".into()));
            }
            keys.push(StatementKey {
                key_tag: u16::try_from(number(&entry["keyTag"], "keyTag")?)
                    .map_err(|_| bad("keyTag is out of range".into()))?,
                algorithm: u8::try_from(number(&entry["algorithm"], "algorithm")?)
                    .map_err(|_| bad("algorithm is out of range".into()))?,
                flags: u16::try_from(number(&entry["flags"], "flags")?)
                    .map_err(|_| bad("flags is out of range".into()))?,
                sha256,
            });
        }
        Ok(ZoneKeyStatement {
            apex,
            keys,
            action: text(&predicate["action"], "action")?,
        })
    }

    /// Checks that the Statement describes the proven set and the key
    /// observed (§4.2).
    ///
    /// Three facts, refused independently: the apex is the one whose RRSIG
    /// signed this answer (stops a set logged for one zone being replayed
    /// into another); the claimed set is exactly the chain-proven set, digest
    /// for digest and metadata for metadata (stops a statement describing
    /// keys its own chain never proved); and the key that signed this answer
    /// is a member (the point of the whole exercise).
    fn check_binds(
        &self,
        apex: &hickory_resolver::proto::rr::Name,
        key: &ZoneKey<'_>,
        proven: &[Vec<u8>],
    ) -> Result<(), ProofError> {
        // The Statement and the certificate have to name the same control
        // plane. The certificate's SAN is what the monitor indexes on and
        // what the declaration hangs under; a Statement naming something
        // else would describe a key set for a zone nobody checked.
        if !same_name(&self.apex, &apex.to_string()) {
            return Err(ProofError::Binding(format!(
                "the entry's statement names apex {}, its certificate names {apex}",
                self.apex
            )));
        }

        // The claimed set is the proven set, exactly. Compared as canonical
        // statements: `for_keys` derives every field from the rdatas alone,
        // so equality here pins digest, tag, algorithm, flags and order all
        // at once.
        let derived = ZoneKeyStatement::for_keys(&self.apex, proven, &self.action);
        if derived.keys != self.keys {
            return Err(ProofError::Binding(
                "the statement's key set is not the set its chain proves".into(),
            ));
        }

        // Membership: the key that signed this answer is in the proven set.
        let digest = hex_lower(&sha256(key.dnskey_rdata));
        if !self
            .keys
            .iter()
            .any(|k| k.sha256.eq_ignore_ascii_case(&digest))
        {
            return Err(ProofError::Binding(format!(
                "the answer was signed by key tag {}, which is not in the authorized set",
                key.key_tag
            )));
        }

        // Only a key set being *put into service* is authorization. A
        // `retire` entry is a breadcrumb for monitors and is allowed to be
        // chainless on the publish side (a retired zone may have no DS
        // left), so a client that accepted one would accept an entry
        // carrying no proof of delegation at all — the exact evasion the
        // chain requirement exists to close.
        if !matches!(self.action.as_str(), "create" | "rollover") {
            return Err(ProofError::Binding(format!(
                "the entry's action is {}, and only create or rollover authorizes a key",
                self.action
            )));
        }
        Ok(())
    }
}

/// A parsed checkpoint: the log's signed statement of a tree.
///
/// The signed-note format (Go's `sumdb/note`): text lines, a blank line,
/// then signature lines. The signature covers the text and its final
/// newline, and nothing after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    /// The origin line — the log's name for itself.
    pub origin: String,
    /// The number of entries the tree contains.
    pub tree_size: u64,
    /// The Merkle root over those entries.
    pub root_hash: [u8; 32],
    /// The exact bytes the signatures cover.
    signed: Vec<u8>,
    /// One entry per signature line: the key name it claims, its four-byte
    /// key hint, and the signature itself.
    ///
    /// **This is a list, and it must stay one**: a real Sigstore checkpoint
    /// carries the log's own signature *plus* a line per witness that
    /// cosigned the tree — the checked-in fixtures have four. This design
    /// does not interpret cosignatures in any way, but it has to tolerate
    /// them: a single-signature parser would reject every checkpoint the
    /// production log actually serves. The *name* is kept because it says
    /// which line is the log speaking about its own tree, and the hint
    /// because it is the one field that binds a key to an origin (see
    /// [`Checkpoint::verify_signature`]).
    signatures: Vec<Signature>,
}

/// One signature line of a signed note.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Signature {
    /// The key name the line claims to be.
    name: String,
    /// The four-byte key hint the signature blob is prefixed with.
    hint: [u8; 4],
    /// The signature over the note's signed bytes.
    signature: Vec<u8>,
}

impl Checkpoint {
    /// Parses a signed note.
    pub fn parse(bytes: &[u8]) -> Result<Checkpoint, ProofError> {
        let bad = |why: &str| ProofError::Malformed(format!("checkpoint: {why}"));
        let text = std::str::from_utf8(bytes).map_err(|_| bad("not UTF-8"))?;
        // The **last** blank line, as Go's `sumdb/note` does
        // (`bytes.LastIndex`). Splitting at the first is a real divergence
        // rather than a stylistic one: the checkpoint is not covered by the
        // leaf hash, so it is attacker-malleable in the zone's TXT, and
        // appending `"\n— attacker <b64>\n"` to a *genuine* checkpoint makes
        // the first blank line fall before the log's own signature line.
        // Split there and `signed` is exactly the real note, the real
        // signature verifies over it, and the appended block is accepted as
        // part of the signature section. Go splits after it and refuses. Two
        // readers disagreeing about which bytes a log signed is the thing
        // this whole design exists to prevent.
        let split = text
            .rfind("\n\n")
            .ok_or_else(|| bad("no blank line between the note and its signatures"))?;
        let signed = &text[..split + 1];
        let mut lines = signed.lines();
        let origin = lines.next().ok_or_else(|| bad("no origin line"))?;
        let tree_size: u64 = lines
            .next()
            .ok_or_else(|| bad("no tree size line"))?
            .parse()
            .map_err(|_| bad("the tree size is not a number"))?;
        let root = base64_decode(lines.next().ok_or_else(|| bad("no root hash line"))?)
            .map_err(|_| bad("the root hash is not base64"))?;
        let root_hash: [u8; 32] = root
            .as_slice()
            .try_into()
            .map_err(|_| bad("the root hash is not 32 bytes"))?;

        let mut signatures = Vec::new();
        for line in text[split + 2..].lines().filter(|l| !l.is_empty()) {
            // U+2014 EM DASH, then the key name, then base64(keyhint || sig).
            let rest = line
                .strip_prefix("\u{2014} ")
                .ok_or_else(|| bad("a signature line does not start with an em dash"))?;
            let (name, encoded) = rest
                .split_once(' ')
                .ok_or_else(|| bad("a signature line has no signature"))?;
            let blob = base64_decode(encoded).map_err(|_| bad("a signature is not base64"))?;
            if blob.len() <= 4 {
                return Err(bad("a signature is shorter than its key hint"));
            }
            let mut hint = [0u8; 4];
            hint.copy_from_slice(&blob[..4]);
            signatures.push(Signature {
                name: name.to_string(),
                hint,
                signature: blob[4..].to_vec(),
            });
        }
        if signatures.is_empty() {
            return Err(bad("no signature lines"));
        }
        Ok(Checkpoint {
            origin: origin.to_string(),
            tree_size,
            root_hash,
            signed: signed.as_bytes().to_vec(),
            signatures,
        })
    }

    /// Verifies that some pinned key signed this checkpoint **as the log this
    /// note says it is**.
    ///
    /// The public entry point a monitor uses: a client reaches the same check
    /// through [`verify`], but a monitor holds a checkpoint on its own and
    /// must be able to ask the question directly.
    pub fn verify_under(&self, logs: &LogKeys) -> Result<(), ProofError> {
        match logs
            .keys()
            .iter()
            .any(|key| self.verify_signature(key).is_ok())
        {
            true => Ok(()),
            false => Err(ProofError::Checkpoint(format!(
                "no signature on the checkpoint from {} verifies under a pinned \
                 log key naming {}",
                self.origin, self.origin
            ))),
        }
    }

    /// Verifies that the pinned log key signed this note **as its own origin**.
    ///
    /// Only the line whose name equals the note's `origin` counts. Witness
    /// lines sit beside it and are tolerated — a real Sigstore checkpoint
    /// carries three — but they are never the line that decides, and that
    /// distinction is the whole check.
    ///
    /// A signature that verifies is not by itself an answer to *which log this
    /// is*. In a C2SP cosigning arrangement a key signs other logs' notes as a
    /// witness, so "some pinned key signed these bytes" would let an unpinned
    /// log Y's checkpoint — cosigned by pinned key X — travel with
    /// `log_id = id(X)` and an inclusion path into Y's tree, and the only check
    /// that says which log an entry is in would pass while looking intact.
    ///
    /// **The binding that closes that is the pin's own origin, and nothing
    /// read out of the note.** The signature line's name and four-byte hint sit
    /// *after* the blank line —
    /// the region no signature covers — while the hint's derivation
    /// (`SHA-256(origin ‖ 0x0A ‖ 0x01 ‖ raw32)`, the C2SP note key id) is
    /// public arithmetic over public inputs. An attacker holding a genuine
    /// foreign note simply rewrites both. Go's `sumdb/note` does not have this
    /// problem because a name there means something the *caller* supplied: its
    /// `Verifiers` table maps `(name, hash) → key`. [`LogKey::origin`] is that
    /// table entry, carried from the trusted root that named the key, and it
    /// is compared against the origin line, which *is* inside the signed
    /// bytes.
    ///
    /// The line name and the hint stay as selectors, which is all they can
    /// honestly be: they pick which line to try. A key with no pinned origin —
    /// one from a `--rekor-key` file, whose grammar has nowhere to put a name
    /// — gets the old, weaker treatment, and that is the honest answer for a
    /// pin that arrived without one.
    fn verify_signature(&self, key: &LogKey) -> Result<(), ProofError> {
        // The pin's own name for this log, checked first and against the
        // **signed** origin line. Everything else this function reads about
        // *which* log signed is in the unsigned tail: a signature line's name
        // and its four-byte hint both sit after the blank line, where no
        // signature covers them, and the hint's derivation is public. So on
        // their own they are selectors an attacker rewrites at will, and
        // "some pinned key signed these bytes" is all they can establish —
        // which is exactly what a cosigned foreign log's checkpoint would
        // satisfy. A pin that carries an origin turns that into a statement:
        // this key vouches for this tree *as this log*.
        if key
            .origin
            .as_deref()
            .is_some_and(|pinned| pinned != self.origin)
        {
            return Err(ProofError::Checkpoint(format!(
                "the checkpoint says it is from {}, but the pinned key that \
                 signs it is the key for another log",
                self.origin
            )));
        }
        let signed = self.signatures.iter().any(|line| {
            line.name == self.origin
                && key.note_hint(&self.origin).is_none_or(|id| id == line.hint)
                && key.verify(&self.signed, &line.signature).is_ok()
        });
        match signed {
            true => Ok(()),
            false => Err(ProofError::Checkpoint(format!(
                "the checkpoint from {} carries no signature by {} itself that \
                 verifies under the pinned log key",
                self.origin, self.origin
            ))),
        }
    }
}

/// The signature algorithm a pinned log key uses.
/// One pinned log verification key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogKey {
    /// SHA-256 of the DER SubjectPublicKeyInfo — the `log_id` a proof names.
    pub id: [u8; 32],
    /// The scheme and raw key material (`crate::pubkey`).
    key: RawKey,
    /// The checkpoint origin this key is pinned *for*, when the pin came from
    /// an artifact that named one.
    ///
    /// Go's `sumdb/note` takes its origin↔key binding from the caller's
    /// `(name, hash) → key` verifier table: a name means something because
    /// the *verifier* was constructed with it. This is that table entry. A
    /// Sigstore trusted root names each shard's `baseUrl` beside its key, and
    /// the checkpoint origin of every shard it names is that URL's host — so
    /// the binding is available from the same signed artifact the key came
    /// from, and [`crate::tuf::tlog_keys`] carries it through.
    ///
    /// `None` for a key read from a `--rekor-key` file, whose grammar is bare
    /// SubjectPublicKeyInfo with nowhere to put a name. Such a key is
    /// unbound, and the check below says so rather than pretending otherwise.
    origin: Option<String>,
}

impl LogKey {
    /// The four-byte C2SP note key id this key has for `origin`, where the
    /// derivation is unambiguous.
    ///
    /// `SHA-256(origin ‖ 0x0A ‖ 0x01 ‖ raw32)` truncated to four bytes, which
    /// is the id a signed note's key hint carries — for Ed25519, the arm C2SP
    /// numbers `0x01`. `None` for a P-256 key: Sigstore publishes SHA-256 over
    /// the whole SubjectPublicKeyInfo as that arm's `logId.keyId`, so there is
    /// no one derivation to check a hint against and nothing is claimed.
    fn note_hint(&self, origin: &str) -> Option<[u8; 4]> {
        match self.key.scheme {
            Scheme::EcdsaP256Sha256 => None,
            Scheme::Ed25519 => {
                let mut input = Vec::with_capacity(origin.len() + 34);
                input.extend_from_slice(origin.as_bytes());
                input.push(0x0a);
                input.push(0x01);
                input.extend_from_slice(&self.key.point);
                let digest = sha256(&input);
                Some([digest[0], digest[1], digest[2], digest[3]])
            }
        }
    }

    /// Verifies a checkpoint signature under the shared double-encoding rule
    /// ([`RawKey::verifies`]). The only real checkpoint fixtures — both from
    /// the Ed25519 shard — exercise none of the ECDSA half.
    fn verify(&self, message: &[u8], signature: &[u8]) -> Result<(), ProofError> {
        match self.key.verifies(message, signature) {
            true => Ok(()),
            false => Err(ProofError::Checkpoint("signature does not verify".into())),
        }
    }
}

/// The set of logs this client will accept a record from.
///
/// A file of keys *replaces* the embedded set rather than adding to it —
/// the same "an override is a different universe" semantics as
/// `--dnssec-anchor`. An empty set accepts nothing.
///
/// A key is selected by `log_id`, and **there are two 32-byte log ids in play
/// and only one of them is ours.** This format's is SHA-256 over the DER
/// SubjectPublicKeyInfo, computed here from the key bytes themselves. Rekor's
/// `TransparencyLogEntry.logId.keyId` — which sits a few lines away from the
/// checkpoint in the same JSON response, and is exactly as long, and looks
/// exactly as plausible — is the C2SP **note key id**,
/// `SHA-256(origin ‖ 0x0A ‖ 0x01 ‖ raw32)`. Copying that value into a proof
/// produces a record that matches no pin and fails with "unknown log", which
/// reads like a misconfigured pin set rather than the mix-up it is. Sigstore's
/// trusted root shows the same split — it agrees with us for the P-256 log
/// and disagrees for the Ed25519 one — which is why [`crate::tuf::tlogs`]
/// derives the id from `publicKey.rawBytes` and deliberately never reads the
/// `logId.keyId` beside it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogKeys {
    keys: Vec<LogKey>,
}

impl LogKeys {
    /// The bootstrap set: the logs the embedded `trusted_root.json` names
    /// (see [`crate::tuf::EMBEDDED_TRUSTED_ROOT`]).
    ///
    /// These are what a client runs on until it learns better. A client that
    /// accepts a chain from Sigstore's TUF repository runs on the tlogs its
    /// trusted root names instead, persisted in SQLite config as
    /// `rekor.pin_state` and **replacing** this set rather than
    /// unioning with it (§10, [`crate::tuf`]). So the resolution order is
    /// `--rekor-key` if given, else the last TUF-verified pin set, else this.
    /// Rotating a Sigstore log key — or a whole log — is therefore not a new
    /// build any more; only a TUF-*root*-level incident is.
    pub fn embedded() -> LogKeys {
        crate::tuf::tlog_keys(crate::tuf::EMBEDDED_TRUSTED_ROOT.as_bytes()).unwrap_or_default()
    }

    /// Reads a key file: PEM `PUBLIC KEY` blocks, or one base64
    /// SubjectPublicKeyInfo per line. `#` starts a comment.
    pub fn from_file(path: &Path) -> Result<LogKeys, ProofError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| ProofError::UnknownLog(format!("log key file {}: {e}", path.display())))?;
        let keys = LogKeys::parse(&text)?;
        if keys.is_empty() {
            // An empty pin set verifies nothing, forever, quietly.
            return Err(ProofError::UnknownLog(format!(
                "log key file {}: no public keys in the file",
                path.display()
            )));
        }
        Ok(keys)
    }

    /// Parses key material from text.
    pub fn parse(text: &str) -> Result<LogKeys, ProofError> {
        let mut keys = Vec::new();
        let mut block: Option<String> = None;
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            match line {
                "" => {}
                "-----BEGIN PUBLIC KEY-----" => block = Some(String::new()),
                "-----END PUBLIC KEY-----" => {
                    let body = block.take().ok_or_else(|| {
                        ProofError::UnknownLog("a PEM block ends before it begins".into())
                    })?;
                    keys.push(LogKey::from_spki(&base64_decode(&body).map_err(|_| {
                        ProofError::UnknownLog("a PEM block is not base64".into())
                    })?)?);
                }
                _ => match &mut block {
                    Some(body) => body.push_str(line),
                    None => keys.push(LogKey::from_spki(&base64_decode(line).map_err(|_| {
                        ProofError::UnknownLog("a key line is not base64".into())
                    })?)?),
                },
            }
        }
        if block.is_some() {
            return Err(ProofError::UnknownLog("a PEM block is never closed".into()));
        }
        Ok(LogKeys { keys })
    }

    /// A pin set from keys already parsed — how [`crate::tuf::tlog_keys`]
    /// builds one, so the origin each key is pinned for survives.
    pub(crate) fn from_keys(keys: Vec<LogKey>) -> LogKeys {
        LogKeys { keys }
    }

    /// The key a proof's `log_id` names, if this client pins it.
    pub fn find(&self, log_id: &[u8; 32]) -> Option<&LogKey> {
        self.keys.iter().find(|key| &key.id == log_id)
    }

    /// Whether any log is pinned at all.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// The pinned keys, for `synch doctor`.
    pub fn keys(&self) -> &[LogKey] {
        &self.keys
    }
}

impl LogKey {
    /// The same key, pinned for the checkpoint origin `origin`.
    ///
    /// Used by [`crate::tuf::tlog_keys`], which reads the origin out of the
    /// same trusted root that named the key.
    pub(crate) fn for_origin(self, origin: String) -> LogKey {
        LogKey {
            origin: Some(origin),
            ..self
        }
    }

    /// Parses a DER SubjectPublicKeyInfo holding a P-256 or Ed25519 key
    /// (`RawKey::from_spki`). The `id` is SHA-256 over the DER bytes exactly
    /// as given.
    pub(crate) fn from_spki(der: &[u8]) -> Result<LogKey, ProofError> {
        match RawKey::from_spki(der) {
            Some(key) => Ok(LogKey {
                id: sha256(der),
                key,
                origin: None,
            }),
            None => Err(ProofError::UnknownLog(
                "a log key is neither an ECDSA P-256 nor an Ed25519 SubjectPublicKeyInfo".into(),
            )),
        }
    }
}

/// The DSSE Pre-Authentication Encoding (DSSE §2): the bytes actually
/// signed, so a payload cannot be reinterpreted under another type.
pub fn pae(payload_type: &str, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + payload_type.len() + 32);
    out.extend_from_slice(b"DSSEv1 ");
    out.extend_from_slice(payload_type.len().to_string().as_bytes());
    out.push(b' ');
    out.extend_from_slice(payload_type.as_bytes());
    out.push(b' ');
    out.extend_from_slice(payload.len().to_string().as_bytes());
    out.push(b' ');
    out.extend_from_slice(payload);
    out
}

/// Verifies an ECDSA P-256/SHA-256 signature in ASN.1/DER form against a
/// DNSSEC algorithm 13 public key (64 bytes of coordinates).
///
/// DER, not the raw `r || s` of DNSSEC: a Rekor entry's `signature.content`
/// is what the log indexed, and Rekor indexes DER. aws-lc-rs hashes the
/// message (the DSSE PAE) internally.
fn verify_ecdsa_p256_asn1(
    public: &[u8],
    message: &[u8],
    signature: &[u8],
) -> Result<(), ProofError> {
    let mut uncompressed = Vec::with_capacity(65);
    uncompressed.push(0x04);
    uncompressed.extend_from_slice(public);
    signature::UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_ASN1, &uncompressed)
        .verify(message, signature)
        .map_err(|_| ProofError::Attribution("signature does not verify".into()))
}

/// Wraps a raw 64-byte uncompressed P-256 point in a DER SubjectPublicKeyInfo.
///
/// The prefix is the fixed algorithm identifier for `id-ecPublicKey` over
/// `prime256v1` plus the bit-string header and the `0x04` uncompressed-point
/// tag — the same 27 bytes `LogKey::from_spki` strips back off.
pub fn p256_spki(point: &[u8]) -> Vec<u8> {
    let mut der = Vec::with_capacity(P256_SPKI_PREFIX.len() + point.len());
    der.extend_from_slice(P256_SPKI_PREFIX);
    der.extend_from_slice(point);
    der
}

/// Walks an RFC 6962 §2.1.1 audit path from a leaf to a root.
///
/// The leaf's index and the tree size decide which side each sibling sits
/// on, which is why both travel with the proof: a path alone proves nothing
/// about *where* in the tree an entry is.
pub fn verify_inclusion(
    index: u64,
    tree_size: u64,
    leaf_hash: [u8; 32],
    path: &[[u8; 32]],
    root: [u8; 32],
) -> Result<(), ProofError> {
    if index >= tree_size {
        return Err(ProofError::Inclusion(format!(
            "entry {index} is outside a tree of {tree_size}"
        )));
    }
    let mut node = index;
    let mut last = tree_size - 1;
    let mut hash = leaf_hash;
    for sibling in path {
        if last == 0 {
            return Err(ProofError::Inclusion(
                "the audit path is longer than the tree is deep".into(),
            ));
        }
        if node % 2 == 1 || node == last {
            hash = node_hash(sibling, &hash);
            while node != 0 && node.is_multiple_of(2) {
                node /= 2;
                last /= 2;
            }
        } else {
            hash = node_hash(&hash, sibling);
        }
        node /= 2;
        last /= 2;
    }
    if last != 0 {
        return Err(ProofError::Inclusion(
            "the audit path is shorter than the tree is deep".into(),
        ));
    }
    if hash != root {
        return Err(ProofError::Inclusion(
            "the audit path does not reach the checkpoint's root".into(),
        ));
    }
    Ok(())
}

/// An RFC 6962 interior node hash.
pub fn node_hash(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut input = Vec::with_capacity(65);
    input.push(0x01);
    input.extend_from_slice(left);
    input.extend_from_slice(right);
    sha256(&input)
}

/// An RFC 6962 leaf hash over already-serialized entry bytes.
pub fn leaf_hash(entry: &[u8]) -> [u8; 32] {
    let mut input = Vec::with_capacity(entry.len() + 1);
    input.push(0x00);
    input.extend_from_slice(entry);
    sha256(&input)
}

/// SHA-256.
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    let digest = digest::digest(&digest::SHA256, bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(digest.as_ref());
    out
}

/// Two DNS names, compared as DNS compares them: parsed and normalized, so a
/// spelling that is not a name (`"x.."`) never equals one that is (`"x."`).
///
/// A name that does not parse equals nothing, including itself — which is the
/// right answer for a Statement field that is supposed to be a zone.
fn same_name(a: &str, b: &str) -> bool {
    match (chain::parse_name(a), chain::parse_name(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Lowercase hex, the form the Statement's digests are written in.
fn hex_lower(bytes: &[u8]) -> String {
    hex::encode(bytes)
}

/// Appends a JSON string literal, escaping what JSON requires escaped.
///
/// Part of the canonical Statement *writer*, which is publisher-side.
#[cfg(any(test, feature = "sim"))]
fn json_string(out: &mut String, value: &str) {
    synch_core::json::json_string_into(out, value);
}

/// Standard base64 with padding.
pub fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Strips whitespace and padding, then decodes with `engine`: the one body
/// behind every padding-optional decoder here and in [`crate::tuf`], so a
/// hardening change to what these trust-boundary parsers accept cannot land in
/// one of them and not the others.
fn decode_padless(
    engine: &impl base64::Engine,
    text: &str,
) -> Result<Vec<u8>, base64::DecodeError> {
    let trimmed: String = text
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '=')
        .collect();
    engine.decode(&trimmed)
}

/// Standard base64, padding optional.
pub(crate) fn base64_decode(text: &str) -> Result<Vec<u8>, ()> {
    decode_padless(&base64::engine::general_purpose::STANDARD_NO_PAD, text).map_err(|_| ())
}

/// base64url without padding — how a proof travels in a TXT record.
#[cfg(any(test, feature = "sim"))]
pub(crate) fn base64url_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Decodes a base64url TXT payload, padding optional.
fn base64url_decode(text: &str) -> Result<Vec<u8>, ProofError> {
    decode_padless(&base64::engine::general_purpose::URL_SAFE_NO_PAD, text)
        .map_err(|e| ProofError::Malformed(format!("not base64url: {e}")))
}

/// A bounds-checked reader over the wire format.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes, at: 0 }
    }

    fn take(&mut self, len: usize, what: &str) -> Result<&'a [u8], ProofError> {
        let end = self.at.checked_add(len).ok_or_else(|| {
            ProofError::Malformed(format!("{what}: length {len} overflows the record"))
        })?;
        if end > self.bytes.len() {
            return Err(ProofError::Malformed(format!(
                "{what}: wanted {len} bytes, {} remain",
                self.bytes.len() - self.at
            )));
        }
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    fn u8(&mut self, what: &str) -> Result<u8, ProofError> {
        Ok(self.take(1, what)?[0])
    }

    fn u16(&mut self, what: &str) -> Result<u16, ProofError> {
        let bytes = self.take(2, what)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn u64(&mut self, what: &str) -> Result<u64, ProofError> {
        let bytes = self.take(8, what)?;
        let mut array = [0u8; 8];
        array.copy_from_slice(bytes);
        Ok(u64::from_be_bytes(array))
    }

    fn array32(&mut self, what: &str) -> Result<[u8; 32], ProofError> {
        let bytes = self.take(32, what)?;
        let mut array = [0u8; 32];
        array.copy_from_slice(bytes);
        Ok(array)
    }

    fn blob16(&mut self, what: &str) -> Result<&'a [u8], ProofError> {
        let len = self.u16(what)?;
        self.take(usize::from(len), what)
    }

    fn finish(&self) -> Result<(), ProofError> {
        match self.bytes.len() - self.at {
            0 => Ok(()),
            extra => Err(ProofError::Malformed(format!(
                "{extra} bytes after the end of the record"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proof() -> RekorProof {
        RekorProof {
            log_id: [7u8; 32],
            log_index: 1_234_567,
            statement: b"{\"_type\":\"x\"}".to_vec(),
            canonicalized_body: b"{\"kind\":\"hashedrekord\"}".to_vec(),
            checkpoint: "log.example\n4\nAAAA\n\n\u{2014} log.example AAAAAAAA\n"
                .as_bytes()
                .to_vec(),
            inclusion_path: vec![[1u8; 32], [2u8; 32]],
        }
    }

    fn big_proof() -> RekorProof {
        RekorProof {
            statement: vec![b'x'; 3000],
            canonicalized_body: vec![b'y'; 3000],
            ..proof()
        }
    }

    /// A proof is served in pieces, and the pieces are self-describing.
    #[test]
    fn a_proof_is_chunked_and_reassembles_in_any_order() {
        // Big enough to need several records: the whole reason the format
        // has a header at all.
        let big = big_proof();
        let mut records = big.to_txt().expect("encodes");
        assert!(records.len() > 1, "a real proof spans records");
        for record in &records {
            assert!(record.starts_with(PROOF_TXT_PREFIX));
            assert!(
                record.len() < 4096,
                "a record has to fit the tightest provider limit: {}",
                record.len()
            );
        }

        // Order is carried in the records, not in the answer: DNS gives no
        // ordering guarantee across an RRset.
        records.reverse();
        let proofs = proofs_from_txt(&records);
        let [Ok(got)] = proofs.as_slice() else {
            panic!("one proof, one candidate: {proofs:?}");
        };
        assert_eq!(got, &big);
    }

    /// Chunks of different proofs cannot be spliced into one that decodes:
    /// the group is the digest of the whole encoded record, so a mixed set
    /// either fails to reassemble or reassembles to something whose digest
    /// is not the group it claimed.
    #[test]
    fn chunks_from_two_proofs_do_not_splice() {
        let a = RekorProof {
            statement: vec![b'a'; 3000],
            ..proof()
        };
        let b = RekorProof {
            statement: vec![b'b'; 3000],
            ..proof()
        };
        let (ra, rb) = (a.to_txt().unwrap(), b.to_txt().unwrap());
        assert!(ra.len() > 1 && rb.len() > 1);
        // b's first chunk relabelled with a's group and index.
        let group = ra[0].split_whitespace().nth(1).unwrap().to_string();
        let payload = rb[0].split_whitespace().nth(3).unwrap();
        let forged = format!("{PROOF_TXT_PREFIX} {group} 1/{} {payload}", ra.len());
        let spliced: Vec<String> = std::iter::once(forged)
            .chain(ra[1..].iter().cloned())
            .collect();
        assert!(
            proofs_from_txt(&spliced).iter().all(|r| r.is_err()),
            "a spliced set must not yield a proof"
        );
    }

    #[test]
    fn a_truncated_or_padded_record_is_malformed() {
        let bytes = proof().encode().expect("a small record encodes");
        for cut in [0, 1, 10, 42, bytes.len() - 1] {
            assert!(matches!(
                RekorProof::decode(&bytes[..cut]),
                Err(ProofError::Malformed(_))
            ));
        }
        let mut extra = bytes;
        extra.push(0);
        assert!(matches!(
            RekorProof::decode(&extra),
            Err(ProofError::Malformed(_))
        ));
    }

    #[test]
    fn the_dsse_pae_is_the_dsse_pae() {
        // DSSE §2's own example shape: "DSSEv1 SP len SP type SP len SP body".
        assert_eq!(
            pae("application/example", b"hello"),
            b"DSSEv1 19 application/example 5 hello".to_vec()
        );
    }

    #[test]
    fn statements_round_trip_through_their_canonical_form() {
        // Two keys, deliberately supplied out of canonical order: a KSK
        // (flags 257) and a ZSK (flags 256), the split-key shape a
        // provider-hosted zone has. `for_keys` sorts and derives every field
        // from the rdatas alone.
        let ksk = [&[0x01u8, 0x01, 0x03, 0x0d][..], &[0xab; 64][..]].concat();
        let zsk = [&[0x01u8, 0x00, 0x03, 0x0d][..], &[0xcd; 64][..]].concat();
        let statement =
            ZoneKeyStatement::for_keys("sync.example.", &[ksk.clone(), zsk.clone()], "rollover");
        assert_eq!(statement.keys.len(), 2);
        let sorted: Vec<u16> = statement.keys.iter().map(|k| k.key_tag).collect();
        let mut expected = vec![chain::key_tag(&ksk), chain::key_tag(&zsk)];
        expected.sort_unstable();
        assert_eq!(sorted, expected, "keys are in canonical tag order");

        let json = statement.to_json();
        assert_eq!(ZoneKeyStatement::parse(&json).unwrap(), statement);
        let text = String::from_utf8(json).unwrap();
        assert!(text.starts_with("{\"_type\":\"https://in-toto.io/Statement/v1\","));
        assert!(!text.contains(' '), "the canonical form has no whitespace");
        assert!(text.ends_with("\"action\":\"rollover\"}}"), "{text}");
        assert!(text.contains("\"predicateType\":\"https://synchronicity.sh/zone-key/v2\""));

        // One rendering: the same set supplied in the other order is the
        // same bytes.
        let swapped = ZoneKeyStatement::for_keys("sync.example.", &[zsk, ksk], "rollover");
        assert_eq!(swapped.to_json(), statement.to_json());

        // An rdata with no complete header renders `flags` and `algorithm` as
        // zero *together* — the rule the Gleam publisher follows over the
        // same bytes. Deriving each field on its own would put `flags: 258,
        // algorithm: 0` in one canonical Statement and all-zero in the other,
        // and the Statement is what the two sides have to agree on byte for
        // byte.
        for stub in [vec![], vec![0x01], vec![0x01, 0x02], vec![0x01, 0x02, 0x03]] {
            let derived =
                ZoneKeyStatement::for_keys("sync.example.", std::slice::from_ref(&stub), "create");
            let [only] = derived.keys.as_slice() else {
                panic!("one rdata, one key");
            };
            assert_eq!(
                (only.flags, only.algorithm),
                (0, 0),
                "{stub:?} has no header to read, so neither field is invented"
            );
        }

        // A statement of the wrong type is refused, as is anything
        // unparseable.
        assert!(matches!(
            ZoneKeyStatement::parse(
                br#"{"_type":"https://in-toto.io/Statement/v0.1","subject":[]}"#
            ),
            Err(ProofError::Binding(_))
        ));
        assert!(matches!(
            ZoneKeyStatement::parse(b"not json"),
            Err(ProofError::Malformed(_))
        ));
    }

    #[test]
    fn merkle_paths_verify_against_a_known_tree() {
        // Four leaves: root = H(H(a,b), H(c,d)); the audit path for leaf 2
        // is [d, H(a,b)] and no other ordering reaches the root.
        let leaves: Vec<[u8; 32]> = (0..4u8).map(|i| leaf_hash(&[i])).collect();
        let ab = node_hash(&leaves[0], &leaves[1]);
        let cd = node_hash(&leaves[2], &leaves[3]);
        let root = node_hash(&ab, &cd);
        verify_inclusion(2, 4, leaves[2], &[leaves[3], ab], root).unwrap();
        // Wrong sibling order, short path, wrong root, out-of-range index.
        assert!(verify_inclusion(2, 4, leaves[2], &[ab, leaves[3]], root).is_err());
        assert!(verify_inclusion(2, 4, leaves[2], &[leaves[3]], root).is_err());
        assert!(verify_inclusion(2, 4, leaves[2], &[leaves[3], ab], [0u8; 32]).is_err());
        assert!(verify_inclusion(4, 4, leaves[0], &[], root).is_err());
        // A one-leaf tree: the leaf is the root, with an empty path.
        verify_inclusion(0, 1, leaves[0], &[], leaves[0]).unwrap();
    }

    #[test]
    fn checkpoints_parse_or_are_refused() {
        let note = "rekor.example\n17\n8N7C7fXJnu41Y7f/eR2Rqjr3FzLQqZ5jGVLPZaAJcXA=\n\n\u{2014} rekor.example AAAAAAECAw==\n";
        let checkpoint = Checkpoint::parse(note.as_bytes()).unwrap();
        // The signed bytes stop at the blank line.
        assert!(checkpoint.signed.ends_with(b"cXA=\n"));
        assert!(!String::from_utf8(checkpoint.signed.clone())
            .unwrap()
            .contains('\u{2014}'));

        for broken in [
            "no blank line\n1\nAAAA\n",
            "origin\nnotanumber\nAAAA\n\n\u{2014} n AAAAAAECAw==\n",
            "origin\n1\nAAAA\n\n\u{2014} n AAAAAAECAw==\n",
            "origin\n1\n8N7C7fXJnu41Y7f/eR2Rqjr3FzLQqZ5jGVLPZaAJcXA=\n\n",
            "origin\n1\n8N7C7fXJnu41Y7f/eR2Rqjr3FzLQqZ5jGVLPZaAJcXA=\n\n- n AAAAAAECAw==\n",
        ] {
            assert!(
                matches!(
                    Checkpoint::parse(broken.as_bytes()),
                    Err(ProofError::Malformed(_))
                ),
                "{broken:?} must not parse"
            );
        }
    }

    /// The one external reality anchor for the checkpoint half: a genuine
    /// signed checkpoint from log2025-1.rekor.sigstore.dev, verified against
    /// the key this build embeds — nothing in this file authored it. It also
    /// pins the parse *shape*: a real note carries the log's signature plus
    /// three witness cosignatures, which a single-signature parser would
    /// reject.
    #[test]
    fn a_real_sigstore_checkpoint_verifies_under_the_embedded_pin_set() {
        let note = include_bytes!("../tests/fixtures/sigstore_checkpoint.txt");
        let checkpoint = Checkpoint::parse(note).expect("a real checkpoint must parse");
        assert_eq!(checkpoint.origin, "log2025-1.rekor.sigstore.dev");
        assert!(checkpoint.tree_size > 0);
        assert_eq!(
            checkpoint.signatures.len(),
            4,
            "the log's own signature plus three witness cosignatures"
        );

        let embedded = LogKeys::embedded();
        assert!(
            embedded
                .keys()
                .iter()
                .any(|key| checkpoint.verify_signature(key).is_ok()),
            "the real checkpoint must verify under an embedded Sigstore key"
        );

        // And the anchor has teeth: flip one byte of the signed body and no
        // embedded key vouches for it any more.
        let mut tampered = checkpoint.clone();
        tampered.signed[0] ^= 0x01;
        assert!(
            !embedded
                .keys()
                .iter()
                .any(|key| tampered.verify_signature(key).is_ok()),
            "a tampered checkpoint must not verify"
        );

        // A block appended after the genuine note is not read as the genuine
        // one plus noise. The checkpoint is attacker-malleable (not covered
        // by the leaf hash), so splitting at the *first* blank line keeps
        // `signed` exactly the real note bytes, the log's signature still
        // verifies, and the appended line rides along as a cosignature. Go's
        // `sumdb/note` splits at the last blank line and refuses; two readers
        // disagreeing about which bytes a log signed is what this design
        // exists to prevent.
        let mut forged = note.to_vec();
        forged.extend_from_slice("\n— attacker AAAAAAAA\n".as_bytes());
        let accepted = Checkpoint::parse(&forged).is_ok_and(|c| {
            embedded
                .keys()
                .iter()
                .any(|k| c.verify_signature(k).is_ok())
        });
        assert!(
            !accepted,
            "a checkpoint carrying a line the log never signed must not verify"
        );
    }

    /// Only the log's own signature line counts, not any line a pinned key
    /// happens to have signed — and the four-byte key hint is where C2SP
    /// makes the origin↔key binding checkable.
    ///
    /// A witness key signs other logs' notes, so an unpinned log's checkpoint
    /// cosigned by a pinned key could travel under the pinned key's log_id;
    /// requiring the signer's own name to *be* the origin binds the key to
    /// the log. For Ed25519 the note key id is SHA-256(origin ‖ 0x0A ‖ 0x01 ‖
    /// raw32), so the hint is a checkable statement of the same binding;
    /// Sigstore's P-256 logs publish SHA-256 over the SubjectPublicKeyInfo
    /// instead, so no derivation is right for that arm and none is claimed.
    #[test]
    fn only_the_line_naming_the_origin_can_vouch_for_a_checkpoint() {
        let note = include_bytes!("../tests/fixtures/sigstore_checkpoint.txt");
        let checkpoint = Checkpoint::parse(note).expect("a real checkpoint parses");
        let embedded = LogKeys::embedded();
        checkpoint
            .verify_under(&embedded)
            .expect("the log's own line verifies under a pinned key");
        let ed25519 = embedded
            .keys()
            .iter()
            .find(|key| key.key.scheme == Scheme::Ed25519)
            .expect("the embedded set has an Ed25519 shard")
            .clone();

        // Rename the note's origin, leaving every signature untouched. The
        // log's line no longer *names* this origin — checked before any
        // signature is — and the signatures do not cover these bytes either.
        let renamed = String::from_utf8_lossy(note).replacen(
            "log2025-1.rekor.sigstore.dev\n",
            "log2025-1.rekor.sigstore.dev.dev.2\n",
            1,
        );
        assert!(Checkpoint::parse(renamed.as_bytes())
            .expect("still a note")
            .verify_under(&embedded)
            .is_err());

        // Witness lines are genuine signatures over these exact bytes by keys
        // with names of their own; as the note's origin they must not vouch
        // for a log they merely cosigned. Remove the log's own line: nothing
        // verifies, whatever the remaining names say.
        let without_own: String = String::from_utf8_lossy(note)
            .lines()
            .filter(|line| !line.starts_with("\u{2014} log2025-1.rekor.sigstore.dev "))
            .map(|line| format!("{line}\n"))
            .collect();
        let witnessed = Checkpoint::parse(without_own.as_bytes()).expect("witness lines parse");
        assert_eq!(witnessed.signatures.len(), 3);
        assert!(
            witnessed.verify_under(&embedded).is_err(),
            "cosignatures are tolerated and never authoritative"
        );

        // The hint: the real one on the log's own line is the C2SP id
        // exactly, and it depends on the origin.
        let own = checkpoint
            .signatures
            .iter()
            .find(|line| line.name == checkpoint.origin)
            .expect("the log signs its own note");
        assert_eq!(ed25519.note_hint(&checkpoint.origin), Some(own.hint));
        assert_ne!(
            ed25519.note_hint("other.log.example"),
            Some(own.hint),
            "the id would say nothing about the origin if it did not depend on it"
        );
        // A line whose hint is not that id does not verify, even though the
        // signature bytes beside it are the log's own...
        let mut tampered = checkpoint.clone();
        for line in &mut tampered.signatures {
            line.hint[0] ^= 0x01;
        }
        assert!(tampered.verify_signature(&ed25519).is_err());
        // ...and nothing is claimed for P-256, where Sigstore's own id is a
        // different derivation entirely.
        let p256 = embedded
            .keys()
            .iter()
            .find(|key| key.key.scheme == Scheme::EcdsaP256Sha256)
            .expect("the embedded set has a P-256 shard");
        assert_eq!(p256.note_hint(&checkpoint.origin), None);
    }

    /// A pinned key does not vouch for a tree that is not its own log's,
    /// however the note's *unsigned* tail is spelled. The line name and the
    /// four-byte hint both sit after the blank line where no signature
    /// reaches; the binding has to come from the pin, where the trusted root
    /// put it. Built rather than captured: no real Sigstore checkpoint has a
    /// key signing a note whose origin is not its own log.
    #[test]
    fn a_pinned_key_vouches_only_for_the_log_it_is_pinned_for() {
        use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair};
        let rng = aws_lc_rs::rand::SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).expect("keygen");
        let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("key");
        let spki = [
            crate::pubkey::ED25519_SPKI_PREFIX,
            pair.public_key().as_ref(),
        ]
        .concat();
        let key = LogKey::from_spki(&spki).expect("an ed25519 pin");

        // A note whose signed origin names some *other* log, signed by this
        // key: a genuine signature over bytes that say they belong elsewhere.
        let note = "other-log.example\n7\nAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\n";
        // The attacker writes both unsigned fields to whatever passes: the
        // origin as the line name, and the hint the derivation says goes with
        // that pair.
        let hint = key.note_hint("other-log.example").expect("ed25519 has one");
        let blob = base64_encode(&[&hint[..], pair.sign(note.as_bytes()).as_ref()].concat());
        let signed = format!("{note}\n\u{2014} other-log.example {blob}\n");
        let checkpoint = Checkpoint::parse(signed.as_bytes()).expect("a note");

        // Unbound, the note is accepted: name matches origin, hint matches the
        // derivation, signature verifies. This is what a pin from a
        // `--rekor-key` file gets, and it is the whole of the old check.
        checkpoint
            .verify_signature(&key)
            .expect("an unbound pin has only the note's own word to go on");

        // Pinned for the log it actually belongs to, the same bytes are
        // refused — the attacker cannot rewrite the pin.
        let bound = key.clone().for_origin("our-log.example".to_string());
        assert!(matches!(
            checkpoint.verify_signature(&bound),
            Err(ProofError::Checkpoint(_))
        ));
        // And pinned for its own origin it verifies, so the refusal above is
        // the binding rather than something incidental about the note.
        let matching = key.for_origin("other-log.example".to_string());
        checkpoint
            .verify_signature(&matching)
            .expect("a key pinned for this origin vouches for it");
    }

    /// Reassembly is linear in the records offered, not quadratic. The
    /// records are the zone's and the threat model's attacker *is* the zone:
    /// sixteen validated TXT names hold tens of thousands of minimal
    /// `sync1p` records, and spreading claimed totals across 1..=255 made
    /// the old rescan-per-index cost about a second of CPU inside one
    /// `poll`. Asserted as a wall-clock ceiling rather than an operation
    /// count, because the count is what changed.
    #[test]
    fn reassembly_does_not_rescan_every_part_for_every_claimed_total() {
        let chunk = "A".repeat(8);
        let records: Vec<String> = (0..20_000)
            .map(|i| {
                // One group, every index spread over every plausible total,
                // so the old form paid sum-of-totals times the record count.
                let total = (i % 255) + 1;
                let index = (i % total) + 1;
                format!("sync1p abcdef01 {index}/{total} {chunk}")
            })
            .collect();
        let started = std::time::Instant::now();
        let out = proofs_from_txt(&records);
        let elapsed = started.elapsed();
        // Nothing reassembles — the payloads are not a proof — which is the
        // point: the cost is paid before any of that is known.
        assert!(out.iter().all(|r| r.is_err()));
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "reassembly of {} records took {elapsed:?}",
            records.len()
        );
    }

    /// The pin set the trusted root yields carries each shard's origin.
    #[test]
    fn pinned_log_keys_carry_the_origin_the_trusted_root_named_them_at() {
        let keys = crate::tuf::tlog_keys(crate::tuf::EMBEDDED_TRUSTED_ROOT.as_bytes())
            .expect("the embedded trusted root parses");
        let origins: Vec<&str> = keys
            .keys()
            .iter()
            .filter_map(|key| key.origin.as_deref())
            .collect();
        assert!(
            origins.contains(&"log2025-1.rekor.sigstore.dev"),
            "the shard the checked-in checkpoint comes from must be pinned for \
             its own origin: {origins:?}"
        );
        // And the real checkpoint still verifies against that bound pin, which
        // is what says the derivation matches what Sigstore actually signs.
        let note = include_bytes!("../tests/fixtures/sigstore_checkpoint.txt");
        Checkpoint::parse(note)
            .expect("a real checkpoint parses")
            .verify_under(&keys)
            .expect("a real checkpoint verifies under the origin-bound pin set");
    }

    /// A group is one reading of its records, and a zone that contradicts
    /// itself is refused rather than guessed at. Records claiming different
    /// totals are separate readings — a rollover legitimately serves a
    /// five-part set beside a nine-part one — so a raised total cannot hole
    /// the real set. Within one reading each index arrives once; every
    /// record arrives DNSSEC-validated, so duplicating an index takes the
    /// zone's signing key, who can delete the records instead (the honest
    /// trade: this makes a denial cheaper for the party who can mount one).
    #[test]
    fn a_group_is_one_reading_and_a_contradiction_is_refused() {
        let big = big_proof();
        let records = big.to_txt().expect("a multi-part proof");
        let total = records.len();
        assert!(total > 1, "the fixture must actually span several parts");
        let group = records[0]
            .split(' ')
            .nth(1)
            .expect("a group field")
            .to_string();
        let reassembles = |set: &[String]| -> Vec<RekorProof> {
            proofs_from_txt(set)
                .into_iter()
                .filter_map(Result::ok)
                .collect()
        };
        assert_eq!(reassembles(&records), vec![big.clone()]);

        // A record inventing a larger count is a *different* reading, and the
        // real one is still read.
        let mut inflated = records.clone();
        inflated.push(format!("{PROOF_TXT_PREFIX} {group} 9/9 AAAA"));
        assert_eq!(
            reassembles(&inflated),
            vec![big.clone()],
            "a raised total is one more reading, not a hole in the real one"
        );

        // A duplicate of a real index, inside the real reading, is a refusal
        // naming the index — not a silent pick between two chunks.
        let mut duplicated = records.clone();
        duplicated.push(format!("{PROOF_TXT_PREFIX} {group} 1/{total} AAAA"));
        let refused = proofs_from_txt(&duplicated);
        assert!(
            matches!(&refused[0], Err(ProofError::Malformed(why))
                if why.contains("arrived more than once")),
            "{refused:?}"
        );

        // A genuinely incomplete group is refused too, and says which part is
        // missing rather than which count it gave up on.
        let missing = &records[1..];
        let refusal = proofs_from_txt(missing);
        assert!(
            matches!(&refusal[0], Err(ProofError::Malformed(why)) if why.contains("did not arrive")),
            "{refusal:?}"
        );
    }

    #[test]
    fn pinned_keys_parse_from_pem_and_bare_base64() {
        // A well-formed P-256 SubjectPublicKeyInfo over an arbitrary point:
        // parsing is structural, and a point that is not on the curve fails
        // later, at verification, where aws-lc-rs checks it.
        let mut der = vec![
            0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06,
            0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00, 0x04,
        ];
        der.extend_from_slice(&[0x11; 64]);
        let base64 = base64_encode(&der);
        let keys = LogKeys::parse(&base64).unwrap();
        assert_eq!(keys.keys().len(), 1);
        assert!(keys.find(&sha256(&der)).is_some());

        // The refusal arms of the key-file grammar.
        assert!(LogKeys::parse("").unwrap().is_empty());
        assert!(LogKeys::parse("not base64!!").is_err());
        assert!(LogKeys::parse(&base64_encode(b"short")).is_err());
        assert!(LogKeys::parse("-----BEGIN PUBLIC KEY-----\n").is_err());
    }

    #[test]
    fn the_embedded_pin_set_is_the_sigstore_snapshot() {
        // The exact production keys, pinned by id — which is SHA-256 over
        // the DER SubjectPublicKeyInfo, *this format's* convention. (The
        // trusted root's own logId agrees for the P-256 log and not for the
        // Ed25519 one; a proof's log_id must match ours.) A changed id here
        // is a changed key, which is a rollover ceremony, not an edit.
        let keys = LogKeys::embedded();
        assert_eq!(keys.keys().len(), 2);
        let rekor_v1: [u8; 32] =
            hex::decode("c0d23d6ad406973f9559f3ba2d1ca01f84147d8ffc5b8445c224f98b9591801d")
                .unwrap()
                .try_into()
                .unwrap();
        let log2025_1: [u8; 32] =
            hex::decode("b54813cb63d8859870a5e78500cc6adcfdf59723edae93ee8d25faf2475a0690")
                .unwrap()
                .try_into()
                .unwrap();
        assert!(keys.find(&rekor_v1).is_some(), "rekor.sigstore.dev");
        assert!(
            keys.find(&log2025_1).is_some(),
            "log2025-1.rekor.sigstore.dev"
        );
    }
}
