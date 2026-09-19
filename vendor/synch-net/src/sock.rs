//! The `sync/sock/1` ALPN: one invocation per incoming stream
//! (`docs/SOCKETS.md` §4).
//!
//! This module carries bytes and nothing else. It does not know what a socket
//! is, where a program comes from, or what makes one runnable — those live
//! behind [`SocketService`], which the engine implements. What is here is the
//! part that is genuinely the network's: the accept gate, the `Open` handshake,
//! the control uni-stream, and the decision to let a stream live as long as it
//! likes.
//!
//! That last one is why this ALPN does not reuse `serve::serve_connection`
//! the way the other two do. Their connection loop bounds a stream at two
//! minutes and a connection at eight in flight, and both are right for a
//! request/response protocol and wrong here: a socket that proxies is
//! *supposed* to be long-lived, and its concurrency bound is the socket's
//! own armed `max_streams` rather than a number this layer picks.
//!
//! The two bounds still apply to the one phase they are right for. A stream
//! that never finishes its `Open` handshake is not an invocation — it has no
//! runtime, no admission, and no deadline of its own, and without a bound it
//! owns a task and a buffer for as long as the peer keeps the connection. So
//! the handshake is covered by the shared accept path's per-stream timeout
//! and per-connection in-flight cap, and the bound ends the moment the
//! invocation is admitted.

use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};
use synch_core::{
    NodeId, RefuseCode, SockClosed, SockOpen, SockOpened, SockStatus, MAX_OPEN_FRAME_LEN,
};
use synch_sock::{Admission, DuplexStream};
use synch_store::Store;

use crate::{error::NetError, frame, serve::MAX_CONCURRENT_STREAMS};

/// Makes the control uni-stream observable before the first invocation ends.
/// QUIC does not announce an opened uni-stream to its receiver until bytes are
/// sent on it, so without this preamble both sides wait forever: the client for
/// `control()`, the server for an invocation whose status it could write.
const CONTROL_READY: &[u8] = b"sync/sock/control/1\0";

/// What the engine has to supply for this ALPN to serve anything.
///
/// Two calls rather than one, because the reply to an `Open` has to name the
/// content root that is about to run — so resolution and authorization finish
/// *before* the stream becomes the guest's, and an admission is what travels
/// between those two moments.
#[async_trait::async_trait]
pub trait SocketService: std::fmt::Debug + Send + Sync + 'static {
    /// Resolves and authorizes an `Open`, or says why not.
    async fn admit(
        &self,
        peer: NodeId,
        addr: String,
        stream_index: u64,
        open: &SockOpen,
    ) -> Result<Admission, (RefuseCode, String)>;

    /// Runs an admitted invocation to completion.
    ///
    /// `peer_gone` fires when the caller's connection closes. The invocation
    /// must end on it: the stream itself may never fail — after a clean FIN
    /// the runtime's reader has already exited, so a connection that closes
    /// afterwards leaves the guest's stream looking open — but the caller is
    /// gone all the same, and an invocation that keeps running is a slot held
    /// for nobody.
    async fn run(
        &self,
        admission: Admission,
        stream: DuplexStream,
        peer_gone: tokio::sync::oneshot::Receiver<SockStatus>,
    ) -> SockStatus;
}

/// Serves `sync/sock/1`.
#[derive(Clone)]
pub(crate) struct SockProtocol {
    store: Arc<Store>,
    service: Arc<dyn SocketService>,
    on_unknown_key: Option<Arc<tokio::sync::Notify>>,
    state: Arc<ProtocolState>,
    /// How long a stream may take to complete its `Open` handshake.
    ///
    /// The shared accept path's per-stream bound, applied to the handshake
    /// only: a stream that never becomes an invocation has no runtime of its
    /// own, and without this it owns a task and a buffer for as long as the
    /// peer keeps the connection. An admitted invocation runs unbounded —
    /// the socket runtime's own deadlines govern it.
    open_timeout: Duration,
    /// The endpoint-wide in-flight gate, shared with every other ALPN mounted
    /// on this endpoint (`crate::serve::Inflight`).
    ///
    /// Taken around the accept gate only, never around an admitted
    /// invocation. A socket invocation is *supposed* to be long-lived, so
    /// holding an endpoint-wide slot for its whole run would make one
    /// `--listen` client's open sessions a ceiling on every other peer's
    /// requests. What the gate is for is the unbounded thing — the store call
    /// an unauthenticated dialer reaches by completing a handshake.
    inflight: crate::serve::Inflight,
}

