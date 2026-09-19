//! The connection lifecycle both ALPNs are served under (§3.2, §6.3).
//!
//! Everything either protocol does around a request is the same: refuse a
//! connection whose device key has no live binding and ring the §3.4 bell,
//! re-check the binding on every message rather than once per session, bound how
//! many requests one connection may have in flight, and bound how long any one
//! of them may take. What differs is only what a stream *carries*, which is the
//! closure each handler passes in.
//!
//! One implementation, because two drift: a stream deadline that exists on one
//! ALPN and not the other is not a policy, it is an oversight with a second
//! home to hide in.

use std::sync::Arc;

use iroh::{
    endpoint::{Connection, RecvStream, SendStream},
    protocol::AcceptError,
};
use synch_core::{now_ns, NodeId};
use synch_store::Store;

/// How many requests one connection may have in flight at once.
///
/// Handling streams one at a time to completion bounds nothing — a peer can
/// open more connections — while bounding throughput: the work behind a request
/// runs on the blocking pool, so a connection serializing its streams cannot
/// accept the next one while a window is being built, and §6.3's swarm
/// behaviour does not survive a client that pipelines.
///
/// Per connection, and only per connection — nothing caps how many connections
/// one peer opens, so this is not a bound on a node's concurrent work. It is
/// not meant to be: reaching this code at all requires a live binding, and §12
/// places a member that opens connections abusively under `synch trust rm`
/// rather than under a rate limiter. What this does bound is the cost of one
/// connection, which is what keeps an ordinary peer's pipelining from being
/// mistaken for that.
///
/// An embedder that cannot take §12's trust stance — one process serving
/// several clusters that do not trust each other — asks for
/// [`NetOptions::max_inflight_requests`](crate::NetOptions::max_inflight_requests)
/// as well, which bounds the same cost across the whole endpoint.
pub(crate) const MAX_CONCURRENT_STREAMS: usize = 8;

/// How long one request may take, start to finish.
///
/// Covers the read as well as the work. iroh's transport already tears down a
/// peer that has crashed or been partitioned away — it keeps a 5 s keep-alive
/// against a 15 s idle timeout — so what this bounds is a peer that holds its
/// session open deliberately and sends nothing: without it that stream owns a
/// task for as long as the peer likes, and the per-message binding re-check
/// above never comes round again. Generous, because one window of a large
/// object is real disk work.
pub(crate) const STREAM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// The endpoint-wide in-flight gate, when the embedder asked for one.
///
/// Shared by every connection on every ALPN this endpoint serves — see
/// [`NetOptions::max_inflight_requests`](crate::NetOptions::max_inflight_requests)
/// for why an embedder wants it and a daemon does not.
pub(crate) type Inflight = Option<Arc<tokio::sync::Semaphore>>;

/// Takes a slot in the endpoint-wide gate, if there is one.
///
/// `None` when no gate is configured, which is the daemon's case and costs
/// nothing. The semaphore is never closed, so the error arm is unreachable in
/// practice; it is written as "no slot, stop serving" rather than unwrapped,
/// because a closed gate must not be read as an open one.
pub(crate) async fn slot(
    inflight: &Inflight,
) -> Result<Option<tokio::sync::OwnedSemaphorePermit>, ()> {
    match inflight {
        None => Ok(None),
        Some(gate) => match gate.clone().acquire_owned().await {
            Ok(permit) => Ok(Some(permit)),
            Err(_) => Err(()),
        },
    }
}

