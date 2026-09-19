//! Zone-key transparency end to end (docs/REKOR-ZONE-KEY.md §9): the positive case proves a correctly logged key set is accepted; every other case breaks exactly one thing.

mod common;

use common::{fixture, fixture_field, member_records, write};
use synch_net::{
    chain,
    dns::{self, DnssecResolver, RekorPolicy, ResolverOptions},
    error::NetError,
    rekor::{self, HashedRekordBody, LogKeys, ProofError, RekorProof, ZoneKey, DSSE_PAYLOAD_TYPE},
    sim::{hashedrekord_body, SimLog, SimZone},
    zonecert::{ChainLink, DnssecChain, OID_DNSSEC_CHAIN},
    MemberSet,
};

type TrustAnchors = hickory_resolver::proto::dnssec::TrustAnchors;

/// The verification must fail with an error matching the pattern.
macro_rules! refuses {
    ($result:expr, $pat:pat_param $(if $guard:expr)?) => {
        assert!(matches!($result, Err($pat) $(if $guard)?));
    };
}

fn anchors(zone: &SimZone) -> TrustAnchors {
    let file = write(&zone.anchor_record());
    TrustAnchors::from_file(file.path()).unwrap()
}

/// The chain a certificate carries, decoded only after [`chain::authorize`] accepted it.
fn carried_chain(body: &HashedRekordBody, anchors: &TrustAnchors) -> DnssecChain {
    chain::authorize(&body.certificate, anchors).expect("the carried chain must authorize");
    let ext = body.certificate.extension(OID_DNSSEC_CHAIN).unwrap();
    DnssecChain::decode(ext).expect("the chain decodes")
}

fn key_for<'a>(apex: &'a str, rdata: &'a [u8], tag: u16) -> ZoneKey<'a> {
    ZoneKey {
        domain: apex,
        signing_zone: apex,
        key_tag: tag,
        dnskey_rdata: rdata,
    }
}

fn zone_and_log() -> (SimZone, SimLog) {
    let zone = SimZone::new("cluster.example", member_records());
    (zone, SimLog::new("rekor.sim"))
}

fn logged_zone() -> (SimZone, SimLog, RekorProof) {
    let (zone, mut log) = zone_and_log();
    let proof = log.publish(&zone, "create");
    (zone, log, proof)
}

fn verify(proof: &RekorProof, zone: &SimZone, log: &SimLog) -> Result<(), ProofError> {
    verify_under(proof, zone, &LogKeys::parse(&log.key_pem()).unwrap())
}

fn verify_under(proof: &RekorProof, zone: &SimZone, keys: &LogKeys) -> Result<(), ProofError> {
    let apex = zone.apex();
    let rdata = zone.dnskey_rdata();
    let key = key_for(&apex, &rdata, zone.key_tag());
    rekor::verify(proof, &key, keys, &anchors(zone)).map(|_| ())
}

/// A resolver against `url` with `log_key` pinned under `policy`; TUF is off.
fn resolver(
    url: &str,
    anchor: &std::path::Path,
    log_key: &std::path::Path,
    policy: RekorPolicy,
) -> DnssecResolver {
    DnssecResolver::with_options(&ResolverOptions {
        doh_url: Some(url.to_string()),
        trust_anchor: Some(anchor.to_path_buf()),
        rekor: Some(policy),
        rekor_key: Some(log_key.to_path_buf()),
        rekor_state: None,
        rekor_config: None,
        tuf_url: None,
        no_tuf: true,
        tuf_root: None,
    })
    .unwrap()
}

/// Serves `zone` over loopback DoH and resolves it with `log`'s key under `policy`.
async fn resolve(zone: SimZone, log: &SimLog, policy: RekorPolicy) -> Result<MemberSet, NetError> {
    let anchor = write(&zone.anchor_record());
    let log_key = write(&log.key_pem());
    let (url, server) = zone.serve().await;
    let result = resolver(&url, anchor.path(), log_key.path(), policy)
        .member_set("cluster.example")
        .await;
    server.abort();
    result.map(|(set, _)| set)
}

