//! A signed DNSSEC zone and a transparency log served to a client that
//! cannot tell them from the real things — test support (§3.2, §9 of
//! docs/REKOR-ZONE-KEY.md).
//!
//! What the e2e suites need and the real DNS cannot give them: a zone whose
//! signing key *is* the trust anchor, served from a loopback endpoint, so the
//! full validation path — TXT, RRSIG, DNSKEY, anchor — runs against traffic
//! the test controls. [`SimLog`] does the same for the log half: it appends
//! entries, publishes checkpoints and answers with audit paths in exactly the
//! formats [`crate::rekor`] consumes, so every verification failure can be
//! provoked one at a time. Everything here is deliberate test machinery:
//! hidden from the docs, no API stability, no place in a production call path.

use std::sync::Arc;

use hickory_resolver::proto::{
    dnssec::{
        crypto::EcdsaSigningKey, rdata::DNSSECRData, rdata::DNSKEY, rdata::RRSIG, Algorithm,
        DigestType, DnssecSigner, PublicKey, SigningKey,
    },
    op::Message,
    rr::{rdata::TXT, DNSClass, Name, RData, Record, RecordSet, RecordType},
};

use crate::{
    chain,
    dns::TXT_PREFIX,
    rekor::{self, RekorProof, ZoneKeyStatement},
    tuf::{self, TufMetadata},
    x509::{self, SelfSigned},
    zonecert::{ChainLink, DnssecChain, OID_DNSSEC_CHAIN},
};

/// One signed zone: an origin, its TXT membership records, and the key that
/// signs both — which the test installs as the whole root of trust.
#[doc(hidden)]
#[allow(missing_debug_implementations)]
pub struct SimZone {
    origin: Name,
    signer: DnssecSigner,
    dnskey: DNSKEY,
    /// The PKCS#8 the zone key was loaded from, kept so the zone can also
    /// sign things that are not RRsets — a DSSE payload, in this design.
    pkcs8: Vec<u8>,
    txt: Vec<String>,
    ttl: u32,
    /// Proof records served at `_synchronicity-rekor.<origin>`, each one
    /// base64url as [`RekorProof::to_txt`] renders it. Empty is the
    /// not-yet-upgraded control plane.
    pub rekor_txt: Vec<String>,
    /// The control-plane attach records served at `_synchronicity-cp.<origin>`,
    /// e.g. `v=synccp1 url=https://sync.example`. One per node of the
    /// deployment's fleet, since a daemon holds a tunnel to each. Empty is a
    /// deployment that does not offer cloud attach, and the name does not
    /// exist.
    pub cp_txt: Vec<String>,
    /// Extra DNSKEYs served at the apex after the zone's own key.
    extra_dnskeys: Vec<DNSKEY>,
    /// If set, membership (and impersonated) TXT is signed by this instead of
    /// the zone CSK. Other RRsets stay under `signer`.
    txt_signer: Option<DnssecSigner>,
    declaration_signer: Option<DnssecSigner>,
    /// If set, the control-plane attach record is signed by this instead of
    /// the zone CSK.
    ///
    /// The shape a compromised parent produces: a second key added to the
    /// apex DNSKEY RRset (so every answer it signs validates), used to sign
    /// only the record that redirects a daemon to a control plane, while the
    /// membership answer keeps its genuine signature by the logged key. A
    /// harness that could only sign every RRset with one key could not say
    /// whether the transparency gate reaches this record.
    cp_signer: Option<DnssecSigner>,
    /// A TXT RRset this zone serves at a name it **does not own**, signed by
    /// its own key.
    ///
    /// This is the forgery an attacker mounts when they hold any
    /// DNSSEC-signed zone: an RRSIG is a signature over an RRset, and
    /// nothing about making one requires the signer to be the owner. Whether
    /// it is *accepted* is the validator's job — and hickory does not do it
    /// (RFC 4035 §5.3.1, skipped there with a TODO), so `crate::dns` has to.
    /// A harness that could not express the forgery could not test the
    /// defense.
    pub impersonate: Option<(Name, Vec<String>)>,
    /// TXT records appended to the membership answer **after signing**, at
    /// the queried name, in a class the RRSIG does not cover.
    ///
    /// This is the other forgery available to anyone who can add a record to
    /// a response — an on-path attacker against a plaintext DoH endpoint, or
    /// the resolver itself. It costs nothing to mount: hickory groups RRsets
    /// by `(name, record_type)` and stamps its verdict on every member, while
    /// the signed-data construction filters by class and drops these — so the
    /// honest RRSIG still verifies and the spliced record comes back marked
    /// `Proof::Secure` having been signed by nobody.
    ///
    /// The defense is `dns::covered_by_signed_data`, and it is only testable
    /// if the harness can put an unsigned record inside a validated answer.
    pub splice_foreign_class: Vec<String>,
}

impl SimZone {
    /// Builds a zone for `origin` (e.g. `cluster.example`) publishing `txt`
    /// under `_synchronicity.<origin>`, signed by a fresh ECDSA P-256 key.
    pub fn new(origin: &str, txt: Vec<String>) -> SimZone {
        SimZone::for_name(
            Name::from_utf8(format!("{origin}.")).expect("origin name"),
            txt,
        )
    }

    /// The same, for a name that is already an FQDN — including the root,
    /// which `new`'s "append a dot" spelling cannot express.
    pub fn for_name(origin: Name, txt: Vec<String>) -> SimZone {
        SimZone::keyed(origin, txt, |public| DNSKEY::from_key(public))
    }

    /// A zone whose DNSKEY carries the flags the caller names, rather than the
    /// ordinary secure-entry-point zone key.
    ///
    /// The flags are what RFC 4034 §2.1.1 and RFC 5011 §2.1 turn on — a key with
    /// no Zone Key bit may not verify an RRset, and a REVOKE-flagged key may not
    /// be used at all — and the DNSSEC stack underneath this design reads
    /// neither. A harness that could only mint well-flagged keys could not test
    /// the rules.
    pub fn with_flags(origin: &str, txt: Vec<String>, zone_key: bool, revoke: bool) -> SimZone {
        SimZone::keyed(
            Name::from_utf8(format!("{origin}.")).expect("origin name"),
            txt,
            move |public| DNSKEY::new(zone_key, true, revoke, public.to_owned()),
        )
    }

    fn keyed(
        origin: Name,
        txt: Vec<String>,
        dnskey_of: impl FnOnce(&hickory_resolver::proto::dnssec::PublicKeyBuf) -> DNSKEY,
    ) -> SimZone {
        let algorithm = Algorithm::ECDSAP256SHA256;
        let pkcs8 = EcdsaSigningKey::generate_pkcs8(algorithm).expect("keygen");
        let key = EcdsaSigningKey::from_pkcs8(&pkcs8, algorithm).expect("key load");
        let public = key.to_public_key().expect("public key");
        let dnskey = dnskey_of(&public);
        let signer = DnssecSigner::new(
            dnskey.clone(),
            Box::new(key),
            origin.clone(),
            std::time::Duration::from_secs(86_400),
        );
        SimZone {
            origin,
            signer,
            dnskey,
            pkcs8: pkcs8.secret_pkcs8_der().to_vec(),
            txt,
            ttl: 300,
            rekor_txt: Vec::new(),
            cp_txt: Vec::new(),
            extra_dnskeys: Vec::new(),
            txt_signer: None,
            declaration_signer: None,
            cp_signer: None,
            impersonate: None,
            splice_foreign_class: Vec::new(),
        }
    }

    /// The apex, as an FQDN with its root dot.
    pub fn apex(&self) -> String {
        self.origin.to_string()
    }

    /// The zone key's DNSKEY, for a test that wants to serve it somewhere else.
    pub fn dnskey(&self) -> DNSKEY {
        self.dnskey.clone()
    }

    /// The zone key's DNSKEY rdata: flags, protocol, algorithm, key.
    pub fn dnskey_rdata(&self) -> Vec<u8> {
        let mut rdata = Vec::with_capacity(4 + 64);
        rdata.extend_from_slice(&self.dnskey.flags().to_be_bytes());
        rdata.push(3);
        rdata.push(u8::from(self.dnskey.public_key().algorithm()));
        rdata.extend_from_slice(self.dnskey.public_key().public_bytes());
        rdata
    }

    /// The zone key's tag, as an RRSIG names it.
    pub fn key_tag(&self) -> u16 {
        self.dnskey.calculate_key_tag().expect("key tag")
    }

