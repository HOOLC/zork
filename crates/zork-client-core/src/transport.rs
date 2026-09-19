//! Shared embedded client-only Mesh startup. Hosts supply paths, configuration
//! and a Tokio executor; no Station, Agent, workspace scanner or socket pool.
mod enrollment;
pub use enrollment::{resolve_invitation, Enrollment};
use std::path::Path;
use zork_config::MeshConfig;
use zork_mesh::managed;

/// The network lifetime is independent of optional cloud account sessions.
pub type Runtime = managed::Runtime;

pub async fn start(root: &Path, config: &MeshConfig) -> anyhow::Result<(Runtime, String)> {
    let mut runtime = managed::start_client(root, config).await?;
    let result = async {
        let node = runtime.node();
        managed::configure(root, config, &node).await?;
        node.identity().await
    }
    .await;
    match result {
        Ok(identity) => Ok((runtime, identity)),
        Err(error) => {
            let _ = runtime.shutdown().await;
            Err(error)
        }
    }
}
