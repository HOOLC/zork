//! Bounded product operations on an owned Synch engine node. No daemon, local
//! control protocol, command interpreter or control socket is involved.
mod folders;
mod relay;
mod tree;
use crate::{MAX_ARTIFACT, MAX_FRAME};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    time::Duration,
};
use synch_core::{NodeId, OriginId, SockStatus};
use synch_engine::{EngineError, Node, VersionPolicy};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
pub use tree::{
    TreeCatalog, TreeChunk, TreeCursor, TreeEntry, TreePage, TreeQuery, TreeSpace, TreeVersion,
};

const CALL_TIMEOUT: Duration = Duration::from_secs(30);

fn artifact_timeout(bytes: u64) -> Duration {
    CALL_TIMEOUT + Duration::from_secs(bytes.div_ceil(256 * 1024))
}

#[derive(Debug)]
pub(crate) struct StartupPublication {
    pub space: String,
    pub flush: bool,
    pub done: tokio::sync::oneshot::Sender<Result<()>>,
}

#[derive(Clone, Debug)]
struct ActiveNode {
    node: Arc<RwLock<Option<Node>>>,
    alive: Arc<AtomicBool>,
    startup_publish: tokio::sync::mpsc::Sender<StartupPublication>,
    relay_configs: Arc<crate::relay_access::RelayAccess>,
}