#[test]
fn the_attribution_check_binds_to_the_certificates_signer() {
    let (zone, mut log) = zone_and_log();
    let stranger = SimZone::new("cluster.example", member_records());
    let statement = zone.zone_key_statement("create").to_json();
    let certificate = zone.zone_key_certificate();

    // In the tree with a matching PAE digest — but signed by a key that is not the certificate's.
    let body = hashedrekord_body(&statement, &stranger.sign_dsse(&statement), &certificate);
    refuses!(
        verify(&log.log_body(statement.clone(), body), &zone, &log),
        ProofError::Attribution(_)
    );

    // The decoupling this claim exists for: a key that is not a zone key at all —
    // the chain, not the signature, authorizes the key set (a provider-held key).
    let ephemeral = SimZone::new("cluster.example", Vec::new());
    let certificate =
        ephemeral.certificate(&[(OID_DNSSEC_CHAIN.to_vec(), zone.dnssec_chain().encode())]);
    let body = hashedrekord_body(&statement, &ephemeral.sign_dsse(&statement), &certificate);
    verify(&log.log_body(statement, body), &zone, &log)
        .expect("the real signer authorizes the chain-proven set");
}

#[test]
fn a_certificate_naming_another_apex_fails_binding() {
    // The attack this design exists to make loud: the right key, on the public record, under a name that is not this zone.
    let (zone, mut log) = zone_and_log();
    let chain_ext = (OID_DNSSEC_CHAIN.to_vec(), zone.dnssec_chain().encode());
    let statement = zone.zone_key_statement("create");
    let certificate = zone.certificate_for("somewhere.else", &[chain_ext]);
    let proof = log.log_certified(&zone, &statement, &certificate);
    refuses!(verify(&proof, &zone, &log), ProofError::Binding(_));
    // The same lie one layer down: the statement names another apex.
    let mut statement = zone.zone_key_statement("create");
    statement.apex = "other.example.".into();
    let proof = log.log_statement(&zone, &statement);
    refuses!(verify(&proof, &zone, &log), ProofError::Binding(_));
}

#[test]
fn a_statement_describing_keys_its_chain_never_proved_fails_binding() {
    // The claimed set must be exactly the set the chain proves, digest for digest.
    let (zone, mut log) = zone_and_log();
    let stranger = SimZone::new("cluster.example", member_records());
    let mut statement = zone.zone_key_statement("create");
    statement.keys[0].sha256 = hex::encode(rekor::sha256(&stranger.dnskey_rdata()));
    let proof = log.log_statement(&zone, &statement);
    refuses!(verify(&proof, &zone, &log), ProofError::Binding(_));
}

/// An unknown extension is carried into the leaf and never asked for — but must not become a *reason to accept* either.
#[test]
fn an_extension_the_client_does_not_know_is_carried_and_ignored() {
    let (zone, mut log) = zone_and_log();
    let statement = zone.zone_key_statement("create");
    // An arc this build has no name for, holding bytes that decode as nothing.
    let unknown = (
        vec![0x2b, 0x06, 0x01, 0x04, 0x01, 0x86, 0x8d, 0x1f, 0x01],
        b"opaque".to_vec(),
    );
    let chain_ext = (OID_DNSSEC_CHAIN.to_vec(), zone.dnssec_chain().encode());
    let proof = log.log_certified(
        &zone,
        &statement,
        &zone.certificate(&[chain_ext, unknown.clone()]),
    );
    verify(&proof, &zone, &log).unwrap();
    // Really in there — the acceptance above is not passing vacuously.
    let body = HashedRekordBody::parse(&proof.canonicalized_body).unwrap();
    assert!(body.certificate.extension(&unknown.0).is_some());
}

