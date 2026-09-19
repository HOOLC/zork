//! The crate's one signature-verification core.
//!
//! [`crate::rekor::LogKey`] and [`crate::tuf::TufKey`] carry the same two key
//! shapes — an uncompressed P-256 point or 32 Ed25519 bytes — verify under the
//! same double-encoding rule, and strip the same SubjectPublicKeyInfo
//! prefixes. This is the only signature-verification code in the crate, so a
//! fix to any of that must never have to land twice: the wrappers keep only
//! what is their own (a log id and origin pin, a key-table entry).

use aws_lc_rs::signature;

/// The signature scheme a verification key uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scheme {
    /// ECDSA P-256 with SHA-256.
    EcdsaP256Sha256,
    /// Ed25519.
    Ed25519,
}

/// The fixed DER prefix of a SubjectPublicKeyInfo holding an uncompressed
/// P-256 point: the algorithm identifier for `id-ecPublicKey` over
/// `prime256v1`, the bit-string header, and the `0x04` uncompressed-point tag.
pub(crate) const P256_SPKI_PREFIX: &[u8] = &[
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00, 0x04,
];

/// The same for an Ed25519 key.
pub(crate) const ED25519_SPKI_PREFIX: &[u8] = &[
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

/// A raw verification key: the scheme, and the key material as the verifier
/// wants it — an uncompressed `0x04`-tagged P-256 point, or 32 Ed25519 bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawKey {
    pub(crate) scheme: Scheme,
    pub(crate) point: Vec<u8>,
}

impl RawKey {
    /// Reads a DER SubjectPublicKeyInfo holding either recognized key shape.
    ///
    /// Deliberately narrow: two shapes are recognized and everything else is
    /// refused, rather than a general ASN.1 reader parsing whatever it is
    /// handed.
    pub(crate) fn from_spki(der: &[u8]) -> Option<RawKey> {
        if let Some(point) = der.strip_prefix(P256_SPKI_PREFIX) {
            if point.len() == 64 {
                return Some(RawKey {
                    scheme: Scheme::EcdsaP256Sha256,
                    point: uncompressed(point),
                });
            }
        }
        if let Some(point) = der.strip_prefix(ED25519_SPKI_PREFIX) {
            if point.len() == 32 {
                return Some(RawKey {
                    scheme: Scheme::Ed25519,
                    point: point.to_vec(),
                });
            }
        }
        None
    }

    /// [`RawKey::from_spki`], where the caller already knows which scheme the
    /// key must have: a well-formed key of the other scheme is a refusal, not
    /// a detection.
    pub(crate) fn from_spki_as(der: &[u8], scheme: Scheme) -> Option<RawKey> {
        RawKey::from_spki(der).filter(|key| key.scheme == scheme)
    }

    /// The pre-SPKI form: raw key material, as Sigstore's roots 1–4 wrote it.
    pub(crate) fn from_raw(bytes: &[u8], scheme: Scheme) -> Option<RawKey> {
        let point = match scheme {
            Scheme::EcdsaP256Sha256 => match bytes {
                [0x04, ..] if bytes.len() == 65 => bytes.to_vec(),
                _ if bytes.len() == 64 => uncompressed(bytes),
                _ => return None,
            },
            Scheme::Ed25519 if bytes.len() == 32 => bytes.to_vec(),
            Scheme::Ed25519 => return None,
        };
        Some(RawKey { scheme, point })
    }

    /// Whether `signature` verifies over `message`, accepting **either** ECDSA
    /// encoding.
    ///
    /// An ECDSA signature travels two ways — IEEE P1363's fixed 64-byte
    /// `r ‖ s`, and ASN.1/DER — and Sigstore signs both its notes and its TUF
    /// metadata with DER: the live `rekor.sigstore.dev` signature is 70 bytes
    /// opening `30 44 02 20`, an unmistakable DER header. A DER signature can
    /// never satisfy a fixed-width verifier, so a verifier that took only one
    /// encoding would refuse every artifact from a P-256 key with an error
    /// that reads like a misconfigured pin set. Ed25519 has one encoding.
    ///
    /// Accepting both is not a weakening: either encoding of a valid
    /// signature is a valid signature by that key, and nothing here treats a
    /// signature as unique — two spellings of one signature, conceding
    /// nothing beyond the malleability ASN.1 already has.
    pub(crate) fn verifies(&self, message: &[u8], signature_bytes: &[u8]) -> bool {
        let algorithms: &[&dyn signature::VerificationAlgorithm] = match self.scheme {
            Scheme::EcdsaP256Sha256 => &[
                &signature::ECDSA_P256_SHA256_ASN1,
                &signature::ECDSA_P256_SHA256_FIXED,
            ],
            Scheme::Ed25519 => &[&signature::ED25519],
        };
        algorithms.iter().any(|algorithm| {
            signature::UnparsedPublicKey::new(*algorithm, &self.point)
                .verify(message, signature_bytes)
                .is_ok()
        })
    }
}

/// Tags a raw 64-byte P-256 coordinate pair as an uncompressed point.
fn uncompressed(point: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(65);
    out.push(0x04);
    out.extend_from_slice(point);
    out
}
