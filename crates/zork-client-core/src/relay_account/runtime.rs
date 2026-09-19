use super::*;
use std::future::Future;
use tokio::task::JoinHandle;

/// Short-lived relay admission. Never contains a refresh credential.
#[derive(Clone, PartialEq, Eq)]
pub struct Access {
    origin: String,
    token: String,
    expires_at: i64,
}
impl Access {
    pub fn origin(&self) -> &str {
        &self.origin
    }
    pub fn token(&self) -> &str {
        &self.token
    }
    pub fn expires_at(&self) -> i64 {
        self.expires_at
    }
}
impl std::fmt::Debug for Access {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelayAccess")
            .field("origin", &self.origin)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}
impl From<&RelaySession> for Access {
    fn from(value: &RelaySession) -> Self {
        Self {
            origin: value.origin.clone(),
            token: value.token.clone(),
            expires_at: value.expires_at,
        }
    }
}

/// Owned by a Station/client runtime, never by a view or observer.
pub struct RelayAccountTask {
    task: JoinHandle<()>,
}
impl Drop for RelayAccountTask {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Account {
    pub fn attach_mesh(&self, node: zork_mesh::node::MeshNode) -> Result<RelayAccountTask> {
        let origin = self.origin.clone();
        self.maintain(move |access| {
            let node = node.clone();
            let origin = origin.clone();
            async move {
                node.set_relay_access(&origin, access.as_ref().map(Access::token))
                    .await
            }
        })
    }

    /// The sink receives complete current values, including an initial None.
    /// File notifications are registered before reading. Only expiry deadlines
    /// and failed operations use timers; a logged-out idle client does not poll.
    pub fn maintain<F, Fut>(&self, mut apply: F) -> Result<RelayAccountTask>
    where
        F: FnMut(Option<Access>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        let source = zork_notify::files::Source::new([storage::path(&self.root)])?;
        let mut changes = source.subscribe();
        let account = self.clone();
        let task = tokio::spawn(async move {
            let _source = source;
            let mut applied: Option<Option<Access>> = None;
            let mut backoff = 1u64;
            loop {
                let mut failed = false;
                // Apply local logout/expiry before awaiting any remote work.
                let local = account.cached_access().unwrap_or(None);
                if applied.as_ref() != Some(&local) {
                    if apply(local.clone()).await.is_ok() {
                        applied = Some(local);
                    } else {
                        failed = true;
                    }
                }
                let work = async {
                    let pending = match account.flush_revocations().await {
                        Ok(count) => count > 0,
                        Err(_) => true,
                    };
                    (pending, account.access(false).await.is_err())
                };
                tokio::pin!(work);
                let (pending, refresh_failed) =
                    if let Some(access) = account.cached_access().unwrap_or(None) {
                        let expiry =
                            Duration::from_secs((access.expires_at - storage::now()).max(0) as u64);
                        tokio::select! {
                            result = &mut work => result,
                            _ = tokio::time::sleep(expiry) => {
                                if apply(None).await.is_ok() { applied = Some(None); }
                                else { failed = true; }
                                work.await
                            }
                        }
                    } else {
                        work.await
                    };
                failed |= refresh_failed;
                let local = account.cached_access().unwrap_or(None);
                if applied.as_ref() != Some(&local) {
                    if apply(local.clone()).await.is_ok() {
                        applied = Some(local.clone());
                    } else {
                        failed = true;
                    }
                }
                let file = storage::read(&account.root).ok();
                let session = file.as_ref().and_then(|f| account.matching(f));
                let now = storage::now();
                let mut wait =
                    session.map(|s| Duration::from_secs((s.expires_at - now - 60).max(1) as u64));
                if failed || pending {
                    let retry = Duration::from_secs(backoff);
                    // Access expiry is an independent deadline even during a
                    // network failure. Never retain an expired credential.
                    wait = Some(
                        local
                            .as_ref()
                            .map(|a| Duration::from_secs((a.expires_at - now).max(1) as u64))
                            .map_or(retry, |expiry| retry.min(expiry)),
                    );
                    backoff = (backoff * 2).min(60);
                } else {
                    backoff = 1;
                }
                if let Some(wait) = wait {
                    tokio::select! {
                        result = changes.changed() => { if result.is_err() { break; } },
                        _ = tokio::time::sleep(wait) => {}
                    }
                } else if changes.changed().await.is_err() {
                    break;
                }
            }
        });
        Ok(RelayAccountTask { task })
    }
}
