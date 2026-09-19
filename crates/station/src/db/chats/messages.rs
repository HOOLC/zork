use super::*;
use zork_client_types::files::{self, FileRef};

pub struct PreparedFile {
    pub reference: FileRef,
    pub source_path: String,
    pub workspace: String,
    pub media_type: String,
    pub content: Vec<u8>,
}

impl StationDb {
    pub fn record_receiving_policy(
        &self,
        agent: &str,
        target: &str,
        chat: &str,
        generation: u64,
        subscribed: bool,
    ) -> Result<()> {
        let changed=self.conn.lock().expect("db mutex").execute("INSERT INTO chat_receiving_policy VALUES(?1,?2,?3,?4,?5) ON CONFLICT(agent_id,target,chat_id) DO UPDATE SET generation=excluded.generation,subscribed=excluded.subscribed WHERE excluded.generation>generation",params![agent,target,chat,generation,subscribed])?;
        if changed > 0 {
            self.chat_topics
                .publish([Topic::Mailbox, Topic::AgentInput(agent.into())]);
        }
        Ok(())
    }

    pub fn accepts_chat_input(&self, agent: &str, target: &str, notice: &Notice) -> Result<bool> {
        accepts_input(&self.conn.lock().expect("db mutex"), agent, target, notice)
    }
    pub fn chat_visible_message(&self, id: &str) -> Result<VisibleMessageRow> {
        self.published_messages()?.query_row("SELECT sequence,message_id,session_key,role,text,kind,created_at FROM visible_message_content WHERE message_id=?1",[id],super::super::map_visible_message_row).context("chat_message_not_found")
    }
    pub fn prepare_chat_file(workspace: &Path, path: &Path) -> Result<PreparedFile> {
        let (source_path, name, media_type, content) =
            super::super::artifacts::prepare_file(workspace, path)?;
        let reference = FileRef {
            id: format!("file-{}", ulid::Ulid::new()),
            name,
            byte_len: content.len(),
            content_root: zork_mesh::content_root(&content),
        };
        anyhow::ensure!(reference.valid(), "invalid_attachment");
        Ok(PreparedFile {
            reference,
            source_path,
            workspace: workspace.to_string_lossy().into_owned(),
            media_type: media_type.into(),
            content,
        })
    }

