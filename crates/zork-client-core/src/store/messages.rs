//! Mutable projections of the append-only delivered message source. Original
//! source positions remain intact even when results fold into earlier cards.
use super::ClientStore;
use crate::api::{MessagePage, TranscriptMessage};
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction};

const LOCAL_CURSOR: &str = "zork-cache:";
#[cfg(test)]
mod interaction_tests;

pub(super) fn initialize(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS message_history(
            node TEXT NOT NULL, session TEXT NOT NULL, older_cursor TEXT,
            PRIMARY KEY(node,session));
         CREATE TABLE IF NOT EXISTS messages(
            node TEXT NOT NULL, session TEXT NOT NULL, id TEXT NOT NULL,
            position INTEGER, value TEXT NOT NULL,
            request_id TEXT, status TEXT NOT NULL DEFAULT 'sent'
                CHECK(status IN ('sending','sent','failed')),
            attempted INTEGER NOT NULL DEFAULT 1, sent_at_ms INTEGER NOT NULL DEFAULT 0,
            error TEXT,
            PRIMARY KEY(node,session,id), UNIQUE(node,session,position), UNIQUE(node,request_id),
            CHECK((status='sent')=(position IS NOT NULL)));
         CREATE INDEX IF NOT EXISTS messages_dispatch ON messages(node,status,attempted);
         CREATE TABLE IF NOT EXISTS pending_business_card_results(
            node TEXT NOT NULL, session TEXT NOT NULL, request_id TEXT NOT NULL,
            value TEXT NOT NULL, PRIMARY KEY(node,session,request_id));",
    )?;
    let columns = tx
        .prepare("PRAGMA table_info(messages)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns.iter().any(|name| name == "links") {
        tx.execute_batch("ALTER TABLE messages ADD COLUMN links TEXT;")?;
    }
    tx.execute_batch("CREATE INDEX IF NOT EXISTS messages_links ON messages(node,session,position) WHERE links IS NOT NULL AND links!='[]';")?;
    let has_epoch = tx
        .prepare("PRAGMA table_info(message_history)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .iter()
        .any(|name| name == "source_epoch");
    if !has_epoch {
        tx.execute_batch("ALTER TABLE message_history ADD COLUMN source_epoch TEXT;")?;
    }
    let legacy: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='delivered_messages')",
        [], |row| row.get(0),
    )?;
    if legacy {
        tx.execute_batch(
            "INSERT INTO messages(node,session,id,position,value)
            SELECT node,session,id,position,value FROM delivered_messages;
            DROP TABLE delivered_messages;",
        )?;
    }
    Ok(())
}

fn identity(message: &TranscriptMessage) -> Result<&str> {
    let TranscriptMessage::Message { metadata, .. } = message;
    metadata
        .id
        .as_deref()
        .filter(|id| !id.is_empty())
        .context("delivered message has no identity")
}

pub(super) fn authorized(conn: &Connection, node: &str, generation: u64) -> Result<()> {
    ensure!(
        super::replica::binding_generation(conn, node)? == generation,
        "message cache binding changed"
    );
    let revoked: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM replica_bindings WHERE peer=?1 AND revoked=1)",
        [node],
        |r| r.get(0),
    )?;
    ensure!(!revoked, "message cache access revoked");
    Ok(())
}

fn bounds(conn: &Connection, node: &str, session: &str) -> Result<(Option<i64>, Option<i64>)> {
    Ok(conn.query_row(
        "SELECT
            (SELECT position FROM messages WHERE node=?1 AND session=?2 AND position IS NOT NULL ORDER BY position LIMIT 1),
            (SELECT position FROM messages WHERE node=?1 AND session=?2 AND position IS NOT NULL ORDER BY position DESC LIMIT 1)",
        params![node,session], |r| Ok((r.get(0)?,r.get(1)?)),
    )?)
}

