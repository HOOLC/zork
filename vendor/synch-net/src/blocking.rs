//! The blocking-pool handoff, pinned to this crate's error type.
//!
//! The implementation is [`crate::blocking::offload`], shared with every other crate
//! that needs it. This is only the type pin: without it each call site has to
//! annotate the error type, since the generic helper cannot infer it from a
//! bare `Ok(())`.
//!
//! Why anything is offloaded at all: both ALPN handlers run inside iroh's
//! connection tasks, on the same workers that drive every other connection,
//! timer, and control request in the process. Serving a slice reads a payload
//! and an outboard off disk; receiving one decodes into sparse files and fsyncs
//! them; a head offer walks a trie in SQLite. All of it is blocking and bounded
//! by object or tree size rather than by anything the runtime can interrupt, so
//! a single large transfer served inline would stall every other connection
//! this node has. Nothing stays inline, including the per-connection and
//! per-stream binding check in `serve`: the cost of a store call on a worker is
//! the wait for the one connection mutex, not only the query.

use crate::error::NetError;

/// Runs a blocking store operation on the blocking pool and awaits its result.
pub(crate) async fn offload<T, F>(f: F) -> Result<T, NetError>
where
    F: FnOnce() -> Result<T, NetError> + Send + 'static,
    T: Send + 'static,
{
    synch_core::offload(f).await
}
