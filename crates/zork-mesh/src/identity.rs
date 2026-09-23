//! Own the device key and exclusive lifetime of the current Mesh store.
use anyhow::{Context, Result};
use iroh::SecretKey;
use std::{io::Write, path::Path};

pub(crate) fn open(directory: &Path) -> Result<(std::fs::File, SecretKey)> {
    use fs2::FileExt;
    std::fs::create_dir_all(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
    }
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("lifecycle.lock"))?;
    lock.try_lock_exclusive()
        .context("Mesh identity is already in use")?;
    let path = directory.join("device.key");
    let key = match std::fs::read(&path) {
        Ok(bytes) => SecretKey::from_bytes(
            &bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("invalid Mesh identity"))?,
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let key = SecretKey::generate();
            let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
            temporary.write_all(&key.to_bytes())?;
            temporary.as_file().sync_all()?;
            temporary.persist_noclobber(&path)?;
            #[cfg(unix)]
            std::fs::File::open(directory)?.sync_all()?;
            key
        }
        Err(error) => return Err(error.into()),
    };
    Ok((lock, key))
}