    /// A second P-256 DNSKEY with the same RFC 4034 key tag, plus a signer
    /// that will sign as this zone. Caps the search so a flaky RNG cannot
    /// hang the suite.
    pub fn colliding_key(&self) -> (DNSKEY, DnssecSigner) {
        let algorithm = Algorithm::ECDSAP256SHA256;
        let want = self.key_tag();
        for _ in 0..1_000_000 {
            let pkcs8 = EcdsaSigningKey::generate_pkcs8(algorithm).expect("keygen");
            let key = EcdsaSigningKey::from_pkcs8(&pkcs8, algorithm).expect("key load");
            let public = key.to_public_key().expect("public key");
            let dnskey = DNSKEY::from_key(&public);
            if dnskey.calculate_key_tag().ok() != Some(want) {
                continue;
            }
            let signer = DnssecSigner::new(
                dnskey.clone(),
                Box::new(key),
                self.origin.clone(),
                std::time::Duration::from_secs(86_400),
            );
            return (dnskey, signer);
        }
        panic!("no P-256 key with tag {want} in 1_000_000 draws");
    }

    /// Serves `dnskey` at the apex after the zone's own key.
    pub fn add_dnskey(&mut self, dnskey: DNSKEY) {
        self.extra_dnskeys.push(dnskey);
    }

    /// Signs membership (and impersonated) TXT with `signer` instead of the
    /// zone CSK.
    pub fn sign_txt_with(&mut self, signer: DnssecSigner) {
        self.txt_signer = Some(signer);
    }

    /// A signer holding *this zone's key* but naming `signer_name` in the
    /// RRSIGs it makes.
    ///
    /// The one shape that reaches `verify_declaration`'s signer-name check.
    /// An RRSIG carries its own `signer_name`, and verification reconstructs
    /// the signed data from the record, so a signature made under a foreign
    /// name still verifies against the zone's DNSKEY — the key material and
    /// the key tag both match. Only the name comparison catches it.
    pub fn signer_named(&self, signer_name: &str) -> DnssecSigner {
        // A fresh key, deliberately. `verify_declaration` compares the signer
        // name *before* it verifies the signature, so what this has to
        // produce is an RRSIG naming the wrong zone — whether that RRSIG
        // would also verify is the next check's business, and making the key
        // match would only test the two checks together.
        let algorithm = Algorithm::ECDSAP256SHA256;
        let pkcs8 = EcdsaSigningKey::generate_pkcs8(algorithm).expect("keygen");
        let key = EcdsaSigningKey::from_pkcs8(&pkcs8, algorithm).expect("key load");
        DnssecSigner::new(
            self.dnskey.clone(),
            Box::new(key),
            chain::parse_name(signer_name).expect("a signer name"),
            std::time::Duration::from_secs(86_400),
        )
    }

    /// Signs the transparency declaration with `signer` instead of the zone
    /// CSK — see [`SimZone::signer_named`].
    pub fn sign_declaration_with(&mut self, signer: DnssecSigner) {
        self.declaration_signer = Some(signer);
    }

    /// Signs the control-plane attach record with `signer` instead of the
    /// zone CSK — see the `cp_signer` field.
    pub fn sign_cp_with(&mut self, signer: DnssecSigner) {
        self.cp_signer = Some(signer);
    }

    /// A **revoked** zone key for this zone, with a signer for it.
    ///
    /// RFC 5011 §2.1: a key carrying REVOKE "MUST NOT be used". hickory's
    /// verifier does not look at flags at all, so honoring that is entirely
    /// `chain::usable_signer`'s job, and expressing the rule needs a key that
    /// is genuinely in the RRset and genuinely forbidden.
    pub fn revoked_key(&self) -> (DNSKEY, DnssecSigner) {
        let algorithm = Algorithm::ECDSAP256SHA256;
        let pkcs8 = EcdsaSigningKey::generate_pkcs8(algorithm).expect("keygen");
        let key = EcdsaSigningKey::from_pkcs8(&pkcs8, algorithm).expect("key load");
        let public = key.to_public_key().expect("public key");
        let dnskey = DNSKEY::new(true, true, true, public);
        let signer = DnssecSigner::new(
            dnskey.clone(),
            Box::new(key),
            self.origin.clone(),
            std::time::Duration::from_secs(86_400),
        );
        (dnskey, signer)
    }

    /// A second P-256 zone key for this zone, with a signer for it.
    ///
    /// Unlike [`SimZone::colliding_key`] the tag is whatever it comes out as:
    /// this is an ordinary extra key, the shape a zone has mid-rollover and
    /// the shape a compromised parent adds. Served at the apex once it is
    /// handed to [`SimZone::add_dnskey`], so anything it signs validates.
    pub fn second_key(&self) -> (DNSKEY, DnssecSigner) {
        let algorithm = Algorithm::ECDSAP256SHA256;
        let pkcs8 = EcdsaSigningKey::generate_pkcs8(algorithm).expect("keygen");
        let key = EcdsaSigningKey::from_pkcs8(&pkcs8, algorithm).expect("key load");
        let public = key.to_public_key().expect("public key");
        let dnskey = DNSKEY::from_key(&public);
        let signer = DnssecSigner::new(
            dnskey.clone(),
            Box::new(key),
            self.origin.clone(),
            std::time::Duration::from_secs(86_400),
        );
        (dnskey, signer)
    }

    /// The DS field an operator hands a registrar: `<tag> <alg> 2 <sha256
    /// hex>` over the owner name and the DNSKEY rdata (RFC 4034 §5.1.4).
    pub fn ds_field(&self) -> String {
        let rdata = self.dnskey_rdata();
        let mut input = name_wire(&self.origin);
        input.extend_from_slice(&rdata);
        format!(
            "{} {} 2 {}",
            self.key_tag(),
            u8::from(self.dnskey.public_key().algorithm()),
            hex::encode(rekor::sha256(&input))
        )
    }

    /// Signs a DSSE payload's PAE with the zone key — which doubles as the
    /// sim's entry *signer*: the certificate's SubjectPublicKeyInfo is this
    /// key, so attribution verifies. Nothing requires the signer to be the
    /// zone key; tests that want a distinct signer mint their own
    /// certificate around another key.
    ///
    /// DER/ASN.1, the encoding a Rekor entry's `signature.content` carries
    /// (and what the client's attribution check verifies), not the raw
    /// `r||s` of a DNSSEC signature.
    pub fn sign_dsse(&self, payload: &[u8]) -> Vec<u8> {
        sign_p256_der(&self.pkcs8, &rekor::pae(rekor::DSSE_PAYLOAD_TYPE, payload))
    }

    /// The zone key's DER SubjectPublicKeyInfo — what the logged entry's
    /// certificate carries as its SubjectPublicKeyInfo.
    pub fn spki(&self) -> Vec<u8> {
        rekor::p256_spki(self.dnskey.public_key().public_bytes())
    }

    /// The apex DNSKEY RRset and its RRSIG, as records.
    ///
    /// `inception` moves the signature's validity window, which is how a test
    /// asks for a chain that was valid when it was logged and is expired now
    /// — the archival case the client and the monitor must both still accept.
    pub fn dnskey_records(&self, inception: time::OffsetDateTime) -> Vec<Record> {
        let mut set = RecordSet::new(self.origin.clone(), RecordType::DNSKEY, 0);
        // The zone's own key first, then whatever else it publishes at the
        // apex: an RRset in a chain has to be the RRset the zone serves, or a
        // test about the *other* keys of a proven set has no way to speak.
        for dnskey in std::iter::once(&self.dnskey).chain(&self.extra_dnskeys) {
            set.insert(
                Record::from_rdata(
                    self.origin.clone(),
                    self.ttl,
                    RData::DNSSEC(DNSSECRData::DNSKEY(dnskey.clone())),
                ),
                0,
            );
        }
        let rrsig = RRSIG::from_rrset(&set, DNSClass::IN, inception, &self.signer)
            .expect("sign dnskey set");
        set.insert_rrsig(Record::from_rdata(
            self.origin.clone(),
            self.ttl,
            RData::DNSSEC(DNSSECRData::RRSIG(rrsig)),
        ));
        set.records(true).cloned().collect()
    }

    /// One TXT RRset at `owner`, signed by this zone's key as this zone.
    ///
    /// `owner` is free rather than derived so a test can sign a name the zone
    /// would never publish — a wildcard, or a name under somebody else — and
    /// see the validator refuse it for the right reason.
    pub fn signed_txt(
        &self,
        owner: Name,
        text: &str,
        inception: time::OffsetDateTime,
    ) -> Vec<Record> {
        self.signed_txt_by(owner, text, inception, &self.signer)
    }

    /// The same, under a caller-chosen signer.
    pub(crate) fn signed_txt_by(
        &self,
        owner: Name,
        text: &str,
        inception: time::OffsetDateTime,
        signer: &DnssecSigner,
    ) -> Vec<Record> {
        let mut set = RecordSet::new(owner.clone(), RecordType::TXT, 0);
        set.insert(
            Record::from_rdata(
                owner.clone(),
                self.ttl,
                RData::TXT(TXT::new(vec![text.to_string()])),
            ),
            0,
        );
        let rrsig =
            RRSIG::from_rrset(&set, DNSClass::IN, inception, signer).expect("sign txt rrset");
        set.insert_rrsig(Record::from_rdata(
            owner,
            self.ttl,
            RData::DNSSEC(DNSSECRData::RRSIG(rrsig)),
        ));
        set.records(true).cloned().collect()
    }