/// Pages must describe a contiguous range: catch-up overlaps the cached tail;
/// older pages extend its head. Never invent an ordering inside an unknown gap.
fn insert_page(
    tx: &Transaction<'_>,
    node: &str,
    session: &str,
    page: &MessagePage,
    older: bool,
) -> Result<(bool, bool, Vec<crate::pages::ConversationPage>)> {
    let mut reset = false;
    let mut links = Vec::new();
    if let Some(epoch) = &page.source_epoch {
        ensure!(!epoch.is_empty(), "message source epoch missing");
        if let Some(previous) = source_epoch(tx, node, session)? {
            if previous != *epoch {
                ensure!(!older, "message source changed during pagination");
                tx.execute(
                    "DELETE FROM messages WHERE node=?1 AND session=?2 AND status='sent'",
                    params![node, session],
                )?;
                tx.execute(
                    "DELETE FROM pending_business_card_results WHERE node=?1 AND session=?2",
                    params![node, session],
                )?;
                tx.execute(
                    "DELETE FROM message_history WHERE node=?1 AND session=?2",
                    params![node, session],
                )?;
                reset = true;
            }
        }
        tx.execute("INSERT INTO message_history(node,session,source_epoch) VALUES(?1,?2,?3) ON CONFLICT(node,session) DO UPDATE SET source_epoch=excluded.source_epoch WHERE message_history.source_epoch IS NOT excluded.source_epoch",params![node,session,epoch])?;
    }
    let (min, max) = bounds(tx, node, session)?;
    let mut lookup =
        tx.prepare_cached("SELECT position FROM messages WHERE node=?1 AND session=?2 AND id=?3")?;
    let mut seen = std::collections::HashSet::new();
    let mut rows = Vec::new();
    let mut previous_source = None;
    for message in &page.items {
        let id = identity(message)?;
        let TranscriptMessage::Message { metadata, .. } = message;
        if let Some(sequence) = metadata.source_sequence {
            ensure!(
                sequence > 0 && previous_source.is_none_or(|previous| sequence > previous),
                "message source page order changed"
            );
            previous_source = Some(sequence);
        }
        if let (Some(epoch), Some(source)) = (&page.source_epoch, &metadata.source_epoch) {
            ensure!(epoch == source, "mixed message source epochs");
        }
        if !seen.insert(id) {
            continue;
        }
        let position: Option<i64> = lookup
            .query_row(params![node, session, id], |r| r.get::<_, Option<i64>>(0))
            .optional()?
            .flatten();
        rows.push((id, message, position));
    }
    let first = rows.iter().position(|(_, _, pos)| pos.is_some());
    let last = rows.iter().rposition(|(_, _, pos)| pos.is_some());
    if let (Some(first), Some(last)) = (first, last) {
        ensure!(
            rows[first..=last].iter().all(|(_, _, pos)| pos.is_some()),
            "message cache has an interior history gap"
        );
        ensure!(
            first == 0 || rows[first].2 == min,
            "history page does not overlap the cached head"
        );
        ensure!(
            last + 1 == rows.len() || rows[last].2 == max,
            "history page does not overlap the cached tail"
        );
    }
    let prefix = first.unwrap_or(if older { rows.len() } else { 0 });
    let mut head = min
        .unwrap_or(0)
        .checked_sub(prefix as i64)
        .context("message position overflow")?;
    let mut tail = max.unwrap_or(-1);
    let mut insert = tx.prepare_cached(
        "INSERT INTO messages(node,session,id,position,value) VALUES(?1,?2,?3,?4,?5)
         ON CONFLICT(node,session,id) DO UPDATE SET position=excluded.position,value=excluded.value,
            status='sent',attempted=1,error=NULL WHERE messages.position IS NULL",
    )?;
    let mut first_position = None;
    let mut received = false;
    for (index, (id, message, position)) in rows.iter().enumerate() {
        let position = match position {
            Some(position) => *position,
            None => {
                let position = if index < prefix {
                    let position = head;
                    head += 1;
                    position
                } else {
                    tail = tail.checked_add(1).context("message position overflow")?;
                    tail
                };
                received |= insert.execute(params![
                    node,
                    session,
                    id,
                    position,
                    serde_json::to_string(message)?
                ])? > 0;
                position
            }
        };
        enrich_source(tx, node, session, message)?;
        links.extend(index_links(tx, node, session, message)?);
        first_position.get_or_insert(position);
    }
    let interaction_changed = merge_interactions(tx, node, session, &page.items)?;
    let new_min = bounds(tx, node, session)?.0;
    if min.is_none()
        || (first_position.is_some() && first_position == new_min)
        || (older && rows.is_empty())
    {
        tx.execute("INSERT INTO message_history(node,session,older_cursor) VALUES(?1,?2,?3) ON CONFLICT(node,session) DO UPDATE SET older_cursor=excluded.older_cursor WHERE older_cursor IS NOT excluded.older_cursor",
            params![node,session,page.older_cursor])?;
    }
    Ok((interaction_changed || received || reset, reset, links))
}

