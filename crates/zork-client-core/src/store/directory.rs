//! Versioned Mesh membership and its device-directory projection commit together.
use super::*;
use anyhow::{ensure, Context};
use zork_config::membership::{MeshGroup, MeshNames};

/// Cached Mesh groups and display-name directories, keyed by authority.
pub(super) struct Naming {
    groups: Vec<(MeshGroup, Option<MeshNames>)>,
}

pub(super) fn naming(conn: &Connection) -> Result<Naming> {
    let mut rows = conn.prepare(
        "SELECT key, value FROM cache WHERE node='device' AND (key LIKE 'naming-group:%' OR key LIKE 'naming-names:%')",
    )?;
    let rows = rows
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut groups = Vec::new();
    let mut names = std::collections::HashMap::new();
    for (key, value) in rows {
        if key.starts_with("naming-group:") {
            if let Ok(group) = serde_json::from_str::<MeshGroup>(&value) {
                groups.push(group);
            }
        } else if let Ok(directory) = serde_json::from_str::<MeshNames>(&value) {
            names.insert(directory.authority.clone(), directory);
        }
    }
    groups.sort_by(|a: &MeshGroup, b| b.revision.cmp(&a.revision));
    Ok(Naming {
        groups: groups
            .into_iter()
            .map(|group| {
                let names = names.remove(&group.authority);
                (group, names)
            })
            .collect(),
    })
}

/// The display and machine names of `origin`, preferring the Mesh the saved
/// device belongs to. `None` for a device outside any known Mesh.
pub(super) fn display_name(
    naming: &Naming,
    authority: Option<&str>,
    origin: &str,
) -> Option<zork_config::membership::ResolvedName> {
    let preferred = naming
        .groups
        .iter()
        .filter(|(group, _)| Some(group.authority.as_str()) == authority);
    let other = naming
        .groups
        .iter()
        .filter(|(group, _)| Some(group.authority.as_str()) != authority);
    preferred
        .chain(other)
        .filter(|(group, _)| {
            group.contains(origin) || group.clients.iter().any(|c| c.origin == origin)
        })
        .find_map(|(group, names)| {
            zork_config::membership::resolve(group, names.as_ref())
                .into_iter()
                .find(|device| device.origin == origin)
        })
}

impl ClientStore {
    /// Records the Mesh membership and the authority's display names that a
    /// Station reported. Each keeps its own version, so a late snapshot from
    /// another device never brings back an older name. Returns whether the
    /// visible names may have changed.
    pub fn apply_mesh_names(&self, group: &MeshGroup, names: Option<&MeshNames>) -> Result<bool> {
        if group.validate().is_err() {
            return Ok(false);
        }
        let names = names.filter(|n| n.authority == group.authority && n.validate().is_ok());
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        let read = |key: &str| -> Result<Option<String>> {
            Ok(tx
                .query_row(
                    "SELECT value FROM cache WHERE node='device' AND key=?1",
                    [key],
                    |row| row.get(0),
                )
                .optional()?)
        };
        let mut changed = false;
        let group_key = format!("naming-group:{}", group.authority);
        let stored: Option<MeshGroup> =
            read(&group_key)?.and_then(|v| serde_json::from_str(&v).ok());
        let group_update = stored.map_or(true, |stored| group.revision > stored.revision);
        let names_key = format!("naming-names:{}", group.authority);
        let stored: Option<MeshNames> =
            read(&names_key)?.and_then(|v| serde_json::from_str(&v).ok());
        let names_update = names.filter(|n| n.supersedes(stored.as_ref()));
        if group_update {
            tx.execute("INSERT INTO cache(node,key,value) VALUES ('device',?1,?2) ON CONFLICT(node,key) DO UPDATE SET value=excluded.value",params![group_key,serde_json::to_string(group)?])?;
            changed = true;
        }
        if let Some(names) = names_update {
            tx.execute("INSERT INTO cache(node,key,value) VALUES ('device',?1,?2) ON CONFLICT(node,key) DO UPDATE SET value=excluded.value",params![names_key,serde_json::to_string(names)?])?;
            changed = true;
        }
        tx.commit()?;
        drop(conn);
        if changed {
            self.directory_changed();
        }
        Ok(changed)
    }