    /// The zone's transparency declaration and the RRSIG it made over it —
    /// the chain's bottom link, and the thing that makes an entry the zone's
    /// own statement rather than a copy of its public records.
    pub fn declaration_records(&self, inception: time::OffsetDateTime) -> Vec<Record> {
        self.signed_txt_by(
            self.transparency_name(),
            chain::TRANSPARENCY_TEXT,
            inception,
            self.declaration_signer.as_ref().unwrap_or(&self.signer),
        )
    }

    /// The DNSSEC chain this zone's entries carry.
    ///
    /// A simulated zone *is* its own trust anchor — the tests install its
    /// DNSKEY with `--dnssec-anchor` — so the ladder above the declaration is
    /// the degenerate one-link shape: the anchored zone's own DNSKEY RRset,
    /// self-signed. Real deployments anchored at the ICANN root produce the
    /// DS ladder instead; both shapes are validated by the same walk
    /// (`crate::chain`).
    pub fn dnssec_chain(&self) -> DnssecChain {
        self.dnssec_chain_at(time::OffsetDateTime::now_utc() - time::Duration::hours(1))
    }

    /// The same chain with the RRSIG inception moved (see `dnskey_records`).
    pub fn dnssec_chain_at(&self, inception: time::OffsetDateTime) -> DnssecChain {
        DnssecChain {
            links: vec![
                ChainLink {
                    zone: self.transparency_name().to_string(),
                    rrs: chain::encode_rrs(&self.declaration_records(inception))
                        .expect("encode declaration link"),
                },
                ChainLink {
                    zone: self.apex(),
                    rrs: chain::encode_rrs(&self.dnskey_records(inception))
                        .expect("encode chain link"),
                },
            ],
        }
    }

    /// The DS RRset for `child`, signed by *this* zone — a real delegation
    /// step, as a parent publishes it.
    pub(crate) fn ds_records_for(
        &self,
        child: &SimZone,
        inception: time::OffsetDateTime,
    ) -> Vec<Record> {
        self.ds_records_by(child, inception, DigestType::SHA256, None)
    }

    /// The same over RFC 4509 digest type 4, which `chain::covers` also
    /// follows and which a registrar may publish instead of type 2.
    pub(crate) fn ds_records_sha384_for(
        &self,
        child: &SimZone,
        inception: time::OffsetDateTime,
    ) -> Vec<Record> {
        self.ds_records_by(child, inception, DigestType::SHA384, None)
    }

    /// The same, signed by a caller-chosen key of this zone.
    ///
    /// A parent's DS RRset is verified against the parent's **whole**
    /// DNSKEY RRset — `verify_ds_set` passes it unfiltered — so the only
    /// thing stopping a key the standards forbid from signing a delegation is
    /// the flag check inside `verify_rrset`. Expressing that needs a DS signed
    /// by a key that is in the RRset and must not be used, which is what this
    /// is for.
    pub(crate) fn ds_records_signed_by(
        &self,
        child: &SimZone,
        inception: time::OffsetDateTime,
        signer: &DnssecSigner,
    ) -> Vec<Record> {
        self.ds_records_by(child, inception, DigestType::SHA256, Some(signer))
    }

    fn ds_records_by(
        &self,
        child: &SimZone,
        inception: time::OffsetDateTime,
        digest_type: DigestType,
        signer: Option<&DnssecSigner>,
    ) -> Vec<Record> {
        use hickory_resolver::proto::dnssec::rdata::DS;
        let mut set = RecordSet::new(child.origin.clone(), RecordType::DS, 0);
        // Built by hand rather than with `DS::from_key`, which derives the key
        // tag from the bare public key instead of the whole DNSKEY rdata that
        // RFC 4034 App. B specifies. `calculate_key_tag` is the correct one —
        // it is what the chain validator matches RRSIGs against, and what the
        // real `cloudflare.com` fixture agrees with.
        let ds = DS::new(
            child.dnskey.calculate_key_tag().expect("key tag"),
            child.dnskey.public_key().algorithm(),
            digest_type,
            child
                .dnskey
                .to_digest(&child.origin, digest_type)
                .expect("ds digest")
                .as_ref()
                .to_owned(),
        );
        set.insert(
            Record::from_rdata(
                child.origin.clone(),
                self.ttl,
                RData::DNSSEC(DNSSECRData::DS(ds)),
            ),
            0,
        );
        let rrsig = RRSIG::from_rrset(
            &set,
            DNSClass::IN,
            inception,
            signer.unwrap_or(&self.signer),
        )
        .expect("sign ds set");
        set.insert_rrsig(Record::from_rdata(
            child.origin.clone(),
            self.ttl,
            RData::DNSSEC(DNSSECRData::RRSIG(rrsig)),
        ));
        set.records(true).cloned().collect()
    }

    /// The self-signed certificate that carries this zone's name into a
    /// Merkle leaf, with whatever extensions the caller wants inside it.
    ///
    /// Nothing validates this certificate — not Rekor, not the client, not
    /// the monitor. It is a key envelope whose SAN is the payload.
    pub fn certificate(&self, extensions: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
        self.certificate_for(self.apex().trim_end_matches('.'), extensions)
    }

    /// The same certificate with an arbitrary SAN — how a test mints the
    /// entry an attacker would: this zone's key, somebody else's name, or a
    /// string that is not a name at all.
    ///
    /// The SAN is written **verbatim**. Nothing is trimmed or normalized on
    /// the way in, because the shapes worth testing here are exactly the ones
    /// normalization would erase: `"x.."` has to reach the certificate as
    /// `"x.."` or the test proves nothing.
    pub fn certificate_for(&self, apex: &str, extensions: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
        let spki = self.spki();
        let serial = rekor::sha256(&spki);
        SelfSigned {
            common_name: "synchronicity zone key",
            dns_name: apex,
            spki: &spki,
            serial: &serial[..20],
            not_before: x509::x509_time(1_700_000_000),
            not_after: x509::x509_time(4_900_000_000),
            extensions,
        }
        .build(|tbs| sign_p256_der(&self.pkcs8, tbs))
    }

    /// The certificate an ordinary `create` or `rollover` entry carries: the
    /// chain, and nothing else the design reads.
    pub fn zone_key_certificate(&self) -> Vec<u8> {
        self.certificate(&[(OID_DNSSEC_CHAIN.to_vec(), self.dnssec_chain().encode())])
    }

    /// The Statement this zone's control plane would publish for its key
    /// set — here the one-key degenerate case, a CSK zone.
    pub fn zone_key_statement(&self, action: &str) -> ZoneKeyStatement {
        ZoneKeyStatement::for_keys(&self.apex(), &[self.dnskey_rdata()], action)
    }

    /// The name this zone's transparency declaration lives under.
    pub fn transparency_name(&self) -> Name {
        chain::transparency_name(&self.origin).expect("transparency name")
    }

    /// The name the control-plane attach record lives under.
    pub(crate) fn cp_name(&self) -> Name {
        Name::from_utf8(format!("{}.{}", crate::dns::CP_TXT_PREFIX, self.origin)).expect("cp name")
    }

    /// Which proof part `name` is the owner for, if any.
    fn rekor_part_index(&self, name: &Name) -> Option<usize> {
        let text = name.to_string();
        let origin = self.origin.to_string();
        let label = text.strip_suffix(&format!(".{origin}"))?;
        match label.strip_prefix(rekor::REKOR_TXT_PREFIX)? {
            "" => Some(1),
            rest => rest.strip_prefix('-')?.parse().ok(),
        }
    }

    /// The trust-anchor line for this zone's key, in the file syntax
    /// `--dnssec-anchor` reads. Whoever anchors this line trusts this zone —
    /// and nothing signed under the real root.
    pub fn anchor_record(&self) -> String {
        let mut out = format!(
            "{} IN DNSKEY 257 3 13 {}\n",
            self.origin,
            base64(self.dnskey.public_key().public_bytes())
        );
        // Extra apex keys belong in the same universe: leaving them out
        // sends hickory looking for a DS this zone does not serve.
        for dnskey in &self.extra_dnskeys {
            out.push_str(&format!(
                "{} IN DNSKEY {} 3 {} {}\n",
                self.origin,
                dnskey.flags(),
                u8::from(dnskey.public_key().algorithm()),
                base64(dnskey.public_key().public_bytes())
            ));
        }
        out
    }