#[test]
fn a_broken_audit_path_fails_inclusion() {
    // A one-leaf tree has an empty audit path; put the entry among neighbours.
    let (zone, mut log) = zone_and_log();
    log.append(b"an earlier entry");
    let mut proof = log.publish(&zone, "create");
    log.append(b"a later entry");
    log.refresh(&mut proof);
    verify(&proof, &zone, &log).expect("the proof is sound before it is broken");
    assert!(!proof.inclusion_path.is_empty());
    // A stale path does not reach a newer root; refreshing against the grown tree fixes it (the control plane's weekly refresh).
    let mut stale = proof.clone();
    log.append(b"some other entry");
    log.append(b"and another");
    stale.checkpoint = log.checkpoint();
    refuses!(verify(&stale, &zone, &log), ProofError::Inclusion(_));
    let mut refreshed = proof;
    log.refresh(&mut refreshed);
    verify(&refreshed, &zone, &log).unwrap();
}

#[test]
fn a_checkpoint_from_another_log_fails() {
    let (zone, log, mut proof) = logged_zone();
    // A pin set that names no log accepts nothing, however sound the entry.
    refuses!(
        verify_under(&proof, &zone, &LogKeys::default()),
        ProofError::UnknownLog(_)
    );
    let other = SimLog::new("impostor.sim");
    proof.checkpoint = other.note(1, log.root());
    refuses!(verify(&proof, &zone, &log), ProofError::Checkpoint(_));
    proof.log_id = other.log_id();
    refuses!(verify(&proof, &zone, &log), ProofError::UnknownLog(_));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_zone_that_publishes_its_proof_resolves_under_require() {
    // Require is the default in every trust configuration (§4.1).
    assert_eq!(
        ResolverOptions::default().rekor_policy(),
        RekorPolicy::Require
    );
    let (mut zone, log, proof) = logged_zone();
    // A rollover window publishes two records; the client picks by key tag.
    let mut other_log = SimLog::new("rekor.sim");
    let stranger = SimZone::new("cluster.example", member_records());
    let other = other_log.publish(&stranger, "rollover");
    zone.rekor_txt = other.to_txt().expect("encodes");
    zone.rekor_txt.extend(proof.to_txt().expect("encodes"));
    let set = resolve(zone, &log, RekorPolicy::Require)
        .await
        .expect("a logged key resolves");
    assert_eq!(set.bindings.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_colliding_tag_unlogged_zsk_does_not_inherit_the_old_proof() {
    // Identifying the signer by 16-bit tag alone would accept this: a new ZSK with the old key's tag, old proof still served.
    let (mut zone, log, proof) = logged_zone();
    let (zsk, signer) = zone.colliding_key();
    assert_eq!(zsk.calculate_key_tag().unwrap(), zone.key_tag());
    zone.add_dnskey(zsk);
    zone.sign_txt_with(signer);
    zone.rekor_txt = proof.to_txt().expect("encodes");
    let error = resolve(zone, &log, RekorPolicy::Require).await.unwrap_err();
    assert!(matches!(error, NetError::RekorBinding { .. }), "{error}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_proof_record_covering_only_someone_elses_keys_is_refused() {
    // Every served record is a candidate, so one proving another set is refused, not filtered into looking absent.
    let (mut zone, mut log) = zone_and_log();
    let stranger = SimZone::new("cluster.example", member_records());
    zone.rekor_txt = log.publish(&stranger, "create").to_txt().expect("encodes");
    let error = resolve(zone, &log, RekorPolicy::Require).await.unwrap_err();
    assert!(matches!(error, NetError::RekorChain { .. }), "{error}");
}

/// The checked-in proof both halves assert against — a fixture neither side can quietly regenerate.
const REKOR_FIXTURES: &str = "../../control-plane/test/fixtures/rekor";

#[test]
fn the_shared_fixture_decodes_and_verifies() {
    let f = |name| fixture(REKOR_FIXTURES, name);
    let field = |name| fixture_field(REKOR_FIXTURES, name);
    let proof = RekorProof::decode(&f("proof.bin")).expect("a v4 proof");
    assert_eq!(proof.statement, f("statement.json"));
    assert_eq!(proof.canonicalized_body, f("canonicalized-body.bin"));
    assert_eq!(proof.checkpoint, f("checkpoint.txt"));
    assert_eq!(proof.log_id.to_vec(), f("log-id.bin"));
    assert_eq!(proof.inclusion_path.concat(), f("inclusion-path.bin"));
    assert_eq!(proof.log_index.to_string(), field("log_index"));
    // The part ceiling is a *coupling*: raising it on the publishing side alone silently truncates every proof past the old limit.
    assert_eq!(rekor::MAX_PROOF_PARTS.to_string(), field("max_proof_parts"));
    // The timing terms pin constants the control plane must match.
    assert_eq!(
        dns::DEFAULT_TRUST_GRACE.as_secs().to_string(),
        field("client_trust_grace")
    );
    assert_eq!(dns::MIN_TTL.as_secs().to_string(), field("client_min_ttl"));
    let window = dns::CONTROL_PLANE_REPUBLISH_WINDOW.as_secs().to_string();
    assert_eq!(window, field("control_plane_republish_window"));
    assert_eq!(proof.encode().unwrap(), f("proof.bin"));
    let body = HashedRekordBody::parse(&proof.canonicalized_body).unwrap();
    assert_eq!(body.certificate_der, f("certificate.der"));
    let apex = field("apex");
    assert_eq!(
        body.certificate.single_dns_name().unwrap().to_string(),
        apex
    );
    let anchor_file = write(&String::from_utf8(f("anchor.key")).unwrap());
    let anchors = TrustAnchors::from_file(anchor_file.path()).unwrap();
    assert_eq!(
        carried_chain(&body, &anchors).encode(),
        f("dnssec-chain.der")
    );
    let dnskey_rdata = f("dnskey.bin");
    assert_eq!(body.certificate.spki, rekor::p256_spki(&dnskey_rdata[4..]));
    let key = key_for(
        &apex,
        &dnskey_rdata,
        field("key_tag").parse().expect("key tag"),
    );
    let logs = LogKeys::parse(&String::from_utf8(f("log-key.pem")).unwrap()).unwrap();
    let verified = rekor::verify(&proof, &key, &logs, &anchors).expect("the fixture must verify");
    assert_eq!(verified.action, field("action"));
    assert_eq!(verified.log_index, proof.log_index);
}

/// The certificate encoders across two implementations of one DER format: Gleam-written
/// crossval bytes (`gleam run -m tools/gen_crossval`), asserted by both suites.
#[test]
fn the_gleam_certificate_encoders_agree_with_this_one() {
    use synch_net::x509::Certificate;

    let f = |name| fixture(REKOR_FIXTURES, name);
    // Restated here rather than shared: the Gleam side reads this list from its own generator.
    let pattern = |n: usize| -> Vec<u8> { (0..n).map(|i| (i * 7 % 256) as u8).collect() };
    let links: Vec<ChainLink> = [
        ("sync.test.", vec![0xaa, 0xbb, 0xcc]),
        (".", vec![0x01, 0x02]),
        // The two links that reach DER's long-form lengths.
        ("long.sync.test.", pattern(200)),
        ("longer.sync.test.", pattern(256)),
    ]
    .into_iter()
    .map(|(zone, rrs)| ChainLink {
        zone: zone.into(),
        rrs,
    })
    .collect();
    let chain = DnssecChain { links };
    assert_eq!(chain.encode(), f("crossval/chain.der"));
    assert_eq!(
        DnssecChain::decode(&f("crossval/chain.der")).unwrap(),
        chain
    );
    let certificate = Certificate::parse(&f("crossval/certificate.der")).expect("must parse");
    assert_eq!(
        certificate.single_dns_name().unwrap().to_string(),
        "sync.test."
    );
    assert_eq!(certificate.spki.len(), 91);
    assert_eq!(
        certificate.extension(OID_DNSSEC_CHAIN),
        Some(f("crossval/chain.der").as_slice())
    );
}

/// Both DS digest types against the Gleam side's bytes; the SHA-384 arm pins a real
/// cross-implementation bug that made type-4-DS zones unpublishable.
#[test]
fn the_ds_digests_match_the_control_planes() {
    use hickory_resolver::proto::rr::Name;
    let zone: Name = "Sync.Test.".parse().unwrap();
    let mut dnskey_rdata = vec![0x01, 0x01, 0x03, 0x0d];
    dnskey_rdata.extend_from_slice(&[0u8; 63]);
    dnskey_rdata.push(7);
    assert_eq!(
        chain::ds_digest_sha256_for_tests(&zone, &dnskey_rdata),
        fixture(REKOR_FIXTURES, "crossval/ds-digest-sha256.bin"),
    );
    assert_eq!(
        chain::ds_digest_sha384_for_tests(&zone, &dnskey_rdata),
        fixture(REKOR_FIXTURES, "crossval/ds-digest-sha384.bin"),
    );
}

/// A real published Rekor entry (`tests/fixtures/rekor_v3`, see PROVENANCE.txt), verified
/// in full offline: a genuine `hashedrekord` v0.0.2 entry from `log2025-1.rekor.sigstore.dev`.
mod real_rekor_v3 {
    use super::*;

    const V3: &str = "tests/fixtures/rekor_v3";
    const APEX: &str = "zone-key-transparency.demo.invalid";
    const LOG_INDEX: u64 = 68_295_246;
    const KEY_TAG: u16 = 32_784;

    fn real_anchors() -> (tempfile::TempDir, TrustAnchors) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("anchor.key");
        std::fs::write(&path, fixture(V3, "anchor.txt")).expect("write anchor");
        let anchors = TrustAnchors::from_file(&path).expect("the anchor parses");
        (dir, anchors)
    }

    /// The key the entry's own chain proves, read out of the chain — the same rule the monitor follows.
    fn chain_proven_key(body: &HashedRekordBody) -> Vec<u8> {
        let (_dir, anchors) = real_anchors();
        let authorized =
            chain::authorize(&body.certificate, &anchors).expect("the real chain must authorize");
        let [rdata] = authorized.proven_keys.as_slice() else {
            panic!("a one-key zone proves one key");
        };
        rdata.clone()
    }

    /// The entire client verifier over a real entry in a real log — the claim's half rests on no simulation.
    #[test]
    fn the_real_entry_verifies_end_to_end() {
        let proof = RekorProof::decode(&fixture(V3, "proof.bin")).expect("a current proof record");
        assert_eq!(proof.log_index, LOG_INDEX);
        let body = HashedRekordBody::parse(&proof.canonicalized_body).unwrap();
        let dnskey_rdata = chain_proven_key(&body);
        assert_eq!(chain::key_tag(&dnskey_rdata), KEY_TAG);
        let apex = format!("{APEX}.");
        let key = key_for(&apex, &dnskey_rdata, KEY_TAG);
        let (_dir, anchors) = real_anchors();
        let verified = rekor::verify(&proof, &key, &LogKeys::embedded(), &anchors)
            .expect("a real published entry must verify under the real log's key");
        assert_eq!(verified.log_index, LOG_INDEX);
        assert_eq!(verified.action, "rollover");
        // The leaf's digest is the Statement's DSSE PAE — the link between the log's commitment and the served bytes.
        let statement = fixture(V3, "statement.json");
        let parsed = rekor::ZoneKeyStatement::parse(&statement).expect("this build's claim");
        assert_eq!(parsed.apex, format!("{APEX}."));
        assert_eq!(parsed.action, "rollover");
        assert_eq!(parsed.keys[0].key_tag, KEY_TAG);
        assert_eq!(
            body.digest,
            rekor::sha256(&rekor::pae(DSSE_PAYLOAD_TYPE, &statement))
        );
    }
    #[tokio::test]
    async fn a_garbled_proof_record_is_refused_as_malformed() {
        let (mut zone, log) = zone_and_log();
        zone.rekor_txt = vec!["this is not a proof".to_string()];
        let error = resolve(zone, &log, RekorPolicy::Require).await.unwrap_err();
        assert!(matches!(error, NetError::RekorMalformed { .. }), "{error}");
    }
    #[test]
    fn a_raw_public_key_entry_is_refused_outright() {
        // Valid Rekor, but apex-anonymous: no monitor could ever have seen it.
        let (zone, mut log, _) = logged_zone();
        let payload = zone.zone_key_statement("create").to_json();
        let pae = rekor::pae(rekor::DSSE_PAYLOAD_TYPE, &payload);
        let body = format!(
            "{{\"apiVersion\":\"0.0.2\",\"kind\":\"hashedrekord\",\"spec\":{{\"hashedRekordV002\":\
         {{\"data\":{{\"algorithm\":\"SHA2_256\",\"digest\":\"{}\"}},\"signature\":\
         {{\"content\":\"{}\",\"verifier\":{{\"keyDetails\":\"PKIX_ECDSA_P256_SHA_256\",\
         \"publicKey\":{{\"rawBytes\":\"{}\"}}}}}}}}}}}}",
            rekor::base64_encode(&rekor::sha256(&pae)),
            rekor::base64_encode(&zone.sign_dsse(&payload)),
            rekor::base64_encode(&zone.spki()),
        );
        let proof = log.log_body(payload, body.into_bytes());
        refuses!(verify(&proof, &zone, &log), ProofError::Binding(_));
    }
    #[test]
    fn an_entry_with_no_chain_is_refused_on_the_monitors_behalf() {
        let (zone, mut log) = zone_and_log();
        let statement = zone.zone_key_statement("create");
        let proof = log.log_certified(&zone, &statement, &zone.certificate(&[]));
        refuses!(verify(&proof, &zone, &log), ProofError::Chain(why) if why.contains("no DNSSEC chain"));
    }
    #[test]
    fn a_retire_entry_is_never_authorization() {
        // Retires may be published chainless, so treating one as authorization accepts an entry carrying no proof of delegation.
        let (zone, mut log) = zone_and_log();
        let proof = log.publish(&zone, "retire");
        refuses!(verify(&proof, &zone, &log), ProofError::Binding(_));
    }

    /// Inclusion in the real tree, with teeth.
    #[test]
    fn the_real_entry_is_included_in_the_real_tree() {
        let proof = RekorProof::decode(&fixture(V3, "proof.bin")).expect("a current proof record");
        let checkpoint = rekor::Checkpoint::parse(&proof.checkpoint).unwrap();
        assert_eq!(checkpoint.origin, "log2025-1.rekor.sigstore.dev");
        let included = |body: &[u8]| {
            rekor::verify_inclusion(
                proof.log_index,
                checkpoint.tree_size,
                rekor::leaf_hash(body),
                &proof.inclusion_path,
                checkpoint.root_hash,
            )
        };
        included(&proof.canonicalized_body).expect("the audit path must reach the real root");
        let mut tampered = proof.canonicalized_body.clone();
        tampered[0] ^= 0x01;
        assert!(included(&tampered).is_err());
    }

    /// The certificate really does carry the apex into the leaf, and its chain at a size the log accepted.
    #[test]
    fn the_real_entrys_certificate_carries_its_apex_and_its_chain() {
        let body = HashedRekordBody::parse(&fixture(V3, "canonicalized_body.json"))
            .expect("a real entry body must parse");
        assert_eq!(body.certificate_der, fixture(V3, "certificate.der"));
        let san = body.certificate.single_dns_name().unwrap().to_string();
        assert_eq!(san, format!("{APEX}."));
        // The size is the empirical answer to "will Rekor take a certificate this big": it did, HTTP 201.
        assert_eq!(body.certificate_der.len(), 1118);
        let ext = body.certificate.extension(OID_DNSSEC_CHAIN).unwrap();
        let carried = DnssecChain::decode(ext).expect("and it decodes");
        // The chain starts at the declaration and the apex is the link above it: an entry nobody but the zone could have produced.
        let declaration = format!("{}.{APEX}.", chain::TRANSPARENCY_LABEL);
        assert_eq!(carried.links[0].zone, declaration);
        assert_eq!(carried.links[1].zone, format!("{APEX}."));
    }
}

/// A certificate that decodes two ways decodes no ways: Go's crypto/x509 and OpenSSL disagree
/// about these bytes, so the invariant must live in this parser.
#[test]
fn a_certificate_with_two_readings_is_refused() {
    use synch_net::x509::{Certificate, SelfSigned};

    let spki = rekor::p256_spki(&[0x11; 64]);
    let base = || SelfSigned {
        common_name: "synchronicity zone key",
        dns_name: "sync.example",
        spki: &spki,
        serial: &[0x01],
        not_before: synch_net::x509::x509_time(1_760_000_000),
        not_after: synch_net::x509::x509_time(1_900_000_000),
        extensions: &[],
    };
    let sig = |_: &[u8]| vec![0x30, 0x06, 0x02, 0x01, 0x01, 0x02, 0x01, 0x01];
    let good = base().build(sig);
    assert!(Certificate::parse(&good).is_ok(), "the control must parse");
    // Trailing bytes after the outer SEQUENCE are refused.
    let mut trailing = good.clone();
    trailing.push(0x00);
    assert!(Certificate::parse(&trailing).is_err());
    // Two copies of the chain extension: first-value and last-value readers would disagree about the deciding evidence.
    let ext = |v: &str| (OID_DNSSEC_CHAIN.to_vec(), v.as_bytes().to_vec());
    let exts = [ext("first"), ext("second")];
    let mut spec = base();
    spec.extensions = &exts;
    let parsed = Certificate::parse(&spec.build(sig)).expect("it is still a certificate");
    assert!(parsed.extension(OID_DNSSEC_CHAIN).is_none());
}

/// A lying record claiming 1/255 parts would cost every resolving client 254 round trips.
#[test]
fn a_records_part_count_is_capped_at_what_a_proof_can_need() {
    use synch_net::rekor::{parts_claimed, MAX_PROOF_PARTS};

    let liar = &["sync1p aabbccdd 1/255 QUJD".to_string()];
    assert_eq!(parts_claimed(liar), MAX_PROOF_PARTS);
    assert_eq!(parts_claimed(&["sync1p aabbccdd 1/3 QUJD".to_string()]), 3);
    assert_eq!(parts_claimed(&[]), 1);
    // The cap must clear a real proof with room to spare: an ICANN-rooted one is 8202 chars (§3).
    let capacity = MAX_PROOF_PARTS * rekor::PROOF_CHUNK_CHARS;
    assert!(capacity > 3 * 8202, "the cap must admit a real proof");
}

/// Real chain links are kilobytes: DER long-form lengths the 30-byte crossval fixture never exercises.
#[test]
fn chain_links_use_ders_long_form_lengths_exactly() {
    // A link whose rdata crosses each boundary the encoding changes at.
    let cases = [
        (127usize, vec![0x7f_u8]),     // the last value the short form can carry
        (128, vec![0x81, 0x80]),       // the first long form, one length byte
        (256, vec![0x82, 0x01, 0x00]), // two length bytes
    ];
    let chain = |size: usize| DnssecChain {
        links: vec![ChainLink {
            zone: "sync.example.".into(),
            rrs: vec![0xab; size],
        }],
    };
    for (size, len_bytes) in cases {
        let der = chain(size).encode();
        // The rdata OCTET STRING: tag 0x04, then the DER length for that size.
        let mut wanted = vec![0x04u8];
        wanted.extend_from_slice(&len_bytes);
        assert!(
            der.windows(wanted.len()).any(|w| w == wanted),
            "a {size}-byte link must encode its length as {len_bytes:02x?}"
        );
        assert_eq!(DnssecChain::decode(&der).unwrap(), chain(size));
    }
    // The reader refuses a long-form length under 128: rewrite the innermost OCTET STRING length 0x05 as 0x81 0x05.
    let mut der = chain(5).encode();
    let at = der.windows(2).position(|w| w == [0x04, 0x05]).unwrap();
    der.splice(at + 1..at + 2, [0x81, 0x05]);
    assert!(
        DnssecChain::decode(&der).is_err(),
        "a long-form length under 128 is a second spelling and must be refused"
    );
}
