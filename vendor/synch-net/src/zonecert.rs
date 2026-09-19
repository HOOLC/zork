//! The custom certificate extension a zone-key entry carries, and the OID
//! that names it (docs/REKOR-ZONE-KEY.md §2).
//!
//! The certificate in a Rekor leaf gets the apex into the log's Merkle tree
//! (see [`crate::x509`]). The extension gets *evidence* in beside it, so that
//! a monitor consuming the log can decide, from the leaf alone and with no
//! DNS query at all, whether the key it just saw is authorized for that zone:
//!
//! - **The DNSSEC chain** (`OID_DNSSEC_CHAIN`) — the delegation from the
//!   apex's DS up to the root DNSKEY, as raw signed RRsets. A monitor
//!   validates it offline against the IANA root trust anchor and asks
//!   **nothing at all about time**: RRSIG validity windows are not checked on
//!   either side, because a Merkle leaf carries no trustworthy clock
//!   (`integratedTime` is outside the commitment) and because RRSIGs expire
//!   in weeks while entries are read for years. An entry from 2029 verifies
//!   in 2039 exactly as it did the day it was logged. See `crate::chain`.
//!
//! The client does not read it either: it validates DNSSEC natively and
//! already holds the DNSKEY it is asking about, so re-deriving authorization
//! from a copy of the chain inside the entry would be a slower way to learn
//! the same fact. It is monitor food, and the client enforces its presence
//! only because an entry without one would be invisible to a monitor (see
//! `rekor::verify`).

#[cfg(any(test, feature = "sim"))]
use crate::x509::tlv;
use crate::x509::{Der, X509Error};

/// The DNSSEC chain extension: `2.25.1555716359`.
///
/// `1555716359` is `0xdcba5907` — the first four bytes of UUID
/// `dcba5907-a9a9-4de1-89fe-7b22794d9fbe` — masked into 31 bits. **Do not
/// restore the full 128-bit UUID arc**: Go's `encoding/asn1` rejects OID
/// components that overflow `int32`, so Rekor's certificate parser fails on
/// the wide form and the log refuses the entry with an opaque `400`. See the
/// module docs — this cost a live submission to find, because OpenSSL and
/// Erlang both parse the wide form without complaint.
///
/// These are the OID's DER *content* bytes: `0x69` is `2.25` packed into the
/// first byte (40 × 2 + 25 = 105), and the rest is the arc in base-128
/// continuation form.
pub const OID_DNSSEC_CHAIN: &[u8] = &[0x69, 0x85, 0xe5, 0xe9, 0xb2, 0x07];

// ------------------------------------------------------------ DNSSEC chain

/// One zone's worth of the delegation chain.
///
/// `rrs` is a run of **uncompressed wire-format** resource records —
/// `NAME | TYPE | CLASS | TTL | RDLENGTH | RDATA`, names spelled out in full
/// because a Merkle leaf has no message to compress against. The records a
/// link carries are the ones *owned* by `zone`:
///
/// - the bottom link is the **declaration** at
///   `_synchronicity-transparency.<apex>`: its `TXT` RRset and the `RRSIG`
///   the apex made over it, which narrows who can mint an entry about a zone
///   to zones that have declared themselves control planes (see
///   [`crate::chain`] for what that does and does not deliver);
/// - the apex link holds the apex's `DNSKEY` RRset + `RRSIG` — the key set
///   the entry claims — and its `DS` RRset + `RRSIG` (signed by the parent);
/// - every ancestor link holds that zone's `DNSKEY` RRset + `RRSIG` and its
///   `DS` RRset + `RRSIG`;
/// - the root link holds only the root `DNSKEY` RRset + `RRSIG`, which the
///   IANA trust anchor terminates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainLink {
    /// The owner zone, as an FQDN (`"."` for the root).
    pub zone: String,
    /// The concatenated wire-format RRs owned by `zone`.
    pub rrs: Vec<u8>,
}

/// `DnssecChain ::= SEQUENCE OF Link`, declaration first, root last.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DnssecChain {
    /// The links, ordered upward from the declaration below the apex.
    pub links: Vec<ChainLink>,
}

