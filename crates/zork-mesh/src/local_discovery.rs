//! Same-user, same-host address hints for iroh's existing QUIC transport.
//! A hint grants no trust. iroh still checks membership and the peer's key.
use anyhow::{ensure, Context, Result};
use fs2::FileExt;
use futures_util::{future, stream, StreamExt};
use iroh::{
    address_lookup::{AddressLookup, Error, Item},
    Endpoint, EndpointAddr, EndpointId,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::Read,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Hint {
    endpoint: String,
    addresses: Vec<SocketAddr>,
}

#[derive(Debug, Clone)]
struct LocalLookup {
    directory: PathBuf,
}

enum Located {
    Unknown,
    Stopped,
    Active(EndpointAddr),
}

impl LocalLookup {
    fn locate(&self, id: EndpointId) -> Result<Located> {
        let path = self.directory.join(format!("{id}.json"));
        let file = match File::open(&path) {
            Ok(file) => file,
            _ => return Ok(Located::Unknown),
        };
        // Probe the same inode we read. Locking a separate pathname could
        // mistake an old port for a newly started registration after rename.
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(Located::Stopped),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error.into()),
        }
        let mut bytes = Vec::new();
        file.take(4097).read_to_end(&mut bytes)?;
        if bytes.len() > 4096 {
            return Ok(Located::Unknown);
        }
        let hint: Hint = serde_json::from_slice(&bytes)?;
        ensure!(
            hint.endpoint == id.to_string(),
            "local endpoint hint identity mismatch"
        );
        ensure!(
            !hint.addresses.is_empty()
                && hint.addresses.len() <= 8
                && hint
                    .addresses
                    .iter()
                    .all(|addr| addr.ip().is_loopback() && addr.port() != 0),
            "invalid loopback endpoint hint"
        );
        let addr = hint
            .addresses
            .into_iter()
            .fold(EndpointAddr::new(id), |addr, ip| addr.with_ip_addr(ip));
        Ok(Located::Active(addr))
    }
}

impl AddressLookup for LocalLookup {
    fn resolve(
        &self,
        id: EndpointId,
    ) -> Option<stream::BoxStream<'static, std::result::Result<Item, Error>>> {
        let lookup = self.clone();
        Some(Box::pin(
            stream::once(async move {
                let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
                let source =
                    zork_notify::files::Source::new([lookup.directory.join(format!("{id}.json"))])
                        .ok()?;
                let mut changes = source.subscribe();
                loop {
                    changes.checkpoint();
                    let reader = lookup.clone();
                    match tokio::task::spawn_blocking(move || reader.locate(id)).await {
                        Ok(Ok(Located::Active(addr))) => {
                            return Some(Ok(Item::new(addr.into(), "zork_loopback", None)))
                        }
                        // A previously local peer may be starting alongside us.
                        // Remote/unknown identities never incur this local wait.
                        Ok(Ok(Located::Stopped)) if tokio::time::Instant::now() < deadline => {
                            if !matches!(
                                tokio::time::timeout_at(deadline, changes.changed()).await,
                                Ok(Ok(()))
                            ) {
                                return None;
                            }
                        }
                        _ => return None,
                    }
                }
            })
            .filter_map(future::ready),
        ))
    }
}

/// Kept until the endpoint shuts down. The cached hint is intentionally retained
/// so a later simultaneous restart can wait briefly for its new leased address.
pub(crate) struct Registration {
    _hint_lease: File,
    _identity_lease: File,
}

fn directory() -> Option<PathBuf> {
    // Transport diagnostics can deliberately exercise a relay between local
    // test processes, simulating separate hosts instead of taking loopback.
    if std::env::var_os("ZORK_MESH_LOCAL_DISCOVERY").is_some_and(|value| value == "0") {
        return None;
    }
    let home = std::env::var_os("HOME")?;
    let home = PathBuf::from(home);
    if !home.is_absolute() {
        return None;
    }
    #[cfg(target_os = "macos")]
    let cache = home.join("Library/Caches");
    #[cfg(not(target_os = "macos"))]
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join(".cache"));
    Some(cache.join("zork/mesh-loopback-v1"))
}

fn loopback_addresses(endpoint: &Endpoint) -> Vec<SocketAddr> {
    endpoint
        .bound_sockets()
        .into_iter()
        .filter_map(|mut addr| {
            if addr.ip().is_unspecified() {
                addr.set_ip(match addr.ip() {
                    IpAddr::V4(_) => Ipv4Addr::LOCALHOST.into(),
                    IpAddr::V6(_) => Ipv6Addr::LOCALHOST.into(),
                });
            }
            // A socket explicitly bound to a LAN interface cannot receive loopback
            // traffic. Preserve that deliberate interface selection.
            (addr.ip().is_loopback() && addr.port() != 0).then_some(addr)
        })
        .collect()
}

fn register(directory: &Path, id: EndpointId, addresses: Vec<SocketAddr>) -> Result<Registration> {
    std::fs::create_dir_all(directory)?;
    ensure!(
        !std::fs::symlink_metadata(directory)?
            .file_type()
            .is_symlink(),
        "local endpoint directory is a symlink"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
    }
    let path = directory.join(format!("{id}.json"));
    let lease = zork_config::service::exclusive_lock(&path.with_extension("lock"))?;
    let mut temp = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer(
        &mut temp,
        &Hint {
            endpoint: id.to_string(),
            addresses,
        },
    )?;
    temp.as_file().try_lock_exclusive()?;
    let hint_lease = temp.persist(path).context("publish local endpoint hint")?;
    Ok(Registration {
        _hint_lease: hint_lease,
        _identity_lease: lease,
    })
}

pub(crate) async fn install(endpoint: &Endpoint) -> Result<Option<Registration>> {
    let Some(directory) = directory() else {
        return Ok(None);
    };
    let addresses = loopback_addresses(endpoint);
    let id = endpoint.id();
    let writer = directory.clone();
    let registration = if addresses.is_empty() {
        None
    } else {
        Some(tokio::task::spawn_blocking(move || register(&writer, id, addresses)).await??)
    };
    endpoint
        .address_lookup()
        .context("iroh address lookup is unavailable")?
        .add(LocalLookup { directory });
    Ok(registration)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_live_registration_resolves_and_restart_replaces_the_port() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let id = iroh::SecretKey::generate().public();
        let lookup = LocalLookup {
            directory: directory.path().into(),
        };
        let first: SocketAddr = "127.0.0.1:12345".parse()?;
        let registration = register(directory.path(), id, vec![first])?;
        assert!(matches!(lookup.locate(id)?, Located::Active(_)));
        drop(registration);
        assert!(matches!(lookup.locate(id)?, Located::Stopped));
        let second: SocketAddr = "127.0.0.1:23456".parse()?;
        let _registration = register(directory.path(), id, vec![second])?;
        let Located::Active(addr) = lookup.locate(id)? else {
            panic!("new registration missing");
        };
        assert_eq!(addr.ip_addrs().copied().collect::<Vec<_>>(), vec![second]);
        Ok(())
    }

    #[test]
    fn local_hints_cannot_redirect_to_a_non_loopback_address() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let id = iroh::SecretKey::generate().public();
        let _registration = register(directory.path(), id, vec!["192.0.2.1:12345".parse()?])?;
        let lookup = LocalLookup {
            directory: directory.path().into(),
        };
        ensure!(lookup.locate(id).is_err(), "accepted a non-loopback hint");
        Ok(())
    }
}