    /// The saved device's identity and every device of its Mesh with their
    /// display names, for validating a rename before it is sent.
    pub fn mesh_names_of(
        &self,
        id: &str,
    ) -> Result<Option<(String, Vec<zork_config::membership::ResolvedName>)>> {
        let conn = self.0.lock().unwrap();
        let saved: Option<String> = conn
            .query_row("SELECT value FROM nodes WHERE id=?1", [id], |row| {
                row.get(0)
            })
            .optional()?;
        let Some(saved) = saved else {
            return Ok(None);
        };
        let node: SavedNode = serde_json::from_str(&saved)?;
        let Some(origin) = super::origin_of(&conn, &node)? else {
            return Ok(None);
        };
        let naming = naming(&conn)?;
        let group = naming
            .groups
            .iter()
            .filter(|(group, _)| {
                group.contains(&origin) || group.clients.iter().any(|c| c.origin == origin)
            })
            .max_by_key(|(group, _)| Some(group.authority.as_str()) == node.group.as_deref());
        Ok(group.map(|(group, names)| {
            (
                origin.clone(),
                zork_config::membership::resolve(group, names.as_ref()),
            )
        }))
    }

    /// Keeps a display-name directory returned by a rename, if it is newer.
    pub fn apply_display_names(&self, names: &MeshNames) -> Result<bool> {
        if names.validate().is_err() {
            return Ok(false);
        }
        let key = format!("naming-names:{}", names.authority);
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        let stored: Option<MeshNames> = tx
            .query_row(
                "SELECT value FROM cache WHERE node='device' AND key=?1",
                [&key],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .and_then(|v| serde_json::from_str(&v).ok());
        if !names.supersedes(stored.as_ref()) {
            return Ok(false);
        }
        tx.execute("INSERT INTO cache(node,key,value) VALUES ('device',?1,?2) ON CONFLICT(node,key) DO UPDATE SET value=excluded.value",params![key,serde_json::to_string(names)?])?;
        tx.commit()?;
        drop(conn);
        self.directory_changed();
        Ok(true)
    }

    pub(crate) fn directory_events(&self) -> zork_observe::ValueSubscription<()> {
        self.settings_events("\0directory")
    }
    pub(crate) fn directory_changed(&self) {
        self.settings_changed("\0directory");
    }

    pub(crate) fn apply_mesh_directory(
        &self,
        anchor: &str,
        identity: &str,
        group: &MeshGroup,
    ) -> Result<bool> {
        group.validate()?;
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        let saved: Option<String> = tx
            .query_row("SELECT value FROM nodes WHERE id=?1", [anchor], |row| {
                row.get(0)
            })
            .optional()?;
        let saved: SavedNode = serde_json::from_str(&saved.context("directory_anchor_removed")?)?;
        ensure!(
            saved.group.as_deref() == Some(&group.authority)
                && saved
                    .mesh
                    .as_ref()
                    .is_some_and(|peer| group.contains(&peer.origin)),
            "directory_authority_mismatch"
        );
        let key = format!("mesh-membership:{}", group.authority);
        let previous: Option<String> = tx
            .query_row(
                "SELECT value FROM cache WHERE node='device' AND key=?1",
                [&key],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(previous) = previous {
            let previous: MeshGroup = serde_json::from_str(&previous)?;
            if group.revision < previous.revision {
                return Ok(false);
            }
            if group.revision == previous.revision {
                ensure!(group == &previous, "mesh_revision_conflict");
                return Ok(false);
            }
        }
        let allowed = group.clients.iter().any(|client| client.origin == identity);
        let old = tx
            .prepare("SELECT value FROM nodes")?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let old = old
            .into_iter()
            .map(|value| serde_json::from_str::<SavedNode>(&value))
            .collect::<serde_json::Result<Vec<_>>>()?;
        for node in &old {
            if node.group.as_deref() == Some(&group.authority)
                && (!allowed || !group.contains(&node.id))
            {
                tx.execute("DELETE FROM nodes WHERE id=?1", [&node.id])?;
            }
        }
        if allowed {
            for member in &group.members {
                let node = SavedNode {
                    machine_name: None,
                    id: member.origin.clone(),
                    name: member.name.clone(),
                    url: String::new(),
                    token: None,
                    local: false,
                    mesh: Some(RemoteNode {
                        routes: member.routes.clone(),
                        origin: member.origin.clone(),
                        addr: member.addr.clone(),
                    }),
                    group: Some(group.authority.clone()),
                };
                tx.execute("INSERT INTO nodes(id,value) VALUES (?1,?2) ON CONFLICT(id) DO UPDATE SET value=excluded.value",params![node.id,serde_json::to_string(&node)?])?;
            }
        }
        let count: usize = tx.query_row("SELECT COUNT(*) FROM nodes", [], |row| row.get(0))?;
        ensure!(count <= 16, "最多添加 16 台设备");
        tx.execute("INSERT INTO cache(node,key,value) VALUES ('device',?1,?2) ON CONFLICT(node,key) DO UPDATE SET value=excluded.value",params![key,serde_json::to_string(group)?])?;
        tx.commit()?;
        drop(conn);
        self.directory_changed();
        self.notifications_changed();
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zork_config::membership::MeshDevice;
    fn device(key: char) -> MeshDevice {
        MeshDevice {
            routes: None,
            origin: format!("key:{}", key.to_string().repeat(52)),
            name: key.to_string(),
            addr: None,
        }
    }
    fn saved(member: &MeshDevice, authority: &str) -> SavedNode {
        SavedNode {
            machine_name: None,
            id: member.origin.clone(),
            name: member.name.clone(),
            url: String::new(),
            token: None,
            local: false,
            mesh: Some(RemoteNode {
                routes: member.routes.clone(),
                origin: member.origin.clone(),
                addr: None,
            }),
            group: Some(authority.into()),
        }
    }
    #[test]
    fn account_logout_preserves_manual_peers_and_cached_history() -> Result<()> {
        let root = tempfile::tempdir()?;
        let store = ClientStore::open(root.path())?;
        let mut manual = saved(&device('y'), "manual");
        manual.group = None;
        let mut account = saved(&device('b'), "account");
        account.group = None;
        store.save_node(&manual)?;
        store.replace_account_nodes(&[manual.clone(), account.clone()])?;
        store.put(&account.id, "draft:chat", &"preserved")?;
        drop(store);
        let store = ClientStore::open(root.path())?;
        let removed = store.replace_account_nodes(&[])?;
        assert_eq!(removed, vec![account.id.clone()]);
        assert_eq!(store.nodes()?, vec![manual]);
        assert_eq!(
            store.get::<String>(&account.id, "draft:chat")?.as_deref(),
            Some("preserved")
        );
        assert!(store
            .get::<Vec<String>>("device", "account_peers")?
            .unwrap()
            .is_empty());
        Ok(())
    }

    #[test]
    fn directory_revision_survives_reopen_and_removal_preserves_history() -> Result<()> {
        let root = tempfile::tempdir()?;
        let store = ClientStore::open(root.path())?;
        let (a, b, c, phone) = (device('y'), device('b'), device('n'), device('d'));
        let mut group = MeshGroup {
            authority: a.origin.clone(),
            revision: 1,
            members: vec![a.clone(), b.clone(), c.clone()],
            clients: vec![phone.clone()],
        };
        store.save_node(&saved(&a, &a.origin))?;
        store.put(&c.origin, "draft:chat", &"preserved")?;
        assert!(store.apply_mesh_directory(&a.origin, &phone.origin, &group)?);
        assert_eq!(store.nodes()?.len(), 3);
        let old = group.clone();
        group.members.pop();
        group.revision += 1;
        assert!(store.apply_mesh_directory(&b.origin, &phone.origin, &group)?);
        assert_eq!(store.nodes()?.len(), 2);
        drop(store);
        let store = ClientStore::open(root.path())?;
        assert!(!store.apply_mesh_directory(&a.origin, &phone.origin, &old)?);
        assert_eq!(store.nodes()?.len(), 2);
        assert_eq!(
            store.get::<String>(&c.origin, "draft:chat")?.as_deref(),
            Some("preserved")
        );
        let mut conflict = group.clone();
        conflict.members[0].name = "forged".into();
        assert!(store
            .apply_mesh_directory(&a.origin, &phone.origin, &conflict)
            .is_err());
        group.clients.clear();
        group.revision += 1;
        assert!(store.apply_mesh_directory(&a.origin, &phone.origin, &group)?);
        assert!(store.nodes()?.is_empty());
        assert!(store
            .apply_mesh_directory(&a.origin, &phone.origin, &old)
            .is_err());
        Ok(())
    }
}
