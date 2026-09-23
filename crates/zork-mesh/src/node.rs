//! Authenticated iroh transport and immutable attachment objects.
use crate::{MAX_ARTIFACT, MAX_FRAME};
use anyhow::{ensure, Context, Result};
use iroh::{Endpoint, EndpointAddr, EndpointId as NodeId};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio::io::AsyncWriteExt;
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
const CONTROL_DIAL_TIMEOUT: Duration = Duration::from_secs(7);
const CONTROL_DIAL_ATTEMPTS: u32 = 2;
fn artifact_timeout(bytes: u64) -> Duration {
    CALL_TIMEOUT + Duration::from_secs(bytes.div_ceil(256 * 1024))
}
#[derive(Debug)]
pub(crate) struct ActiveNode {
    pub endpoint: Endpoint,
    pub peers: RwLock<std::collections::BTreeMap<String, EndpointAddr>>,
    pub trusted: RwLock<std::collections::HashSet<String>>,
    pub connections: Arc<crate::control::Connections>,
    pub dialed: tokio::sync::Mutex<std::collections::HashMap<String, iroh::endpoint::Connection>>,
}
#[derive(Clone, Debug)]
pub struct MeshNode {
    data_dir: PathBuf,
    active: Arc<RwLock<Option<Arc<ActiveNode>>>>,
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
    pub fn unbound(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            active: Default::default(),
        }
    }
    pub(crate) fn from_endpoint(data_dir: PathBuf, active: ActiveNode) -> Self {
        Self {
            data_dir,
            active: Arc::new(RwLock::new(Some(Arc::new(active)))),
        }
    }
    pub(crate) fn close(&self) {
        self.active.write().unwrap().take();
    }
    fn runtime(&self) -> Result<Arc<ActiveNode>> {
        let active = self
            .active
            .read()
            .unwrap()
            .clone()
            .context("Mesh node is not running")?;
        ensure!(!active.endpoint.is_closed(), "Mesh node is closed");
        Ok(active)
    }
    pub fn attach(&self, other: &Self) -> Result<()> {
        ensure!(
            self.data_dir == other.data_dir,
            "cannot attach a different Mesh identity directory"
        );
        *self.active.write().unwrap() = Some(other.runtime()?);
        Ok(())
    }
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
    pub fn is_running(&self) -> bool {
        self.runtime().is_ok()
    }
    pub fn same_runtime(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.active, &other.active)
    }
    pub fn account_proof(&self, session: &str) -> Result<String> {
        let active = self.runtime()?;
        let origin = format!("key:{}", active.endpoint.id().to_z32());
        let signature = active
            .endpoint
            .secret_key()
            .sign(format!("zork-account-device-v1:{session}:{origin}").as_bytes());
        Ok(signature
            .to_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect())
    }
    pub async fn identity(&self) -> Result<String> {
        Ok(format!("key:{}", self.runtime()?.endpoint.id().to_z32()))
    }
    pub fn address(&self) -> Result<EndpointAddr> {
        Ok(self.runtime()?.endpoint.addr())
    }
    pub fn addresses(&self) -> Result<futures_util::stream::BoxStream<'static, EndpointAddr>> {
        use futures_util::StreamExt;
        use iroh::Watcher;
        Ok(self.runtime()?.endpoint.watch_addr().stream().boxed())
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

    pub async fn peer_address(&self, origin: &str) -> Result<Option<EndpointAddr>> {
        Ok(self.runtime()?.peers.read().unwrap().get(origin).cloned())
    }
    pub async fn trust(&self, origin: &str, _note: &str, addr: Option<&str>) -> Result<()> {
        let key = NodeId::from_z32(origin.strip_prefix("key:").context("key origin required")?)?;
        let active = self.runtime()?;
        let mut peers = active.peers.write().unwrap();
        let address = peers
            .entry(origin.into())
            .or_insert_with(|| EndpointAddr::new(key));
        if let Some(addr) = addr {
            address.addrs.insert(iroh::TransportAddr::Ip(addr.parse()?));
        }
        self.persist_peers(&peers)?;
        active.trusted.write().unwrap().insert(origin.into());
        Ok(())
    }
    fn persist_peers(
        &self,
        peers: &std::collections::BTreeMap<String, EndpointAddr>,
    ) -> Result<()> {
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new_in(&self.data_dir)?;
        serde_json::to_writer(&mut file, peers)?;
        file.flush()?;
        file.as_file().sync_all()?;
        file.persist(self.data_dir.join("peers.json"))?;
        Ok(())
    }
    pub async fn remember_peer_address(
        &self,
        origin: &str,
        address: serde_json::Value,
    ) -> Result<()> {
        let address: EndpointAddr = serde_json::from_value(address)?;
        ensure!(
            origin == format!("key:{}", address.id.to_z32()) && address.addrs.len() <= 24,
            "peer_address_identity_mismatch"
        );
        for relay in address.relay_urls() {
            zork_config::services::validate_endpoint(relay.as_str())?;
        }
        let active = self.runtime()?;
        let mut peers = active.peers.write().unwrap();
        peers.insert(origin.into(), address);
        self.persist_peers(&peers)
    }
    pub async fn untrust(&self, origin: &str) -> Result<()> {
        let active = self.runtime()?;
        {
            let mut peers = active.peers.write().unwrap();
            peers.remove(origin);
            self.persist_peers(&peers)?;
        }
        active.trusted.write().unwrap().remove(origin);
        active.connections.revoke(origin);
        if let Some(connection) = active.dialed.lock().await.remove(origin) {
            connection.close(403u32.into(), b"access revoked");
        }
        Ok(())
    }
    pub async fn is_trusted(&self, origin: &str) -> Result<bool> {
        Ok(self.runtime()?.trusted.read().unwrap().contains(origin))
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
                let active = self.runtime()?;
                ensure!(self.is_trusted(origin).await?, "mesh_peer_not_paired");
                let cached = active
                    .dialed
                    .lock()
                    .await
                    .get(origin)
                    .filter(|connection| connection.close_reason().is_none())
                    .cloned();
                let connection = if let Some(connection) = cached {
                    connection
                } else {
                    let connection = active
                        .endpoint
                        .connect(address, crate::control::ALPN)
                        .await?;
                    ensure!(self.is_trusted(origin).await?, "mesh_peer_not_paired");
                    active
                        .dialed
                        .lock()
                        .await
                        .insert(origin.into(), connection.clone());
                    connection
                };
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

    pub async fn put(&self, space: &str, path: &str, bytes: &[u8]) -> Result<ObjectRef> {
        ensure!(bytes.len() <= MAX_ARTIFACT, "artifact exceeds 300 MiB");
        let object = ObjectRef {
            origin: self.identity().await?,
            space: space.into(),
            path: path.into(),
            root: crate::content_root(bytes),
            size: bytes.len() as u64,
        };
        self.cache_object(&object, bytes).await?;
        Ok(object)
    }
    async fn cache_object(&self, object: &ObjectRef, bytes: &[u8]) -> Result<()> {
        object.verify(bytes)?;
        let directory = self.data_dir.join("objects");
        tokio::fs::create_dir_all(&directory).await?;
        let destination = directory.join(&object.root);
        let temporary = tempfile::NamedTempFile::new_in(&directory)?;
        tokio::fs::write(temporary.path(), bytes).await?;
        temporary.as_file().sync_all()?;
        temporary.persist(destination)?;
        Ok(())
    }
    pub(crate) async fn local_object(&self, object: &ObjectRef) -> Result<Vec<u8>> {
        ensure!(
            object.size <= MAX_ARTIFACT as u64,
            "artifact exceeds 300 MiB"
        );
        let hash: blake3::Hash = object.root.parse()?;
        ensure!(hash.to_hex().as_str() == object.root, "invalid object hash");
        let path = self.data_dir.join("objects").join(&object.root);
        let bytes = tokio::fs::read(path).await?;
        object.verify(&bytes)?;
        Ok(bytes)
    }
    pub async fn read(&self, object: &ObjectRef) -> Result<Vec<u8>> {
        ensure!(
            object.size <= MAX_ARTIFACT as u64,
            "artifact exceeds 300 MiB"
        );
        ensure!(
            object.origin == self.identity().await? || self.is_trusted(&object.origin).await?,
            "mesh_peer_not_paired"
        );
        if let Ok(bytes) = self.local_object(object).await {
            return Ok(bytes);
        }
        tokio::time::timeout(artifact_timeout(object.size), async {
            let mut stream = self
                .connect_tunnel(
                    &object.origin,
                    &serde_json::json!({"v":1,"request":{"kind":"object","object":object}}),
                )
                .await?
                .0;
            use tokio::io::AsyncReadExt;
            let mut bytes = Vec::new();
            (&mut stream)
                .take(object.size + 1)
                .read_to_end(&mut bytes)
                .await?;
            object.verify(&bytes)?;
            ensure!(
                self.is_trusted(&object.origin).await?,
                "mesh_peer_not_paired"
            );
            self.cache_object(object, &bytes).await?;
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
    async fn refresh_peer_route(&self, origin: &str) -> Result<()> {
        use futures_util::StreamExt;
        let key = NodeId::from_z32(origin.strip_prefix("key:").context("key origin required")?)?;
        let active = self.runtime()?;
        let Ok(lookup) = active.endpoint.address_lookup() else {
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
            self.remember_peer_address(origin, serde_json::to_value(address)?)
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
        let active = client.runtime()?;
        let lookup = active.endpoint.address_lookup().unwrap().clone();
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
        let found = client.peer_address(&origin).await?.unwrap();
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
            .runtime()?
            .endpoint
            .address_lookup()
            .unwrap()
            .add(DelayedRoute {
                address: expected.clone(),
                source: "cached-dns",
                delay: Duration::ZERO,
            });
        client.refresh_peer_route(&origin).await?;
        let found = client.peer_address(&origin).await?.unwrap();
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
            .runtime()?
            .endpoint
            .address_lookup()
            .unwrap()
            .add(lookup.clone());
        client.refresh_peer_route(&origin).await?;
        let first = client.peer_address(&origin).await?.unwrap();
        assert_eq!(
            first.ip_addrs().copied().collect::<Vec<_>>(),
            vec!["127.0.0.1:23456".parse()?]
        );
        lookup.set_endpoint_info(
            iroh::EndpointAddr::new(key).with_ip_addr("127.0.0.1:34567".parse()?),
        );
        client.refresh_peer_route(&origin).await?;
        let second = client.peer_address(&origin).await?.unwrap();
        assert_eq!(
            second.ip_addrs().copied().collect::<Vec<_>>(),
            vec!["127.0.0.1:34567".parse()?]
        );
        runtime.shutdown().await?;
        Ok(())
    }
}

pub fn validate_peer_address(origin: &str, value: &serde_json::Value) -> Result<()> {
    let address: EndpointAddr = serde_json::from_value(value.clone())?;
    ensure!(
        origin == format!("key:{}", address.id.to_z32()) && address.addrs.len() <= 24,
        "peer_address_identity_mismatch"
    );
    for relay in address.relay_urls() {
        zork_config::services::validate_endpoint(relay.as_str())?;
    }
    Ok(())
}
