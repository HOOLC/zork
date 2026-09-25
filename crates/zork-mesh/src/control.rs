//! Framed RPC, subscriptions and byte tunnels on the native control ALPN.
//!
//! The ALPN carries the control framing version. Any incompatible framing
//! change must bump [`CONTROL_VERSION`] (and so the ALPN): peers then fail the
//! TLS handshake at once instead of timing out on bytes they cannot parse, and
//! [`VERSION_ALPN`] tells the dialer which side has to upgrade.
use crate::MAX_FRAME;
/// Control framing version spoken by this build.
pub const CONTROL_VERSION: u32 = 2;
pub(crate) const ALPN: &[u8] = b"zork-control/2";
/// Framing used before the prefaced control streams. The same ALPN name
/// survived that change, so such peers are refused explicitly, never served.
pub(crate) const LEGACY_ALPN: &[u8] = b"zork-control/1";
/// Stable forever: answers which control versions this device speaks.
pub(crate) const VERSION_ALPN: &[u8] = b"zork-version";
const LEGACY_CLOSE: u32 = 426;
const STREAM_PREFACE: &[u8; 4] = b"ZC01";
use anyhow::{ensure, Context, Result};
use iroh::{
    endpoint::{Connection, RecvStream, SendStream},
    protocol::AcceptError,
    EndpointId,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context as TaskContext, Poll},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

/// Identity authenticated by QUIC, never by the request payload.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Peer {
    pub origin: String,
    pub device_key: String,
}
impl From<EndpointId> for Peer {
    fn from(key: EndpointId) -> Self {
        Self {
            origin: format!("key:{}", key.to_z32()),
            device_key: key
                .as_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        }
    }
}
pub fn key_origin(hex: &str) -> Result<String> {
    ensure!(
        hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid device key"
    );
    let mut bytes = [0; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16)?;
    }
    Ok(Peer::from(EndpointId::from_bytes(&bytes)?).origin)
}

pub enum Reply {
    Once(Value),
    Subscription(tokio::sync::mpsc::Receiver<Value>),
    Tunnel {
        upstream: tokio::net::TcpStream,
        cancelled: tokio::sync::watch::Receiver<bool>,
        guard: Option<Box<dyn Send>>,
    },
}

/// One bidirectional stream. Frame reads retain partial data across cancellation;
/// dropping it cancels this request without closing other streams.
#[derive(Debug)]
pub struct ControlStream {
    connection: Connection,
    send: SendStream,
    recv: RecvStream,
    buffer: Vec<u8>,
    closed: bool,
}
impl ControlStream {
    pub(crate) fn new(connection: Connection, send: SendStream, recv: RecvStream) -> Self {
        Self {
            connection,
            send,
            recv,
            buffer: Vec::new(),
            closed: false,
        }
    }
    /// QUIC can still consider a connection live after the peer was killed.
    /// No business bytes are sent until this stream has a current peer waiter.
    pub(crate) async fn establish(&mut self) -> Result<()> {
        self.send.write_all(STREAM_PREFACE).await?;
        let mut reply = [0; 4];
        self.recv.read_exact(&mut reply).await?;
        ensure!(
            &reply == STREAM_PREFACE,
            "unsupported native control stream"
        );
        Ok(())
    }
    async fn accept_preface(&mut self) -> Result<()> {
        let mut request = [0; 4];
        self.recv.read_exact(&mut request).await?;
        ensure!(
            &request == STREAM_PREFACE,
            "unsupported native control stream"
        );
        self.send.write_all(STREAM_PREFACE).await?;
        Ok(())
    }
    pub async fn write(&mut self, value: &Value) -> Result<()> {
        let bytes = serde_json::to_vec(value)?;
        ensure!(
            !bytes.is_empty() && bytes.len() <= MAX_FRAME,
            "control message too large"
        );
        self.send
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .await?;
        self.send.write_all(&bytes).await?;
        self.send.flush().await?;
        Ok(())
    }
    pub fn finish(&mut self) -> Result<()> {
        self.send.finish()?;
        Ok(())
    }
    pub fn routes(
        &self,
    ) -> futures_util::stream::BoxStream<'static, crate::route::ConnectionRoute> {
        crate::route::observe(Some(self.connection.clone()))
    }
    pub async fn read(&mut self) -> Result<Option<Value>> {
        self.next().await
    }
    pub async fn next(&mut self) -> Result<Option<Value>> {
        loop {
            if self.closed {
                return Ok(None);
            }
            if self.buffer.len() >= 4 {
                let len = u32::from_be_bytes(self.buffer[..4].try_into()?) as usize;
                ensure!(len > 0 && len <= MAX_FRAME, "invalid control frame size");
                if self.buffer.len() >= len + 4 {
                    let value = serde_json::from_slice(&self.buffer[4..len + 4])?;
                    self.buffer.drain(..len + 4);
                    return Ok(Some(value));
                }
            }
            let mut bytes = [0; 8192];
            let read = AsyncReadExt::read(&mut self.recv, &mut bytes).await?;
            if read == 0 {
                ensure!(self.buffer.is_empty(), "incomplete control frame");
                self.closed = true;
                return Ok(None);
            }
            self.buffer.extend_from_slice(&bytes[..read]);
        }
    }
    pub async fn splice(mut self, upstream: &mut tokio::net::TcpStream) -> Result<()> {
        tokio::io::copy_bidirectional(&mut self, upstream).await?;
        Ok(())
    }
}
impl AsyncRead for ControlStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        // The response and first tunnel bytes can arrive together.
        if !self.buffer.is_empty() {
            let count = self.buffer.len().min(buf.remaining());
            buf.put_slice(&self.buffer[..count]);
            self.buffer.drain(..count);
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.recv).poll_read(cx, buf)
    }
}
impl AsyncWrite for ControlStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.send)
            .poll_write(cx, buf)
            .map_err(std::io::Error::other)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.send).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.send).poll_shutdown(cx)
    }
}

