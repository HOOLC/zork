use zork_ui::components::message::MessageDocument;

/// A sent comment batch's reply documents (per pair) and extra text.
pub type CommentDocuments = (
    Vec<std::rc::Rc<MessageDocument>>,
    Option<std::rc::Rc<MessageDocument>>,
);

#[derive(Default)]
pub struct MessageRenderDocument {
    pub(crate) interaction: std::cell::RefCell<Option<Box<super::interaction::Rendered>>>,
    document: std::cell::OnceCell<MessageDocument>,
    files: std::cell::OnceCell<std::sync::Arc<Vec<zork_client_core::files::FileRef>>>,
    /// Text selection and passage marks search: the plain text, or for a
    /// comment batch its quotable text (replies then extra text).
    selection_text: std::cell::OnceCell<gpui::SharedString>,
    comments: std::cell::OnceCell<Option<CommentDocuments>>,
}
impl std::ops::Deref for MessageRenderDocument {
    type Target = std::cell::OnceCell<MessageDocument>;
    fn deref(&self) -> &Self::Target {
        &self.document
    }
}
impl MessageRenderDocument {
    pub fn files(&self, content: &str) -> &[zork_client_core::files::FileRef] {
        self.cached_files(content).as_slice()
    }
    pub fn shared_files(
        &self,
        content: &str,
    ) -> std::sync::Arc<Vec<zork_client_core::files::FileRef>> {
        self.cached_files(content).clone()
    }
    fn cached_files(
        &self,
        content: &str,
    ) -> &std::sync::Arc<Vec<zork_client_core::files::FileRef>> {
        self.files.get_or_init(|| {
            std::sync::Arc::new({
                zork_client_core::files::decode(content)
                    .map(|(_, mut files)| {
                        files.truncate(zork_client_core::files::MAX_FILES);
                        files
                    })
                    .unwrap_or_default()
            })
        })
    }
    pub fn document(&self, role: &crate::api::Role, content: &str) -> &MessageDocument {
        self.document
            .get_or_init(|| message_document(role, content))
    }
    /// The text quotes are taken from and passages are marked in.
    pub fn selection_text(&self, role: &crate::api::Role, content: &str) -> gpui::SharedString {
        self.selection_text
            .get_or_init(|| {
                if *role == crate::api::Role::User
                    && zork_client_core::comments::decode_batch(content).is_some()
                {
                    zork_client_core::comments::quotable_text(content).into()
                } else {
                    self.document(role, content).shared_plain_text()
                }
            })
            .clone()
    }
    /// Reply documents of a sent comment batch, `None` for other messages.
    pub fn comment_documents(&self, role: &crate::api::Role, content: &str) -> Option<&CommentDocuments> {
        self.comments
            .get_or_init(|| {
                if *role != crate::api::Role::User {
                    return None;
                }
                zork_client_core::comments::decode_batch(content).map(|batch| {
                    (
                        batch
                            .pairs
                            .iter()
                            .map(|pair| std::rc::Rc::new(MessageDocument::plain(pair.reply.trim())))
                            .collect(),
                        (!batch.extra_text.trim().is_empty())
                            .then(|| std::rc::Rc::new(MessageDocument::plain(batch.extra_text.trim()))),
                    )
                })
            })
            .as_ref()
    }
}

pub fn message_document(role: &crate::api::Role, content: &str) -> MessageDocument {
    let body = zork_client_core::files::decode(content).map(|(text, _)| text);
    let text = crate::comments::display_text(body.as_deref().unwrap_or(content));
    match role {
        crate::api::Role::User => MessageDocument::plain(&text),
        crate::api::Role::Assistant => MessageDocument::parse(&text),
    }
}

/// Documents follow the core's ordered edits. Normal delivery never scans or
/// reallocates a document cell for every message in the conversation.
#[derive(Default)]
pub struct TranscriptRenderCache {
    source: crate::transcript::Transcript,
    documents: zork_client_core::observe::List<MessageRenderDocument>,
}
impl TranscriptRenderCache {
    #[cfg(feature = "headless-bench")]
    pub fn parsed_document_count(&self) -> usize {
        self.documents
            .iter()
            .filter(|document| document.get().is_some())
            .count()
    }

    pub fn prepare(
        &mut self,
        source: &crate::transcript::Transcript,
    ) -> zork_client_core::observe::List<MessageRenderDocument> {
        if source.ptr_eq(&self.source) {
            return self.documents.clone();
        }
        self.reset(source);
        self.documents.clone()
    }

    fn same_document(
        a: &crate::transcript::TranscriptLine,
        b: &crate::transcript::TranscriptLine,
    ) -> bool {
        let (
            crate::transcript::TranscriptLine::Message {
                role: ar,
                content: ac,
                ..
            },
            crate::transcript::TranscriptLine::Message {
                role: br,
                content: bc,
                ..
            },
        ) = (a, b);
        ar == br && ac == bc
    }

    /// A reset is exceptional (first attach, changed source or journal overrun).
    /// Preserve unchanged ends, including separate cells for anonymous duplicates.
    fn reset(&mut self, source: &crate::transcript::Transcript) {
        let prefix = source
            .iter()
            .zip(self.source.iter())
            .take_while(|(a, b)| a == b)
            .count();
        let suffix = source
            .iter()
            .skip(prefix)
            .rev()
            .zip(self.source.iter().skip(prefix).rev())
            .take_while(|(a, b)| a == b)
            .count();
        let documents = zork_client_core::observe::List::from_shared((0..source.len()).map(|i| {
            if i < prefix {
                self.documents.shared(i).unwrap()
            } else if i >= source.len() - suffix {
                self.documents
                    .shared(self.source.len() - (source.len() - i))
                    .unwrap()
            } else {
                std::sync::Arc::new(MessageRenderDocument::default())
            }
        }));
        self.source = source.clone();
        self.documents = documents;
    }

