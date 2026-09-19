//! The DNSSEC chain validator — the one implementation both client and monitor
//! run, so *client-accepted implies tier A* holds only because there is one.
//! Everything here is signed by zones the suite holds keys for.

mod common;

use hickory_resolver::proto::{dnssec::TrustAnchors, rr::Name};
use synch_net::{
    chain::{self, ChainError, TRANSPARENCY_TEXT},
    sim::{SimDelegation, SimZone},
    zonecert::{ChainLink, DnssecChain},
};

fn apex(text: &str) -> Name {
    chain::parse_name(text).expect("a test apex is a name")
}

/// An hour ago: RRSIG validity brackets "now", and the chain walk reads no clock.
fn inception() -> time::OffsetDateTime {
    time::OffsetDateTime::now_utc() - time::Duration::hours(1)
}

/// A chain link carrying `records`, owned by `zone`.
fn link(zone: &Name, records: Vec<hickory_resolver::proto::rr::Record>) -> ChainLink {
    ChainLink {
        zone: zone.to_string(),
        rrs: chain::encode_rrs(&records).expect("encode link"),
    }
}

/// The anchors a reader of `zone`'s self-anchored chain installs.
fn anchored_at(zone: &SimZone) -> TrustAnchors {
    let file = common::write(&zone.anchor_record());
    TrustAnchors::from_file(file.path()).expect("read anchor")
}

/// `chain::validate` of `zone`'s self-anchored chain for `apex`, expecting success.
fn valid(zone: &SimZone, apex_name: &Name) -> (Vec<Vec<u8>>, Name, chain::ValidChain) {
    chain::validate(&zone.dnssec_chain(), apex_name, &anchored_at(zone))
        .expect("a declared zone validates")
}

/// The refusal `chain::validate` of `zone`'s self-anchored chain yields.
fn invalid(zone: &SimZone, apex_name: &Name) -> ChainError {
    chain::validate(&zone.dnssec_chain(), apex_name, &anchored_at(zone))
        .expect_err("must be refused")
}

/// The self-anchored positive base case every negative test mutates.
#[test]
fn a_declared_zone_proves_the_keys_its_declaration_sits_under() {
    let zone = SimZone::new("cluster.example", Vec::new());
    let (proven, signing_zone, valid) = valid(&zone, &apex("cluster.example."));
    assert_eq!(signing_zone, apex("cluster.example."));
    assert_eq!(valid.anchor_zone, "cluster.example.");
    assert!(valid.anchored_directly, "the apex is its own anchor here");
    assert_eq!(valid.links, 1, "the declaration is not a delegation step");
    assert_eq!(proven, vec![zone.dnskey_rdata()]);
}

/// RFC 4035 §5.3.1: the signer is the closest enclosing zone; a foreign-named
/// signature may still be well-formed.
#[test]
fn a_declaration_signed_under_another_zones_name_is_refused() {
    let mut zone = SimZone::new("cluster.example", Vec::new());
    zone.sign_declaration_with(zone.signer_named("somewhere.else."));
    let text = invalid(&zone, &apex("cluster.example.")).to_string();
    assert!(text.contains("somewhere.else"), "{text}");
    assert!(text.contains("cluster.example"), "{text}");
}

/// Three declaration forgeries on the same ladder: a stranger's key, foreign-zone records, a non-declaration TXT.
#[test]
fn a_declaration_signed_by_a_stranger_is_refused() {
    let ladder = SimDelegation::new("cluster.example", Vec::new());
    let apex_name = apex("cluster.example.");
    let anchors = anchored_at(&ladder.root);
    let validate = |chain: &DnssecChain| chain::validate(chain, &apex_name, &anchors);

    // Same owner name, same signer name, a key the apex's DNSKEY RRset does not contain.
    let impostor = SimZone::for_name(apex("cluster.example."), Vec::new());
    let mut forged = ladder.chain();
    forged.links[0] = link(
        &ladder.apex.transparency_name(),
        impostor.declaration_records(inception()),
    );
    assert!(matches!(validate(&forged), Err(ChainError::Signature(_))));

    // A declaration for one zone does not carry another — refused on the name before any signature.
    let theirs = SimZone::new("other.example", Vec::new());
    let mut spliced = ladder.chain();
    spliced.links[0] = link(
        &theirs.transparency_name(),
        theirs.declaration_records(inception()),
    );
    assert!(matches!(validate(&spliced), Err(ChainError::Structure(_))));

    // A TXT at the declaration's name that does not say what a declaration says is not one.
    let zone = SimZone::new("cluster.example", Vec::new());
    let mut chain_ = zone.dnssec_chain();
    let owner = zone.transparency_name();
    chain_.links[0] = link(
        &owner,
        zone.signed_txt(owner.clone(), "v=sync1 something", inception()),
    );
    let error = chain::validate(&chain_, &apex_name, &anchored_at(&zone))
        .expect_err("a differently-worded record is not a declaration");
    assert!(
        matches!(&error, ChainError::Structure(why) if why.contains(TRANSPARENCY_TEXT)),
        "{error}"
    );
}

