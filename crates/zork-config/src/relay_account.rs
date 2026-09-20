//! Private, origin-bound relay credentials. Configuration files never contain them.
use anyhow::{ensure, Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RelaySession {
    pub origin: String,
    pub token: String,
    pub refresh_token: String,
    pub subject: String,
    pub email: Option<String>,
    pub session_id: String,
    pub expires_at: i64,
    pub session_expires_at: i64,
    pub refresh_expires_at: i64,
    /// Persisted before a refresh request so a crash can retry the same rotation.
    pub pending_refresh: Option<String>,
}
impl std::fmt::Debug for RelaySession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelaySession")
            .field("origin", &self.origin)
            .field("subject", &self.subject)
            .field("session_id", &self.session_id)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}
impl RelaySession {
    pub fn active(&self) -> bool {
        !self.token.is_empty()
            && self.expires_at > now()
            && self.session_expires_at > now()
            && self.refresh_expires_at > now()
    }
    pub fn renewable(&self) -> bool {
        !self.refresh_token.is_empty()
            && self.session_expires_at > now()
            && self.refresh_expires_at > now()
    }
}
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Revocation {
    pub session: RelaySession,
    pub all: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountFile {
    pub version: u8,
    pub current: Option<RelaySession>,
    pub pending_revocations: Vec<Revocation>,
    /// Binds an in-flight browser login to the operation that still owns it.
    pub login_attempt: Option<String>,
}
impl Default for AccountFile {
    fn default() -> Self {
        Self {
            version: 2,
            current: None,
            pending_revocations: Vec::new(),
            login_attempt: None,
        }
    }
}

pub fn path(root: &Path) -> PathBuf {
    root.join("account/relay.json")
}

/// An app-owned node and transport share the profile's account, while retaining
/// their own Mesh identities. The marker contains no path or credential.
pub fn bind_profile(profile: &Path) -> Result<()> {
    fs::create_dir_all(profile)?;
    for name in ["node", "transport"] {
        let child = profile.join(name);
        let _lock = try_lock(&child)?.context("account is in use while binding the profile")?;
        let marker = child.join("account/profile");
        if marker.exists() {
            ensure!(
                resolve_root(&child)? == fs::canonicalize(profile)?,
                "invalid account profile binding"
            );
            continue;
        }
        let old = read(&child)?;
        ensure!(
            old.current.is_none()
                && old.pending_revocations.is_empty()
                && old.login_attempt.is_none(),
            "the owned node has an independent account; log it out before opening this profile"
        );
        let mut file = options().write(true).create_new(true).open(marker)?;
        file.write_all(b"zork-profile-v1\n")?;
        file.sync_all()?;
    }
    Ok(())
}

pub fn resolve_root(root: &Path) -> Result<PathBuf> {
    let marker = root.join("account/profile");
    let mut file = match options().read(true).open(&marker) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(root.to_owned()),
        Err(error) => return Err(error).context("read account profile binding"),
    };
    ensure!(
        file.metadata()?.is_file() && file.metadata()?.len() == 16,
        "invalid account profile binding"
    );
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    ensure!(
        contents == "zork-profile-v1\n",
        "invalid account profile binding"
    );
    let root = fs::canonicalize(root)?;
    ensure!(
        matches!(
            root.file_name().and_then(|n| n.to_str()),
            Some("node" | "transport")
        ),
        "invalid account profile child"
    );
    let parent = root.parent().context("account profile parent missing")?;
    ensure!(
        !parent.join("account/profile").exists(),
        "nested account profile bindings are not allowed"
    );
    Ok(parent.to_owned())
}

pub fn canonical_origin(value: &str) -> Result<String> {
    let url = url::Url::parse(value).context("invalid relay origin")?;
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "relay origin must not contain credentials or query data"
    );
    ensure!(
        url.scheme() == "https"
            || (url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "[::1]"))),
        "relay account requires HTTPS (HTTP is allowed only on loopback)"
    );
    ensure!(url.host_str().is_some(), "relay origin must have a host");
    Ok(url.origin().ascii_serialization())
}
pub fn control_origin(relay_urls: Option<&[String]>) -> Option<String> {
    canonical_origin(relay_urls?.first()?).ok()
}

fn private_dir(root: &Path) -> Result<PathBuf> {
    let dir = root.join("account");
    ensure!(
        !fs::symlink_metadata(&dir).is_ok_and(|m| m.file_type().is_symlink()),
        "account directory must not be a symlink"
    );
    fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

fn options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    options
}

/// The owner keeps this lock through read, network rotation, and atomic replace.
pub struct AccountLock(File);

impl Drop for AccountLock {
    fn drop(&mut self) {
        // Closing alone can leave the lock held by a concurrently forked
        // child until it execs. The account owner ends the critical section.
        let _ = FileExt::unlock(&self.0);
    }
}

pub fn try_lock(root: &Path) -> Result<Option<AccountLock>> {
    let dir = private_dir(root)?;
    let lock = options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("relay.lock"))?;
    match lock.try_lock_exclusive() {
        Ok(()) => Ok(Some(AccountLock(lock))),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub fn read(root: &Path) -> Result<AccountFile> {
    let path = path(root);
    let mut file = match options().read(true).open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(AccountFile::default());
        }
        Err(error) => return Err(error).context("read private relay account"),
    };
    ensure!(
        file.metadata()?.is_file() && file.metadata()?.len() <= 512 * 1024,
        "invalid relay account file"
    );
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid relay account file"))?;
    // The old unsigned/display-only JWT file cannot establish an origin-bound
    // session. A new Google login is required; never send that credential.
    if value.get("version").is_none() {
        return Ok(AccountFile::default());
    }
    let account: AccountFile =
        serde_json::from_value(value).map_err(|_| anyhow::anyhow!("invalid relay account file"))?;
    ensure!(account.version == 2, "unsupported relay account format");
    if let Some(session) = &account.current {
        validate(session)?;
    }
    for revocation in &account.pending_revocations {
        validate(&revocation.session)?;
    }
    Ok(account)
}
pub fn load(root: &Path) -> Result<Option<RelaySession>> {
    Ok(read(root)?.current)
}

