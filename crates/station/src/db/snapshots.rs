//! Immutable file bodies live in the published tree. SQL stores only references.
use super::*;
use std::io::{Read, Seek, SeekFrom, Write};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct Snapshot {
    pub root: String,
    pub name: String,
    pub byte_len: usize,
}

impl rusqlite::types::FromSql for Snapshot {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        serde_json::from_str(value.as_str()?)
            .map_err(|error| rusqlite::types::FromSqlError::Other(Box::new(error)))
    }
}
impl rusqlite::ToSql for Snapshot {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(serde_json::to_string(self)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?
            .into())
    }
}

impl GatewayDb {
    pub(super) fn snapshot_path(&self, snapshot: &Snapshot) -> Result<PathBuf> {
        let file = zork_client_types::files::FileRef {
            id: "file-snapshot".into(),
            name: snapshot.name.clone(),
            byte_len: snapshot.byte_len,
            content_root: snapshot.root.clone(),
        };
        anyhow::ensure!(file.valid(), "invalid_file_snapshot");
        Ok(self
            .files_root
            .join("attachments")
            .join(&snapshot.root)
            .join(&snapshot.name))
    }

    /// Finish and fsync the body before committing a message or artifact row.
    /// A failed SQL transaction can leave an unreferenced file, never a visible
    /// message referring to a partial upload. Identical submissions share bytes.
    pub(super) fn freeze_file(&self, name: &str, bytes: &[u8]) -> Result<Snapshot> {
        let snapshot = Snapshot {
            root: zork_mesh::content_root(bytes),
            name: name.into(),
            byte_len: bytes.len(),
        };
        let path = self.snapshot_path(&snapshot)?;
        let parent = path.parent().context("snapshot_parent_missing")?;
        fs::create_dir_all(parent)?;
        if path.try_exists()? {
            self.read_snapshot(&snapshot)?;
            return Ok(snapshot);
        }
        fs::create_dir_all(&self.file_staging)?;
        let mut staging = tempfile::NamedTempFile::new_in(&self.file_staging)?;
        staging.write_all(bytes)?;
        staging.as_file().sync_all()?;
        let mut permissions = staging.as_file().metadata()?.permissions();
        permissions.set_readonly(true);
        staging.as_file().set_permissions(permissions)?;
        match staging.persist_noclobber(&path) {
            Ok(_) => {}
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                self.read_snapshot(&snapshot)?;
            }
            Err(error) => return Err(error.error.into()),
        }
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(snapshot)
    }

    pub(super) fn read_snapshot(&self, snapshot: &Snapshot) -> Result<Vec<u8>> {
        let bytes = self.snapshot_range(snapshot, 0, snapshot.byte_len)?;
        anyhow::ensure!(
            bytes.len() == snapshot.byte_len && zork_mesh::content_root(&bytes) == snapshot.root,
            "file_snapshot_changed"
        );
        Ok(bytes)
    }

    pub(super) fn snapshot_range(
        &self,
        snapshot: &Snapshot,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<u8>> {
        anyhow::ensure!(offset <= snapshot.byte_len, "invalid_attachment_offset");
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options.open(self.snapshot_path(snapshot)?)?;
        let metadata = file.metadata()?;
        anyhow::ensure!(
            metadata.is_file() && metadata.len() == snapshot.byte_len as u64,
            "file_snapshot_changed"
        );
        file.seek(SeekFrom::Start(offset as u64))?;
        let mut bytes = Vec::with_capacity(limit.min(snapshot.byte_len - offset));
        file.take(limit.min(snapshot.byte_len - offset) as u64)
            .read_to_end(&mut bytes)?;
        Ok(bytes)
    }
}
