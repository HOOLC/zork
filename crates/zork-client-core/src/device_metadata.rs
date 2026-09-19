//! Apply authenticated device metadata to saved connection names, preserving identity.
use crate::store::ClientStore;
use serde_json::Value;
pub(crate) fn record(
    store: &ClientStore,
    peer: &str,
    path: &str,
    value: &Value,
) -> anyhow::Result<()> {
    if matches!(path, "/v1/node/info" | "/v1/node/name") {
        if let Some(name) = value["name"]
            .as_str()
            .filter(|name| !name.trim().is_empty())
        {
            if let Some(mut node) = store.nodes()?.into_iter().find(|n| n.id == peer) {
                if node.name != name {
                    node.name = name.into();
                    store.save_node(&node)?;
                }
            }
        }
    }
    let group = if path == "/v1/node/mesh" {
        &value["config"]["group"]
    } else if path == "/v1/node/name" {
        &value["group"]
    } else {
        return Ok(());
    };
    let Ok(group) = serde_json::from_value::<zork_config::membership::MeshGroup>(group.clone())
    else {
        return Ok(());
    };
    group.validate()?;
    if !group.members.iter().any(|m| m.origin == peer) {
        return Ok(());
    }
    let key = format!("device-names:{}", group.authority);
    if store
        .get::<u64>("device", &key)?
        .is_some_and(|revision| revision >= group.revision)
    {
        return Ok(());
    }
    for mut node in store.nodes()? {
        let origin = node
            .mesh
            .as_ref()
            .map(|m| m.origin.as_str())
            .unwrap_or(&node.id);
        if let Some(member) = group.members.iter().find(|m| m.origin == origin) {
            if node.name != member.name {
                node.name = member.name.clone();
                store.save_node(&node)?;
            }
        }
    }
    store.put("device", &key, &group.revision)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{RemoteNode, SavedNode};
    use serde_json::json;
    #[test]
    fn names_follow_authenticated_metadata_without_changing_connections() {
        let dir = tempfile::tempdir().unwrap();
        let store = ClientStore::open(dir.path()).unwrap();
        let origin = format!("key:{}", "y".repeat(52));
        store
            .save_node(&SavedNode {
                id: origin.clone(),
                name: "old".into(),
                url: "".into(),
                token: None,
                local: false,
                mesh: Some(RemoteNode {
                    routes: None,
                    origin: origin.clone(),
                    addr: Some("127.0.0.1:12".into()),
                }),
                group: Some(origin.clone()),
            })
            .unwrap();
        record(
            &store,
            &origin,
            "/v1/node/name",
            &json!({"name":"小熊工作室"}),
        )
        .unwrap();
        assert_eq!(store.nodes().unwrap()[0].name, "小熊工作室");
        assert_eq!(
            store.nodes().unwrap()[0]
                .mesh
                .as_ref()
                .unwrap()
                .addr
                .as_deref(),
            Some("127.0.0.1:12")
        );
        let group = json!({"authority":origin,"revision":2,"members":[{"origin":origin,"name":"新名字","addr":null}],"clients":[]});
        record(
            &store,
            &origin,
            "/v1/node/mesh",
            &json!({"config":{"group":group}}),
        )
        .unwrap();
        assert_eq!(store.nodes().unwrap()[0].name, "新名字");
        let mut stale = group.clone();
        stale["revision"] = json!(1);
        stale["members"][0]["name"] = json!("过期名字");
        record(
            &store,
            &origin,
            "/v1/node/mesh",
            &json!({"config":{"group":stale}}),
        )
        .unwrap();
        assert_eq!(store.nodes().unwrap()[0].name, "新名字");
    }
}
