//! Device-local state exists independently of any running Station.
mod configuration_submissions;
mod directory;
mod message_delivery;
mod messages;
pub(crate) use configuration_submissions::ConfigurationDelivery;
mod new_chat;
mod operations;
mod outgoing;
mod replica;
use anyhow::Result;
pub use replica::{ReplicaApply, ReplicaState};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Mutex};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedNode {
    pub id: String,
    pub name: String,
    pub url: String,
    pub token: Option<String>,
    pub local: bool,
    #[serde(default)]
    pub mesh: Option<RemoteNode>,
    #[serde(default)]
    pub group: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RemoteNode {
    pub origin: String,
    pub addr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routes: Option<zork_config::membership::MeshRoutes>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedMessage {
    pub request_id: String,
    pub session_id: String,
    pub content: String,
    #[serde(default = "legacy_delivery_uncertain")]
    pub attempted: bool,
    #[serde(default)]
    pub sent_at_ms: u64,
    #[serde(default)]
    pub error: Option<String>,
}
impl QueuedMessage {
    pub fn delivery_status(&self) -> &'static str {
        if self.error.is_some() {
            "failed"
        } else {
            "sending"
        }
    }
}
pub fn delivery_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// Older clients did not record delivery attempts. Treat those receipts as
// uncertain, so upgrading never exposes a false "withdraw unsent" action.
fn legacy_delivery_uncertain() -> bool {
    true
}
pub struct ClientStore(
    Mutex<Connection>,
    pub(crate) Mutex<std::collections::HashMap<String, std::sync::Weak<crate::state::Device>>>,
    tokio::sync::watch::Sender<u64>,
    Mutex<std::collections::HashMap<String, std::sync::Arc<zork_observe::ValueSource<()>>>>,
);
impl ClientStore {
    pub(crate) fn submit_current_draft(
        &self,
        peer: &str,
        session: &str,
        text: String,
    ) -> Result<QueuedMessage> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        let key = format!("draft:{session}");
        let raw: Option<String> = tx
            .query_row(
                "SELECT value FROM cache WHERE node=?1 AND key=?2",
                params![peer, key],
                |r| r.get(0),
            )
            .optional()?;
        let raw = raw
            .map(|s| serde_json::from_str::<String>(&s))
            .transpose()?
            .unwrap_or_default();
        let mut draft = crate::state::Draft::decode(raw);
        draft.text = text;
        let content = draft.submission(&draft.text);
        let message = QueuedMessage {
            request_id: ulid::Ulid::new().to_string(),
            session_id: session.into(),
            content,
            attempted: false,
            sent_at_ms: delivery_now_ms(),
            ..Default::default()
        };
        let message = outgoing::insert_and_clear_draft(&tx, peer, &message)?;
        tx.commit()?;
        drop(conn);
        self.delivery_changed();
        Ok(message)
    }
    pub(crate) fn edit_draft(
        &self,
        peer: &str,
        session: &str,
        action: crate::state::DraftAction,
    ) -> Result<()> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        let key = format!("draft:{session}");
        let raw: Option<String> = tx
            .query_row(
                "SELECT value FROM cache WHERE node=?1 AND key=?2",
                params![peer, key],
                |r| r.get(0),
            )
            .optional()?;
        let raw = raw
            .map(|s| serde_json::from_str::<String>(&s))
            .transpose()?
            .unwrap_or_default();
        let mut draft = crate::state::Draft::decode(raw);
        draft.apply(session, action)?;
        let value = serde_json::to_string(&draft.encoded())?;
        tx.execute("INSERT INTO cache(node,key,value) VALUES(?1,?2,?3) ON CONFLICT(node,key) DO UPDATE SET value=excluded.value",params![peer,key,value])?;
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn delivery_events(&self) -> tokio::sync::watch::Receiver<u64> {
        self.2.subscribe()
    }
    fn delivery_changed(&self) {
        self.2
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }

    pub fn open(root: &Path) -> Result<Self> {
        let channel = zork_config::channel::activate_for_data(root)?;
        zork_config::channel::claim(root, channel)?;
        std::fs::create_dir_all(root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))?;
        }
        let path = root.join("client.db");
        let mut conn = Connection::open(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS nodes(id TEXT PRIMARY KEY, value TEXT NOT NULL); CREATE TABLE IF NOT EXISTS cache(node TEXT NOT NULL,key TEXT NOT NULL,value TEXT NOT NULL,PRIMARY KEY(node,key)); CREATE TABLE IF NOT EXISTS blobs(node TEXT NOT NULL,key TEXT NOT NULL,value BLOB NOT NULL,PRIMARY KEY(node,key));")?;
        replica::initialize(&tx)?;
        operations::initialize(&tx)?;
        messages::initialize(&tx)?;
        message_delivery::initialize(&tx)?;
        configuration_submissions::initialize(&tx)?;
        tx.commit()?;
        Ok(Self(
            Mutex::new(conn),
            Mutex::new(Default::default()),
            tokio::sync::watch::channel(0).0,
            Mutex::new(Default::default()),
        ))
    }
    /// Persist user intent separately from the lifetime of the node process.
    pub fn local_node_enabled(&self) -> Result<bool> {
        if let Some(enabled) = self.get("device", "local-node-enabled")? {
            return Ok(enabled);
        }
        // Older installs only recorded nodes that had actually been enabled.
        let enabled = self.nodes()?.iter().any(|node| node.local);
        self.set_local_node_enabled(enabled)?;
        Ok(enabled)
    }
    pub fn set_local_node_enabled(&self, enabled: bool) -> Result<()> {
        self.put("device", "local-node-enabled", &enabled)
    }
    pub fn put_blob(&self, node: &str, key: &str, bytes: &[u8]) -> Result<()> {
        self.0.lock().expect("client database").execute("INSERT INTO blobs(node,key,value) VALUES (?1,?2,?3) ON CONFLICT(node,key) DO UPDATE SET value=excluded.value",params![node,key,bytes])?;
        Ok(())
    }
    pub fn blob(&self, node: &str, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self
            .0
            .lock()
            .expect("client database")
            .query_row(
                "SELECT value FROM blobs WHERE node=?1 AND key=?2",
                params![node, key],
                |r| r.get(0),
            )
            .optional()?)
    }
    /// Save the local message as sending and clear its draft in one transaction.
    pub fn enqueue_and_clear_draft(
        &self,
        node: &str,
        message: &QueuedMessage,
    ) -> Result<QueuedMessage> {
        let mut conn = self.0.lock().expect("client database");
        let transaction = conn.transaction()?;
        let message = outgoing::insert_and_clear_draft(&transaction, node, message)?;
        transaction.commit()?;
        drop(conn);
        self.delivery_changed();
        Ok(message)
    }
    pub fn enqueue(&self, node: &str, message: &QueuedMessage) -> Result<()> {
        message_delivery::insert(&self.0.lock().expect("client database"), node, message)?;
        self.delivery_changed();
        Ok(())
    }
    pub fn nodes(&self) -> Result<Vec<SavedNode>> {
        let conn = self.0.lock().expect("client database");
        let rows = conn
            .prepare("SELECT value FROM nodes ORDER BY rowid")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().map(|row| {
            let mut node:SavedNode=serde_json::from_str(&row)?;
            let name:Option<String>=conn.query_row("SELECT json_extract(value,'$.name') FROM replica_entities WHERE peer=?1 AND scope=?2 AND kind='device' AND id='self' AND value IS NOT NULL",params![node.id,zork_client_types::sync::Scope::Catalog{}.key()],|r|r.get(0)).optional()?.flatten();
            if let Some(name)=name.filter(|name|!name.trim().is_empty()){node.name=name;}
            Ok(node)
        }).collect()
    }

    pub(crate) fn replace_account_nodes(&self, candidates: &[SavedNode]) -> Result<Vec<String>> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        let previous: Option<String> = tx
            .query_row(
                "SELECT value FROM cache WHERE node='device' AND key='account_peers'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let previous: Vec<String> = previous
            .map(|value| serde_json::from_str(&value))
            .transpose()?
            .unwrap_or_default();
        let old: Vec<SavedNode> = tx
            .prepare("SELECT value FROM nodes")?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(|value| serde_json::from_str(&value))
            .collect::<serde_json::Result<_>>()?;
        let removed_groups: Vec<_> = old
            .iter()
            .filter(|node| {
                previous.contains(&node.id) && !candidates.iter().any(|next| next.id == node.id)
            })
            .filter_map(|node| node.group.clone())
            .collect();
        let removed: Vec<_> = old
            .iter()
            .filter(|node| {
                (previous.contains(&node.id) && !candidates.iter().any(|next| next.id == node.id))
                    || node
                        .group
                        .as_ref()
                        .is_some_and(|group| removed_groups.contains(group))
            })
            .map(|node| node.id.clone())
            .collect();
        let mut changed = 0;
        for id in &removed {
            changed += tx.execute("DELETE FROM nodes WHERE id=?1", [id])?;
        }
        let mut owned = Vec::new();
        for candidate in candidates {
            let existing = old.iter().find(|node| node.id == candidate.id);
            if existing.is_some() && !previous.contains(&candidate.id) {
                continue;
            }
            let mut node = candidate.clone();
            node.group = existing.and_then(|node| node.group.clone());
            changed += tx.execute("INSERT INTO nodes(id,value) VALUES (?1,?2) ON CONFLICT(id) DO UPDATE SET value=excluded.value WHERE nodes.value != excluded.value",params![node.id,serde_json::to_string(&node)?])?;
            owned.push(node.id);
        }
        tx.execute("INSERT INTO cache(node,key,value) VALUES('device','account_peers',?1) ON CONFLICT(node,key) DO UPDATE SET value=excluded.value WHERE cache.value != excluded.value", [serde_json::to_string(&owned)?])?;
        tx.commit()?;
        drop(conn);
        if changed > 0 {
            self.directory_changed();
            self.notifications_changed();
        }
        Ok(removed)
    }

    pub fn save_node(&self, node: &SavedNode) -> Result<()> {
        let changed = self.0.lock().expect("client database").execute("INSERT INTO nodes(id,value) VALUES (?1,?2) ON CONFLICT(id) DO UPDATE SET value=excluded.value WHERE nodes.value != excluded.value",params![node.id,serde_json::to_string(node)?])?;
        if changed > 0 {
            self.directory_changed();
            self.notifications_changed();
        }
        Ok(())
    }
    /// Removing a connection preserves its local history, files and drafts.
    pub fn remove_node(&self, id: &str) -> Result<()> {
        self.0
            .lock()
            .expect("client database")
            .execute("DELETE FROM nodes WHERE id=?1", [id])?;
        self.directory_changed();
        self.notifications_changed();
        Ok(())
    }
    pub fn put<T: Serialize>(&self, node: &str, key: &str, value: &T) -> Result<()> {
        let changed = self.0.lock().expect("client database").execute("INSERT INTO cache(node,key,value) VALUES (?1,?2,?3) ON CONFLICT(node,key) DO UPDATE SET value=excluded.value WHERE cache.value != excluded.value",params![node,key,serde_json::to_string(value)?])?;
        if changed > 0 && node == "device" && key == "last-node" {
            self.directory_changed();
        }
        if changed > 0
            && matches!(
                key,
                "public-settings"
                    | "node-operation"
                    | "node-update-check"
                    | "settings-command"
                    | "profile-authorization"
            )
        {
            self.settings_changed(node);
        }
        if changed > 0
            && matches!(
                key,
                crate::notifications::KEY
                    | crate::notifications::PREFERENCES
                    | crate::notifications::mobile::BACKGROUND
            )
        {
            self.notifications_changed();
        }
        Ok(())
    }
    pub(crate) fn notification_events(&self) -> zork_observe::ValueSubscription<()> {
        self.settings_events("\0notifications")
    }
    /// Fence late authorization results against a committed revocation.
    pub(crate) fn put_authorized_settings<T: Serialize>(
        &self,
        node: &str,
        key: &str,
        value: &T,
    ) -> Result<()> {
        let conn = self.0.lock().expect("client database");
        let revoked = conn
            .query_row(
                "SELECT revoked FROM replica_bindings WHERE peer=?1",
                [node],
                |r| r.get::<_, bool>(0),
            )
            .optional()?
            .unwrap_or(false);
        anyhow::ensure!(!revoked, "设备访问权限已撤销");
        let changed = conn.execute("INSERT INTO cache(node,key,value) VALUES (?1,?2,?3) ON CONFLICT(node,key) DO UPDATE SET value=excluded.value WHERE cache.value != excluded.value",params![node,key,serde_json::to_string(value)?])?;
        drop(conn);
        if changed > 0 {
            self.settings_changed(node);
        }
        Ok(())
    }
    pub(crate) fn notifications_changed(&self) {
        self.settings_changed("\0notifications");
    }
    pub(crate) fn settings_events(&self, node: &str) -> zork_observe::ValueSubscription<()> {
        self.3
            .lock()
            .unwrap()
            .entry(node.into())
            .or_insert_with(|| std::sync::Arc::new(zork_observe::ValueSource::new(())))
            .subscribe()
    }
    fn settings_changed(&self, node: &str) {
        let source = self.3.lock().unwrap().get(node).cloned();
        if let Some(source) = source {
            source.publish_changed((), zork_observe::Topics::ALL);
        }
    }
    pub fn get<T: serde::de::DeserializeOwned>(&self, node: &str, key: &str) -> Result<Option<T>> {
        let data: Option<String> = self
            .0
            .lock()
            .expect("client database")
            .query_row(
                "SELECT value FROM cache WHERE node=?1 AND key=?2",
                params![node, key],
                |r| r.get(0),
            )
            .optional()?;
        data.map(|data| Ok(serde_json::from_str(&data)?))
            .transpose()
    }
}

