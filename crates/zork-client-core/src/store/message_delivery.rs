//! Sending is a state of the local message, not a second copy of its body.
use super::{ClientStore, QueuedMessage};
use crate::api::{MessageMetadata, Role, TranscriptMessage};
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

pub(super) fn initialize(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    let legacy: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='outbox')",
        [],
        |row| row.get(0),
    )?;
    if legacy {
        let rows = tx
            .prepare("SELECT node,value FROM outbox ORDER BY rowid")?
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (node, value) in rows {
            let pending: QueuedMessage = serde_json::from_str(&value)?;
            let id = message_id(&pending);
            // An already cached source echo is the confirmation, including a
            // crash between the old cache insert and old outbox deletion.
            let confirmed: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE node=?1 AND session=?2 AND id=?3 AND position IS NOT NULL)",
                params![node, pending.session_id, id],
                |row| row.get(0),
            )?;
            if confirmed {
                tx.execute(
                    "UPDATE messages SET request_id=?4,attempted=1,sent_at_ms=?5 WHERE node=?1 AND session=?2 AND id=?3",
                    params![node, pending.session_id, id, pending.request_id, pending.sent_at_ms],
                )?;
            } else {
                insert(tx, &node, &pending)?;
            }
        }
        tx.execute_batch("DROP TABLE outbox")?;
    }
    // A process restart does not dispatch unfinished sends again. Their source
    // echo can still arrive through normal reading and change failed to sent.
    tx.execute(
        "UPDATE messages SET status='failed',attempted=1,error='发送中断，请手动重发' WHERE status='sending'",
        [],
    )?;
    Ok(())
}

pub(super) fn message_id(message: &QueuedMessage) -> String {
    format!("client-{}-{}", message.session_id, message.request_id)
}

pub(super) fn insert(conn: &Connection, node: &str, message: &QueuedMessage) -> Result<()> {
    let record = TranscriptMessage::Message {
        role: Role::User,
        content: message.content.clone(),
        metadata: MessageMetadata {
            id: Some(message_id(message)),
            ..Default::default()
        },
    };
    conn.execute(
        "INSERT INTO messages(node,session,id,position,value,request_id,status,attempted,sent_at_ms,error)
         VALUES(?1,?2,?3,NULL,?4,?5,?6,?7,?8,?9)",
        params![
            node, message.session_id, message_id(message), serde_json::to_string(&record)?,
            message.request_id, if message.error.is_some() { "failed" } else { "sending" },
            message.attempted, message.sent_at_ms, message.error,
        ],
    )?;
    Ok(())
}

const PENDING: &str = "SELECT request_id,session,value,attempted,sent_at_ms,error FROM messages WHERE node=?1 AND status IN ('sending','failed')";

fn queued(row: &rusqlite::Row<'_>) -> rusqlite::Result<QueuedMessage> {
    let raw: String = row.get(2)?;
    let record: TranscriptMessage = serde_json::from_str(&raw).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let TranscriptMessage::Message { content, .. } = record;
    Ok(QueuedMessage {
        request_id: row.get(0)?,
        session_id: row.get(1)?,
        content,
        attempted: row.get(3)?,
        sent_at_ms: row.get(4)?,
        error: row.get(5)?,
    })
}

fn pending(conn: &Connection, node: &str, request: &str) -> Result<Option<QueuedMessage>> {
    Ok(conn
        .query_row(
            &format!("{PENDING} AND request_id=?2"),
            params![node, request],
            queued,
        )
        .optional()?)
}

pub(super) fn upload_keys(
    conn: &Connection,
    node: &str,
) -> Result<std::collections::HashSet<String>> {
    let rows = conn
        .prepare(PENDING)?
        .query_map([node], queued)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .flat_map(|message| {
            zork_client_types::files::decode(&message.content)
                .into_iter()
                .flat_map(|(_, files)| files)
                .map(|file| format!("upload:{}", file.id))
        })
        .collect())
}

