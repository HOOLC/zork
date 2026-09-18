//! A platform settings snapshot reads the same committed product replica.
//! Legacy owners have one explicitly scoped public cache, never arbitrary HTTP.
use crate::{api::StationClient, state::Device, store::ClientStore};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::sync::Arc;
use zork_client_types::sync::Kind;

fn present_profile(mut profile: Value) -> Value {
    profile["verified"] = json!(profile["account"]["verified"]
        .as_bool()
        .or_else(|| profile["account"]["ok"].as_bool())
        .or_else(|| profile["verified"].as_bool())
        .unwrap_or(false));
    // The mobile adapter consumes the same normalized quota as desktop. It
    // must not interpret provider-specific account/usage payloads itself.
    profile["quota"] = json!(crate::api::ProfileQuota::from_value(&profile["rateLimits"]));
    profile
}

pub(crate) fn cached(store: &ClientStore, peer: &str) -> Result<Value> {
    if store.replica_revoked(peer)? {
        return Ok(
            json!({"ready":false,"revoked":true,"cached":true,"error":"设备访问权限已撤销"}),
        );
    }
    if let Some((cursor, records)) = store.replica_catalog(peer)? {
        let mut info = Value::Null;
        let mut profile_ready = true;
        let mut profile_error = Value::Null;
        let mut agents = vec![];
        let mut profiles = vec![];
        let mut providers = vec![];
        for record in records {
            let Some(value) = record.value else {
                continue;
            };
            match record.kind {
                Kind::Device => info = value,
                Kind::Resource if record.id == "profiles" => {
                    profile_ready = value["ready"].as_bool().unwrap_or(false);
                    profile_error = value["error"].clone();
                }
                Kind::Agent => agents.push(value),
                Kind::Profile => profiles.push(present_profile(value)),
                Kind::Provider => providers.push(value),
                _ => {}
            }
        }
        return Ok(
            json!({"ready":true,"cached":true,"info":info,"agents":agents,"profiles":profiles,"providers":providers,"cursor":cursor,"profiles_ready":profile_ready,"profiles_error":profile_error}),
        );
    }
    let mut snapshot = store
        .get::<Value>(peer, "public-settings")?
        .unwrap_or_else(|| json!({"ready":false,"cached":true}));
    if let Some(profiles) = snapshot["profiles"].as_array_mut() {
        for profile in profiles {
            *profile = present_profile(profile.take());
        }
    }
    Ok(snapshot)
}

pub(crate) fn snapshot(store: &ClientStore, peer: &str) -> Result<Value> {
    if store.replica_revoked(peer)? {
        return Ok(
            json!({"ready":false,"revoked":true,"cached":true,"error":"设备访问权限已撤销"}),
        );
    }
    let device = store
        .1
        .lock()
        .unwrap()
        .get(peer)
        .and_then(std::sync::Weak::upgrade);
    let state = device.as_ref().map(|d| d.snapshot());
    let profiles = device.as_ref().map(|d| d.profiles().snapshot());
    let saved_authorization: Option<Value> = store.get(peer, "profile-authorization")?;
    let authorization = profiles
        .as_ref()
        .and_then(|p| p.authorization.clone())
        .or_else(|| {
            saved_authorization
                .as_ref()
                .filter(|value| value.is_object() && value["completed"] != true)
                .cloned()
        });
    let authorization_busy = profiles.as_ref().is_some_and(|p| p.authorization_busy);
    let authorization_error = profiles
        .as_ref()
        .and_then(|p| p.authorization_error.clone());
    let authorization_complete = profiles.as_ref().is_some_and(|p| p.authorization_complete)
        || saved_authorization
            .as_ref()
            .is_some_and(|v| v["completed"] == true);
    let operation: Option<Value> = store.get(peer, "node-operation")?;
    let command: Option<Value> = store.get(peer, "settings-command")?;
    let update_check: Option<Value> = store.get(peer, "node-update-check")?;
    let online = state.as_ref().is_some_and(|s| {
        s.online == Some(true)
            && s.confirmed_at_ms
                .is_some_and(|at| crate::store::delivery_now_ms().saturating_sub(at) < 60_000)
    });
    let error = state.as_ref().and_then(|s| s.connection_error.clone());
    let mut snapshot = cached(store, peer)?;
    if snapshot["revoked"] == true {
        return Ok(snapshot);
    }
    // Storage checkpoints and legacy cache tokens are not UI revisions. A
    // committed cursor-only advance must not turn unchanged settings into data.
    if let Some(fields) = snapshot.as_object_mut() {
        fields.remove("cursor");
        fields.remove("version");
    }
    snapshot["authorization"] = json!(authorization);
    snapshot["authorization_busy"] = json!(authorization_busy);
    snapshot["authorization_error"] = json!(authorization_error);
    snapshot["authorization_complete"] = json!(authorization_complete);
    snapshot["operation"] = json!(operation);
    snapshot["command"] = json!(command);
    snapshot["update_check"] = json!(update_check);
    snapshot["profile_refreshing"] = json!(profiles.as_ref().map(|p| &p.refreshing));
    snapshot["profile_failed"] = json!(profiles.as_ref().map(|p| &p.failed));
    snapshot["connection_state"] = json!(match state.as_ref().and_then(|s| s.online) {
        None => "connecting",
        Some(true) if online => "online",
        _ => "offline",
    });
    snapshot["online"] = json!(online);
    if error.is_some() {
        snapshot["error"] = json!(error);
    }
    Ok(snapshot)
}

