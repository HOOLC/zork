//! Public navigation metadata; creation provenance never follows subscriptions.
use super::*;

pub(super) fn initialize(conn: &Connection) -> Result<()> {
    let columns = conn
        .prepare("PRAGMA table_info(chat_channels)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (name, definition) in [("creator", "TEXT"), ("last_message_at", "TEXT")] {
        if !columns.iter().any(|column| column == name) {
            conn.execute_batch(&format!(
                "ALTER TABLE chat_channels ADD COLUMN {name} {definition};"
            ))?;
        }
    }
    Ok(())
}

pub(super) fn migrate_creators(conn: &Connection) -> Result<()> {
    if conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM chat_metadata WHERE key='navigation-v1')",
        [],
        |row| row.get::<_, bool>(0),
    )? {
        return Ok(());
    }
    // Only legacy assignment records establish a creator. A first author,
    // receiver or home association is not evidence of creation.
    conn.execute_batch(
        "SAVEPOINT chat_navigation_migration;
         UPDATE chat_channels SET creator=(
           SELECT json_object('id',w.leader_id,'kind','agent','name',json_extract(a.value,'$.name'))
           FROM worker_tasks w LEFT JOIN node_agents a ON a.id=w.leader_id
           WHERE w.session_key=chat_channels.session_key)
         WHERE creator IS NULL AND EXISTS(SELECT 1 FROM worker_tasks w WHERE w.session_key=chat_channels.session_key);
         UPDATE chat_channels SET last_message_at=(SELECT created_at FROM visible_messages
           WHERE session_key=chat_channels.session_key ORDER BY sequence DESC LIMIT 1)
         WHERE last_message_at IS NULL;",
    )?;
    let keys = conn
        .prepare("SELECT session_key FROM chat_channels WHERE title='Chat' OR title=''")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for key in keys {
        set_legacy_title(conn, &key)?;
    }
    conn.execute_batch("INSERT INTO chat_metadata(key,value) VALUES('navigation-v1','1'); RELEASE chat_navigation_migration;")?;
    Ok(())
}

pub(super) fn set_legacy_title(conn: &Connection, key: &str) -> Result<()> {
    let title: Option<String> = conn.query_row(
        "SELECT COALESCE(
        (SELECT NULLIF(title,'') FROM product_tasks WHERE session_key=?1),
        (SELECT NULLIF(goal,'') FROM worker_tasks WHERE session_key=?1))",
        [key],
        |r| r.get(0),
    )?;
    if let Some(title) = title {
        let title = title
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(160)
            .collect::<String>();
        conn.execute(
            "UPDATE chat_channels SET title=?2 WHERE session_key=?1 AND (title='Chat' OR title='')",
            params![key, title],
        )?;
    }
    Ok(())
}

impl GatewayDb {
    pub fn chat_navigation(&self) -> Result<Vec<Channel>> {
        let conn = self.conn.lock().expect("db mutex");
        let rows = conn
            .prepare(&format!(
                "{CHANNEL_SELECT} ORDER BY created_at DESC,chat_id"
            ))?
            .query_map([], map_channel)?
            .map(|row| row.map(|row| row.channel))
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}
