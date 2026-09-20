mod pool;
pub use pool::{compatible_model, AccountLease, AccountPool};
mod app;
mod discovery;
mod execution;
pub use discovery::{
    discover_models, refresh_models, DiscoveredModel, ModelDiscovery, ModelUpdate,
};
pub mod providers;
mod storage;

pub use app::{
    delete as delete_profile, get as get_profile, get_with_status as get_profile_with_status,
    list as list_profiles, list_with_status as list_profiles_with_status, put as put_profile,
    read as read_profile, select_model, set_model_enabled, update_models, update_models_checked,
    update_name, ModelApi, ModelCapabilities, ModelLimits, ProfileDocument, ProfileModel,
    ProfileView,
};
pub use execution::{load_selected, ProviderExecution};
pub use providers::{catalog as list_providers, AuthProvider, DeviceCode, DeviceCodePoll};
pub use storage::{DataRootPaths, MemoryStore, ProfilePaths, ProfileStore};

use anyhow::Result;
use reqwest::Client;
use serde_json::json;
use tracing::error;

pub async fn refresh_profile(
    paths: &impl ProfilePaths,
    statuses: &impl ProfileStore,
    http: &Client,
    profile_id: &str,
) -> Result<()> {
    let document = read_profile(paths, profile_id)?;
    let probed_auth = document.auth.clone();
    let provider = providers::get(&document.provider)?;
    let request_document = serde_json::to_value(&document)?;
    match provider.probe(http, &request_document).await {
        Ok(snapshot) => {
            if app::commit_probe_auth(paths, profile_id, &probed_auth, snapshot.auth)? {
                statuses.upsert_probe(profile_id, &snapshot.account, &snapshot.rate_limits)?;
            }
        }
        Err(probe_error) => {
            let failed = json!({ "ok": false, "error": probe_error.to_string() });
            if app::commit_probe_auth(paths, profile_id, &probed_auth, None)? {
                statuses.upsert_probe(profile_id, &failed, &failed)?;
            }
            error!(profile = %profile_id, error = %probe_error, "profile status probe failed");
        }
    }
    Ok(())
}
