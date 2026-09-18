//! Pure transcript projection and reconciliation helpers.
//!
//! The HTTP/SSE client exposes Station-delivered message identities; internal
//! execution events remain separate. These helpers keep ordering and optimistic-send
//! rules explicit and testable without constructing a GPUI window.

use crate::api::{AgentStatus, MessageMetadata, Role, TranscriptMessage};

/// Structurally shared transcript snapshots; row payloads are immutable.
pub type Transcript = zork_observe::List<TranscriptLine>;

/// One renderable transcript row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TranscriptLine {
    Message {
        role: Role,
        content: String,
        metadata: MessageMetadata,
    },
}

/// Convert a station-delivered message into a renderable line.
pub fn transcript_line_from(item: &TranscriptMessage) -> Option<TranscriptLine> {
    let TranscriptMessage::Message { metadata, .. } = item;
    if crate::interactions::result(metadata).is_some() {
        return None;
    }
    match item {
        TranscriptMessage::Message {
            role,
            content,
            metadata,
        } => {
            let mut metadata = metadata.clone();
            metadata.interaction_view =
                crate::interactions::view(&metadata, None, &Default::default());
            Some(TranscriptLine::Message {
                role: *role,
                content: content.clone(),
                metadata,
            })
        }
    }
}

pub fn project_page_with_pending(
    items: &[TranscriptMessage],
    pending_user: &mut Vec<String>,
) -> Vec<TranscriptLine> {
    let mut lines: Vec<_> = items.iter().filter_map(transcript_line_from).collect();
    for content in items.iter().rev().take(32).filter_map(|item| match item {
        TranscriptMessage::Message {
            role: Role::User,
            content,
            ..
        } => Some(content.as_str()),
        _ => None,
    }) {
        take_pending_user_echo(pending_user, content);
    }
    for content in pending_user.iter() {
        let is_already_projected = lines.iter().rev().take(32).any(|line| {
            matches!(
                line,
                TranscriptLine::Message {
                    role: Role::User,
                    content: projected, ..
                } if projected == content
            )
        });
        if !is_already_projected {
            lines.push(TranscriptLine::Message {
                role: Role::User,
                content: content.clone(),
                metadata: MessageMetadata::default(),
            });
        }
    }
    lines
}

/// Insert one local user row before the network request completes.
pub fn begin_optimistic_user(
    lines: &mut Vec<TranscriptLine>,
    pending_user: &mut Vec<String>,
    content: String,
) {
    lines.push(TranscriptLine::Message {
        role: Role::User,
        content: content.clone(),
        metadata: MessageMetadata::default(),
    });
    pending_user.push(content);
}

/// Consume the matching pending entry when the station user-message echo arrives.
/// `true` means the caller must suppress that echo because the local row is
/// already visible.
pub fn take_pending_user_echo(pending_user: &mut Vec<String>, content: &str) -> bool {
    let Some(position) = pending_user.iter().position(|item| item == content) else {
        return false;
    };
    pending_user.remove(position);
    true
}

/// Remove the pending optimistic row after a failed station send.
pub fn rollback_optimistic_user(
    lines: &mut Vec<TranscriptLine>,
    pending_user: &mut Vec<String>,
    content: &str,
) -> bool {
    let Some(position) = pending_user.iter().position(|item| item == content) else {
        return false;
    };
    pending_user.remove(position);

    let Some(line_position) = lines.iter().rposition(|line| {
        matches!(
            line,
            TranscriptLine::Message {
                role: Role::User,
                content: projected, ..
            } if projected == content
        )
    }) else {
        return false;
    };
    lines.remove(line_position);
    true
}

/// Reconcile a retried SSE delivery or an optimistic local row with the durable
/// Station identity. Equal prose in two distinct delivered messages stays distinct.
pub fn reconcile_delivery(
    lines: &mut [TranscriptLine],
    pending: &mut Vec<String>,
    message: &TranscriptMessage,
) -> Option<usize> {
    let TranscriptMessage::Message {
        role,
        content,
        metadata,
    } = message;
    let existing=metadata.id.as_ref().and_then(|id|lines.iter().position(|line|matches!(line,TranscriptLine::Message{metadata,..} if metadata.id.as_ref()==Some(id))));
    let optimistic = if *role == Role::User && take_pending_user_echo(pending, content) {
        lines.iter().rposition(|line|matches!(line,TranscriptLine::Message{role:Role::User,content:previous,metadata} if previous==content&&metadata.id.is_none()))
    } else {
        None
    };
    let index = existing.or(optimistic)?;
    lines[index] = transcript_line_from(message)?;
    Some(index)
}

fn same_message(a: &TranscriptLine, b: &TranscriptLine) -> bool {
    let (
        TranscriptLine::Message {
            role: a_role,
            content: a_text,
            metadata: a_meta,
        },
        TranscriptLine::Message {
            role: b_role,
            content: b_text,
            metadata: b_meta,
        },
    ) = (a, b);
    if let (Some(a), Some(b)) = (&a_meta.id, &b_meta.id) {
        a == b
    } else {
        a_role == b_role && a_text == b_text
    }
}

/// Prepend an oldest-first history page. The longest exact page-boundary
/// overlap is removed so a retried page cannot repeat already visible rows.
/// Returns the number of inserted renderable rows.
pub fn prepend_older_lines(lines: &mut Vec<TranscriptLine>, items: &[TranscriptMessage]) -> usize {
    let older: Vec<_> = items.iter().filter_map(transcript_line_from).collect();
    let max_overlap = older.len().min(lines.len());
    let overlap = (1..=max_overlap)
        .rev()
        .find(|count| {
            older[older.len() - count..]
                .iter()
                .zip(&lines[..*count])
                .all(|(a, b)| same_message(a, b))
        })
        .unwrap_or(0);
    let inserted = older.len() - overlap;
    if inserted > 0 {
        lines.splice(0..0, older[..inserted].iter().cloned());
    }
    inserted
}

