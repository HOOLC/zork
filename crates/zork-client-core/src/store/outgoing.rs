//! Admit a send, its generated attachment and draft clearing atomically.
use super::QueuedMessage;
use anyhow::{ensure, Result};
use rusqlite::{params, Transaction};
use zork_client_types::{
    chat::MAX_MESSAGE_TEXT_BYTES,
    comments,
    files::{self, FileRef},
};

pub(super) fn insert_and_clear_draft(
    tx: &Transaction<'_>,
    node: &str,
    message: &QueuedMessage,
) -> Result<QueuedMessage> {
    crate::valid_content(&message.content, false)?;
    let mut message = message.clone();
    let (text, mut references) =
        files::decode(&message.content).unwrap_or_else(|| (message.content.clone(), vec![]));
    ensure!(
        files::valid(&references),
        "每条消息最多 16 个附件，共 1200 MiB"
    );
    if text.len() > MAX_MESSAGE_TEXT_BYTES {
        // Preserve the exact body for plain text; structured comments and
        // inline text attachments use their existing human-readable projection.
        let text = if comments::decode_document(&text).is_some() {
            comments::display_text(&text)
        } else {
            text
        };
        ensure!(
            text.len() <= files::MAX_FILE_BYTES,
            "消息正文超过 300 MiB，无法作为单个文件发送"
        );
        let file = FileRef {
            id: format!("file-{}", ulid::Ulid::new()),
            name: "message.txt".into(),
            byte_len: text.len(),
            content_root: zork_mesh::content_root(text.as_bytes()),
        };
        references.push(file.clone());
        ensure!(
            files::valid(&references),
            "正文转为文件后，每条消息最多 16 个附件，共 1200 MiB"
        );
        tx.execute(
            "INSERT INTO blobs(node,key,value) VALUES (?1,?2,?3)",
            params![node, format!("upload:{}", file.id), text.as_bytes()],
        )?;
        message.content = files::compose("", &references);
    }
    super::message_delivery::insert(tx, node, &message)?;
    for (key, value) in [
        (format!("draft:{}", message.session_id), "\"\""),
        (format!("draft-comments:{}", message.session_id), "[]"),
    ] {
        tx.execute("INSERT INTO cache(node,key,value) VALUES (?1,?2,?3) ON CONFLICT(node,key) DO UPDATE SET value=excluded.value", params![node, key, value])?;
    }
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ClientStore;

    fn pending(content: String) -> QueuedMessage {
        QueuedMessage {
            request_id: "send".into(),
            session_id: "chat".into(),
            content,
            attempted: false,
            ..Default::default()
        }
    }

    #[test]
    fn generated_file_message_and_draft_commit_or_rollback_together() {
        let root = tempfile::tempdir().unwrap();
        let text = "完整正文\n".repeat(6000);
        let message;
        let file;
        {
            let store = ClientStore::open(root.path()).unwrap();
            store.put("node", "draft:chat", &text).unwrap();
            store.0.lock().unwrap().execute_batch("CREATE TRIGGER fail_send BEFORE INSERT ON messages BEGIN SELECT RAISE(ABORT,'fixture failure'); END;").unwrap();
            assert!(store
                .enqueue_and_clear_draft("node", &pending(text.clone()))
                .is_err());
            assert!(store.outbox("node").unwrap().is_empty());
            assert_eq!(
                store
                    .0
                    .lock()
                    .unwrap()
                    .query_row("SELECT COUNT(*) FROM blobs", [], |row| row.get::<_, u64>(0))
                    .unwrap(),
                0
            );
            assert_eq!(
                store.get::<String>("node", "draft:chat").unwrap().unwrap(),
                text
            );
            store
                .0
                .lock()
                .unwrap()
                .execute_batch("DROP TRIGGER fail_send")
                .unwrap();
            message = store
                .enqueue_and_clear_draft("node", &pending(text.clone()))
                .unwrap();
            file = files::decode(&message.content).unwrap().1.remove(0);
            assert_eq!(
                store
                    .blob("node", &format!("upload:{}", file.id))
                    .unwrap()
                    .unwrap(),
                text.as_bytes()
            );
        }
        let store = ClientStore::open(root.path()).unwrap();
        assert_eq!(store.outbox("node").unwrap()[0].content, message.content);
        assert_eq!(
            store
                .blob("node", &format!("upload:{}", file.id))
                .unwrap()
                .unwrap(),
            text.as_bytes()
        );
        store.put("node", "draft:chat", &"next draft").unwrap();
        assert!(store
            .enqueue_and_clear_draft("node", &pending(text))
            .is_err());
        assert_eq!(
            store
                .get::<String>("node", "draft:chat")
                .unwrap()
                .as_deref(),
            Some("next draft")
        );
        assert_eq!(
            store
                .0
                .lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM blobs", [], |row| row.get::<_, u64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn conversion_keeps_readable_comments_and_respects_attachment_limits() {
        let root = tempfile::tempdir().unwrap();
        let store = ClientStore::open(root.path()).unwrap();
        let content = comments::compose_document(
            &"x".repeat(MAX_MESSAGE_TEXT_BYTES + 1),
            &[comments::DraftComment {
                id: "comment".into(),
                source: comments::CommentSource {
                    quote: "quoted text".into(),
                    ..Default::default()
                },
                comment: "keep this comment".into(),
            }],
            &[comments::TextAttachment {
                id: "text".into(),
                name: "notes.md".into(),
                content: "keep these notes".into(),
            }],
        );
        let queued = store
            .enqueue_and_clear_draft("node", &pending(content.clone()))
            .unwrap();
        let file = files::decode(&queued.content).unwrap().1.remove(0);
        assert_eq!(
            String::from_utf8(
                store
                    .blob("node", &format!("upload:{}", file.id))
                    .unwrap()
                    .unwrap()
            )
            .unwrap(),
            comments::display_text(&content)
        );
        let references: Vec<_> = (0..files::MAX_FILES)
            .map(|i| FileRef {
                id: format!("file-{i}"),
                name: "existing.txt".into(),
                byte_len: 0,
                content_root: zork_mesh::content_root(b""),
            })
            .collect();
        store.put("node", "draft:other", &"keep draft").unwrap();
        let mut next = pending(files::compose(
            &"x".repeat(MAX_MESSAGE_TEXT_BYTES + 1),
            &references,
        ));
        next.request_id = "other".into();
        next.session_id = "other".into();
        assert!(store.enqueue_and_clear_draft("node", &next).is_err());
        next.content = "x".repeat(files::MAX_FILE_BYTES + 1);
        assert!(store.enqueue_and_clear_draft("node", &next).is_err());
        assert_eq!(
            store
                .get::<String>("node", "draft:other")
                .unwrap()
                .as_deref(),
            Some("keep draft")
        );
        assert_eq!(store.outbox("node").unwrap().len(), 1);
    }
}
