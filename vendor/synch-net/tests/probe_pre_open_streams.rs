//! Probe: pre-Open streams on `sync/sock/1` are bounded (finding 6, fixed
//! `2026-08-28`).
//!
//! Before the fix, the sock ALPN re-implemented the accept loop without the
//! guards the shared path (`serve::serve_connection`) applies to the other
//! ALPNs: a stream that announced an `Open` frame's length and then sent
//! nothing parked a task and a buffer with no timeout and no in-flight
//! bound — counted nowhere, held for as long as the peer kept the
//! connection.
//!
//! The fix applies both guards to the one phase they are right for, the
//! handshake: a per-connection in-flight cap (`MAX_CONCURRENT_STREAMS`, as
//! on the shared path) and a per-stream timeout (`sockets_open_timeout`,
//! defaulting to the shared path's `STREAM_TIMEOUT`). The bound ends the
//! moment the `Open` is admitted — an invocation runs under the socket
//! runtime's own deadlines, and a `--listen` client multiplexing long-lived
//! invocations over one connection is not capped by this layer.
//!
//! Asserted fixed behavior: half-Open streams are dropped by the callee
//! after the handshake timeout — the client's recv halves reach EOF — and
//! a complete `Open` on a fresh stream is still answered, because the cap
//! and the timeout never touch admitted invocations.

#![cfg(all(
    any(target_os = "linux", target_os = "macos", target_os = "openbsd"),
    any(target_arch = "x86_64", target_arch = "aarch64")
))]

use std::sync::Arc;
use std::time::Duration;

use iroh_base::SecretKey;
use synch_core::{RefuseCode, SockOpen, SockOpened, SockStatus, ALPN_SOCK};
use synch_net::endpoint::{Net, NetOptions};
use synch_net::sock::SocketService;
use synch_sock::{
    Admission, DuplexStream, EffectivePolicy, HostError, ObjectInfo, PeerIdentity, SocketHost,
    SocketId,
};

#[derive(Debug)]
struct NoTree;

