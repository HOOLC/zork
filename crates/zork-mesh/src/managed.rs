//! Lifecycle tasks owned by the host's existing Tokio runtime.
use crate::node::MeshNode;
use anyhow::{ensure, Context, Result};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio::{
    sync::{broadcast, oneshot},
    task::{JoinHandle, JoinSet},
};
use zork_config::MeshConfig;

pub(crate) fn network_options(config: &MeshConfig) -> Result<synch_net::NetOptions> {
    let mut effective = config.clone();
    zork_config::services::ServicesConfig::load_from_install()?.apply_defaults(&mut effective)?;
    Ok(synch_net::NetOptions {
        offline: effective.offline,
        bind_addr: effective.bind.as_deref().map(str::parse).transpose()?,
        relay_urls: effective.relay_urls.unwrap_or_default(),
        relay_quic_port: effective.relay_quic_port,
        discovery_url: effective.discovery_url,
        quic_discovery_urls: effective.quic_discovery_urls.unwrap_or_default(),
        ..Default::default()
    })
}

pub fn data_dir(root: &Path) -> PathBuf {
    root.join("mesh/synch")
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
    #[cfg(test)]
    shutdown_loops: broadcast::Sender<()>,
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
        let result = task.await.context("Synch background task failed");
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
    start_mode(root, config, false, None, Vec::new()).await
}

/// Starts a node that also serves the native control protocol to its peers.
pub async fn start_with_control(
    root: &Path,
    config: &MeshConfig,
    control: Arc<dyn crate::control::ControlHandler>,
) -> Result<Runtime> {
    start_mode(root, config, false, Some(control), Vec::new()).await
}

/// Retract obsolete source roles before scanners and publishers can start.
pub async fn start_with_control_retiring_sources(
    root: &Path,
    config: &MeshConfig,
    control: Arc<dyn crate::control::ControlHandler>,
    sources: &[&str],
) -> Result<Runtime> {
    start_mode(
        root,
        config,
        false,
        Some(control),
        sources.iter().map(|s| (*s).to_owned()).collect(),
    )
    .await
}

/// Retract obsolete sources before restoring background publication.
pub async fn start_retiring_sources(
    root: &Path,
    config: &MeshConfig,
    sources: &[&str],
) -> Result<Runtime> {
    start_mode(
        root,
        config,
        false,
        None,
        sources.iter().map(|s| (*s).to_owned()).collect(),
    )
    .await
}

/// A mobile/desktop access client owns an identity but never executes sockets
/// or watches local workspaces. It still exchanges and verifies remote objects.
pub async fn start_client(root: &Path, config: &MeshConfig) -> Result<Runtime> {
    ensure!(
        config.workspaces.is_empty(),
        "client cannot host workspaces"
    );
    start_mode(root, config, true, None, Vec::new()).await
}

async fn start_mode(
    root: &Path,
    config: &MeshConfig,
    client_only: bool,
    control: Option<Arc<dyn crate::control::ControlHandler>>,
    retiring: Vec<String>,
) -> Result<Runtime> {
    validate(config)?;
    let (root, config) = (root.to_owned(), config.clone());
    let (ready, started) = oneshot::channel();
    // Node::open must finish its owned initialization even if the caller stops
    // waiting. An undelivered Runtime is dropped here and shuts itself down;
    // cancellation cannot drop a half-open engine while releasing its lock.
    tokio::spawn(async move {
        let result = start_owned(&root, &config, client_only, control, &retiring).await;
        let _ = ready.send(result);
    });
    started.await.context("Mesh startup task failed")?
}