    pub fn copy_chat_file(&self, source: &str, target: &str, id: &str) -> Result<PreparedFile> {
        let source = self.chat(source)?;
        let mut reference = self.conversation_file_ref(&source.session_key, id)?;
        let content = self.conversation_file_bytes(&source.session_key, &reference)?;
        let (workspace, media_type): (String, String) = self
            .conn
            .lock()
            .expect("db mutex")
            .query_row(
                "SELECT workspace,media_type FROM artifact_file_catalog WHERE artifact_id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .context("attachment_unavailable")?;
        if source.channel.chat_id != target {
            reference.id = format!("file-{}", ulid::Ulid::new());
        }
        Ok(PreparedFile {
            reference,
            source_path: format!("chat:{}/{id}", source.channel.chat_id),
            workspace,
            media_type,
            content,
        })
    }

    #[allow(clippy::too_many_arguments)]
    #[cfg(test)]
    pub fn post_chat_message(
        &self,
        request_key: &str,
        id: &str,
        chat: &str,
        author: &Author,
        text: &str,
        attachments: &[PreparedFile],
        reply_to: Option<&str>,
        mentions: &[String],
    ) -> Result<Message> {
        self.post_chat_with_pages(
            request_key,
            id,
            chat,
            author,
            text,
            attachments,
            reply_to,
            mentions,
            &[],
        )
    }
    #[allow(clippy::too_many_arguments)]
    #[cfg(test)]
    pub fn post_chat_with_pages(
        &self,
        request_key: &str,
        id: &str,
        chat: &str,
        author: &Author,
        text: &str,
        attachments: &[PreparedFile],
        reply_to: Option<&str>,
        mentions: &[String],
        pages: &[zork_client_types::pages::DeliveredPage],
    ) -> Result<Message> {
        self.post_chat_content(
            Some(request_key),
            id,
            chat,
            author,
            text,
            attachments,
            reply_to,
            mentions,
            pages,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn post_chat_content(
        &self,
        request_key: Option<&str>,
        id: &str,
        chat: &str,
        author: &Author,
        text: &str,
        attachments: &[PreparedFile],
        reply_to: Option<&str>,
        mentions: &[String],
        pages: &[zork_client_types::pages::DeliveredPage],
        interaction: Option<&Value>,
    ) -> Result<Message> {
        self.post_chat_content_from_client(
            request_key,
            id,
            chat,
            author,
            text,
            attachments,
            reply_to,
            mentions,
            pages,
            interaction,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn post_chat_content_from_client(
        &self,
        request_key: Option<&str>,
        id: &str,
        chat: &str,
        author: &Author,
        text: &str,
        attachments: &[PreparedFile],
        reply_to: Option<&str>,
        mentions: &[String],
        pages: &[zork_client_types::pages::DeliveredPage],
        interaction: Option<&Value>,
        client_id: Option<&str>,
    ) -> Result<Message> {
        anyhow::ensure!(
            text.len() <= zork_client_types::chat::MAX_MESSAGE_TEXT_BYTES,
            "message_too_large_submit_as_file"
        );
        anyhow::ensure!(
            !text.trim().is_empty() || !attachments.is_empty() || interaction.is_some(),
            "empty_message"
        );
        let refs = attachments
            .iter()
            .map(|f| f.reference.clone())
            .collect::<Vec<_>>();
        anyhow::ensure!(files::valid(&refs), "invalid_attachments");
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        if let Some(key) = request_key {
            command_active(&tx, key)?;
        }
        let business = interaction.and_then(zork_client_types::interaction::MessageContent::parse);
        let hydrated = business
            .as_ref()
            .map(|content| super::cards::hydrate(&tx, content, id, &author.id))
            .transpose()?
            .map(serde_json::to_value)
            .transpose()?;
        let interaction = hydrated.as_ref().or(interaction);
        if let Some(request_id) = business.as_ref().map(|content| content.request_id.as_str()) {
            if let Some(existing) = super::cards::existing_binding(&tx, request_id, chat)? {
                if let Some(key) = request_key {
                    finish(&tx, key, &serde_json::to_value(&existing)?)?;
                }
                tx.commit()?;
                self.flush_messages(&conn)?;
                return Ok(existing);
            }
        }
        let channel = tx
            .query_row(
                &format!("{CHANNEL_SELECT} WHERE chat_id=?1"),
                [chat],
                map_channel,
            )
            .context("chat_not_found")?;
        let now = now_rfc3339();
        for file in attachments {
            anyhow::ensure!(
                file.content.len() == file.reference.byte_len
                    && zork_mesh::content_root(&file.content) == file.reference.content_root,
                "attachment_snapshot_changed"
            );
            let existing: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM artifact_file_catalog WHERE artifact_id=?1)",
                [&file.reference.id],
                |r| r.get(0),
            )?;
            if existing {
                let owned:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM conversation_file_snapshots WHERE artifact_id=?1 AND session_key=?2 UNION ALL SELECT 1 FROM task_file_snapshots a JOIN product_tasks t ON t.task_id=a.task_id WHERE a.artifact_id=?1 AND t.session_key=?2)",params![file.reference.id,channel.session_key],|r|r.get(0))?;
                anyhow::ensure!(owned, "attachment_not_in_chat");
            } else {
                let version:i64=tx.query_row("SELECT COALESCE(MAX(version),0)+1 FROM conversation_file_snapshots WHERE session_key=?1 AND source_path=?2",params![channel.session_key,file.source_path],|r|r.get(0))?;
                let snapshot = self.freeze_file(&file.reference.name, &file.content)?;
                tx.execute("INSERT INTO conversation_file_snapshots(artifact_id,session_key,name,source_path,media_type,workspace,snapshot,version,created_at,caption) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,NULL)",
                    params![file.reference.id,channel.session_key,file.reference.name,file.source_path,file.media_type,file.workspace,snapshot,version,now])?;
            }
            tx.execute(
                "INSERT OR IGNORE INTO chat_file_metadata(artifact_id,reference) VALUES(?1,?2)",
                params![file.reference.id, serde_json::to_string(&file.reference)?],
            )?;
        }
        let content = files::compose(text, &refs);
        let mut topics = append_visible(
            &tx,
            &channel,
            id,
            author,
            &content,
            reply_to,
            mentions,
            pages,
            interaction,
            client_id,
            &now,
        )?;
        if let Some(request_id) = business.as_ref().map(|content| content.request_id.as_str()) {
            topics.extend(super::cards::bind(&tx, request_id, chat, id)?);
        }
        let result = message(&tx, id)?;
        if let Some(key) = request_key {
            finish(&tx, key, &serde_json::to_value(&result)?)?;
        }
        tx.commit()?;
        self.flush_messages(&conn)?;
        self.chat_topics.publish(topics);
        Ok(result)
    }

    pub fn record_chat_source(&self, _agent: &str, target: &str) -> Result<()> {
        // One feed per peer multiplexes every local Agent's notifications.
        let inserted = self.conn.lock().expect("db mutex").execute(
            "INSERT OR IGNORE INTO chat_sources(agent_id,target) VALUES ('*',?1)",
            [target],
        )?;
        if inserted > 0 {
            self.chat_topics.publish([Topic::Sources]);
        }
        Ok(())
    }

    pub fn chat_sources(&self) -> Result<Vec<(String, String, Option<String>, i64)>> {
        Ok(self
            .conn
            .lock()
            .expect("db mutex")
            .prepare(
                "SELECT agent_id,target,epoch,position FROM chat_sources ORDER BY agent_id,target",
            )?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Accept the remote batch and its replay cursor in one transaction. Agent
    /// mailbox delivery happens later using each notice's stable receipt ID.
    pub fn accept_chat_notices(
        &self,
        receiver_node: &str,
        target: &str,
        page: &NoticePage,
    ) -> Result<bool> {
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let (epoch, position): (Option<String>, i64) = tx
            .query_row(
                "SELECT epoch,position FROM chat_sources WHERE agent_id='*' AND target=?1",
                [target],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .context("chat_source_not_registered")?;
        anyhow::ensure!(
            epoch.as_ref().is_none_or(|e| e == &page.epoch),
            "chat_epoch_changed"
        );
        anyhow::ensure!(
            page.after >= 0 && page.after <= position && page.through >= page.after,
            "invalid_chat_cursor"
        );
        let mut previous = page.after;
        for notice in &page.items {
            anyhow::ensure!(
                notice.epoch == page.epoch
                    && notice.sequence > previous
                    && notice.sequence <= page.through,
                "invalid_chat_event_order"
            );
            let agent = if target == "local" {
                anyhow::ensure!(!notice.agent_ref.contains('/'), "invalid_chat_recipient");
                notice.agent_ref.as_str()
            } else {
                notice
                    .agent_ref
                    .strip_prefix(&format!("{receiver_node}/"))
                    .context("invalid_chat_recipient")?
            };
            anyhow::ensure!(
                !agent.is_empty() && agent.len() <= 256 && !agent.contains('/'),
                "invalid_chat_recipient"
            );
            previous = notice.sequence;
        }
        anyhow::ensure!(previous == page.through, "invalid_chat_page_boundary");
        if page.through <= position && epoch.is_some() {
            return Ok(false);
        }
        let mut inserted = 0;
        let mut topics = vec![];
        for notice in &page.items {
            if notice.sequence <= position || !notice.active {
                continue;
            }
            let agent = if target == "local" {
                anyhow::ensure!(!notice.agent_ref.contains('/'), "invalid_chat_recipient");
                notice.agent_ref.as_str()
            } else {
                notice
                    .agent_ref
                    .strip_prefix(&format!("{receiver_node}/"))
                    .context("invalid_chat_recipient")?
            };
            if !accepts_input(&tx, agent, target, notice)? {
                continue;
            }
            let id = format!(
                "channel-{}-{}",
                &zork_mesh::content_root(target.as_bytes())[..16],
                notice.id
            );
            let added=tx.execute("INSERT OR IGNORE INTO chat_mailbox(id,agent_id,target,notice) VALUES (?1,?2,?3,?4)",params![id,agent,target,serde_json::to_string(notice)?])?;
            inserted += added;
            if added > 0 {
                topics.push(Topic::AgentInput(agent.into()));
            }
        }
        tx.execute(
            "UPDATE chat_sources SET epoch=?2,position=?3 WHERE agent_id='*' AND target=?1",
            params![target, page.epoch, page.through.max(position)],
        )?;
        tx.commit()?;
        self.flush_messages(&conn)?;
        if inserted > 0 {
            topics.push(Topic::Mailbox);
            self.chat_topics.publish(topics);
        }
        Ok(true)
    }

    #[cfg(test)]
    pub fn pending_chat_inputs(&self) -> Result<Vec<(String, String, String, Notice)>> {
        self.pending_chat_inputs_for(None)
    }

    pub fn pending_chat_inputs_for(
        &self,
        agent: Option<&str>,
    ) -> Result<Vec<(String, String, String, Notice)>> {
        let conn = self.conn.lock().expect("db mutex");
        let rows=conn.prepare("SELECT q.id,q.agent_id,q.target,q.notice FROM chat_mailbox q WHERE q.delivered=0 AND (?1 IS NULL OR q.agent_id=?1) AND NOT EXISTS(
          SELECT 1 FROM chat_mailbox older WHERE older.agent_id=q.agent_id AND older.target=q.target AND older.delivered=0 AND older.rowid<q.rowid)
          ORDER BY q.rowid LIMIT 32")?
            .query_map([agent],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(id, agent, target, value)| {
                Ok((id, agent, target, serde_json::from_str(&value)?))
            })
            .collect()
    }

    pub fn finish_chat_input(&self, id: &str) -> Result<()> {
        self.conn.lock().expect("db mutex").execute(
            "UPDATE chat_mailbox SET delivered=1 WHERE id=?1 AND delivered=0",
            [id],
        )?;
        Ok(())
    }
}

/// The same transaction can append a business outcome and commit its effect.
#[allow(clippy::too_many_arguments)]
pub(super) fn append_visible(
    conn: &Connection,
    channel: &ChatRow,
    id: &str,
    author: &Author,
    text: &str,
    reply_to: Option<&str>,
    mentions: &[String],
    pages: &[zork_client_types::pages::DeliveredPage],
    interaction: Option<&Value>,
    client_id: Option<&str>,
    now: &str,
) -> Result<Vec<Topic>> {
    conn.execute("INSERT INTO visible_messages(message_id,session_key,connection_id,conversation_id,root_message_id,role,text,kind,created_at)
        SELECT ?1,key,connection_id,channel_id,root_thread_ts,?3,?4,?6,?5 FROM sessions WHERE key=?2",
        params![id,channel.session_key,if author.kind==AuthorKind::User {"user"}else{"assistant"},text,now,if pages.is_empty(){"message"}else{"page"}])?;
    if let Some(content) = interaction {
        interactions::insert(conn, id, content)?;
    }
    super::super::pages::record_pages(conn, &channel.session_key, id, pages, now)?;
    let row = conn.query_row("SELECT sequence,message_id,session_key,role,text,kind,created_at FROM visible_message_content WHERE message_id=?1",[id],super::super::map_visible_message_row)?;
    let topics = record_with_client(
        conn,
        &row,
        Some(author),
        reply_to,
        mentions,
        true,
        client_id,
    )?;
    conn.execute(
        "UPDATE sessions SET updated_at=?2 WHERE key=?1",
        params![channel.session_key, now],
    )?;
    Ok(topics)
}

fn accepts_input(conn: &Connection, agent: &str, target: &str, notice: &Notice) -> Result<bool> {
    if target == "local" {
        let current = preferences(conn, &notice.message.chat_id, agent)?;
        return Ok(current.subscribed && current.revision == notice.generation);
    }
    let known:Option<(u64,bool)>=conn.query_row("SELECT generation,subscribed FROM chat_receiving_policy WHERE agent_id=?1 AND target=?2 AND chat_id=?3",params![agent,target,notice.message.chat_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    Ok(known.is_none_or(|(generation, subscribed)| {
        notice.generation > generation || (subscribed && notice.generation == generation)
    }))
}