    /// Serves the zone over plaintext RFC 8484 DoH on a loopback port,
    /// returning the endpoint URL. The task serves until aborted or dropped.
    pub async fn serve(self) -> (String, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let zone = Arc::new(self);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let url = format!(
            "http://127.0.0.1:{}/dns-query",
            listener.local_addr().expect("local addr").port()
        );
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let zone = zone.clone();
                tokio::spawn(async move {
                    let mut raw = Vec::new();
                    let mut buf = [0u8; 4096];
                    let query = loop {
                        let n = stream.read(&mut buf).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        raw.extend_from_slice(&buf[..n]);
                        if let Some(split) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
                            let length: usize = head
                                .lines()
                                .find_map(|l| l.strip_prefix("content-length:"))
                                .and_then(|v| v.trim().parse().ok())
                                .unwrap_or(0);
                            if raw.len() - split - 4 >= length {
                                break raw[split + 4..split + 4 + length].to_vec();
                            }
                        }
                    };
                    let Ok(request) = Message::from_vec(&query) else {
                        return;
                    };
                    let reply = zone.answer(&request).to_vec().expect("encode reply");
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/dns-message\r\n\
                         content-length: {}\r\nconnection: close\r\n\r\n",
                        reply.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.write_all(&reply).await;
                });
            }
        });
        (url, task)
    }

    /// Answers one query: the TXT set, the DNSKEY set, or an empty NOERROR.
    fn answer(&self, request: &Message) -> Message {
        let mut response = request.clone().into_response();
        let Some(query) = request.queries.first().cloned() else {
            return response;
        };
        let name = query.name().clone();
        let mut set = match query.query_type() {
            // Served *before* the zone's own names, because the whole point
            // is a name this zone has no business answering for.
            RecordType::TXT
                if self
                    .impersonate
                    .as_ref()
                    .is_some_and(|(owner, _)| *owner == name) =>
            {
                let (owner, texts) = self.impersonate.as_ref().expect("checked");
                let mut set = RecordSet::new(owner.clone(), RecordType::TXT, 0);
                for text in texts {
                    set.insert(
                        Record::from_rdata(
                            owner.clone(),
                            self.ttl,
                            RData::TXT(TXT::new(vec![text.clone()])),
                        ),
                        0,
                    );
                }
                set
            }
            RecordType::TXT if name == self.txt_name() => {
                let mut set = RecordSet::new(name, RecordType::TXT, 0);
                for text in &self.txt {
                    set.insert(
                        Record::from_rdata(
                            self.txt_name(),
                            self.ttl,
                            RData::TXT(TXT::new(vec![text.clone()])),
                        ),
                        0,
                    );
                }
                set
            }
            // The declaration is served like any other record: the chain
            // carries a copy, but a zone that publishes one really does have
            // it in DNS, and the collector reads it from there.
            RecordType::TXT if name == self.transparency_name() => {
                let mut set = RecordSet::new(name.clone(), RecordType::TXT, 0);
                set.insert(
                    Record::from_rdata(
                        name,
                        self.ttl,
                        RData::TXT(TXT::new(vec![chain::TRANSPARENCY_TEXT.to_string()])),
                    ),
                    0,
                );
                set
            }
            // Proof parts live one per name: part 1 at the base, later parts
            // one label along. The zone serves whichever the query asks for.
            RecordType::TXT
                if !self.rekor_txt.is_empty() && self.rekor_part_index(&name).is_some() =>
            {
                let index = self.rekor_part_index(&name).expect("checked");
                let mine: Vec<String> = self
                    .rekor_txt
                    .iter()
                    // A record with no readable header still belongs
                    // somewhere: the base name, where a client looks first,
                    // so "the zone published gibberish" stays reachable.
                    .filter(|record| crate::rekor::part_index_of(record).unwrap_or(1) == index)
                    .cloned()
                    .collect();
                if mine.is_empty() {
                    return response;
                }
                self.chunked_txt(name, &mine)
            }
            RecordType::TXT if !self.cp_txt.is_empty() && name == self.cp_name() => {
                let mut set = RecordSet::new(name.clone(), RecordType::TXT, 0);
                for text in &self.cp_txt {
                    set.insert(
                        Record::from_rdata(
                            name.clone(),
                            self.ttl,
                            RData::TXT(TXT::new(vec![text.clone()])),
                        ),
                        0,
                    );
                }
                set
            }
            RecordType::DNSKEY if name == self.origin => {
                let mut set = RecordSet::new(name, RecordType::DNSKEY, 0);
                set.insert(
                    Record::from_rdata(
                        self.origin.clone(),
                        self.ttl,
                        RData::DNSSEC(DNSSECRData::DNSKEY(self.dnskey.clone())),
                    ),
                    0,
                );
                // Extra keys come *after* the zone CSK so a first-match
                // tag lookup still sees the logged key first — the
                // colliding-tag attack the client's signer check closes.
                for dnskey in &self.extra_dnskeys {
                    set.insert(
                        Record::from_rdata(
                            self.origin.clone(),
                            self.ttl,
                            RData::DNSSEC(DNSSECRData::DNSKEY(dnskey.clone())),
                        ),
                        0,
                    );
                }
                set
            }
            _ => return response,
        };
        // Inception an hour ago: RRSIG validity has to bracket "now".
        let inception = time::OffsetDateTime::now_utc() - time::Duration::hours(1);
        let qname = query.name();
        let membership_txt = query.query_type() == RecordType::TXT
            && (*qname == self.txt_name()
                || self
                    .impersonate
                    .as_ref()
                    .is_some_and(|(owner, _)| owner == qname));
        let cp_txt = query.query_type() == RecordType::TXT && *qname == self.cp_name();
        let signer = if membership_txt {
            self.txt_signer.as_ref().unwrap_or(&self.signer)
        } else if cp_txt {
            self.cp_signer.as_ref().unwrap_or(&self.signer)
        } else {
            &self.signer
        };
        let rrsig = RRSIG::from_rrset(&set, DNSClass::IN, inception, signer).expect("sign rrset");
        set.insert_rrsig(Record::from_rdata(
            set.name().clone(),
            self.ttl,
            RData::DNSSEC(DNSSECRData::RRSIG(rrsig)),
        ));

        response.add_answers(set.records(true).cloned());
        // Spliced in *after* signing and outside the RecordSet entirely, at
        // the queried name but in another class: the record an attacker adds
        // to a response, covered by no signature in it.
        if !self.splice_foreign_class.is_empty() && *query.name() == self.txt_name() {
            for text in &self.splice_foreign_class {
                let mut record = Record::from_rdata(
                    self.txt_name(),
                    self.ttl,
                    RData::TXT(TXT::new(vec![text.clone()])),
                );
                record.dns_class = DNSClass::CH;
                response.add_answer(record);
            }
        }
        response
    }

    /// The name the membership records live under.
    pub(crate) fn txt_name(&self) -> Name {
        Name::from_utf8(format!("{TXT_PREFIX}.{}", self.origin)).expect("txt name")
    }

    /// One TXT RRset carrying long base64url payloads.
    ///
    /// A proof is kilobytes; TXT carries it as consecutive ≤255-byte
    /// character-strings, which the client concatenates before decoding (§3).
    fn chunked_txt(&self, owner: Name, payloads: &[String]) -> RecordSet {
        let mut set = RecordSet::new(owner.clone(), RecordType::TXT, 0);
        for text in payloads {
            let chunks = text
                .as_bytes()
                .chunks(255)
                .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
                .collect();
            set.insert(
                Record::from_rdata(owner.clone(), 86_400, RData::TXT(TXT::new(chunks))),
                0,
            );
        }
        set
    }
}

/// A synthetic delegation ladder: root → TLD → apex, each with its own key.
///
/// Why this exists is worth stating, because its absence hid two real bugs.
/// [`SimZone::dnssec_chain`] only ever emits the degenerate **one-link,
/// self-anchored** shape — the zone is its own trust anchor, which is what
/// `--dnssec-anchor` deployments and every sim test used. So the suites that
/// assert the client↔monitor invariant "over every shape the two sides could
/// disagree about" were, in fact, exercising a single branch of
/// [`crate::chain::validate`], and never the multi-link ladder production
/// actually emits. A divergence that only appears once a chain has a parent
/// to climb to was invisible.
///
/// This is that ladder, small enough to build in a test and real enough to
/// walk: three zones, three keys, DS records signed by actual parents, and a
/// root whose DNSKEY is the only thing a reader has to be told to trust.
///
/// The zone cut between the middle zone and the apex is a *parameter*, because
/// DNS cuts are not one label each: [`SimDelegation::spanning`] delegates an
/// apex several labels below its parent, which is the shape a label-counting
/// ladder rule makes unencodable.
#[doc(hidden)]
#[allow(missing_debug_implementations)]
pub struct SimDelegation {
    /// The root zone. Its DNSKEY is the trust anchor.
    pub root: SimZone,
    /// The intermediate zone, e.g. `example.`.
    pub tld: SimZone,
    /// The zone the entry is about, e.g. `cluster.example.`.
    pub apex: SimZone,
}