async fn start_owned(
    root: &Path,
    config: &MeshConfig,
    client_only: bool,
    control: Option<Arc<dyn crate::control::ControlHandler>>,
    retiring: &[String],
) -> Result<Runtime> {
    validate(config)?;
    let channel = zork_config::channel::activate_for_data(root)?;
    ensure!(
        config.channel.is_none_or(|expected| expected == channel),
        "mesh_channel_mismatch"
    );
    zork_config::channel::claim(root, channel)?;
    let data = data_dir(root);
    let init_data = data.clone();
    let (lock, clean_start) = tokio::task::spawn_blocking(move || -> Result<_> {
        let _scope = synch_core::BlockingScope::enter();
        let lock = synch_engine::LifecycleLock::acquire(&init_data)?;
        if !init_data.join("synchronicity.db").exists() {
            synch_engine::Node::init(&init_data, None)?;
        }
        // Exclusive ownership excludes a live legacy daemon before its stale
        // local control artifacts are removed during migration.
        for name in ["control.sock", "control.token"] {
            match std::fs::remove_file(init_data.join(name)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                init_data.parent().context("Mesh parent directory")?,
                std::fs::Permissions::from_mode(0o700),
            )?;
        }
        let clean_start = crate::clean_start::consume(&init_data)?;
        Ok((lock, clean_start))
    })
    .await??;
    synch_net::tls::install_crypto_provider();
    let mut options = synch_engine::NodeConfig::new(data.clone());
    // Zork serves native control; no node executes published socket programs.
    options.socket_workers = 0;
    let control_connections = Arc::new(crate::control::Connections::default());
    options.net = network_options(config)?;
    options.net.control = control.map(|handler| {
        Arc::new(crate::control::ControlService::new(
            handler,
            control_connections.clone(),
        )) as Arc<dyn synch_net::ControlProtocol>
    });
    options.dns.no_tuf = config.offline;
    let engine = synch_engine::Node::open(options).await?;
    // Publish the bound loopback endpoint before readoption: two same-host
    // peers must be able to find each other while both are still starting.
    let local_registration = match crate::local_discovery::install(engine.net().endpoint()).await {
        Ok(registration) => registration,
        Err(error) => {
            tracing::warn!(%error, "local Mesh address discovery unavailable; using normal discovery");
            None
        }
    };
    // Discover current LAN ports even after a peer restarts. These are only
    // routing hints: QUIC still pins the key and Synch still checks membership.
    let lan_registration = if std::env::var_os("ZORK_MESH_LAN_DISCOVERY").is_none_or(|v| v != "0") {
        Some(crate::lan_discovery::install(
            engine.net().endpoint(),
            "zork-mesh-v1",
        )?)
    } else {
        None
    };
    let alive = Arc::new(AtomicBool::new(true));
    let (startup_publish, mut pending_publish) = tokio::sync::mpsc::channel(1);
    let handle = MeshNode::from_engine(
        engine.clone(),
        alive.clone(),
        startup_publish,
        control_connections,
    );
    let setup: Result<()> = async {
        // Zork currently supports static key identities. Its own enrollment
        // service owns membership; there is no CLI or cloud-tunnel lifecycle.
        ensure!(
            engine.origin().to_string().starts_with("key:"),
            "Zork Mesh requires a key origin"
        );
        handle
            .blocking(|node| {
                node.disable_cloud()?;
                node.reopen_interrupted_uploads()?;
                Ok(())
            })
            .await?;
        if !clean_start {
            engine.readopt_self_on_startup().await?;
        }
        for source in retiring {
            handle.retire_source(source).await?;
        }
        tracing::info!(clean_start, "Mesh startup history checked");
        Ok(())
    }
    .await;
    if let Err(error) = setup {
        let _ = engine.shutdown().await;
        return Err(error);
    }
    let (stop_loops, _) = broadcast::channel::<()>(1);
    let mut loops = JoinSet::new();
    let pushing = engine.clone();
    let mut stop_push = stop_loops.subscribe();
    loops.spawn(async move {
        loop {
            tokio::select! {
                _ = stop_push.recv() => break,
                request = pending_publish.recv() => {
                    let Some(request) = request else { break; };
                    // This local scan owns a non-cancellable blocking task.
                    // Drain it before releasing the database or recording a
                    // clean close; dropping its future would detach the write.
                    if let Err(error) = pushing.scan_source_and_stage_async(&request.space).await {
                        tracing::warn!(%error, "startup Mesh publication failed");
                    }
                }
            }
        }
        "startup-publication"
    });
    macro_rules! run {
        ($name:literal, $method:ident) => {{
            let node = engine.clone();
            let mut stop = stop_loops.subscribe();
            loops.spawn(async move {
                node.$method(async move {
                    let _ = stop.recv().await;
                })
                .await;
                $name
            });
        }};
    }
    run!("anti-entropy", run_anti_entropy);
    run!("publisher", run_publisher);
    run!("maintenance", run_maintenance);
    if !client_only {
        run!("scanner", run_scanner);
        run!("watcher", run_watcher);
        run!("replicas", run_replicas);
        run!("checkouts", run_checkouts);
    }
    #[cfg(test)]
    let shutdown_loops = stop_loops.clone();
    let (stop, stopped) = oneshot::channel();
    let closing = handle.clone();
    let task = tokio::spawn(async move {
        let failure = tokio::select! {
            _ = stopped => None,
            result = loops.join_next() => Some(format!("Synch background loop ended unexpectedly: {result:?}")),
        };
        alive.store(false, Ordering::Release);
        let _ = stop_loops.send(());
        let mut errors = Vec::new();
        // Close transport while loops drain so a loop awaiting an unreachable
        // peer can wake. Synch still flushes its local publisher on stop, and
        // both completions precede releasing the engine and lifecycle lock.
        let draining_started = std::time::Instant::now();
        if let Some(registration) = lan_registration {
            registration.shutdown().await;
        }
        let transport_stop = async {
            if client_only {
                // Iroh can leave a closed QUIC connection in wait_all_draining
                // after remote subscription use. Access clients have
                // no execution sockets. Bound their network drain, then drop
                // the router/endpoint only after the local publisher is done.
                match tokio::time::timeout(std::time::Duration::from_secs(10), engine.shutdown())
                    .await
                {
                    Ok(result) => result.map(|()| true),
                    Err(_) => {
                        tracing::warn!(
                            "Mesh accessor transport did not drain; closing its endpoint"
                        );
                        Ok(false)
                    }
                }
            } else {
                engine.shutdown().await.map(|()| true)
            }
        };
        let (shutdown, ()) = tokio::join!(transport_stop, async {
            while let Some(result) = loops.join_next().await {
                if let Err(error) = result {
                    errors.push(error.to_string());
                }
            }
        });
        tracing::info!(
            elapsed_ms = draining_started.elapsed().as_millis() as u64,
            "Synch transport and loops drained"
        );
        // A scanner may stage its final batch after the publisher loop has
        // flushed on stop. Commit it locally after every producer has drained.
        let final_publication = engine.publish_staged().await;
        drop(local_registration);
        closing.release_engine();
        drop(engine);
        let clean_close = shutdown?;
        final_publication?;
        if let Some(error) = failure {
            anyhow::bail!(error);
        }
        ensure!(
            errors.is_empty(),
            "Synch background tasks failed: {}",
            errors.join(", ")
        );
        if clean_close {
            if let Err(error) =
                tokio::task::spawn_blocking(move || crate::clean_start::record(&data)).await?
            {
                tracing::warn!(%error, "Mesh clean restart receipt unavailable");
            }
        }
        drop(lock);
        Ok(())
    });
    Ok(Runtime {
        node: handle,
        stop: Some(stop),
        task: Some(task),
        #[cfg(test)]
        shutdown_loops,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn config() -> MeshConfig {
        MeshConfig {
            enabled: true,
            offline: true,
            ..Default::default()
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn access_client_preserves_identity_and_declines_socket_execution() -> Result<()> {
        let root = tempfile::tempdir()?;
        let mut first = start_client(root.path(), &config()).await?;
        let id = first.node().identity().await?;
        assert_eq!(
            first
                .node()
                .blocking(|node| Ok(node.config().socket_workers))
                .await?,
            0
        );
        first.shutdown().await?;
        let mut reopened = start_client(root.path(), &config()).await?;
        assert_eq!(reopened.node().identity().await?, id);
        assert!(!data_dir(root.path()).join("control.sock").exists());
        reopened.shutdown().await?;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn startup_retirement_preserves_files_and_excludes_them_from_later_scans() -> Result<()> {
        let root = tempfile::tempdir()?;
        let old = root.path().join("internal");
        let old_zork = root.path().join("old-zork");
        let shared = root.path().join("shared");
        std::fs::create_dir(&old)?;
        std::fs::create_dir(&old_zork)?;
        std::fs::write(old_zork.join("private"), b"old internal state")?;
        std::fs::create_dir(&shared)?;
        std::fs::write(old.join("history"), b"original history")?;
        std::fs::write(shared.join("keep"), b"deliberately shared")?;
        let mut seeded = start_client(root.path(), &config()).await?;
        let identity = seeded.node().identity().await?;
        seeded.node().add_filesystem_source("files", &old).await?;
        seeded
            .node()
            .add_filesystem_source("zork", &old_zork)
            .await?;
        seeded
            .node()
            .add_filesystem_source("shared", &shared)
            .await?;
        let engine = seeded.node().blocking(|node| Ok(node)).await?;
        engine.scan_source_and_stage_async("files").await?;
        engine.scan_source_and_stage_async("zork").await?;
        engine.scan_source_and_stage_async("shared").await?;
        engine.publish_staged().await?;
        drop(engine);
        seeded.shutdown().await?;
        // A restored scanner must never snapshot newly appended internal data.
        std::fs::write(old.join("history"), b"original history\nnew event")?;
        std::fs::write(old.join("new-private-file"), b"private")?;
        for _ in 0..2 {
            let mut current =
                start_retiring_sources(root.path(), &config(), &["files", "zork"]).await?;
            assert_eq!(current.node().identity().await?, identity);
            let engine = current.node().blocking(|node| Ok(node)).await?;
            let (_, scans) = engine.scan_and_stage_async_with_reports().await?;
            assert!(scans
                .iter()
                .all(|(space, _)| space != "files" && space != "zork"));
            engine.publish_staged().await?;
            current
                .node()
                .blocking(|node| {
                    for source in ["files", "zork"] {
                        assert!(node.store().source(source)?.is_none());
                        assert!(node.store().local_files(source)?.is_empty());
                        assert_eq!(node.store().count_entries(node.origin(), source)?, 0);
                    }
                    node.resolve("shared", "keep", &synch_engine::VersionPolicy::Newest)?;
                    Ok(())
                })
                .await?;
            drop(engine);
            current.shutdown().await?;
        }
        assert_eq!(
            std::fs::read(old.join("history"))?,
            b"original history\nnew event"
        );
        assert_eq!(std::fs::read(old.join("new-private-file"))?, b"private");
        assert_eq!(
            std::fs::read(old_zork.join("private"))?,
            b"old internal state"
        );
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn initial_source_scans_do_not_wait_for_an_unresponsive_peer() -> Result<()> {
        let root = tempfile::tempdir()?;
        let peer_root = tempfile::tempdir()?;
        let mut runtime = start(root.path(), &config()).await?;
        let mut peer = start(peer_root.path(), &config()).await?;
        let peer_id = peer.node().identity().await?;
        peer.shutdown().await?;
        // Keep the UDP port open but never answer the QUIC handshake. The
        // ordinary push path waits its dial timeout; local readiness must not.
        let blackhole = std::net::UdpSocket::bind("127.0.0.1:0")?;
        let node = runtime.node();
        node.trust(
            &peer_id,
            "unresponsive peer",
            Some(&blackhole.local_addr()?.to_string()),
        )
        .await?;
        let source = root.path().join("startup-source");
        std::fs::create_dir(&source)?;
        std::fs::write(source.join("entry"), b"ready locally")?;
        node.add_filesystem_source("startup", &source).await?;
        // Control readiness no longer publishes a program. Queuing all real
        // business sources must still remain independent of peer delivery.
        tokio::time::timeout(Duration::from_secs(3), async {
            for index in 0..3 {
                let space = format!("startup-following-{index}");
                let source = root.path().join(&space);
                std::fs::create_dir(&source)?;
                std::fs::write(source.join("entry"), b"independent local scan")?;
                node.add_filesystem_source(&space, &source).await?;
                node.schedule_source_scan(&space).await?;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("initial source scans waited for remote publication")??;
        // Shutdown owns the publication task and drains Synch's other loops;
        // reopening proves the lock and network lifetime were released.
        tokio::time::timeout(Duration::from_secs(30), runtime.shutdown())
            .await
            .context("startup publisher did not stop")??;
        let started = std::time::Instant::now();
        let mut reopened =
            tokio::time::timeout(Duration::from_secs(1), start(root.path(), &config()))
                .await
                .context("clean restart waited for the unresponsive peer")??;
        eprintln!(
            "clean Mesh restart with unresponsive peer: {:?}",
            started.elapsed()
        );
        reopened.shutdown().await?;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn loopback_discovery_keeps_synch_authentication_and_recovers_after_restart() -> Result<()>
    {
        let a_root = tempfile::tempdir()?;
        let b_root = tempfile::tempdir()?;
        // No relay, DNS discovery, or manually exchanged UDP addresses.
        let network = config();
        let mut a = start_client(a_root.path(), &network).await?;
        let mut b = start_client(b_root.path(), &network).await?;
        let a_id = a.node().identity().await?;
        let b_id = b.node().identity().await?;
        a.node().add_api_source("local-test").await?;
        let first = a
            .node()
            .put("local-test", "first", b"authenticated Synch bytes")
            .await?;
        ensure!(
            b.node().read(&first).await.is_err(),
            "address discovery granted trust"
        );
        a.node().trust(&b_id, "local B", None).await?;
        b.node().trust(&a_id, "local A", None).await?;
        let started = std::time::Instant::now();
        let bytes = tokio::time::timeout(Duration::from_secs(3), b.node().read(&first)).await??;
        ensure!(
            bytes == b"authenticated Synch bytes",
            "Synch transfer mismatch"
        );
        eprintln!("first loopback Synch transfer: {:?}", started.elapsed());
        a.shutdown().await?;
        b.shutdown().await?;

        // Persisted peers start together with new ephemeral ports. Recovery
        // must still run through Synch, using each other's newly leased hints.
        let started = std::time::Instant::now();
        let (mut a, mut b) = tokio::time::timeout(Duration::from_secs(3), async {
            tokio::try_join!(
                start_client(a_root.path(), &network),
                start_client(b_root.path(), &network)
            )
        })
        .await??;
        ensure!(
            a.node().identity().await? == a_id && b.node().identity().await? == b_id,
            "restart replaced an identity"
        );
        eprintln!(
            "simultaneous loopback Synch recovery: {:?}",
            started.elapsed()
        );
        let started = std::time::Instant::now();
        for index in 0..3 {
            let payload = format!("new bytes after restart {index}");
            let object = a
                .node()
                .put(
                    "local-test",
                    &format!("restarted-{index}"),
                    payload.as_bytes(),
                )
                .await?;
            let bytes =
                tokio::time::timeout(Duration::from_secs(3), b.node().read(&object)).await??;
            ensure!(
                bytes == payload.as_bytes(),
                "restarted Synch transfer mismatch"
            );
        }
        eprintln!("three further Synch transfers: {:?}", started.elapsed());
        a.shutdown().await?;
        b.shutdown().await?;
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn restored_database_and_old_receipt_still_readopt_the_peer_head() -> Result<()> {
        let root = tempfile::tempdir()?;
        let witness_root = tempfile::tempdir()?;
        let mut runtime = start_client(root.path(), &config()).await?;
        let mut witness = start_client(witness_root.path(), &config()).await?;
        let id = runtime.node().identity().await?;
        let witness_id = witness.node().identity().await?;
        runtime.node().trust(&witness_id, "witness", None).await?;
        witness.node().trust(&id, "publisher", None).await?;
        runtime.node().add_api_source("restore-test").await?;
        let first = runtime
            .node()
            .put("restore-test", "version", b"one")
            .await?;
        witness.node().read(&first).await?;
        runtime.shutdown().await?;
        let data = data_dir(root.path());
        let database = data.join(synch_store::DB_FILE);
        let backup = std::fs::read(&database)?;
        let old_receipt = std::fs::read(data.join("clean-start.json"))?;

        runtime = start_client(root.path(), &config()).await?;
        let second = runtime
            .node()
            .put("restore-test", "version", b"two")
            .await?;
        let witness_engine = witness.node().blocking(|node| Ok(node)).await?;
        witness_engine
            .sync_with_peer(&synch_core::NodeId::from_z32(&id[4..])?)
            .await?;
        drop(witness_engine);
        ensure!(
            witness.node().read(&second).await? == b"two",
            "witness did not retain the newer version"
        );
        runtime.shutdown().await?;

        // Restore both the old SQLite file and its old clean-exit receipt.
        // Changed file identity/timestamps must force authenticated readoption.
        for suffix in ["-wal", "-shm"] {
            let _ = std::fs::remove_file(data.join(format!("{}{suffix}", synch_store::DB_FILE)));
        }
        std::fs::write(&database, backup)?;
        std::fs::write(data.join("clean-start.json"), old_receipt)?;
        runtime = start_client(root.path(), &config()).await?;
        runtime
            .node()
            .blocking(|node| {
                let entry = node.resolve(
                    "restore-test",
                    "version",
                    &synch_engine::VersionPolicy::Origin(node.origin().clone()),
                )?;
                ensure!(
                    entry.content == Some(synch_core::Hash::new(b"two")),
                    "startup published from an old database without readoption"
                );
                Ok(())
            })
            .await?;
        runtime.shutdown().await?;
        witness.shutdown().await?;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_shutdown_flushes_staged_local_changes() -> Result<()> {
        let root = tempfile::tempdir()?;
        let mut runtime = start_client(root.path(), &config()).await?;
        let source = root.path().join("source");
        std::fs::create_dir(&source)?;
        std::fs::write(source.join("pending"), b"must survive shutdown")?;
        runtime
            .node()
            .add_filesystem_source("pending-test", &source)
            .await?;
        let engine = runtime.node().blocking(|engine| Ok(engine)).await?;
        engine.scan_source_and_stage_async("pending-test").await?;
        ensure!(engine.publisher().pending() > 0, "fixture was not staged");
        drop(engine);
        runtime.shutdown().await?;
        let mut reopened = start_client(root.path(), &config()).await?;
        reopened
            .node()
            .blocking(|engine| {
                let entry = engine.resolve(
                    "pending-test",
                    "pending",
                    &synch_engine::VersionPolicy::Newest,
                )?;
                ensure!(
                    entry.content == Some(synch_core::Hash::new(b"must survive shutdown")),
                    "staged data was lost"
                );
                Ok(())
            })
            .await?;
        reopened.shutdown().await?;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn library_node_preserves_identity_without_a_control_service() -> Result<()> {
        let root = tempfile::Builder::new()
            .prefix("zlib-")
            .tempdir_in("/tmp")?;
        let mut config = config();
        config.synch_binary = Some(root.path().join("missing-synch"));
        let mut runtime = start(root.path(), &config).await?;
        let node = runtime.node();
        let origin = node.identity().await?;
        ensure!(
            !node.data_dir().join("control.sock").exists(),
            "daemon control socket created"
        );
        ensure!(
            !node.data_dir().join("control.token").exists(),
            "daemon control token created"
        );
        ensure!(
            start(root.path(), &config).await.is_err(),
            "duplicate ownership accepted"
        );
        node.add_api_source("test-artifacts").await?;
        let object = node
            .put(
                "test-artifacts",
                "receipt",
                b"persist across Station restart",
            )
            .await?;
        tokio::time::timeout(Duration::from_secs(30), runtime.shutdown()).await??;
        ensure!(
            node.identity().await.is_err(),
            "closed handle remained active"
        );
        // A stopped legacy daemon may leave a Unix socket pathname/token.
        #[cfg(unix)]
        {
            let socket =
                std::os::unix::net::UnixListener::bind(node.data_dir().join("control.sock"))?;
            drop(socket);
        }
        std::fs::write(node.data_dir().join("control.token"), [7u8; 32])?;
        let mut reopened = start(root.path(), &config).await?;
        ensure!(
            !node.data_dir().join("control.sock").exists()
                && !node.data_dir().join("control.token").exists(),
            "legacy control artifacts were retained"
        );
        ensure!(
            reopened.node().identity().await? == origin,
            "identity replaced"
        );
        ensure!(
            reopened.node().read(&object).await? == b"persist across Station restart",
            "publication lost"
        );
        reopened.shutdown().await?;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_owner_closes_tasks_and_releases_database() -> Result<()> {
        let root = tempfile::Builder::new()
            .prefix("zdrop-")
            .tempdir_in("/tmp")?;
        let runtime = start(root.path(), &config()).await?;
        let node = runtime.node();
        let task = runtime.task.as_ref().unwrap().abort_handle();
        drop(runtime);
        tokio::time::timeout(Duration::from_secs(30), async {
            while !task.is_finished() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        ensure!(
            node.identity().await.is_err(),
            "dropped owner left an active node"
        );
        let mut next = start(root.path(), &config()).await?;
        next.shutdown().await?;
        Ok(())
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unexpected_loop_exit_fails_the_host_and_closes_the_node() -> Result<()> {
        let root = tempfile::Builder::new()
            .prefix("zfail-")
            .tempdir_in("/tmp")?;
        let mut runtime = start(root.path(), &config()).await?;
        let node = runtime.node();
        runtime.shutdown_loops.send(())?;
        let result = tokio::time::timeout(Duration::from_secs(30), runtime.wait()).await?;
        ensure!(
            result.is_err(),
            "background loop exit was hidden from the Station"
        );
        ensure!(
            node.identity().await.is_err(),
            "failed tasks left an active engine"
        );
        runtime.shutdown().await?;
        let mut next = start(root.path(), &config()).await?;
        next.shutdown().await?;
        Ok(())
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelling_startup_does_not_leave_an_unowned_node() -> Result<()> {
        let root = tempfile::Builder::new()
            .prefix("zcancel-")
            .tempdir_in("/tmp")?;
        let data = root.path().to_owned();
        let caller = tokio::spawn(async move { start(&data, &config()).await });
        // Initialization is now running, but the original caller goes away.
        tokio::task::yield_now().await;
        caller.abort();
        let _ = caller.await;
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if let Ok(mut runtime) = start(root.path(), &config()).await {
                    runtime.shutdown().await?;
                    return Ok::<_, anyhow::Error>(());
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await??;
        Ok(())
    }
}