fn migrate(tx: &Transaction<'_>, node: &str, session: &str) -> Result<()> {
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM message_history WHERE node=?1 AND session=?2)",
        params![node, session],
        |r| r.get(0),
    )?;
    if exists {
        return Ok(());
    }
    let key = format!("messages:{session}");
    let http_key = format!("http:/v1/im/sessions/{session}/messages");
    let raw: Option<String> = tx
        .query_row(
            "SELECT value FROM cache WHERE node=?1 AND key=?2",
            params![node, key],
            |r| r.get(0),
        )
        .optional()?;
    let page = match raw {
        Some(raw) => Some(serde_json::from_str::<MessagePage>(&raw)?),
        None => {
            let raw: Option<String> = tx
                .query_row(
                    "SELECT value FROM cache WHERE node=?1 AND key=?2",
                    params![node, http_key],
                    |r| r.get(0),
                )
                .optional()?;
            raw.map(|raw| {
                let value: serde_json::Value = serde_json::from_str(&raw)?;
                Ok::<_, anyhow::Error>(serde_json::from_value::<MessagePage>(
                    value["body"].clone(),
                )?)
            })
            .transpose()?
        }
    };
    if let Some(mut page) = page {
        // Do not discard an old payload that cannot be migrated safely.
        for message in &page.items {
            identity(message)?;
        }
        let mut statement = tx.prepare(
            "SELECT request_id FROM messages WHERE node=?1 AND session=?2 AND status!='sent'",
        )?;
        let pending = statement
            .query_map(params![node, session], |r| r.get::<_, String>(0))?
            .map(|id| id.map(|id| format!("client-{session}-{id}")))
            .collect::<rusqlite::Result<std::collections::HashSet<_>>>()?;
        page.items
            .retain(|message| identity(message).is_ok_and(|id| !pending.contains(id)));
        insert_page(tx, node, session, &page, false)?;
        tx.execute(
            "DELETE FROM cache WHERE node=?1 AND key IN (?2,?3)",
            params![node, key, http_key],
        )?;
    }
    Ok(())
}

impl ClientStore {
    /// Store only newly delivered identities, atomically with the history boundary.
    pub fn cache_message_page(
        &self,
        node: &str,
        session: &str,
        page: &MessagePage,
        before: Option<&str>,
    ) -> Result<()> {
        self.cache_message_page_at(node, session, page, before, self.replica_generation(node)?)
    }

