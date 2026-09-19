//! Versioned Mesh membership and its device-directory projection commit together.
use super::*;
use anyhow::{ensure, Context};
use zork_config::membership::MeshGroup;

impl ClientStore {
    pub(crate) fn directory_events(&self) -> zork_observe::ValueSubscription<()> {
        self.settings_events("\0directory")
    }
    pub(crate) fn directory_changed(&self) {
        self.settings_changed("\0directory");
    }

    pub(crate) fn current_mesh(&self) -> Result<Option<MeshGroup>> {
        let mut groups: Vec<_> = self
            .nodes()?
            .into_iter()
            .filter_map(|node| node.group)
            .collect();
        groups.sort();
        groups.dedup();
        ensure!(
            groups.len() <= 1,
            "存在多个已保存的 Mesh，请先选择要保留的连接"
        );
        let Some(authority) = groups.first() else {
            return Ok(None);
        };
        Ok(Some(
            self.get("device", &format!("mesh-membership:{authority}"))?
                .context("正在同步当前 Mesh，请稍后再试")?,
        ))
    }

    /// Accept a directory only from a saved, authenticated member of that Mesh.
    /// The caller serializes transport reconciliation with this transaction.
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
    use zork_config::membership::{MeshDevice, MeshVersion};
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
    #[test]
    fn switch_commit_requires_the_confirmed_revision_and_is_atomic() -> Result<()> {
        let root = tempfile::tempdir()?;
        let store = ClientStore::open(root.path())?;
        let (a, b, phone) = (device('y'), device('b'), device('n'));
        let old = MeshGroup {
            authority: a.origin.clone(),
            revision: 1,
            members: vec![a.clone()],
            clients: vec![phone.clone()],
        };
        let target = MeshGroup {
            authority: b.origin.clone(),
            revision: 1,
            members: vec![b.clone()],
            clients: vec![phone.clone()],
        };
        store.save_node(&saved(&a, &a.origin))?;
        store.apply_mesh_directory(&a.origin, &phone.origin, &old)?;
        store.put(&a.origin, "draft:chat", &"preserved")?;
        store.put(
            "device",
            "invitation",
            &serde_json::json!({"invitation":{"id":"next"},"switch_from":MeshVersion::of(&old)}),
        )?;
        let mut newer = old.clone();
        newer.revision += 1;
        newer.members[0].name = "renamed".into();
        store.apply_mesh_directory(&a.origin, &phone.origin, &newer)?;
        assert!(store
            .accept_invitation_if(
                "next",
                &[saved(&b, &b.origin)],
                &crate::Network::default(),
                &target
            )
            .is_err());
        assert_eq!(store.nodes()?[0].id, a.origin);
        assert!(store
            .get::<serde_json::Value>("device", "invitation")?
            .is_some());
        store.put(
            "device",
            "invitation",
            &serde_json::json!({"invitation":{"id":"next"},"switch_from":MeshVersion::of(&newer)}),
        )?;
        store.accept_invitation_if(
            "next",
            &[saved(&b, &b.origin)],
            &crate::Network::default(),
            &target,
        )?;
        assert_eq!(store.nodes()?[0].id, b.origin);
        assert_eq!(
            store.get::<String>(&a.origin, "draft:chat")?.as_deref(),
            Some("preserved")
        );
        assert!(store
            .get::<serde_json::Value>("device", "invitation")?
            .is_none());
        Ok(())
    }
}