impl SimDelegation {
    /// A ladder terminating at `apex`, delegated one label at a time from its
    /// immediate parent — the ordinary shape, e.g. `cluster.example`.
    pub fn new(apex: &str, txt: Vec<String>) -> SimDelegation {
        let apex_name = Name::from_utf8(format!("{apex}.")).expect("apex name");
        assert!(
            apex_name.num_labels() >= 2,
            "a sim delegation is root → TLD → apex, so the apex needs a parent \
             between it and the root"
        );
        let tld = apex_name.base_name();
        SimDelegation::between(Name::root(), tld, apex_name, txt)
    }

    /// A ladder whose bottom cut spans **several labels**: `tld` delegates
    /// `apex` directly, with no zone at the names in between.
    ///
    /// This is a perfectly ordinary DNS arrangement — `example.com` publishing
    /// NS and DS for `cp.acme.example.com` with nothing at `acme.example.com` —
    /// and it is the shape no label-counting ladder rule can encode: a link for
    /// the empty non-terminal has neither DNSKEY nor DS, and omitting it breaks
    /// a parent-name check. `apex` must be strictly below `tld`.
    pub fn spanning(apex: &str, tld: &str, txt: Vec<String>) -> SimDelegation {
        let apex_name = Name::from_utf8(format!("{apex}.")).expect("apex name");
        let tld_name = Name::from_utf8(format!("{tld}.")).expect("tld name");
        assert!(
            tld_name.zone_of(&apex_name) && tld_name != apex_name,
            "the delegating zone must be a proper ancestor of the apex"
        );
        SimDelegation::between(Name::root(), tld_name, apex_name, txt)
    }

    fn between(root: Name, tld: Name, apex: Name, txt: Vec<String>) -> SimDelegation {
        SimDelegation {
            root: SimZone::for_name(root, Vec::new()),
            tld: SimZone::for_name(tld, Vec::new()),
            apex: SimZone::for_name(apex, txt),
        }
    }

    /// The chain an entry for this apex carries: the apex's declaration, its
    /// DNSKEY set and DS, then the TLD's own DNSKEY and DS, then the root's
    /// DNSKEY — bottom first, root last, exactly as [`crate::chain`] walks it.
    pub fn chain(&self) -> DnssecChain {
        self.chain_at(time::OffsetDateTime::now_utc() - time::Duration::hours(1))
    }

    /// The same, with every RRSIG's inception moved.
    pub fn chain_at(&self, inception: time::OffsetDateTime) -> DnssecChain {
        let link = |zone: &SimZone, records: Vec<Record>| ChainLink {
            zone: zone.apex(),
            rrs: chain::encode_rrs(&records).expect("encode chain link"),
        };
        // The apex link carries its own DNSKEY RRset beside the DS: the walk
        // proves the RRset — the authorized key set — and the DS need only
        // cover the key that signed it, never every key in it.
        let mut apex_records = self.apex.dnskey_records(inception);
        apex_records.extend(self.tld.ds_records_for(&self.apex, inception));
        let mut tld_records = self.tld.dnskey_records(inception);
        tld_records.extend(self.root.ds_records_for(&self.tld, inception));
        DnssecChain {
            links: vec![
                ChainLink {
                    zone: self.apex.transparency_name().to_string(),
                    rrs: chain::encode_rrs(&self.apex.declaration_records(inception))
                        .expect("encode declaration link"),
                },
                link(&self.apex, apex_records),
                link(&self.tld, tld_records),
                link(&self.root, self.root.dnskey_records(inception)),
            ],
        }
    }

    /// The same ladder with the apex's key set **replaced** by `impostor`'s,
    /// leaving the parent's real DS RRset in place.
    ///
    /// Everywhere else the DS and the key set it covers are derived from the
    /// same key, so this shape exercises the DS→DNSKEY binding directly.
    ///
    /// What it produces is exactly the forgery the ladder exists to refuse: a
    /// delegation ladder is *public data*, so anyone can fetch a victim's real
    /// DS, its parent's DNSKEY set and the root's, and stand a key set of
    /// their own at the victim's apex name. `impostor` must therefore be a
    /// zone at the **same name** as the apex — a different key at the victim's
    /// name is what an attacker has — and its declaration travels with it,
    /// because the walk verifies the declaration under whatever set it proved.
    ///
    /// Everything else in the chain is genuine. Only the digest comparison
    /// stands between it and a validating chain for an attacker's keys.
    pub fn chain_with_substituted_apex_keys(&self, impostor: &SimZone) -> DnssecChain {
        assert_eq!(
            impostor.origin, self.apex.origin,
            "the substituted key set has to sit at the apex's own name, or the \
             link's owner-name rule refuses it before the digest is reached"
        );
        let inception = time::OffsetDateTime::now_utc() - time::Duration::hours(1);
        let link = |zone: &SimZone, records: Vec<Record>| ChainLink {
            zone: zone.apex(),
            rrs: chain::encode_rrs(&records).expect("encode chain link"),
        };
        // The impostor's key set, and the *real* parent DS beside it.
        let mut apex_records = impostor.dnskey_records(inception);
        apex_records.extend(self.tld.ds_records_for(&self.apex, inception));
        let mut tld_records = self.tld.dnskey_records(inception);
        tld_records.extend(self.root.ds_records_for(&self.tld, inception));
        DnssecChain {
            links: vec![
                ChainLink {
                    zone: impostor.transparency_name().to_string(),
                    rrs: chain::encode_rrs(&impostor.declaration_records(inception))
                        .expect("encode declaration link"),
                },
                link(&self.apex, apex_records),
                link(&self.tld, tld_records),
                link(&self.root, self.root.dnskey_records(inception)),
            ],
        }
    }

    /// The same ladder with the apex delegated by a **SHA-384** DS.
    ///
    /// RFC 4509 digest type 4 is an ordinary delegation a registrar may
    /// publish, and `chain::covers` dispatches on the type.
    pub fn chain_with_sha384_ds(&self) -> DnssecChain {
        let inception = time::OffsetDateTime::now_utc() - time::Duration::hours(1);
        let link = |zone: &SimZone, records: Vec<Record>| ChainLink {
            zone: zone.apex(),
            rrs: chain::encode_rrs(&records).expect("encode chain link"),
        };
        let mut apex_records = self.apex.dnskey_records(inception);
        apex_records.extend(self.tld.ds_records_sha384_for(&self.apex, inception));
        let mut tld_records = self.tld.dnskey_records(inception);
        tld_records.extend(self.root.ds_records_for(&self.tld, inception));
        DnssecChain {
            links: vec![
                ChainLink {
                    zone: self.apex.transparency_name().to_string(),
                    rrs: chain::encode_rrs(&self.apex.declaration_records(inception))
                        .expect("encode declaration link"),
                },
                link(&self.apex, apex_records),
                link(&self.tld, tld_records),
                link(&self.root, self.root.dnskey_records(inception)),
            ],
        }
    }

    /// The same ladder, with the apex's DS RRset signed by a key of the TLD's
    /// that the standards forbid from signing anything.
    ///
    /// `verify_ds_set` hands `verify_rrset` the parent's **whole** DNSKEY
    /// RRset, unfiltered, so the flag check inside `verify_rrset` is the only
    /// thing that stops a revoked key from authorizing a delegation. `extra`
    /// is served in the TLD's RRset (so it validates as part of it) and its
    /// signer makes the DS.
    pub fn chain_with_ds_signed_by(&mut self, extra: DNSKEY, signer: &DnssecSigner) -> DnssecChain {
        let inception = time::OffsetDateTime::now_utc() - time::Duration::hours(1);
        let link = |zone: &SimZone, records: Vec<Record>| ChainLink {
            zone: zone.apex(),
            rrs: chain::encode_rrs(&records).expect("encode chain link"),
        };
        // Served in the TLD's own RRset, so it validates as a member of the
        // set the root's DS proves — the position a revoked key really sits in.
        self.tld.add_dnskey(extra);

        let mut apex_records = self.apex.dnskey_records(inception);
        apex_records.extend(self.tld.ds_records_signed_by(&self.apex, inception, signer));
        let mut tld_records = self.tld.dnskey_records(inception);
        tld_records.extend(self.root.ds_records_for(&self.tld, inception));
        DnssecChain {
            links: vec![
                ChainLink {
                    zone: self.apex.transparency_name().to_string(),
                    rrs: chain::encode_rrs(&self.apex.declaration_records(inception))
                        .expect("encode declaration link"),
                },
                link(&self.apex, apex_records),
                link(&self.tld, tld_records),
                link(&self.root, self.root.dnskey_records(inception)),
            ],
        }
    }

    /// The trust-anchor line a reader of this ladder installs: the *root's*
    /// key, not the apex's. This is the shape a real ICANN-rooted deployment
    /// has, and the one the self-anchored sim chain cannot express.
    pub fn anchor_record(&self) -> String {
        self.root.anchor_record()
    }

