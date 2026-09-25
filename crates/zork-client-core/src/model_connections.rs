//! Model connections across every saved device, for clients that manage them
//! globally. Each device carries its own load state: a device that could not
//! be read keeps its cached connections and reports why, never "none".
use crate::{settings, store::ClientStore};
use anyhow::Result;
use serde_json::{json, Value};

pub(crate) const LOADED_AT: &str = "settings-loaded-at";

/// One device's projection. `refresh_error` is the failure of the read that
/// produced `snapshot`, if any; a cache-only read passes `None`.
pub(crate) fn device(
    peer: &str,
    name: &str,
    snapshot: &Value,
    refresh_error: Option<&str>,
    loaded_at_ms: Option<u64>,
) -> Value {
    let text = |value: &Value| value.as_str().filter(|s| !s.is_empty()).map(str::to_owned);
    if snapshot["revoked"] == true {
        return json!({"peer":peer,"name":name,"state":"revoked","error":"设备访问权限已撤销",
            "cached":false,"loaded_at_ms":null,"profiles":[],"providers":[]});
    }
    let ready = snapshot["ready"] == true;
    let error = refresh_error
        .map(str::to_owned)
        .or_else(|| text(&snapshot["profiles_error"]));
    let state = match (&error, ready) {
        (Some(_), _) => "failed",
        (None, false) => "loading",
        (None, true) if snapshot["profiles_ready"] == false => "loading",
        (None, true) => "ready",
    };
    let list = |key: &str| match &snapshot[key] {
        Value::Array(items) if ready => Value::Array(items.clone()),
        _ => json!([]),
    };
    json!({"peer":peer,"name":name,"state":state,"error":error,
        "cached":ready && error.is_some(),
        "loaded_at_ms":loaded_at_ms,
        "profiles":list("profiles"),
        "providers":list("providers")})
}

fn loaded_at(store: &ClientStore, peer: &str) -> Result<Option<u64>> {
    store.get(peer, LOADED_AT)
}

/// Every saved device's committed connections, without network I/O.
pub(crate) fn cached(store: &ClientStore) -> Result<Value> {
    let mut devices = vec![];
    for node in store.nodes()? {
        let snapshot = settings::cached(store, &node.id)?;
        devices.push(device(
            &node.id,
            &node.name,
            &snapshot,
            None,
            loaded_at(store, &node.id)?,
        ));
    }
    Ok(json!({"devices":devices}))
}

/// Reads every device concurrently. One device's failure only marks that
/// device; the others still complete.
pub(crate) async fn refresh(
    store: std::sync::Arc<ClientStore>,
    station: impl Fn(&str) -> Result<std::sync::Arc<crate::api::StationClient>>,
) -> Result<Value> {
    let nodes = store.nodes()?;
    let reads = nodes.iter().map(|node| {
        let client = station(&node.id);
        let store = store.clone();
        async move {
            let refreshed = match client {
                Ok(client) => settings::refresh(client, store.clone(), &node.id).await,
                Err(error) => Err(error),
            };
            let (snapshot, error) = match refreshed {
                // A refresh reports its own failure while keeping the cache.
                Ok(snapshot) if snapshot["cached"] == false => (snapshot, None),
                Ok(snapshot) => {
                    let error = snapshot["error"]
                        .as_str()
                        .unwrap_or("设备暂时无法连接")
                        .to_owned();
                    (snapshot, Some(error))
                }
                Err(error) => (settings::cached(&store, &node.id)?, Some(error.to_string())),
            };
            Ok::<_, anyhow::Error>(device(
                &node.id,
                &node.name,
                &snapshot,
                error.as_deref(),
                loaded_at(&store, &node.id)?,
            ))
        }
    });
    let devices = futures_util::future::join_all(reads)
        .await
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    Ok(json!({"devices":devices}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SavedNode;

    fn node(id: &str, name: &str) -> SavedNode {
        SavedNode {
            machine_name: None,
            color_key: None,
            id: id.into(),
            name: name.into(),
            url: String::new(),
            token: None,
            local: false,
            mesh: None,
            group: None,
        }
    }

    #[test]
    fn failed_read_keeps_cached_connections_and_reports_the_reason() {
        let snapshot = json!({"ready":true,"profiles":[{"profile_id":"p"}],"providers":[],"profiles_ready":true});
        let entry = device("a", "Studio", &snapshot, Some("连接超时"), Some(42));
        assert_eq!(entry["state"], "failed");
        assert_eq!(entry["error"], "连接超时");
        assert_eq!(entry["cached"], true);
        assert_eq!(entry["loaded_at_ms"], 42);
        assert_eq!(entry["profiles"][0]["profile_id"], "p");
    }

    #[test]
    fn never_loaded_device_is_loading_not_empty_and_failure_without_cache_is_not_cached() {
        let loading = device("a", "A", &json!({"ready":false}), None, None);
        assert_eq!(loading["state"], "loading");
        let failed = device("a", "A", &json!({"ready":false}), Some("离线"), None);
        assert_eq!(failed["state"], "failed");
        assert_eq!(failed["cached"], false);
        assert_eq!(failed["profiles"], json!([]));
    }

    #[test]
    fn profile_catalog_state_and_revocation_are_distinct() {
        let pending = json!({"ready":true,"profiles":[],"providers":[],"profiles_ready":false});
        assert_eq!(device("a", "A", &pending, None, None)["state"], "loading");
        let broken = json!({"ready":true,"profiles":[],"providers":[],"profiles_ready":true,"profiles_error":"boom"});
        assert_eq!(device("a", "A", &broken, None, None)["state"], "failed");
        let ready = json!({"ready":true,"profiles":[],"providers":[],"profiles_ready":true,"profiles_error":null});
        assert_eq!(device("a", "A", &ready, None, None)["state"], "ready");
        let revoked = device(
            "a",
            "A",
            &json!({"ready":false,"revoked":true}),
            None,
            Some(1),
        );
        assert_eq!(revoked["state"], "revoked");
        assert_eq!(revoked["profiles"], json!([]));
    }

    #[test]
    fn cached_lists_every_saved_device_with_its_own_state() {
        let root = tempfile::tempdir().unwrap();
        let store = ClientStore::open(root.path()).unwrap();
        store.save_node(&node("a", "Studio")).unwrap();
        store.save_node(&node("b", "mini1")).unwrap();
        store
            .put("a", "public-settings", &json!({"ready":true,"cached":true,"info":{},"agents":[],
                "profiles":[{"profile_id":"p","provider":"anthropic","verified":true}],"providers":[]}))
            .unwrap();
        store.put("a", LOADED_AT, &7u64).unwrap();
        let result = cached(&store).unwrap();
        let devices = result["devices"].as_array().unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0]["name"], "Studio");
        assert_eq!(devices[0]["state"], "ready");
        assert_eq!(devices[0]["loaded_at_ms"], 7);
        assert_eq!(devices[0]["profiles"][0]["verified"], true);
        assert!(devices[0]["profiles"][0]["quota"].is_object());
        assert_eq!(devices[1]["state"], "loading");
        assert_eq!(devices[1]["profiles"], json!([]));
    }
}