/// RFC 4035 §5.3.2: a wildcard expansion's RRSIG carries a short label count — the tell a catch-all declared nothing.
#[test]
fn a_wildcard_expansion_is_not_a_declaration() {
    let zone = SimZone::new("cluster.example", Vec::new());
    // Signed as `*.cluster.example.` and served under the declaration's name: the signature is genuine, the labels one short.
    let wildcard = apex("*.cluster.example.");
    let mut records = zone.signed_txt(wildcard, TRANSPARENCY_TEXT, inception());
    let owner = zone.transparency_name();
    for record in &mut records {
        record.name = owner.clone();
    }
    let mut chain_ = zone.dnssec_chain();
    chain_.links[0] = link(&owner, records);
    let error = chain::validate(&chain_, &apex("cluster.example."), &anchored_at(&zone))
        .expect_err("a wildcard expansion must not declare");
    assert!(
        matches!(&error, ChainError::Structure(why) if why.contains("wildcard")),
        "{error}"
    );
}

/// A DNSKEY without the Zone Key flag cannot anchor or sign (RFC 4034 §2.1.1) —
/// hickory reads neither flag — nor can an RFC 5011 REVOKE-flagged key.
#[test]
fn a_key_that_is_not_a_zone_key_signs_nothing_and_proves_nothing() {
    for (zone_key, revoke, needle) in [(false, false, "zone key"), (true, true, "unrevoked")] {
        let zone = SimZone::with_flags("cluster.example", Vec::new(), zone_key, revoke);
        let error = invalid(&zone, &apex("cluster.example."));
        assert!(
            matches!(&error, ChainError::Anchor(why) if why.contains(needle)),
            "{error}"
        );
    }
}

/// A revoked key published *beside* a live one is excluded from the proven set
/// while the RRset still verifies — otherwise a revoked key could mint a forged DS.
#[test]
fn a_revoked_key_beside_a_good_one_is_not_in_the_proven_set() {
    let mut zone = SimZone::new("cluster.example", Vec::new());
    let revoked = SimZone::with_flags("cluster.example", Vec::new(), true, true);
    zone.add_dnskey(revoked.dnskey());
    let (proven, _, _) = chain::validate(
        &zone.dnssec_chain(),
        &apex("cluster.example."),
        &anchored_at(&zone),
    )
    .expect("the live key still anchors and signs");
    assert_eq!(
        proven,
        vec![zone.dnskey_rdata()],
        "the revoked key is published, verified as part of the RRset, and not authorized"
    );
    assert!(!proven.contains(&revoked.dnskey_rdata()));
}

/// A chain padded with signatures costs a bounded number of verifications:
/// pairing is quadratic on hostile data, so a link offering dozens is refused.
#[test]
fn a_chain_padded_with_signatures_is_refused_rather_than_walked() {
    let zone = SimZone::new("cluster.example", Vec::new());
    let records = zone.dnskey_records(inception());
    let rrsig = records
        .iter()
        .find(|record| {
            matches!(
                record.data,
                hickory_resolver::proto::rr::RData::DNSSEC(
                    hickory_resolver::proto::dnssec::rdata::DNSSECRData::RRSIG(_)
                )
            )
        })
        .expect("the set is signed")
        .clone();

    // Copies of the real RRSIG with one byte flipped: each is a pairing the walk must try, none verifies.
    let mut junk = chain::encode_rrs(&[rrsig]).expect("encode the decoy");
    *junk.last_mut().expect("a signature has bytes") ^= 0x01;
    let mut rrs = Vec::new();
    for _ in 0..64 {
        rrs.extend_from_slice(&junk);
    }
    rrs.extend_from_slice(&chain::encode_rrs(&records).expect("encode the honest set"));

    let mut padded = zone.dnssec_chain();
    padded.links[1] = ChainLink {
        zone: "cluster.example.".into(),
        rrs,
    };
    let error = chain::validate(&padded, &apex("cluster.example."), &anchored_at(&zone))
        .expect_err("a padded link must be refused, not walked");
    assert!(
        matches!(&error, ChainError::Signature(why) if why.contains("pairings")),
        "{error}"
    );
}

