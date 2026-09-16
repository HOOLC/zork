//! The blocking-pool handoff, pinned to this crate's error type.
//!
//! The implementation is [`crate::blocking::offload`], shared with every other crate
//! that needs it. This is only the type pin: without it each call site has to
//! annotate the error type, since the generic helper cannot infer it from a
//! bare `Ok(())`.
//!
//! Why anything is offloaded at all (§10): almost everything the node does to
//! durable state is blocking, and deliberately so — the store is
//! runtime-agnostic and `tokio::fs` would only hide the same calls behind a
//! slower façade. What matters is where it runs. A worker thread hashing a
//! 10 GB file is not polling anything: the endpoint stops answering peers, the
//! control socket stops answering `synch status`, and the timers driving
//! anti-entropy fire late. There is no "short enough to stay inline"
//! exception — §10 retired it, and `Store::conn` asserts it is gone.

use crate::error::Result;

/// Runs a blocking operation on the blocking pool and awaits its result.
///
/// The closure owns everything it touches, which is why [`Node`](crate::Node)
/// is cheap to clone: callers clone the handle into the closure rather than
/// borrowing across the await.
pub(crate) async fn offload<T, F>(f: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    synch_core::offload(f).await
}