#[derive(Debug, Default)]
struct ProtocolState {
    stopping: AtomicBool,
    active_streams: AtomicUsize,
    changed: tokio::sync::Notify,
}

impl ProtocolState {
    fn stop(&self) {
        self.stopping.store(true, Ordering::Release);
        self.changed.notify_waiters();
    }

    fn is_stopping(&self) -> bool {
        self.stopping.load(Ordering::Acquire)
    }

    fn enter(self: &Arc<Self>) -> Option<ActiveStream> {
        if self.is_stopping() {
            return None;
        }
        self.active_streams.fetch_add(1, Ordering::AcqRel);
        if self.is_stopping() {
            self.leave();
            return None;
        }
        Some(ActiveStream {
            state: self.clone(),
        })
    }

    fn leave(&self) {
        if self.active_streams.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.changed.notify_waiters();
        }
    }

    async fn cancelled(&self) {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.is_stopping() {
                return;
            }
            changed.await;
        }
    }

    async fn drained(&self) {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.active_streams.load(Ordering::Acquire) == 0 {
                return;
            }
            changed.await;
        }
    }
}

#[derive(Debug)]
struct ActiveStream {
    state: Arc<ProtocolState>,
}

impl Drop for ActiveStream {
    fn drop(&mut self) {
        self.state.leave();
    }
}

impl std::fmt::Debug for SockProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SockProtocol")
    }
}

impl SockProtocol {
    /// Builds a handler over a store and a service.
    pub(crate) fn new(
        store: Arc<Store>,
        service: Arc<dyn SocketService>,
        open_timeout: Duration,
    ) -> Self {
        SockProtocol {
            store,
            service,
            on_unknown_key: None,
            state: Arc::new(ProtocolState::default()),
            open_timeout,
            inflight: None,
        }
    }

    /// Gates this handler's accept path on the endpoint-wide semaphore.
    pub(crate) fn inflight(mut self, gate: crate::serve::Inflight) -> Self {
        self.inflight = gate;
        self
    }

    /// Rings `wake` whenever a connection is refused for an unknown key (§3.4).
    pub(crate) fn on_unknown_key(mut self, wake: Option<Arc<tokio::sync::Notify>>) -> Self {
        self.on_unknown_key = wake;
        self
    }

    /// Refuses new socket streams and wakes incomplete handshakes.
    pub(crate) fn stop(&self) {
        self.state.stop();
    }

    /// Waits until every stream accepted before [`stop`](Self::stop) has
    /// delivered its final response or refusal.
    pub(crate) async fn drain(&self) {
        if self.state.is_stopping() {
            self.state.drained().await;
        }
    }
}

impl ProtocolHandler for SockProtocol {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let remote = connection.remote_id();

        if self.state.is_stopping() {
            connection.close(0u32.into(), b"shutdown");
            return Ok(());
        }

        // The same accept gate as the other two ALPNs — literally: the §3.2
        // rule is membership policy, and this file's own rationale for
        // `serve_connection` is "one implementation, because two drift".
        {
            let Ok(_admission) = crate::serve::slot(&self.inflight).await else {
                return Ok(());
            };
            crate::serve::admit(
                &self.store,
                &connection,
                &remote,
                self.on_unknown_key.as_ref(),
            )
            .await?;
        }

        // One uni-stream per connection, opened before anything is served, so
        // that a status always has somewhere to go. A trailer on the data
        // stream would cost a length prefix on every proxied byte, and a
        // RESET_STREAM would discard output the program had already written.
        let control = match connection.open_uni().await {
            Ok(mut stream) => {
                if let Err(e) = stream.write_all(CONTROL_READY).await {
                    tracing::debug!(peer = %remote.fmt_short(), "control preamble failed: {e}");
                    return Err(AcceptError::from_err(std::io::Error::other(e)));
                }
                Arc::new(tokio::sync::Mutex::new(stream))
            }
            Err(e) => {
                tracing::debug!(peer = %remote.fmt_short(), "no control stream: {e}");
                return Err(AcceptError::from_err(std::io::Error::other(e)));
            }
        };

