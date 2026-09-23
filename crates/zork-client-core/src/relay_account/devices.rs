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
            if let Err(error) = &result {
                tracing::warn!(%error, "account device discovery will retry");
            }
            if authenticated || result.is_err() {
                tokio::select! { _ = tokio::time::sleep(Duration::from_secs(15)) => {}, changed = changes.changed() => if changed.is_err() { break } }
            } else if changes.changed().await.is_err() {
                break;
            }
        }
    })))
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