/// Inbound control sessions close when the local runtime revokes a peer.
#[derive(Debug, Default)]
pub(crate) struct Connections {
    next: std::sync::atomic::AtomicU64,
    sessions: std::sync::Mutex<std::collections::HashMap<(String, u64), Connection>>,
}
struct ConnectionLease {
    owner: Arc<Connections>,
    key: (String, u64),
}
impl Drop for ConnectionLease {
    fn drop(&mut self) {
        self.owner.sessions.lock().unwrap().remove(&self.key);
    }
}
impl Connections {
    fn register(self: &Arc<Self>, peer: String, connection: Connection) -> ConnectionLease {
        let key = (
            peer,
            self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        );
        self.sessions
            .lock()
            .unwrap()
            .insert(key.clone(), connection);
        ConnectionLease {
            owner: self.clone(),
            key,
        }
    }
    pub(crate) fn revoke(&self, origin: &str) {
        let sessions: Vec<_> = {
            let mut sessions = self.sessions.lock().unwrap();
            let keys: Vec<_> = sessions
                .keys()
                .filter(|(peer, _)| peer == origin)
                .cloned()
                .collect();
            keys.into_iter()
                .filter_map(|key| sessions.remove(&key))
                .collect()
        };
        for connection in sessions {
            connection.close(403u32.into(), b"access revoked");
        }
    }
}

