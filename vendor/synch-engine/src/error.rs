//! Errors produced by the engine.

use synch_core::NodeId;

/// The engine result alias.
pub type Result<T> = std::result::Result<T, EngineError>;

/// An error from the node API.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// The store failed.
    #[error(transparent)]
    Store(#[from] synch_store::StoreError),
    /// A trie operation failed.
    #[error(transparent)]
    Mpt(#[from] synch_mpt::MptError),
    /// The network failed.
    #[error(transparent)]
    Net(#[from] synch_net::NetError),
    /// Filesystem I/O failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// A blocking operation did not run to completion.
    ///
    /// Blocking work — scanning, hashing, publishing, CAS reads and writes —
    /// runs on tokio's blocking pool rather than on a runtime worker
    /// (the `blocking` module), and the pool reports only two failures: the
    /// closure panicked, or the runtime is shutting down under it. Neither
    /// says anything about the state the operation was midway through, which
    /// is why it is surfaced rather than treated as a no-op.
    #[error("a blocking task did not complete: {0}")]
    Blocking(String),
    /// A record could not be encoded or decoded.
    #[error("record: {0}")]
    Record(String),

    /// A path or key was invalid.
    #[error(transparent)]
    Key(#[from] synch_core::KeyError),
    /// The node has not been initialized (`synch init`).
    #[error("this data directory has no identity: run `synch init` first")]
    NotInitialized,
    /// The node has no active device key.
    #[error("no active device key")]
    NoActiveKey,
    /// The membership zone has not named this node yet (§3.1).
    ///
    /// Not a failure to start so much as a state to wait in: the daemon polls
    /// until a record binds this key, and everything that would publish is
    /// refused until it does.
    #[error(
        "{domain} does not name this node yet — publish a record for it:\n  \
         _synchronicity.{domain}. IN TXT \"v=sync1 id=<name> nk={} apex=<apex>\"\n\
         this node waits on a reduced control socket until it does; if it is a \
         delegate and is not meant to be named by that zone, re-run \
         `synch domain set {domain} --delegate`",
        node_id.to_z32()
    )]
    Unidentified {
        /// The zone that was asked.
        domain: String,
        /// The device key it did not name. Boxed to keep the enum small.
        node_id: Box<NodeId>,
    },
    /// A space, origin, or entry was not found.
    #[error("not found: {0}")]
    NotFound(String),
    /// A caller supplied an invalid argument.
    #[error("{0}")]
    Invalid(String),
    /// A `strict` read met a divergent path (§8).
    ///
    /// Divergence is data, not a fault: the versions are carried here so the
    /// surface that refused the read can name them.
    #[error(
        "{space}/{path} has {} versions and the policy is strict:\n  {}",
        versions.len(),
        versions.join("\n  ")
    )]
    Divergent {
        /// The space.
        space: String,
        /// The path within it.
        path: String,
        /// One rendered line per version, newest first.
        versions: Vec<String>,
    },
    /// This node is in key-loss recovery and must not publish (§3.4).
    ///
    /// Publishing anyway would mint heads at a seq every peer correctly
    /// rejects, with nothing on either side saying why.
    #[error(
        "{origin} is in key-loss recovery: peers hold heads for it up to seq {observed_seq}, \
         so a publish at seq {would_publish} would be rejected by every one of them. \
         Run `synch recover` to collect what peers have seen and resume publishing above it"
    )]
    InRecovery {
        /// This node's own origin.
        origin: synch_core::OriginId,
        /// The highest seq any peer has advertised for it.
        observed_seq: u64,
        /// The seq the refused publish would have carried.
        would_publish: u64,
    },
}

impl EngineError {
    /// Builds an invalid-argument error.
    pub fn invalid(msg: impl Into<String>) -> Self {
        EngineError::Invalid(msg.into())
    }

    /// Builds a not-found error.
    pub fn not_found(msg: impl Into<String>) -> Self {
        EngineError::NotFound(msg.into())
    }
}

impl From<synch_core::record::CodecError> for EngineError {
    fn from(e: synch_core::record::CodecError) -> Self {
        EngineError::Record(e.0)
    }
}

impl synch_core::TaskLost for EngineError {
    fn task_lost(reason: String) -> Self {
        EngineError::Blocking(reason)
    }
}