    /// The certificate an entry for this apex carries, with the ladder as its
    /// chain extension.
    pub fn certificate(&self) -> Vec<u8> {
        self.apex
            .certificate(&[(OID_DNSSEC_CHAIN.to_vec(), self.chain().encode())])
    }
}

/// A deterministic in-memory transparency log (§9).
///
/// Entries go in, checkpoints and audit paths come out, in exactly the
/// formats [`crate::rekor`] parses — an RFC 6962 tree over
/// `SHA-256(0x00 || entry)` leaves, and a signed note carrying the origin,
/// the tree size and the root. Nothing here is a Rekor implementation; it is
/// the smallest thing that can produce a *correct* proof, so that a test can
/// then produce exactly one incorrect one.
#[doc(hidden)]
#[allow(missing_debug_implementations)]
pub struct SimLog {
    origin: String,
    pkcs8: Vec<u8>,
    spki: Vec<u8>,
    leaves: Vec<[u8; 32]>,
}

impl SimLog {
    /// The name this log signs its checkpoints as, and the host a trusted
    /// root names it at.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// A log named `origin` with a fresh key. The key is fixed for the life
    /// of the log, so its id, its PEM and every checkpoint it signs are one
    /// coherent universe a test can pin.
    ///
    /// **Signs ASN.1/DER, because Sigstore does.** A simulator that agrees with
    /// the implementation rather than with the world tests nothing: signing the
    /// fixed 64-byte `r ‖ s` form would produce exactly the bytes a
    /// fixed-width-only verifier wants, keeping the whole P-256 path green while
    /// it stayed unusable against the real log.
    pub fn new(origin: &str) -> SimLog {
        let rng = aws_lc_rs::rand::SystemRandom::new();
        let pkcs8 = aws_lc_rs::signature::EcdsaKeyPair::generate_pkcs8(
            &aws_lc_rs::signature::ECDSA_P256_SHA256_ASN1_SIGNING,
            &rng,
        )
        .expect("keygen")
        .as_ref()
        .to_vec();
        let key = aws_lc_rs::signature::EcdsaKeyPair::from_pkcs8(
            &aws_lc_rs::signature::ECDSA_P256_SHA256_ASN1_SIGNING,
            &pkcs8,
        )
        .expect("key load");
        // The public key comes back as an uncompressed point; the SPKI and
        // DNSSEC both want it without the 0x04 tag.
        let point = aws_lc_rs::signature::KeyPair::public_key(&key).as_ref()[1..].to_vec();
        SimLog {
            origin: origin.to_string(),
            pkcs8,
            spki: p256_spki(&point),
            leaves: Vec::new(),
        }
    }

    /// The pinned-key file this log's clients would carry.
    pub fn key_pem(&self) -> String {
        let mut pem = String::from("-----BEGIN PUBLIC KEY-----\n");
        let body = rekor::base64_encode(&self.spki);
        for line in body.as_bytes().chunks(64) {
            pem.push_str(&String::from_utf8_lossy(line));
            pem.push('\n');
        }
        pem.push_str("-----END PUBLIC KEY-----\n");
        pem
    }

    /// The log's id: SHA-256 over its DER SubjectPublicKeyInfo.
    pub fn log_id(&self) -> [u8; 32] {
        rekor::sha256(&self.spki)
    }

    /// The log's DER SubjectPublicKeyInfo — what a `trusted_root.json`
    /// carries as a tlog's `publicKey.rawBytes`.
    pub fn spki(&self) -> Vec<u8> {
        self.spki.clone()
    }

    /// Appends one entry, returning its index.
    pub fn append(&mut self, entry: &[u8]) -> u64 {
        self.leaves.push(rekor::leaf_hash(entry));
        self.leaves.len() as u64 - 1
    }

    /// The current Merkle root.
    pub fn root(&self) -> [u8; 32] {
        merkle_root(&self.leaves)
    }

    /// The audit path from one entry to the current root.
    pub fn inclusion_path(&self, index: u64) -> Vec<[u8; 32]> {
        audit_path(index as usize, &self.leaves)
    }

    /// A signed note over the current tree.
    pub fn checkpoint(&self) -> Vec<u8> {
        self.note(self.leaves.len() as u64, self.root())
    }

    /// A signed note over any tree — how a test builds a checkpoint that is
    /// perfectly well signed and describes the wrong tree, or one signed by
    /// the wrong log entirely.
    pub fn note(&self, tree_size: u64, root: [u8; 32]) -> Vec<u8> {
        let body = format!(
            "{}\n{tree_size}\n{}\n",
            self.origin,
            rekor::base64_encode(&root)
        );
        // **ASN.1/DER, because that is what Sigstore signs notes with** —
        // the live `rekor.sigstore.dev` signature is 70 bytes opening
        // `30 44 02 20`. Signing the raw `r ‖ s` form here would agree with a
        // fixed-width-only verifier rather than with the log.
        let signature = sign_p256_der(&self.pkcs8, body.as_bytes());
        // The four-byte key hint is a selector, never a credential; the
        // verifier tries the pinned key regardless of what it says.
        let mut blob = self.log_id()[..4].to_vec();
        blob.extend_from_slice(&signature);
        format!(
            "{body}\n\u{2014} {} {}\n",
            self.origin,
            rekor::base64_encode(&blob)
        )
        .into_bytes()
    }

    /// Logs a Statement the zone key signs, and returns the whole proof —
    /// what `controlplane rekor-publish` ends up storing. The entry is a real
    /// `hashedrekord` v0.0.2 body over the Statement's DSSE PAE, with the
    /// zone's apex-naming certificate as its verifier.
    pub fn log_statement(&mut self, zone: &SimZone, statement: &ZoneKeyStatement) -> RekorProof {
        self.log_certified(zone, statement, &zone.zone_key_certificate())
    }

    /// The same, with a certificate the caller chose — how a test logs an
    /// entry whose chain is missing, broken, or about somebody else's key.
    pub fn log_certified(
        &mut self,
        zone: &SimZone,
        statement: &ZoneKeyStatement,
        certificate: &[u8],
    ) -> RekorProof {
        let payload = statement.to_json();
        let signature = zone.sign_dsse(&payload);
        let body = hashedrekord_body(&payload, &signature, certificate);
        self.log_body(payload, body)
    }

    /// Appends a prebuilt entry body and returns the whole proof. The
    /// primitive `log_statement` builds on, and the seam a test uses to log a
    /// body that is well formed but wrong — a signature by a stranger, a
    /// verifier that is not the signer's key — one flaw at a time.
    pub fn log_body(&mut self, statement: Vec<u8>, body: Vec<u8>) -> RekorProof {
        let mut proof = RekorProof {
            log_id: self.log_id(),
            log_index: 0,
            statement,
            canonicalized_body: body,
            checkpoint: Vec::new(),
            inclusion_path: Vec::new(),
        };
        proof.log_index = self.append(&proof.canonicalized_body);
        proof.checkpoint = self.checkpoint();
        proof.inclusion_path = self.inclusion_path(proof.log_index);
        proof
    }

    /// Logs a zone's current key set with the standard Statement.
    pub fn publish(&mut self, zone: &SimZone, action: &str) -> RekorProof {
        let statement = zone.zone_key_statement(action);
        self.log_statement(zone, &statement)
    }

    /// Re-issues the checkpoint and audit path for an already-logged entry,
    /// which is what the control plane's refresh does once the tree grows.
    pub fn refresh(&self, proof: &mut RekorProof) {
        proof.checkpoint = self.checkpoint();
        proof.inclusion_path = self.inclusion_path(proof.log_index);
    }
}

/// A synthetic TUF repository with its own root keys (§10.5).
///
/// The real chain is checked in as a conformance fixture, because canonical
/// JSON has to be right against the bytes a real repository serves. This is
/// for everything the real repository cannot be made to do on demand: root
/// rotation across several versions, a threshold that is not met, an expired
/// timestamp, a rolled-back version, a tampered target, and a trusted root
/// that *drops* a log key — revocation reaching the pin set, which is the
/// half of §10 that a fixture can never demonstrate.
///
/// Roles are signed the way Sigstore signs them: the root role with ECDSA
/// P-256 and DER signatures, the online roles — timestamp, snapshot, targets
/// — with a single Ed25519 key, so both schemes and both signature
/// encodings are exercised by everything this produces.
#[doc(hidden)]
#[allow(missing_debug_implementations)]
pub struct SimTuf {
    /// The root-role keys, and how many of them must sign.
    root_keys: Vec<SimTufKey>,
    root_threshold: u64,
    /// The one key the timestamp, snapshot and targets roles share.
    online: SimTufKey,
    /// Every `root.json` this repository has published, ascending.
    roots: Vec<Vec<u8>>,
    timestamp_version: u64,
    snapshot_version: u64,
    targets_version: u64,
    trusted_root: Vec<u8>,
    /// When the online roles expire, seconds since the epoch.
    pub expires: i64,
    /// When the root role expires, seconds since the epoch.
    pub root_expires: i64,
}

