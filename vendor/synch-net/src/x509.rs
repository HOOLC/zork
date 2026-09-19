//! Just enough X.509 to put a zone name inside a Merkle leaf.
//!
//! Rekor v2's `Verifier` is a protobuf oneof — a raw public key or an X.509
//! certificate — and Rekor performs **no** certificate validation whatsoever
//! (`pkg/verifier/certificate`: parse, take `cert.PublicKey`, stop; no chain
//! building, no Fulcio root, no expiry, no CA policy). The whole `Signature`
//! message, certificate DER included, is copied verbatim into the
//! canonicalized body the leaf commits to. A self-signed certificate carrying
//! the apex as a `dNSName` SAN therefore writes the zone name, in the clear,
//! into the log's own Merkle leaf — which is the only way this design gets a
//! *monitorable name* out of a log that has exactly one entry type and no
//! room for a payload (docs/REKOR-ZONE-KEY.md §2).
//!
//! So the certificate here is a **key envelope, not a trust assertion**:
//! nothing validates its signature, issuer or validity window; it exists to
//! carry three things — the SubjectPublicKeyInfo, the apex, and one custom
//! extension (see [`crate::zonecert`]) — through a field Rekor serializes
//! verbatim.
//!
//! This module is deliberately a *narrow* DER reader rather than a general
//! X.509 stack: it extracts a SPKI, the `dNSName` SANs and an extension by
//! OID. A general parser would be a much larger attack surface for bytes an
//! attacker chooses, and every field this design turns on is one of those
//! three. The writer beside it is harness-only and is not compiled into a
//! shipped client at all — see the building section below.

use std::fmt;

use hickory_resolver::proto::rr::Name;

/// Why a certificate could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("certificate: {0}")]
pub struct X509Error(String);

impl X509Error {
    fn new(why: impl fmt::Display) -> X509Error {
        X509Error(why.to_string())
    }
}

impl From<&str> for X509Error {
    fn from(why: &str) -> X509Error {
        X509Error(why.to_string())
    }
}

// ------------------------------------------------------------------- OIDs

/// `id-ce-basicConstraints` (2.5.29.19).
#[cfg(any(test, feature = "sim"))]
pub(crate) const OID_BASIC_CONSTRAINTS: &[u8] = &[0x55, 0x1d, 0x13];
/// `id-ce-keyUsage` (2.5.29.15).
#[cfg(any(test, feature = "sim"))]
pub(crate) const OID_KEY_USAGE: &[u8] = &[0x55, 0x1d, 0x0f];
/// `id-ce-subjectAltName` (2.5.29.17).
pub(crate) const OID_SUBJECT_ALT_NAME: &[u8] = &[0x55, 0x1d, 0x11];
/// `id-at-commonName` (2.5.4.3).
#[cfg(any(test, feature = "sim"))]
pub(crate) const OID_COMMON_NAME: &[u8] = &[0x55, 0x04, 0x03];
/// `ecdsa-with-SHA256` (1.2.840.10045.4.3.2).
#[cfg(any(test, feature = "sim"))]
pub(crate) const OID_ECDSA_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];

// ---------------------------------------------------------------- parsing

/// One extension, as it sits in the certificate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extension {
    /// The OID's DER *content* bytes — the value after tag and length, which
    /// is how every OID constant in this file is written.
    pub oid: Vec<u8>,
    /// Whether a consumer that does not understand it must reject the
    /// certificate. Nothing in this design does; recorded for completeness.
    pub critical: bool,
    /// The `extnValue` OCTET STRING's contents.
    pub value: Vec<u8>,
}

/// The parts of a certificate this design turns on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Certificate {
    /// The DER SubjectPublicKeyInfo, byte-exact: the key the log vouches for.
    pub spki: Vec<u8>,
    /// Every `dNSName` in the subjectAltName extension, in order.
    pub dns_names: Vec<String>,
    /// Every extension, in order.
    pub extensions: Vec<Extension>,
}

