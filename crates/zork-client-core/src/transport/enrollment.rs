//! Invitation transport uses device identity, independently of cloud accounts.
use anyhow::Result;
use std::{ops::Deref, path::Path, sync::Arc};
use zork_config::MeshConfig;

pub struct Enrollment {
    transport: Arc<zork_mesh::enrollment::Enrollment>,
}
impl Deref for Enrollment {
    type Target = zork_mesh::enrollment::Enrollment;
    fn deref(&self) -> &Self::Target {
        &self.transport
    }
}
impl Enrollment {
    pub async fn bind(root: &Path, config: &MeshConfig) -> Result<Self> {
        Self::bind_at(root, root, config).await
    }
    async fn bind_at(root: &Path, key_root: &Path, config: &MeshConfig) -> Result<Self> {
        let mut effective = config.clone();
        if !effective.offline {
            zork_config::services::ServicesConfig::load_for_data_root(
                &zork_config::relay_account::resolve_root(root)?,
            )?
            .apply_defaults(&mut effective)?;
        }
        let config = &effective;
        let transport = Arc::new(zork_mesh::enrollment::Enrollment::bind(key_root, config).await?);
        Ok(Self { transport })
    }
}