impl SimTuf {
    /// A repository at root version 1 whose `trusted_root.json` names
    /// `tlogs` — each a DER SubjectPublicKeyInfo, as [`SimLog::spki`]
    /// produces.
    ///
    /// `now` is the moment the caller intends to verify at; everything
    /// expires a year later, and every test that wants an expiry failure
    /// says so by moving one of the two `expires` fields.
    pub fn new(now: i64, tlogs: &[&SimLog]) -> SimTuf {
        let mut repo = SimTuf {
            root_keys: (0..3).map(|_| SimTufKey::p256()).collect(),
            root_threshold: 2,
            online: SimTufKey::ed25519(),
            roots: Vec::new(),
            timestamp_version: 1,
            snapshot_version: 1,
            targets_version: 1,
            trusted_root: Vec::new(),
            expires: now + 365 * 86_400,
            root_expires: now + 365 * 86_400,
        };
        repo.set_tlogs(tlogs);
        repo.publish_root(None);
        repo
    }

    /// The `root.json` a client of this repository would embed: version 1.
    pub fn embedded_root(&self) -> Vec<u8> {
        self.roots.first().cloned().expect("root version 1")
    }

    /// A [`crate::tuf::PinState`] anchored at that embedded root and nothing
    /// else — a fresh install.
    pub fn embedded_state(&self) -> crate::tuf::PinState {
        crate::tuf::PinState::anchored(&self.embedded_root())
    }

    /// The current root version.
    pub fn root_version(&self) -> u64 {
        self.roots.len() as u64
    }

    /// Replaces the logs the `trusted_root.json` target names, bumping
    /// targets, snapshot and timestamp the way a repository publish does.
    ///
    /// Passing fewer keys than last time is how a test asks the question
    /// §10.2 exists to answer: does a key Sigstore removes actually leave
    /// the client's pin set?
    /// **Each shard is named at its own URL.** A trusted root's `baseUrl` is
    /// where a shard is served, and the origin its checkpoints carry is that
    /// URL's host — the pairing `tuf::tlog_keys` carries into the pin set so a
    /// key vouches for its own tree rather than for any tree a pinned key
    /// signed. A harness that gave every simulated shard one URL could not
    /// express a pin bound to the wrong log, which is the shape that check
    /// exists for.
    pub fn set_tlogs(&mut self, tlogs: &[&SimLog]) {
        self.trusted_root = serde_json::json!({
            "mediaType": "application/vnd.dev.sigstore.trustedroot+json;version=0.1",
            "tlogs": tlogs
                .iter()
                .map(|log| {
                    serde_json::json!({
                        "baseUrl": format!("https://{}", log.origin()),
                        "hashAlgorithm": "SHA2_256",
                        "publicKey": { "rawBytes": rekor::base64_encode(&log.spki()) },
                    })
                })
                .collect::<Vec<_>>(),
        })
        .to_string()
        .into_bytes();
        self.targets_version += 1;
        self.snapshot_version += 1;
        self.timestamp_version += 1;
    }

    /// Publishes a new root version, signed by the old root's keys and its
    /// own. `rekey` replaces the root-role key set, which is the rotation
    /// the whole chain walk exists for.
    pub fn rotate_root(&mut self, rekey: bool) {
        let previous = rekey.then(|| self.root_keys.clone());
        if rekey {
            self.root_keys = (0..3).map(|_| SimTufKey::p256()).collect();
        }
        self.publish_root(previous.as_deref());
    }

    /// What a walk of this repository collects: every root, ascending, then
    /// the four files the chain authenticates.
    pub fn metadata(&self) -> TufMetadata {
        self.metadata_from(1)
    }

    /// The same, with the roots below `first` withheld — a mirror serving a
    /// chain a client embedded lower down cannot reach.
    pub fn metadata_from(&self, first: u64) -> TufMetadata {
        TufMetadata {
            roots: self
                .roots
                .iter()
                .skip(first.saturating_sub(1) as usize)
                .cloned()
                .collect(),
            timestamp: self.timestamp(),
            snapshot: self.snapshot(),
            targets: self.targets(),
            trusted_root: self.trusted_root.clone(),
        }
    }

    fn publish_root(&mut self, previous: Option<&[SimTufKey]>) {
        let version = self.roots.len() as u64 + 1;
        let mut keys = serde_json::Map::new();
        for key in self.root_keys.iter().chain([&self.online]) {
            keys.insert(key.id(), key.to_json());
        }
        let root_ids: Vec<String> = self.root_keys.iter().map(SimTufKey::id).collect();
        let online = vec![self.online.id()];
        let signed = serde_json::json!({
            "_type": "root",
            "spec_version": "1.0.31",
            "version": version,
            "expires": rfc3339(self.root_expires),
            "consistent_snapshot": true,
            "keys": keys,
            "roles": {
                "root": { "keyids": root_ids, "threshold": self.root_threshold },
                "timestamp": { "keyids": online.clone(), "threshold": 1 },
                "snapshot": { "keyids": online.clone(), "threshold": 1 },
                "targets": { "keyids": online, "threshold": 1 },
            },
        });
        // Both roots sign: the old one says who may succeed it, the new one
        // proves it holds the keys it claims.
        let signers = self.root_keys.iter().chain(previous.unwrap_or(&[]));
        self.roots.push(sign_metadata(&signed, signers));
    }

    fn timestamp(&self) -> Vec<u8> {
        let snapshot = self.snapshot();
        let signed = serde_json::json!({
            "_type": "timestamp",
            "spec_version": "1.0.31",
            "version": self.timestamp_version,
            "expires": rfc3339(self.expires),
            "meta": {
                "snapshot.json": {
                    "version": self.snapshot_version,
                    "length": snapshot.len(),
                    "hashes": { "sha256": hex::encode(rekor::sha256(&snapshot)) },
                },
            },
        });
        sign_metadata(&signed, [&self.online])
    }

    fn snapshot(&self) -> Vec<u8> {
        let targets = self.targets();
        let signed = serde_json::json!({
            "_type": "snapshot",
            "spec_version": "1.0.31",
            "version": self.snapshot_version,
            "expires": rfc3339(self.expires),
            "meta": {
                // Sigstore's own snapshot lists targets.json by version
                // alone; the hashes here make the tampered-target case
                // reachable at this level too.
                "targets.json": {
                    "version": self.targets_version,
                    "length": targets.len(),
                    "hashes": { "sha256": hex::encode(rekor::sha256(&targets)) },
                },
            },
        });
        sign_metadata(&signed, [&self.online])
    }

    fn targets(&self) -> Vec<u8> {
        let signed = serde_json::json!({
            "_type": "targets",
            "spec_version": "1.0.31",
            "version": self.targets_version,
            "expires": rfc3339(self.expires),
            "targets": {
                tuf::TRUSTED_ROOT_TARGET: {
                    "length": self.trusted_root.len(),
                    "hashes": { "sha256": hex::encode(rekor::sha256(&self.trusted_root)) },
                },
            },
        });
        sign_metadata(&signed, [&self.online])
    }
}

/// Served the way a real TUF repository serves it: by consistent-snapshot
/// path, so a walk that resolved the wrong version — or read a digest out of
/// the wrong field — finds nothing here rather than quietly assembling
/// something that happens to verify.
impl tuf::Repo for SimTuf {
    fn get(&self, path: &str) -> Result<Option<Vec<u8>>, String> {
        if let Some(version) = path
            .strip_suffix(".root.json")
            .and_then(|v| v.parse::<u64>().ok())
        {
            return Ok(self.roots.get(version.saturating_sub(1) as usize).cloned());
        }
        Ok(match path {
            "timestamp.json" => Some(self.timestamp()),
            _ if path == format!("{}.snapshot.json", self.snapshot_version) => {
                Some(self.snapshot())
            }
            _ if path == format!("{}.targets.json", self.targets_version) => Some(self.targets()),
            _ if path
                == format!(
                    "targets/{}.{}",
                    hex::encode(rekor::sha256(&self.trusted_root)),
                    tuf::TRUSTED_ROOT_TARGET
                ) =>
            {
                Some(self.trusted_root.clone())
            }
            // Every other path is a file this repository does not have,
            // which is how the root walk learns where to stop.
            _ => None,
        })
    }
}

/// One signing key of a synthetic repository.
#[derive(Clone)]
struct SimTufKey {
    pkcs8: Vec<u8>,
    spki: Vec<u8>,
    scheme: &'static str,
}

