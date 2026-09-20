use super::{Device, Observable, Subscription};
use crate::comments::{self, DraftComment};
use std::sync::Arc;

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct Draft {
    pub text: String,
    pub comments: Vec<DraftComment>,
    pub attachments: Vec<comments::TextAttachment>,
    pub files: Vec<zork_client_types::files::FileRef>,
}
pub const TEXT_ATTACHMENT_LIMIT: usize = 24 * 1024;

#[derive(serde::Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum DraftAction {
    Edit { text: String },
    PutComment { comment: DraftComment },
    RemoveComment { id: String },
    AttachText { name: String, bytes: Vec<u8> },
    RemoveAttachment { id: String },
}
impl Draft {
    pub(crate) fn submission(&self, text: &str) -> String {
        let verbatim = comments::draft_document(text, &self.comments, &self.attachments);
        let body = if verbatim.len() > zork_client_types::chat::MAX_MESSAGE_TEXT_BYTES
            && !verbatim.trim().is_empty()
        {
            verbatim
        } else {
            comments::compose_document(text, &self.comments, &self.attachments)
        };
        zork_client_types::files::compose(&body, &self.files)
    }

    pub(crate) fn decode(raw: String) -> Self {
        let (raw, files) = zork_client_types::files::decode(&raw).unwrap_or((raw, vec![]));
        let (text, comments, attachments) =
            comments::decode_document(&raw).unwrap_or((raw, vec![], vec![]));
        Self {
            text,
            comments,
            attachments,
            files,
        }
    }
    pub(crate) fn encoded(&self) -> String {
        zork_client_types::files::compose(
            &comments::draft_document(&self.text, &self.comments, &self.attachments),
            &self.files,
        )
    }
    pub(crate) fn apply(&mut self, session: &str, action: DraftAction) -> anyhow::Result<()> {
        crate::valid_session(session)?;
        match action {
            DraftAction::Edit { text } => self.text = text,
            DraftAction::PutComment { comment } => {
                anyhow::ensure!(
                    comment.source.session_id == session,
                    "comment belongs to another conversation"
                );
                if let Some(old) = self.comments.iter_mut().find(|c| c.id == comment.id) {
                    *old = comment;
                } else {
                    self.comments.push(comment);
                }
            }
            DraftAction::RemoveComment { id } => self.comments.retain(|c| c.id != id),
            DraftAction::AttachText { name, bytes } => {
                anyhow::ensure!(
                    bytes.len() <= TEXT_ATTACHMENT_LIMIT,
                    "文本附件最大支持 24 KiB"
                );
                anyhow::ensure!(self.attachments.len() < 4, "一次最多添加 4 个文本附件");
                let content = String::from_utf8(bytes)
                    .map_err(|_| anyhow::anyhow!("文本附件需要使用 UTF-8 编码"))?;
                self.attachments.push(comments::TextAttachment {
                    id: ulid::Ulid::new().to_string(),
                    name: name.chars().take(128).collect(),
                    content,
                });
            }
            DraftAction::RemoveAttachment { id } => self.attachments.retain(|a| a.id != id),
        }
        crate::valid_content(&self.encoded(), true)
    }
}
impl Device {
    pub fn edit_draft_action(&self, session: &str, action: DraftAction) -> anyhow::Result<()> {
        let _serial = self.draft_gate.lock().unwrap();
        let mut draft = self.draft(session).as_ref().clone();
        draft.apply(session, action)?;
        self.save_draft_state(session, draft)
    }
    fn draft_state(&self, session: &str) -> Arc<Observable<Draft>> {
        let mut drafts = self.drafts.lock().unwrap();
        drafts
            .entry(session.to_owned())
            .or_insert_with(|| {
                let raw = self
                    .cache
                    .as_ref()
                    .and_then(|(store, node)| {
                        store
                            .get::<String>(node, &format!("draft:{session}"))
                            .ok()
                            .flatten()
                    })
                    .unwrap_or_default();
                let (raw, files) = zork_client_types::files::decode(&raw).unwrap_or((raw, vec![]));
                let draft = match comments::decode_document(&raw) {
                    Some((text, comments, attachments)) => Draft {
                        text,
                        comments,
                        attachments,
                        files,
                    },
                    None => Draft {
                        text: raw,
                        comments: self
                            .cache
                            .as_ref()
                            .and_then(|(store, node)| {
                                store
                                    .get(node, &format!("draft-comments:{session}"))
                                    .ok()
                                    .flatten()
                            })
                            .unwrap_or_default(),
                        attachments: vec![],
                        files,
                    },
                };
                Arc::new(Observable::new(draft))
            })
            .clone()
    }
    pub(super) fn publish_cleared_draft(&self, session: &str) {
        self.draft_state(session).publish(Draft::default());
    }
    pub fn draft(&self, session: &str) -> Arc<Draft> {
        self.draft_state(session).read()
    }
    pub fn subscribe_draft(&self, session: &str) -> Subscription<Draft> {
        self.draft_state(session).subscribe()
    }
    pub(super) fn save_draft_state(&self, session: &str, draft: Draft) -> anyhow::Result<()> {
        let state = self.draft_state(session);
        if *state.read() == draft {
            return Ok(());
        }
        if let Some((store, node)) = &self.cache {
            let content = zork_client_types::files::compose(
                &comments::draft_document(&draft.text, &draft.comments, &draft.attachments),
                &draft.files,
            );
            store.put(node, &format!("draft:{session}"), &content)?;
        }
        state.publish(draft);
        Ok(())
    }
    /// Text editing is distinct from cursor/selection updates. Idempotent edits
    /// neither write storage nor wake consumers.
    pub(crate) fn edit_document(&self, session: &str, draft: Draft) -> anyhow::Result<()> {
        let _serial = self.draft_gate.lock().unwrap();
        crate::valid_session(session)?;
        self.save_draft_state(session, draft)
    }
    pub(super) fn reload_draft(&self, session: &str) -> anyhow::Result<()> {
        if let Some((store, node)) = &self.cache {
            let raw = store
                .get::<String>(node, &format!("draft:{session}"))?
                .unwrap_or_default();
            let (raw, files) = zork_client_types::files::decode(&raw).unwrap_or((raw, vec![]));
            let (text, comments, attachments) =
                comments::decode_document(&raw).unwrap_or((raw, vec![], vec![]));
            self.draft_state(session).publish(Draft {
                text,
                comments,
                attachments,
                files,
            });
        }
        Ok(())
    }
    pub fn edit_draft(&self, session: &str, text: String) -> anyhow::Result<()> {
        let _serial = self.draft_gate.lock().unwrap();
        crate::valid_session(session)?;
        let mut draft = self.draft(session).as_ref().clone();
        draft.text = text;
        self.save_draft_state(session, draft)
    }
    pub fn put_comment(&self, session: &str, comment: DraftComment) -> anyhow::Result<()> {
        let _serial = self.draft_gate.lock().unwrap();
        crate::valid_session(session)?;
        anyhow::ensure!(
            comment.source.session_id == session,
            "comment belongs to another conversation"
        );
        let mut draft = self.draft(session).as_ref().clone();
        if let Some(existing) = draft.comments.iter_mut().find(|c| c.id == comment.id) {
            *existing = comment;
        } else {
            draft.comments.push(comment);
        }
        self.save_draft_state(session, draft)
    }
    pub fn remove_comment(&self, session: &str, id: &str) -> anyhow::Result<()> {
        let _serial = self.draft_gate.lock().unwrap();
        crate::valid_session(session)?;
        let mut draft = self.draft(session).as_ref().clone();
        draft.comments.retain(|comment| comment.id != id);
        self.save_draft_state(session, draft)
    }
    pub fn clear_draft(&self, session: &str) -> anyhow::Result<()> {
        let _serial = self.draft_gate.lock().unwrap();
        self.save_draft_state(session, Draft::default())
    }
}
