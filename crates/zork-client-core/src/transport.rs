//! Shared embedded client-only Mesh startup. Hosts supply paths, configuration
//! and a Tokio executor; no Station, Agent, workspace scanner or socket pool.
mod enrollment;
pub use enrollment::{resolve_invitation, Enrollment};
use std::path::Path;
use zork_config::MeshConfig;
use zork_mesh::managed;

/// Own account renewal for exactly the lifetime of this network runtime.
pub struct Runtime {
    network: managed::Runtime,
    account: Option<crate::relay_account::RelayAccountTask>,
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.account.take();
    }
}
impl Runtime {
    pub fn node(&self) -> zork_mesh::node::MeshNode {
        self.network.node()
    }
    pub fn is_finished(&self) -> bool {
        self.network.is_finished()
    }
    pub async fn wait(&mut self) -> anyhow::Result<()> {
        self.network.wait().await
    }
    pub async fn shutdown(&mut self) -> anyhow::Result<()> {
        self.account.take();
        self.network.shutdown().await
    }
}
pub fn own(root: &Path, config: &MeshConfig, network: managed::Runtime) -> anyhow::Result<Runtime> {
    let account = if config.offline {
        None
    } else {
        zork_config::relay_account::control_origin(config.relay_urls.as_deref())
            .map(|origin| {
                crate::relay_account::Account::new(root, &origin)?.attach_mesh(network.node())
            })
            .transpose()?
    };
    Ok(Runtime { network, account })
}
pub async fn start(root: &Path, config: &MeshConfig) -> anyhow::Result<(Runtime, String)> {
    let mut runtime = managed::start_client(root, config).await?;
    let result = async {
        let node = runtime.node();
        managed::configure(root, config, &node).await?;
        node.identity().await
    }
    .await;
    match result {
        Ok(identity) => Ok((own(root, config, runtime)?, identity)),
        Err(error) => {
            let _ = runtime.shutdown().await;
            Err(error)
        }
    }
}