fn validate(session: &RelaySession) -> Result<()> {
    ensure!(
        canonical_origin(&session.origin)? == session.origin,
        "noncanonical relay origin"
    );
    ensure!(
        !session.token.is_empty()
            && session.token.len() <= 4096
            && !session.refresh_token.is_empty()
            && session.refresh_token.len() <= 4096,
        "invalid relay credential"
    );
    ensure!(
        !session.subject.is_empty()
            && session.subject.len() <= 128
            && session.session_id.len() == 26,
        "invalid relay session identity"
    );
    ensure!(
        session.expires_at <= session.session_expires_at
            && session.refresh_expires_at <= session.session_expires_at,
        "invalid relay session expiry"
    );
    Ok(())
}

/// Call only while holding try_lock. Restrictive permissions apply at creation,
/// before any credential bytes are written. Both file and rename are durable.
pub fn write(root: &Path, account: &AccountFile) -> Result<()> {
    if let Some(session) = &account.current {
        validate(session)?;
    }
    ensure!(
        account.version == 2 && account.pending_revocations.len() <= 64,
        "invalid relay account state"
    );
    let dir = private_dir(root)?;
    let temporary = dir.join(format!(".relay-{}.tmp", crate::random_token()));
    let result = (|| -> Result<()> {
        let mut file = options().write(true).create_new(true).open(&temporary)?;
        file.write_all(&serde_json::to_vec(account)?)?;
        file.sync_all()?;
        fs::rename(&temporary, path(root))?;
        #[cfg(unix)]
        File::open(&dir)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> RelaySession {
        RelaySession {
            origin: "https://relay.example".into(),
            token: "access-secret".into(),
            refresh_token: "refresh-secret".into(),
            subject: "google-sub".into(),
            email: None,
            session_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".into(),
            expires_at: now() + 300,
            session_expires_at: now() + 3600,
            refresh_expires_at: now() + 1800,
            pending_refresh: None,
        }
    }
    #[test]
    fn round_trip_private_permissions_and_exclusive_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let lock = try_lock(dir.path()).unwrap().unwrap();
        assert!(try_lock(dir.path()).unwrap().is_none());
        let mut account = AccountFile::default();
        account.current = Some(session());
        write(dir.path(), &account).unwrap();
        assert_eq!(load(dir.path()).unwrap(), account.current);
        assert!(!format!("{account:?}").contains("secret"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path(dir.path())).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(dir.path().join("account"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        drop(lock);
        assert!(try_lock(dir.path()).unwrap().is_some());
    }
    #[cfg(unix)]
    #[test]
    fn dropping_owner_unlocks_even_with_an_inherited_descriptor() {
        let dir = tempfile::tempdir().unwrap();
        let owner = try_lock(dir.path()).unwrap().unwrap();
        // A concurrently forked child can retain the same open-file
        // description until exec, even when CLOEXEC is set.
        let inherited = owner.0.try_clone().unwrap();
        drop(owner);
        assert!(try_lock(dir.path()).unwrap().is_some());
        drop(inherited);
    }
    #[test]
    fn legacy_credentials_and_insecure_or_credential_origins_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        private_dir(dir.path()).unwrap();
        fs::write(
            path(dir.path()),
            r#"{"token":"old-secret","expires_at":4102444800}"#,
        )
        .unwrap();
        assert!(load(dir.path()).unwrap().is_none());
        for origin in [
            "http://relay.example",
            "https://user:secret@relay.example",
            "https://relay.example?token=secret",
            "file:///tmp",
        ] {
            assert!(canonical_origin(origin).is_err(), "{origin}");
        }
        assert_eq!(
            canonical_origin("https://relay.example/relay").unwrap(),
            "https://relay.example"
        );
        assert_eq!(
            canonical_origin("http://127.0.0.1:1234").unwrap(),
            "http://127.0.0.1:1234"
        );
    }

    #[test]
    fn obsolete_config_bearer_is_discarded_and_never_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::ensure_layout(dir.path()).unwrap();
        let mut value = serde_json::to_value(config).unwrap();
        value["mesh"]["relay_token"] = "obsolete-secret".into();
        fs::write(
            crate::config_path(dir.path()),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        let loaded = crate::load_config(dir.path()).unwrap();
        assert!(!serde_json::to_string(&loaded)
            .unwrap()
            .contains("relay_token"));
        assert!(load(dir.path()).unwrap().is_none());
    }
    #[cfg(unix)]
    #[test]
    fn never_follow_private_file_or_directory_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(other.path(), dir.path().join("account")).unwrap();
        assert!(try_lock(dir.path()).is_err());
        fs::remove_file(dir.path().join("account")).unwrap();
        private_dir(dir.path()).unwrap();
        std::os::unix::fs::symlink(other.path().join("target"), path(dir.path())).unwrap();
        assert!(read(dir.path()).is_err());
    }
}