        let mut index = 0u64;
        // The shared accept path's in-flight cap, scoped to handshakes. A
        // permit is held only until the `Open` is admitted: from then on the
        // stream is an invocation governed by the socket runtime's own
        // bounds, and a `--listen` client multiplexing many long-lived
        // invocations over one connection must not be capped by this layer.
        let handshake = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_STREAMS));
        loop {
            let accepted = tokio::select! {
                _ = self.state.cancelled() => break,
                accepted = connection.accept_bi() => accepted,
            };
            let Ok((mut send, mut recv)) = accepted else {
                break;
            };
            let Some(active) = self.state.enter() else {
                let _ = frame::write_frame(
                    &mut send,
                    &SockOpened::Refused {
                        code: RefuseCode::Busy,
                        message: "the node is shutting down".into(),
                    },
                )
                .await;
                let _ = send.finish();
                break;
            };
            // Per stream, not just per connection: a binding revoked
            // mid-session must stop the next invocation. A stream already
            // running is left alone — cutting it would be a partial write to
            // whatever the program is talking to.
            if !crate::serve::still_admitted(&self.store, &connection, &remote).await {
                break;
            }
            // A stream whose handshake never completes must not pile up
            // beyond the shared path's cap. The permit is taken before the
            // task, so the stream sits unread in the accept queue rather
            // than owning a task, once the cap is reached.
            let permit = tokio::select! {
                _ = self.state.cancelled() => break,
                permit = handshake.clone().acquire_owned() => match permit {
                    Ok(permit) => permit,
                    Err(_) => break,
                },
            };

            let stream_id = send.id().index();
            // The endpoint id rather than a socket address: iroh may be
            // carrying this connection over a relay or over any of several
            // paths, and `sy_peer_addr` says as much. What a program can rely
            // on is the device key, which is what authenticated the peer.
            let addr = remote.to_string();
            let handler = self.clone();
            let control = control.clone();
            let conn = connection.clone();
            let this_index = index;
            index += 1;

            tokio::spawn(async move {
                let _active = active;
                // The handshake is bounded: an `Open` that never arrives is
                // dropped after `open_timeout`, permit and all. What is
                // dropped is a stream that was never an invocation — nothing
                // is on the control stream for it, and nothing is owed.
                let admission = match tokio::time::timeout(
                    handler.open_timeout,
                    handler.open_stream(remote, addr, this_index, &mut send, &mut recv),
                )
                .await
                {
                    Ok(Some(admission)) => admission,
                    Ok(None) => return, // a refusal is already on the wire
                    Err(_) => {
                        tracing::debug!(
                            peer = %remote.fmt_short(),
                            "socket Open timed out; the stream never became an invocation"
                        );
                        return;
                    }
                };
                drop(permit);
                // The caller's connection closing must end the invocation
                // even when the stream itself never fails — after a clean
                // FIN the runtime's reader has already exited, so the
                // connection dying afterwards leaves the guest's stream
                // looking open, and nothing else would ever end it. The
                // watcher only sends: the invocation's ending status is the
                // same non-fault `Deadline` a failed stream produces, and the
                // receiver lives or dies with the run below.
                let (peer_gone_tx, peer_gone_rx) = tokio::sync::oneshot::channel();
                let watcher = tokio::spawn(async move {
                    let _ = conn.closed().await;
                    let _ = peer_gone_tx.send(SockStatus::Deadline);
                });
                let status = handler
                    .service
                    .run(admission, DuplexStream::new(recv, send), peer_gone_rx)
                    .await;
                watcher.abort();
                let mut control = control.lock().await;
                let _ = frame::write_frame(&mut control, &SockClosed { stream_id, status }).await;
            });
        }
        // Router treats the handler future as the lifetime of the connection.
        // Keep it alive until the detached stream tasks have written their
        // completion frames; returning here earlier closes the control stream
        // underneath them.
        if self.state.is_stopping() {
            self.state.drained().await;
        }
        Ok(())
    }
}

