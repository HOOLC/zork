//! Errors produced by the networking layer.

use synch_core::Hash;

/// The networking result alias.
pub type Result<T> = std::result::Result<T, NetError>;

/// An error from the endpoint, either ALPN, or the DNSSEC resolver.
#[derive(Debug, thiserror::Error)]
pub enum NetError {
    /// Binding or dialing the iroh endpoint failed.
    #[error("endpoint: {0}")]
    Endpoint(String),
    /// The host's certificate trust store could not be read ([`crate::tls`]).
    #[error("system certificate store: {0}")]
    Tls(String),
    /// A QUIC stream failed.
    #[error("stream: {0}")]
    Stream(String),
    /// Reading from a QUIC stream failed.
    #[error("read: {0}")]
    Read(String),
    /// A message could not be encoded.
    #[error("encode: {0}")]
    Encode(String),
    /// A message could not be decoded.
    #[error("decode: {0}")]
    Decode(String),
    /// A framed message exceeded the size cap (§12).
    #[error("frame of {0} bytes exceeds the maximum")]
    FrameTooLarge(usize),
    /// The peer sent a message that does not belong at this point.
    #[error("unexpected message: {0}")]
    Unexpected(String),
    /// The peer is not a member: its device key has no live binding (§3.2).
    #[error("peer {0} has no live binding")]
    Untrusted(String),
    /// A trie node did not hash to the hash it was requested by (§5.2).
    ///
    /// This is a protocol violation, not a transient failure: the connection is
    /// dropped.
    #[error("peer served a trie node that does not hash to {expected}")]
    NodeHashMismatch {
        /// The hash the node was requested by.
        expected: Hash,
    },
    /// An out-of-line value did not hash to the hash it was requested by.
    #[error("peer served a trie value that does not hash to {expected}")]
    ValueHashMismatch {
        /// The hash the value was requested by.
        expected: Hash,
    },
    /// The store failed.
    #[error(transparent)]
    Store(#[from] synch_store::StoreError),
    /// A blocking store operation did not run to completion.
    ///
    /// Store work runs on tokio's blocking pool rather than on a runtime
    /// worker (the `blocking` module); the pool reports only a panicked closure
    /// or a runtime shutting down under it.
    #[error("a blocking task did not complete: {0}")]
    Blocking(String),
    /// A trie operation failed.
    #[error(transparent)]
    Mpt(#[from] synch_mpt::MptError),
    /// The DNSSEC resolver failed or refused a response (§3.2, fail closed).
    #[error("dns: {0}")]
    Dns(String),
    /// The zone publishes no transparency record for the key that signed the
    /// answer (docs/REKOR-ZONE-KEY.md §4.3).
    ///
    /// Absence on a not-yet-upgraded control plane reads differently from
    /// every variant below it, which are alarms: this one is a zone that has
    /// not caught up, or a key nobody ever logged.
    #[error("zone key transparency: {name} publishes no record for key tag {key_tag}")]
    RekorAbsent {
        /// The proof record's owner name.
        name: String,
        /// The key tag the answer's RRSIG named.
        key_tag: u16,
    },
    /// A proof record exists but is not a v4 `RekorProof`.
    #[error("zone key transparency: malformed proof at {name}: {reason}")]
    RekorMalformed {
        /// The proof record's owner name.
        name: String,
        /// What failed to decode.
        reason: String,
    },
    /// The entry's DSSE signature does not verify under the certificate's
    /// own key: the entry misattributes itself.
    #[error("zone key transparency: {name}: attribution: {reason}")]
    RekorAttribution {
        /// The proof record's owner name.
        name: String,
        /// Which attribution check failed.
        reason: String,
    },
    /// The logged Statement does not describe the key and zone observed.
    #[error("zone key transparency: {name}: binding: {reason}")]
    RekorBinding {
        /// The proof record's owner name.
        name: String,
        /// Which binding check failed.
        reason: String,
    },
    /// The entry is not in the tree its checkpoint commits to.
    #[error("zone key transparency: {name}: inclusion: {reason}")]
    RekorInclusion {
        /// The proof record's owner name.
        name: String,
        /// Where the audit path failed.
        reason: String,
    },
    /// The checkpoint is not signed by the log it claims to come from.
    #[error("zone key transparency: {name}: checkpoint: {reason}")]
    RekorCheckpoint {
        /// The proof record's owner name.
        name: String,
        /// Which checkpoint check failed.
        reason: String,
    },
    /// Sigstore's TUF repository served nothing that verified, so the pin
    /// set did not move (docs/REKOR-ZONE-KEY.md §10.2).
    ///
    /// Never fatal to a refresh, by design: expiry gates updates, never
    /// operation, and a repository that is unreachable, stale or hostile
    /// leaves the client exactly where it was. The variant exists so `synch
    /// doctor` can say *which* way the chain broke — a threshold failure and
    /// a stale timestamp are very different news.
    #[error("tuf pin refresh: {repository}: {class}: {reason}")]
    Tuf {
        /// The repository that was walked.
        repository: String,
        /// The failure class: chain, threshold, signature, expiry, rollback,
        /// target-hash, malformed.
        class: &'static str,
        /// What failed, in the verifier's own words.
        reason: String,
    },
    /// The entry lives in a log this client was never told to trust.
    #[error("zone key transparency: {name}: unknown log: {reason}")]
    RekorUnknownLog {
        /// The proof record's owner name.
        name: String,
        /// Which log was named, and what is pinned instead.
        reason: String,
    },
    /// The entry carries no DNSSEC chain, or one that does not establish
    /// that this key was delegated for this zone. Refused on the monitors'
    /// behalf: an entry a monitor would file as noise must not be an entry a
    /// client accepts (docs/REKOR-ZONE-KEY.md §5.5).
    #[error("zone key transparency: {name}: {reason}")]
    RekorChain {
        /// The proof record's owner name.
        name: String,
        /// How the chain failed, in the validator's own words.
        reason: String,
    },
    /// A caller supplied an invalid argument.
    #[error("{0}")]
    Invalid(String),
}

impl From<iroh::endpoint::WriteError> for NetError {
    fn from(e: iroh::endpoint::WriteError) -> Self {
        NetError::Stream(e.to_string())
    }
}

impl From<iroh::endpoint::ReadExactError> for NetError {
    fn from(e: iroh::endpoint::ReadExactError) -> Self {
        NetError::Read(e.to_string())
    }
}

impl From<iroh::endpoint::ConnectionError> for NetError {
    fn from(e: iroh::endpoint::ConnectionError) -> Self {
        NetError::Stream(e.to_string())
    }
}

impl From<iroh::endpoint::ClosedStream> for NetError {
    fn from(e: iroh::endpoint::ClosedStream) -> Self {
        NetError::Stream(e.to_string())
    }
}

impl synch_core::TaskLost for NetError {
    fn task_lost(reason: String) -> Self {
        NetError::Blocking(reason)
    }
}