/// A direct library handle. Desktop clients can retain this handle across a
/// network-settings restart; the host explicitly rebinds it to its new node.
#[derive(Clone, Debug)]
pub struct MeshNode {
    data_dir: PathBuf,
    active: Arc<RwLock<Option<ActiveNode>>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ObjectRef {
    pub origin: String,
    pub space: String,
    pub path: String,
    pub root: String,
    pub size: u64,
}
impl ObjectRef {
    pub fn verify(&self, bytes: &[u8]) -> Result<()> {
        let root: blake3::Hash = self.root.parse().context("invalid content root")?;
        ensure!(
            self.size <= MAX_ARTIFACT as u64
                && bytes.len() as u64 == self.size
                && blake3::hash(bytes) == root,
            "object integrity mismatch"
        );
        Ok(())
    }
}

impl MeshNode {
    /// An unbound handle performs no I/O; the owner must attach its running node.
    pub fn unbound(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            active: Arc::new(RwLock::new(None)),
        }
    }
    pub(crate) fn from_engine(
        node: Node,
        alive: Arc<AtomicBool>,
        startup_publish: tokio::sync::mpsc::Sender<StartupPublication>,
    ) -> Self {
        Self {
            data_dir: node.config().data_dir.clone(),
            active: Arc::new(RwLock::new(Some(ActiveNode {
                node: Arc::new(RwLock::new(Some(node))),
                alive,
                startup_publish,
                relay_configs: Arc::default(),
            }))),
        }
    }
    pub(crate) fn release_engine(&self) {
        if let Some(active) = self.active.read().expect("Mesh node").as_ref() {
            active.node.write().expect("Mesh engine").take();
        }
    }
    pub fn attach(&self, other: &Self) -> Result<()> {
        ensure!(
            self.data_dir == other.data_dir,
            "cannot attach a different Mesh identity directory"
        );
        let active = other.active.read().expect("Mesh node").clone();
        *self.active.write().expect("Mesh node") = active;
        Ok(())
    }
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
    pub fn is_running(&self) -> bool {
        self.engine().is_ok()
    }
    fn engine(&self) -> Result<Node> {
        let guard = self.active.read().expect("Mesh node");
        let active = guard.as_ref().context("Mesh node is not running")?;
        ensure!(
            active.alive.load(Ordering::Acquire),
            "Mesh node is shutting down"
        );
        let node = active
            .node
            .read()
            .expect("Mesh engine")
            .clone()
            .context("Mesh node is closed")?;
        Ok(node)
    }
    pub(crate) async fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce(Node) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let node = self.engine()?;
        tokio::task::spawn_blocking(move || {
            let _scope = synch_core::BlockingScope::enter();
            work(node)
        })
        .await
        .context("Mesh store task failed")?
    }
    pub async fn identity(&self) -> Result<String> {
        Ok(self.engine()?.origin().to_string())
    }
    pub fn address(&self) -> Result<iroh::EndpointAddr> {
        Ok(self.engine()?.net().addr())
    }
    pub async fn trust(&self, origin: &str, note: &str, addr: Option<&str>) -> Result<()> {
        let key = NodeId::from_z32(origin.strip_prefix("key:").context("key origin required")?)?;
        let note = note.to_owned();
        let addr = addr.map(str::parse::<std::net::SocketAddr>).transpose()?;
        self.blocking(move |node| {
            node.trust_add(key, Some(&note))?;
            if let Some(addr) = addr {
                node.remember_peer(&iroh::EndpointAddr::new(key).with_ip_addr(addr))?;
            }
            Ok(())
        })
        .await
    }
    /// Addresses obtained over the invitation's pinned encrypted endpoint.
    pub async fn remember_peer_address(
        &self,
        origin: &str,
        address: serde_json::Value,
    ) -> Result<()> {
        let address: iroh::EndpointAddr = serde_json::from_value(address)?;
        let key = NodeId::from_z32(origin.strip_prefix("key:").context("key origin required")?)?;
        ensure!(
            address.id == key && address.addrs.len() <= 24,
            "peer_address_identity_mismatch"
        );
        for relay in address.relay_urls() {
            zork_config::services::validate_endpoint(relay.as_str())?;
        }
        self.blocking(move |node| {
            node.remember_peer(&address)?;
            Ok(())
        })
        .await
    }
    pub async fn untrust(&self, origin: &str) -> Result<()> {
        let origin: OriginId = origin.parse()?;
        self.blocking(move |node| {
            node.store().remove_origin_bindings(&origin)?;
            Ok(())
        })
        .await
    }
    pub async fn grant_delegation(
        &self,
        key: &str,
        spaces: Vec<String>,
        ttl: Duration,
        note: &str,
    ) -> Result<()> {
        let key = NodeId::from_z32(key)?;
        let until =
            synch_core::now_ns().saturating_add(ttl.as_nanos().min(i64::MAX as u128) as i64);
        let note = note.to_owned();
        self.blocking(move |node| {
            let change = node.delegate_add(key, &spaces, until, Some(&note))?;
            node.publish(&[change])?;
            Ok(())
        })
        .await
    }
    pub async fn revoke_delegation(&self, key: &str) -> Result<()> {
        let key = NodeId::from_z32(key)?;
        self.blocking(move |node| {
            match node.delegate_remove(&key) {
                Ok(change) => {
                    node.publish(&[change])?;
                }
                Err(EngineError::NotFound(_)) => {}
                Err(error) => return Err(error.into()),
            }
            Ok(())
        })
        .await
    }
    pub async fn add_api_source(&self, space: &str) -> Result<()> {
        let space = space.to_owned();
        self.blocking(move |node| Ok(node.add_api_source(&space)?))
            .await
    }
    pub async fn add_filesystem_source(&self, space: &str, path: &Path) -> Result<()> {
        let (space, path) = (space.to_owned(), path.to_owned());
        self.blocking(move |node| Ok(node.add_filesystem_source(&space, path)?))
            .await
    }
    /// Retire a publication role without touching the source's files.
    pub async fn retire_source(&self, space: &str) -> Result<()> {
        let space = space.to_owned();
        self.blocking(move |node| {
            if node.store().source(&space)?.is_some() {
                let changes = node.source_removal(&space)?;
                node.publish(&changes)?;
                node.finish_source_removal(&space)?;
            }
            Ok(())
        })
        .await
    }
    /// Queue the initial filesystem scan in the owned runtime. Adding a source
    /// only arms Synch's watcher; an unchanged directory may produce no hint.
    pub async fn schedule_source_scan(&self, space: &str) -> Result<()> {
        let sender = self
            .active
            .read()
            .expect("Mesh node")
            .as_ref()
            .context("Mesh node is not running")?
            .startup_publish
            .clone();
        let (done, _) = tokio::sync::oneshot::channel();
        sender
            .send(StartupPublication {
                space: space.into(),
                flush: false,
                done,
            })
            .await
            .context("Mesh publisher stopped")
    }
    pub async fn activate_bridge(&self, token: String, max_streams: u32) -> Result<()> {
        self.blocking(move |node| {
            Ok(node.socket_activate(&synch_store::SocketActivation {
                space: "zork-control".into(),
                path: "mesh.sock".into(),
                config: vec![("bridge_token".into(), token)],
                max_streams: Some(max_streams),
                note: "Zork private Mesh ingress".into(),
                activated_at: synch_core::now_ns(),
            })?)
        })
        .await
    }
    pub async fn publish(&self) -> Result<()> {
        tokio::time::timeout(CALL_TIMEOUT, self.engine()?.scan_publish_push()).await??;
        Ok(())
    }
    /// Commit the startup bridge locally before readiness. Peer delivery belongs
    /// to the owned runtime: an unreachable peer must not delay local startup.
    pub(crate) async fn publish_for_startup(
        &self,
        space: &str,
        path: &str,
        content: blake3::Hash,
    ) -> Result<()> {
        let sender = self
            .active
            .read()
            .expect("Mesh node")
            .as_ref()
            .context("Mesh node is not running")?
            .startup_publish
            .clone();
        let (done, mut completed) = tokio::sync::oneshot::channel();
        // Upstream exposes the local commit through its SQLite authority, while
        // flush_staged also waits for network push. Observe only this database
        // during startup so an unreachable peer cannot delay local readiness.
        let database = self.blocking(|node| Ok(node.store().db_path())).await?;
        let mut wal = database.as_os_str().to_owned();
        wal.push("-wal");
        let wal = std::path::PathBuf::from(wal);
        let commits = zork_notify::files::Source::new([database, wal])?;
        let mut changes = commits.subscribe();
        let (space, path) = (space.to_owned(), path.to_owned());
        tokio::time::timeout(CALL_TIMEOUT, async {
            sender
                .send(StartupPublication {
                    space: space.clone(),
                    flush: true,
                    done,
                })
                .await
                .context("Mesh publisher stopped")?;
            let mut finished = false;
            loop {
                changes.checkpoint();
                let (space, path) = (space.clone(), path.clone());
                let committed = self
                    .blocking(move |node| {
                        Ok(node
                            .resolve(&space, &path, &VersionPolicy::Origin(node.origin().clone()))
                            .is_ok_and(|entry| entry.content == Some(content.into())))
                    })
                    .await?;
                if committed {
                    return Ok(());
                }
                ensure!(!finished, "startup Mesh entry was not published");
                tokio::select! {
                    result = &mut completed => {
                        result.context("Mesh publisher stopped")??;
                        // A completed scan must have made this exact version
                        // visible; never report readiness for a skipped file.
                        finished = true;
                    }
                    change = changes.changed() => { change?; }
                }
            }
        })
        .await
        .context("local Mesh publication timeout")?
    }
    pub async fn pin(&self, object: &ObjectRef) -> Result<()> {
        ensure!(
            object.size <= MAX_ARTIFACT as u64,
            "artifact exceeds 300 MiB"
        );
        self.engine()?
            .pin_object(&object.root.parse()?, Some(object.size))
            .await?;
        Ok(())
    }
    pub async fn put(&self, space: &str, path: &str, bytes: &[u8]) -> Result<ObjectRef> {
        ensure!(bytes.len() <= MAX_ARTIFACT, "artifact exceeds 300 MiB");
        let node = self.engine()?;
        tokio::time::timeout(artifact_timeout(bytes.len() as u64), async {
            // Scratch lives with the node and is removed even on cancellation.
            let scratch = tempfile::Builder::new()
                .prefix("zork-put-")
                .tempfile_in(&self.data_dir)?;
            tokio::fs::write(scratch.path(), bytes).await?;
            let (root, size) = node
                .commit_api_file(space, path, scratch.path(), synch_core::now_ns())
                .await?;
            node.scan_publish_push().await?;
            let object = ObjectRef {
                origin: node.origin().to_string(),
                space: space.into(),
                path: path.into(),
                root: root.to_string(),
                size,
            };
            object.verify(bytes)?;
            Ok(object)
        })
        .await
        .context("Mesh publication timeout")?
    }
    pub async fn read(&self, object: &ObjectRef) -> Result<Vec<u8>> {
        ensure!(
            object.size <= MAX_ARTIFACT as u64,
            "artifact exceeds 300 MiB"
        );
        let expected: synch_core::Hash = object.root.parse()?;
        let node = self.engine()?;
        let policy = VersionPolicy::Origin(object.origin.parse()?);
        tokio::time::timeout(artifact_timeout(object.size), async {
            let range = node
                .prepare_range(&object.space, &object.path, &policy, 0, Some(object.size))
                .await;
            let range = match range {
                Err(EngineError::NotFound(_)) => {
                    // An authenticated RPC can name its newly published object
                    // before the background tree sync arrives. Catch up with
                    // that owner; a general round may choose a different peer.
                    if let Some(key) = object.origin.strip_prefix("key:") {
                        node.sync_with_peer(&NodeId::from_z32(key)?).await?;
                    } else {
                        node.anti_entropy_round().await?;
                    }
                    node.prepare_range(&object.space, &object.path, &policy, 0, Some(object.size))
                        .await?
                }
                result => result?,
            };
            ensure!(
                range.root == expected && range.len() == object.size,
                "object integrity mismatch"
            );
            let bytes = node
                .cas_backend()
                .read_range(range.root, range.start, range.len())
                .await?;
            object.verify(&bytes)?;
            Ok(bytes)
        })
        .await
        .context("Mesh read timeout")?
    }
    pub async fn exchange(
        &self,
        origin: &str,
        payload: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let started = std::time::Instant::now();
        let result = tokio::time::timeout(CALL_TIMEOUT, async {
            let mut socket = self.subscribe(origin, payload).await?;
            socket.send.shutdown().await?;
            let value = socket.next().await?.context("missing Mesh response")?;
            ensure!(socket.next().await?.is_none(), "multiple Mesh responses");
            Ok(value)
        })
        .await;
        let result = match result {
            Ok(result) => result,
            Err(error) => Err(anyhow::Error::new(error).context("Mesh exchange timeout")),
        };
        let path = payload["request"]["path"]
            .as_str()
            .unwrap_or("")
            .split('?')
            .next()
            .unwrap_or("");
        if let Err(error) = &result {
            tracing::warn!(peer = origin, path, elapsed_ms = started.elapsed().as_millis(), error = %error, "Mesh exchange failed");
        }
        result
    }
    /// Refresh routing hints before opening a new socket. A cached address may
    /// name a UDP port from before the remote process restarted. Discovery
    /// cannot grant access: the socket handshake still pins the trusted key.
    async fn refresh_peer_route(&self, origin: &str) -> Result<()> {
        use futures_util::StreamExt;
        let key = NodeId::from_z32(origin.strip_prefix("key:").context("key origin required")?)?;
        let node = self.engine()?;
        let network = node.net();
        let Ok(lookup) = network.endpoint().address_lookup() else {
            return Ok(());
        };
        let lookup = lookup.clone();
        // A fast cached DNS/DHT answer can still name the old UDP port after
        // a restart. Do not cancel slower live LAN/loopback discovery as soon
        // as that first hint arrives. Keep relay-only answers as well.
        let deadline = tokio::time::Instant::now() + Duration::from_millis(1200);
        let stream = lookup.resolve(key);
        futures_util::pin_mut!(stream);
        let mut found = iroh::EndpointAddr::new(key);
        while let Ok(Some(result)) = tokio::time::timeout_at(deadline, stream.next()).await {
            let Ok(Ok(item)) = result else { continue };
            let local = matches!(item.provenance(), "zork_loopback" | "mdns");
            let address = item.into_endpoint_addr();
            if address.id != key {
                continue;
            }
            if local && address.ip_addrs().next().is_some() {
                // A current local advertisement supersedes cached remote hints.
                found = address;
                break;
            }
            found.addrs.extend(address.addrs);
        }
        let found = (!found.addrs.is_empty()).then_some(found);
        if let Some(address) = found {
            self.blocking(move |node| {
                node.remember_peer(&address)?;
                Ok(())
            })
            .await?;
        }
        Ok(())
    }
    /// One authenticated control handshake, followed by an unframed service stream.
    pub async fn connect_service(&self, origin: &str, id: &str) -> Result<ServiceStream> {
        self.connect_tunnel(
            origin,
            &serde_json::json!({"v":1,"request":{"kind":"service","id":id}}),
        )
        .await
    }

    /// Open a typed, peer-authorized tunnel; the remote handler selects its endpoint.
    pub async fn connect_tunnel(
        &self,
        origin: &str,
        payload: &serde_json::Value,
    ) -> Result<ServiceStream> {
        self.refresh_peer_route(origin).await?;
        let connection = tokio::time::timeout(
            Duration::from_secs(10),
            self.engine()?
                .connect_socket(&origin.parse()?, "zork-control", "mesh.sock", vec![]),
        )
        .await??;
        let mut socket = Subscription::new(connection);
        tokio::time::timeout(Duration::from_secs(25), async {
            let request = serde_json::to_vec(payload)?;
            ensure!(
                !request.is_empty() && request.len() <= MAX_FRAME,
                "mesh request too large"
            );
            socket.send.write_u32(request.len() as u32).await?;
            socket.send.write_all(&request).await?;
            let reply = crate::bridge::read_frame(&mut socket.recv).await?;
            ensure!(
                reply["ok"] == true,
                "{}",
                reply["error"].as_str().unwrap_or("service unavailable")
            );
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        Ok(ServiceStream(socket))
    }

    pub async fn subscribe(
        &self,
        origin: &str,
        payload: &serde_json::Value,
    ) -> Result<Subscription> {
        let bytes = serde_json::to_vec(payload)?;
        ensure!(
            !bytes.is_empty() && bytes.len() <= MAX_FRAME,
            "mesh request too large"
        );
        self.refresh_peer_route(origin).await?;
        let node = self.engine()?;
        use tracing::Instrument;
        let attempt = ulid::Ulid::new();
        let started = std::time::Instant::now();
        let span = tracing::debug_span!("mesh_socket", %attempt, peer = origin);
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            node.connect_socket(&origin.parse()?, "zork-control", "mesh.sock", vec![])
                .instrument(span),
        )
        .await;
        tracing::debug!(%attempt, elapsed_ms = started.elapsed().as_millis(), timed_out = result.is_err(), "Mesh socket open completed");
        let connection = result
            .context("连接设备超时：已重新查询当前地址")?
            .context("无法建立设备连接")?;
        let mut socket = Subscription::new(connection);
        socket
            .send
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .await?;
        socket.send.write_all(&bytes).await?;
        socket.send.flush().await?;
        Ok(socket)
    }
}