impl SockProtocol {
    /// The `Open` handshake: read the frame, admit, answer.
    ///
    /// Returns the admission, or `None` when the stream never became an
    /// invocation — a refusal is already on the wire in its own frame, and
    /// repeating it as a status would say the same thing twice in two
    /// vocabularies. This is the phase the caller's timeout and in-flight
    /// permit cover: a stream that never completes it has no runtime of its
    /// own, so it must not own a task for as long as the peer likes.
    async fn open_stream(
        &self,
        peer: NodeId,
        addr: String,
        index: u64,
        send: &mut iroh::endpoint::SendStream,
        recv: &mut iroh::endpoint::RecvStream,
    ) -> Option<Admission> {
        let open = match tokio::select! {
            _ = self.state.cancelled() => {
                let _ = frame::write_frame(
                    send,
                    &SockOpened::Refused {
                        code: RefuseCode::Busy,
                        message: "the node is shutting down".into(),
                    },
                ).await;
                let _ = send.finish();
                return None;
            }
            open = read_open(recv) => open,
        } {
            Ok(open) => open,
            Err(e) => {
                tracing::debug!(peer = %peer.fmt_short(), "bad socket Open: {e}");
                let _ = frame::write_frame(
                    send,
                    &SockOpened::Refused {
                        code: RefuseCode::NoSuchPath,
                        message: format!("malformed Open: {e}"),
                    },
                )
                .await;
                let _ = send.finish();
                return None;
            }
        };

        let admission = match tokio::select! {
            _ = self.state.cancelled() => {
                let _ = frame::write_frame(
                    send,
                    &SockOpened::Refused {
                        code: RefuseCode::Busy,
                        message: "the node is shutting down".into(),
                    },
                ).await;
                let _ = send.finish();
                return None;
            }
            admission = self.service.admit(peer, addr, index, &open) => admission,
        } {
            Ok(admission) => admission,
            Err((code, message)) => {
                tracing::debug!(
                    peer = %peer.fmt_short(),
                    socket = format!("{}/{}", open.space, open.path),
                    "socket refused: {} ({message})", code.as_str()
                );
                let _ = frame::write_frame(send, &SockOpened::Refused { code, message }).await;
                let _ = send.finish();
                return None;
            }
        };

        let accepted = SockOpened::Ok {
            program: admission.program_root,
            invocation: admission.id,
        };
        if frame::write_frame(send, &accepted).await.is_err() {
            return None;
        }
        Some(admission)
    }
}

/// Reads and validates the `Open` frame.
///
/// The frame bound is applied by the framing layer before the decode, so an
/// oversized `Open` never becomes an allocation. Validation runs before the
/// service sees it, so nothing downstream has to reason about a path with `..`
/// in it.
async fn read_open(recv: &mut iroh::endpoint::RecvStream) -> Result<SockOpen, NetError> {
    let bytes = frame::read_bounded(recv, MAX_OPEN_FRAME_LEN).await?;
    let open: SockOpen =
        postcard::from_bytes(&bytes).map_err(|e| NetError::Decode(format!("Open: {e}")))?;
    open.validate()
        .map_err(|e| NetError::Unexpected(e.to_string()))?;
    Ok(open)
}

/// The connecting side: one QUIC connection, one stream per invocation.
#[derive(Debug)]
pub struct SockClient {
    connection: Connection,
}

/// A live invocation on the caller's side.
#[derive(Debug)]
pub struct SockStream {
    /// The content root the callee says is running, so the caller can audit
    /// what it actually reached.
    pub program: synch_core::Hash,
    /// The callee's id for this invocation.
    pub invocation: u64,
    /// Bytes to the program.
    pub send: iroh::endpoint::SendStream,
    /// Bytes from the program.
    pub recv: iroh::endpoint::RecvStream,
}

/// Why an `Open` did not become an invocation.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{}: {message}", code.as_str())]
pub struct Refused {
    /// The machine-readable reason.
    pub code: RefuseCode,
    /// What the callee said about it.
    pub message: String,
}

impl SockClient {
    /// Wraps an established connection on this ALPN.
    pub fn new(connection: Connection) -> Self {
        SockClient { connection }
    }

    /// Opens one invocation.
    pub async fn open(&self, open: &SockOpen) -> Result<Result<SockStream, Refused>, NetError> {
        open.validate()
            .map_err(|e| NetError::Unexpected(e.to_string()))?;
        let (mut send, mut recv) = self
            .connection
            .open_bi()
            .await
            .map_err(|e| NetError::Unexpected(e.to_string()))?;
        frame::write_frame(&mut send, open).await?;

        let answer: SockOpened = frame::read_frame(&mut recv).await?;
        match answer {
            SockOpened::Ok {
                program,
                invocation,
            } => Ok(Ok(SockStream {
                program,
                invocation,
                send,
                recv,
            })),
            SockOpened::Refused { code, message } => Ok(Err(Refused { code, message })),
        }
    }

