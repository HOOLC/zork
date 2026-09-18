//! Channel facts and durable Agent delivery. All writes share the Station
//! transaction; subscriptions are preferences, never posting permissions.
use super::*;
pub(crate) mod agent_configuration;
mod agents;
mod execution;
pub(crate) mod provider_login;
pub use execution::WorkNotice;
pub(crate) mod cards;
pub(super) mod interactions;
mod messages;
mod navigation;
mod transfers;
pub use agents::configuration_revision;
#[cfg(test)]
mod tests;
pub use messages::PreparedFile;
use serde::{Deserialize, Serialize};
use zork_client_types::chat::{
    Author, AuthorKind, Channel, DeliveryMode, Message, Participant, Preferences, StartAt,
    UpdatePreferences,
};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Topic {
    Catalog,
    Channel(String),
    Recipient(String),
    Sources,
    Mailbox,
    AgentInput(String),
    UserInteraction(String),
    UserInteractionDelivery,
}

#[derive(Clone, Debug)]
pub struct ChatRow {
    pub channel: Channel,
    pub session_key: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Notice {
    pub epoch: String,
    #[serde(default)]
    pub generation: u64,
    pub sequence: i64,
    pub id: String,
    pub agent_ref: String,
    pub message: Message,
    pub delivery: DeliveryMode,
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work: Option<WorkNotice>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NoticePage {
    pub epoch: String,
    pub after: i64,
    pub through: i64,
    pub more: bool,
    pub items: Vec<Notice>,
}

pub struct CommandReceipt {
    pub object_id: String,
    pub result: Option<Value>,
    pub cancelled: bool,
}

pub(super) fn initialize(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS chat_channels(
          chat_id TEXT PRIMARY KEY, session_key TEXT NOT NULL UNIQUE REFERENCES sessions(key),
          title TEXT NOT NULL, epoch TEXT NOT NULL, created_at TEXT NOT NULL,
          message_count INTEGER NOT NULL DEFAULT 0, creator TEXT, last_message_at TEXT);
        CREATE TABLE IF NOT EXISTS chat_message_facts(
          message_id TEXT PRIMARY KEY REFERENCES visible_messages(message_id),
          chat_id TEXT NOT NULL REFERENCES chat_channels(chat_id),
          author_id TEXT NOT NULL, author TEXT NOT NULL, reply_to TEXT, mentions TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS chat_message_channel ON chat_message_facts(chat_id,message_id);
        CREATE INDEX IF NOT EXISTS chat_message_author ON chat_message_facts(chat_id,author_id);
        CREATE TABLE IF NOT EXISTS chat_participants(
          chat_id TEXT NOT NULL REFERENCES chat_channels(chat_id), author_id TEXT NOT NULL,
          author TEXT NOT NULL, message_count INTEGER NOT NULL, first_sequence INTEGER NOT NULL,
          PRIMARY KEY(chat_id,author_id));
        CREATE TABLE IF NOT EXISTS chat_preferences(
          chat_id TEXT NOT NULL REFERENCES chat_channels(chat_id), agent_ref TEXT NOT NULL,
          value TEXT NOT NULL, generation INTEGER NOT NULL DEFAULT 0,
          PRIMARY KEY(chat_id,agent_ref));
        CREATE TABLE IF NOT EXISTS chat_notices(
          sequence INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE,
          agent_ref TEXT NOT NULL, recipient_node TEXT NOT NULL DEFAULT 'local', chat_id TEXT NOT NULL, message_id TEXT NOT NULL,
          generation INTEGER NOT NULL, delivery TEXT NOT NULL, active INTEGER NOT NULL DEFAULT 1,
          UNIQUE(agent_ref,chat_id,message_id,generation));
        CREATE INDEX IF NOT EXISTS chat_notice_reader ON chat_notices(recipient_node,sequence);
        CREATE TABLE IF NOT EXISTS chat_receipts(
          request_key TEXT PRIMARY KEY, fingerprint TEXT NOT NULL, object_id TEXT NOT NULL,
          result TEXT, cancelled INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE IF NOT EXISTS chat_sources(
          agent_id TEXT NOT NULL,target TEXT NOT NULL,epoch TEXT,position INTEGER NOT NULL DEFAULT 0,
          PRIMARY KEY(agent_id,target));
        CREATE TABLE IF NOT EXISTS chat_mailbox(
          id TEXT PRIMARY KEY,agent_id TEXT NOT NULL,target TEXT NOT NULL,notice TEXT NOT NULL,
          delivered INTEGER NOT NULL DEFAULT 0);
        CREATE INDEX IF NOT EXISTS chat_mailbox_pending ON chat_mailbox(agent_id,target,delivered);
        CREATE TABLE IF NOT EXISTS chat_metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL);",
    )?;
    let has_client_id = conn
        .prepare("PRAGMA table_info(chat_message_facts)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .iter()
        .any(|name| name == "client_id");
    if !has_client_id {
        conn.execute_batch("ALTER TABLE chat_message_facts ADD COLUMN client_id TEXT;")?;
    }
    navigation::initialize(conn)?;
    conn.execute(
        "INSERT OR IGNORE INTO chat_metadata(key,value) VALUES ('epoch',?1)",
        [ulid::Ulid::new().to_string()],
    )?;
    transfers::initialize(conn)?;
    cards::initialize(conn)?;
    // Old IDs remain aliases of the same channel. No old acceptance decision
    // becomes a new message or a new delivery during migration.
    conn.execute(
        "INSERT OR IGNORE INTO chat_channels(chat_id,session_key,title,epoch,created_at)
         SELECT s.id,s.key,COALESCE(NULLIF(s.channel_name,''),
           (SELECT json_extract(a.value,'$.name') FROM node_agents a WHERE a.session_key=s.key),
           (SELECT NULLIF(t.title,'') FROM product_tasks t WHERE t.session_key=s.key),'Chat'),
           (SELECT value FROM chat_metadata WHERE key='epoch'),s.created_at
         FROM sessions s WHERE s.platform='local_gui' AND s.id IS NOT NULL
           AND COALESCE(s.channel_type,'') != 'agent_control'",
        [],
    )?;
    navigation::migrate_creators(conn)?;
    // Existing IM bindings already received new input. Preserve that receiving
    // preference once; newly created public channels have no implicit receiver.
    let bindings=conn.prepare("SELECT c.session_key FROM chat_channels c JOIN sessions s ON s.key=c.session_key WHERE COALESCE(s.channel_type,'')!='channel'")?
        .query_map([],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for key in bindings {
        ensure_legacy_receiver(conn, &key)?;
    }
    let mut after = 0;
    loop {
        let old = conn
            .prepare(
                "SELECT m.sequence,m.message_id,m.session_key,m.role,'',m.kind,m.created_at
         FROM visible_messages m JOIN chat_channels c ON c.session_key=m.session_key
         WHERE m.archived=0 AND NOT EXISTS(SELECT 1 FROM chat_message_facts f WHERE f.message_id=m.message_id)
           AND m.sequence>?1 ORDER BY m.sequence LIMIT 128",
            )?
            .query_map([after], super::map_visible_message_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if old.is_empty() {
            break;
        }
        for message in old {
            record(conn, &message, None, None, &[], false)?;
            after = message.sequence;
        }
    }
    Ok(())
}

fn map_channel(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChatRow> {
    Ok(ChatRow {
        channel: Channel {
            chat_id: row.get(0)?,
            title: row.get(2)?,
            created_at: row.get(4)?,
            message_count: row.get(5)?,
            creator: row
                .get::<_, Option<String>>(6)?
                .map(|value| {
                    serde_json::from_str(&value).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            6,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })
                })
                .transpose()?,
            last_message_at: row.get(7)?,
        },
        session_key: row.get(1)?,
    })
}
const CHANNEL_SELECT: &str = "SELECT chat_id,session_key,title,epoch,created_at,message_count,creator,last_message_at FROM chat_channels";

pub(super) fn ensure_channel(conn: &Connection, key: &str) -> Result<Option<String>> {
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO chat_channels(chat_id,session_key,title,epoch,created_at,creator)
         SELECT id,key,COALESCE(NULLIF(channel_name,''),'Chat'),
           (SELECT value FROM chat_metadata WHERE key='epoch'),created_at,
           (SELECT json_object('id',w.leader_id,'kind','agent','name',json_extract(a.value,'$.name'))
            FROM worker_tasks w LEFT JOIN node_agents a ON a.id=w.leader_id WHERE w.session_key=sessions.key)
         FROM sessions WHERE key=?1 AND platform='local_gui' AND id IS NOT NULL
           AND COALESCE(channel_type,'') != 'agent_control'",
        [key],
    )?;
    if inserted > 0 {
        navigation::set_legacy_title(conn, key)?;
    }
    Ok(conn
        .query_row(
            "SELECT chat_id FROM chat_channels WHERE session_key=?1",
            [key],
            |r| r.get(0),
        )
        .optional()?)
}

pub(super) fn ensure_legacy_receiver(conn: &Connection, key: &str) -> Result<()> {
    let Some(chat) = ensure_channel(conn, key)? else {
        return Ok(());
    };
    let kind: Option<String> = conn.query_row(
        "SELECT channel_type FROM sessions WHERE key=?1",
        [key],
        |r| r.get(0),
    )?;
    if kind.as_deref() == Some("channel") {
        return Ok(());
    }
    let migrated = conn.execute(
        "INSERT OR IGNORE INTO chat_metadata(key,value) VALUES(?1,'1')",
        [format!("binding:{chat}")],
    )?;
    if migrated == 0 {
        return Ok(());
    }
    let agent:Option<String>=conn.query_row("SELECT id FROM node_agents WHERE session_key=?1 UNION ALL SELECT worker_id FROM worker_tasks WHERE session_key=?1 LIMIT 1",[key],|r|r.get(0)).optional()?;
    let agent = agent.unwrap_or_else(|| format!("session:{key}"));
    let preferences = Preferences {
        subscribed: true,
        revision: 1,
        ..Default::default()
    };
    conn.execute("INSERT OR IGNORE INTO chat_preferences(chat_id,agent_ref,value,generation) VALUES(?1,?2,?3,1)",params![chat,agent,serde_json::to_string(&preferences)?])?;
    Ok(())
}

pub(super) fn legacy_author(conn: &Connection, message: &VisibleMessageRow) -> Result<Author> {
    let delegated = message.message_id.starts_with("assignment-") || message.message_id.starts_with("rework-")
        || conn.query_row("SELECT EXISTS(SELECT 1 FROM mesh_links WHERE role='owner' AND session_key=?1 AND 'mesh-'||assignment_id||'-goal'=?2)",
            params![message.session_key,message.message_id], |r| r.get::<_, bool>(0))?;
    let agent: Option<String> = if delegated {
        conn.query_row(
            "SELECT leader_id FROM worker_tasks WHERE session_key=?1",
            [&message.session_key],
            |r| r.get(0),
        )
        .optional()?
    } else if message.role == "assistant" {
        conn.query_row("SELECT id FROM node_agents WHERE session_key=?1 UNION ALL SELECT worker_id FROM worker_tasks WHERE session_key=?1 LIMIT 1", [&message.session_key], |r| r.get(0)).optional()?
    } else {
        None
    };
    let kind = if agent.is_some() || message.role == "assistant" {
        AuthorKind::Agent
    } else {
        AuthorKind::User
    };
    let id = agent.unwrap_or_else(|| {
        if kind == AuthorKind::User {
            "local-user".into()
        } else {
            format!("session:{}", message.session_key)
        }
    });
    let name = conn
        .query_row(
            "SELECT json_extract(value,'$.name') FROM node_agents WHERE id=?1",
            [&id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(Author { id, kind, name })
}

pub(super) fn message(conn: &Connection, id: &str) -> Result<Message> {
    let source: Option<String> = conn.query_row("SELECT message_value(sequence) FROM visible_messages WHERE message_id=?1 AND archived=1",
        [id],|r|r.get(0)).optional()?;
    if let Some(source) = source {
        return Ok(serde_json::from_str(&source)?);
    }
    let (chat_id, author, text, mentions, reply_to, created_at, client_id): (
        String,
        String,
        String,
        String,
        Option<String>,
        String,
        Option<String>,
    ) = conn
        .query_row(
            "SELECT f.chat_id,f.author,m.text,f.mentions,f.reply_to,m.created_at,f.client_id
         FROM chat_message_facts f JOIN visible_message_content m ON m.message_id=f.message_id
         WHERE f.message_id=?1",
            [id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            },
        )
        .context("chat_message_not_found")?;
    let (text, attachments) = zork_client_types::files::decode(&text).unwrap_or((text, vec![]));
    Ok(Message {
        message_id: id.into(),
        chat_id,
        author: serde_json::from_str(&author)?,
        client_id,
        text,
        attachments,
        mentions: serde_json::from_str(&mentions)?,
        reply_to,
        interaction: interactions::content(conn, id)?,
        created_at,
    })
}

fn enqueue(
    conn: &Connection,
    agent: &str,
    message: &Message,
    generation: u64,
    preferences: &Preferences,
) -> Result<Option<Topic>> {
    let reply_author: Option<String> = message
        .reply_to
        .as_ref()
        .map(|id| {
            conn.query_row(
                "SELECT author_id FROM chat_message_facts WHERE message_id=?1 AND chat_id=?2",
                params![id, message.chat_id],
                |r| r.get(0),
            )
            .optional()
        })
        .transpose()?
        .flatten();
    if preferences.receives(agent, message, reply_author.as_deref()) {
        let node = agent
            .split_once('/')
            .map(|(node, _)| node)
            .unwrap_or("local");
        let inserted = conn.execute("INSERT OR IGNORE INTO chat_notices(id,agent_ref,recipient_node,chat_id,message_id,generation,delivery) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![ulid::Ulid::new().to_string(),agent,node,message.chat_id,message.message_id,generation,serde_json::to_string(&preferences.delivery)?])?;
        return Ok((inserted > 0).then(|| Topic::Recipient(node.into())));
    }
    Ok(None)
}

pub(super) fn record(
    conn: &Connection,
    row: &VisibleMessageRow,
    author: Option<&Author>,
    reply_to: Option<&str>,
    mentions: &[String],
    deliver: bool,
) -> Result<Vec<Topic>> {
    record_with_client(conn, row, author, reply_to, mentions, deliver, None)
}

pub(super) fn record_with_client(
    conn: &Connection,
    row: &VisibleMessageRow,
    author: Option<&Author>,
    reply_to: Option<&str>,
    mentions: &[String],
    deliver: bool,
    client_id: Option<&str>,
) -> Result<Vec<Topic>> {
    let Some(chat_id) = ensure_channel(conn, &row.session_key)? else {
        return Ok(vec![]);
    };
    if let Some(reply) = reply_to {
        anyhow::ensure!(conn.query_row("SELECT EXISTS(SELECT 1 FROM chat_message_facts WHERE chat_id=?1 AND message_id=?2)",params![chat_id,reply],|r|r.get::<_,bool>(0))?,"invalid_reply_reference");
    }
    let author = match author {
        Some(author) => author.clone(),
        None => legacy_author(conn, row)?,
    };
    let inserted = conn.execute("INSERT OR IGNORE INTO chat_message_facts(message_id,chat_id,author_id,author,reply_to,mentions,client_id) VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![row.message_id,chat_id,author.id,serde_json::to_string(&author)?,reply_to,serde_json::to_string(mentions)?,client_id])?;
    if inserted == 0 {
        return Ok(vec![]);
    }
    let mut topics = vec![Topic::Channel(chat_id.clone()), Topic::Catalog];
    if row.role == "user" {
        // Retain the descriptive metadata of legacy aliases without deriving
        // review/completion state or changing the meaning of a new message.
        let title = zork_client_types::comments::display_text(&row.text)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(160)
            .collect::<String>();
        conn.execute("UPDATE product_tasks SET goal=?2,title=CASE WHEN title='' THEN ?3 ELSE title END WHERE session_key=?1 AND goal=''",params![row.session_key,row.text,title])?;
    }
    conn.execute(
        "UPDATE chat_channels SET message_count=message_count+1,last_message_at=?2 WHERE chat_id=?1",
        params![chat_id, row.created_at],
    )?;
    if author.kind != AuthorKind::System {
        conn.execute("INSERT INTO chat_participants(chat_id,author_id,author,message_count,first_sequence) VALUES (?1,?2,?3,1,?4)
        ON CONFLICT(chat_id,author_id) DO UPDATE SET message_count=message_count+1,author=excluded.author",
        params![chat_id,author.id,serde_json::to_string(&author)?,row.sequence])?;
    }
    if deliver {
        let preferences = conn
            .prepare("SELECT agent_ref,generation,value FROM chat_preferences WHERE chat_id=?1")?
            .query_map([&chat_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, u64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let message = message(conn, &row.message_id)?;
        anyhow::ensure!(
            serde_json::to_vec(&message)?.len() <= 96 * 1024,
            "message_metadata_too_large"
        );
        for (agent, generation, value) in preferences {
            topics.extend(enqueue(
                conn,
                &agent,
                &message,
                generation,
                &serde_json::from_str(&value)?,
            )?);
        }
    }
    Ok(topics)
}

impl StationDb {
    pub fn chat(&self, id: &str) -> Result<ChatRow> {
        self.published_messages()?.query_row(&format!("{CHANNEL_SELECT} WHERE chat_id=?1 OR session_key=?1 OR session_key IN(SELECT session_key FROM product_tasks WHERE task_id=?1) OR session_key IN(SELECT key FROM sessions WHERE id=?1)"),[id],map_channel).context("chat_not_found")
    }

    pub fn chats(&self, after: Option<&str>, limit: usize) -> Result<Vec<Channel>> {
        anyhow::ensure!((1..=100).contains(&limit), "invalid_limit");
        Ok(self
            .published_messages()?
            .prepare(&format!(
                "{CHANNEL_SELECT} WHERE (?1 IS NULL OR chat_id>?1) ORDER BY chat_id LIMIT ?2"
            ))?
            .query_map(params![after, limit], map_channel)?
            .map(|r| r.map(|c| c.channel))
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn chat_begin(&self, key: &str, fingerprint: &str) -> Result<CommandReceipt> {
        self.chat_begin_with_id(key, fingerprint, &ulid::Ulid::new().to_string())
    }

    /// Reserve the caller's message identity for a new operation. Recovery must
    /// keep the original receipt, including identities issued by older versions.
    pub fn chat_begin_with_id(
        &self,
        key: &str,
        fingerprint: &str,
        object_id: &str,
    ) -> Result<CommandReceipt> {
        let mut conn = self.published_messages()?;
        let tx = conn.transaction()?;
        tx.execute("INSERT OR IGNORE INTO chat_receipts(request_key,fingerprint,object_id) VALUES (?1,?2,?3)",params![key,fingerprint,object_id])?;
        let (saved, id, result, cancelled): (String, String, Option<String>, bool) = tx.query_row(
            "SELECT fingerprint,object_id,result,cancelled FROM chat_receipts WHERE request_key=?1",
            [key],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        anyhow::ensure!(saved == fingerprint, "idempotency_conflict");
        tx.commit()?;
        Ok(CommandReceipt {
            object_id: id,
            result: result.map(|v| receipt_result(&conn, &v)).transpose()?,
            cancelled,
        })
    }

    pub fn chat_cancel(&self, key: &str, fingerprint: &str) -> Result<Option<Value>> {
        let receipt = self.chat_begin(key, fingerprint)?;
        if receipt.result.is_some() {
            return Ok(receipt.result);
        }
        let conn = self.conn.lock().expect("db mutex");
        conn.execute(
            "UPDATE chat_receipts SET cancelled=1 WHERE request_key=?1 AND result IS NULL",
            [key],
        )?;
        let result: Option<String> = conn.query_row(
            "SELECT result FROM chat_receipts WHERE request_key=?1",
            [key],
            |r| r.get(0),
        )?;
        result.map(|v| receipt_result(&conn, &v)).transpose()
    }

    pub fn create_chat(&self, key: &str, id: &str, title: &str) -> Result<Channel> {
        self.create_chat_as(key, id, title, None)
    }

    pub fn create_chat_as(
        &self,
        key: &str,
        id: &str,
        title: &str,
        creator: Option<&Author>,
    ) -> Result<Channel> {
        anyhow::ensure!(
            !title.trim().is_empty() && title.len() <= 512,
            "invalid_chat_title"
        );
        let path = self.workspaces_root.join("channels").join(id);
        fs::create_dir_all(&path)?;
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        command_active(&tx, key)?;
        let session_key = format!("local_gui:{id}:{id}");
        let now = now_rfc3339();
        tx.execute("INSERT INTO sessions(key,id,connection_id,platform,channel_id,channel_name,channel_type,root_thread_ts,workspace_path,created_at,updated_at)
            VALUES (?1,?2,'local_gui','local_gui',?2,?3,'channel',?2,?4,?5,?5)",params![session_key,id,title,path.to_string_lossy(),now])?;
        ensure_channel(&tx, &session_key)?;
        if let Some(creator) = creator {
            tx.execute(
                "UPDATE chat_channels SET creator=?2 WHERE chat_id=?1 AND creator IS NULL",
                params![id, serde_json::to_string(creator)?],
            )?;
        }
        let channel = tx
            .query_row(
                &format!("{CHANNEL_SELECT} WHERE chat_id=?1"),
                [id],
                map_channel,
            )?
            .channel;
        finish(&tx, key, &serde_json::to_value(&channel)?)?;
        tx.commit()?;
        self.message_log.ensure_chat(id)?;
        self.chat_topics.publish([Topic::Catalog]);
        Ok(channel)
    }

    pub fn chat_preferences(&self, chat: &str, agent: &str) -> Result<Preferences> {
        self.chat(chat)?;
        preferences(&self.conn.lock().expect("db mutex"), chat, agent)
    }

    pub fn update_chat_preferences(
        &self,
        key: &str,
        chat: &str,
        agent: &str,
        update: &UpdatePreferences,
    ) -> Result<Preferences> {
        anyhow::ensure!(
            !update.changes.is_empty() || update.start.is_some(),
            "empty_preferences_change"
        );
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        command_active(&tx, key)?;
        tx.query_row(
            "SELECT chat_id FROM chat_channels WHERE chat_id=?1",
            [chat],
            |r| r.get::<_, String>(0),
        )
        .context("chat_not_found")?;
        let previous = preferences(&tx, chat, agent)?;
        anyhow::ensure!(
            update
                .expected_revision
                .is_none_or(|r| r == previous.revision),
            "chat_preferences_conflict"
        );
        let mut next = update.changes.apply(&previous);
        let changed = next != previous || update.start.is_some();
        if changed {
            next.revision = previous
                .revision
                .checked_add(1)
                .context("preferences_revision_exhausted")?;
            tx.execute("INSERT INTO chat_preferences(chat_id,agent_ref,value,generation) VALUES (?1,?2,?3,?4)
              ON CONFLICT(chat_id,agent_ref) DO UPDATE SET value=excluded.value,generation=excluded.generation",
              params![chat,agent,serde_json::to_string(&next)?,next.revision])?;
            // Pending notices use the policy under which they were enqueued.
            // A changed receiving rule revokes those not yet accepted downstream.
            tx.execute(
                "UPDATE chat_notices SET active=0 WHERE chat_id=?1 AND agent_ref=?2 AND active=1",
                params![chat, agent],
            )?;
            if next.subscribed {
                if let Some(StartAt::After { message_id }) = &update.start {
                    let after:i64=tx.query_row("SELECT m.sequence FROM visible_message_content m JOIN chat_message_facts f ON f.message_id=m.message_id WHERE f.chat_id=?1 AND f.message_id=?2",params![chat,message_id],|r|r.get(0)).context("invalid_message_cursor")?;
                    let mut after = after;
                    loop {
                        let ids=tx.prepare("SELECT m.sequence,m.message_id FROM visible_message_content m JOIN chat_message_facts f ON f.message_id=m.message_id WHERE f.chat_id=?1 AND m.sequence>?2 ORDER BY m.sequence LIMIT 128")?
                            .query_map(params![chat,after],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
                        if ids.is_empty() {
                            break;
                        }
                        for (sequence, id) in ids {
                            enqueue(&tx, agent, &message(&tx, &id)?, next.revision, &next)?;
                            after = sequence;
                        }
                    }
                }
            }
        }
        let mut value = serde_json::to_value(&next)?;
        value["chat_id"] = json!(chat);
        finish(&tx, key, &value)?;
        tx.commit()?;
        if changed {
            self.chat_topics.publish([
                Topic::Channel(chat.into()),
                Topic::Recipient(
                    agent
                        .split_once('/')
                        .map(|(node, _)| node)
                        .unwrap_or("local")
                        .into(),
                ),
            ]);
        }
        Ok(next)
    }

    pub fn chat_participants(&self, chat: &str) -> Result<Vec<Participant>> {
        self.chat(chat)?;
        let conn = self.published_messages()?;
        let rows=conn.prepare("SELECT p.author,p.message_count,s.value FROM chat_participants p LEFT JOIN chat_preferences s ON s.chat_id=p.chat_id AND s.agent_ref=p.author_id WHERE p.chat_id=?1 ORDER BY p.first_sequence")?
            .query_map([chat],|r|Ok((r.get::<_,String>(0)?,r.get::<_,u64>(1)?,r.get::<_,Option<String>>(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(author, count, prefs)| {
                Ok(Participant {
                    author: serde_json::from_str(&author)?,
                    message_count: count,
                    subscribed: prefs
                        .map(|p| serde_json::from_str::<Preferences>(&p))
                        .transpose()?
                        .is_some_and(|p| p.subscribed),
                })
            })
            .collect()
    }

    pub fn chat_message(&self, chat: &str, id: &str) -> Result<Message> {
        let conn = self.published_messages()?;
        let message = message(&conn, id)?;
        anyhow::ensure!(message.chat_id == chat, "chat_message_not_found");
        Ok(message)
    }

    pub fn chat_messages(
        &self,
        chat: &str,
        before: Option<i64>,
        limit: usize,
    ) -> Result<Vec<(i64, Message)>> {
        anyhow::ensure!((1..=100).contains(&limit), "invalid_limit");
        let binding = self.chat(chat)?;
        let conn = self.published_messages()?;
        let rows = conn
            .prepare(
                "SELECT m.sequence,m.message_id FROM visible_message_content m
            WHERE m.session_key=?1 AND m.sequence<=?2
            ORDER BY m.sequence DESC LIMIT ?3",
            )?
            .query_map(
                params![
                    binding.session_key,
                    before.map_or(i64::MAX, |sequence| sequence.saturating_sub(1)),
                    limit
                ],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(seq, id)| Ok((seq, message(&conn, &id)?)))
            .collect()
    }

    pub fn chat_notice_page(
        &self,
        recipient_node: &str,
        epoch: Option<&str>,
        after: i64,
    ) -> Result<NoticePage> {
        anyhow::ensure!(after >= 0, "invalid_mesh_cursor");
        let conn = self.published_messages()?;
        let head: i64 = conn.query_row(
            "SELECT COALESCE(MAX(sequence),0) FROM chat_notices",
            [],
            |r| r.get(0),
        )?;
        anyhow::ensure!(after <= head, "invalid_mesh_cursor");
        let current: String = conn.query_row(
            "SELECT value FROM chat_metadata WHERE key='epoch'",
            [],
            |r| r.get(0),
        )?;
        anyhow::ensure!(epoch.is_none_or(|e| e == current), "chat_epoch_changed");
        let rows=conn.prepare("SELECT sequence,id,message_id,delivery,active,agent_ref,generation FROM chat_notices WHERE recipient_node=?1 AND sequence>?2 ORDER BY sequence LIMIT 33")?
            .query_map(params![recipient_node,after],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,bool>(4)?,r.get::<_,String>(5)?,r.get::<_,u64>(6)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut items = Vec::new();
        let mut bytes = 0;
        let mut through = after;
        let total = rows.len();
        for (sequence, id, message_id, delivery, active, agent_ref, generation) in
            rows.into_iter().take(32)
        {
            let notice = Notice {
                epoch: current.clone(),
                generation,
                sequence,
                id,
                work: execution::notice(&conn, &agent_ref, &message_id)?,
                agent_ref,
                message: message(&conn, &message_id)?,
                delivery: serde_json::from_str(&delivery)?,
                active,
            };
            let size = serde_json::to_vec(&notice)?.len();
            anyhow::ensure!(size <= 128 * 1024, "chat_notification_too_large");
            if bytes + size > 192 * 1024 {
                break;
            }
            bytes += size;
            through = sequence;
            items.push(notice);
        }
        let more = items.len() < total;
        Ok(NoticePage {
            epoch: current,
            after,
            through,
            more,
            items,
        })
    }
}

fn preferences(conn: &Connection, chat: &str, agent: &str) -> Result<Preferences> {
    conn.query_row(
        "SELECT value FROM chat_preferences WHERE chat_id=?1 AND agent_ref=?2",
        params![chat, agent],
        |r| r.get::<_, String>(0),
    )
    .optional()?
    .map(|v| serde_json::from_str(&v).map_err(Into::into))
    .unwrap_or_else(|| Ok(Preferences::default()))
}
pub(super) fn command_active(conn: &Connection, key: &str) -> Result<()> {
    let active: bool = conn.query_row(
        "SELECT cancelled=0 AND result IS NULL FROM chat_receipts WHERE request_key=?1",
        [key],
        |r| r.get(0),
    )?;
    anyhow::ensure!(active, "chat_command_cancelled");
    Ok(())
}
pub(super) fn finish(conn: &Connection, key: &str, value: &Value) -> Result<()> {
    let mut value = value.clone();
    if value["message_id"].is_string()
        && value["chat_id"].is_string()
        && value["author"].is_object()
    {
        let id = value["message_id"].clone();
        let object = value.as_object_mut().unwrap();
        for field in [
            "message_id",
            "chat_id",
            "author",
            "text",
            "attachments",
            "mentions",
            "reply_to",
            "interaction",
            "created_at",
        ] {
            object.remove(field);
        }
        object.insert("$source_message".into(), id);
    }
    conn.execute(
        "UPDATE chat_receipts SET result=?2 WHERE request_key=?1",
        params![key, serde_json::to_string(&value)?],
    )?;
    Ok(())
}

pub(super) fn receipt_result(conn: &Connection, raw: &str) -> Result<Value> {
    let mut value: Value = serde_json::from_str(raw)?;
    if let Some(id) = value.get("$source_message").and_then(Value::as_str) {
        let source = serde_json::to_value(message(conn, id)?)?;
        let object = value.as_object_mut().unwrap();
        object.remove("$source_message");
        object.extend(source.as_object().unwrap().clone());
    }
    Ok(value)
}