pub trait ControlHandler: Send + Sync + std::fmt::Debug + 'static {
    fn serve(
        &self,
        peer: Peer,
        request: Value,
        stream: ControlStream,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>;
}
#[derive(Debug)]
pub(crate) struct ControlService {
    handler: Arc<dyn ControlHandler>,
    slots: Arc<tokio::sync::Semaphore>,
    connections: Arc<Connections>,
}
impl ControlService {
    pub(crate) fn new(handler: Arc<dyn ControlHandler>, connections: Arc<Connections>) -> Self {
        Self {
            handler,
            connections,
            slots: Arc::new(tokio::sync::Semaphore::new(128)),
        }
    }
}
impl iroh::protocol::ProtocolHandler for ControlService {
    fn accept(
        &self,
        connection: Connection,
    ) -> impl std::future::Future<Output = Result<(), AcceptError>> + Send {
        let handler = self.handler.clone();
        let slots = self.slots.clone();
        let connections = self.connections.clone();
        Box::pin(async move {
            let peer = Peer::from(connection.remote_id());
            let _lease = connections.register(peer.origin.clone(), connection.clone());
            let mut requests = tokio::task::JoinSet::new();
            loop {
                let streams = tokio::select! {
                    streams = connection.accept_bi() => streams,
                    _ = requests.join_next(), if !requests.is_empty() => continue,
                };
                let Ok((send, recv)) = streams else { break };
                let Ok(permit) = slots.clone().try_acquire_owned() else {
                    continue;
                };
                let handler = handler.clone();
                let peer = peer.clone();
                let connection = connection.clone();
                requests.spawn(async move {
                    let _permit = permit;
                    let cancelled = send.stopped();
                    let mut stream = ControlStream::new(connection, send, recv);
                    let serve = async {
                        let request = tokio::time::timeout(Duration::from_secs(25), async {
                            stream.accept_preface().await?;
                            stream.read().await?.context("missing control request")
                        }).await.context("control request header timeout")??;
                        handler.serve(peer, request, stream).await
                    };
                    tokio::select! {
                        _ = cancelled => {},
                        result = serve => {
                            if let Err(error) = result { tracing::debug!(%error, "control request ended"); }
                        }
                    }
                });
            }
            requests.shutdown().await;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stale_live_session_is_replaced_before_any_business_request_is_sent() -> Result<()> {
        let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
            .clear_address_lookup()
            .relay_mode(iroh::RelayMode::Disabled)
            .clear_ip_transports()
            .bind_addr("127.0.0.1:0")?
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await?;
        let accepted = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(AtomicUsize::new(0));
        let server = endpoint.clone();
        let connections = accepted.clone();
        let received = requests.clone();
        let task = tokio::spawn(async move {
            let mut tasks = tokio::task::JoinSet::new();
            while let Some(incoming) = server.accept().await {
                let index = connections.fetch_add(1, Ordering::SeqCst);
                let received = received.clone();
                tasks.spawn(async move {
                    let connection = incoming.await?;
                    let mut streams = 0;
                    while let Ok((send, recv)) = connection.accept_bi().await {
                        let mut stream = ControlStream::new(connection.clone(), send, recv);
                        if index == 0 && streams > 0 {
                            let mut preface = [0; 4];
                            stream.recv.read_exact(&mut preface).await?;
                            ensure!(
                                &preface == STREAM_PREFACE,
                                "business bytes preceded readiness"
                            );
                            let mut byte = [0];
                            ensure!(
                                tokio::time::timeout(
                                    Duration::from_secs(1),
                                    stream.recv.read(&mut byte)
                                )
                                .await
                                .is_err(),
                                "business bytes were sent to the stale session"
                            );
                            connection.closed().await;
                            break;
                        }
                        streams += 1;
                        stream.accept_preface().await?;
                        let request = stream.next().await?.context("request")?;
                        received.fetch_add(1, Ordering::SeqCst);
                        stream.write(&request).await?;
                        stream.finish()?;
                    }
                    Ok::<_, anyhow::Error>(())
                });
            }
            while let Some(result) = tasks.join_next().await {
                result??;
            }
            Ok::<_, anyhow::Error>(())
        });
        let root = tempfile::tempdir()?;
        let mut runtime = crate::managed::start_client(
            root.path(),
            &zork_config::MeshConfig {
                enabled: true,
                offline: true,
                bind: Some("127.0.0.1:0".into()),
                ..Default::default()
            },
        )
        .await?;
        let node = runtime.node();
        let origin = format!("key:{}", endpoint.id().to_z32());
        node.trust(&origin, "fixture", None).await?;
        node.remember_peer_address(&origin, serde_json::to_value(endpoint.addr())?)
            .await?;
        for value in [serde_json::json!({"id":1}), serde_json::json!({"id":2})] {
            let reply =
                tokio::time::timeout(Duration::from_secs(12), node.exchange(&origin, &value))
                    .await??;
            ensure!(reply == value, "reply mismatch");
        }
        ensure!(
            requests.load(Ordering::SeqCst) == 2,
            "business request replayed"
        );
        ensure!(
            accepted.load(Ordering::SeqCst) == 2,
            "cached session was not replaced"
        );
        runtime.shutdown().await?;
        endpoint.close().await;
        task.await??;
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct DeviceHandler {
    pub node: crate::node::MeshNode,
    pub business: Option<Arc<dyn ControlHandler>>,
    pub objects: Arc<tokio::sync::Semaphore>,
}
impl ControlHandler for DeviceHandler {
    fn serve(
        &self,
        peer: Peer,
        request: Value,
        mut stream: ControlStream,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> {
        let node = self.node.clone();
        let business = self.business.clone();
        let objects = self.objects.clone();
        Box::pin(async move {
            // Installation confirms a pinned invitation before ordinary trust exists.
            // The Station validates its one-use capability and the authenticated peer.
            let confirms_invitation =
                request["v"] == 1 && request["request"]["kind"] == "confirm_join";
            if !confirms_invitation && !node.is_trusted(&peer.origin).await? {
                stream
                    .write(&serde_json::json!({"v":1,"ok":false,"error":"mesh_peer_not_paired"}))
                    .await?;
                stream.finish()?;
                return Ok(());
            }
            if request["v"] == 1 && request["request"]["kind"] == "object" {
                let _permit = objects
                    .try_acquire_owned()
                    .context("attachment capacity reached")?;
                let object: crate::node::ObjectRef =
                    serde_json::from_value(request["request"]["object"].clone())?;
                ensure!(
                    object.origin == node.identity().await?,
                    "object origin mismatch"
                );
                let bytes = node.local_object(&object).await?;
                ensure!(node.is_trusted(&peer.origin).await?, "mesh_peer_not_paired");
                stream.write(&serde_json::json!({"v":1,"ok":true})).await?;
                stream.write_all(&bytes).await?;
                stream.finish()?;
                Ok(())
            } else {
                business
                    .context("device does not host a Station")?
                    .serve(peer, request, stream)
                    .await
            }
        })
    }
}

/// The peer runs an incompatible Zork build; retrying cannot help.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionMismatch {
    PeerOlder,
    PeerNewer,
}
impl std::fmt::Display for VersionMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::PeerOlder => "对方设备的 Zork 版本过旧，无法连接（upgrade_required: peer_older）。请升级对方设备后重试",
            Self::PeerNewer => "对方设备的 Zork 版本较新，无法连接（upgrade_required: peer_newer）。请升级本机 Zork 后重试",
        })
    }
}
impl std::error::Error for VersionMismatch {}

/// Answers version probes. It never serves requests and never needs trust:
/// the reply only lists protocol versions.
#[derive(Debug, Clone)]
pub(crate) struct VersionService;
impl iroh::protocol::ProtocolHandler for VersionService {
    fn accept(
        &self,
        connection: Connection,
    ) -> impl std::future::Future<Output = Result<(), AcceptError>> + Send {
        async move {
            let _ = tokio::time::timeout(Duration::from_secs(5), async {
                let mut send = connection.open_uni().await?;
                send.write_all(&serde_json::to_vec(
                    &serde_json::json!({"control":[CONTROL_VERSION]}),
                )?)
                .await?;
                send.finish()?;
                connection.closed().await;
                Ok::<_, anyhow::Error>(())
            })
            .await;
            Ok(())
        }
    }
}

/// Refuses dialers that speak the old framing under the old ALPN, promptly
/// and with a readable reason, instead of letting them hang.
#[derive(Debug, Clone)]
pub(crate) struct LegacyRefusal;
impl iroh::protocol::ProtocolHandler for LegacyRefusal {
    fn accept(
        &self,
        connection: Connection,
    ) -> impl std::future::Future<Output = Result<(), AcceptError>> + Send {
        async move {
            connection.close(
                LEGACY_CLOSE.into(),
                "对方设备的 Zork 版本较新，请升级本机 Zork (upgrade_required)".as_bytes(),
            );
            Ok(())
        }
    }
}

/// After a failed control dial, find out whether the peer is reachable but
/// speaks another control version. `None` means "not a version problem".
pub(crate) async fn probe_version(
    endpoint: &iroh::Endpoint,
    address: iroh::EndpointAddr,
) -> Option<VersionMismatch> {
    let probe = async {
        match endpoint.connect(address.clone(), VERSION_ALPN).await {
            Ok(connection) => {
                let reply = async {
                    let mut recv = connection.accept_uni().await?;
                    let bytes = recv.read_to_end(1024).await?;
                    Ok::<_, anyhow::Error>(serde_json::from_slice::<Value>(&bytes)?)
                }
                .await;
                connection.close(0u32.into(), b"version known");
                let versions: Vec<u32> = reply
                    .ok()?
                    .get("control")?
                    .as_array()?
                    .iter()
                    .filter_map(|v| v.as_u64().map(|v| v as u32))
                    .collect();
                if versions.is_empty() || versions.contains(&CONTROL_VERSION) {
                    None
                } else if versions.iter().all(|v| *v < CONTROL_VERSION) {
                    Some(VersionMismatch::PeerOlder)
                } else {
                    Some(VersionMismatch::PeerNewer)
                }
            }
            // Builds without the version probe predate it; a successful legacy
            // handshake identifies them.
            Err(_) => match endpoint.connect(address, LEGACY_ALPN).await {
                Ok(connection) => {
                    connection.close(0u32.into(), b"version known");
                    Some(VersionMismatch::PeerOlder)
                }
                Err(_) => None,
            },
        }
    };
    tokio::time::timeout(Duration::from_secs(4), probe)
        .await
        .ok()
        .flatten()
}

#[cfg(test)]
mod version_tests {
    use super::*;

