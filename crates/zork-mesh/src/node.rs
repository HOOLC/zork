//! Bounded product operations on an owned Synch engine node. No daemon, local
//! control protocol, command interpreter or control socket is involved.
mod folders;
mod tree;
use crate::{MAX_ARTIFACT, MAX_FRAME};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    time::Duration,
};
use synch_core::{NodeId, OriginId};
use synch_engine::{EngineError, Node, VersionPolicy};
use tokio::io::AsyncWriteExt;
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
}

#[derive(Clone, Debug)]
struct ActiveNode {
    node: Arc<RwLock<Option<Node>>>,
    alive: Arc<AtomicBool>,
    startup_publish: tokio::sync::mpsc::Sender<StartupPublication>,
    relay_endpoints: Arc<tokio::sync::Mutex<Vec<std::sync::Weak<synch_net::RelayAccess>>>>,
    control_connections: Arc<crate::control::Connections>,
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

/// One lost QUIC handshake must not fail the caller: refresh the peer
/// route and retry once with a fresh address before giving up.
const CONTROL_DIAL_TIMEOUT: Duration = Duration::from_secs(7);
const CONTROL_DIAL_ATTEMPTS: u32 = 2;

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
        control_connections: Arc<crate::control::Connections>,
    ) -> Self {
        Self {
            data_dir: node.config().data_dir.clone(),
            active: Arc::new(RwLock::new(Some(ActiveNode {
                node: Arc::new(RwLock::new(Some(node))),
                alive,
                startup_publish,
                relay_endpoints: Default::default(),
                control_connections,
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
    /// Whether handles track the same live transport ownership/generation.
    pub fn same_runtime(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.active, &other.active)
    }

    pub async fn identity(&self) -> Result<String> {
        Ok(self.engine()?.origin().to_string())
    }
    pub fn address(&self) -> Result<iroh::EndpointAddr> {
        Ok(self.engine()?.net().addr())
    }
    pub fn addresses(
        &self,
    ) -> Result<futures_util::stream::BoxStream<'static, iroh::EndpointAddr>> {
        use futures_util::StreamExt;
        use iroh::Watcher;
        Ok(self
            .engine()?
            .net()
            .endpoint()
            .watch_addr()
            .stream()
            .boxed())
    }
    pub fn routes(&self) -> Result<zork_config::membership::MeshRoutes> {
        let address = self.address()?;
        let mut direct: Vec<_> = address.ip_addrs().copied().collect();
        let mut relays: Vec<_> = address.relay_urls().map(ToString::to_string).collect();
        direct.sort();
        direct.dedup();
        relays.sort();
        relays.dedup();
        let routes = zork_config::membership::MeshRoutes { direct, relays };
        routes.validate()?;
        Ok(routes)
    }
    pub async fn remember_routes(
        &self,
        origin: &str,
        routes: &zork_config::membership::MeshRoutes,
    ) -> Result<()> {
        routes.validate()?;
        let key = NodeId::from_z32(origin.strip_prefix("key:").context("key origin required")?)?;
        let mut address = iroh::EndpointAddr::new(key);
        for addr in &routes.direct {
            address = address.with_ip_addr(*addr);
        }
        for relay in &routes.relays {
            address = address.with_relay_url(relay.parse()?);
        }
        self.remember_peer_address(origin, serde_json::to_value(address)?)
            .await
    }

    /// The peer's currently cached transport address, if the endpoint knows one.
    pub async fn peer_address(&self, origin: &str) -> Result<Option<iroh::EndpointAddr>> {
        let key = NodeId::from_z32(origin.strip_prefix("key:").context("key origin required")?)?;
        self.blocking(move |node| Ok(node.peer_addr(&key)?)).await
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
    /// Retry only connection setup, before submitting any business request.
    pub async fn connect_control(&self, origin: &str) -> Result<crate::control::ControlStream> {
        let key = NodeId::from_z32(origin.strip_prefix("key:").context("key origin required")?)?;
        let mut last_error = None;
        for attempt in 0..CONTROL_DIAL_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_secs(1)).await;
                self.refresh_peer_route(origin).await?;
            }
            let address = self
                .peer_address(origin)
                .await?
                .unwrap_or_else(|| iroh::EndpointAddr::new(key));
            let dial = async {
                let connection = self.engine()?.net().connect_control(address).await?;
                let (send, recv) = connection.open_bi().await?;
                let mut stream = crate::control::ControlStream::new(connection.clone(), send, recv);
                match tokio::time::timeout(Duration::from_secs(2), stream.establish()).await {
                    Ok(Ok(())) => Ok(stream),
                    failure => {
                        // Close this precise session, never a newer concurrent dial.
                        // Its cache entry will be discarded by Net::live on retry.
                        connection.close(408u32.into(), b"control stream unavailable");
                        match failure {
                            Ok(Err(error)) => Err(error),
                            Err(error) => Err(anyhow::Error::new(error)
                                .context("control stream handshake timeout")),
                            Ok(Ok(())) => unreachable!(),
                        }
                    }
                }
            };
            match tokio::time::timeout(CONTROL_DIAL_TIMEOUT, dial).await {
                Ok(Ok(stream)) => return Ok(stream),
                Ok(Err(error)) => last_error = Some(error),
                Err(error) => last_error = Some(error.into()),
            }
        }
        Err(last_error.context("control dial failed")?).context("无法连接设备：已重新查询当前地址")
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
        if let Some(active) = self.active.read().unwrap().as_ref() {
            active.control_connections.revoke(origin);
        }
        let key = NodeId::from_z32(origin.strip_prefix("key:").context("key origin required")?)?;
        self.engine()?.net().disconnect(&key);
        let origin: OriginId = origin.parse()?;
        self.blocking(move |node| {
            node.store().remove_origin_bindings(&origin)?;
            Ok(())
        })
        .await
    }
    /// Account lifecycle updates only matching relay origins, on this runtime
    /// and its enrollment endpoints. No account credential reaches membership.
    pub async fn set_relay_access(&self, origin: &str, access_token: Option<&str>) -> Result<()> {
        let node = self.engine()?;
        let endpoints = self
            .active
            .read()
            .expect("Mesh node")
            .as_ref()
            .context("Mesh node closed")?
            .relay_endpoints
            .clone();
        let mut endpoints = endpoints.lock().await;
        node.net().set_relay_access(origin, access_token).await?;
        endpoints.retain(|endpoint| endpoint.strong_count() > 0);
        for endpoint in endpoints.iter().filter_map(std::sync::Weak::upgrade) {
            endpoint.set(origin, access_token).await?;
        }
        Ok(())
    }

    pub(crate) async fn register_relay_endpoint(
        &self,
        endpoint: &Arc<synch_net::RelayAccess>,
    ) -> Result<()> {
        let node = self.engine()?;
        let endpoints = self
            .active
            .read()
            .expect("Mesh node")
            .as_ref()
            .context("Mesh node closed")?
            .relay_endpoints
            .clone();
        let mut endpoints = endpoints.lock().await;
        endpoint.inherit(node.net().relay_access()).await?;
        endpoints.retain(|endpoint| endpoint.strong_count() > 0);
        endpoints.push(Arc::downgrade(endpoint));
        Ok(())
    }

    pub async fn is_trusted(&self, origin: &str) -> Result<bool> {
        let key = NodeId::from_z32(origin.strip_prefix("key:").context("key origin required")?)?;
        self.blocking(move |node| Ok(node.store().is_trusted_key(&key, synch_core::now_ns())?))
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
        sender
            .send(StartupPublication {
                space: space.into(),
            })
            .await
            .context("Mesh publisher stopped")
    }
    pub async fn publish(&self) -> Result<()> {
        tokio::time::timeout(CALL_TIMEOUT, self.engine()?.scan_publish_push()).await??;
        Ok(())
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
            socket.finish()?;
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
            let local = matches!(item.provenance(), "zork_loopback" | "mdns" | "zork_lan");
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
        let mut stream = self.subscribe(origin, payload).await?;
        let reply = tokio::time::timeout(Duration::from_secs(25), stream.next())
            .await??
            .context("missing tunnel response")?;
        ensure!(
            reply["v"] == 1 && reply["ok"] == true,
            "{}",
            reply["error"].as_str().unwrap_or("service unavailable")
        );
        Ok(ServiceStream(stream))
    }

    pub async fn subscribe(
        &self,
        origin: &str,
        payload: &serde_json::Value,
    ) -> Result<Subscription> {
        // Validation precedes the dial. Once writing starts, never replay a
        // request whose remote acceptance may be unknown.
        ensure!(
            serde_json::to_vec(payload)?.len() <= MAX_FRAME,
            "mesh request too large"
        );
        let mut stream = self.connect_control(origin).await?;
        tokio::time::timeout(Duration::from_secs(25), stream.write(payload)).await??;
        Ok(stream)
    }
}

pub struct ServiceStream(Subscription);
impl ServiceStream {
    pub async fn forward(mut self, local: &mut tokio::net::TcpStream, prefix: &[u8]) -> Result<()> {
        self.0.write_all(prefix).await?;
        self.0.splice(local).await
    }
}

pub type Subscription = crate::control::ControlStream;

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