impl Certificate {
    /// Parses a DER `Certificate`.
    ///
    /// Only the fields below are read; everything else is skipped by
    /// structure. In particular the signature is *not* checked — a
    /// self-signature over a key envelope proves nothing the SPKI does not
    /// already prove, and Rekor did not check it either.
    pub fn parse(der: &[u8]) -> Result<Certificate, X509Error> {
        // Trailing bytes after the certificate are refused. They are a
        // second encoding of the same value — the thing a verifier must
        // never accept — and they are exactly how the bytes in a public log
        // come to mean one thing here and another to an auditor reading the
        // same leaf with a stricter parser (Go's rejects them).
        let mut whole = Der::new(der);
        let mut outer = whole.sequence("Certificate")?;
        if !whole.is_empty() {
            return Err(X509Error::new("bytes after the certificate"));
        }
        let mut tbs = outer.sequence("tbsCertificate")?;
        // [0] EXPLICIT Version — optional, and always present in a v3
        // certificate, which is the only kind that can carry extensions.
        let _ = tbs.optional(0xa0);
        tbs.any("serialNumber")?;
        tbs.sequence("signature")?;
        tbs.any("issuer")?;
        tbs.any("validity")?;
        tbs.any("subject")?;
        let spki = tbs.raw_sequence("subjectPublicKeyInfo")?.to_vec();

        // issuerUniqueID [1], subjectUniqueID [2], extensions [3] — all
        // optional, and only the last matters here.
        let _ = tbs.optional(0x81);
        let _ = tbs.optional(0x82);
        let mut extensions = Vec::new();
        if let Some(bytes) = tbs.optional(0xa3) {
            // The `[3]` wrapper holds one `Extensions` SEQUENCE and nothing
            // else. Reading the first and discarding the remainder is how a
            // *second* SEQUENCE — with a second subjectAltName, or a second
            // chain extension — becomes invisible to this parser while sitting
            // inside the leaf, which defeats the exactly-one rules below
            // without touching them. Same rule as the trailing bytes after the
            // certificate, applied at every level rather than only the outer
            // one.
            let mut list = Der::new(bytes).only_sequence("the Extensions sequence")?;
            while !list.is_empty() {
                let mut ext = list.sequence("Extension")?;
                let oid = ext.tagged(0x06, "extnID")?.to_vec();
                let critical = match ext.optional(0x01) {
                    // DER says FALSE is absent, but a BOOLEAN that says so
                    // explicitly is still readable, and refusing it here
                    // would refuse a certificate over a spelling.
                    Some(value) => value.first().is_some_and(|b| *b != 0),
                    None => false,
                };
                let value = ext.tagged(0x04, "extnValue")?.to_vec();
                ext.finish("an extension's extnValue")?;
                extensions.push(Extension {
                    oid,
                    critical,
                    value,
                });
            }
        }
        // Nothing may follow the extensions block inside tbsCertificate;
        // otherwise a second `[3]` TLV could evade the exactly-one rules below.
        tbs.finish("the tbsCertificate")?;

        // Exactly one subjectAltName, or none. RFC 5280 says an extension
        // appears at most once, and taking the *first* of two would let a
        // certificate mean one thing to this parser and another to any reader
        // that took the last — which, for the extension that carries the zone
        // name, is the whole game.
        let mut sans = extensions.iter().filter(|e| e.oid == OID_SUBJECT_ALT_NAME);
        let dns_names = match (sans.next(), sans.next()) {
            (Some(san), None) => parse_san(&san.value)?,
            (None, _) => Vec::new(),
            (Some(_), Some(_)) => {
                return Err(X509Error::new(
                    "the certificate carries more than one subjectAltName extension",
                ))
            }
        };
        // The two fields after `tbsCertificate`, and then nothing. Unused —
        // the self-signature proves nothing the SPKI does not, as above — but
        // read, because the "no second encoding of the same value" rule this
        // parser applies at every other level is not applied by skipping a
        // level. Without this, `SEQUENCE { tbsCertificate }` with no signature
        // at all, or one with a trailing member, parses here and is refused by
        // any auditor reading the same log entry with a stricter parser.
        outer.sequence("signatureAlgorithm")?;
        outer.any("signatureValue")?;
        outer.finish("the certificate")?;
        Ok(Certificate {
            spki,
            dns_names,
            extensions,
        })
    }

    /// The value of the extension with this OID, if the certificate has
    /// exactly one.
    ///
    /// **Exactly one, for the same reason `subjectAltName` is:** returning
    /// the first of two would let a certificate mean one thing to this
    /// parser and another to any reader that took the last — and this
    /// extension carries the DNSSEC chain, the evidence that decides whether
    /// a monitor reports an entry or files it in the silent bin. Go's
    /// `crypto/x509` rejects duplicate extensions outright, so the public
    /// log would not have accepted such a certificate anyway — but that is
    /// somebody else's parser, not where this design keeps its invariants.
    pub fn extension(&self, oid: &[u8]) -> Option<&[u8]> {
        let mut matching = self.extensions.iter().filter(|e| e.oid == oid);
        match (matching.next(), matching.next()) {
            (Some(only), None) => Some(only.value.as_slice()),
            _ => None,
        }
    }