/// Serves one accepted connection until the peer closes it or its binding
/// lapses.
///
/// `on_request` is called once when the connection is admitted and again for
/// every stream accepted on it, which is where a handler that tracks peer
/// sightings does its bookkeeping. `dispatch` handles one stream — under the
/// peer's cryptographically established device key, which the handshake settled
/// — reports its own failures to the peer in its own vocabulary, and finishes
/// the send side.
///
/// `inflight` is the endpoint-wide gate. It is taken around the admission
/// check as well as around every dispatched stream, and that is deliberate:
/// [`admit`] is a store call, it is the one store call an *unauthenticated*
/// dialer can reach, and a gate that started after admission would leave the
/// cheapest thing to send — a QUIC handshake — buying an unbounded queue of
/// blocking-pool work.
pub(crate) async fn serve_connection<D, F, S, G>(
    store: &Arc<Store>,
    connection: Connection,
    on_unknown_key: Option<&Arc<tokio::sync::Notify>>,
    inflight: &Inflight,
    mut on_request: G,
    dispatch: D,
) -> Result<(), AcceptError>
where
    D: Fn(NodeId, SendStream, RecvStream) -> F + Clone + Send + 'static,
    F: std::future::Future<Output = ()> + Send + 'static,
    G: FnMut(NodeId) -> S,
    S: std::future::Future<Output = ()>,
{
    let remote = connection.remote_id();
    {
        let Ok(_admission) = slot(inflight).await else {
            return Ok(());
        };
        admit(store, &connection, &remote, on_unknown_key).await?;
        on_request(remote).await;
    }

    let limit = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_STREAMS));
    while let Ok((send, recv)) = connection.accept_bi().await {
        // Before the binding re-check, not after it: that check is a store
        // call too, so a stream that never gets past it has still spent a
        // slot on the blocking pool.
        let Ok(shared) = slot(inflight).await else {
            break;
        };
        if !still_admitted(store, &connection, &remote).await {
            break;
        }
        on_request(remote).await;
        let Ok(permit) = limit.clone().acquire_owned().await else {
            break;
        };
        let dispatch = dispatch.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let _shared = shared;
            if tokio::time::timeout(STREAM_TIMEOUT, dispatch(remote, send, recv))
                .await
                .is_err()
            {
                tracing::debug!(peer = %remote.fmt_short(), "stream timed out");
            }
        });
    }
    Ok(())
}

/// Admits a freshly accepted connection, or refuses and closes it (§3.2):
/// a device key with no live binding is not a peer, and the connection is
/// closed immediately after the handshake. Rings the §3.4 unknown-key bell on
/// refusal — the far side of a key rotation arrives exactly this way.
///
/// One refusal for every ALPN, because the gate is membership policy, not
/// protocol: a third handler restating it is a third place for the §3.2 rule
/// to drift.
pub(crate) async fn admit(
    store: &Arc<Store>,
    connection: &Connection,
    remote: &NodeId,
    on_unknown_key: Option<&Arc<tokio::sync::Notify>>,
) -> Result<(), AcceptError> {
    if trusted(store, remote).await {
        return Ok(());
    }
    tracing::debug!(peer = %remote.fmt_short(), "refusing connection: no live binding");
    if let Some(wake) = on_unknown_key {
        wake.notify_waiters();
    }
    connection.close(0u32.into(), b"untrusted");
    Err(AcceptError::from_err(std::io::Error::other(
        "peer has no live binding",
    )))
}

/// The per-message half of [`admit`] (§3.2): a binding revoked or expired
/// mid-connection must cut off further requests, not linger for the life of
/// the QUIC session. On a lapse the connection is closed and `false` returned;
/// work already in flight is a conversation in progress and is left to finish.
pub(crate) async fn still_admitted(
    store: &Arc<Store>,
    connection: &Connection,
    remote: &NodeId,
) -> bool {
    if trusted(store, remote).await {
        return true;
    }
    tracing::debug!(peer = %remote.fmt_short(), "closing connection: binding lapsed");
    connection.close(0u32.into(), b"untrusted");
    false
}

/// Whether a device key has a live binding, decided on the blocking pool.
///
/// Off the runtime, like every other store read (§10). It looks like the one
/// lookup small enough to stay inline — one indexed row — and it is not: the
/// cost is not the query, it is the wait for the store's single connection
/// mutex, which a publish batch or a GC pass holds for as long as it runs.
/// This is also the only store call in the process an *unauthenticated* dialer
/// can reach, so running it inline let anyone who could complete a QUIC
/// handshake park a runtime worker behind whatever was writing.
///
/// A failure to reach the store is not a grant: anything but a definite `true`
/// closes the connection, which is the same fail-closed reading the inline
/// version had.
async fn trusted(store: &Arc<Store>, remote: &NodeId) -> bool {
    let store = store.clone();
    let remote = *remote;
    let answer: Result<bool, crate::error::NetError> =
        crate::blocking::offload(move || Ok(store.is_trusted_key(&remote, now_ns())?)).await;
    matches!(answer, Ok(true))
}
