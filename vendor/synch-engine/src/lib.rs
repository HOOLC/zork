//! The embeddable synchronicity node (§11).
//!
//! Everything a host application needs: identity and membership, indexed
//! spaces with a streaming scanner, a publisher that turns staged changes into
//! one signed root, the anti-entropy scheduler, the verified content fetcher,
//! and optional replica checkouts. The two shipped binaries are thin shells over this
//! crate, and any Rust application can embed a full node the same way.
#![deny(missing_docs)]

pub mod aae;
pub mod adopt_tree;
mod blocking;
pub mod checkout;
pub mod cloud;
pub mod compare;
pub mod config;
pub mod error;
pub mod fetcher;
pub mod ignore;
mod join;
pub mod lifecycle;
pub mod membership;
pub mod node;
pub mod publisher;
pub mod reconcile;
pub mod recovery;
pub mod reference;
pub mod replica;
pub mod rotation;
pub mod scanner;
pub mod sockets;
#[cfg(test)]
mod testkit;
pub mod tree;
pub mod uploads;
pub mod watcher;

pub use adopt_tree::AdoptTreeOptions;
pub use compare::{CompareChange, CompareReport, CompareStatus};
pub use config::{
    default_data_dir, NodeConfig, DEFAULT_REPLICA_CONCURRENCY, MAX_REPLICA_CONCURRENCY,
};
pub use error::{EngineError, Result};
pub use fetcher::{PreparedRange, Provider};
pub use lifecycle::{LifecycleLock, LIFECYCLE_FILE};
pub use membership::{DomainHealth, DomainRefresh, ResolverStatus};
pub use node::Node;
pub use publisher::Publisher;
pub use reconcile::{FetchOutcome, Promotion, Syncer};
pub use recovery::RecoveryOptions;
pub use reference::EntryRef;
pub use rotation::PeerBindings;
pub use scanner::Adoption;
pub(crate) use scanner::CloneKind;
pub use sockets::TreeWriter;
pub use synch_net::dns::RekorPolicy;
pub use synch_sock::{HostError, PutCondition, PutReceipt, SocketWriter};
pub use synch_store::{Donor, ProvenSubtree, Selection, Version, VersionPolicy, VersionSet};
pub use uploads::{CompletedUpload, Deleted, PartStaging, DEFAULT_UPLOAD_TTL};
