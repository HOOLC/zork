use super::*;

impl MeshNode {
    /// Replace only the configured relay routes owned by this account issuer.
    /// Removal closes existing relay sockets; direct QUIC, discovery, identity,
    /// and the relay's original QAD configuration are preserved.
    pub async fn set_relay_access(&self, origin: &str, token: Option<&str>) -> Result<()> {
        let active = self
            .active
            .read()
            .expect("Mesh node")
            .clone()
            .context("Mesh node is not running")?;
        let engine = active
            .node
            .read()
            .expect("Mesh engine")
            .clone()
            .context("Mesh node is closed")?;
        if engine.config().net.offline {
            return Ok(());
        }
        let endpoint = engine.net().endpoint().clone();
        active
            .relay_configs
            .apply(&endpoint, &engine.config().net.relay_urls, origin, token)
            .await
    }
}
