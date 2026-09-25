//! Lifecycle tasks owned by the host's existing Tokio runtime.
use crate::node::MeshNode;
use anyhow::{ensure, Context, Result};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{sync::oneshot, task::JoinHandle};
use zork_config::MeshConfig;

pub(crate) fn network_options(config: &MeshConfig) -> Result<crate::network::Options> {
    crate::network::options(config)
}

pub fn data_dir(root: &Path) -> PathBuf {
    root.join("mesh/iroh")
}
pub fn same_transport(a: &MeshConfig, b: &MeshConfig) -> bool {
    a.enabled == b.enabled
        && a.offline == b.offline
        && a.bind == b.bind
        && a.relay_urls == b.relay_urls
        && a.relay_quic_port == b.relay_quic_port
        && a.discovery_url == b.discovery_url
        && a.quic_discovery_urls == b.quic_discovery_urls
}
pub fn validate(config: &MeshConfig) -> Result<()> {
    ensure!(
        config
            .channel
            .is_none_or(|channel| Some(channel) == zork_config::channel::current().ok()),
        "mesh_channel_mismatch"
    );
    if let Some(bind) = &config.bind {
        bind.parse::<std::net::SocketAddr>()
            .context("invalid Mesh bind address")?;
    }
    if let Some(group) = &config.group {
        group.validate()?;
    }
    zork_config::services::ServicesConfig {
        relay_urls: config.relay_urls.clone(),
        relay_quic_port: config.relay_quic_port,
        discovery_url: config.discovery_url.clone(),
        quic_discovery_urls: config.quic_discovery_urls.clone(),
    }
    .validate()?;
    ensure!(
        config.peers.len() <= 32 && config.workspaces.len() <= 32,
        "Mesh supports at most 32 peers and 32 workspaces"
    );
    let mut origins = std::collections::HashSet::new();
    let mut ids = std::collections::HashSet::new();
    for workspace in &config.workspaces {
        ensure!(
            !workspace.id.is_empty()
                && workspace.id.len() <= 64
                && workspace
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "invalid Mesh workspace ID"
        );
        ensure!(ids.insert(&workspace.id), "duplicate Mesh workspace ID");
        ensure!(
            workspace.path.is_absolute() && workspace.path.is_dir(),
            "Mesh workspace must be an existing absolute directory"
        );
        ensure!(
            !workspace.profile_id.is_empty()
                && !workspace.model.is_empty()
                && !workspace.thinking.is_empty(),
            "Mesh workspace needs a local model selection"
        );
    }
    for peer in &config.peers {
        if let Some(routes) = &peer.routes {
            routes.validate()?;
        }
        ensure!(
            peer.origin.starts_with("key:")
                && peer.origin.len() == 56
                && peer.origin[4..]
                    .bytes()
                    .all(|b| b"ybndrfg8ejkmcpqxot1uwisza345h769".contains(&b)),
            "Mesh initially requires key origins"
        );
        ensure!(
            peer.name.len() <= 128 && peer.execute.len() <= 32,
            "Mesh peer metadata too large"
        );
        ensure!(origins.insert(&peer.origin), "duplicate Mesh peer");
        ensure!(
            peer.execute.iter().all(|id| ids.contains(id)),
            "Mesh execute grant references an unknown local workspace"
        );
    }
    Ok(())
}