    pub(crate) fn cache_message_page_at(
        &self,
        node: &str,
        session: &str,
        page: &MessagePage,
        before: Option<&str>,
        generation: u64,
    ) -> Result<()> {
        let mut conn = self.0.lock().expect("client database");
        let tx = conn.transaction()?;
        authorized(&tx, node, generation)?;
        migrate(&tx, node, session)?;
        if let Some(before) = before {
            let cursor: Option<String> = tx
                .query_row(
                    "SELECT older_cursor FROM message_history WHERE node=?1 AND session=?2",
                    params![node, session],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            // Another reader may already have extended this range.
            if cursor.as_deref() != Some(before) {
                return Ok(());
            }
        }
        let (changed, reset, links) = insert_page(&tx, node, session, page, before.is_some())?;
        tx.commit()?;
        drop(conn);
        self.publish_message_links(node, session, reset, links);
        if changed {
            self.delivery_changed();
        }
        Ok(())
    }

    pub(crate) fn cache_delivered_message(
        &self,
        node: &str,
        session: &str,
        message: &TranscriptMessage,
        generation: u64,
    ) -> Result<()> {
        let mut conn = self.0.lock().expect("client database");
        let tx = conn.transaction()?;
        authorized(&tx, node, generation)?;
        migrate(&tx, node, session)?;
        let id = identity(message)?;
        let TranscriptMessage::Message { metadata, .. } = message;
        if let Some(epoch) = &metadata.source_epoch {
            ensure!(
                source_epoch(&tx, node, session)?.is_none_or(|saved| saved == *epoch),
                "message source changed"
            );
            tx.execute("INSERT INTO message_history(node,session,source_epoch) VALUES(?1,?2,?3) ON CONFLICT(node,session) DO UPDATE SET source_epoch=excluded.source_epoch WHERE message_history.source_epoch IS NOT excluded.source_epoch",params![node,session,epoch])?;
        }
        // A replay cannot replace an already merged card with its initial request.
        let received = tx.execute("INSERT INTO messages(node,session,id,position,value)
            SELECT ?1,?2,?3,COALESCE(MAX(position),-1)+1,?4 FROM messages WHERE node=?1 AND session=?2
            ON CONFLICT(node,session,id) DO UPDATE SET position=excluded.position,value=excluded.value,
                status='sent',attempted=1,error=NULL WHERE messages.position IS NULL",
            params![node,session,id,serde_json::to_string(message)?])? > 0;
        enrich_source(&tx, node, session, message)?;
        let links = index_links(&tx, node, session, message)?;
        let changed =
            merge_interactions(&tx, node, session, std::slice::from_ref(message))? || received;
        tx.commit()?;
        drop(conn);
        self.publish_message_links(node, session, false, links);
        if changed {
            self.delivery_changed();
        }
        Ok(())
    }

    /// Read only the card records touched by this incoming batch. Results keep
    /// their original source rows, so history cursors never follow display folds.
    pub(crate) fn cached_interaction_updates(
        &self,
        node: &str,
        session: &str,
        items: &[TranscriptMessage],
        generation: u64,
    ) -> Result<Vec<TranscriptMessage>> {
        let ids = interaction_roots(items);
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let conn = self.0.lock().expect("client database");
        authorized(&conn, node, generation)?;
        ids.iter()
            .filter_map(|id| match cached_message(&conn, node, session, id) {
                Ok(Some(message)) => Some(Ok(message)),
                Ok(None) => None,
                Err(error) => Some(Err(error)),
            })
            .collect()
    }

    pub(crate) fn cached_message_at(
        &self,
        node: &str,
        session: &str,
        id: &str,
        generation: u64,
    ) -> Result<Option<TranscriptMessage>> {
        let conn = self.0.lock().expect("client database");
        authorized(&conn, node, generation)?;
        cached_message(&conn, node, session, id)
    }

    /// A local cursor is private to this store and must never reach the Gateway.
    pub fn cached_messages(
        &self,
        node: &str,
        session: &str,
        before: Option<&str>,
        limit: usize,
    ) -> Result<Option<MessagePage>> {
        self.cached_messages_at(node, session, before, limit, self.replica_generation(node)?)
    }

    pub(crate) fn cached_messages_at(
        &self,
        node: &str,
        session: &str,
        before: Option<&str>,
        limit: usize,
        generation: u64,
    ) -> Result<Option<MessagePage>> {
        ensure!(
            limit > 0 && limit < i64::MAX as usize,
            "invalid cached message limit"
        );
        let mut cursor_epoch = None;
        let position = match before {
            Some(cursor) => match cursor.strip_prefix(LOCAL_CURSOR) {
                Some(position) => {
                    let position = if let Some((epoch, position)) = position.rsplit_once(':') {
                        cursor_epoch = Some(epoch);
                        position
                    } else {
                        position
                    };
                    Some(position.parse::<i64>()?)
                }
                None => return Ok(None),
            },
            None => None,
        };
        let mut conn = self.0.lock().expect("client database");
        let tx = conn.transaction()?;
        authorized(&tx, node, generation)?;
        migrate(&tx, node, session)?;
        let cursor: Option<Option<String>> = tx
            .query_row(
                "SELECT older_cursor FROM message_history WHERE node=?1 AND session=?2",
                params![node, session],
                |r| r.get(0),
            )
            .optional()?;
        let Some(remote_cursor) = cursor else {
            return Ok(None);
        };
        let source_epoch = source_epoch(&tx, node, session)?;
        ensure!(
            cursor_epoch.is_none() || cursor_epoch == source_epoch.as_deref(),
            "cached message source changed"
        );
        let mut statement = tx.prepare_cached("SELECT position,value FROM messages WHERE node=?1 AND session=?2 AND position < ?3 ORDER BY position DESC LIMIT ?4")?;
        let mut rows = statement
            .query_map(
                params![
                    node,
                    session,
                    position.unwrap_or(i64::MAX),
                    (limit + 1) as i64
                ],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let older_cursor = if rows.len() > limit {
            rows.pop();
            Some(local_cursor(
                source_epoch.as_deref(),
                rows.last().unwrap().0,
            ))
        } else {
            remote_cursor
        };
        let items: Vec<TranscriptMessage> = rows
            .into_iter()
            .rev()
            .map(|(_, value)| serde_json::from_str(&value))
            .collect::<serde_json::Result<_>>()?;
        drop(statement);
        let mut links = Vec::new();
        for message in &items {
            index_links(&tx, node, session, message)?;
            let saved: String = tx.query_row(
                "SELECT links FROM messages WHERE node=?1 AND session=?2 AND id=?3",
                params![node, session, identity(message)?],
                |row| row.get(0),
            )?;
            links.extend(serde_json::from_str::<Vec<crate::pages::ConversationPage>>(
                &saved,
            )?);
        }
        tx.commit()?;
        drop(conn);
        self.publish_message_links(node, session, false, links);
        Ok(Some(MessagePage {
            source_epoch,
            items,
            older_cursor,
        }))
    }

    /// Extend a reader's consumed source range without moving its head forward
    /// when an older page and a live refresh finish in a different order.
    pub(crate) fn cached_history_boundary(
        &self,
        node: &str,
        session: &str,
        head: Option<&str>,
        candidate: Option<&str>,
        generation: u64,
    ) -> Result<(Option<String>, Option<String>)> {
        let conn = self.0.lock().expect("client database");
        authorized(&conn, node, generation)?;
        let oldest: Option<(String, i64)> = conn.query_row(
            "SELECT id,position FROM messages WHERE node=?1 AND session=?2 AND position IS NOT NULL AND id IN (?3,?4) ORDER BY position LIMIT 1",
            params![node, session, head, candidate],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ).optional()?;
        ensure!(
            oldest.is_some() || (head.is_none() && candidate.is_none()),
            "message history head missing"
        );
        if let Some((id, position)) = &oldest {
            let older: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM messages WHERE node=?1 AND session=?2 AND position<?3)",params![node,session,position],|r|r.get(0))?;
            if older {
                return Ok((
                    Some(id.clone()),
                    Some(local_cursor(
                        source_epoch(&conn, node, session)?.as_deref(),
                        *position,
                    )),
                ));
            }
        }
        let cursor = conn
            .query_row(
                "SELECT older_cursor FROM message_history WHERE node=?1 AND session=?2",
                params![node, session],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        Ok((oldest.map(|(id, _)| id), cursor))
    }
}

fn cached_message(
    conn: &Connection,
    node: &str,
    session: &str,
    id: &str,
) -> Result<Option<TranscriptMessage>> {
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM messages WHERE node=?1 AND session=?2 AND id=?3",
            params![node, session, id],
            |r| r.get(0),
        )
        .optional()?;
    value
        .map(|v| serde_json::from_str(&v).map_err(Into::into))
        .transpose()
}

fn interaction_roots(items: &[TranscriptMessage]) -> std::collections::BTreeSet<String> {
    items
        .iter()
        .filter_map(|message| {
            let TranscriptMessage::Message { metadata, .. } = message;
            if crate::interactions::request(metadata).is_some() {
                metadata.id.clone()
            } else {
                crate::interactions::result(metadata).map(|r| r.request_message_id)
            }
        })
        .collect()
}

fn merge_interactions(
    tx: &Transaction<'_>,
    node: &str,
    session: &str,
    items: &[TranscriptMessage],
) -> Result<bool> {
    use crate::interactions;
    let mut changed = false;
    for message in items {
        let TranscriptMessage::Message { metadata, .. } = message;
        if let Some(incoming) = interactions::result_event(metadata) {
            if let Some(mut target) =
                cached_message(tx, node, session, &incoming.result.request_message_id)?
            {
                let TranscriptMessage::Message { metadata, .. } = &mut target;
                if interactions::merge_result(metadata, &incoming)? {
                    tx.execute(
                        "UPDATE messages SET value=?4 WHERE node=?1 AND session=?2 AND id=?3",
                        params![
                            node,
                            session,
                            incoming.result.request_message_id,
                            serde_json::to_string(&target)?
                        ],
                    )?;
                    changed = true;
                }
                tx.execute("DELETE FROM pending_business_card_results WHERE node=?1 AND session=?2 AND request_id=?3", params![node, session, incoming.result.request_message_id])?;
            } else {
                let old: Option<String> = tx.query_row("SELECT value FROM pending_business_card_results WHERE node=?1 AND session=?2 AND request_id=?3", params![node, session, incoming.result.request_message_id], |r| r.get(0)).optional()?;
                let old = old
                    .map(|s| serde_json::from_str::<interactions::ResultEvent>(&s))
                    .transpose()?;
                if let Some(old) = &old {
                    ensure!(
                        old.handler == incoming.handler && old.request_id == incoming.request_id,
                        "interaction_business_mismatch"
                    );
                    if old.result.revision == incoming.result.revision {
                        ensure!(old == &incoming, "interaction_result_conflict");
                    }
                }
                if old
                    .as_ref()
                    .is_none_or(|old| old.result.revision < incoming.result.revision)
                {
                    tx.execute("INSERT INTO pending_business_card_results VALUES(?1,?2,?3,?4) ON CONFLICT(node,session,request_id) DO UPDATE SET value=excluded.value", params![node, session, incoming.result.request_message_id, serde_json::to_string(&incoming)?])?;
                    changed = true;
                }
            }
            if incoming.handler == interactions::AGENT_CONFIGURATION {
                changed |= tx.execute(
                "DELETE FROM agent_configuration_outbox WHERE node=?1 AND session=?2 AND message_id=?3",
                params![node, session, incoming.result.request_message_id],
            )? > 0;
            }
        }
    }
    for message in items {
        let TranscriptMessage::Message { metadata, .. } = message;
        if interactions::request(metadata).is_none() {
            continue;
        }
        let id = identity(message)?;
        let pending: Option<String> = tx.query_row("SELECT value FROM pending_business_card_results WHERE node=?1 AND session=?2 AND request_id=?3", params![node, session, id], |r| r.get(0)).optional()?;
        if let Some(pending) = pending {
            let result = serde_json::from_str(&pending)?;
            let mut target =
                cached_message(tx, node, session, id)?.context("interaction_request_missing")?;
            let TranscriptMessage::Message { metadata, .. } = &mut target;
            if interactions::merge_result(metadata, &result)? {
                tx.execute(
                    "UPDATE messages SET value=?4 WHERE node=?1 AND session=?2 AND id=?3",
                    params![node, session, id, serde_json::to_string(&target)?],
                )?;
                changed = true;
            }
            tx.execute("DELETE FROM pending_business_card_results WHERE node=?1 AND session=?2 AND request_id=?3", params![node, session, id])?;
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_epoch_reset_preserves_pending_messages_and_rejects_old_source_frames() {
        let root = tempfile::tempdir().unwrap();
        let store = ClientStore::open(root.path()).unwrap();
        let source = |epoch: &str, id: &str, sequence: i64| {
            serde_json::from_value(serde_json::json!({
            "type":"message","id":id,"role":"user","content":"body","source_epoch":epoch,"source_sequence":sequence,
        })).unwrap()
        };
        let page = MessagePage {
            source_epoch: Some("first-source".into()),
            items: vec![
                source("first-source", "one", 1),
                source("first-source", "four", 4),
            ],
            older_cursor: None,
        };
        store
            .cache_message_page("node", "chat", &page, None)
            .unwrap();
        let observer = Connection::open(root.path().join("client.db")).unwrap();
        let version: u64 = observer
            .query_row("PRAGMA data_version", [], |row| row.get(0))
            .unwrap();
        store
            .cache_message_page("node", "chat", &page, None)
            .unwrap();
        store
            .cache_delivered_message("node", "chat", &page.items[1], 0)
            .unwrap();
        assert_eq!(
            observer
                .query_row("PRAGMA data_version", [], |row| row.get::<_, u64>(0))
                .unwrap(),
            version
        );
        let old_cursor = store
            .cached_messages("node", "chat", None, 1)
            .unwrap()
            .unwrap()
            .older_cursor
            .unwrap();
        store
            .enqueue(
                "node",
                &super::super::QueuedMessage {
                    request_id: "pending".into(),
                    session_id: "chat".into(),
                    content: "still local".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        store
            .cache_message_page(
                "node",
                "chat",
                &MessagePage {
                    source_epoch: Some("restored-source".into()),
                    items: vec![],
                    older_cursor: None,
                },
                None,
            )
            .unwrap();
        let saved = store
            .cached_messages("node", "chat", None, 100)
            .unwrap()
            .unwrap();
        assert!(saved.items.is_empty());
        assert_eq!(saved.source_epoch.as_deref(), Some("restored-source"));
        assert_eq!(store.outbox("node").unwrap().len(), 1);
        assert!(store
            .cached_messages("node", "chat", Some(&old_cursor), 100)
            .is_err());
        assert!(store
            .cache_delivered_message(
                "node",
                "chat",
                &source("first-source", "client-chat-pending", 5),
                0
            )
            .is_err());
        assert_eq!(
            store.outbox("node").unwrap()[0].delivery_status(),
            "sending"
        );
        store
            .cache_delivered_message(
                "node",
                "chat",
                &source("restored-source", "client-chat-pending", 1),
                0,
            )
            .unwrap();
        assert!(store.outbox("node").unwrap().is_empty());
        assert_eq!(
            store
                .cached_messages("node", "chat", None, 100)
                .unwrap()
                .unwrap()
                .items
                .len(),
            1
        );
    }
    use crate::api::{MessageMetadata, Role};

    fn page(ids: &[&str], cursor: Option<&str>) -> MessagePage {
        MessagePage {
            source_epoch: None,
            items: ids
                .iter()
                .map(|id| TranscriptMessage::Message {
                    role: Role::Assistant,
                    content: format!("message {id}"),
                    metadata: MessageMetadata {
                        id: Some((*id).into()),
                        ..Default::default()
                    },
                })
                .collect(),
            older_cursor: cursor.map(str::to_owned),
        }
    }
    fn ids(page: &MessagePage) -> Vec<&str> {
        page.items
            .iter()
            .map(|message| identity(message).unwrap())
            .collect()
    }

    #[test]
    fn multiple_chats_survive_restart_and_page_locally_in_gateway_order() {
        let directory = tempfile::tempdir().unwrap();
        let store = ClientStore::open(directory.path()).unwrap();
        // IDs deliberately sort differently from gateway history order.
        store
            .cache_message_page("node", "a", &page(&["z", "a", "m"], Some("remote")), None)
            .unwrap();
        store
            .cache_message_page("node", "b", &page(&["m", "z"], None), None)
            .unwrap();
        store
            .cache_message_page("other-node", "a", &page(&["other"], None), None)
            .unwrap();
        store
            .cache_message_page("node", "a", &page(&["old2", "old1"], None), Some("remote"))
            .unwrap();
        store
            .cache_message_page(
                "node",
                "a",
                &page(&["a", "m", "new"], Some("irrelevant")),
                None,
            )
            .unwrap();
        drop(store);
        let store = ClientStore::open(directory.path()).unwrap();
        let tail = store
            .cached_messages("node", "a", None, 2)
            .unwrap()
            .unwrap();
        assert_eq!(ids(&tail), ["m", "new"]);
        let middle = store
            .cached_messages("node", "a", tail.older_cursor.as_deref(), 2)
            .unwrap()
            .unwrap();
        assert_eq!(ids(&middle), ["z", "a"]);
        let oldest = store
            .cached_messages("node", "a", middle.older_cursor.as_deref(), 2)
            .unwrap()
            .unwrap();
        assert_eq!(ids(&oldest), ["old2", "old1"]);
        assert!(oldest.older_cursor.is_none());
        assert_eq!(
            ids(&store
                .cached_messages("node", "b", None, 100)
                .unwrap()
                .unwrap()),
            ["m", "z"]
        );
        assert_eq!(
            ids(&store
                .cached_messages("other-node", "a", None, 100)
                .unwrap()
                .unwrap()),
            ["other"]
        );
    }

    #[test]
    fn replay_does_not_rewrite_messages_or_regress_the_history_boundary() {
        let directory = tempfile::tempdir().unwrap();
        let store = ClientStore::open(directory.path()).unwrap();
        let initial = page(&["m2", "m3"], Some("before2"));
        store
            .cache_message_page("node", "chat", &initial, None)
            .unwrap();
        store
            .cache_message_page(
                "node",
                "chat",
                &page(&["m1", "m2"], Some("before1")),
                Some("before2"),
            )
            .unwrap();
        let changes = store
            .0
            .lock()
            .unwrap()
            .query_row::<u64, _, _>("SELECT total_changes()", [], |r| r.get(0))
            .unwrap();
        store
            .cache_message_page("node", "chat", &initial, None)
            .unwrap();
        store
            .cache_delivered_message("node", "chat", &initial.items[1], 0)
            .unwrap();
        assert_eq!(
            store
                .0
                .lock()
                .unwrap()
                .query_row::<u64, _, _>("SELECT total_changes()", [], |r| r.get(0))
                .unwrap(),
            changes
        );
        let cached = store
            .cached_messages("node", "chat", None, 100)
            .unwrap()
            .unwrap();
        assert_eq!(ids(&cached), ["m1", "m2", "m3"]);
        assert_eq!(cached.older_cursor.as_deref(), Some("before1"));
        assert!(store
            .cached_messages("node", "chat", Some("before1"), 100)
            .unwrap()
            .is_none());
        // Even a conflicting duplicate cannot overwrite the delivered payload.
        let mut duplicate = initial.items[0].clone();
        let TranscriptMessage::Message { content, .. } = &mut duplicate;
        *content = "changed".into();
        store
            .cache_delivered_message("node", "chat", &duplicate, 0)
            .unwrap();
        assert_eq!(
            store
                .cached_messages("node", "chat", None, 100)
                .unwrap()
                .unwrap(),
            cached
        );
    }

    #[test]
    fn migration_preserves_pending_rows_in_the_message_table() {
        let directory = tempfile::tempdir().unwrap();
        let store = ClientStore::open(directory.path()).unwrap();
        let pending = super::super::QueuedMessage {
            request_id: "request".into(),
            session_id: "chat".into(),
            content: "unsent".into(),
            attempted: false,
            sent_at_ms: 0,
            error: None,
        };
        store.enqueue("node", &pending).unwrap();
        store
            .put(
                "node",
                "messages:chat",
                &page(&["old", "client-chat-request"], Some("remote")),
            )
            .unwrap();
        store
            .put(
                "node",
                "http:/v1/im/sessions/http/messages",
                &serde_json::json!({"body":page(&["http-old"],None)}),
            )
            .unwrap();
        let migrated = store
            .cached_messages("node", "chat", None, 100)
            .unwrap()
            .unwrap();
        assert_eq!(ids(&migrated), ["old"]);
        assert_eq!(migrated.older_cursor.as_deref(), Some("remote"));
        assert_eq!(store.outbox("node").unwrap(), [pending]);
        assert!(store
            .get::<MessagePage>("node", "messages:chat")
            .unwrap()
            .is_none());
        assert_eq!(
            ids(&store
                .cached_messages("node", "http", None, 100)
                .unwrap()
                .unwrap()),
            ["http-old"]
        );

        let mut invalid = page(&["valid", "invalid"], None);
        let TranscriptMessage::Message { metadata, .. } = &mut invalid.items[1];
        metadata.id = None;
        store.put("node", "messages:invalid", &invalid).unwrap();
        assert!(store.cached_messages("node", "invalid", None, 100).is_err());
        assert_eq!(
            store
                .get::<MessagePage>("node", "messages:invalid")
                .unwrap(),
            Some(invalid)
        );
        assert_eq!(
            bounds(&store.0.lock().unwrap(), "node", "invalid").unwrap(),
            (None, None)
        );
    }

    #[test]
    fn revoked_history_is_deleted_and_stale_writers_cannot_restore_it() {
        let directory = tempfile::tempdir().unwrap();
        let store = ClientStore::open(directory.path()).unwrap();
        let initial = page(&["m1"], None);
        store
            .cache_message_page("node", "chat", &initial, None)
            .unwrap();
        store
            .cache_message_page("other", "chat", &initial, None)
            .unwrap();
        store.revoke_replica("node").unwrap();
        assert!(store.cached_messages("node", "chat", None, 100).is_err());
        assert!(store
            .cache_message_page("node", "chat", &initial, None)
            .is_err());
        assert_eq!(
            bounds(&store.0.lock().unwrap(), "node", "chat").unwrap(),
            (None, None)
        );
        assert!(store
            .cached_messages("other", "chat", None, 100)
            .unwrap()
            .is_some());
        store
            .0
            .lock()
            .unwrap()
            .execute(
                "UPDATE replica_bindings SET revoked=0 WHERE peer='node'",
                [],
            )
            .unwrap();
        assert!(store
            .cache_message_page_at("node", "chat", &initial, None, 0)
            .is_err());
        assert!(store
            .cache_delivered_message("node", "chat", &initial.items[0], 0)
            .is_err());
        assert!(store
            .cached_messages("node", "chat", None, 100)
            .unwrap()
            .is_none());
        store
            .cache_message_page("node", "chat", &initial, None)
            .unwrap();
    }

    #[test]
    fn rejects_interior_gaps_without_publishing_partial_rows() {
        let directory = tempfile::tempdir().unwrap();
        let store = ClientStore::open(directory.path()).unwrap();
        let initial = page(&["a", "c"], Some("remote"));
        store
            .cache_message_page("node", "chat", &initial, None)
            .unwrap();
        assert!(store
            .cache_message_page("node", "chat", &page(&["older", "a", "b", "c"], None), None)
            .is_err());
        assert_eq!(
            store
                .cached_messages("node", "chat", None, 100)
                .unwrap()
                .unwrap(),
            initial
        );
    }
}

fn source_epoch(conn: &Connection, node: &str, session: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT source_epoch FROM message_history WHERE node=?1 AND session=?2",
            params![node, session],
            |r| r.get(0),
        )
        .optional()?
        .flatten())
}
fn local_cursor(epoch: Option<&str>, position: i64) -> String {
    match epoch {
        Some(epoch) => format!("{LOCAL_CURSOR}{epoch}:{position}"),
        None => format!("{LOCAL_CURSOR}{position}"),
    }
}

fn enrich_source(
    conn: &Connection,
    node: &str,
    session: &str,
    message: &TranscriptMessage,
) -> Result<()> {
    let TranscriptMessage::Message { metadata, .. } = message;
    if let (Some(epoch), Some(sequence)) = (&metadata.source_epoch, metadata.source_sequence) {
        ensure!(sequence > 0, "invalid message source position");
        conn.execute("UPDATE messages SET value=json_set(value,'$.source_epoch',?4,'$.source_sequence',?5)
            WHERE node=?1 AND session=?2 AND id=?3 AND json_extract(value,'$.source_sequence') IS NULL",
            params![node,session,identity(message)?,epoch,sequence])?;
    }
    Ok(())
}

fn index_links(
    conn: &Connection,
    node: &str,
    session: &str,
    message: &TranscriptMessage,
) -> Result<Vec<crate::pages::ConversationPage>> {
    let id = identity(message)?;
    let needed: bool = conn.query_row(
        "SELECT links IS NULL FROM messages WHERE node=?1 AND session=?2 AND id=?3",
        params![node, session, id],
        |row| row.get(0),
    )?;
    if needed {
        let links = crate::pages::message_links(session, message);
        conn.execute(
            "UPDATE messages SET links=?4 WHERE node=?1 AND session=?2 AND id=?3 AND links IS NULL",
            params![node, session, id, serde_json::to_string(&links)?],
        )?;
        return Ok(links);
    }
    Ok(Vec::new())
}
impl ClientStore {
    fn publish_message_links(
        &self,
        node: &str,
        session: &str,
        reset: bool,
        links: Vec<crate::pages::ConversationPage>,
    ) {
        if !reset && links.is_empty() {
            return;
        }
        let device = self
            .1
            .lock()
            .unwrap()
            .get(node)
            .and_then(std::sync::Weak::upgrade);
        if let Some(device) = device {
            device.apply_message_links(session, reset, links);
        }
    }
    pub(crate) fn message_links(&self, node: &str) -> Result<Vec<crate::pages::ConversationPage>> {
        let conn = self.0.lock().expect("client database");
        let mut query = conn.prepare("SELECT links FROM messages WHERE node=?1 AND status='sent' AND links IS NOT NULL AND links!='[]' ORDER BY session,position")?;
        let mut links = std::collections::BTreeMap::new();
        for raw in query.query_map([node], |row| row.get::<_, String>(0))? {
            for page in serde_json::from_str::<Vec<crate::pages::ConversationPage>>(&raw?)? {
                links.insert(page.id.clone(), page);
            }
        }
        Ok(links.into_values().collect())
    }
}
