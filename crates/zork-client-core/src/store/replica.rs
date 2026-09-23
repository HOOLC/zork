//! Crash-safe scope replicas. Incoming pages are invisible staging data until
//! entities and their checkpoint can be committed in one transaction.
use super::ClientStore;
use anyhow::{ensure, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use zork_client_types::sync::{Cursor, Kind, Page, Record, Scope};

pub(super) fn initialize(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS replica_bindings(peer TEXT PRIMARY KEY,generation INTEGER NOT NULL DEFAULT 0,revoked INTEGER NOT NULL DEFAULT 0);
         CREATE TABLE IF NOT EXISTS replica_scopes(peer TEXT NOT NULL,scope TEXT NOT NULL,cursor TEXT NOT NULL,last_batch TEXT NOT NULL,PRIMARY KEY(peer,scope));
         CREATE TABLE IF NOT EXISTS replica_entities(peer TEXT NOT NULL,scope TEXT NOT NULL,kind TEXT NOT NULL,id TEXT NOT NULL,revision INTEGER NOT NULL,value TEXT,PRIMARY KEY(peer,scope,kind,id));
         CREATE INDEX IF NOT EXISTS replica_entity_revision ON replica_entities(peer,scope,revision);
         CREATE TABLE IF NOT EXISTS replica_batches(peer TEXT NOT NULL,scope TEXT NOT NULL,header TEXT NOT NULL,next_index INTEGER NOT NULL,PRIMARY KEY(peer,scope));
         CREATE TABLE IF NOT EXISTS replica_staging(peer TEXT NOT NULL,scope TEXT NOT NULL,kind TEXT NOT NULL,id TEXT NOT NULL,revision INTEGER NOT NULL,value TEXT,PRIMARY KEY(peer,scope,kind,id));
         CREATE TABLE IF NOT EXISTS replica_pages(peer TEXT NOT NULL,scope TEXT NOT NULL,page_index INTEGER NOT NULL,value TEXT NOT NULL,PRIMARY KEY(peer,scope,page_index));",
    )?;
    Ok(())
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ReplicaState {
    pub cursor: Option<Cursor>,
    pub staging: bool,
}
impl ReplicaState {
    pub fn ready(&self) -> bool {
        self.cursor.is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplicaApply {
    Staged,
    AlreadyApplied,
    Published { cursor: Cursor, changed: usize },
}

fn checkpoint(conn: &Connection, peer: &str, scope: &str) -> Result<Option<(Cursor, String)>> {
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT cursor,last_batch FROM replica_scopes WHERE peer=?1 AND scope=?2",
            params![peer, scope],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    row.map(|(cursor, batch)| Ok((serde_json::from_str(&cursor)?, batch)))
        .transpose()
}
fn clear_staging(tx: &Transaction<'_>, peer: &str, scope: &str) -> Result<()> {
    for table in ["replica_batches", "replica_staging", "replica_pages"] {
        tx.execute(
            &format!("DELETE FROM {table} WHERE peer=?1 AND scope=?2"),
            params![peer, scope],
        )?;
    }
    Ok(())
}
fn payload(value: Option<String>) -> Result<Option<serde_json::Value>> {
    value
        .map(|value| Ok(serde_json::from_str(&value)?))
        .transpose()
}
fn upsert(
    conn: &Connection,
    table: &str,
    peer: &str,
    scope: &str,
    record: &Record,
) -> Result<bool> {
    debug_assert!(matches!(table, "replica_entities" | "replica_staging"));
    let old: Option<(u64,Option<String>)> = conn.query_row(
        &format!("SELECT revision,value FROM {table} WHERE peer=?1 AND scope=?2 AND kind=?3 AND id=?4"),
        params![peer,scope,record.kind.key(),record.id],|r|Ok((r.get(0)?,r.get(1)?)),
    ).optional()?;
    if let Some((revision, value)) = old {
        if revision > record.revision {
            return Ok(false);
        }
        if revision == record.revision {
            ensure!(
                payload(value)? == record.value,
                "sync_conflicting_entity_revision"
            );
            return Ok(false);
        }
    }
    let value = record
        .value
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    conn.execute(&format!("INSERT INTO {table}(peer,scope,kind,id,revision,value) VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(peer,scope,kind,id) DO UPDATE SET revision=excluded.revision,value=excluded.value"),
        params![peer,scope,record.kind.key(),record.id,record.revision,value])?;
    Ok(true)
}
fn record_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, String, u64, Option<String>)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
}
fn decode(row: (String, String, u64, Option<String>)) -> Result<Record> {
    Ok(Record {
        kind: serde_json::from_value(serde_json::Value::String(row.0))?,
        id: row.1,
        revision: row.2,
        value: payload(row.3)?,
    })
}

pub(super) fn apply_receipt(
    conn: &Connection,
    peer: &str,
    receipt: &zork_client_types::sync::Receipt,
) -> Result<()> {
    let scope = Scope::Catalog {}.key();
    let Some((cursor, _)) = checkpoint(conn, peer, &scope)? else {
        return Ok(());
    };
    ensure!(cursor.owner == receipt.owner, "sync_owner_mismatch");
    if cursor.epoch != receipt.epoch {
        return Ok(());
    }
    if let Some(record) = &receipt.entity {
        let validation = Cursor {
            sequence: record.revision,
            ..cursor
        };
        record.validate(&validation).map_err(anyhow::Error::msg)?;
        upsert(conn, "replica_entities", peer, &scope, record)?;
    }
    Ok(())
}

pub(super) fn binding_generation(conn: &Connection, peer: &str) -> Result<u64> {
    Ok(conn
        .query_row(
            "SELECT generation FROM replica_bindings WHERE peer=?1",
            [peer],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0))
}

// The caller holds a transaction that fences attachment access against revocation.
fn attachment_binding(conn: &Connection, peer: &str, generation: u64) -> Result<()> {
    let binding: Option<(u64, bool)> = conn
        .query_row(
            "SELECT generation,revoked FROM replica_bindings WHERE peer=?1",
            [peer],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    ensure!(
        binding.unwrap_or((0, false)) == (generation, false),
        "sync_authorization_changed"
    );
    Ok(())
}

impl ClientStore {
    pub(crate) fn attachment_blob_at(
        &self,
        peer: &str,
        key: &str,
        generation: u64,
    ) -> Result<Option<Vec<u8>>> {
        let mut conn = self.0.lock().expect("client database");
        let tx = conn.transaction()?;
        attachment_binding(&tx, peer, generation)?;
        let bytes = tx
            .query_row(
                "SELECT value FROM blobs WHERE node=?1 AND key=?2",
                params![peer, key],
                |row| row.get(0),
            )
            .optional()?;
        tx.commit()?;
        Ok(bytes)
    }
    pub(crate) fn put_attachment_blob_at(
        &self,
        peer: &str,
        key: &str,
        bytes: &[u8],
        generation: u64,
    ) -> Result<()> {
        let mut conn = self.0.lock().expect("client database");
        let tx = conn.transaction()?;
        attachment_binding(&tx, peer, generation)?;
        tx.execute("INSERT INTO blobs(node,key,value) VALUES (?1,?2,?3) ON CONFLICT(node,key) DO UPDATE SET value=excluded.value",params![peer,key,bytes])?;
        tx.commit()?;
        Ok(())
    }
    pub fn replica_generation(&self, peer: &str) -> Result<u64> {
        binding_generation(&self.0.lock().expect("client database"), peer)
    }
    pub fn replica_revoked(&self, peer: &str) -> Result<bool> {
        Ok(self
            .0
            .lock()
            .expect("client database")
            .query_row(
                "SELECT revoked FROM replica_bindings WHERE peer=?1",
                [peer],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }
    pub fn revoke_replica(&self, peer: &str) -> Result<()> {
        let mut conn = self.0.lock().expect("client database");
        let tx = conn.transaction()?;
        tx.execute("INSERT INTO replica_bindings(peer,generation,revoked) VALUES(?1,1,1) ON CONFLICT(peer) DO UPDATE SET generation=generation+1,revoked=1",[peer])?;
        for table in [
            "messages",
            "message_history",
            "pending_business_card_results",
            "agent_configuration_outbox",
            "replica_scopes",
            "replica_entities",
            "replica_batches",
            "replica_staging",
            "replica_pages",
        ] {
            let owner_column = if matches!(
                table,
                "messages"
                    | "message_history"
                    | "pending_business_card_results"
                    | "agent_configuration_outbox"
            ) {
                "node"
            } else {
                "peer"
            };
            let confirmed = if table == "messages" {
                " AND status='sent'"
            } else {
                ""
            };
            tx.execute(
                &format!("DELETE FROM {table} WHERE {owner_column}=?1{confirmed}"),
                [peer],
            )?;
        }
        tx.execute("DELETE FROM cache WHERE node=?1 AND (key IN ('agents','sessions','inbox','drive','read-markers','public-settings','profile-authorization','mesh-admin-invitation','settings-command','node-update-check','node-operation','notification-ledger-v1') OR key LIKE 'http:%' OR key LIKE 'messages:%' OR key LIKE 'leader-tasks:%' OR key LIKE 'history:%')",[peer])?;
        let uploads = super::message_delivery::upload_keys(&tx, peer)?;
        let blobs = tx
            .prepare("SELECT key FROM blobs WHERE node=?1")?
            .query_map([peer], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for key in blobs {
            if !uploads.contains(&key) {
                tx.execute(
                    "DELETE FROM blobs WHERE node=?1 AND key=?2",
                    params![peer, key],
                )?;
            }
        }
        tx.commit()?;
        drop(conn);
        self.settings_changed(peer);
        self.notifications_changed();
        Ok(())
    }
    pub fn replica_state(&self, peer: &str, scope: &Scope) -> Result<ReplicaState> {
        let conn = self.0.lock().expect("client database");
        let scope = scope.key();
        Ok(ReplicaState {
            cursor: checkpoint(&conn, peer, &scope)?.map(|v| v.0),
            staging: conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM replica_batches WHERE peer=?1 AND scope=?2)",
                params![peer, scope],
                |r| r.get(0),
            )?,
        })
    }

    /// Resume a staged export after a process restart. Committed UI state stays
    /// visible while the remaining pages arrive.
    pub fn replica_pull(&self, peer: &str, scope: &Scope) -> Result<zork_client_types::sync::Pull> {
        let conn = self.0.lock().expect("client database");
        let staged: Option<(String, u32)> = conn
            .query_row(
                "SELECT header,next_index FROM replica_batches WHERE peer=?1 AND scope=?2",
                params![peer, scope.key()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (after, continuation) = if let Some((header, index)) = staged {
            let header: Page = serde_json::from_str(&header)?;
            (
                header.from,
                Some(zork_client_types::sync::Continuation {
                    batch_id: header.batch_id,
                    index,
                }),
            )
        } else {
            (checkpoint(&conn, peer, &scope.key())?.map(|v| v.0), None)
        };
        Ok(zork_client_types::sync::Pull {
            scope: scope.clone(),
            after,
            continuation,
        })
    }

    pub fn discard_replica_staging_at(
        &self,
        peer: &str,
        scope: &Scope,
        generation: u64,
    ) -> Result<()> {
        let mut conn = self.0.lock().expect("client database");
        let tx = conn.transaction()?;
        ensure!(
            binding_generation(&tx, peer)? == generation,
            "sync_authorization_changed"
        );
        clear_staging(&tx, peer, &scope.key())?;
        tx.commit()?;
        Ok(())
    }

    /// The caller pins `owner` from its authenticated peer binding, not from
    /// an untrusted response. A changed owner requires explicit re-binding.
    pub fn apply_replica_page(&self, peer: &str, owner: &str, page: &Page) -> Result<ReplicaApply> {
        self.apply_replica_page_at(peer, owner, self.replica_generation(peer)?, page)
    }
    pub fn apply_replica_page_at(
        &self,
        peer: &str,
        owner: &str,
        generation: u64,
        page: &Page,
    ) -> Result<ReplicaApply> {
        zork_client_types::sync::identifier(peer).map_err(anyhow::Error::msg)?;
        page.validate().map_err(anyhow::Error::msg)?;
        ensure!(owner == page.through.owner, "sync_owner_mismatch");
        let scope = page.through.scope.key();
        let mut conn = self.0.lock().expect("client database");
        let tx = conn.transaction()?;
        ensure!(
            binding_generation(&tx, peer)? == generation,
            "sync_authorization_changed"
        );
        let previous = checkpoint(&tx, peer, &scope)?;
        if let Some((cursor, batch)) = &previous {
            ensure!(cursor.owner == owner, "sync_owner_changed");
            if batch == &page.batch_id && cursor == &page.through {
                return Ok(ReplicaApply::AlreadyApplied);
            }
        }
        match &page.from {
            Some(from) => ensure!(
                previous.as_ref().is_some_and(|(old, _)| old == from),
                "sync_checkpoint_mismatch"
            ),
            None => {
                if let Some((old, _)) = &previous {
                    ensure!(
                        old.epoch != page.through.epoch || page.through.sequence >= old.sequence,
                        "sync_stale_snapshot"
                    );
                }
            }
        }
        let staged: Option<(String, u32)> = tx
            .query_row(
                "SELECT header,next_index FROM replica_batches WHERE peer=?1 AND scope=?2",
                params![peer, scope],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let mut next_index = 0;
        let mut start = true;
        if let Some((header, index)) = staged {
            let header: Page = serde_json::from_str(&header)?;
            if header.batch_id == page.batch_id {
                ensure!(header.same_batch(page), "sync_batch_changed");
                next_index = index;
                start = false;
            } else {
                ensure!(page.index == 0, "sync_unknown_batch");
            }
        }
        if start {
            ensure!(page.index == 0, "sync_missing_first_page");
            clear_staging(&tx, peer, &scope)?;
            let mut header = page.clone();
            header.records.clear();
            tx.execute(
                "INSERT INTO replica_batches(peer,scope,header,next_index) VALUES (?1,?2,?3,0)",
                params![peer, scope, serde_json::to_string(&header)?],
            )?;
        }
        if page.index < next_index {
            let saved: String = tx.query_row(
                "SELECT value FROM replica_pages WHERE peer=?1 AND scope=?2 AND page_index=?3",
                params![peer, scope, page.index],
                |r| r.get(0),
            )?;
            ensure!(
                serde_json::from_str::<Page>(&saved)? == *page,
                "sync_page_changed"
            );
            return Ok(ReplicaApply::Staged);
        }
        ensure!(page.index == next_index, "sync_page_gap");
        ensure!(page.index < u32::MAX, "sync_page_overflow");
        for record in &page.records {
            upsert(&tx, "replica_staging", peer, &scope, record)?;
        }
        tx.execute(
            "INSERT INTO replica_pages(peer,scope,page_index,value) VALUES (?1,?2,?3,?4)",
            params![peer, scope, page.index, serde_json::to_string(page)?],
        )?;
        tx.execute(
            "UPDATE replica_batches SET next_index=?3 WHERE peer=?1 AND scope=?2",
            params![peer, scope, page.index + 1],
        )?;
        if !page.last {
            tx.commit()?;
            return Ok(ReplicaApply::Staged);
        }

        let mut changed = 0;
        if page.from.is_none() {
            if previous
                .as_ref()
                .is_some_and(|(old, _)| old.epoch != page.through.epoch)
            {
                changed += tx.execute(
                    "DELETE FROM replica_entities WHERE peer=?1 AND scope=?2",
                    params![peer, scope],
                )?;
            } else {
                // Absence is deletion only for a completed, whole-scope snapshot.
                // A newer separately confirmed entity must survive an older snapshot cut.
                changed+=tx.execute("UPDATE replica_entities SET revision=?3,value=NULL WHERE peer=?1 AND scope=?2 AND revision<=?3 AND value IS NOT NULL AND NOT EXISTS(SELECT 1 FROM replica_staging s WHERE s.peer=replica_entities.peer AND s.scope=replica_entities.scope AND s.kind=replica_entities.kind AND s.id=replica_entities.id)",params![peer,scope,page.through.sequence])?;
            }
        }
        // Keep the write transaction bounded in memory even for large snapshot batches.
        let mut after: Option<(String, String)> = None;
        loop {
            let (kind, id) = after.clone().unwrap_or_default();
            let rows=tx.prepare("SELECT kind,id,revision,value FROM replica_staging WHERE peer=?1 AND scope=?2 AND (kind>?3 OR (kind=?3 AND id>?4)) ORDER BY kind,id LIMIT 256")?
                .query_map(params![peer,scope,kind,id],record_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
            if rows.is_empty() {
                break;
            }
            for row in rows {
                after = Some((row.0.clone(), row.1.clone()));
                changed += usize::from(upsert(
                    &tx,
                    "replica_entities",
                    peer,
                    &scope,
                    &decode(row)?,
                )?);
            }
        }
        tx.execute("INSERT INTO replica_scopes(peer,scope,cursor,last_batch) VALUES (?1,?2,?3,?4) ON CONFLICT(peer,scope) DO UPDATE SET cursor=excluded.cursor,last_batch=excluded.last_batch",params![peer,scope,serde_json::to_string(&page.through)?,page.batch_id])?;
        clear_staging(&tx, peer, &scope)?;
        tx.execute(
            "UPDATE replica_bindings SET revoked=0 WHERE peer=?1",
            [peer],
        )?;
        tx.commit()?;
        drop(conn);
        if matches!(page.through.scope, Scope::Catalog {}) {
            self.settings_changed(peer);
        }
        Ok(ReplicaApply::Published {
            cursor: page.through.clone(),
            changed,
        })
    }

    /// Catalog metadata and its cursor are read under one SQLite lock. Message
    /// bodies use scoped page queries and never enter the navigation snapshot.
    pub fn replica_catalog(&self, peer: &str) -> Result<Option<(Cursor, Vec<Record>)>> {
        let conn = self.0.lock().expect("client database");
        let scope = Scope::Catalog {}.key();
        let Some((cursor, _)) = checkpoint(&conn, peer, &scope)? else {
            return Ok(None);
        };
        let rows=conn.prepare("SELECT kind,id,revision,value FROM replica_entities WHERE peer=?1 AND scope=?2 AND value IS NOT NULL ORDER BY kind,id")?
            .query_map(params![peer,scope],record_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Some((
            cursor,
            rows.into_iter().map(decode).collect::<Result<Vec<_>>>()?,
        )))
    }

    pub fn replica_record(
        &self,
        peer: &str,
        scope: &Scope,
        kind: Kind,
        id: &str,
    ) -> Result<Option<Record>> {
        let conn = self.0.lock().expect("client database");
        let row=conn.query_row("SELECT kind,id,revision,value FROM replica_entities WHERE peer=?1 AND scope=?2 AND kind=?3 AND id=?4",params![peer,scope.key(),kind.key(),id],record_row).optional()?;
        row.map(decode).transpose()
    }
    /// Bounded, stable ID pagination. Tombstones remain persisted but aren't rows in a list.
    pub fn replica_records(
        &self,
        peer: &str,
        scope: &Scope,
        kind: Kind,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Record>> {
        ensure!((1..=1024).contains(&limit), "invalid_replica_page_limit");
        let conn = self.0.lock().expect("client database");
        let rows=conn.prepare("SELECT kind,id,revision,value FROM replica_entities WHERE peer=?1 AND scope=?2 AND kind=?3 AND id>?4 AND value IS NOT NULL ORDER BY id LIMIT ?5")?
            .query_map(params![peer,scope.key(),kind.key(),after.unwrap_or_default(),limit],record_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().map(decode).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn page(
        batch: &str,
        sequence: u64,
        from: Option<u64>,
        index: u32,
        last: bool,
        records: Vec<Record>,
    ) -> Page {
        let cursor = |sequence| Cursor {
            owner: "owner".into(),
            epoch: "epoch".into(),
            scope: Scope::Catalog {},
            sequence,
        };
        Page {
            protocol: 1,
            batch_id: batch.into(),
            from: from.map(cursor),
            through: cursor(sequence),
            index,
            last,
            records,
        }
    }
    fn agent(id: &str, revision: u64, name: &str) -> Record {
        Record {
            kind: Kind::Agent,
            id: id.into(),
            revision,
            value: Some(json!({"name":name})),
        }
    }
    fn apply(store: &ClientStore, page: &Page) -> Result<ReplicaApply> {
        store.apply_replica_page("peer", "owner", page)
    }
    #[test]
    fn revoked_binding_rejects_late_pages_and_preserves_local_intent() {
        let dir = tempfile::tempdir().unwrap();
        let store = ClientStore::open(dir.path()).unwrap();
        store.put("peer", "draft:chat", &"keep my draft").unwrap();
        store
            .put(
                "peer",
                "http:/v1/node/agents",
                &serde_json::json!({"body":{"items":[{"id":"old"}]}}),
            )
            .unwrap();
        let old_generation = store.replica_generation("peer").unwrap();
        let initial = page("initial", 1, None, 0, true, vec![]);
        store
            .apply_replica_page_at("peer", "owner", old_generation, &initial)
            .unwrap();
        store.revoke_replica("peer").unwrap();
        assert!(store.replica_revoked("peer").unwrap());
        assert!(store
            .apply_replica_page_at("peer", "owner", old_generation, &initial)
            .is_err());
        assert_eq!(
            store
                .get::<String>("peer", "draft:chat")
                .unwrap()
                .as_deref(),
            Some("keep my draft")
        );
        assert!(store
            .get::<serde_json::Value>("peer", "http:/v1/node/agents")
            .unwrap()
            .is_none());
        let current = store.replica_generation("peer").unwrap();
        let first = page("rebound", 2, None, 0, false, vec![]);
        store
            .apply_replica_page_at("peer", "owner", current, &first)
            .unwrap();
        assert!(store
            .discard_replica_staging_at("peer", &Scope::Catalog {}, old_generation)
            .is_err());
        assert!(
            store
                .replica_state("peer", &Scope::Catalog {})
                .unwrap()
                .staging
        );
        let last = page("rebound", 2, None, 1, true, vec![]);
        store
            .apply_replica_page_at("peer", "owner", current, &last)
            .unwrap();
        assert!(!store.replica_revoked("peer").unwrap());
    }
    #[test]
    fn pages_are_invisible_until_atomically_published_and_resume_after_restart() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = ClientStore::open(dir.path())?;
        let first = page("snapshot", 2, None, 0, false, vec![agent("a", 1, "A")]);
        assert_eq!(apply(&store, &first)?, ReplicaApply::Staged);
        assert!(!store.replica_state("peer", &Scope::Catalog {})?.ready());
        assert!(store
            .replica_record("peer", &Scope::Catalog {}, Kind::Agent, "a")?
            .is_none());
        drop(store);
        let store = ClientStore::open(dir.path())?;
        assert_eq!(apply(&store, &first)?, ReplicaApply::Staged);
        let last = page("snapshot", 2, None, 1, true, vec![agent("b", 2, "B")]);
        assert!(matches!(
            apply(&store, &last)?,
            ReplicaApply::Published { .. }
        ));
        assert_eq!(
            store
                .replica_records("peer", &Scope::Catalog {}, Kind::Agent, None, 10)?
                .len(),
            2
        );
        assert_eq!(apply(&store, &last)?, ReplicaApply::AlreadyApplied);
        Ok(())
    }
    #[test]
    fn empty_snapshot_is_ready_and_failed_refresh_does_not_clear_it() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = ClientStore::open(dir.path())?;
        apply(&store, &page("empty", 0, None, 0, true, vec![]))?;
        assert!(store.replica_state("peer", &Scope::Catalog {})?.ready());
        apply(
            &store,
            &page("new", 1, None, 0, false, vec![agent("a", 1, "A")]),
        )?;
        assert_eq!(
            store
                .replica_state("peer", &Scope::Catalog {})?
                .cursor
                .unwrap()
                .sequence,
            0
        );
        assert!(store
            .replica_records("peer", &Scope::Catalog {}, Kind::Agent, None, 10)?
            .is_empty());
        Ok(())
    }
    #[test]
    fn contradictory_versions_roll_back_entities_and_checkpoint() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = ClientStore::open(dir.path())?;
        apply(
            &store,
            &page("one", 1, None, 0, true, vec![agent("a", 1, "A")]),
        )?;
        let bad = page(
            "bad",
            2,
            Some(1),
            0,
            true,
            vec![agent("b", 2, "B"), agent("a", 1, "Changed")],
        );
        assert!(apply(&store, &bad).is_err());
        assert!(store
            .replica_record("peer", &Scope::Catalog {}, Kind::Agent, "b")?
            .is_none());
        assert_eq!(
            store
                .replica_state("peer", &Scope::Catalog {})?
                .cursor
                .unwrap()
                .sequence,
            1
        );
        Ok(())
    }
    #[test]
    fn deletion_and_stale_updates_do_not_resurrect_records() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = ClientStore::open(dir.path())?;
        apply(
            &store,
            &page("one", 1, None, 0, true, vec![agent("a", 1, "A")]),
        )?;
        let removed = Record {
            kind: Kind::Agent,
            id: "a".into(),
            revision: 2,
            value: None,
        };
        apply(&store, &page("two", 2, Some(1), 0, true, vec![removed]))?;
        apply(
            &store,
            &page("three", 3, Some(2), 0, true, vec![agent("a", 1, "Old")]),
        )?;
        assert!(store
            .replica_record("peer", &Scope::Catalog {}, Kind::Agent, "a")?
            .unwrap()
            .value
            .is_none());
        let gap = page("gap", 5, Some(4), 0, true, vec![]);
        assert!(apply(&store, &gap).is_err());
        Ok(())
    }
    #[test]
    fn replacement_snapshot_removes_absent_entities_only_after_last_page() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = ClientStore::open(dir.path())?;
        apply(
            &store,
            &page(
                "one",
                2,
                None,
                0,
                true,
                vec![agent("a", 1, "A"), agent("b", 2, "B")],
            ),
        )?;
        apply(
            &store,
            &page("two", 3, None, 0, false, vec![agent("a", 3, "A2")]),
        )?;
        assert!(store
            .replica_record("peer", &Scope::Catalog {}, Kind::Agent, "b")?
            .unwrap()
            .value
            .is_some());
        apply(&store, &page("two", 3, None, 1, true, vec![]))?;
        assert!(store
            .replica_record("peer", &Scope::Catalog {}, Kind::Agent, "b")?
            .unwrap()
            .value
            .is_none());
        Ok(())
    }
    #[test]
    fn epoch_reset_and_peer_isolation_preserve_local_user_intent() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = ClientStore::open(dir.path())?;
        store.put("peer", "draft:chat", &"unsent")?;
        let initial = page("one", 9, None, 0, true, vec![agent("a", 9, "A")]);
        apply(&store, &initial)?;
        store.apply_replica_page("another", "owner", &initial)?;
        let mut reset = page("reset", 1, None, 0, true, vec![agent("b", 1, "B")]);
        reset.through.epoch = "restored".into();
        apply(&store, &reset)?;
        assert!(store
            .replica_record("peer", &Scope::Catalog {}, Kind::Agent, "a")?
            .is_none());
        assert!(store
            .replica_record("another", &Scope::Catalog {}, Kind::Agent, "a")?
            .is_some());
        assert_eq!(
            store.get::<String>("peer", "draft:chat")?,
            Some("unsent".into())
        );
        assert!(store
            .apply_replica_page("peer", "different-owner", &reset)
            .is_err());
        Ok(())
    }
    #[test]
    fn page_gaps_and_changed_replays_are_rejected() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = ClientStore::open(dir.path())?;
        let first = page("one", 2, None, 0, false, vec![agent("a", 1, "A")]);
        apply(&store, &first)?;
        let mut altered = first.clone();
        altered.records[0].value = Some(json!({"name":"different"}));
        assert!(apply(&store, &altered).is_err());
        assert!(apply(&store, &page("one", 2, None, 2, true, vec![])).is_err());
        assert!(!store.replica_state("peer", &Scope::Catalog {})?.ready());
        Ok(())
    }
}
