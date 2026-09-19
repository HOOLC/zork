//! iroh endpoint, the two ALPN protocols, and DNSSEC membership.
//!
//! `sync/mpt/1` carries head gossip and trie fetches (§5); `sync/blob/1`
//! carries verified bao slices (§6.4). Both are gated on the bindings table, so
//! a device key with no live binding is closed out immediately after the QUIC
//! handshake (§3.2).
#![deny(missing_docs)]

pub mod blob;
mod blocking;
pub mod chain;
pub mod dns;
pub mod endpoint;
pub mod error;
pub mod frame;
pub mod mpt;
pub mod process;
mod punch;
pub use punch::RelayHttpRoute;
mod relay_access;
pub use relay_access::RelayAccess;
mod pubkey;
pub mod rekor;
mod serve;
/// Test machinery — a simulated signed zone, transparency log and TUF
/// repository. Behind the non-default `sim` feature: it is 1300 lines with
/// no place in a shipped binary, and no business being part of this crate's
/// stable surface.
#[cfg(feature = "sim")]
#[doc(hidden)]
pub mod sim;
pub mod sock;
#[cfg(test)]
mod testing;
pub mod tls;
pub mod tuf;
pub mod x509;
pub mod zonecert;

pub use blob::{proof_window, BlobClient, Proof, ProofOutcome};
pub use dns::{
    ControlPlaneRecord, DialHint, DnssecResolver, MemberResolver, MemberSet, RekorPolicy,
    ResolverOptions,
};
pub use endpoint::{
    ControlProtocol, DEFAULT_PUBLIC_DNS, Net, NetOptions, ALPN_CONTROL, default_public_dns,
    configure_endpoint,
};
pub use error::{NetError, Result};
pub use mpt::{HeadSink, MptClient};
pub use rekor::{ProofError, RekorProof};
pub use tuf::{PinState, TufError, TufMetadata};
