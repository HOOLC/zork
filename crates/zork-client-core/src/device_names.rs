//! Mesh-wide display names. The Mesh authority owns them; this client checks
//! a new name against the names it knows, sends the rename through the
//! device's Station and keeps the authority's result. Surfaces only show the
//! names `ClientStore::nodes()` resolves.
use crate::{api::StationClient, store::ClientStore};
use anyhow::{Context, Result};
use serde_json::json;

/// Checks a new display name for the saved device `id`: empty, too long, or
/// already used by another device of the same Mesh. The authority checks
/// again, so a name taken meanwhile is still rejected.
pub fn validate(store: &ClientStore, id: &str, name: &str) -> Result<String> {
    let others = match store.mesh_names_of(id)? {
        Some((origin, devices)) => devices
            .into_iter()
            .filter(|device| device.origin != origin)
            .map(|device| device.display)
            .collect(),
        None => vec![],
    };
    let others: Vec<&str> = others.iter().map(String::as_str).collect();
    crate::device_edit::validate_display_name(name, &others).map_err(anyhow::Error::msg)
}

/// Words for rename failures reported by a Station or the Mesh authority.
fn explain(error: crate::api::ApiError) -> anyhow::Error {
    let message = match &error {
        crate::api::ApiError::Api { message, .. } => message.clone(),
        _ => return error.into(),
    };
    anyhow::anyhow!(match message.as_str() {
        "mesh_not_ready" => "Mesh 尚未就绪，请稍后重试".to_owned(),
        "mesh_membership_missing" => "这台设备尚未加入 Mesh".to_owned(),
        "mesh_member_required" | "mesh_device_not_found" => {
            "这台设备已不在 Mesh 中，无法修改名称".to_owned()
        }
        "mesh_peer_unavailable" | "mesh_request_failed" => {
            "暂时连不上 Mesh 管理设备，请稍后重试".to_owned()
        }
        _ => message,
    })
}

