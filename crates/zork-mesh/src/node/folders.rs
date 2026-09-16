//! Synch metadata wakeups and explicit publication for tests or immediate writes.
use super::*;

impl MeshNode {
    pub async fn source_changes(&self) -> Result<zork_notify::files::Source> {
        let database = self.blocking(|n| Ok(n.store().db_path())).await?;
        let mut wal = database.as_os_str().to_owned();
        wal.push("-wal");
        Ok(zork_notify::files::Source::new([
            database,
            PathBuf::from(wal),
        ])?)
    }

    pub async fn scan_source(&self, space: &str) -> Result<()> {
        let node = self.engine()?;
        node.scan_source_and_stage_async(space).await?;
        node.flush_staged().await?;
        Ok(())
    }
}
