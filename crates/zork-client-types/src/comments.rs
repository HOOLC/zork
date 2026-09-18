//! Structured, conversation-local comments. Only the main composer sends them.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
}