/// Renames the saved device `id` for the whole Mesh through its Station.
/// A Station too old to know display names renames its own machine name,
/// which is also what every client shows for it.
pub async fn rename(
    store: &ClientStore,
    client: &StationClient,
    id: &str,
    name: &str,
) -> Result<serde_json::Value> {
    let name = validate(store, id, name)?;
    let result = client
        .node_request(
            http::Method::PUT,
            "/v1/node/mesh/names".into(),
            Some(json!({"name":name})),
        )
        .await;
    let value = match result {
        Err(error) if matches!(error.status(), Some(404 | 405)) => {
            let name = zork_config::membership::validate_device_name(&name)?;
            let value = client
                .node_request(
                    http::Method::PUT,
                    "/v1/node/name".into(),
                    Some(json!({"name":name})),
                )
                .await
                .map_err(explain)?;
            crate::device_metadata::record(store, id, "/v1/node/name", &value)?;
            return Ok(value);
        }
        result => result.map_err(explain)?,
    };
    if value["names"].is_object() {
        let names: zork_config::membership::MeshNames =
            serde_json::from_value(value["names"].clone()).context("invalid display names")?;
        store.apply_display_names(&names)?;
    } else {
        // Outside a Mesh the device has only its own name.
        crate::device_metadata::record(store, id, "/v1/node/name", &value)?;
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{RemoteNode, SavedNode};
    use zork_config::membership::{MeshDevice, MeshGroup, MeshNames};

    fn key(c: char) -> String {
        format!("key:{}", c.to_string().repeat(52))
    }
    fn device(c: char, name: &str) -> MeshDevice {
        MeshDevice {
            origin: key(c),
            name: name.into(),
            addr: None,
            routes: None,
        }
    }
    fn saved(c: char, name: &str, authority: &str) -> SavedNode {
        SavedNode {
            id: key(c),
            name: name.into(),
            url: String::new(),
            token: None,
            local: false,
            mesh: Some(RemoteNode {
                origin: key(c),
                addr: None,
                routes: None,
            }),
            group: Some(authority.into()),
            machine_name: None,
            color_key: None,
        }
    }
    fn mesh() -> MeshGroup {
        MeshGroup {
            authority: key('y'),
            revision: 3,
            members: vec![
                device('y', "MacBook-Air"),
                device('b', "zuozijians-Mac-Studio"),
            ],
            clients: vec![
                device('n', "zuozijiandeMacBook-Air 客户端"),
                device('d', "PLP110"),
            ],
        }
    }

    #[test]
    fn listed_devices_show_display_names_and_keep_machine_names() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = ClientStore::open(dir.path())?;
        let group = mesh();
        store.save_node(&saved('y', "MacBook-Air", &group.authority))?;
        // A Station reached by URL is matched through the identity it reported.
        store.save_node(&SavedNode {
            mesh: None,
            group: None,
            id: "local".into(),
            name: "本机".into(),
            ..saved('x', "", "")
        })?;
        store.put("local", "mesh-origin", &key('b'))?;
        // An older Station sends no names: this client derives the same letters.
        assert!(store.apply_mesh_names(&group, None)?);
        let names = |store: &ClientStore| -> Result<Vec<(String, String, Option<String>)>> {
            Ok(store
                .nodes()?
                .into_iter()
                .map(|n| (n.id, n.name, n.machine_name))
                .collect())
        };
        assert_eq!(
            names(&store)?,
            vec![
                (key('y'), "A".into(), Some("MacBook-Air".into())),
                (
                    "local".into(),
                    "B".into(),
                    Some("zuozijians-Mac-Studio".into())
                ),
            ]
        );
        let listed = store.nodes()?.remove(0);
        assert_eq!(listed.device_name().accessible(), "A（MacBook-Air）");

        // The authority's directory replaces the derived letters; an older
        // revision arriving late does not bring a previous name back.
        let mut authority = MeshNames::new(&group.authority);
        authority.reconcile(&group);
        authority.rename(&group, &key('b'), "Studio")?;
        let renamed = authority.clone();
        authority.rename(&group, &key('b'), "工作室")?;
        assert!(store.apply_mesh_names(&group, Some(&authority))?);
        assert!(!store.apply_display_names(&renamed)?);
        assert_eq!(store.nodes()?[1].name, "工作室");
        // Colour follows join order, not the name: renaming keeps it.
        let colors: Vec<_> = store.nodes()?.into_iter().map(|n| n.color_key).collect();
        assert_eq!(colors, [Some("seq:0".to_owned()), Some("seq:1".to_owned())]);
        // A stale membership snapshot never replaces a newer one.
        let mut stale = group.clone();
        stale.revision = 2;
        stale.members.pop();
        assert!(!store.apply_mesh_names(&stale, Some(&renamed))?);
        assert_eq!(store.nodes()?[1].name, "工作室");
        Ok(())
    }

    #[test]
    fn rename_is_checked_against_the_mesh_before_sending() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = ClientStore::open(dir.path())?;
        let group = mesh();
        store.save_node(&saved('b', "zuozijians-Mac-Studio", &group.authority))?;
        store.apply_mesh_names(&group, None)?;
        let studio = key('b');
        assert_eq!(validate(&store, &studio, "  Studio ")?, "Studio");
        // Keeping its own letter is allowed; another device's is not.
        assert_eq!(validate(&store, &studio, "b")?, "b");
        let taken = validate(&store, &studio, "c").unwrap_err().to_string();
        assert!(taken.contains("已被 Mesh 中的另一台设备使用"), "{taken}");
        assert_eq!(
            validate(&store, &studio, " ").unwrap_err().to_string(),
            "请输入显示名称"
        );
        assert!(validate(&store, &studio, &"x".repeat(25)).is_err());
        // A device outside any Mesh only gets the basic checks.
        assert_eq!(validate(&store, "unknown", "A")?, "A");
        Ok(())
    }
}