#[cfg(test)]
mod startup_tests {
    use super::*;

    #[test]
    fn a_failed_legacy_migration_rolls_back_the_entire_schema_change() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("client.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE cache(node TEXT,key TEXT,value TEXT NOT NULL,PRIMARY KEY(node,key));
            INSERT INTO cache VALUES('device','local-node-enabled','true');
            CREATE TABLE outbox(node TEXT,value TEXT);
            INSERT INTO outbox VALUES('peer','invalid queued message');",
        )
        .unwrap();
        drop(conn);
        assert!(ClientStore::open(root.path()).is_err());
        let conn = Connection::open(&path).unwrap();
        assert_eq!(
            conn.query_row("SELECT value FROM outbox", [], |row| row
                .get::<_, String>(0))
                .unwrap(),
            "invalid queued message"
        );
        assert!(!conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='messages')",
                [],
                |row| row.get::<_, bool>(0)
            )
            .unwrap());
        // Repair only this malformed fixture, then prove a normal retry works.
        conn.execute("DELETE FROM outbox", []).unwrap();
        drop(conn);
        assert!(ClientStore::open(root.path())
            .unwrap()
            .local_node_enabled()
            .unwrap());
    }
}

#[cfg(test)]
mod draft_transaction_tests {
    use super::*;
    #[test]
    fn queued_batch_clears_only_its_own_draft_and_failure_preserves_draft() {
        let root = tempfile::tempdir().unwrap();
        let store = ClientStore::open(root.path()).unwrap();
        store
            .put("a", "draft:conversation", &"pending text")
            .unwrap();
        store
            .put("a", "draft-comments:conversation", &vec!["comment"])
            .unwrap();
        store
            .put("b", "draft:conversation", &"other device")
            .unwrap();
        let message = QueuedMessage {
            request_id: "one".into(),
            session_id: "conversation".into(),
            content: "batch".into(),
            attempted: false,
            sent_at_ms: crate::store::delivery_now_ms(),
            ..Default::default()
        };
        store.enqueue_and_clear_draft("a", &message).unwrap();
        assert_eq!(
            store
                .get::<String>("a", "draft:conversation")
                .unwrap()
                .as_deref(),
            Some("")
        );
        assert!(store
            .get::<Vec<String>>("a", "draft-comments:conversation")
            .unwrap()
            .unwrap()
            .is_empty());
        assert_eq!(
            store
                .get::<String>("b", "draft:conversation")
                .unwrap()
                .as_deref(),
            Some("other device")
        );
        store.put("a", "draft:conversation", &"next draft").unwrap();
        assert!(store.enqueue_and_clear_draft("a", &message).is_err());
        assert_eq!(
            store
                .get::<String>("a", "draft:conversation")
                .unwrap()
                .as_deref(),
            Some("next draft")
        );
        assert_eq!(store.outbox("a").unwrap().len(), 1);
    }
}