pub(crate) async fn refresh(
    client: Arc<StationClient>,
    store: Arc<ClientStore>,
    peer: &str,
) -> Result<Value> {
    let device = Device::open(client.clone(), Some((store.clone(), peer.to_owned())), true);
    let result:Result<()>=async {
        if device.refresh_replica_catalog().await? {return Ok(());}
        let get=|path:&str|client.node_request(reqwest::Method::GET,path.into(),None);
        let (info,agents,profiles,providers)=tokio::try_join!(get("/v1/node/info"),get("/v1/node/agents"),get("/v1/im/profiles"),get("/v1/node/providers"))?;
        // These are existing public APIs; reject missing collections so a failed
        // or incompatible response cannot replace prior data with an empty list.
        let agents=agents["items"].as_array().context("Agent catalog unavailable")?;
        let profiles=profiles["items"].as_array().context("Profile catalog unavailable")?;
        let providers=providers["providers"].as_array().context("Provider catalog unavailable")?;
        let snapshot=json!({"ready":true,"cached":true,"info":info,"agents":agents,"profiles":profiles,"providers":providers,"version":ulid::Ulid::new().to_string()});
        store.put(peer,"public-settings",&snapshot)?;
        Ok(())
    }.await;
    let mut snapshot = snapshot(&store, peer)?;
    match result {
        Ok(()) => {
            snapshot["cached"] = json!(false);
            snapshot["online"] = json!(true);
            snapshot["connection_state"] = json!("online");
        }
        Err(error) => {
            snapshot["online"] = json!(false);
            snapshot["connection_state"] = json!("offline");
            snapshot["error"] = json!(error.to_string());
        }
    }
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_snapshot_uses_shared_quota_and_keeps_unreported_distinct_from_failure() {
        let profile = present_profile(json!({"profile_id":"p","verified":true,
            "rateLimits":{"ok":true,"rateLimits":{"primary":{"usedPercent":28,"windowDurationMins":300},
            "credits":{"balance":0,"unit":"USD"}}}}));
        assert_eq!(profile["verified"], true);
        assert_eq!(profile["quota"]["windows"][0]["remaining"], 72.0);
        assert_eq!(profile["quota"]["windows"][0]["minutes"], 300);
        assert_eq!(profile["quota"]["balance"], json!([0.0, "USD"]));
        let absent = present_profile(json!({"rateLimits":{"ok":true,"reported":false}}));
        assert_eq!(absent["quota"]["failed"], false);
        assert_eq!(absent["quota"]["windows"], json!([]));
        let failed =
            present_profile(json!({"rateLimits":{"ok":false,"error":"private diagnostic"}}));
        assert_eq!(failed["quota"]["failed"], true);
        assert!(!failed["quota"].to_string().contains("private diagnostic"));
    }
}