// ------------------------------------------- the delegation half of the walk
// A link's DNSKEY RRset is believed because a DS its parent signed covers a
// key in it.

/// A key set the parent's DS does not cover authorizes nothing, even when every
/// other byte of the ladder is genuine — the forgery that costs an attacker
/// nothing but a keypair. The impostor is minted with the same key tag.
#[test]
fn a_key_set_the_parents_ds_does_not_cover_authorizes_nothing() {
    let real = SimDelegation::new("cluster.example", vec![]);
    let anchors = anchored_at(&real.root);
    let (proven, signing_zone, _) =
        chain::validate(&real.chain(), &apex("cluster.example."), &anchors)
            .expect("a genuine ladder walks to the root");
    assert_eq!(signing_zone, apex("cluster.example."));
    assert_eq!(proven.len(), 1);

    let mut impostor = SimZone::for_name(apex("cluster.example."), vec![]);
    while impostor.key_tag() != real.apex.key_tag() {
        impostor = SimZone::for_name(apex("cluster.example."), vec![]);
    }
    let error = chain::validate(
        &real.chain_with_substituted_apex_keys(&impostor),
        &apex("cluster.example."),
        &anchors,
    )
    .expect_err("a key set no DS covers must authorize nothing");
    assert!(
        matches!(&error, ChainError::Signature(why) if why.contains("matches a DS")),
        "the refusal must be the DS binding: {error}"
    );
}

/// RFC 4509 digest type 4 is ordinary registrar business, and `covers` dispatches
/// on the type — nothing reached that arm until this existed.
#[test]
fn a_delegation_published_with_a_sha384_ds_still_walks() {
    let delegation = SimDelegation::new("cluster.example", vec![]);
    let (proven, _, walked) = chain::validate(
        &delegation.chain_with_sha384_ds(),
        &apex("cluster.example."),
        &anchored_at(&delegation.root),
    )
    .expect("a type-4 delegation is an ordinary delegation");
    assert_eq!(proven.len(), 1);
    assert_eq!(walked.anchor_zone, ".");
}

/// A revoked key cannot sign a child's DS: `verify_ds_set` hands `verify_rrset`
/// the parent's whole RRset, so the flag rule inside it is the only guard
/// between RFC 5011's "MUST NOT be used" and a repudiated key authorizing a delegation.
#[test]
fn a_revoked_parent_key_cannot_sign_a_childs_ds() {
    let mut delegation = SimDelegation::new("cluster.example", vec![]);
    let (revoked, signer) = delegation.tld.revoked_key();
    let chain = delegation.chain_with_ds_signed_by(revoked, &signer);
    let error = chain::validate(
        &chain,
        &apex("cluster.example."),
        &anchored_at(&delegation.root),
    )
    .expect_err("a repudiated key authorizes no delegation");
    assert!(
        matches!(&error, ChainError::Signature(why) if why.contains("unrevoked zone key")),
        "the refusal must be the flag rule: {error}"
    );
}

/// A chain the **control plane collected** walks under the client's own validator —
/// the only place the two implementations' chain *semantics* meet, and the publisher
/// writes to a public append-only log, so a divergence found after the fact is
/// permanent. Fixture regenerated with `gleam run -m tools/gen_crossval`.
#[test]
fn a_chain_the_control_plane_collected_walks_under_this_validator() {
    let crossval = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../control-plane/test/fixtures/rekor/crossval");
    let der = std::fs::read(crossval.join("chain-collected.der"))
        .expect("the Gleam-collected chain fixture");
    let chain = DnssecChain::decode(&der).expect("a chain this reader can decode");

    // The shape the collector is supposed to produce, asserted before the crypto.
    assert_eq!(chain.links.len(), 3, "declaration, apex, root");
    assert_eq!(
        chain.links[0].zone,
        "_synchronicity-transparency.sync.test."
    );
    assert_eq!(chain.links[1].zone, "sync.test.");
    assert_eq!(chain.links[2].zone, ".");

    let anchors = TrustAnchors::from_file(&crossval.join("chain-anchor.key"))
        .expect("the root anchor the same run wrote");
    let (proven, signing_zone, walked) = chain::validate(&chain, &apex("sync.test."), &anchors)
        .expect("a chain the control plane collected must walk here");
    assert_eq!(signing_zone, apex("sync.test."));
    assert_eq!(walked.anchor_zone, ".");
    assert!(
        !walked.anchored_directly,
        "the fixture is a real descent, not a self-anchored zone"
    );
    assert_eq!(proven.len(), 1);
}
