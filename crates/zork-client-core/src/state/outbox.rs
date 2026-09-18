use super::{Device, Subscription};
use crate::{delivery::DeliveryPump, store::QueuedMessage};
use std::{collections::HashMap, sync::Arc};

#[derive(Clone, Default, PartialEq)]
pub struct Outbox {
    pub items: Arc<Vec<QueuedMessage>>,
    pub by_message_id: Arc<HashMap<String, usize>>,
    pub phases: Arc<HashMap<String, String>>,
}
impl Device {
    pub fn recover_outbox(&self) -> Arc<Outbox> {
        self.reload_outbox();
        self.outbox()
    }
    pub fn outbox(&self) -> Arc<Outbox> {
        self.outbox.read()
    }
    pub fn subscribe_outbox(&self) -> Subscription<Outbox> {
        self.outbox.subscribe()
    }
    pub fn start_delivery(self: &Arc<Self>) {
        let Some((store, node)) = &self.cache else {
            return;
        };
        let mut task = self.delivery_task.lock().unwrap();
        if task.is_some() {
            return;
        }
        self.reload_outbox();
        let mut changes = store.delivery_events();
        let pump = DeliveryPump::start(self.client.clone(), store.clone(), node.clone());
        let weak = Arc::downgrade(self);
        *task = Some(self.client.spawn(async move {
            let _pump = pump;
            // These are the shared core controllers, not UI pages. Keep their
            // source feed alive while a local message still awaits an echo.
            let mut receiving = HashMap::new();
            loop {
                changes.borrow_and_update();
                let Some(device) = weak.upgrade() else {
                    return;
                };
                device.reload_outbox();
                let pending = device.outbox();
                let sessions: std::collections::HashSet<_> = pending
                    .items
                    .iter()
                    .map(|message| message.session_id.clone())
                    .collect();
                receiving.retain(|session, _| sessions.contains(session));
                for session in sessions {
                    receiving.entry(session.clone()).or_insert_with(|| {
                        let conversation = device.conversation(&session);
                        conversation.start();
                        conversation
                    });
                }
                for conversation in device
                    .conversations
                    .lock()
                    .unwrap()
                    .values()
                    .filter_map(std::sync::Weak::upgrade)
                    .collect::<Vec<_>>()
                {
                    conversation.sync_interactions();
                }
                drop(device);
                if changes.changed().await.is_err() {
                    return;
                }
            }
        }));
    }
    pub(super) fn reload_outbox(&self) {
        let serial = self.outbox_gate.lock().unwrap();
        let items = self
            .cache
            .as_ref()
            .and_then(|(store, node)| store.outbox(node).ok())
            .unwrap_or_default();
        let by_message_id = items
            .iter()
            .enumerate()
            .map(|(i, m)| (format!("client-{}-{}", m.session_id, m.request_id), i))
            .collect();
        let phases = items
            .iter()
            .map(|m| (m.request_id.clone(), m.delivery_status().to_owned()))
            .collect();
        let changed = self.outbox.publish(Outbox {
            items: Arc::new(items),
            by_message_id: Arc::new(by_message_id),
            phases: Arc::new(phases),
        });
        drop(serial);
        if changed {
            for conversation in self
                .conversations
                .lock()
                .unwrap()
                .values()
                .filter_map(std::sync::Weak::upgrade)
                .collect::<Vec<_>>()
            {
                conversation.sync_outbox();
            }
        }
    }
    /// Commit the explicit send and source-draft clearing together. Core then
    /// publishes both projections; the UI never owns an optimistic outbox copy.
    pub fn enqueue(&self, session: &str, content: String) -> anyhow::Result<QueuedMessage> {
        let _serial = self.draft_gate.lock().unwrap();
        self.enqueue_locked(session, content)
    }
    /// Compose the canonical draft, including comments and attachments, inside
    /// the same serialized command that commits its delivery and clears it.
    pub fn submit_draft(&self, session: &str, text: &str) -> anyhow::Result<Option<QueuedMessage>> {
        crate::valid_session(session)?;
        let _serial = self.draft_gate.lock().unwrap();
        let draft = self.draft(session);
        let content = draft.submission(text);
        if content.is_empty() {
            return Ok(None);
        }
        self.enqueue_locked(session, content).map(Some)
    }
    fn enqueue_locked(&self, session: &str, content: String) -> anyhow::Result<QueuedMessage> {
        crate::valid_session(session)?;
        crate::valid_content(&content, false)?;
        if let Some(summary) = self
            .snapshot()
            .sessions
            .iter()
            .find(|s| s.session_id == session)
        {
            anyhow::ensure!(
                crate::conversation::can_send(summary),
                "This conversation cannot accept messages"
            );
        }
        let (store, node) = self
            .cache
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("persistent delivery is unavailable"))?;
        let message = QueuedMessage {
            request_id: ulid::Ulid::new().to_string(),
            session_id: session.into(),
            content,
            attempted: false,
            sent_at_ms: crate::store::delivery_now_ms(),
            ..Default::default()
        };
        let message = store.enqueue_and_clear_draft(node, &message)?;
        self.publish_cleared_draft(session);
        self.reload_outbox();
        Ok(message)
    }
    pub fn withdraw_delivery(&self, id: &str) -> anyhow::Result<Option<QueuedMessage>> {
        let (store, node) = self
            .cache
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("persistent delivery is unavailable"))?;
        let _serial = self.draft_gate.lock().unwrap();
        let message = store.withdraw_to_draft(node, id)?;
        if let Some(message) = &message {
            self.reload_draft(&message.session_id)?;
            if let Some(conversation) = self
                .conversations
                .lock()
                .unwrap()
                .get(&message.session_id)
                .and_then(std::sync::Weak::upgrade)
            {
                conversation.remove_failed(id);
            }
        }
        self.reload_outbox();
        Ok(message)
    }
    pub fn retry_delivery(&self, id: &str) -> anyhow::Result<()> {
        let (store, node) = self
            .cache
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("persistent delivery is unavailable"))?;
        store.retry_delivery(node, id)?;
        self.reload_outbox();
        Ok(())
    }
    pub fn delete_failed_delivery(&self, id: &str) -> anyhow::Result<()> {
        let (store, node) = self
            .cache
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("persistent delivery is unavailable"))?;
        let session = store
            .outbox(node)?
            .iter()
            .find(|m| m.request_id == id)
            .map(|m| m.session_id.clone());
        store.delete_failed(node, id)?;
        if let Some(session) = session {
            if let Some(conversation) = self
                .conversations
                .lock()
                .unwrap()
                .get(&session)
                .and_then(std::sync::Weak::upgrade)
            {
                conversation.remove_failed(id);
            }
        }
        self.reload_outbox();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn long_messages_become_files_and_preserve_whitespace_and_existing_attachments() {
        use zork_client_types::{
            chat::MAX_MESSAGE_TEXT_BYTES,
            files::{self, FileRef},
        };
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::store::ClientStore::open(root.path()).unwrap());
        let device = Device::open(
            Arc::new(crate::api::StationClient::new("http://127.0.0.1:9", None)),
            Some((store.clone(), "node".into())),
            false,
        );
        let text = format!("  {}\n", "é".repeat(MAX_MESSAGE_TEXT_BYTES / 2 + 1));
        device
            .edit_draft_action(
                "chat",
                super::super::DraftAction::Edit { text: text.clone() },
            )
            .unwrap();
        assert_eq!(device.draft("chat").text, text);
        let existing = device
            .attach_file("chat", "existing.txt", b"keep this file")
            .unwrap();
        let queued = device.submit_draft("chat", &text).unwrap().unwrap();
        let (body, attachments) = files::decode(&queued.content).unwrap();
        assert!(body.is_empty());
        assert_eq!(attachments.len(), 2);
        assert_eq!(attachments[0], existing);
        assert_eq!(attachments[1].name, "message.txt");
        assert_eq!(
            store
                .blob("node", &format!("upload:{}", attachments[1].id))
                .unwrap()
                .unwrap(),
            text.as_bytes()
        );
        assert_eq!(store.outbox("node").unwrap(), vec![queued]);
        assert_eq!(*device.draft("chat"), super::super::Draft::default());
        let at_limit = "x".repeat(MAX_MESSAGE_TEXT_BYTES);
        assert_eq!(
            device.enqueue("chat", at_limit.clone()).unwrap().content,
            at_limit
        );
        let files = vec![FileRef {
            id: "file-test".into(),
            name: "test.txt".into(),
            byte_len: 1,
            content_root: "a".repeat(64),
        }];
        let body = "\n".repeat(MAX_MESSAGE_TEXT_BYTES);
        let encoded = files::compose(&body, &files);
        assert!(
            encoded.len() > 64 * 1024,
            "fixture includes JSON escaping and attachment metadata"
        );
        assert_eq!(
            device.enqueue("chat", encoded.clone()).unwrap().content,
            encoded
        );
        assert_eq!(store.outbox("node").unwrap().len(), 3);
    }

    #[test]
    fn submission_uses_canonical_comments_and_attachments_and_clears_atomically() {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::store::ClientStore::open(root.path()).unwrap());
        let device = Device::open(
            Arc::new(crate::api::StationClient::new("http://127.0.0.1:9", None)),
            Some((store.clone(), "node".into())),
            true,
        );
        let comment = crate::comments::DraftComment {
            id: "comment".into(),
            source: crate::comments::CommentSource {
                session_id: "chat".into(),
                quote: "selected text".into(),
                ..Default::default()
            },
            comment: "explain".into(),
        };
        let attachment = crate::comments::TextAttachment {
            id: "attachment".into(),
            name: "notes.txt".into(),
            content: "keep this attachment".into(),
        };
        device
            .edit_document(
                "chat",
                super::super::Draft {
                    text: "old input".into(),
                    comments: vec![comment.clone()],
                    attachments: vec![attachment.clone()],
                    files: vec![],
                },
            )
            .unwrap();
        let queued = device
            .submit_draft("chat", "current input")
            .unwrap()
            .unwrap();
        let (text, comments, attachments) =
            crate::comments::decode_document(&queued.content).unwrap();
        assert_eq!(text, "current input");
        assert_eq!(comments, vec![comment]);
        assert_eq!(attachments, vec![attachment]);
        assert_eq!(*device.draft("chat"), super::super::Draft::default());
        assert_eq!(store.outbox("node").unwrap().len(), 1);
        assert!(device.submit_draft("chat", " ").unwrap().is_none());
        assert_eq!(store.outbox("node").unwrap().len(), 1);
    }
}