    /// Reads the next completed-invocation notice from the control stream.
    ///
    /// Best effort: a caller that only pipes bytes never has to touch this, and
    /// one that wants an exit status waits on it after its stream ends.
    pub async fn next_closed(
        &self,
        control: &mut iroh::endpoint::RecvStream,
    ) -> Result<SockClosed, NetError> {
        frame::read_frame(control).await
    }

    /// Accepts the callee's control uni-stream.
    pub async fn control(&self) -> Result<iroh::endpoint::RecvStream, NetError> {
        let mut control = self
            .connection
            .accept_uni()
            .await
            .map_err(|e| NetError::Unexpected(e.to_string()))?;
        let mut ready = [0u8; CONTROL_READY.len()];
        control.read_exact(&mut ready).await?;
        if ready != CONTROL_READY {
            return Err(NetError::Unexpected(
                "socket control stream has an invalid preamble".into(),
            ));
        }
        Ok(control)
    }

    /// The underlying connection, for callers that want to close it.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        endpoint::NetOptions,
        testing::{test_store, trusting_pair},
    };
    use synch_core::{Hash, OriginId};
    use synch_sock::{EffectivePolicy, HostError, ObjectInfo, PeerIdentity, SocketHost, SocketId};

    #[derive(Debug)]
    struct NoTree;

    #[async_trait::async_trait]
    impl SocketHost for NoTree {
        fn open(&self, _origin: Option<&str>, _path: &str) -> Result<ObjectInfo, HostError> {
            Err(HostError::NotFound)
        }

        fn open_root(&self, _root: &Hash) -> Result<ObjectInfo, HostError> {
            Err(HostError::NotFound)
        }

        fn list_page(
            &self,
            _prefix: &str,
            _start_after: Option<&str>,
            _limit: usize,
        ) -> Result<synch_sock::ListPage, HostError> {
            Err(HostError::NotFound)
        }

        async fn pread(&self, _root: Hash, _offset: u64, _len: u64) -> Result<Vec<u8>, HostError> {
            Err(HostError::NotFound)
        }
    }

    #[derive(Debug, Default)]
    struct ShutdownService {
        release: tokio::sync::Notify,
    }

    #[async_trait::async_trait]
    impl SocketService for ShutdownService {
        async fn admit(
            &self,
            peer: NodeId,
            addr: String,
            stream_index: u64,
            open: &SockOpen,
        ) -> Result<Admission, (RefuseCode, String)> {
            Ok(Admission {
                program: Arc::new(Vec::new()),
                program_root: Hash::EMPTY,
                socket: SocketId::new(&open.space, &open.path),
                peer: PeerIdentity {
                    origin: OriginId::Key(peer),
                    device_key: peer,
                    spaces: None,
                    addr,
                    stream_index,
                },
                policy: EffectivePolicy::default(),
                meta: open.meta.clone(),
                self_origin: open.origin.clone(),
                host: Arc::new(NoTree),
                id: 7,
                slot: None,
            })
        }

        async fn run(
            &self,
            _admission: Admission,
            _stream: DuplexStream,
            _peer_gone: tokio::sync::oneshot::Receiver<SockStatus>,
        ) -> SockStatus {
            self.release.notified().await;
            SockStatus::Shutdown
        }
    }

    #[tokio::test]
    async fn drain_flushes_shutdown_status_before_endpoint_close() {
        let (_server_dir, store) = test_store();
        let service = Arc::new(ShutdownService::default());
        let options = NetOptions {
            sockets: Some(service.clone()),
            ..NetOptions::loopback()
        };
        let (server, client, _client_dir) = trusting_pair(store, options).await;
        let socket = client.connect_sock(server.direct_addr()).await.unwrap();
        let open = SockOpen::new(OriginId::Key(server.id()), "code", "hold.sock", vec![]);
        let mut control = socket.control().await.unwrap();
        let stream = socket.open(&open).await.unwrap().unwrap();

        server.stop_socket_admission();
        service.release.notify_one();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            server.drain_socket_streams(),
        )
        .await
        .expect("accepted socket streams drain");

        let closed = socket.next_closed(&mut control).await.unwrap();
        assert_eq!(closed.status, SockStatus::Shutdown);
        assert_eq!(closed.stream_id, 0);

        drop(stream);
        drop(control);
        drop(socket);
        client.shutdown().await.unwrap();
        server.shutdown().await.unwrap();
    }
}