impl SimTufKey {
    fn p256() -> SimTufKey {
        let rng = aws_lc_rs::rand::SystemRandom::new();
        let pkcs8 = aws_lc_rs::signature::EcdsaKeyPair::generate_pkcs8(
            &aws_lc_rs::signature::ECDSA_P256_SHA256_ASN1_SIGNING,
            &rng,
        )
        .expect("keygen")
        .as_ref()
        .to_vec();
        let key = aws_lc_rs::signature::EcdsaKeyPair::from_pkcs8(
            &aws_lc_rs::signature::ECDSA_P256_SHA256_ASN1_SIGNING,
            &pkcs8,
        )
        .expect("key load");
        let point = aws_lc_rs::signature::KeyPair::public_key(&key).as_ref()[1..].to_vec();
        SimTufKey {
            pkcs8,
            spki: p256_spki(&point),
            scheme: "ecdsa-sha2-nistp256",
        }
    }

    fn ed25519() -> SimTufKey {
        let rng = aws_lc_rs::rand::SystemRandom::new();
        let pkcs8 = aws_lc_rs::signature::Ed25519KeyPair::generate_pkcs8(&rng)
            .expect("keygen")
            .as_ref()
            .to_vec();
        let key = aws_lc_rs::signature::Ed25519KeyPair::from_pkcs8(&pkcs8).expect("key load");
        // The same prefix `LogKey::from_spki` strips back off, named once
        // there rather than spelled out again here.
        let mut spki = crate::pubkey::ED25519_SPKI_PREFIX.to_vec();
        spki.extend_from_slice(aws_lc_rs::signature::KeyPair::public_key(&key).as_ref());
        SimTufKey {
            pkcs8,
            spki,
            scheme: "ed25519",
        }
    }

    /// The key object a root's key table holds.
    fn to_json(&self) -> serde_json::Value {
        let mut pem = String::from("-----BEGIN PUBLIC KEY-----\n");
        pem.push_str(&rekor::base64_encode(&self.spki));
        pem.push_str("\n-----END PUBLIC KEY-----\n");
        serde_json::json!({
            "keytype": match self.scheme {
                "ed25519" => "ed25519",
                _ => "ecdsa",
            },
            "scheme": self.scheme,
            "keyval": { "public": pem },
        })
    }

    /// The TUF key id: SHA-256 over the canonical JSON of the key object.
    fn id(&self) -> String {
        crate::tuf::key_id(&self.to_json()).expect("key id")
    }

    fn sign(&self, message: &[u8]) -> Vec<u8> {
        let rng = aws_lc_rs::rand::SystemRandom::new();
        match self.scheme {
            "ed25519" => aws_lc_rs::signature::Ed25519KeyPair::from_pkcs8(&self.pkcs8)
                .expect("key load")
                .sign(message)
                .as_ref()
                .to_vec(),
            _ => aws_lc_rs::signature::EcdsaKeyPair::from_pkcs8(
                &aws_lc_rs::signature::ECDSA_P256_SHA256_ASN1_SIGNING,
                &self.pkcs8,
            )
            .expect("key load")
            .sign(&rng, message)
            .expect("sign")
            .as_ref()
            .to_vec(),
        }
    }
}

/// Renders a `signed` object as a TUF metadata file, signed by each key.
///
/// The signatures cover the *canonical* JSON of `signed`, while the file
/// itself carries the serialization below — exactly the split that makes
/// canonical JSON load-bearing.
fn sign_metadata<'a>(
    signed: &serde_json::Value,
    keys: impl IntoIterator<Item = &'a SimTufKey>,
) -> Vec<u8> {
    let canonical = crate::tuf::canonical_json(signed).expect("canonical json");
    let signatures: Vec<serde_json::Value> = keys
        .into_iter()
        .map(|key| {
            serde_json::json!({
                "keyid": key.id(),
                "sig": hex::encode(key.sign(&canonical)),
            })
        })
        .collect();
    serde_json::json!({ "signatures": signatures, "signed": signed })
        .to_string()
        .into_bytes()
}

/// A unix timestamp as the RFC 3339 form TUF `expires` fields carry —
/// the inverse of the conversion `crate::tuf` uses to read these back.
fn rfc3339(seconds: i64) -> String {
    let (year, month, day, hour, minute, second) = synch_core::civil::civil_from_unix(seconds);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// RFC 6962 §2.1: the Merkle tree hash over a list of leaf hashes.
fn merkle_root(leaves: &[[u8; 32]]) -> [u8; 32] {
    match leaves {
        [] => rekor::sha256(&[]),
        [only] => *only,
        _ => {
            let split = split_point(leaves.len());
            rekor::node_hash(
                &merkle_root(&leaves[..split]),
                &merkle_root(&leaves[split..]),
            )
        }
    }
}

/// RFC 6962 §2.1.1: the audit path from one leaf to the root.
fn audit_path(index: usize, leaves: &[[u8; 32]]) -> Vec<[u8; 32]> {
    if leaves.len() <= 1 {
        return Vec::new();
    }
    let split = split_point(leaves.len());
    if index < split {
        let mut path = audit_path(index, &leaves[..split]);
        path.push(merkle_root(&leaves[split..]));
        path
    } else {
        let mut path = audit_path(index - split, &leaves[split..]);
        path.push(merkle_root(&leaves[..split]));
        path
    }
}

/// The largest power of two strictly less than `n` — RFC 6962's split.
fn split_point(n: usize) -> usize {
    let mut k = 1;
    while k * 2 < n {
        k *= 2;
    }
    k
}

/// A real `hashedrekord` v0.0.2 body over a Statement's DSSE PAE.
///
/// The field order is the live log's, byte for byte (`apiVersion`, `kind`,
/// `spec` → `hashedRekordV002` → `data{algorithm,digest}`,
/// `signature{content, verifier{keyDetails, x509Certificate{rawBytes}}}`),
/// because the Merkle leaf commits to these exact bytes — this is the shape
/// a genuine `log2025-1.rekor.sigstore.dev` entry has (see the conformance
/// fixture `tests/fixtures/rekor_v3`). `digest` is always the SHA-256 of the
/// PAE of `statement`, so a test that logs a mangled statement still produces
/// an internally consistent entry — the flaw it is probing surfaces at the
/// check it means to, not at a digest mismatch.
#[doc(hidden)]
pub fn hashedrekord_body(statement: &[u8], signature_der: &[u8], certificate: &[u8]) -> Vec<u8> {
    let digest = rekor::sha256(&rekor::pae(rekor::DSSE_PAYLOAD_TYPE, statement));
    let mut out = String::new();
    out.push_str("{\"apiVersion\":\"0.0.2\",\"kind\":\"hashedrekord\",\"spec\":{\"hashedRekordV002\":{\"data\":{\"algorithm\":\"SHA2_256\",\"digest\":\"");
    out.push_str(&rekor::base64_encode(&digest));
    out.push_str("\"},\"signature\":{\"content\":\"");
    out.push_str(&rekor::base64_encode(signature_der));
    out.push_str("\",\"verifier\":{\"keyDetails\":\"PKIX_ECDSA_P256_SHA_256\",\"x509Certificate\":{\"rawBytes\":\"");
    out.push_str(&rekor::base64_encode(certificate));
    out.push_str("\"}}}}}}");
    out.into_bytes()
}

/// Signs with ECDSA P-256/SHA-256, producing the ASN.1/DER form a Rekor
/// entry signature carries. The zone key's PKCS#8 was minted for FIXED
/// signing, but aws-lc-rs lets the same key material load under either
/// encoding.
fn sign_p256_der(pkcs8: &[u8], message: &[u8]) -> Vec<u8> {
    let rng = aws_lc_rs::rand::SystemRandom::new();
    let key = aws_lc_rs::signature::EcdsaKeyPair::from_pkcs8(
        &aws_lc_rs::signature::ECDSA_P256_SHA256_ASN1_SIGNING,
        pkcs8,
    )
    .expect("key load");
    key.sign(&rng, message).expect("sign").as_ref().to_vec()
}

/// A DER SubjectPublicKeyInfo around a raw uncompressed P-256 point.
///
/// The real one. A private copy of the same 27 bytes lived here, which is
/// exactly the shape that drifts: the harness mints the keys the parser is
/// then asserted against, so a divergence between the two would make the
/// suite agree with itself about a format neither one gets right.
fn p256_spki(point: &[u8]) -> Vec<u8> {
    crate::rekor::p256_spki(point)
}

/// The canonical wire form of a name: lowercase labels, length-prefixed,
/// terminated by the root.
fn name_wire(name: &Name) -> Vec<u8> {
    let mut out = Vec::new();
    for label in name.to_lowercase().iter() {
        out.push(label.len() as u8);
        out.extend_from_slice(label);
    }
    out.push(0);
    out
}

/// Standard base64 with padding — enough for one DNSKEY, not a dependency.
fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from_be_bytes([0, b[0], b[1], b[2]]);
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}