    /// The single `dNSName` a zone-key certificate must carry, **parsed**.
    ///
    /// Exactly one: a certificate with two names is a certificate that means
    /// two things to a monitor indexing by name, and this design has no use
    /// for that ambiguity.
    ///
    /// Parsed, and returned parsed, because a `dNSName` that is not a DNS
    /// name is not a SAN this design can act on — and because a raw string is
    /// how a client and a monitor come to disagree about whether
    /// `victim.example..` names the same zone as `victim.example.` (see
    /// [`crate::chain::authorize`]). A name that does not parse is refused
    /// here, once, for everybody.
    pub fn single_dns_name(&self) -> Result<Name, X509Error> {
        let text = match self.dns_names.as_slice() {
            [one] => one,
            names => {
                return Err(X509Error::new(format!(
                    "a zone-key certificate carries exactly one dNSName SAN, not {}",
                    names.len()
                )))
            }
        };
        // The SAN must be the name *spelled canonically*, not merely a string
        // that parses to it. This is the one field the whole certificate
        // exists to carry: the apex is written into the Merkle leaf so that
        // anyone reading the log can index it (docs/REKOR-ZONE-KEY.md §2.1).
        // `Name::from_utf8` reads DNS *presentation* format, where
        // `CLUSTER.EXAMPLE` and `clus\ter.example` are both
        // `cluster.example.` — so an attacker who has taken the delegation
        // can mint an entry this client accepts for `cluster.example` while
        // the leaf contains no such string, and every reader that indexes by
        // byte pattern — a `grep`, a CT-style indexer, an operator watching
        // for their own apex — misses it. Accepted and unfindable is the
        // exact shape §4.2.1 forbids: a client must enforce whatever property
        // makes an entry discoverable, or an attacker simply omits it.
        //
        // So the bytes must be the canonical presentation of the name they
        // decode to. A trailing dot is the one tolerated difference, because
        // it is the same name written absolute.
        let mut name = Name::from_utf8(text)
            .map_err(|e| X509Error::new(format!("the dNSName SAN {text:?} is not a name: {e}")))?;
        name.set_fqdn(true);
        if name.is_root() {
            // An empty SAN parses as the DNS root, which is a name but never
            // a zone this design mints a key for. Refusing it here keeps `""`
            // from becoming a value that compares equal to something.
            return Err(X509Error::new(
                "the dNSName SAN is the DNS root, which is not a zone this design logs",
            ));
        }
        let name = name.to_lowercase();
        let canonical = name.to_string();
        if text.as_str() != canonical.trim_end_matches('.') && text.as_str() != canonical {
            return Err(X509Error::new(format!(
                "the dNSName SAN is spelled {text:?}, not {:?} — a log entry has to \
                 carry its apex the way a reader indexing the log would search for it",
                canonical.trim_end_matches('.')
            )));
        }
        Ok(name)
    }
}

/// The `dNSName` entries of a `GeneralNames`.
///
/// Other name forms (rfc822, IP, URI, …) are skipped rather than refused:
/// they cannot be mistaken for a zone name, and refusing them would be a
/// parse rule with no security content.
fn parse_san(value: &[u8]) -> Result<Vec<String>, X509Error> {
    let mut names = Vec::new();
    // One GeneralNames sequence, with nothing after it — see the same rule in
    // `Certificate::parse`: a second sequence here is a second set of names
    // this parser would never see.
    let mut list = Der::new(value).only_sequence("the subjectAltName sequence")?;
    while !list.is_empty() {
        let (tag, body) = list.next("GeneralName")?;
        if tag == 0x82 {
            names.push(
                std::str::from_utf8(body)
                    .map_err(|_| X509Error::new("a dNSName is not UTF-8"))?
                    .to_string(),
            );
        }
    }
    Ok(names)
}

// ---------------------------------------------------------------- building
//
// **Everything below this line is the cross-validation encoder, not the
// production one, and it is gated out of a shipped client.** Certificates that
// reach the public log are built by `control-plane/src/cp_crypto_ffi.erl` with
// OTP's `public_key` — the ASN.1 module is the reference encoder, and a
// certificate an external tool cannot read would defeat the point of putting it
// in a public log. This half is reachable only from `crate::sim` and the test
// suites, so it sits behind the same `sim` feature they do: the asymmetry — a
// client that reads DER and never writes it — is then a fact about the binary
// rather than a claim in a comment.
//
// It earns its place by being a *second, independent* encoder: the shared
// fixtures under `control-plane/test/fixtures/rekor/crossval` are written by
// the Gleam side and read by the parser above, and this builder lets the
// tests mint certificate shapes the control plane will not produce on demand
// — a SAN that is not a name, two SAN extensions, a chain for the wrong zone.
// Two implementations of one DER format drift silently unless something
// outside both of them holds the bytes still; this is one of the two.