impl Device {
    pub(crate) fn apply_message_links(
        &self,
        session: &str,
        reset: bool,
        links: Vec<crate::pages::ConversationPage>,
    ) {
        self.commit(|state| {
            if state.revoked {
                return;
            }
            let links: Vec<_> = links
                .into_iter()
                .filter(|link| {
                    (reset && link.session_id == session)
                        || state
                            .pages
                            .references
                            .iter()
                            .find(|page| {
                                page.session_id == link.session_id && page.page.url == link.page.url
                            })
                            .is_none_or(|existing| {
                                existing.id.starts_with("markdown-")
                                    && (&existing.created_at, &existing.message_id)
                                        < (&link.created_at, &link.message_id)
                            })
                })
                .collect();
            if links.is_empty()
                && (!reset
                    || !state
                        .pages
                        .references
                        .iter()
                        .any(|page| page.session_id == session && page.id.starts_with("markdown-")))
            {
                return;
            }
            let pages = Arc::make_mut(&mut state.pages);
            if reset {
                pages
                    .references
                    .retain(|page| page.session_id != session || !page.id.starts_with("markdown-"));
            }
            for link in links {
                if let Some(existing) = pages.references.iter_mut().find(|page| {
                    page.session_id == link.session_id && page.page.url == link.page.url
                }) {
                    if existing.id.starts_with("markdown-")
                        && (&existing.created_at, &existing.message_id)
                            < (&link.created_at, &link.message_id)
                    {
                        *existing = link;
                    }
                } else {
                    pages.references.push(link);
                }
            }
        });
    }
}