/// Owns the Synch invocation and control stream for the entire TCP forwarding lifetime.
pub struct ServiceStream(Subscription);
impl ServiceStream {
    pub async fn forward(mut self, local: &mut tokio::net::TcpStream, prefix: &[u8]) -> Result<()> {
        self.0.send.write_all(prefix).await?;
        let (mut read, mut write) = local.split();
        tokio::try_join!(
            async {
                tokio::io::copy(&mut read, &mut self.0.send).await?;
                self.0.send.shutdown().await
            },
            async {
                tokio::io::copy(&mut self.0.recv, &mut write).await?;
                write.shutdown().await
            }
        )?;
        Ok(())
    }
}

/// Framed and backpressured direct Synch streams. Dropping the stream releases
/// the QUIC connection (or cancels the local invocation). Reads are cancel-safe.
pub struct Subscription {
    connection: Option<iroh::endpoint::Connection>,
    send: Box<dyn AsyncWrite + Unpin + Send>,
    recv: Box<dyn AsyncRead + Unpin + Send>,
    completion: Pin<Box<dyn Future<Output = Result<SockStatus>> + Send>>,
    buffer: Vec<u8>,
    closed: bool,
}
impl Subscription {
    fn new(connection: synch_engine::sockets::SocketConnection) -> Self {
        use synch_engine::sockets::SocketConnection;
        let route_connection = match &connection {
            SocketConnection::Remote { client, .. } => Some(client.connection().clone()),
            SocketConnection::Local { .. } => None,
        };
        let (send, recv, completion): (
            _,
            _,
            Pin<Box<dyn Future<Output = Result<SockStatus>> + Send>>,
        ) = match connection {
            SocketConnection::Remote {
                client,
                mut control,
                stream,
            } => (
                Box::new(stream.send) as Box<dyn AsyncWrite + Unpin + Send>,
                Box::new(stream.recv) as Box<dyn AsyncRead + Unpin + Send>,
                Box::pin(async move {
                    let _client = client;
                    Ok(
                        synch_net::frame::read_frame::<synch_core::SockClosed>(&mut control)
                            .await?
                            .status,
                    )
                }),
            ),
            SocketConnection::Local {
                stream, completion, ..
            } => {
                let (recv, send) = tokio::io::split(stream);
                (
                    Box::new(send) as Box<dyn AsyncWrite + Unpin + Send>,
                    Box::new(recv) as Box<dyn AsyncRead + Unpin + Send>,
                    Box::pin(async move { Ok(completion.await?) }),
                )
            }
        };
        Self {
            connection: route_connection,
            send,
            recv,
            completion,
            buffer: vec![],
            closed: false,
        }
    }
    pub fn routes(
        &self,
    ) -> futures_util::stream::BoxStream<'static, crate::route::ConnectionRoute> {
        crate::route::observe(self.connection.clone())
    }

    pub async fn next(&mut self) -> Result<Option<serde_json::Value>> {
        loop {
            if self.closed {
                return Ok(None);
            }
            if self.buffer.len() >= 4 {
                let len = u32::from_be_bytes(self.buffer[..4].try_into()?) as usize;
                ensure!(len > 0 && len <= MAX_FRAME, "invalid Mesh frame");
                if self.buffer.len() >= len + 4 {
                    let value = serde_json::from_slice(&self.buffer[4..len + 4])?;
                    self.buffer.drain(..len + 4);
                    return Ok(Some(value));
                }
            }
            let mut bytes = [0u8; 8192];
            let read = self.recv.read(&mut bytes).await?;
            if read == 0 {
                ensure!(self.buffer.is_empty(), "incomplete Mesh frame");
                let status = self.completion.as_mut().await?;
                self.closed = true;
                ensure!(status.exit_code() == 0, "Mesh socket closed: {status:?}");
                return Ok(None);
            }
            ensure!(
                self.buffer.len() + read <= MAX_FRAME + 4 + bytes.len(),
                "Mesh frame too large"
            );
            self.buffer.extend_from_slice(&bytes[..read]);
        }
    }
}

