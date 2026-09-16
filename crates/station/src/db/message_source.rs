//! The business transaction stages a complete publication. Its immutable source
//! is synced before any delivery, then the temporary SQL body is cleared.
use super::*;
use crate::message_log::{MessageLog, Record};
use rusqlite::functions::FunctionFlags;
use zork_client_types::chat::Message;

pub(super) fn install_reader(conn: &Connection, log: Arc<MessageLog>) -> Result<()> {
    let body_log = log.clone();
    conn.create_scalar_function("message_value", 1, FunctionFlags::SQLITE_UTF8, move |ctx| {
        body_log
            .get(ctx.get(0)?)
            .and_then(|record| Ok(serde_json::to_string(&record.message)?))
            .map_err(|error| rusqlite::Error::UserFunctionError(error.into()))
    })?;
    conn.create_scalar_function("message_text", 3, FunctionFlags::SQLITE_UTF8, move |ctx| {
        if !ctx.get::<bool>(2)? {
            return ctx.get::<String>(1);
        }
        log.get(ctx.get(0)?)
            .map(|record| record.text())
            .map_err(|error| rusqlite::Error::UserFunctionError(error.into()))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zork_client_types::chat::{Author, AuthorKind};
    use zork_client_types::sync::{Kind, Pull, Reply, Scope};

    fn chat(db: &GatewayDb) -> String {
        db.chat_begin("create", "create").unwrap();
        db.create_chat("create", "owned-chat", "Message archive")
            .unwrap()
            .chat_id
    }
    fn author() -> Author {
        Author {
            id: "writer".into(),
            kind: AuthorKind::Agent,
            name: Some("Writer".into()),
        }
    }
    fn send(db: &GatewayDb, chat: &str, id: &str) -> Message {
        db.post_chat_content(
            None,
            id,
            chat,
            &author(),
            "完整正文\n```rs\nlet x = 1;\n```",
            &[],
            None,
            &[],
            &[],
            None,
        )
        .unwrap()
    }

    #[test]
    fn local_script_cards_survive_source_rebuild_without_user_action_registration() {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("workspaces");
        let db = GatewayDb::open(root.path(), &work).unwrap();
        let chat = chat(&db);
        let script: zork_client_types::local_script::Card = serde_json::from_value(json!({
            "title":"Developer options", "source":"await android.startActivity({action:'android.settings.APPLICATION_DEVELOPMENT_SETTINGS'});"
        })).unwrap();
        let payload = serde_json::to_value(script).unwrap();
        let posted = db.post_chat_content(None, "script", &chat, &author(), "", &[], None, &[], &[], Some(&payload)).unwrap();
        assert_eq!(posted.interaction.as_ref(), Some(&payload));
        assert!(db.interaction_result(&chat, "script").unwrap().is_none());
        assert_eq!(db.conn.lock().unwrap().query_row("SELECT COUNT(*) FROM interaction_registrations", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        let mut bad = payload.clone(); bad["version"] = json!(999);
        assert!(db.post_chat_content(None, "unsupported", &chat, &author(), "", &[], None, &[], &[], Some(&bad)).is_err());
        assert_eq!(db.chat(&chat).unwrap().channel.message_count, 1);
        drop(db);
        fs::remove_dir_all(root.path().join("cache")).unwrap();
        let db = GatewayDb::open(root.path(), &work).unwrap();
        assert_eq!(db.chat_message(&chat, "script").unwrap().interaction, Some(payload));
        assert!(db.interaction_result(&chat, "script").unwrap().is_none());
        assert_eq!(db.chat(&chat).unwrap().channel.message_count, 1);
    }

    #[test]
    fn ordinary_sends_read_source_and_rebuild_both_location_and_message_indexes() {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("workspaces");
        let db = GatewayDb::open(root.path(), &work).unwrap();
        let chat = chat(&db);
        let first = send(&db, &chat, "first");
        let second = send(&db, &chat, "second");
        assert_ne!(first.message_id, second.message_id);
        {
            let conn = db.conn.lock().unwrap();
            let rows: usize = conn
                .query_row(
                    "SELECT COUNT(*) FROM visible_messages WHERE archived=1 AND text=''",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(rows, 2);
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM chat_receipts", [], |r| r
                    .get::<_, usize>(0))
                    .unwrap(),
                1
            );
            assert!(conn
                .execute(
                    "UPDATE visible_messages SET text='changed' WHERE message_id='first'",
                    []
                )
                .is_err());
        }
        let Reply::Page { page } = db
            .sync_pull(
                "owner",
                &Pull {
                    scope: Scope::Conversation { id: chat.clone() },
                    after: None,
                    continuation: None,
                },
            )
            .unwrap()
        else {
            panic!("message export missing")
        };
        let exported = page
            .records
            .iter()
            .find(|record| record.kind == Kind::Message && record.id == "first")
            .unwrap();
        assert_eq!(exported.value.as_ref().unwrap()["text"], first.text);
        {
            let conn = db.conn.lock().unwrap();
            // Simulate a lost rebuildable projection. The binding itself is
            // still authoritative business state and must not be recreated.
            conn.execute_batch("DELETE FROM chat_notices; DELETE FROM chat_message_facts; DELETE FROM visible_messages;").unwrap();
        }
        drop(db);
        fs::remove_dir_all(root.path().join("cache")).unwrap();
        let db = GatewayDb::open(root.path(), &work).unwrap();
        assert_eq!(db.chat_message(&chat, "first").unwrap(), first);
        assert_eq!(db.chat_message(&chat, "second").unwrap(), second);
        assert_eq!(db.chat(&chat).unwrap().channel.message_count, 2);
        assert_eq!(db.chat_messages(&chat, None, 100).unwrap().len(), 2);
        assert_eq!(db.message_log.positions(0, 100).unwrap(), vec![1, 2]);
    }

    #[test]
    fn migration_and_interrupted_publication_keep_the_physical_source_prefix() {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("workspaces");
        let db = GatewayDb::open(root.path(), &work).unwrap();
        let chat = chat(&db);
        let binding = db.chat(&chat).unwrap();
        let now = now_rfc3339();
        {
            let conn = db.conn.lock().unwrap();
            for id in ["persisted-before-crash", "old-sql-only"] {
                conn.execute("INSERT INTO visible_messages(message_id,session_key,connection_id,conversation_id,root_message_id,role,text,kind,created_at)
                    SELECT ?1,key,connection_id,channel_id,root_thread_ts,'assistant',?2,'message',?3 FROM sessions WHERE key=?4",
                    params![id,format!("unchanged {id}"),now,binding.session_key]).unwrap();
                let row=conn.query_row("SELECT sequence,message_id,session_key,role,text,kind,created_at FROM visible_messages WHERE message_id=?1",[id],map_visible_message_row).unwrap();
                chats::record(&conn, &row, Some(&author()), None, &[], false).unwrap();
            }
            let message = chats::message(&conn, "persisted-before-crash").unwrap();
            db.message_log
                .append(&[Record {
                    sequence: 1,
                    session_key: binding.session_key.clone(),
                    role: "assistant".into(),
                    kind: Some("message".into()),
                    message,
                    pages: vec![zork_client_types::pages::DeliveredPage {
                        page: pages::page_link("Original page", "https://example.com/original", "")
                            .unwrap(),
                    }],
                }])
                .unwrap();
            // The synced source exists; SQL still has its pending publication.
            // The rebuildable page catalog no longer carries this older page.
        }
        drop(db);
        let db = GatewayDb::open(root.path(), &work).unwrap();
        assert_eq!(db.message_log.positions(0, 100).unwrap(), vec![1, 2]);
        assert_eq!(
            db.message_log.get(1).unwrap().pages[0].page.title,
            "Original page"
        );
        assert_eq!(
            db.list_visible_messages(&binding.session_key, None, 100)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            db.chat_message(&chat, "old-sql-only").unwrap().text,
            "unchanged old-sql-only"
        );
        assert_eq!(
            db.conn
                .lock()
                .unwrap()
                .query_row("SELECT SUM(length(text)) FROM visible_messages", [], |r| {
                    r.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
        let next = send(&db, &chat, "manual-new-send");
        assert_eq!(
            db.chat_visible_message(&next.message_id).unwrap().sequence,
            3
        );
        assert_eq!(db.message_log.positions(0, 100).unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn source_cursors_are_scoped_to_chat_and_lineage_and_survive_index_rebuild() {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("workspaces");
        let db = GatewayDb::open(root.path(), &work).unwrap();
        let chat = chat(&db);
        send(&db, &chat, "message");
        let cursor = db.message_cursor(&chat, 1).unwrap();
        db.chat_begin("other-create", "other-create").unwrap();
        let other = db
            .create_chat("other-create", "other-chat", "Other")
            .unwrap()
            .chat_id;
        assert!(db.parse_message_cursor(&other, &cursor).is_err());
        assert!(db.parse_message_cursor(&chat, "0").is_err());
        drop(db);
        fs::remove_dir_all(root.path().join("cache")).unwrap();
        let db = GatewayDb::open(root.path(), &work).unwrap();
        assert_eq!(db.parse_message_cursor(&chat, &cursor).unwrap(), 1);
        drop(db);
        // A restored older lineage explicitly receives a fresh source epoch.
        let path = root
            .path()
            .join("chats")
            .join(&chat)
            .join(".zork/source.json");
        let mut metadata: Value = serde_json::from_reader(fs::File::open(&path).unwrap()).unwrap();
        metadata["epoch"] = json!(ulid::Ulid::new().to_string());
        fs::write(path, serde_json::to_vec(&metadata).unwrap()).unwrap();
        let db = GatewayDb::open(root.path(), &work).unwrap();
        assert!(db.parse_message_cursor(&chat, &cursor).is_err());
        assert_eq!(
            db.chat_message(&chat, "message").unwrap().text,
            "完整正文\n```rs\nlet x = 1;\n```"
        );
    }

    #[test]
    fn failed_source_write_keeps_staging_private_until_storage_recovers() {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("workspaces");
        let db = GatewayDb::open(root.path(), &work).unwrap();
        let chat = chat(&db);
        let blocked = root
            .path()
            .join("chats")
            .join(&chat)
            .join("messages/00000000000000000001.jsonl");
        fs::create_dir(&blocked).unwrap();
        assert!(db
            .post_chat_content(
                None,
                "first",
                &chat,
                &author(),
                "keep complete pending publication",
                &[],
                None,
                &[],
                &[],
                None
            )
            .is_err());
        assert!(db.chat(&chat).is_err());
        assert!(db.chat_messages(&chat, None, 100).is_err());
        assert_eq!(
            db.conn
                .lock()
                .unwrap()
                .query_row(
                    "SELECT text FROM visible_messages WHERE archived=0",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "keep complete pending publication"
        );
        fs::remove_dir(blocked).unwrap();
        assert_eq!(db.chat(&chat).unwrap().channel.message_count, 1);
        assert_eq!(
            db.chat_message(&chat, "first").unwrap().text,
            "keep complete pending publication"
        );
        assert_eq!(db.message_log.positions(0, 100).unwrap(), vec![1]);
    }

    #[test]
    fn recovering_an_older_page_message_keeps_the_latest_page_projection() {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("workspaces");
        let db = GatewayDb::open(root.path(), &work).unwrap();
        let chat = chat(&db);
        for (id, title) in [("old", "Old title"), ("latest", "Latest title")] {
            let page = pages::page_link(title, "https://example.com/page", "description").unwrap();
            let mut writer = author();
            writer.name = Some(title.into());
            db.post_chat_content(
                None,
                id,
                &chat,
                &writer,
                title,
                &[],
                None,
                &[],
                &[zork_client_types::pages::DeliveredPage { page }],
                None,
            )
            .unwrap();
        }
        {
            let conn = db.conn.lock().unwrap();
            conn.execute_batch("DELETE FROM chat_notices WHERE message_id='old'; DELETE FROM chat_message_facts WHERE message_id='old'; DELETE FROM visible_messages WHERE message_id='old';").unwrap();
        }
        drop(db);
        let db = GatewayDb::open(root.path(), &work).unwrap();
        let conn = db.conn.lock().unwrap();
        assert_eq!(
            conn.query_row("SELECT message_id FROM conversation_pages", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "latest"
        );
        assert_eq!(
            db.message_log.get(1).unwrap().pages[0].page.title,
            "Old title"
        );
        assert_eq!(
            conn.query_row(
                "SELECT json_extract(author,'$.name') FROM chat_participants",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            "Latest title"
        );
    }

    #[test]
    #[ignore = "standalone 100000-message Station source and database page measurement"]
    fn hundred_thousand_source_records_use_indexed_public_page_reads() {
        use std::time::Instant;
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("workspaces");
        let db = GatewayDb::open(root.path(), &work).unwrap();
        let chat = chat(&db);
        let key = db.chat(&chat).unwrap().session_key;
        for start in (1..=100_000).step_by(128) {
            let records = (start..(start + 128).min(100_001))
                .map(|sequence| Record {
                    sequence,
                    session_key: key.clone(),
                    role: "assistant".into(),
                    kind: Some("message".into()),
                    pages: vec![],
                    message: Message {
                        client_id: None,
                        message_id: format!("m{sequence}"),
                        chat_id: chat.clone(),
                        author: author(),
                        text: format!(
                            "记录 {sequence}\n{}\n```rust\nlet record = {sequence};\n```",
                            "中英文消息及完整正文 mixed text\n".repeat(16)
                        ),
                        attachments: vec![],
                        mentions: vec![],
                        reply_to: None,
                        interaction: None,
                        created_at: "2026-09-14T00:00:00Z".into(),
                    },
                })
                .collect::<Vec<_>>();
            db.message_log.append(&records).unwrap();
        }
        let started = Instant::now();
        db.recover_message_projection(&db.conn.lock().unwrap())
            .unwrap();
        let projection_ms = started.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(db.chat(&chat).unwrap().channel.message_count, 100000);
        drop(db);
        let started = Instant::now();
        let db = GatewayDb::open(root.path(), &work).unwrap();
        let reopen_ms = started.elapsed().as_secs_f64() * 1000.0;
        let mut native = Vec::new();
        let mut agent = Vec::new();
        for index in 0..120 {
            let before = 101 + (index * 7919) % 99_900;
            let started = Instant::now();
            let rows = db.list_visible_messages(&key, Some(before), 100).unwrap();
            assert_eq!(rows.len(), 100);
            assert_eq!(rows.last().unwrap().sequence, before - 1);
            assert!(
                db.has_visible_messages_before(&key, rows[0].sequence)
                    .unwrap()
                    || rows[0].sequence == 1
            );
            native.push(started.elapsed().as_secs_f64() * 1000.0);
            let started = Instant::now();
            let rows = db.chat_messages(&chat, Some(before), 100).unwrap();
            assert_eq!(rows.len(), 100);
            assert_eq!(rows[0].0, before - 1);
            agent.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        let distribution = |mut values: Vec<f64>| {
            values.sort_by(f64::total_cmp);
            json!({"samples":values.len(),"p50_ms":values[59],"p95_ms":values[113],"p99_ms":values[118]})
        };
        println!(
            "{}",
            json!({"records":100000,"page_messages":100,"projection_rebuild_ms":projection_ms,"normal_reopen_ms":reopen_ms,
            "native_page":distribution(native),"agent_page":distribution(agent),"fixture":"Chinese, English and code; immutable log with reconstructed business message projections; warm OS cache; dev profile; no HTTP/UI rendering"})
        );
    }
}

pub(super) fn initialize(conn: &Connection) -> Result<()> {
    let has_column = conn
        .prepare("PRAGMA table_info(visible_messages)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .iter()
        .any(|name| name == "archived");
    if !has_column {
        conn.execute_batch(
            "ALTER TABLE visible_messages ADD COLUMN archived INTEGER NOT NULL DEFAULT 0;",
        )?;
    }
    conn.execute_batch("CREATE INDEX IF NOT EXISTS message_pending_source ON visible_messages(sequence) WHERE archived=0;
        CREATE VIEW IF NOT EXISTS visible_message_content AS SELECT sequence,message_id,session_key,
            connection_id,conversation_id,root_message_id,role,message_text(sequence,text,archived) AS text,
            kind,created_at FROM visible_messages;
        CREATE TRIGGER IF NOT EXISTS immutable_message_source BEFORE UPDATE ON visible_messages
        WHEN OLD.archived=1 AND (NEW.message_id IS NOT OLD.message_id OR NEW.sequence IS NOT OLD.sequence
            OR NEW.session_key IS NOT OLD.session_key OR NEW.role IS NOT OLD.role OR NEW.text!=''
            OR NEW.kind IS NOT OLD.kind OR NEW.created_at IS NOT OLD.created_at OR NEW.archived!=1)
        BEGIN SELECT RAISE(ABORT,'message source is immutable'); END;")?;
    Ok(())
}

impl GatewayDb {
    fn message_stream(&self, session: &str) -> Result<(String, String)> {
        let (key,chat): (String,Option<String>) = self.conn.lock().expect("db mutex").query_row(
            "SELECT s.key,COALESCE(c.chat_id,s.id) FROM sessions s LEFT JOIN chat_channels c ON c.session_key=s.key WHERE s.key=?1 OR s.id=?1 OR c.chat_id=?1",
            [session],|r|Ok((r.get(0)?,r.get(1)?))).context("chat_not_found")?;
        let chat = chat.unwrap_or_else(|| format!("im-{}", blake3::hash(key.as_bytes()).to_hex()));
        let epoch = self.message_log.epoch(&chat)?;
        Ok((chat, epoch))
    }
    pub fn chat_source_epoch(&self, session: &str) -> Result<String> {
        Ok(self.message_stream(session)?.1)
    }
    pub fn chat_source_tail(&self, session_key: &str) -> Result<(String, i64)> {
        let epoch = self.chat_source_epoch(session_key)?;
        let sequence = self.published_messages()?.query_row(
            "SELECT COALESCE(MAX(sequence),0) FROM visible_messages WHERE session_key=?1",
            [session_key],
            |row| row.get(0),
        )?;
        Ok((epoch, sequence))
    }
    pub fn message_cursor(&self, session: &str, sequence: i64) -> Result<String> {
        let (chat, epoch) = self.message_stream(session)?;
        Ok(format!("chat1:{chat}:{epoch}:{sequence}"))
    }
    pub fn parse_message_cursor(&self, session: &str, cursor: &str) -> Result<i64> {
        let sequence = if let Some(cursor) = cursor.strip_prefix("chat1:") {
            let (chat, epoch) = self.message_stream(session)?;
            let mut fields = cursor.split(':');
            anyhow::ensure!(
                fields.next() == Some(chat.as_str()) && fields.next() == Some(epoch.as_str()),
                "message_source_changed"
            );
            let sequence = fields
                .next()
                .context("invalid_message_cursor")?
                .parse::<i64>()?;
            anyhow::ensure!(fields.next().is_none(), "invalid_message_cursor");
            sequence
        } else {
            cursor.parse::<i64>()?
        };
        anyhow::ensure!(sequence > 0, "invalid_message_cursor");
        Ok(sequence)
    }

    pub(super) fn published_messages(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        let conn = self.conn.lock().expect("db mutex");
        self.flush_messages(&conn)?;
        Ok(conn)
    }

    pub(super) fn flush_messages(&self, conn: &Connection) -> Result<()> {
        anyhow::ensure!(
            conn.is_autocommit(),
            "message source requires a committed business transaction"
        );
        loop {
            let rows = conn
                .prepare(
                    "SELECT sequence,message_id,session_key,role,text,kind,created_at
                FROM visible_messages WHERE archived=0 ORDER BY sequence LIMIT 64",
                )?
                .query_map([], map_visible_message_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            if rows.is_empty() {
                return Ok(());
            }
            let records = rows
                .iter()
                .map(|row| {
                    if let Some(saved) = self.message_log.get_optional(row.sequence)? {
                        let (text, attachments) = zork_client_types::files::decode(&row.text)
                            .unwrap_or((row.text.clone(), vec![]));
                        anyhow::ensure!(
                            saved.session_key == row.session_key
                                && saved.message.message_id == row.message_id
                                && saved.role == row.role
                                && saved.kind == row.kind
                                && saved.message.created_at == row.created_at
                                && saved.message.text == text
                                && saved.message.attachments == attachments,
                            "pending publication conflicts with its immutable source"
                        );
                        // After a synced append, projections such as the latest
                        // page per URL can move forward. Recovery keeps the
                        // original complete source instead of reconstructing it
                        // from that newer projection.
                        return Ok(saved);
                    }
                    let chat: Option<String> = conn
                        .query_row(
                            "SELECT chat_id FROM chat_message_facts WHERE message_id=?1",
                            [&row.message_id],
                            |r| r.get(0),
                        )
                        .optional()?;
                    let message = if chat.is_some() {
                        chats::message(conn, &row.message_id)?
                    } else {
                        // Legacy IM bindings have a visible stream, without inventing
                        // a new Chat or exposing Agent execution history as Chat.
                        let id: Option<String> = conn.query_row(
                            "SELECT id FROM sessions WHERE key=?1",
                            [&row.session_key],
                            |r| r.get(0),
                        )?;
                        let (text, attachments) = zork_client_types::files::decode(&row.text)
                            .unwrap_or((row.text.clone(), vec![]));
                        Message {
                            message_id: row.message_id.clone(),
                            chat_id: id.unwrap_or_else(|| {
                                format!("im-{}", blake3::hash(row.session_key.as_bytes()).to_hex())
                            }),
                            author: chats::legacy_author(conn, row)?,
                            client_id: None,
                            text,
                            attachments,
                            mentions: vec![],
                            reply_to: None,
                            interaction: None,
                            created_at: row.created_at.clone(),
                        }
                    };
                    Ok(Record {
                        sequence: row.sequence,
                        session_key: row.session_key.clone(),
                        role: row.role.clone(),
                        kind: row.kind.clone(),
                        message,
                        pages: pages::message_pages(conn, &row.message_id)?,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            self.message_log.append(&records)?;
            let tx = conn.unchecked_transaction()?;
            for row in &rows {
                tx.execute("UPDATE visible_messages SET archived=1,text='' WHERE sequence=?1 AND archived=0",[row.sequence])?;
                tx.execute(
                    "UPDATE tool_interaction_messages SET value='' WHERE message_id=?1",
                    [&row.message_id],
                )?;
                // Old ordinary sends and business publication results may have
                // retained a full response. Keep only the immutable source ref.
                let receipts = tx.prepare("SELECT request_key,result FROM chat_receipts WHERE json_extract(result,'$.message_id')=?1")?
                    .query_map([&row.message_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                for (key, value) in receipts {
                    chats::finish(&tx, &key, &serde_json::from_str(&value)?)?;
                }
            }
            tx.commit()?;
        }
    }

    /// Rebuild missing message projections for existing bindings. Removing a
    /// binding remains a business decision; replay never recreates one.
    pub(super) fn recover_message_projection(&self, conn: &Connection) -> Result<()> {
        let mut after = 0;
        let mut repaired = std::collections::HashSet::new();
        loop {
            let positions = self.message_log.positions(after, 256)?;
            if positions.is_empty() {
                break;
            }
            let tx = conn.unchecked_transaction()?;
            for sequence in positions {
                after = sequence;
                let exists: bool = tx
                    .prepare_cached(
                        "SELECT EXISTS(SELECT 1 FROM visible_messages WHERE sequence=?1)",
                    )?
                    .query_row([sequence], |r| r.get(0))?;
                let facts: bool = tx.prepare_cached("SELECT EXISTS(SELECT 1 FROM chat_message_facts f JOIN visible_messages m ON m.message_id=f.message_id WHERE m.sequence=?1)")?.query_row([sequence],|r|r.get(0))?;
                if exists && facts {
                    continue;
                }
                let record = self.message_log.get(sequence)?;
                let active: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sessions WHERE key=?1)",
                    [&record.session_key],
                    |r| r.get(0),
                )?;
                if !active {
                    continue;
                }
                if !exists {
                    tx.execute("INSERT INTO visible_messages(sequence,message_id,session_key,connection_id,conversation_id,root_message_id,role,text,kind,created_at,archived)
                    SELECT ?1,?2,key,connection_id,channel_id,root_thread_ts,?4,'',?5,?6,1 FROM sessions WHERE key=?3",
                    params![sequence,record.message.message_id,record.session_key,record.role,record.kind,record.message.created_at])?;
                }
                let row = VisibleMessageRow {
                    sequence,
                    message_id: record.message.message_id.clone(),
                    session_key: record.session_key.clone(),
                    role: record.role.clone(),
                    text: record.text(),
                    kind: record.kind.clone(),
                    created_at: record.message.created_at.clone(),
                };
                chats::record_with_client(
                    &tx,
                    &row,
                    Some(&record.message.author),
                    record.message.reply_to.as_deref(),
                    &record.message.mentions,
                    false,
                    record.message.client_id.as_deref(),
                )?;
                repaired.insert(record.message.chat_id.clone());
                pages::record_pages(
                    &tx,
                    &record.session_key,
                    &record.message.message_id,
                    &record.pages,
                    &record.message.created_at,
                )?;
                if let Some(content) = &record.message.interaction {
                    let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM tool_interaction_messages WHERE message_id=?1)",[&record.message.message_id],|r|r.get(0))?;
                    if !exists {
                        chats::interactions::insert(
                            &tx,
                            &record.message.message_id,
                            &serde_json::from_value(content.clone())?,
                        )?;
                    }
                    tx.execute(
                        "UPDATE tool_interaction_messages SET value='' WHERE message_id=?1",
                        [&record.message.message_id],
                    )?;
                }
            }
            for chat in repaired.drain() {
                tx.execute("UPDATE chat_channels SET
            message_count=(SELECT COUNT(*) FROM chat_message_facts f WHERE f.chat_id=chat_channels.chat_id),
            last_message_at=(SELECT m.created_at FROM visible_messages m WHERE m.session_key=chat_channels.session_key ORDER BY m.sequence DESC LIMIT 1)
            WHERE chat_id=?1",[&chat])?;
                tx.execute("UPDATE chat_participants SET
            message_count=(SELECT COUNT(*) FROM chat_message_facts f WHERE f.chat_id=chat_participants.chat_id AND f.author_id=chat_participants.author_id),
            first_sequence=COALESCE((SELECT MIN(m.sequence) FROM visible_messages m JOIN chat_message_facts f ON f.message_id=m.message_id WHERE f.chat_id=chat_participants.chat_id AND f.author_id=chat_participants.author_id),first_sequence),
            author=COALESCE((SELECT f.author FROM visible_messages m JOIN chat_message_facts f ON f.message_id=m.message_id WHERE f.chat_id=chat_participants.chat_id AND f.author_id=chat_participants.author_id ORDER BY m.sequence DESC LIMIT 1),author)
            WHERE chat_id=?1",[&chat])?;
            }
            tx.commit()?;
        }
        // Source positions are never reused, including after deleting a binding.
        conn.execute("INSERT INTO sqlite_sequence(name,seq) SELECT 'visible_messages',?1 WHERE NOT EXISTS(SELECT 1 FROM sqlite_sequence WHERE name='visible_messages')",[after])?;
        conn.execute(
            "UPDATE sqlite_sequence SET seq=MAX(seq,?1) WHERE name='visible_messages'",
            [after],
        )?;
        Ok(())
    }
}
