use super::*;
use zork_client_types::chat::StartChat;

impl StationDb {
    pub fn started_chat(&self, request: &StartChat, author: &Author) -> Result<Option<Channel>> {
        let key = format!("start-chat:{}:{}", author.id, request.request_id);
        let fingerprint = crate::node_access::fingerprint(&(request, author))?;
        let conn = self.published_messages()?;
        let receipt: Option<(String, Option<String>)> = conn
            .query_row(
                "SELECT fingerprint,result FROM chat_receipts WHERE request_key=?1",
                [&key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match receipt {
            Some((saved, result)) => {
                anyhow::ensure!(saved == fingerprint, "idempotency_conflict");
                result
                    .map(|value| Ok(serde_json::from_str(&value)?))
                    .transpose()
            }
            None => Ok(None),
        }
    }
    /// Allocation, receiving subscription and the first message commit together.
    /// Runtime materialization happens on delivery and can resume after a crash.
    pub fn start_chat(
        &self,
        request: &StartChat,
        author: &Author,
        client_identity: Option<&str>,
    ) -> Result<Channel> {
        self.start_chat_command(request, author, client_identity, None)
    }
    pub fn start_chat_command(
        &self,
        request: &StartChat,
        author: &Author,
        client_identity: Option<&str>,
        command: Option<&str>,
    ) -> Result<Channel> {
        anyhow::ensure!(
            ulid::Ulid::from_string(&request.request_id).is_ok(),
            "invalid_request_id"
        );
        request.validate().map_err(anyhow::Error::msg)?;
        let key = format!("start-chat:{}:{}", author.id, request.request_id);
        let fingerprint = crate::node_access::fingerprint(&(request, author))?;
        let receipt = self.chat_begin_with_id(&key, &fingerprint, &request.request_id)?;
        let id = &receipt.object_id;
        let path = self.workspaces_root.join("channels").join(id);
        fs::create_dir_all(&path)?;
        let title = request
            .title
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(&request.content)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(80)
            .collect::<String>();
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        if let Some(command) = command {
            command_active(&tx, command)?;
        }
        // Recheck under the write lock: concurrent retries share one allocation.
        if let Some(result) = tx.query_row(
            "SELECT result FROM chat_receipts WHERE request_key=?1",
            [&key],
            |r| r.get::<_, Option<String>>(0),
        )? {
            let result: Channel = serde_json::from_str(&result)?;
            if let Some(command) = command {
                finish_start_command(&tx, command, &result)?;
            }
            tx.commit()?;
            return Ok(result);
        }
        command_active(&tx, &key)?;
        let session_key = format!("local_gui:{id}:{id}");
        let now = now_rfc3339();
        let profile = if request.profile_id.is_empty() {
            "auto"
        } else {
            &request.profile_id
        };
        tx.execute("INSERT INTO sessions(key,id,connection_id,platform,channel_id,channel_name,channel_type,root_thread_ts,workspace_path,created_at,updated_at,profile_id,model,thinking,initiator_user_id)
            VALUES (?1,?2,'local_gui','local_gui',?2,?3,'chat',?2,?4,?5,?5,?6,?7,?8,?9)",
            params![session_key,id,title,path.to_string_lossy(),now,profile,request.model,request.thinking,author.id])?;
        ensure_legacy_receiver(&tx, &session_key)?;
        if author.kind == AuthorKind::Agent {
            let preferences = Preferences {
                subscribed: true,
                revision: 1,
                ..Default::default()
            };
            tx.execute("INSERT OR IGNORE INTO chat_preferences(chat_id,agent_ref,value,generation) VALUES(?1,?2,?3,1)",params![id,author.id,serde_json::to_string(&preferences)?])?;
        }
        tx.execute(
            "UPDATE chat_channels SET creator=?2 WHERE chat_id=?1",
            params![id, serde_json::to_string(author)?],
        )?;
        let channel = tx.query_row(
            &format!("{CHANNEL_SELECT} WHERE chat_id=?1"),
            [id],
            map_channel,
        )?;
        let content = zork_client_types::files::compose(&request.content, &[]);
        let mut topics = messages::append_visible(
            &tx,
            &channel,
            &format!("client-{id}-{}", request.request_id),
            author,
            &content,
            None,
            None,
            &[],
            &[],
            None,
            client_identity,
            &now,
        )?;
        let result = tx
            .query_row(
                &format!("{CHANNEL_SELECT} WHERE chat_id=?1"),
                [id],
                map_channel,
            )?
            .channel;
        finish(&tx, &key, &serde_json::to_value(&result)?)?;
        if let Some(command) = command {
            finish_start_command(&tx, command, &result)?;
        }
        tx.commit()?;
        self.flush_messages(&conn)?;
        topics.push(Topic::Catalog);
        self.chat_topics.publish(topics);
        Ok(result)
    }
}

fn finish_start_command(conn: &Connection, key: &str, chat: &Channel) -> Result<()> {
    let mut value = serde_json::to_value(chat)?;
    value["session_id"] = json!(chat.chat_id);
    finish(conn, key, &value)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(text: &str) -> StartChat {
        StartChat {
            request_id: ulid::Ulid::new().to_string(),
            content: text.into(),
            model: "model".into(),
            thinking: "high".into(),
            profile_id: "auto".into(),
            title: None,
            client_id: None,
        }
    }
    fn user() -> Author {
        Author {
            id: "local-user".into(),
            kind: AuthorKind::User,
            name: None,
        }
    }
    fn database(root: &Path) -> StationDb {
        StationDb::open(root, &root.join("workspaces")).unwrap()
    }

    #[test]
    fn first_send_commits_one_chat_selection_message_and_receiver_across_restart() {
        let root = tempfile::tempdir().unwrap();
        let db = database(root.path());
        let input = request("你好\n保留首条消息的换行");
        let chat = db
            .start_chat(&input, &user(), Some("local/client"))
            .unwrap();
        assert_eq!(chat.chat_id, input.request_id);
        assert_eq!(chat.message_count, 1);
        assert!(db.node_agents().unwrap().is_empty());
        let session = db.get_session_by_id(&chat.chat_id).unwrap().unwrap();
        assert_eq!(session.channel_type.as_deref(), Some("chat"));
        let configuration = db.agent_configuration(&chat.chat_id).unwrap().unwrap();
        assert_eq!(configuration.selection.model, "model");
        assert_eq!(configuration.selection.thinking, "high");
        assert_eq!(configuration.selection.profile_id, "auto");
        assert!(configuration.end_turn_confirmation.is_some());
        let notices = db.chat_notice_page("local", None, 0).unwrap();
        assert_eq!(notices.items.len(), 1);
        assert_eq!(
            notices.items[0].agent_ref,
            format!("session:{}", session.key)
        );
        assert_eq!(notices.items[0].message.text, input.content);
        assert_eq!(
            notices.items[0].message.client_id.as_deref(),
            Some("local/client")
        );
        assert_eq!(
            db.session_chat_destination(&chat.chat_id).unwrap(),
            Some(("local".into(), chat.chat_id.clone()))
        );
        drop(db);
        let db = database(root.path());
        // Acquiring the node Mesh identity does not change the caller
        // request or permit reallocating its accepted first send.
        assert_eq!(
            db.start_chat(&input, &user(), Some("node/client")).unwrap(),
            chat
        );
        assert_eq!(db.chats(None, 100).unwrap().len(), 1);
        assert_eq!(
            db.chat_notice_page("local", None, 0).unwrap().items.len(),
            1
        );
        let mut changed = input.clone();
        changed.thinking = "low".into();
        assert!(db
            .start_chat(&changed, &user(), Some("node/client"))
            .unwrap_err()
            .to_string()
            .contains("idempotency_conflict"));
    }

    #[test]
    fn failed_first_message_rolls_back_all_visible_objects_and_retry_uses_same_allocation() {
        let root = tempfile::tempdir().unwrap();
        let db = database(root.path());
        let input = request("first");
        db.conn.lock().unwrap().execute_batch("CREATE TRIGGER reject_first BEFORE INSERT ON visible_messages BEGIN SELECT RAISE(ABORT,'fixture failure'); END;").unwrap();
        assert!(db.start_chat(&input, &user(), None).is_err());
        assert!(db.chats(None, 100).unwrap().is_empty());
        assert!(db.get_session_by_id(&input.request_id).unwrap().is_none());
        db.conn
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER reject_first")
            .unwrap();
        assert_eq!(
            db.start_chat(&input, &user(), None).unwrap().chat_id,
            input.request_id
        );
        assert_eq!(
            db.chat_notice_page("local", None, 0).unwrap().items.len(),
            1
        );
    }

    #[test]
    fn new_chats_have_independent_selection_and_quoted_file_envelopes_stay_text() {
        let root = tempfile::tempdir().unwrap();
        let db = database(root.path());
        let quoted =
            "<zork-files version=\"1\">\n{\"text\":\"quoted\",\"files\":[]}\n</zork-files>";
        let first = request(quoted);
        let mut second = request("another task");
        second.model = "other-model".into();
        second.thinking = "medium".into();
        second.profile_id = "explicit".into();
        for input in [&first, &second] {
            db.start_chat(input, &user(), None).unwrap();
        }
        assert_ne!(first.request_id, second.request_id);
        let msg = db
            .chat_message(
                &first.request_id,
                &format!("client-{}-{}", first.request_id, first.request_id),
            )
            .unwrap();
        assert_eq!(msg.text, quoted);
        assert!(msg.attachments.is_empty());
        let a = db.agent_configuration(&first.request_id).unwrap().unwrap();
        let b = db.agent_configuration(&second.request_id).unwrap().unwrap();
        assert_eq!(a.selection.model, "model");
        assert_eq!(b.selection.model, "other-model");
        assert_eq!(b.selection.profile_id, "explicit");
    }

    #[test]
    fn collaboration_creates_a_new_chat_without_agent_definitions_or_grants() {
        let root = tempfile::tempdir().unwrap();
        let db = database(root.path());
        let parent = db.start_chat(&request("parent"), &user(), None).unwrap();
        let parent_session = db.get_session_by_id(&parent.chat_id).unwrap().unwrap();
        let creator = Author {
            id: format!("session:{}", parent_session.key),
            kind: AuthorKind::Agent,
            name: Some("Session".into()),
        };
        let child = db
            .start_chat(&request("independent work"), &creator, None)
            .unwrap();
        let child_session = db.get_session_by_id(&child.chat_id).unwrap().unwrap();
        assert_ne!(parent.chat_id, child.chat_id);
        assert!(db.node_agents().unwrap().is_empty());
        assert!(
            db.chat_preferences(&child.chat_id, &creator.id)
                .unwrap()
                .subscribed
        );
        let child_author = Author {
            id: format!("session:{}", child_session.key),
            kind: AuthorKind::Agent,
            name: Some("Session".into()),
        };
        db.post_chat_content(
            None,
            "reply",
            &child.chat_id,
            &child_author,
            "done",
            &[],
            None,
            &[],
            &[],
            None,
        )
        .unwrap();
        let notices = db.chat_notice_page("local", None, 0).unwrap().items;
        assert_eq!(
            notices
                .iter()
                .filter(|n| n.message.message_id == "reply" && n.agent_ref == creator.id)
                .count(),
            1
        );
        assert!(!notices
            .iter()
            .any(|n| n.message.message_id == "reply" && n.agent_ref == child_author.id));
    }

    #[test]
    fn tool_cancellation_and_receipt_share_the_creation_commit() {
        let root = tempfile::tempdir().unwrap();
        let db = database(root.path());
        let rejected = request("cancelled work");
        db.chat_begin("cancelled-command", "signature").unwrap();
        db.chat_cancel("cancelled-command", "signature").unwrap();
        assert!(db
            .start_chat_command(&rejected, &user(), None, Some("cancelled-command"))
            .is_err());
        assert!(db.chats(None, 100).unwrap().is_empty());
        let accepted = request("accepted work");
        db.chat_begin("accepted-command", "signature").unwrap();
        let chat = db
            .start_chat_command(&accepted, &user(), None, Some("accepted-command"))
            .unwrap();
        let receipt = db
            .chat_begin("accepted-command", "signature")
            .unwrap()
            .result
            .unwrap();
        assert_eq!(receipt["chat_id"], chat.chat_id);
        assert_eq!(receipt["session_id"], chat.chat_id);
        assert_eq!(
            db.chat_cancel("accepted-command", "signature")
                .unwrap()
                .unwrap(),
            receipt
        );
    }
}
