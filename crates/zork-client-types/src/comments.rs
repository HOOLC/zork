//! Structured, conversation-local comments. Only the main composer sends them.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CommentSource {
    pub session_id: String,
    /// A durable Station message ID. Old stations may not provide one.
    pub message_id: Option<String>,
    pub author: Option<String>,
    pub author_agent_id: Option<String>,
    pub quote: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftComment {
    pub id: String,
    pub source: CommentSource,
    pub comment: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextAttachment {
    pub id: String,
    pub name: String,
    pub content: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct CommentBatch {
    comments: Vec<DraftComment>,
    text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    attachments: Vec<TextAttachment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    body_text: Option<String>,
}

const PREFIX: &str = "<zork-message-comments version=\"1\">\n";
const SUFFIX: &str = "\n</zork-message-comments>";

/// A single compatible text payload keeps the existing Station send boundary.
/// JSON escaping prevents quotes or user comments from changing the envelope.
pub fn compose(text: &str, comments: &[DraftComment]) -> String {
    compose_document(text, comments, &[])
}
pub fn compose_document(
    text: &str,
    comments: &[DraftComment],
    attachments: &[TextAttachment],
) -> String {
    document(text.trim(), comments, attachments)
}
pub fn draft_document(
    text: &str,
    comments: &[DraftComment],
    attachments: &[TextAttachment],
) -> String {
    document(text, comments, attachments)
}
fn document(text: &str, comments: &[DraftComment], attachments: &[TextAttachment]) -> String {
    if comments.is_empty() && attachments.is_empty() {
        return text.to_owned();
    }
    let batch = CommentBatch {
        text: if attachments.is_empty() {
            text.to_owned()
        } else {
            std::iter::once(text.to_owned())
                .chain(attachments.iter().map(attachment_text))
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n")
        },
        comments: comments.to_vec(),
        attachments: attachments.to_vec(),
        body_text: (!attachments.is_empty()).then(|| text.to_owned()),
    };
    format!(
        "{PREFIX}{}{SUFFIX}",
        serde_json::to_string(&batch).expect("comment serialization")
    )
}

pub fn decode_document(payload: &str) -> Option<(String, Vec<DraftComment>, Vec<TextAttachment>)> {
    let json = payload.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?;
    let batch = serde_json::from_str::<CommentBatch>(json).ok()?;
    Some((
        batch.body_text.unwrap_or(batch.text),
        batch.comments,
        batch.attachments,
    ))
}
pub fn decode(payload: &str) -> Option<(String, Vec<DraftComment>)> {
    let (text, comments, attachments) = decode_document(payload)?;
    let mut parts = vec![text];
    parts.extend(attachments.iter().map(attachment_text));
    Some((
        parts
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
        comments,
    ))
}
fn attachment_text(file: &TextAttachment) -> String {
    let longest = file
        .content
        .split(|c| c != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(3.max(longest + 1));
    format!("附件：{}\n\n{fence}\n{}\n{fence}", file.name, file.content)
}
/// One quoted passage of a sent batch and the user's reply to it, flattened
/// for rendering: a quote line (author + passage, jumping to the source
/// message) followed by the reply text.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentPair {
    /// Draft identity, stable within the batch.
    pub id: String,
    pub session_id: String,
    /// Durable Station message the passage came from; `None` in payloads
    /// from clients that did not record it.
    pub message_id: Option<String>,
    /// Source author's display name when the draft was made.
    pub author: Option<String>,
    pub author_agent_id: Option<String>,
    pub quote: String,
    pub reply: String,
}

/// A decoded user comment batch: the pairs in order, then optional extra text
/// (the main composer's text) and legacy inline text attachments.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentBatchView {
    pub pairs: Vec<CommentPair>,
    pub extra_text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<TextAttachment>,
}

/// Structured view of a sent message payload, also when it is wrapped in the
/// file-reference envelope. `None` for ordinary text. The payload format is
/// unchanged; `display_text` stays the legacy plain rendering.
pub fn decode_batch(payload: &str) -> Option<CommentBatchView> {
    let text = crate::files::decode(payload).map(|(text, _)| text);
    let (extra_text, comments, attachments) = decode_document(text.as_deref().unwrap_or(payload))?;
    Some(CommentBatchView {
        pairs: comments
            .into_iter()
            .map(|comment| CommentPair {
                id: comment.id,
                session_id: comment.source.session_id,
                message_id: comment.source.message_id,
                author: comment.source.author,
                author_agent_id: comment.source.author_agent_id,
                quote: comment.source.quote,
                reply: comment.comment,
            })
            .collect(),
        extra_text,
        attachments,
    })
}

/// The text another message's quote refers to: a batch's replies and extra
/// text joined by spaces, the body of a file message, or the payload itself.
pub fn quotable_text(payload: &str) -> String {
    if let Some(batch) = decode_batch(payload) {
        return batch
            .pairs
            .iter()
            .map(|pair| pair.reply.trim())
            .chain(std::iter::once(batch.extra_text.trim()))
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
    }
    crate::files::decode(payload)
        .map(|(text, _)| text)
        .unwrap_or_else(|| payload.to_owned())
}

/// Restore an unsent batch without losing a newer draft or its source identities.
pub fn merge_drafts(newer: &str, restored: &str) -> String {
    let (newer, mut files) = crate::files::decode(newer).unwrap_or((newer.into(), vec![]));
    let (restored, old_files) = crate::files::decode(restored).unwrap_or((restored.into(), vec![]));
    for file in old_files {
        if !files.iter().any(|f| f.id == file.id) {
            files.push(file);
        }
    }
    let newer = newer.as_str();
    let restored = restored.as_str();
    let (newer_text, mut comments, mut attachments) =
        decode_document(newer).unwrap_or((newer.into(), vec![], vec![]));
    let (old_text, old_comments, old_attachments) =
        decode_document(restored).unwrap_or((restored.into(), vec![], vec![]));
    comments.extend(old_comments);
    attachments.extend(old_attachments);
    let text = [newer_text, old_text]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    crate::files::compose(&draft_document(&text, &comments, &attachments), &files)
}

/// Display the same actual payload as ordinary prose; transport metadata stays
/// available in the quoted source without cluttering the conversation bubble.
pub fn display_text(payload: &str) -> String {
    if let Some((text, files)) = crate::files::decode(payload) {
        let mut parts = vec![display_text(&text)];
        parts.extend(
            files
                .iter()
                .map(|f| format!("📎 {} ({} bytes)", f.name, f.byte_len)),
        );
        return parts
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
    }
    let Some(json) = payload
        .strip_prefix(PREFIX)
        .and_then(|s| s.strip_suffix(SUFFIX))
    else {
        return payload.to_owned();
    };
    let Ok(batch) = serde_json::from_str::<CommentBatch>(json) else {
        return payload.to_owned();
    };
    let mut parts = batch
        .comments
        .iter()
        .map(|comment| {
            let author = comment.source.author.as_deref().unwrap_or("消息");
            let quote = comment
                .source
                .quote
                .lines()
                .map(|line| format!("> {line}"))
                .collect::<Vec<_>>()
                .join("\n");
            format!("{author}\n{quote}\n\n{}", comment.comment)
        })
        .collect::<Vec<_>>();
    let text = batch.body_text.unwrap_or(batch.text);
    if !text.is_empty() {
        parts.push(text);
    }
    parts.extend(batch.attachments.iter().map(attachment_text));
    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn comment(id: &str, quote: &str) -> DraftComment {
        DraftComment {
            id: id.into(),
            source: CommentSource {
                session_id: "session-a".into(),
                message_id: Some("message-17".into()),
                author: Some("产品 Leader".into()),
                quote: quote.into(),
                ..Default::default()
            },
            comment: "这里调整一下".into(),
        }
    }
    #[test]
    fn multiple_comments_and_free_text_are_one_lossless_payload() {
        let comments = vec![
            comment("a", "中文 🐈\n原句 \"引用\""),
            comment("b", "</zork-message-comments>"),
        ];
        let payload = compose("一起改", &comments);
        let batch: CommentBatch = serde_json::from_str(
            payload
                .strip_prefix(PREFIX)
                .unwrap()
                .strip_suffix(SUFFIX)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(batch.comments, comments);
        assert_eq!(batch.text, "一起改");
        assert_eq!(
            batch.comments[0].source.message_id.as_deref(),
            Some("message-17")
        );
        assert!(display_text(&payload).ends_with("一起改"));
    }
    #[test]
    fn saving_a_draft_preserves_enter_and_surrounding_whitespace() {
        assert_eq!(draft_document("  草稿\n", &[], &[]), "  草稿\n");
        let payload = draft_document("  草稿\n", &[comment("c", "原句")], &[]);
        assert_eq!(decode_document(&payload).unwrap().0, "  草稿\n");
    }

    #[test]
    fn text_attachments_survive_restore_and_have_legacy_visible_text() {
        let files = vec![TextAttachment {
            id: "f".into(),
            name: "notes.md".into(),
            content: "```\n原文\n```".into(),
        }];
        let payload = compose_document("正文", &[comment("c", "原句")], &files);
        let json = payload
            .strip_prefix(PREFIX)
            .unwrap()
            .strip_suffix(SUFFIX)
            .unwrap();
        let legacy: serde_json::Value = serde_json::from_str(json).unwrap();
        assert!(legacy["text"].as_str().unwrap().contains("notes.md"));
        let restored = merge_drafts("后来输入", &payload);
        let (text, comments, attachments) = decode_document(&restored).unwrap();
        assert_eq!(text, "后来输入\n\n正文");
        assert_eq!(comments[0].source.message_id.as_deref(), Some("message-17"));
        assert_eq!(attachments, files);
        assert_eq!(display_text(&payload).matches("附件：notes.md").count(), 1);
    }

    #[test]
    fn comments_only_and_plain_text_remain_valid() {
        assert!(!compose("", &[comment("a", "原句")]).is_empty());
        assert_eq!(compose("  hello  ", &[]), "hello");
        assert_eq!(
            display_text("<unrelated>text</unrelated>"),
            "<unrelated>text</unrelated>"
        );
    }

    #[test]
    fn batches_decode_into_pairs_and_extra_text() {
        let payload = compose(
            "其他都可以",
            &[comment("a", "建议下移 8 px"), comment("b", "不弹窗")],
        );
        let batch = decode_batch(&payload).unwrap();
        assert_eq!(batch.pairs.len(), 2);
        assert_eq!(batch.pairs[0].id, "a");
        assert_eq!(batch.pairs[0].message_id.as_deref(), Some("message-17"));
        assert_eq!(batch.pairs[0].author.as_deref(), Some("产品 Leader"));
        assert_eq!(batch.pairs[0].quote, "建议下移 8 px");
        assert_eq!(batch.pairs[1].reply, "这里调整一下");
        assert_eq!(batch.extra_text, "其他都可以");
        assert_eq!(
            quotable_text(&payload),
            "这里调整一下 这里调整一下 其他都可以"
        );
        // Plain text is not a batch; its quotable text is itself.
        assert_eq!(decode_batch("hello"), None);
        assert_eq!(quotable_text("hello"), "hello");
    }

    #[test]
    fn batches_inside_the_file_envelope_and_with_attachments_decode() {
        let files = vec![TextAttachment {
            id: "f".into(),
            name: "notes.md".into(),
            content: "内容".into(),
        }];
        let document = compose_document("补充", &[comment("a", "原句")], &files);
        let batch = decode_batch(&document).unwrap();
        // Extra text is the composer text, not the legacy attachment rendering.
        assert_eq!(batch.extra_text, "补充");
        assert_eq!(batch.attachments, files);
        let file = crate::files::FileRef {
            id: "file-1".into(),
            name: "a.png".into(),
            byte_len: 3,
            content_root: "a".repeat(64),
        };
        let wrapped = crate::files::compose(&compose("看图", &[comment("a", "原句")]), &[file]);
        let batch = decode_batch(&wrapped).unwrap();
        assert_eq!(batch.pairs[0].quote, "原句");
        assert_eq!(batch.extra_text, "看图");
        assert_eq!(quotable_text(&wrapped), "这里调整一下 看图");
    }

    #[test]
    fn legacy_batches_without_newer_fields_decode() {
        // An early payload: no message_id, author_agent_id, attachments or body_text.
        let legacy = format!(
            "{PREFIX}{}{SUFFIX}",
            r#"{"comments":[{"id":"c1","source":{"session_id":"s","author":"Planner","quote":"验收标准"},"comment":"保留"}],"text":"继续"}"#
        );
        let batch = decode_batch(&legacy).unwrap();
        assert_eq!(batch.pairs[0].message_id, None);
        assert_eq!(batch.pairs[0].author_agent_id, None);
        assert_eq!(batch.pairs[0].quote, "验收标准");
        assert_eq!(batch.pairs[0].reply, "保留");
        assert_eq!(batch.extra_text, "继续");
        // A source missing even session_id still decodes.
        let sparse = format!(
            "{PREFIX}{}{SUFFIX}",
            r#"{"comments":[{"id":"c1","source":{"quote":"q"},"comment":"r"}],"text":""}"#
        );
        assert_eq!(decode_batch(&sparse).unwrap().pairs[0].session_id, "");
        // The legacy plain rendering is unchanged.
        assert_eq!(display_text(&legacy), "Planner\n> 验收标准\n\n保留\n\n继续");
        // A damaged envelope is shown as text, never half-decoded.
        let broken = format!("{PREFIX}{{not json{SUFFIX}");
        assert_eq!(decode_batch(&broken), None);
        assert_eq!(quotable_text(&broken), broken);
    }
}