/// The Station owns this task and awaits it on shutdown. Library handles are
/// cloned into its services; none can start or address an independent daemon.
pub struct Runtime {
    node: MeshNode,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<Result<()>>>,
}
impl Runtime {
    pub fn node(&self) -> MeshNode {
        self.node.clone()
    }
    pub fn is_finished(&self) -> bool {
        self.task.as_ref().is_none_or(|task| task.is_finished())
    }
    pub async fn wait(&mut self) -> Result<()> {
        let Some(task) = self.task.as_mut() else {
            return Ok(());
        };
        let result = task.await.context("Mesh transport task failed");
        self.task.take();
        result?
    }
    pub async fn shutdown(&mut self) -> Result<()> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        self.wait().await
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

pub async fn start(root: &Path, config: &MeshConfig) -> Result<Runtime> {
    start_mode(root, config, false, None).await
}

/// Starts a node that also serves the native control protocol to its peers.
pub async fn start_with_control(
    root: &Path,
    config: &MeshConfig,
    control: Arc<dyn crate::control::ControlHandler>,
) -> Result<Runtime> {
    start_mode(root, config, false, Some(control)).await
}

/// A mobile/desktop access client owns an identity but never executes sockets
/// or watches local workspaces. It still exchanges and verifies remote objects.
pub async fn start_client(root: &Path, config: &MeshConfig) -> Result<Runtime> {
    ensure!(
        config.workspaces.is_empty(),
        "client cannot host workspaces"
    );
    start_mode(root, config, true, None).await
}

async fn start_mode(
    root: &Path,
    config: &MeshConfig,
    client_only: bool,
    control: Option<Arc<dyn crate::control::ControlHandler>>,
) -> Result<Runtime> {
    validate(config)?;
    let (root, config) = (root.to_owned(), config.clone());
    let (ready, started) = oneshot::channel();
    // Node::open must finish its owned initialization even if the caller stops
    // waiting. An undelivered Runtime is dropped here and shuts itself down;
    // cancellation cannot drop a half-open engine while releasing its lock.
    tokio::spawn(async move {
        let result = start_owned(&root, &config, client_only, control).await;
        let _ = ready.send(result);
    });
    started.await.context("Mesh startup task failed")?
}

async fn start_owned(
    root: &Path,
    config: &MeshConfig,
    _client_only: bool,
    control: Option<Arc<dyn crate::control::ControlHandler>>,
) -> Result<Runtime> {
    validate(config)?;
    let channel = zork_config::channel::activate_for_data(root)?;
    ensure!(
        config.channel.is_none_or(|expected| expected == channel),
        "mesh_channel_mismatch"
    );
    zork_config::channel::claim(root, channel)?;
    let data = data_dir(root);
    let opening = data.clone();
    let (lock, key) =
        tokio::task::spawn_blocking(move || crate::identity::open(&opening)).await??;
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let (builder, route) = crate::network::configure_endpoint(
        iroh::Endpoint::builder(iroh::endpoint::presets::N0)
            .secret_key(key)
            .ca_tls_config(iroh::tls::CaTlsConfig::system()),
        &network_options(config)?,
    )
    .await?;
    let endpoint = builder.bind().await?;
    let local = crate::local_discovery::install(&endpoint).await?;
    let lan = if std::env::var_os("ZORK_MESH_LAN_DISCOVERY").is_none_or(|v| v != "0") {
        Some(crate::lan_discovery::install(&endpoint, "zork-mesh-v1")?)
    } else {
        None
    };
    let saved: std::collections::BTreeMap<String, iroh::EndpointAddr> =
        match std::fs::read(data.join("peers.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Default::default(),
            Err(error) => return Err(error.into()),
        };
    // The product configuration remains the authority; a stale route cache cannot restore trust.
    let peers = config
        .peers
        .iter()
        .map(|peer| {
            let key = iroh::EndpointId::from_z32(
                peer.origin
                    .strip_prefix("key:")
                    .context("key origin required")?,
            )?;
            Ok((
                peer.origin.clone(),
                saved
                    .get(&peer.origin)
                    .filter(|address| address.id == key)
                    .cloned()
                    .unwrap_or_else(|| iroh::EndpointAddr::new(key)),
            ))
        })
        .collect::<Result<_>>()?;
    let connections = Arc::new(crate::control::Connections::default());
    let handle = MeshNode::from_endpoint(
        data,
        crate::node::ActiveNode {
            endpoint: endpoint.clone(),
            peers: std::sync::RwLock::new(peers),
            trusted: std::sync::RwLock::new(
                config
                    .peers
                    .iter()
                    .map(|peer| peer.origin.clone())
                    .collect(),
            ),
            connections: connections.clone(),
            dialed: Default::default(),
        },
    );
    let handler = Arc::new(crate::control::DeviceHandler {
        node: handle.clone(),
        business: control,
        objects: Arc::new(tokio::sync::Semaphore::new(4)),
    });
    let router = iroh::protocol::Router::builder(endpoint.clone())
        .accept(
            crate::control::ALPN,
            crate::control::ControlService::new(handler, connections),
        )
        .accept(crate::control::LEGACY_ALPN, crate::control::LegacyRefusal)
        .accept(crate::control::VERSION_ALPN, crate::control::VersionService)
        .spawn();
    configure(root, config, &handle).await?;
    let (stop, stopped) = oneshot::channel();
    let closing = handle.clone();
    let task = tokio::spawn(async move {
        let _ = stopped.await;
        closing.close();
        if let Some(lan) = lan {
            lan.shutdown().await;
        }
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(10), router.shutdown()).await;
        endpoint.close().await;
        route.shutdown().await;
        drop(local);
        drop(lock);
        result.context("Mesh transport shutdown timed out")??;
        Ok(())
    });
    Ok(Runtime {
        node: handle,
        stop: Some(stop),
        task: Some(task),
    })
}

pub async fn configure(root: &Path, config: &MeshConfig, node: &MeshNode) -> Result<()> {
    configure_changed(root, config, node, None).await
}
pub async fn configure_changed(
    root: &Path,
    config: &MeshConfig,
    node: &MeshNode,
    previous_config: Option<&MeshConfig>,
) -> Result<()> {
    let ledger = root.join("mesh/managed-peers.json");
    let previous: Vec<String> = match tokio::fs::read(&ledger).await {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
        Err(e) => return Err(e.into()),
    };
    for origin in previous
        .iter()
        .filter(|origin| !config.peers.iter().any(|p| &p.origin == *origin))
    {
        node.untrust(origin).await?;
    }
    for peer in &config.peers {
        let previous = previous_config
            .and_then(|previous| previous.peers.iter().find(|old| old.origin == peer.origin));
        if previous == Some(peer) {
            continue;
        }
        let routes_changed =
            previous.is_none_or(|old| old.addr != peer.addr || old.routes != peer.routes);
        node.trust(
            &peer.origin,
            &peer.name,
            if routes_changed {
                peer.addr.as_deref()
            } else {
                None
            },
        )
        .await?;
        if routes_changed {
            if let Some(routes) = &peer.routes {
                node.remember_routes(&peer.origin, routes).await?;
            }
        }
    }
    let temp = ledger.with_extension("tmp");
    tokio::fs::write(
        &temp,
        serde_json::to_vec(&config.peers.iter().map(|p| &p.origin).collect::<Vec<_>>())?,
    )
    .await?;
    tokio::fs::rename(temp, ledger).await?;
    Ok(())
}
