//! Synch runs inside the GUI process. No transport helper, Station or Agent
//! is started when the desktop connects to a remote node.
use crate::store::{ClientStore, SavedNode};
use anyhow::{ensure, Context, Result};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

struct Embedded {
    executor: tokio::runtime::Runtime,
    node: crate::transport::Runtime,
    config: zork_config::MeshConfig,
    origin: String,
}
impl Embedded {
    fn stop(mut self) -> Result<()> {
        self.executor.block_on(self.node.shutdown())
    }
}

pub struct ClientMesh {
    root: PathBuf,
    embedded: Arc<Mutex<Option<Embedded>>>,
    quitting: AtomicBool,
    handle: zork_mesh::node::MeshNode,
    store: Option<Arc<ClientStore>>,
}
impl ClientMesh {
    pub fn new(root: PathBuf) -> Self {
        Self {
            handle: zork_mesh::node::MeshNode::unbound(zork_mesh::managed::data_dir(&root)),
            root,
            embedded: Arc::new(Mutex::new(None)),
            quitting: AtomicBool::new(false),
            store: None,
        }
    }
    pub fn with_store(root: PathBuf, store: Arc<ClientStore>) -> Self {
        let mut transport = Self::new(root);
        transport.store = Some(store);
        transport
    }
    pub fn control(&self) -> zork_mesh::node::MeshNode {
        self.handle.clone()
    }
    /// Closing SQLite and network workers runs off the UI thread. App quit
    /// awaits this completion so it cannot cut off a clean node shutdown.
    pub fn shutdown(&self) -> tokio::sync::oneshot::Receiver<()> {
        self.quitting.store(true, Ordering::Release);
        let embedded = self.embedded.clone();
        let root = self.root.clone();
        let (done, completion) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            if let Ok(mut state) = embedded.lock() {
                if let Some(node) = state.take() {
                    if let Err(error) = node.stop() {
                        eprintln!("embedded client Mesh shutdown failed: {error:#}");
                    }
                    let _ = std::fs::remove_file(root.join("client-mesh-ready.json"));
                }
            }
            let _ = done.send(());
        });
        completion
    }
    pub fn start(&self, nodes: &[SavedNode]) -> Result<String> {
        self.start_on(nodes, None)
    }
    /// Called on GPUI's background executor; blocking never runs on the UI thread.
    pub fn start_on(
        &self,
        nodes: &[SavedNode],
        network: Option<&zork_config::MeshConfig>,
    ) -> Result<String> {
        let mut state = self.embedded.lock().expect("client Mesh");
        ensure!(!self.quitting.load(Ordering::Acquire), "客户端正在退出");
        std::fs::create_dir_all(&self.root)?;
        let mut config = if zork_config::config_path(&self.root).exists() {
            zork_config::load_config(&self.root)?
        } else {
            zork_config::FileConfig::default()
        };
        let services = super::load_services()?;
        config.mesh.enabled = true;
        if let Some(network) = network {
            config.mesh.offline = network.offline;
        }
        config.mesh.relay_urls = network
            .and_then(|n| n.relay_urls.clone())
            .or(config.mesh.relay_urls);
        config.mesh.discovery_url = network
            .and_then(|n| n.discovery_url.clone())
            .or(config.mesh.discovery_url);
        services.apply_network(&mut config.mesh)?;
        config.mesh.workspaces.clear();
        config.mesh.peers = nodes
            .iter()
            .filter_map(|node| {
                // The authenticated local Station snapshot records its Mesh
                // identity even though its UI API uses HTTP. Synch still needs
                // mutual trust for this separate same-host transport identity.
                let remote = node.mesh.clone().or_else(|| {
                    if !node.local {
                        return None;
                    }
                    self.store
                        .as_ref()?
                        .get::<String>(&node.id, "mesh-origin")
                        .ok()
                        .flatten()
                        .map(|origin| crate::store::RemoteNode { origin, addr: None })
                });
                remote.map(|remote| zork_config::MeshPeer {
                    origin: remote.origin,
                    name: node.name.clone(),
                    addr: remote.addr,
                    execute: vec![],
                    client: false,
                    collaborate: false,
                })
            })
            .collect();
        zork_mesh::managed::validate(&config.mesh)?;
        zork_config::save_config(&self.root, &config)?;
        let restart = state.as_ref().is_some_and(|current| {
            current.node.is_finished()
                || !zork_mesh::managed::same_transport(&current.config, &config.mesh)
        });
        if restart {
            if let Some(previous) = state.take() {
                if let Err(error) = previous.stop() {
                    eprintln!("previous embedded client Mesh ended: {error:#}");
                }
            }
        }
        if state.is_none() {
            let executor = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("zork-client-mesh")
                .on_thread_start(|| {
                    zork_config::service::clear_signal_mask().expect("unblock Mesh worker signals")
                })
                .enable_all()
                .build()
                .context("create embedded client Mesh runtime")?;
            let (node, origin) = executor
                .block_on(async { crate::transport::start(&self.root, &config.mesh).await })?;
            self.handle.attach(&node.node())?;
            *state = Some(Embedded {
                executor,
                node,
                config: config.mesh.clone(),
                origin,
            });
        }
        let current = state.as_mut().context("embedded client Mesh missing")?;
        current.executor.block_on(zork_mesh::managed::configure(
            &self.root,
            &config.mesh,
            &self.control(),
        ))?;
        current.config = config.mesh;
        ensure!(!self.quitting.load(Ordering::Acquire), "客户端正在退出");
        let origin = current.origin.clone();
        std::fs::write(
            self.root.join("client-mesh-ready.json"),
            serde_json::to_vec(&serde_json::json!({
                "pid": std::process::id(), "origin": origin, "embedded": true
            }))?,
        )?;
        Ok(origin)
    }
}
impl Drop for ClientMesh {
    fn drop(&mut self) {
        let _completion = self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_station_identity_is_trusted_only_from_its_saved_snapshot() -> Result<()> {
        let root = tempfile::tempdir()?;
        let network = zork_config::MeshConfig {
            offline: true,
            ..Default::default()
        };
        let station = ClientMesh::new(root.path().join("station"));
        let origin = station.start_on(&[], Some(&network))?;
        let store = Arc::new(ClientStore::open(&root.path().join("client"))?);
        let local = SavedNode {
            id: "local".into(),
            name: "local Station".into(),
            url: "http://127.0.0.1:9".into(),
            token: None,
            local: true,
            mesh: None,
            group: None,
        };
        let remote_http = SavedNode {
            id: "remote-http".into(),
            local: false,
            ..local.clone()
        };
        store.put(&local.id, "mesh-origin", &origin)?;
        store.put(&remote_http.id, "mesh-origin", &origin)?;
        let client_root = root.path().join("client/transport");
        let client = ClientMesh::with_store(client_root.clone(), store);
        client.start_on(&[local, remote_http], Some(&network))?;
        let peers = zork_config::load_config(&client_root)?.mesh.peers;
        ensure!(
            peers.len() == 1 && peers[0].origin == origin,
            "local Station identity was omitted or inferred for a remote HTTP node"
        );
        ensure!(
            peers[0].addr.is_none(),
            "ephemeral loopback address leaked into durable configuration"
        );
        client.shutdown().blocking_recv()?;
        station.shutdown().blocking_recv()?;
        Ok(())
    }

    #[test]
    fn embedded_desktop_restarts_and_closes_with_no_binary() -> Result<()> {
        let root = tempfile::Builder::new()
            .prefix("zgui-mesh-")
            .tempdir_in("/tmp")?;
        let client = ClientMesh::new(root.path().into());
        let network = zork_config::MeshConfig {
            offline: true,
            ..Default::default()
        };
        let origin = client.start_on(&[], Some(&network))?;
        let retained = client.control();
        ensure!(
            !retained.data_dir().join("control.sock").exists(),
            "created a control socket"
        );
        zork_config::update_config(root.path(), |config| {
            config.mesh.bind = Some("127.0.0.1:0".into());
            config.mesh.synch_binary = Some(root.path().join("missing-synch"));
            Ok(())
        })?;
        ensure!(
            client.start_on(&[], Some(&network))? == origin,
            "restart changed identity"
        );
        {
            let state = client.embedded.lock().unwrap();
            ensure!(
                state
                    .as_ref()
                    .unwrap()
                    .executor
                    .block_on(retained.identity())?
                    == origin,
                "retained client handle did not follow the restarted library node"
            );
        }
        client.shutdown().blocking_recv()?;
        ensure!(
            !root.path().join("client-mesh-ready.json").exists(),
            "readiness leaked after shutdown"
        );
        ensure!(
            client.start_on(&[], Some(&network)).is_err(),
            "quitting client restarted"
        );
        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn embedded_workers_do_not_inherit_gpui_signal_masks() -> Result<()> {
        // Emulate a GPUI/libdispatch caller without changing another test thread.
        struct Restore(libc::sigset_t);
        impl Drop for Restore {
            fn drop(&mut self) {
                unsafe {
                    libc::pthread_sigmask(libc::SIG_SETMASK, &self.0, std::ptr::null_mut());
                }
            }
        }
        let _restore = unsafe {
            let mut mask = std::mem::zeroed();
            let mut previous = std::mem::zeroed();
            libc::sigemptyset(&mut mask);
            libc::sigaddset(&mut mask, libc::SIGUSR1);
            ensure!(
                libc::pthread_sigmask(libc::SIG_BLOCK, &mask, &mut previous) == 0,
                "block test signal"
            );
            Restore(previous)
        };
        let root = tempfile::Builder::new()
            .prefix("zgui-signal-")
            .tempdir_in("/tmp")?;
        let client = ClientMesh::new(root.path().into());
        client.start_on(
            &[],
            Some(&zork_config::MeshConfig {
                offline: true,
                ..Default::default()
            }),
        )?;
        {
            let state = client.embedded.lock().unwrap();
            let current = state.as_ref().unwrap();
            let blocked = current.executor.block_on(async {
                tokio::spawn(async {
                    unsafe {
                        let mut mask = std::mem::zeroed();
                        libc::pthread_sigmask(libc::SIG_BLOCK, std::ptr::null(), &mut mask);
                        libc::sigismember(&mask, libc::SIGUSR1) == 1
                    }
                })
                .await
            })?;
            ensure!(!blocked, "Mesh inherited a blocked eBPF signal");
        }
        client.shutdown().blocking_recv()?;
        Ok(())
    }
}