fn truncate(text: &str, max_chars: usize) -> &str {
    let boundary = text
        .char_indices()
        .nth(max_chars)
        .map(|(byte_index, _)| byte_index)
        .unwrap_or(text.len());
    &text[..boundary]
}

fn short_id(id: &str) -> &str {
    truncate(id, 8)
}

/// Partial history is not the beginning of the task and therefore cannot
/// supply its title. Use the stable id until the true oldest page is present.
pub fn stable_task_title(id: &str, lines: &[TranscriptLine], has_older: bool) -> String {
    if has_older {
        return format!("Task {}", short_id(id));
    }
    lines
        .iter()
        .find_map(|line| match line {
            TranscriptLine::Message {
                role: Role::User,
                content,
                ..
            } => content
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(|line| truncate(line, 72).to_owned()),
            _ => None,
        })
        .unwrap_or_else(|| format!("Task {}", short_id(id)))
}

/// Live activity supplements, but never becomes, a persisted message row.
pub fn should_render_live_activity(
    status: &AgentStatus,
    _last_line: Option<&TranscriptLine>,
) -> bool {
    !matches!(
        status,
        AgentStatus::Clear | AgentStatus::Finished | AgentStatus::Interrupted
    )
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    fn message(id: &str, text: &str) -> TranscriptMessage {
        TranscriptMessage::Message {
            role: Role::User,
            content: text.into(),
            metadata: MessageMetadata {
                id: Some(id.into()),
                created_at: Some("2026-09-06T00:00:00Z".into()),
                ..Default::default()
            },
        }
    }
    #[test]
    fn optimistic_echo_acquires_identity_and_retries_do_not_repeat_it() {
        let mut lines = vec![];
        let mut pending = vec![];
        begin_optimistic_user(&mut lines, &mut pending, "hello".into());
        assert_eq!(
            reconcile_delivery(&mut lines, &mut pending, &message("a", "hello")),
            Some(0)
        );
        assert!(pending.is_empty());
        assert_eq!(
            reconcile_delivery(&mut lines, &mut pending, &message("a", "hello")),
            Some(0)
        );
        assert_eq!(lines.len(), 1);
        assert_eq!(
            reconcile_delivery(&mut lines, &mut pending, &message("b", "hello")),
            None
        );
    }
    #[test]
    fn metadata_enrichment_does_not_break_page_overlap() {
        let mut lines = vec![transcript_line_from(&message("a", "one")).unwrap()];
        let mut enriched = message("a", "one");
        let TranscriptMessage::Message { metadata, .. } = &mut enriched;
        metadata.author_name = Some("Leader".into());
        assert_eq!(
            prepend_older_lines(&mut lines, &[message("older", "first"), enriched]),
            1
        );
        assert_eq!(lines.len(), 2);
    }
}

/// Resolve a human-readable device label only from known identity mappings.
/// A conversation's owning device is not necessarily its message author's device.
pub fn message_device_label(
    metadata: &MessageMetadata,
    local_agents: &std::collections::HashSet<String>,
    local_name: Option<&str>,
    aliases: &std::collections::HashMap<String, String>,
) -> Option<String> {
    if let Some(device) = &metadata.device {
        return Some(aliases.get(device).unwrap_or(device).clone());
    }
    metadata
        .author_agent_id
        .as_ref()
        .filter(|id| local_agents.contains(*id))
        .and(local_name)
        .map(str::to_owned)
}

#[cfg(test)]
mod device_identity_tests {
    use super::*;
    #[test]
    fn unknown_or_remote_authors_are_never_assigned_to_the_viewing_device() {
        let local = std::collections::HashSet::from(["leader".into()]);
        let aliases = std::collections::HashMap::from([("key:remote".into(), "mini2".into())]);
        let mut metadata = MessageMetadata::default();
        assert_eq!(
            message_device_label(&metadata, &local, Some("mini1"), &aliases),
            None
        );
        metadata.author_agent_id = Some("leader".into());
        assert_eq!(
            message_device_label(&metadata, &local, Some("mini1"), &aliases),
            Some("mini1".into())
        );
        metadata.author_agent_id = Some("key:remote/worker".into());
        assert_eq!(
            message_device_label(&metadata, &local, Some("mini1"), &aliases),
            None
        );
        metadata.device = Some("key:remote".into());
        assert_eq!(
            message_device_label(&metadata, &local, Some("mini1"), &aliases),
            Some("mini2".into())
        );
    }
}

/// Merge an overlapping chronological page without dropping already loaded history.
pub fn merge_history_page(lines: &mut Vec<TranscriptLine>, items: &[TranscriptMessage]) {
    let incoming: Vec<_> = items.iter().filter_map(transcript_line_from).collect();
    let prefix = incoming
        .iter()
        .position(|line| lines.iter().any(|old| same_message(old, line)))
        .unwrap_or(0);
    if prefix > 0 {
        lines.splice(0..0, incoming[..prefix].iter().cloned());
    }
    for line in incoming.into_iter().skip(prefix) {
        if let Some(old) = lines.iter_mut().find(|old| same_message(old, &line)) {
            *old = line;
        } else {
            lines.push(line);
        }
    }
}

pub fn reading_anchor(role: crate::api::Role, content: &str) -> String {
    zork_mesh::content_root(format!("{role:?}\n{content}").as_bytes())
}