#[async_trait::async_trait]
impl SocketHost for NoTree {
    fn open(&self, _origin: Option<&str>, _path: &str) -> Result<ObjectInfo, HostError> {
        Err(HostError::NotFound)
    }
    fn open_root(&self, _root: &synch_core::Hash) -> Result<ObjectInfo, HostError> {
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
    async fn pread(
        &self,
        _root: synch_core::Hash,
        _offset: u64,
        _len: u64,
    ) -> Result<Vec<u8>, HostError> {
        Err(HostError::NotFound)
    }
}

/// What an admitted invocation's run came to, for the test to observe.
#[derive(Debug, Default)]
struct OutcomeRecorder {
    last: std::sync::Mutex<Option<SockStatus>>,
    done: tokio::sync::Notify,
}

/// Admits everything; `run` parks until the peer-gone signal, standing in
/// for the engine forwarding the connection-close into the socket runtime.
/// The probe is about streams that never complete — and about the one
/// complete stream that must end when its connection closes.
#[derive(Debug)]
struct InstantService {
    recorded: Arc<OutcomeRecorder>,
}

#[async_trait::async_trait]
impl SocketService for InstantService {
    async fn admit(
        &self,
        peer: synch_core::NodeId,
        addr: String,
        stream_index: u64,
        open: &SockOpen,
    ) -> Result<Admission, (RefuseCode, String)> {
        Ok(Admission {
            program: Arc::new(Vec::new()),
            program_root: synch_core::Hash::EMPTY,
            socket: SocketId::new(&open.space, &open.path),
            peer: PeerIdentity {
                origin: synch_core::OriginId::Key(peer),
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
        peer_gone: tokio::sync::oneshot::Receiver<SockStatus>,
    ) -> SockStatus {
        // Park until the invocation would end: this service stands in for the
        // engine, which forwards the signal into the socket runtime.
        let status = match peer_gone.await {
            Ok(status) => status,
            Err(_) => SockStatus::Ok(0),
        };
        *self.recorded.last.lock().unwrap() = Some(status);
        self.recorded.done.notify_one();
        status
    }
}

fn trust(store: &synch_store::Store, key: synch_core::NodeId) {
    store
        .put_binding(&synch_store::Binding {
            origin: synch_core::OriginId::Key(key),
            node_id: key,
            source: synch_store::BindingSource::Static,
            domain: None,
            issuer: None,
            spaces: Vec::new(),
            note: None,
            added_at: 0,
            expires_at: None,
        })
        .expect("a static binding");
}

#[tokio::test]
async fn half_open_streams_are_dropped_after_the_handshake_timeout() {
    const N: usize = 12;
    // Shorter than the default so the probe runs in seconds, not minutes.
    const OPEN_TIMEOUT: Duration = Duration::from_secs(1);

    let server_dir = tempfile::tempdir().unwrap();
    let server_store = Arc::new(synch_store::Store::open(server_dir.path()).unwrap());
    let server_secret = SecretKey::generate();
    let client_secret = SecretKey::generate();
    trust(&server_store, client_secret.public());
    let client_dir = tempfile::tempdir().unwrap();
    let client_store = Arc::new(synch_store::Store::open(client_dir.path()).unwrap());
    trust(&client_store, server_secret.public());

    let recorded = Arc::new(OutcomeRecorder::default());
    let server = Net::bind(
        server_store,
        server_secret,
        NetOptions {
            sockets: Some(Arc::new(InstantService {
                recorded: recorded.clone(),
            })),
            sockets_open_timeout: Some(OPEN_TIMEOUT),
            ..NetOptions::loopback()
        },
    )
    .await
    .unwrap();
    let client = Net::bind(client_store, client_secret, NetOptions::loopback())
        .await
        .unwrap();

    let connection = client
        .endpoint()
        .connect(server.direct_addr(), ALPN_SOCK)
        .await
        .expect("the socket ALPN connection opens");

    // N streams announce a 9216-byte Open frame (MAX_OPEN_FRAME_LEN
    // territory) and then send nothing more. The send halves are held so the
    // streams stay half-open from the client's point of view; the callee is
    // the one that must give up on them.
    let mut recv_halves = Vec::new();
    for _ in 0..N {
        let (mut send, recv) = connection.open_bi().await.expect("a bi-stream opens");
        send.write_all(&9216u32.to_le_bytes()).await.unwrap();
        std::mem::forget(send);
        recv_halves.push(recv);
    }

    // Give every handshake its timeout, plus slack for the in-flight cap to
    // work through the batches (8 permits, 12 streams).
    tokio::time::sleep(OPEN_TIMEOUT * 3 + Duration::from_secs(1)).await;

    // The callee must have dropped every half-open stream: each recv half
    // reaches EOF (read returns 0) or an error. Before the fix, all N were
    // still parked with no bound in sight.
    let mut dropped = 0usize;
    for mut recv in recv_halves {
        let mut buf = [0u8; 4];
        match tokio::time::timeout(OPEN_TIMEOUT, recv.read(&mut buf)).await {
            Ok(Ok(None)) => dropped += 1, // clean EOF: the callee ended the stream
            Ok(Ok(Some(_))) => {}         // unexpected data
            Ok(Err(_)) => dropped += 1,   // reset: the callee ended the stream
            Err(_) => {}                  // still open: the callee never gave up
        }
    }
    assert_eq!(
        dropped, N,
        "BREAK: {N} pre-Open streams were announced and left half-finished; the callee \
         dropped only {dropped} of them after the {OPEN_TIMEOUT:?} handshake timeout — \
         streams that never become invocations must not own tasks and buffers unbounded"
    );

    // A complete Open on a fresh stream is still answered: the cap and the
    // timeout never touch admitted invocations.
    let (mut send, mut recv) = connection.open_bi().await.expect("a fresh bi-stream opens");
    let open = SockOpen::new(
        synch_core::OriginId::Key(server.id()),
        "code",
        "hold.sock",
        vec![],
    );
    synch_net::frame::write_frame(&mut send, &open)
        .await
        .unwrap();
    let _ = send.finish();
    let answered = tokio::time::timeout(
        Duration::from_secs(3),
        synch_net::frame::read_frame::<SockOpened>(&mut recv),
    )
    .await
    .expect("a complete Open must still be answered after the timeout batch")
    .expect("the Open frame decodes");
    match answered {
        SockOpened::Ok { .. } => {}
        other => panic!("the fresh Open was not admitted: {other:?}"),
    }
}

/// The P1 propagation: closing the caller's connection must reach the
/// invocation even when the stream itself never fails. The service's `run`
/// stands in for the engine and reports what the peer-gone channel delivered
/// — a clean FIN would not fire it; the connection closing does.
#[tokio::test]
async fn connection_closure_signals_the_invocation() {
    let server_dir = tempfile::tempdir().unwrap();
    let server_store = Arc::new(synch_store::Store::open(server_dir.path()).unwrap());
    let server_secret = SecretKey::generate();
    let client_secret = SecretKey::generate();
    trust(&server_store, client_secret.public());
    let client_dir = tempfile::tempdir().unwrap();
    let client_store = Arc::new(synch_store::Store::open(client_dir.path()).unwrap());
    trust(&client_store, server_secret.public());

    let recorded = Arc::new(OutcomeRecorder::default());
    let server = Net::bind(
        server_store,
        server_secret,
        NetOptions {
            sockets: Some(Arc::new(InstantService {
                recorded: recorded.clone(),
            })),
            ..NetOptions::loopback()
        },
    )
    .await
    .unwrap();
    let client = Net::bind(client_store, client_secret, NetOptions::loopback())
        .await
        .unwrap();

    // One complete Open; the invocation parks in the service's run, which
    // awaits the peer-gone channel.
    let connection = client
        .endpoint()
        .connect(server.direct_addr(), ALPN_SOCK)
        .await
        .expect("the socket ALPN connection opens");
    let (mut send, mut recv) = connection.open_bi().await.expect("a bi-stream opens");
    let open = SockOpen::new(
        synch_core::OriginId::Key(server.id()),
        "code",
        "hold.sock",
        vec![],
    );
    synch_net::frame::write_frame(&mut send, &open)
        .await
        .unwrap();
    let _ = send.finish();
    let answered = tokio::time::timeout(
        Duration::from_secs(3),
        synch_net::frame::read_frame::<SockOpened>(&mut recv),
    )
    .await
    .expect("the Open is admitted")
    .expect("the Open frame decodes");
    assert!(matches!(answered, SockOpened::Ok { .. }));

    // The caller's connection closes. The stream itself never failed (the
    // caller's side finished cleanly), so nothing but the connection signal
    // can end the invocation — and it must arrive with the Deadline the
    // watcher sends.
    client.shutdown().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), recorded.done.notified())
        .await
        .expect(
            "BREAK: the connection closed but the invocation never learned — the \
             peer-gone signal did not reach the service's run",
        );
    let status = recorded
        .last
        .lock()
        .unwrap()
        .expect("the service recorded no ending");
    assert_eq!(
        status,
        SockStatus::Deadline,
        "connection closure must reach the invocation as Deadline, got {status:?}"
    );
}