    async fn fixture(alpns: Vec<Vec<u8>>) -> Result<iroh::Endpoint> {
        Ok(iroh::Endpoint::builder(iroh::endpoint::presets::N0)
            .clear_address_lookup()
            .relay_mode(iroh::RelayMode::Disabled)
            .clear_ip_transports()
            .bind_addr("127.0.0.1:0")?
            .alpns(alpns)
            .bind()
            .await?)
    }
    async fn client(root: &std::path::Path) -> Result<crate::managed::Runtime> {
        crate::managed::start_client(
            root,
            &zork_config::MeshConfig {
                enabled: true,
                offline: true,
                bind: Some("127.0.0.1:0".into()),
                ..Default::default()
            },
        )
        .await
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn old_peer_fails_fast_with_an_upgrade_message() -> Result<()> {
        // An old node: only the legacy control ALPN, no version probe.
        let old = fixture(vec![LEGACY_ALPN.to_vec()]).await?;
        let server = old.clone();
        let accept = tokio::spawn(async move {
            while let Some(incoming) = server.accept().await {
                if let Ok(connection) = incoming.await {
                    tokio::spawn(async move { connection.closed().await });
                }
            }
        });
        let root = tempfile::tempdir()?;
        let mut runtime = client(root.path()).await?;
        let node = runtime.node();
        let origin = format!("key:{}", old.id().to_z32());
        node.trust(&origin, "old", None).await?;
        node.remember_peer_address(&origin, serde_json::to_value(old.addr())?)
            .await?;
        let started = std::time::Instant::now();
        let error = node
            .exchange(&origin, &serde_json::json!({"v":1}))
            .await
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<VersionMismatch>(),
            Some(&VersionMismatch::PeerOlder),
            "{error:#}"
        );
        assert!(error.to_string().contains("版本过旧"));
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "no dial timeout"
        );
        runtime.shutdown().await?;
        accept.abort();
        old.close().await;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn newer_peer_is_reported_through_the_version_probe() -> Result<()> {
        let newer = fixture(vec![VERSION_ALPN.to_vec(), b"zork-control/9".to_vec()]).await?;
        let server = newer.clone();
        let accept = tokio::spawn(async move {
            while let Some(incoming) = server.accept().await {
                if let Ok(connection) = incoming.await {
                    tokio::spawn(async move {
                        if let Ok(mut send) = connection.open_uni().await {
                            let _ = send.write_all(br#"{"control":[9]}"#).await;
                            let _ = send.finish();
                        }
                        connection.closed().await
                    });
                }
            }
        });
        let root = tempfile::tempdir()?;
        let mut runtime = client(root.path()).await?;
        let node = runtime.node();
        let origin = format!("key:{}", newer.id().to_z32());
        node.trust(&origin, "newer", None).await?;
        node.remember_peer_address(&origin, serde_json::to_value(newer.addr())?)
            .await?;
        let error = node
            .exchange(&origin, &serde_json::json!({"v":1}))
            .await
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<VersionMismatch>(),
            Some(&VersionMismatch::PeerNewer),
            "{error:#}"
        );
        assert!(error.to_string().contains("升级本机"));
        runtime.shutdown().await?;
        accept.abort();
        newer.close().await;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn current_node_refuses_legacy_dialers_and_answers_probes() -> Result<()> {
        let root = tempfile::tempdir()?;
        let mut runtime = client(root.path()).await?;
        let node = runtime.node();
        let address = node.address()?;
        let dialer = fixture(vec![]).await?;
        // Old client: closed at once with the upgrade reason, not left hanging.
        let legacy = dialer.connect(address.clone(), LEGACY_ALPN).await?;
        let reason = tokio::time::timeout(Duration::from_secs(5), legacy.closed()).await?;
        assert!(format!("{reason}").contains("upgrade_required"), "{reason}");
        // The probe reports this build's version, so a peer can tell which side is old.
        assert_eq!(probe_version(&dialer, address).await, None);
        runtime.shutdown().await?;
        dialer.close().await;
        Ok(())
    }
}
