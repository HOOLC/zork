//! A consumed receipt for an unchanged, fully closed local database.
//!
//! Restored or interrupted databases still need peer readoption. A receipt is
//! removed durably before opening the engine, and only replaced after all owned
//! tasks and the engine have closed successfully under the lifecycle lock.
use anyhow::Result;
use std::path::Path;

#[cfg(unix)]
mod unix {
    use super::*;
    use anyhow::ensure;
    use std::{fs::File, io::Write, os::unix::fs::MetadataExt};

    const RECEIPT: &str = "clean-start.json";

    fn fingerprint(path: &Path) -> Result<Option<String>> {
        let mut file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let stamp = |m: &std::fs::Metadata| {
            (
                m.dev(),
                m.ino(),
                m.len(),
                m.ctime(),
                m.ctime_nsec(),
                m.mtime(),
                m.mtime_nsec(),
            )
        };
        let before = stamp(&file.metadata()?);
        let mut hash = blake3::Hasher::new();
        // File identity/change time also invalidates a copied receipt + backup,
        // even if a restore preserves the old file contents and modification time.
        hash.update(&serde_json::to_vec(&before)?);
        hash.update_reader(&mut file)?;
        ensure!(
            before == stamp(&file.metadata()?),
            "Mesh database changed while closing"
        );
        Ok(Some(hash.finalize().to_hex().to_string()))
    }

    fn snapshot(data: &Path) -> Result<[Option<String>; 2]> {
        let database = fingerprint(&data.join(synch_store::DB_FILE))?;
        ensure!(database.is_some(), "Mesh database missing");
        Ok([
            database,
            fingerprint(&data.join(format!("{}-wal", synch_store::DB_FILE)))?,
        ])
    }

    pub fn consume(data: &Path) -> Result<bool> {
        let path = data.join(RECEIPT);
        let recorded = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<[Option<String>; 2]>(&bytes).ok());
        match std::fs::remove_file(&path) {
            Ok(()) => File::open(data)?.sync_all()?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        }
        Ok(
            recorded
                .is_some_and(|recorded| snapshot(data).is_ok_and(|current| current == recorded)),
        )
    }

    pub fn record(data: &Path) -> Result<()> {
        let snapshot = snapshot(data)?;
        let mut temp = tempfile::NamedTempFile::new_in(data)?;
        temp.write_all(&serde_json::to_vec(&snapshot)?)?;
        temp.as_file().sync_all()?;
        temp.persist(data.join(RECEIPT))?;
        File::open(data)?.sync_all()?;
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn receipt_is_consumed_and_a_copied_backup_cannot_reuse_it() -> Result<()> {
            let root = tempfile::tempdir()?;
            let database = root.path().join(synch_store::DB_FILE);
            std::fs::write(&database, b"closed database")?;
            record(root.path())?;
            assert!(consume(root.path())?);
            assert!(
                !consume(root.path())?,
                "an interrupted launch reused a receipt"
            );
            record(root.path())?;
            let replacement = root.path().join("restored.db");
            std::fs::copy(&database, &replacement)?;
            std::fs::rename(replacement, database)?;
            assert!(
                !consume(root.path())?,
                "a restored database reused a receipt"
            );
            Ok(())
        }

        #[test]
        fn a_changed_or_added_wal_requires_recovery() -> Result<()> {
            let root = tempfile::tempdir()?;
            std::fs::write(root.path().join(synch_store::DB_FILE), b"database")?;
            record(root.path())?;
            std::fs::write(
                root.path().join(format!("{}-wal", synch_store::DB_FILE)),
                b"new head",
            )?;
            assert!(!consume(root.path())?);
            Ok(())
        }
    }
}

#[cfg(unix)]
pub use unix::{consume, record};

// Keep the recovery path on platforms without the file identity/change-time
// checks above. A content-only receipt could itself come from an old backup.
#[cfg(not(unix))]
pub fn consume(_: &Path) -> Result<bool> {
    Ok(false)
}
#[cfg(not(unix))]
pub fn record(_: &Path) -> Result<()> {
    Ok(())
}