/// The one certificate shape this design mints.
///
/// Everything here is either load-bearing (the SPKI, the SAN, the custom
/// extension) or ceremony X.509 requires. The validity window is ceremony:
/// nothing on any verification path reads it, because nothing treats this
/// certificate as a trust assertion.
#[cfg(any(test, feature = "sim"))]
#[derive(Debug, Clone)]
pub struct SelfSigned<'a> {
    /// The subject and issuer common name — one string, because a
    /// self-signed certificate's issuer *is* its subject.
    pub common_name: &'a str,
    /// The zone apex, written as the single `dNSName` SAN. This is the whole
    /// reason the certificate exists.
    pub dns_name: &'a str,
    /// The DER SubjectPublicKeyInfo of the zone key.
    pub spki: &'a [u8],
    /// The serial number's big-endian magnitude bytes (positive; a leading
    /// zero is inserted if the high bit is set).
    pub serial: &'a [u8],
    /// `notBefore` and `notAfter` as `YYMMDDHHMMSSZ` / `YYYYMMDDHHMMSSZ`,
    /// already chosen by the caller (see [`x509_time`], which picks the
    /// encoding RFC 5280 requires for the year in question).
    pub not_before: Time,
    /// See `not_before`.
    pub not_after: Time,
    /// The custom extensions, `(OID content bytes, extnValue contents)`,
    /// emitted non-critical in the order given.
    pub extensions: &'a [(Vec<u8>, Vec<u8>)],
}

#[cfg(any(test, feature = "sim"))]
/// An X.509 time, in the encoding its year forces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Time {
    /// `UTCTime`, for years 1950–2049.
    Utc(String),
    /// `GeneralizedTime`, for everything else.
    Generalized(String),
}

#[cfg(any(test, feature = "sim"))]
impl Time {
    fn der(&self) -> Vec<u8> {
        match self {
            Time::Utc(text) => tlv(0x17, text.as_bytes()),
            Time::Generalized(text) => tlv(0x18, text.as_bytes()),
        }
    }
}

/// The X.509 time for a unix timestamp, in whichever encoding RFC 5280
/// mandates for its year.
#[cfg(any(test, feature = "sim"))]
pub fn x509_time(unix: i64) -> Time {
    let (year, month, day, hour, minute, second) = synch_core::civil::civil_from_unix(unix);
    match (1950..=2049).contains(&year) {
        true => Time::Utc(format!(
            "{:02}{month:02}{day:02}{hour:02}{minute:02}{second:02}Z",
            year % 100
        )),
        false => Time::Generalized(format!(
            "{year:04}{month:02}{day:02}{hour:02}{minute:02}{second:02}Z"
        )),
    }
}

#[cfg(any(test, feature = "sim"))]
impl SelfSigned<'_> {
    /// The `tbsCertificate` DER — the bytes the self-signature covers.
    pub(crate) fn tbs(&self) -> Vec<u8> {
        let mut body = Vec::new();
        // [0] EXPLICIT version = 2, i.e. v3: the version that has extensions.
        body.extend_from_slice(&tlv(0xa0, &tlv(0x02, &[0x02])));
        body.extend_from_slice(&integer(self.serial));
        body.extend_from_slice(&algorithm_identifier());
        let name = rdn_common_name(self.common_name);
        body.extend_from_slice(&name);
        let mut validity = self.not_before.der();
        validity.extend_from_slice(&self.not_after.der());
        body.extend_from_slice(&tlv(0x30, &validity));
        body.extend_from_slice(&name);
        body.extend_from_slice(self.spki);
        body.extend_from_slice(&tlv(0xa3, &tlv(0x30, &self.extension_der())));
        tlv(0x30, &body)
    }

    fn extension_der(&self) -> Vec<u8> {
        let mut out = Vec::new();
        // basicConstraints CA:FALSE, critical — an empty SEQUENCE is
        // `cA` defaulted to FALSE, which is the DER spelling of "not a CA".
        out.extend_from_slice(&extension(OID_BASIC_CONSTRAINTS, true, &tlv(0x30, &[])));
        // keyUsage digitalSignature, critical: one bit, seven unused.
        out.extend_from_slice(&extension(OID_KEY_USAGE, true, &tlv(0x03, &[0x07, 0x80])));
        // The apex. Non-critical, because criticality is a rule for
        // validators and nothing validates this certificate.
        let san = tlv(0x30, &tlv(0x82, self.dns_name.as_bytes()));
        out.extend_from_slice(&extension(OID_SUBJECT_ALT_NAME, false, &san));
        for (oid, value) in self.extensions {
            out.extend_from_slice(&extension(oid, false, value));
        }
        out
    }

    /// Builds the whole certificate, asking `sign` for an ECDSA-P256/SHA-256
    /// DER signature over the `tbsCertificate`.
    ///
    /// The signature is ceremony — no verifier in this system checks it — but
    /// a certificate whose self-signature does not verify is a certificate
    /// some *other* tool will reject while looking at our log entry, and
    /// making the log's contents inspectable is the entire point.
    pub fn build(&self, sign: impl FnOnce(&[u8]) -> Vec<u8>) -> Vec<u8> {
        let tbs = self.tbs();
        let signature = sign(&tbs);
        let mut body = tbs;
        body.extend_from_slice(&algorithm_identifier());
        // A BIT STRING with no unused bits, wrapping the DER signature.
        let mut bits = vec![0x00];
        bits.extend_from_slice(&signature);
        body.extend_from_slice(&tlv(0x03, &bits));
        tlv(0x30, &body)
    }
}