    pub fn apply(
        &mut self,
        source: &crate::transcript::Transcript,
        edits: &[zork_client_core::observe::ListEdit<crate::transcript::TranscriptLine>],
        reset: bool,
    ) {
        if reset {
            self.reset(source);
            return;
        }
        for edit in edits {
            if edit.remove.end > self.source.len() || edit.remove.start > edit.remove.end {
                self.reset(source);
                return;
            }
            let documents = zork_client_core::observe::List::from_shared(
                edit.insert.iter().enumerate().map(|(offset, line)| {
                    if offset < edit.remove.len()
                        && Self::same_document(&self.source[edit.remove.start + offset], line)
                    {
                        self.documents.shared(edit.remove.start + offset).unwrap()
                    } else {
                        std::sync::Arc::new(MessageRenderDocument::default())
                    }
                }),
            );
            self.documents.splice(edit.remove.clone(), documents);
            edit.apply(&mut self.source);
        }
        debug_assert_eq!(self.source.len(), source.len());
        self.source = source.clone();
    }
}

#[cfg(test)]
mod transcript_cache_tests {
    use super::*;
    use crate::{api::Role, transcript::TranscriptLine};
    use std::sync::Arc;
    fn line(text: &str) -> TranscriptLine {
        TranscriptLine::Message {
            role: Role::Assistant,
            content: text.into(),
            metadata: Default::default(),
        }
    }
    #[test]
    fn user_markdown_is_literal_and_assistant_markdown_is_formatted() {
        let source =
            "# 标题\n\n**中文🐈** `code` [链接](https://example.com)\n\n```rust\nlet x = 1;\n```\n";
        let user = message_document(&Role::User, source);
        assert_eq!(user.plain_text(), source);
        assert!(matches!(
            user.blocks(),
            [zork_ui::components::message::MessageBlock::Paragraph(_)]
        ));
        let assistant = message_document(&Role::Assistant, source);
        assert!(!assistant.plain_text().contains("**"));
        assert!(!assistant.plain_text().contains("```"));
        assert!(assistant.plain_text().contains("中文🐈"));
    }
    #[test]
    fn long_message_document_keeps_the_ending() {
        let source = format!("{}END-OF-MESSAGE", "line\n".repeat(100));
        let cached = MessageRenderDocument::default();
        let document = cached.document(&Role::Assistant, &source);
        assert!(document.plain_text().ends_with("END-OF-MESSAGE"));
        assert!(!document.is_truncated());
    }
    #[test]
    fn scrolling_reuses_parsing_and_history_changes_keep_the_right_document() {
        let mut cache = TranscriptRenderCache::default();
        let source: crate::transcript::Transcript = vec![line("**first**"), line("second")].into();
        let docs = cache.prepare(&source);
        assert_eq!(
            docs[0]
                .get_or_init(|| MessageDocument::parse("**first**"))
                .plain_text(),
            "first"
        );
        for _ in 0..120 {
            assert!(docs.ptr_eq(&cache.prepare(&source)));
        }
        let prepended =
            cache.prepare(&vec![line("older"), line("**first**"), line("second")].into());
        assert!(Arc::ptr_eq(
            &prepended.shared(1).unwrap(),
            &docs.shared(0).unwrap()
        ));
        let changed = cache.prepare(
            &vec![
                line("older"),
                line("**edited**"),
                line("second"),
                line("new"),
            ]
            .into(),
        );
        assert!(changed[1].get().is_none());
        assert_eq!(
            changed[1]
                .get_or_init(|| MessageDocument::parse("**edited**"))
                .plain_text(),
            "edited"
        );
    }

    #[test]
    fn sparse_core_edits_keep_unrelated_documents_and_metadata_updates_keep_parsing() {
        use zork_client_core::observe::{List, ListEdit};
        let mut cache = TranscriptRenderCache::default();
        let mut source: crate::transcript::Transcript =
            (0..100_000).map(|i| line(&format!("record {i}"))).collect();
        let before = cache.prepare(&source);
        before[50_000].get_or_init(|| MessageDocument::parse("record 50000"));
        let mut metadata_only = source[50_000].clone();
        let TranscriptLine::Message { metadata, .. } = &mut metadata_only;
        metadata.author_name = Some("updated author".into());
        let edits = vec![
            ListEdit {
                remove: 7..8,
                insert: vec![line("edited")].into(),
            },
            ListEdit {
                remove: 50_000..50_001,
                insert: vec![metadata_only].into(),
            },
            ListEdit {
                remove: 100_000..100_000,
                insert: vec![line("appended")].into(),
            },
        ];
        for edit in &edits {
            assert!(edit.apply(&mut source));
        }
        cache.apply(&source, &edits, false);
        let after = cache.prepare(&source);
        assert!(Arc::ptr_eq(
            &before.shared(80_000).unwrap(),
            &after.shared(80_000).unwrap()
        ));
        assert!(Arc::ptr_eq(
            &before.shared(50_000).unwrap(),
            &after.shared(50_000).unwrap()
        ));
        assert_eq!(after[50_000].get().unwrap().plain_text(), "record 50000");
        assert!(!Arc::ptr_eq(
            &before.shared(7).unwrap(),
            &after.shared(7).unwrap()
        ));
        let removal = ListEdit {
            remove: 0..7,
            insert: List::new(),
        };
        assert!(removal.apply(&mut source));
        cache.apply(&source, &[removal], false);
        assert!(Arc::ptr_eq(
            &after.shared(80_000).unwrap(),
            &cache.prepare(&source).shared(79_993).unwrap()
        ));
    }
}