impl ClientStore {
    /// A read-only dispatch view of the same rows displayed by the conversation.
    pub fn outbox(&self, node: &str) -> Result<Vec<QueuedMessage>> {
        let conn = self.0.lock().expect("client database");
        let rows = conn
            .prepare(&format!("{PENDING} ORDER BY rowid"))?
            .query_map([node], queued)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn begin_delivery(&self, node: &str, id: &str) -> Result<Option<QueuedMessage>> {
        let conn = self.0.lock().expect("client database");
        let Some(mut message) = pending(&conn, node, id)? else {
            return Ok(None);
        };
        if message.attempted || message.error.is_some() {
            return Ok(None);
        }
        message.sent_at_ms = super::delivery_now_ms();
        conn.execute("UPDATE messages SET attempted=1,sent_at_ms=?3 WHERE node=?1 AND request_id=?2 AND status='sending'", params![node,id,message.sent_at_ms])?;
        message.attempted = true;
        drop(conn);
        self.delivery_changed();
        Ok(Some(message))
    }

    pub(crate) fn confirmed_message(
        &self,
        node: &str,
        session: &str,
        id: &str,
        generation: u64,
    ) -> Result<Option<TranscriptMessage>> {
        let conn = self.0.lock().expect("client database");
        super::messages::authorized(&conn, node, generation)?;
        let value: Option<String> = conn.query_row(
            "SELECT value FROM messages WHERE node=?1 AND session=?2 AND id=?3 AND status='sent'",
            params![node,session,id], |row| row.get(0),
        ).optional()?;
        value
            .map(|raw| serde_json::from_str(&raw).map_err(Into::into))
            .transpose()
    }

    pub fn fail_delivery(&self, node: &str, id: &str, error: &str) -> Result<()> {
        let changed = self.0.lock().expect("client database").execute(
            "UPDATE messages SET status='failed',error=?3 WHERE node=?1 AND request_id=?2 AND status='sending'",
            params![node,id,error],
        )?;
        if changed > 0 {
            self.delivery_changed();
        }
        Ok(())
    }

    /// Every explicit resend is a new message. The old source echo, if it
    /// arrives later, can only update the old row.
    pub fn retry_delivery(&self, node: &str, id: &str) -> Result<()> {
        let conn = self.0.lock().expect("client database");
        let mut message = pending(&conn, node, id)?.context("Failed message unavailable")?;
        ensure!(
            message.error.is_some(),
            "Only failed messages can be resent"
        );
        message.request_id = ulid::Ulid::new().to_string();
        message.error = None;
        message.attempted = false;
        message.sent_at_ms = super::delivery_now_ms();
        insert(&conn, node, &message)?;
        drop(conn);
        self.delivery_changed();
        Ok(())
    }

    pub fn delete_failed(&self, node: &str, id: &str) -> Result<()> {
        let conn = self.0.lock().expect("client database");
        if let Some(message) = pending(&conn, node, id)? {
            ensure!(
                message.error.is_some(),
                "Only failed messages can be deleted"
            );
            conn.execute(
                "DELETE FROM messages WHERE node=?1 AND request_id=?2 AND status='failed'",
                params![node, id],
            )?;
        }
        drop(conn);
        self.delivery_changed();
        Ok(())
    }

    pub fn cancel_pending(&self, node: &str, id: &str) -> Result<Option<QueuedMessage>> {
        let conn = self.0.lock().expect("client database");
        let Some(message) = pending(&conn, node, id)? else {
            return Ok(None);
        };
        ensure!(
            !message.attempted,
            "消息已尝试发送，需等待送达确认，不能将它视为已撤回"
        );
        conn.execute("DELETE FROM messages WHERE node=?1 AND request_id=?2 AND status!='sent' AND attempted=0",params![node,id])?;
        drop(conn);
        self.delivery_changed();
        Ok(Some(message))
    }

    pub fn withdraw_to_draft(&self, node: &str, id: &str) -> Result<Option<QueuedMessage>> {
        let mut conn = self.0.lock().expect("client database");
        let tx = conn.transaction()?;
        let Some(message) = pending(&tx, node, id)? else {
            return Ok(None);
        };
        ensure!(
            !message.attempted,
            "消息已尝试发送，需等待送达确认，不能将它视为已撤回"
        );
        let key = format!("draft:{}", message.session_id);
        let previous: Option<String> = tx
            .query_row(
                "SELECT value FROM cache WHERE node=?1 AND key=?2",
                params![node, key],
                |row| row.get(0),
            )
            .optional()?;
        let previous: String = previous
            .map(|value| serde_json::from_str(&value))
            .transpose()?
            .unwrap_or_default();
        let restored = crate::comments::merge_drafts(&previous, &message.content);
        tx.execute("INSERT INTO cache(node,key,value) VALUES(?1,?2,?3) ON CONFLICT(node,key) DO UPDATE SET value=excluded.value",params![node,key,serde_json::to_string(&restored)?])?;
        tx.execute("DELETE FROM messages WHERE node=?1 AND request_id=?2 AND status!='sent' AND attempted=0",params![node,id])?;
        tx.commit()?;
        drop(conn);
        self.delivery_changed();
        Ok(Some(message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::MessagePage;

    fn sending(id: &str) -> QueuedMessage {
        QueuedMessage {
            request_id: id.into(),
            session_id: "chat".into(),
            content: "hello".into(),
            attempted: false,
            sent_at_ms: super::super::delivery_now_ms(),
            error: None,
        }
    }
    fn echo(pending: &QueuedMessage) -> TranscriptMessage {
        TranscriptMessage::Message {
            role: Role::User,
            content: pending.content.clone(),
            metadata: MessageMetadata {
                id: Some(message_id(pending)),
                created_at: Some("2026-09-14T00:00:00Z".into()),
                author_kind: Some(zork_client_types::chat::AuthorKind::User),
                ..Default::default()
            },
        }
    }
    fn receive(store: &ClientStore, pending: &QueuedMessage) {
        store
            .cache_message_page(
                "node",
                "chat",
                &MessagePage {
                    source_epoch: None,
                    items: vec![echo(pending)],
                    older_cursor: None,
                },
                None,
            )
            .unwrap();
    }

    #[test]
    fn saved_sending_message_becomes_sent_in_the_same_row() {
        let root = tempfile::tempdir().unwrap();
        let pending = sending("first");
        let store = ClientStore::open(root.path()).unwrap();
        store.enqueue("node", &pending).unwrap();
        let before: (i64, String, Option<i64>) = store
            .0
            .lock()
            .unwrap()
            .query_row("SELECT rowid,status,position FROM messages", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .unwrap();
        assert_eq!(before.1, "sending");
        assert_eq!(before.2, None);
        assert!(store
            .cached_messages("node", "chat", None, 100)
            .unwrap()
            .is_none());
        receive(&store, &pending);
        let after: (i64, String, i64) = store
            .0
            .lock()
            .unwrap()
            .query_row("SELECT rowid,status,COUNT(*) FROM messages", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .unwrap();
        assert_eq!(after, (before.0, "sent".into(), 1));
        assert!(store.outbox("node").unwrap().is_empty());
        store
            .fail_delivery("node", "first", "late request error")
            .unwrap();
        drop(store);
        let store = ClientStore::open(root.path()).unwrap();
        assert!(store.outbox("node").unwrap().is_empty());
        assert_eq!(
            store
                .cached_messages("node", "chat", None, 100)
                .unwrap()
                .unwrap()
                .items,
            [echo(&pending)]
        );
    }

    #[test]
    fn manual_resend_is_new_and_old_echo_only_confirms_old_send() {
        let root = tempfile::tempdir().unwrap();
        let store = ClientStore::open(root.path()).unwrap();
        let old = sending("first");
        store.enqueue("node", &old).unwrap();
        store.begin_delivery("node", "first").unwrap();
        store
            .fail_delivery("node", "first", "response lost")
            .unwrap();
        store.retry_delivery("node", "first").unwrap();
        let pending = store.outbox("node").unwrap();
        assert_eq!(pending.len(), 2);
        let new = pending
            .iter()
            .find(|message| message.request_id != "first")
            .unwrap()
            .clone();
        assert_eq!(new.content, old.content);
        assert!(!new.attempted);
        receive(&store, &old);
        assert_eq!(store.outbox("node").unwrap(), [new.clone()]);
        store.fail_delivery("node", "first", "late error").unwrap();
        assert_eq!(store.outbox("node").unwrap(), [new.clone()]);
        receive(&store, &new);
        assert!(store.outbox("node").unwrap().is_empty());
        assert_eq!(
            store
                .cached_messages("node", "chat", None, 100)
                .unwrap()
                .unwrap()
                .items,
            [echo(&old), echo(&new)]
        );
    }

    #[test]
    fn migration_merges_old_cached_echo_and_pending_copy_without_losing_source() {
        let root = tempfile::tempdir().unwrap();
        let old = sending("first");
        let conn = Connection::open(root.path().join("client.db")).unwrap();
        conn.execute_batch("CREATE TABLE delivered_messages(node TEXT,session TEXT,id TEXT,position INTEGER,value TEXT);
            CREATE TABLE outbox(node TEXT,request_id TEXT,value TEXT);").unwrap();
        conn.execute(
            "INSERT INTO delivered_messages VALUES('node','chat',?1,7,?2)",
            params![
                message_id(&old),
                serde_json::to_string(&echo(&old)).unwrap()
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO outbox VALUES('node','first',?1)",
            [serde_json::to_string(&old).unwrap()],
        )
        .unwrap();
        drop(conn);
        let store = ClientStore::open(root.path()).unwrap();
        assert!(store.outbox("node").unwrap().is_empty());
        let conn = store.0.lock().unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM messages", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('outbox','delivered_messages')",[],|row|row.get::<_,i64>(0)).unwrap(),0);
        let value: String = conn
            .query_row("SELECT value FROM messages", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            serde_json::from_str::<TranscriptMessage>(&value).unwrap(),
            echo(&old)
        );
    }

    #[test]
    fn restart_and_revoke_preserve_unconfirmed_body_and_attachment() {
        let root = tempfile::tempdir().unwrap();
        let file = zork_client_types::files::FileRef {
            id: "attachment".into(),
            name: "hello.txt".into(),
            byte_len: 5,
            content_root: zork_mesh::content_root(b"hello"),
        };
        let mut pending = sending("first");
        pending.content = zork_client_types::files::compose("body", &[file]);
        let store = ClientStore::open(root.path()).unwrap();
        store
            .put_blob("node", "upload:attachment", b"hello")
            .unwrap();
        store
            .put_blob("node", "download:discard", b"cache")
            .unwrap();
        store.enqueue("node", &pending).unwrap();
        drop(store);
        let store = ClientStore::open(root.path()).unwrap();
        assert_eq!(store.outbox("node").unwrap()[0].delivery_status(), "failed");
        store.revoke_replica("node").unwrap();
        assert_eq!(store.outbox("node").unwrap()[0].content, pending.content);
        assert_eq!(
            store.blob("node", "upload:attachment").unwrap().unwrap(),
            b"hello"
        );
        assert!(store.blob("node", "download:discard").unwrap().is_none());
    }
}