#[cfg(test)]
mod route_recovery_tests {
    use super::*;
    #[derive(Debug)]
    struct DelayedRoute {
        address: iroh::EndpointAddr,
        source: &'static str,
        delay: Duration,
    }
    impl iroh::address_lookup::AddressLookup for DelayedRoute {
        fn resolve(
            &self,
            id: iroh::EndpointId,
        ) -> Option<
            futures_util::stream::BoxStream<
                'static,
                std::result::Result<iroh::address_lookup::Item, iroh::address_lookup::Error>,
            >,
        > {
            if id != self.address.id {
                return None;
            }
            let address = self.address.clone();
            let source = self.source;
            let delay = self.delay;
            Some(Box::pin(futures_util::stream::once(async move {
                tokio::time::sleep(delay).await;
                Ok(iroh::address_lookup::Item::new(
                    address.into(),
                    source,
                    None,
                ))
            })))
        }
    }

    #[tokio::test]
    async fn cached_discovery_cannot_hide_later_lan_address_after_restart() -> Result<()> {
        let root = tempfile::tempdir()?;
        let config = zork_config::MeshConfig {
            enabled: true,
            offline: true,
            ..Default::default()
        };
        let mut runtime = crate::managed::start_client(root.path(), &config).await?;
        let client = runtime.node();
        let key = iroh::SecretKey::generate().public();
        let origin = format!("key:{}", key.to_z32());
        client
            .trust(&origin, "test peer", Some("127.0.0.1:12345"))
            .await?;
        let engine = client.engine()?;
        let lookup = engine.net().endpoint().address_lookup().unwrap().clone();
        lookup.add(DelayedRoute {
            address: iroh::EndpointAddr::new(key).with_ip_addr("127.0.0.1:12345".parse()?),
            source: "cached-dns",
            delay: Duration::ZERO,
        });
        lookup.add(DelayedRoute {
            address: iroh::EndpointAddr::new(key).with_ip_addr("127.0.0.1:23456".parse()?),
            source: "mdns",
            delay: Duration::from_millis(25),
        });
        client.refresh_peer_route(&origin).await?;
        let found = client
            .blocking(move |node| Ok(node.peer_addr(&key)?.unwrap()))
            .await?;
        assert_eq!(
            found.ip_addrs().copied().collect::<Vec<_>>(),
            vec!["127.0.0.1:23456".parse()?]
        );
        runtime.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn relay_only_discovery_replaces_stale_direct_route() -> Result<()> {
        let root = tempfile::tempdir()?;
        let config = zork_config::MeshConfig {
            enabled: true,
            offline: true,
            ..Default::default()
        };
        let mut runtime = crate::managed::start_client(root.path(), &config).await?;
        let client = runtime.node();
        let key = iroh::SecretKey::generate().public();
        let origin = format!("key:{}", key.to_z32());
        client
            .trust(&origin, "test peer", Some("127.0.0.1:12345"))
            .await?;
        let expected =
            iroh::EndpointAddr::new(key).with_relay_url("https://relay.example.test/".parse()?);
        client
            .engine()?
            .net()
            .endpoint()
            .address_lookup()
            .unwrap()
            .add(DelayedRoute {
                address: expected.clone(),
                source: "cached-dns",
                delay: Duration::ZERO,
            });
        client.refresh_peer_route(&origin).await?;
        let found = client
            .blocking(move |node| Ok(node.peer_addr(&key)?.unwrap()))
            .await?;
        assert_eq!(found, expected);
        runtime.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn fresh_discovery_replaces_a_cached_port_without_changing_trust() -> Result<()> {
        let root = tempfile::tempdir()?;
        let config = zork_config::MeshConfig {
            enabled: true,
            offline: true,
            ..Default::default()
        };
        let mut runtime = crate::managed::start_client(root.path(), &config).await?;
        let client = runtime.node();
        let key = iroh::SecretKey::generate().public();
        let origin = format!("key:{}", key.to_z32());
        client
            .trust(&origin, "test peer", Some("127.0.0.1:12345"))
            .await?;
        let lookup = iroh::address_lookup::MemoryLookup::new();
        lookup.set_endpoint_info(
            iroh::EndpointAddr::new(key).with_ip_addr("127.0.0.1:23456".parse()?),
        );
        client
            .engine()?
            .net()
            .endpoint()
            .address_lookup()
            .unwrap()
            .add(lookup.clone());
        client.refresh_peer_route(&origin).await?;
        let first = client
            .blocking(move |node| Ok(node.peer_addr(&key)?.unwrap()))
            .await?;
        assert_eq!(
            first.ip_addrs().copied().collect::<Vec<_>>(),
            vec!["127.0.0.1:23456".parse()?]
        );
        lookup.set_endpoint_info(
            iroh::EndpointAddr::new(key).with_ip_addr("127.0.0.1:34567".parse()?),
        );
        client.refresh_peer_route(&origin).await?;
        let second = client
            .blocking(move |node| Ok(node.peer_addr(&key)?.unwrap()))
            .await?;
        assert_eq!(
            second.ip_addrs().copied().collect::<Vec<_>>(),
            vec!["127.0.0.1:34567".parse()?]
        );
        runtime.shutdown().await?;
        Ok(())
    }
}
