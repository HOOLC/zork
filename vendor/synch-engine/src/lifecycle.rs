//! Exclusive ownership of a data directory's mutable lifecycle.
//!
//! One identity, one database, one CAS — and therefore one process allowed to
//! move any of them at a time. `docs/SERVERLESS.md` §1 states the deployment
//! half of this rule ("run exactly one daemon for a data directory/identity");
//! this is the half the code can enforce.
//!
//! [`Node::open`](crate::node::Node::open) deliberately does **not** take the lock
//! itself. The lock covers a *lifecycle* — a daemon run, an offline CAS
//! migration — which is longer than any one `Node`, and a host that owns
//! several of them (`docs/CLOUD-DATAPLANE.md` §4.1) wants to say so once per
//! data directory rather than have each open take a lock it cannot name.
//! Taking it is therefore the embedder's call, and the daemon is simply the
//! embedder that always makes it.

use std::io;
use std::path::Path;

/// The lock file inside the data directory.
///
/// A stable name, because the point is that two *different* programs —
/// `synch daemon run`, `synch cas migrate`, a hosting service embedding the
/// engine — collide on it. Changing it would silently let them run at once.
/// The lock file's name inside the data directory.
///
/// Public because a caller that clears a data directory has to keep it: the
/// lock is held on an open descriptor, so removing the file does not release
/// what this process holds, but it does let a second process create a fresh
/// one and acquire it — losing the mutual exclusion the lock exists for.
pub const LIFECYCLE_FILE: &str = "lifecycle.lock";

/// Process-held exclusive ownership of a data directory's mutable lifecycle.
///
/// Acquire before opening the [`synch_store::Store`] or any endpoint,
/// and hold for as long as the work lasts — the lock releases on drop, and on
/// process death, which is what makes it safe against a crash that never ran
/// any cleanup.
///
/// The lock is on the open file description, so it excludes other processes
/// *and* a second acquisition inside this one. That second property is the one
/// a multi-tenant host relies on: two tenants accidentally configured onto one
/// data directory fail at the second `acquire` instead of interleaving writes
/// into one database.
#[derive(Debug)]
pub struct LifecycleLock(std::fs::File);

impl LifecycleLock {
    /// Takes the lock, creating the data directory if it does not exist.
    ///
    /// Fails with [`io::ErrorKind::AddrInUse`] when another live holder has
    /// it. That is a refusal, not a wait: whoever holds it may hold it for
    /// hours, and a caller that queued behind it would look like a hang.
    pub fn acquire(data_dir: &Path) -> io::Result<Self> {
        use fs2::FileExt;
        harden_data_dir(data_dir)?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(data_dir.join(LIFECYCLE_FILE))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        file.try_lock_exclusive()
            .map_err(|_| already_running_error(data_dir))?;
        Ok(Self(file))
    }
}

impl Drop for LifecycleLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

/// The error a contended [`LifecycleLock::acquire`] returns.
fn already_running_error(data_dir: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::AddrInUse,
        format!(
            "a daemon is already running for this datadir ({})",
            data_dir.display()
        ),
    )
}

/// Creates the data directory `0700`, as every entry point into one must.
///
/// The database inside carries device secret keys (§3.1), so the directory is
/// hardened before anything is written into it rather than after.
pub fn harden_data_dir(data_dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(data_dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_holder_at_a_time_within_a_process() {
        let dir = tempfile::tempdir().unwrap();
        let held = LifecycleLock::acquire(dir.path()).unwrap();
        let error = LifecycleLock::acquire(dir.path()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AddrInUse);
        drop(held);
        // Released on drop, so the next lifecycle for the same directory —
        // a restarted tenant, a migration after a daemon stopped — proceeds.
        LifecycleLock::acquire(dir.path()).unwrap();
    }

    #[test]
    fn separate_directories_do_not_contend() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let _held_a = LifecycleLock::acquire(a.path()).unwrap();
        // The multi-tenant case: N data directories in one process is N
        // independent lifecycles (`docs/CLOUD-DATAPLANE.md` §4.1).
        let _held_b = LifecycleLock::acquire(b.path()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_data_directory_is_hardened() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("fresh");
        let _held = LifecycleLock::acquire(&nested).unwrap();
        let mode = std::fs::metadata(&nested).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}
