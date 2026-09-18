//! Immutable card payloads and optional business-result references. Local script
//! cards have no result root or cross-client execution record.
use super::*;
use zork_client_types::interaction::{Content, MessageContent};

pub(super) fn content(conn: &Connection, id: &str) -> Result<Option<Value>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT CASE WHEN m.archived=1 THEN json_extract(message_value(m.sequence),'$.interaction') ELSE i.value END FROM tool_interaction_messages i JOIN visible_messages m ON m.message_id=i.message_id WHERE i.message_id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    raw.map(|v| serde_json::from_str(&v).map_err(Into::into))
        .transpose()
}

pub(in crate::db) fn insert(conn: &Connection, id: &str, content: &Value) -> Result<()> {
    let business = MessageContent::parse(content);
    let (root, revision) = if let Some(business) = &business {
        match &business.content {
            Content::Request { request } => {
                request.validate().map_err(anyhow::Error::msg)?;
                (None, 0)
            }
            Content::Result { result } => (Some(result.request_message_id.as_str()), result.revision),
        }
    } else {
        // Local script cards use the existing immutable card payload storage,
        // without a registration, result root, or cross-client execution receipt.
        anyhow::ensure!(zork_client_types::local_script::Card::parse(content).is_some(), "unsupported_message_card");
        (None, 0)
    };
    conn.execute(
        "INSERT INTO tool_interaction_messages(message_id,request_message_id,value,revision) VALUES(?1,?2,?3,?4)",
        params![id, root, serde_json::to_string(content)?, revision],
    )?;
    Ok(())
}

pub(super) fn resolution(conn: &Connection, root: &str) -> Result<Option<Message>> {
    let id: Option<String> = conn
        .query_row(
            "SELECT message_id FROM tool_interaction_messages WHERE request_message_id=?1 ORDER BY revision DESC LIMIT 1",
            [root],
            |r| r.get(0),
        )
        .optional()?;
    id.map(|id| message(conn, &id)).transpose()
}

impl StationDb {
    pub fn interaction_result(&self, chat: &str, root: &str) -> Result<Option<Message>> {
        let conn = self.conn.lock().expect("db mutex");
        let request = message(&conn, root)?;
        anyhow::ensure!(request.chat_id == chat, "interaction_chat_mismatch");
        resolution(&conn, root)
    }
}
