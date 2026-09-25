//! Account-owned discovery; UI never polls or grants peers.
use super::*;
use serde_json::json;
use std::{future::Future, sync::Arc};
use zork_mesh::node::MeshNode;

#[derive(Clone, Debug, Deserialize)]
pub struct Device {
    pub origin: String,
    pub name: String,
    pub station: bool,
    pub address: serde_json::Value,
    pub channel: String,
}
#[derive(Deserialize)]
struct Directory {
    devices: Vec<Device>,
}

pub fn start<F, Fut>(
    root: &Path,
    node: MeshNode,
    station: bool,
    mut apply: F,
) -> Result<zork_notify::Task<()>>
where
    F: FnMut(Vec<Device>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    let account = Account::configured(root)?;
    let watch = zork_notify::files::Source::new([storage::path(account.data_root())])?;
    let mut changes = watch.subscribe();
    let channel = serde_json::to_value(zork_config::channel::current()?)?;
    Ok(zork_notify::Task(tokio::spawn(async move {
        let _watch = watch;
        let mut logged_out_applied = false;
        let mut backoff = Backoff::default();
        loop {
            changes.checkpoint();
            let mut authenticated = false;
            let result = async {
                let access = account.access(false).await?;
                let Some(access) = access else {
                    if !logged_out_applied { apply(vec![]).await?; logged_out_applied = true; }
                    return Ok::<_, anyhow::Error>(());
                };
                authenticated = true;
                logged_out_applied = false;
                let file = storage::read(account.data_root())?;
                let session = account.matching(&file).context("account changed")?;
                let request = json!({"origin":node.identity().await?,"name":zork_config::device_name(),"station":station,"address":node.address()?,"channel":channel,"signature":node.account_proof(&session.session_id)?});
                let directory: Directory = account.request(Method::PUT, "/v1/auth/devices", Some(access.token()), Some(request)).await?;
                ensure!(directory.devices.len() <= 64, "account device limit exceeded");
                // A late reply from a logged-out or replaced account never grants access.
                let _owner = account.lock().await?;
                ensure!(account.cached_access()?.as_ref() == Some(&access), "account changed");
                for device in &directory.devices {
                    zork_mesh::node::validate_peer_address(&device.origin, &device.address)?;
                    ensure!(device.channel == channel.as_str().unwrap_or_default(), "invalid account device channel");
                }
                apply(directory.devices).await
            }.await;
            let wait = match &result {
                Ok(()) => {
                    backoff = Backoff::default();
                    authenticated.then_some(Duration::from_secs(15))
                }
                Err(error) => Some(backoff.failed(error)),
            };
            // Only a different session (login, logout, account switch) cuts a
            // failure backoff short. Our own refresh writes and lock files
            // touch the same account file and must not turn into a fast loop.
            let identity = session_identity(&account);
            let deadline = wait.map(|wait| tokio::time::Instant::now() + wait);
            loop {
                let changed = match deadline {
                    Some(deadline) => tokio::select! {
                        _ = tokio::time::sleep_until(deadline) => break,
                        changed = changes.changed() => changed,
                    },
                    None => changes.changed().await,
                };
                if changed.is_err() {
                    return;
                }
                if result.is_ok() || session_identity(&account) != identity {
                    break;
                }
                changes.checkpoint();
            }
        }
    })))
}

fn session_identity(account: &Account) -> Option<String> {
    storage::read(account.data_root())
        .ok()
        .and_then(|file| account.matching(&file).map(|s| s.session_id.clone()))
}

/// Failure pacing for account discovery. A relay without the device endpoint
/// (HTTP 404) is a deployment mismatch, not a transient fault: back off hard
/// and log it once instead of warning on every attempt.
#[derive(Default)]
struct Backoff {
    failures: u32,
    missing_logged: bool,
}
impl Backoff {
    fn failed(&mut self, error: &anyhow::Error) -> Duration {
        self.failures = self.failures.saturating_add(1);
        if endpoint_missing(error) {
            if !self.missing_logged {
                self.missing_logged = true;
                tracing::warn!(
                    "relay has no account device discovery endpoint (HTTP 404); retrying rarely"
                );
            } else {
                tracing::debug!(%error, "account device discovery endpoint still missing");
            }
            // 10 min, 20 min, … capped at 6 h.
            return Duration::from_secs(
                (600u64 << self.failures.min(7).saturating_sub(1)).min(6 * 3600),
            );
        }
        if error.downcast_ref::<SignedOut>().is_some() {
            // The session ended; the next file change (a new login) restarts us.
            tracing::info!(%error, "account device discovery paused until sign-in");
            return Duration::from_secs(3600);
        }
        tracing::warn!(%error, "account device discovery will retry");
        // 15 s doubling to 10 min.
        Duration::from_secs((15u64 << self.failures.min(6).saturating_sub(1)).min(600))
    }
}

#[cfg(test)]
mod backoff_tests {
    use super::*;
    #[test]
    fn missing_endpoint_backs_off_hard_and_signed_out_waits_for_login() {
        let missing = anyhow::Error::new(ApiError {
            status: StatusCode::NOT_FOUND,
            code: "request_failed",
        });
        let mut backoff = Backoff::default();
        assert_eq!(backoff.failed(&missing), Duration::from_secs(600));
        assert!(backoff.missing_logged);
        assert_eq!(backoff.failed(&missing), Duration::from_secs(1200));
        for _ in 0..10 {
            backoff.failed(&missing);
        }
        assert_eq!(backoff.failed(&missing), Duration::from_secs(6 * 3600));
        let transient = anyhow::anyhow!("relay control plane is unavailable");
        let mut backoff = Backoff::default();
        assert_eq!(backoff.failed(&transient), Duration::from_secs(15));
        assert_eq!(backoff.failed(&transient), Duration::from_secs(30));
        let signed_out = anyhow::Error::new(SignedOut {
            code: "refresh_reused",
        });
        assert!(Backoff::default().failed(&signed_out) >= Duration::from_secs(3600));
    }
}

pub(crate) fn start_client(
    root: &Path,
    node: MeshNode,
    store: Arc<crate::store::ClientStore>,
) -> Result<zork_notify::Task<()>> {
    start(root, node.clone(), false, move |devices| {
        let store = store.clone();
        let node = node.clone();
        async move {
            let devices: Vec<_> = devices
                .into_iter()
                .filter(|device| device.station)
                .collect();
            let candidates = devices
                .iter()
                .map(|device| crate::store::SavedNode {
                    machine_name: None,
                    color_key: None,
                    id: device.origin.clone(),
                    name: device.name.clone(),
                    url: String::new(),
                    token: None,
                    local: false,
                    mesh: Some(crate::store::RemoteNode {
                        origin: device.origin.clone(),
                        addr: None,
                        routes: None,
                    }),
                    group: None,
                })
                .collect::<Vec<_>>();
            for removed in store.replace_account_nodes(&candidates)? {
                node.untrust(&removed).await?;
            }
            for device in devices {
                node.trust(&device.origin, &device.name, None).await?;
                node.remember_peer_address(&device.origin, device.address)
                    .await?;
            }
            Ok(())
        }
    })
}