impl DnssecChain {
    /// The extension value: `SEQUENCE OF SEQUENCE { IA5String, OCTET STRING }`.
    ///
    /// Publisher-side. The control plane writes these certificates and a
    /// resolving client only ever decodes them, so the writer is behind the
    /// harness gate and no shipped client carries it (see [`crate::sim`]).
    #[cfg(any(test, feature = "sim"))]
    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::new();
        for link in &self.links {
            let mut one = tlv(0x16, link.zone.as_bytes());
            one.extend_from_slice(&tlv(0x04, &link.rrs));
            body.extend_from_slice(&tlv(0x30, &one));
        }
        tlv(0x30, &body)
    }

    /// Parses an extension value, refusing trailing bytes.
    pub fn decode(bytes: &[u8]) -> Result<DnssecChain, X509Error> {
        let mut outer = Der::new(bytes);
        let mut list = outer.sequence("DnssecChain")?;
        if !outer.is_empty() {
            return Err(X509Error::from("DnssecChain: bytes after the sequence"));
        }
        let mut links = Vec::new();
        while !list.is_empty() {
            let mut link = list.sequence("Link")?;
            // `zone` is an IA5String, which is ASCII — so check ASCII rather
            // than UTF-8 and then claim ASCII in the error. A DNS name that
            // needs more than ASCII is punycode by the time it reaches here.
            let bytes = link.tagged(0x16, "Link.zone")?;
            if !bytes.is_ascii() {
                return Err(X509Error::from("Link.zone is not ASCII"));
            }
            let zone = String::from_utf8(bytes.to_vec())
                .map_err(|_| X509Error::from("Link.zone is not ASCII"))?;
            let rrs = link.tagged(0x04, "Link.rrs")?.to_vec();
            if !link.is_empty() {
                return Err(X509Error::from("Link: unexpected trailing member"));
            }
            links.push(ChainLink { zone, rrs });
        }
        Ok(DnssecChain { links })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chain_round_trips_and_keeps_its_order() {
        let chain = DnssecChain {
            links: vec![
                ChainLink {
                    zone: "sync.example.".into(),
                    rrs: vec![0xaa; 200],
                },
                ChainLink {
                    zone: "example.".into(),
                    rrs: vec![0xbb; 400],
                },
                ChainLink {
                    zone: ".".into(),
                    rrs: vec![0xcc; 1200],
                },
            ],
        };
        // The apex comes first and the root last: a decoder that sorted or
        // reversed would still round-trip a one-link chain, so assert three.
        let der = chain.encode();
        let back = DnssecChain::decode(&der).unwrap();
        assert_eq!(back, chain);
        assert_eq!(back.links[0].zone, "sync.example.");
        assert_eq!(back.links[2].zone, ".");

        // Trailing bytes and truncation are a second encoding of the same
        // value, which is exactly what a decoder must never accept.
        let mut extra = der.clone();
        extra.push(0);
        assert!(DnssecChain::decode(&extra).is_err());
        assert!(DnssecChain::decode(&der[..der.len() - 1]).is_err());
        assert!(DnssecChain::decode(&[]).is_err());

        // An empty chain is representable — a chainless retire omits the
        // extension, so the codec must not lose the distinction.
        let empty = DnssecChain::default();
        assert_eq!(DnssecChain::decode(&empty.encode()).unwrap(), empty);
    }

    /// The OID arc must stay inside `int32`, forever.
    ///
    /// Go's `encoding/asn1` — and therefore Rekor's `x509.ParseCertificate`,
    /// and therefore Rekor — rejects any OID component that overflows
    /// `int32`; OpenSSL and Erlang parse the wide form happily, which is how
    /// the original 128-bit UUID arcs passed every test in this repo and
    /// still could not be published. If this fails, the fix is a *narrower*
    /// arc, never a wider parser.
    #[test]
    fn the_oid_arc_fits_in_an_int32_because_go_rejects_anything_larger() {
        // 2.25.1555716359, DER content bytes: the first packs `2.25`
        // (40 * 2 + 25), the rest is the arc in base-128 continuation form.
        assert_eq!(
            OID_DNSSEC_CHAIN,
            &[0x69, 0x85, 0xe5, 0xe9, 0xb2, 0x07],
            "the DNSSEC chain OID changed"
        );
        assert_eq!(OID_DNSSEC_CHAIN[0], 40 * 2 + 25);
        // The arc is the first four bytes of its UUID, masked to 31 bits.
        let arc = u128::from(u32::from_be_bytes([0xdc, 0xba, 0x59, 0x07]) & 0x7fff_ffff);
        assert_eq!(arc, 1_555_716_359);
        assert!(
            arc <= i32::MAX as u128,
            "the arc must fit int32 — Rekor's Go parser will refuse the \
             certificate and the log will reject the entry"
        );
    }
}
