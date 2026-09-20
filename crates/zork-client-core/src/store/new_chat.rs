use super::*;
use crate::state::NewChatData;

impl ClientStore {
    /// The first-send operation and its ordinary local message share one commit.
    /// attempted=1 reserves dispatch for NewChat; the ordinary pump only follows
    /// the source echo, which confirms this very same row.
    pub(crate) fn save_new_chat(
        &self,
        node: &str,
        previous: &NewChatData,
        next: &NewChatData,
    ) -> Result<()> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        let revoked = tx
            .query_row(
                "SELECT revoked FROM replica_bindings WHERE peer=?1",
                [node],
                |r| r.get::<_, bool>(0),
            )
            .optional()?
            .unwrap_or(false);
        anyhow::ensure!(!revoked, "设备访问权限已撤销");
        tx.execute("INSERT INTO cache(node,key,value) VALUES(?1,'new-chat',?2) ON CONFLICT(node,key) DO UPDATE SET value=excluded.value",params![node,serde_json::to_string(next)?])?;
        if let Some(request) = &next.pending {
            let exists = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE node=?1 AND request_id=?2)",
                params![node, request.request_id],
                |r| r.get::<_, bool>(0),
            )?;
            if !exists {
                message_delivery::insert(
                    &tx,
                    node,
                    &QueuedMessage {
                        request_id: request.request_id.clone(),
                        session_id: request.request_id.clone(),
                        content: zork_client_types::files::compose(&request.content, &[]),
                        attempted: true,
                        sent_at_ms: delivery_now_ms(),
                        ..Default::default()
                    },
                )?;
            } else if next.busy && !previous.busy {
                tx.execute("UPDATE messages SET status='sending',error=NULL WHERE node=?1 AND request_id=?2 AND position IS NULL",params![node,request.request_id])?;
            }
        } else if next.created.is_none() {
            if let Some(rejected) = &previous.pending {
                tx.execute(
                    "DELETE FROM messages WHERE node=?1 AND request_id=?2 AND position IS NULL",
                    params![node, rejected.request_id],
                )?;
            }
        }
        tx.commit()?;
        drop(conn);
        if previous.pending != next.pending || previous.busy != next.busy {
            self.delivery_changed();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{MessageMetadata, MessagePage, Role, TranscriptMessage};
    use zork_client_types::chat::StartChat;

    fn pending() -> NewChatData {
        let id = ulid::Ulid::new().to_string();
        NewChatData {
            text: "first".into(),
            model: "m".into(),
            thinking: "high".into(),
            profile: "auto".into(),
            busy: true,
            pending: Some(StartChat {
                request_id: id,
                content: "first".into(),
                model: "m".into(),
                thinking: "high".into(),
                profile_id: "auto".into(),
                title: None,
                client_id: None,
            }),
            ..Default::default()
        }
    }
    #[test]
    fn first_send_and_creation_record_are_atomic_and_echo_confirms_the_same_row() {
        let root = tempfile::tempdir().unwrap();
        let store = ClientStore::open(root.path()).unwrap();
        let input = pending();
        let request = input.pending.as_ref().unwrap();
        store
            .save_new_chat("node", &Default::default(), &input)
            .unwrap();
        let original: i64 = store
            .0
            .lock()
            .unwrap()
            .query_row("SELECT rowid FROM messages", [], |r| r.get(0))
            .unwrap();
        assert!(
            store
                .begin_delivery("node", &request.request_id)
                .unwrap()
                .is_none(),
            "ordinary sending must not dispatch a creation message"
        );
        let echo = TranscriptMessage::Message {
            role: Role::User,
            content: input.text.clone(),
            metadata: MessageMetadata {
                id: Some(format!(
                    "client-{}-{}",
                    request.request_id, request.request_id
                )),
                ..Default::default()
            },
        };
        store
            .cache_message_page(
                "node",
                &request.request_id,
                &MessagePage {
                    items: vec![echo],
                    source_epoch: None,
                    older_cursor: None,
                },
                None,
            )
            .unwrap();
        let result: (i64, String, i64) = store
            .0
            .lock()
            .unwrap()
            .query_row("SELECT rowid,status,COUNT(*) FROM messages", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(result, (original, "sent".into(), 1));
        store
            .fail_delivery("node", &request.request_id, "late failure")
            .unwrap();
        assert!(store.outbox("node").unwrap().is_empty());
    }
    #[test]
    fn local_insert_failure_leaves_neither_a_creation_operation_nor_a_message() {
        let root = tempfile::tempdir().unwrap();
        let store = ClientStore::open(root.path()).unwrap();
        store.0.lock().unwrap().execute_batch("CREATE TRIGGER refuse_message BEFORE INSERT ON messages BEGIN SELECT RAISE(ABORT,'storage failure'); END;").unwrap();
        assert!(store
            .save_new_chat("node", &Default::default(), &pending())
            .is_err());
        assert!(store
            .get::<NewChatData>("node", "new-chat")
            .unwrap()
            .is_none());
        assert!(store.outbox("node").unwrap().is_empty());
    }
}
