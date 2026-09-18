//! Public cards supplied by the registered businesses. Registration metadata
//! lives in db::interaction_registry; each adapter owns its persistent card data.
use super::*;
use crate::{
    db::interaction_registry::{self as registry, Registration},
    node_access::Subject,
};
use zork_client_types::interaction::{MessageContent, Request, Resolution, Response};
pub(crate) mod publication;
#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct CardRecord {
    pub request_id: String,
    pub owner: Subject,
    pub invocation_id: String,
    pub request_key: String,
    pub handler: String,
    pub request: Request,
    pub submission: Option<SubmittedResponse>,
    pub result: Option<Resolution>,
}

impl CardRecord {
    pub(super) fn new(
        entry: Registration,
        request: Request,
        submission: Option<SubmittedResponse>,
        result: Option<Resolution>,
    ) -> Self {
        Self {
            request_id: entry.request_id,
            owner: entry.owner,
            invocation_id: entry.invocation_id,
            request_key: entry.request_key,
            handler: entry.handler,
            request,
            submission,
            result,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SubmittedResponse {
    pub response: Response,
    pub actor: String,
}

pub(crate) struct Adapter {
    pub key: &'static str,
    pub initialize: fn(&Connection) -> Result<()>,
    pub cleanup: crate::interaction_registry::CleanupHandler,
    pub read: fn(&Connection, Registration) -> Result<CardRecord>,
    pub write_result: fn(&Connection, &CardRecord, Resolution) -> Result<Resolution>,
}

// Business composition. Adding a card adapter does not change the registration
// schema, lifecycle manager or its payload-free contract.
const ADAPTERS: &[Adapter] = &[
    super::agent_configuration::CARD,
    super::provider_login::CARD,
];
pub(crate) fn adapters() -> &'static [Adapter] {
    ADAPTERS
}
fn adapter(key: &str) -> Result<&'static Adapter> {
    ADAPTERS
        .iter()
        .find(|a| a.key == key)
        .context("interaction_handler_unavailable")
}

pub(super) fn initialize(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS business_card_bindings(
         request_id TEXT NOT NULL, chat_id TEXT NOT NULL,
         message_id TEXT NOT NULL UNIQUE REFERENCES visible_messages(message_id),
         PRIMARY KEY(request_id,chat_id));
         CREATE TABLE IF NOT EXISTS tool_interaction_messages(
         message_id TEXT PRIMARY KEY REFERENCES visible_messages(message_id),
         request_message_id TEXT REFERENCES visible_messages(message_id), value TEXT NOT NULL);
         CREATE INDEX IF NOT EXISTS tool_interaction_root ON tool_interaction_messages(request_message_id);"
    )?;
    for adapter in ADAPTERS {
        (adapter.initialize)(conn)?;
    }
    let has_revision = conn
        .prepare("PRAGMA table_info(tool_interaction_messages)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .iter()
        .any(|name| name == "revision");
    if !has_revision {
        conn.execute_batch("ALTER TABLE tool_interaction_messages ADD COLUMN revision INTEGER NOT NULL DEFAULT 0;
            UPDATE tool_interaction_messages SET revision=COALESCE(json_extract(value,'$.result.revision'),0);")?;
    }
    publication::initialize(conn)?;
    Ok(())
}

pub(super) fn read(conn: &Connection, id: &str) -> Result<CardRecord> {
    if let Some(record) = publication::mirror(conn, id)? {
        return Ok(record);
    }
    let entry = registry::read(conn, id)?;
    (adapter(&entry.handler)?.read)(conn, entry)
}

fn append_result(
    conn: &Connection,
    request: &CardRecord,
    chat: &str,
    root: &str,
) -> Result<(Message, Vec<Topic>)> {
    let mut result = request
        .result
        .clone()
        .context("interaction_result_required")?;
    result.request_message_id = root.into();
    let channel = conn.query_row(
        &format!("{CHANNEL_SELECT} WHERE chat_id=?1"),
        [chat],
        map_channel,
    )?;
    let id = ulid::Ulid::new().to_string();
    let text = serde_json::to_string(
        &json!({"request_id":request.request_id,"outcome":result.outcome,"output":result.output}),
    )?;
    let topics = messages::append_visible(
        conn,
        &channel,
        &id,
        &Author {
            id: "zork".into(),
            kind: AuthorKind::System,
            name: Some("Zork".into()),
        },
        &text,
        Some(root),
        &[super::super::super::channels::request_actor(&request.owner)],
        &[],
        Some(&serde_json::to_value(MessageContent::linked_result(
            request.request_id.clone(),
            request.handler.clone(),
            result,
        ))?),
        None,
        &now_rfc3339(),
    )?;
    Ok((message(conn, &id)?, topics))
}

pub(super) fn resolve(
    conn: &Connection,
    record: &mut CardRecord,
    result: Resolution,
) -> Result<Vec<Topic>> {
    let result = (adapter(&record.handler)?.write_result)(conn, record, result)?;
    publication::record_event(conn, &record.request_id, &result)?;
    if result.outcome.terminal() {
        registry::retire(conn, &record.request_id)?;
    }
    record.result = Some(result);
    let bindings = conn
        .prepare("SELECT chat_id,message_id FROM business_card_bindings WHERE request_id=?1")?
        .query_map([&record.request_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut topics = vec![
        Topic::UserInteraction(record.request_id.clone()),
        Topic::UserInteractionDelivery,
    ];
    for (chat, root) in bindings {
        topics.extend(append_result(conn, record, &chat, &root)?.1);
    }
    Ok(topics)
}

pub(super) fn existing_binding(conn: &Connection, id: &str, chat: &str) -> Result<Option<Message>> {
    let root: Option<String> = conn
        .query_row(
            "SELECT message_id FROM business_card_bindings WHERE request_id=?1 AND chat_id=?2",
            params![id, chat],
            |r| r.get(0),
        )
        .optional()?;
    root.map(|id| message(conn, &id)).transpose()
}

pub(super) fn hydrate(
    conn: &Connection,
    content: &MessageContent,
    id: &str,
    actor: &str,
) -> Result<MessageContent> {
    let request = read(conn, &content.request_id)?;
    anyhow::ensure!(
        super::super::super::channels::request_actor(&request.owner) == actor,
        "interaction_owner_mismatch"
    );
    let snapshot = request.result.map(|mut result| {
        result.request_message_id = id.into();
        result
    });
    Ok(MessageContent::linked(
        request.request_id,
        request.handler,
        request.request,
        snapshot,
    ))
}

pub(super) fn bind(
    conn: &Connection,
    request_id: &str,
    chat: &str,
    root: &str,
) -> Result<Vec<Topic>> {
    conn.execute(
        "INSERT INTO business_card_bindings(request_id,chat_id,message_id) VALUES(?1,?2,?3)",
        params![request_id, chat, root],
    )?;
    let request = read(conn, request_id)?;
    Ok(if request.result.is_some() {
        append_result(conn, &request, chat, root)?.1
    } else {
        vec![]
    })
}

impl StationDb {
    pub(crate) fn business_card(&self, id: &str) -> Result<CardRecord> {
        read(&self.conn.lock().expect("db mutex"), id)
    }
    pub(crate) fn business_card_for_call(
        &self,
        owner: &Subject,
        invocation: &str,
        key: &str,
    ) -> Result<Option<CardRecord>> {
        let conn = self.conn.lock().expect("db mutex");
        registry::find(&conn, owner, invocation, key)?
            .map(|entry| read(&conn, &entry.request_id))
            .transpose()
    }
    pub(crate) fn business_card_message(&self, id: &str, chat: &str) -> Result<Option<Message>> {
        existing_binding(&self.conn.lock().expect("db mutex"), id, chat)
    }
    pub(crate) fn bound_business_card(&self, chat: &str, root: &str) -> Result<String> {
        self.conn
            .lock()
            .expect("db mutex")
            .query_row(
                "SELECT request_id FROM business_card_bindings WHERE chat_id=?1 AND message_id=?2",
                params![chat, root],
                |r| r.get(0),
            )
            .context("interaction_binding_not_found")
    }
}
