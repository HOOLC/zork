//! Framed RPC, subscriptions and byte tunnels on the native control ALPN.
use crate::MAX_FRAME;
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
impl synch_net::ControlProtocol for ControlService {
    fn accept(
        &self,
        connection: Connection,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), AcceptError>> + Send>> {
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
            .alpns(vec![synch_net::ALPN_CONTROL.to_vec()])
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