/// `AlgorithmIdentifier { ecdsa-with-SHA256 }` — no parameters, as RFC 5758
/// requires for the ECDSA-with-SHA2 family.
#[cfg(any(test, feature = "sim"))]
fn algorithm_identifier() -> Vec<u8> {
    tlv(0x30, &tlv(0x06, OID_ECDSA_SHA256))
}

/// `Name ::= RDNSequence` holding one `commonName`.
#[cfg(any(test, feature = "sim"))]
fn rdn_common_name(cn: &str) -> Vec<u8> {
    let attribute = tlv(
        0x30,
        &[tlv(0x06, OID_COMMON_NAME), tlv(0x0c, cn.as_bytes())].concat(),
    );
    tlv(0x30, &tlv(0x31, &attribute))
}

/// One `Extension`, with `critical` omitted when FALSE as DER requires.
#[cfg(any(test, feature = "sim"))]
fn extension(oid: &[u8], critical: bool, value: &[u8]) -> Vec<u8> {
    let mut body = tlv(0x06, oid);
    if critical {
        body.extend_from_slice(&tlv(0x01, &[0xff]));
    }
    body.extend_from_slice(&tlv(0x04, value));
    tlv(0x30, &body)
}

/// A positive `INTEGER` from big-endian magnitude bytes.
#[cfg(any(test, feature = "sim"))]
pub fn integer(magnitude: &[u8]) -> Vec<u8> {
    let trimmed = match magnitude.iter().position(|b| *b != 0) {
        Some(at) => &magnitude[at..],
        None => &[0][..],
    };
    let mut body = Vec::with_capacity(trimmed.len() + 1);
    if trimmed[0] & 0x80 != 0 {
        body.push(0x00);
    }
    body.extend_from_slice(trimmed);
    tlv(0x02, &body)
}

/// A DER tag-length-value.
#[cfg(any(test, feature = "sim"))]
pub(crate) fn tlv(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 6);
    out.push(tag);
    let len = body.len();
    if len < 0x80 {
        out.push(len as u8);
    } else {
        let bytes = len.to_be_bytes();
        let first = bytes
            .iter()
            .position(|b| *b != 0)
            .unwrap_or(bytes.len() - 1);
        out.push(0x80 | (bytes.len() - first) as u8);
        out.extend_from_slice(&bytes[first..]);
    }
    out.extend_from_slice(body);
    out
}

// ------------------------------------------------------------ DER reader

/// A bounds-checked DER reader over one definite-length sequence's contents.
///
/// Definite lengths only, no indefinite form, no re-entrant recursion beyond
/// what the callers above spell out by hand — the shapes this reads are all
/// known in advance.
#[derive(Debug)]
pub(crate) struct Der<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Der<'a> {
    /// A reader over raw DER bytes.
    pub(crate) fn new(bytes: &'a [u8]) -> Der<'a> {
        Der { bytes, at: 0 }
    }

    /// Whether every element has been read.
    pub(crate) fn is_empty(&self) -> bool {
        self.at >= self.bytes.len()
    }

    /// The next element, as `(tag, contents)`.
    pub(crate) fn next(&mut self, what: &str) -> Result<(u8, &'a [u8]), X509Error> {
        let bad = |why: &str| X509Error::new(format!("{what}: {why}"));
        let tag = *self.bytes.get(self.at).ok_or_else(|| bad("truncated"))?;
        let first = *self
            .bytes
            .get(self.at + 1)
            .ok_or_else(|| bad("truncated length"))?;
        let (len, header) = match first {
            n if n < 0x80 => (usize::from(n), 2),
            0x80 => return Err(bad("indefinite lengths are not DER")),
            n => {
                let count = usize::from(n & 0x7f);
                if count > 4 {
                    return Err(bad("length is absurdly long"));
                }
                let start = self.at + 2;
                let slice = self
                    .bytes
                    .get(start..start + count)
                    .ok_or_else(|| bad("truncated length"))?;
                if slice[0] == 0 {
                    return Err(bad("a non-minimal length is not DER"));
                }
                // DER requires the *shortest* encoding, so a value under 128
                // must use the short form. Rejecting only a leading zero
                // byte let `0x81 0x05` through, which is a second spelling
                // of a length that already had one — and two spellings of
                // one value is what a strict reader exists to refuse.
                if count == 1 && slice[0] < 0x80 {
                    return Err(bad("a long-form length under 128 is not DER"));
                }
                (
                    slice
                        .iter()
                        .fold(0usize, |acc, b| (acc << 8) | usize::from(*b)),
                    2 + count,
                )
            }
        };
        let start = self.at + header;
        let body = self
            .bytes
            .get(start..start + len)
            .ok_or_else(|| bad("element runs past the end"))?;
        self.at = start + len;
        Ok((tag, body))
    }

    /// The next element, which must carry `tag`.
    pub(crate) fn tagged(&mut self, tag: u8, what: &str) -> Result<&'a [u8], X509Error> {
        let at = self.at;
        let (actual, body) = self.next(what)?;
        if actual != tag {
            self.at = at;
            return Err(X509Error::new(format!(
                "{what}: expected tag 0x{tag:02x}, found 0x{actual:02x}"
            )));
        }
        Ok(body)
    }

    /// The next element if it carries `tag`, leaving the reader untouched
    /// otherwise — how the optional members of a certificate are read.
    pub(crate) fn optional(&mut self, tag: u8) -> Option<&'a [u8]> {
        let at = self.at;
        match self.next("optional") {
            Ok((actual, body)) if actual == tag => Some(body),
            _ => {
                self.at = at;
                None
            }
        }
    }

    /// A reader over the next element's contents, which must be a SEQUENCE.
    pub(crate) fn sequence(&mut self, what: &str) -> Result<Der<'a>, X509Error> {
        Ok(Der::new(self.tagged(0x30, what)?))
    }

    /// The SEQUENCE this reader holds, and **nothing after it**.
    ///
    /// For a wrapper defined to contain exactly one member: the `[3]` around
    /// `Extensions`, the OCTET STRING around `GeneralNames`. Reading the first
    /// element and dropping whatever follows is how a second copy — a second
    /// SAN, a second chain extension — sits inside a Merkle leaf where this
    /// parser cannot see it, defeating the exactly-one rules without touching
    /// them. Go's `crypto/x509` has the same laxity and will log such a
    /// certificate; OpenSSL refuses it outright. So readers disagree about
    /// those bytes, and this one refuses them.
    ///
    /// A combinator rather than an `is_empty()` at each call site, because the
    /// sites are what get forgotten: the rule belongs to the shape, and the
    /// next wrapper somebody adds gets it without knowing to ask.
    pub(crate) fn only_sequence(mut self, what: &str) -> Result<Der<'a>, X509Error> {
        let inner = self.sequence(what)?;
        self.finish(what)?;
        Ok(inner)
    }

    /// Asserts this reader is exhausted — every member accounted for.
    pub(crate) fn finish(&self, what: &str) -> Result<(), X509Error> {
        match self.is_empty() {
            true => Ok(()),
            false => Err(X509Error::new(format!("bytes after {what}"))),
        }
    }

    /// The next SEQUENCE including its own header — for members carried
    /// verbatim, like the SubjectPublicKeyInfo the key binding compares.
    pub(crate) fn raw_sequence(&mut self, what: &str) -> Result<&'a [u8], X509Error> {
        let start = self.at;
        self.tagged(0x30, what)?;
        Ok(&self.bytes[start..self.at])
    }

    /// Skips one element of any tag.
    pub(crate) fn any(&mut self, what: &str) -> Result<&'a [u8], X509Error> {
        Ok(self.next(what)?.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The DER SubjectPublicKeyInfo every test builds with: an uncompressed
    /// P-256 point.
    const SPKI: &[u8] = &[
        0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08,
        0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00, 0x04, 0x11, 0x11, 0x11,
        0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
        0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
        0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
        0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
        0x11,
    ];

    /// A default self-signed spec; tests vary one or two fields.
    fn spec<'a>(dns_name: &'a str) -> SelfSigned<'a> {
        SelfSigned {
            common_name: "synchronicity zone key",
            dns_name,
            spki: SPKI,
            serial: &[0x01, 0x02, 0x03],
            not_before: x509_time(1_760_000_000),
            not_after: x509_time(4_900_000_000),
            extensions: &[],
        }
    }

    /// A signature no test verifies (Rekor does not either).
    fn fake_sign(_: &[u8]) -> Vec<u8> {
        vec![0x30, 0x06, 0x02, 0x01, 0x01, 0x02, 0x01, 0x01]
    }

    /// A second `[3]` block inside `tbsCertificate` is refused, not skipped.
    ///
    /// The exactly-one rules read the extension *list*, so a second `[3]` TLV
    /// carrying its own list is a way past them that never touches them:
    /// before `tbs.finish` the second block simply sat there unread, and the
    /// certificate meant one thing here and another to any reader that took
    /// the last block (the parser-differential bug this regresses).
    #[test]
    fn a_second_extensions_block_inside_the_tbs_is_refused() {
        let spec = spec("sync.example");
        let sign = fake_sign;

        // The honest certificate parses, so the assertion below is about the
        // second block and not about the builder.
        let good = spec.build(sign);
        let cert = Certificate::parse(&good).expect("the honest certificate parses");
        assert_eq!(cert.single_dns_name().unwrap().to_string(), "sync.example.");

        // The same tbs with a second [3] appended, naming a different zone.
        let smuggled = tlv(
            0xa3,
            &tlv(
                0x30,
                &extension(
                    OID_SUBJECT_ALT_NAME,
                    false,
                    &tlv(0x30, &tlv(0x82, b"attacker.example")),
                ),
            ),
        );
        // Strip the outer SEQUENCE header off the tbs, append, re-wrap.
        let tbs = spec.tbs();
        let header = match tbs[1] {
            n if n < 0x80 => 2,
            n => 2 + usize::from(n & 0x7f),
        };
        let body = tlv(0x30, &[&tbs[header..], &smuggled[..]].concat());
        let forged = tlv(
            0x30,
            &[
                body,
                algorithm_identifier(),
                tlv(0x03, &[&[0x00], &fake_sign(&[])[..]].concat()),
            ]
            .concat(),
        );
        let error = Certificate::parse(&forged).expect_err("a second [3] must not parse");
        assert!(error.to_string().contains("tbsCertificate"), "{error}");
    }

    /// The outer SEQUENCE is closed like every other level.
    ///
    /// The members after `tbsCertificate` were never read, so the one level
    /// that wraps the whole certificate was the one that let anything through.
    /// A log entry has to mean the same thing to an auditor with a stricter
    /// parser; Go's and OpenSSL's both refuse these.
    #[test]
    fn the_outer_certificate_sequence_is_closed_too() {
        let spec = spec("sync.example");
        let signature = tlv(0x03, &[&[0x00], &fake_sign(&[])[..]].concat());

        // Only a tbs, with no signature fields at all.
        let bare = tlv(0x30, &spec.tbs());
        assert!(
            Certificate::parse(&bare).is_err(),
            "a bare tbs must not parse"
        );

        // The honest three members, plus a fourth nobody reads.
        let trailing = tlv(
            0x30,
            &[
                spec.tbs(),
                algorithm_identifier(),
                signature,
                tlv(0x02, &[0x01]),
            ]
            .concat(),
        );
        let error = Certificate::parse(&trailing).expect_err("a trailing member must not parse");
        assert!(error.to_string().contains("the certificate"), "{error}");
    }

    #[test]
    fn a_built_certificate_parses_back_to_what_went_in() {
        let base = spec("sync.example");
        let extra = vec![(vec![0x41, 0x01], b"payload".to_vec())];
        let spec = SelfSigned {
            extensions: &extra,
            ..base.clone()
        };
        let der = spec.build(fake_sign);
        let cert = Certificate::parse(&der).expect("the certificate must parse");
        assert_eq!(cert.single_dns_name().unwrap().to_string(), "sync.example.");
        assert_eq!(cert.extension(&[0x41, 0x01]), Some(&b"payload"[..]));
        assert_eq!(cert.extension(&[0x41, 0x02]), None);
        // The standard three are present, and the two that must be critical
        // are — a monitor reading this certificate in another toolchain must
        // see a well-formed end-entity certificate, not a curiosity.
        let by_oid = |oid: &[u8]| cert.extensions.iter().find(|e| e.oid == oid).cloned();
        assert!(by_oid(OID_BASIC_CONSTRAINTS).unwrap().critical);
        assert!(by_oid(OID_KEY_USAGE).unwrap().critical);
        assert!(!by_oid(OID_SUBJECT_ALT_NAME).unwrap().critical);
        // notBefore is a UTCTime in 2025 and notAfter a GeneralizedTime in
        // 2125 — the boundary RFC 5280 draws at 2050.
        assert_eq!(spec.not_before, Time::Utc("251009085320Z".into()));
        assert_eq!(spec.not_after, Time::Generalized("21250410230640Z".into()));

        // And under long-form DER lengths, the path where hand-rolled DER
        // usually dies: a 300-byte SAN and a 500-byte extension force the
        // multi-byte length in both the writer and the reader.
        let long = "a".repeat(300);
        let extra = vec![(vec![0x41, 0x09], vec![0x7f; 500])];
        let spec = SelfSigned {
            dns_name: &long,
            serial: &[0xff, 0xff],
            not_before: x509_time(0),
            not_after: x509_time(1),
            extensions: &extra,
            ..base
        };
        let der = spec.build(fake_sign);
        let cert = Certificate::parse(&der).unwrap();
        assert_eq!(cert.dns_names, vec![long]);
        assert_eq!(cert.extension(&[0x41, 0x09]).unwrap().len(), 500);
    }

    #[test]
    fn truncated_and_malformed_der_is_refused_rather_than_panicking() {
        let spec = SelfSigned {
            common_name: "cn",
            serial: &[0x01],
            not_before: x509_time(1_760_000_000),
            not_after: x509_time(1_760_000_001),
            ..spec("sync.example")
        };
        let der = spec.build(fake_sign);
        for cut in [0, 1, 2, 5, 30, der.len() - 1] {
            assert!(Certificate::parse(&der[..cut]).is_err(), "cut {cut}");
        }
        assert!(Certificate::parse(&[0x31, 0x00]).is_err());
        assert!(Certificate::parse(&[0x30, 0x80, 0x00, 0x00]).is_err());
    }

    /// A SAN that is not a DNS name is refused here, not normalized away.
    ///
    /// `"x.."` reaching a caller as a string would compare equal to `"x."`
    /// under a trailing-dot trim, which is how a client-accepted entry lands
    /// in a monitor's silent bin (see `crate::chain::authorize`). Arity is
    /// part of the same boundary: two names are not one entry, and none is
    /// not an entry at all.
    #[test]
    fn a_san_that_is_not_a_name_is_refused() {
        let with = |san: &str| Certificate {
            spki: SPKI.to_vec(),
            dns_names: vec![san.to_string()],
            extensions: Vec::new(),
        };
        for bad in ["sync.example..", "sync.example...", "sync..example", ""] {
            assert!(
                with(bad).single_dns_name().is_err(),
                "{bad:?} must not be a usable SAN"
            );
        }
        assert!(Certificate {
            spki: SPKI.to_vec(),
            dns_names: vec!["a.example".into(), "b.example".into()],
            extensions: Vec::new(),
        }
        .single_dns_name()
        .is_err());
        // The absolute and relative spellings of one name are the same name,
        // and both are canonical.
        let canonical = with("sync.example.").single_dns_name().unwrap();
        assert_eq!(with("sync.example").single_dns_name().unwrap(), canonical);
        // But a spelling that merely *parses* to the name is refused, because
        // the SAN is the string a reader indexing the log searches for — a
        // non-canonical spelling would be accepted and unfindable (§4.2.1).
        for evasion in ["SYNC.EXAMPLE.", "Sync.Example", "syn\\c.example"] {
            let error = with(evasion)
                .single_dns_name()
                .expect_err("a non-canonical spelling of the apex must be refused");
            assert!(
                error.to_string().contains("is spelled"),
                "{evasion:?} refused for the wrong reason: {error}"
            );
        }
        // And the evasion really does parse to the name it would have hidden.
        assert_eq!(
            Name::from_utf8("syn\\c.example").unwrap().to_lowercase(),
            Name::from_utf8("sync.example").unwrap().to_lowercase(),
        );
        // Not a suffix match: a certificate for the parent is not a
        // certificate for the child, however tempting the substring is.
        assert_ne!(with("example.").single_dns_name().unwrap(), canonical);
    }

    /// A second `GeneralNames` sequence inside one subjectAltName is refused.
    ///
    /// The exactly-one-*extension* rule never fires here — there is one
    /// extension — so without the wrapper check a reader takes the first
    /// sequence and drops the rest, seeing one name while the leaf carries
    /// two. Go accepts this shape and Rekor will log it; OpenSSL rejects it.
    /// Readers in the world disagree about these bytes, so this one refuses
    /// them.
    #[test]
    fn a_second_general_names_sequence_is_refused() {
        let one = tlv(0x82, b"sync.example");
        let other = tlv(0x82, b"attacker.example");
        // The honest shape reads as one name.
        assert_eq!(
            parse_san(&tlv(0x30, &one)).unwrap(),
            vec!["sync.example".to_string()]
        );
        // Two sequences, back to back, inside one extnValue.
        let mut doubled = tlv(0x30, &one);
        doubled.extend_from_slice(&tlv(0x30, &other));
        let error = parse_san(&doubled).expect_err("a second sequence must be refused");
        assert!(
            error.to_string().contains("bytes after"),
            "refused for the wrong reason: {error}"
        );
    }
}
