//! Platform-independent conversation rules. UI adapters own selection and scroll,
//! while both apply these reducers to Station messages and live activity.
use crate::api::{AgentStatus, ParticipantStatus, SessionSummary, SseEvent, TranscriptMessage};

#[derive(Debug, PartialEq)]
pub enum DecodedSseEvent {
    Transcript(TranscriptMessage),
    Status(AgentStatus),
    Participants(Vec<ParticipantStatus>),
    HistoryChanged,
    MessagesChanged,
    Resync,
}
pub fn decode_sse_event(event: &SseEvent) -> Result<Option<DecodedSseEvent>, serde_json::Error> {
    match event.name.as_str() {
        "message" => serde_json::from_str(&event.data)
            .map(DecodedSseEvent::Transcript)
            .map(Some),
        "status" => serde_json::from_str(&event.data)
            .map(DecodedSseEvent::Status)
            .map(Some),
        "participants" => serde_json::from_str(&event.data)
            .map(DecodedSseEvent::Participants)
            .map(Some),
        "history_changed" => Ok(Some(DecodedSseEvent::HistoryChanged)),
        "messages_changed" => Ok(Some(DecodedSseEvent::MessagesChanged)),
        "resync" => Ok(Some(DecodedSseEvent::Resync)),
        _ => Ok(None),
    }
}
pub fn can_send(session: &SessionSummary) -> bool {
    session.can_send.unwrap_or(session.kind != "agent_control")
}
pub fn apply_status(
    activity: &mut Option<AgentStatus>,
    participants: &mut [ParticipantStatus],
    stop_pending: &mut bool,
    session: Option<&str>,
    status: AgentStatus,
) {
    if matches!(
        status,
        AgentStatus::Interrupted | AgentStatus::Finished | AgentStatus::Failed { .. }
    ) {
        *stop_pending = false;
    }
    *activity = Some(match status {
        AgentStatus::ToolFinished { tool_call_id } => {
            let (mut calls, thinking, deadline) = match activity.take() {
                Some(AgentStatus::ToolsStarted { calls, thinking }) => (calls, thinking, None),
                Some(AgentStatus::ToolsWaiting { calls, deadline_ms }) => {
                    (calls, false, Some(deadline_ms))
                }
                _ => (vec![], false, None),
            };
            calls.retain(|call| call.tool_call_id != tool_call_id);
            if calls.is_empty() {
                AgentStatus::Thinking
            } else if let Some(deadline_ms) = deadline {
                AgentStatus::ToolsWaiting { calls, deadline_ms }
            } else {
                AgentStatus::ToolsStarted { calls, thinking }
            }
        }
        status => status,
    });
    for participant in participants {
        if Some(participant.session_id.as_str()) == session {
            participant.activity = activity.clone();
        }
    }
}

/// A partial device revision must not erase details fetched for the same task
/// revision, nor roll the client back when a stale snapshot arrives.
pub fn merge_sessions(previous: &[SessionSummary], incoming: &mut [SessionSummary]) {
    for session in incoming {
        let Some(previous) = previous
            .iter()
            .find(|s| s.session_id == session.session_id)
            .and_then(|s| s.task.as_ref())
        else {
            continue;
        };
        if session
            .task
            .as_ref()
            .is_some_and(|t| t.revision < previous.revision)
        {
            session.task = Some(previous.clone());
        } else if let Some(task) = session
            .task
            .as_mut()
            .filter(|t| t.revision == previous.revision)
        {
            task.goal = previous.goal.clone();
            task.result_text = previous.result_text.clone();
        }
    }
}

pub fn stop_confirmed(online: bool, status: Option<crate::api::SessionStatus>) -> bool {
    online && status == Some(crate::api::SessionStatus::Wait)
}

/// Present the shared comment/file envelope without exposing wire markup in UI.
///
/// A comment batch also gets `comment_batch`
/// (`zork_client_types::comments::CommentBatchView`: `pairs` of
/// `{id, session_id, message_id, author, author_agent_id, quote, reply}` and
/// `extra_text`), the structured form the redesigned bubble renders;
/// `display_content` stays the legacy plain text.
pub fn project_payload(value: &mut serde_json::Value) {
    let raw = value["content"].as_str().unwrap_or_default().to_owned();
    let (raw, files) = zork_client_types::files::decode(&raw).unwrap_or((raw, vec![]));
    value["file_views"] = serde_json::json!(crate::file_io::views(&files));
    value["files"] = serde_json::json!(files);
    if let Some(batch) = crate::comments::decode_batch(&raw) {
        value["comment_batch"] = serde_json::json!(batch);
    }
    if let Some((text, comments, attachments)) = crate::comments::decode_document(&raw) {
        value["display_content"] = serde_json::json!(crate::comments::display_text(
            &crate::comments::compose(&text, &comments)
        ));
        value["attachments"] = serde_json::json!(attachments);
    } else {
        value["display_content"] = serde_json::json!(raw);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::PublicToolCall;
    #[test]
    fn completing_a_tool_preserves_other_calls_and_terminal_status_confirms_stop() {
        let call = |id: &str| PublicToolCall {
            tool_call_id: id.into(),
            tool_name: "read".into(),
            detail: String::new(),
            labels: Default::default(),
            action: String::new(),
        };
        let mut activity = Some(AgentStatus::ToolsStarted {
            thinking: false,
            calls: vec![call("a"), call("b")],
        });
        let mut participants = vec![ParticipantStatus {
            subscribed: false,
            assigned: false,
            id: "leader".into(),
            name: "Leader".into(),
            avatar: None,
            session_id: "chat".into(),
            activity: activity.clone(),
        }];
        let mut stopping = true;
        apply_status(
            &mut activity,
            &mut participants,
            &mut stopping,
            Some("chat"),
            AgentStatus::ToolFinished {
                tool_call_id: "a".into(),
            },
        );
        assert_eq!(
            activity,
            Some(AgentStatus::ToolsStarted {
                thinking: false,
                calls: vec![call("b")]
            })
        );
        assert_eq!(participants[0].activity, activity);
        assert!(stopping);
        apply_status(
            &mut activity,
            &mut participants,
            &mut stopping,
            Some("chat"),
            AgentStatus::Finished,
        );
        assert!(!stopping);
        assert_eq!(activity, Some(AgentStatus::Finished));
    }
    #[test]
    fn activity_and_internal_history_never_become_delivered_messages() {
        let event = SseEvent {
            name: "status".into(),
            data: r#"{"state":"thinking"}"#.into(),
        };
        assert_eq!(
            decode_sse_event(&event).unwrap(),
            Some(DecodedSseEvent::Status(AgentStatus::Thinking))
        );
        let event = SseEvent {
            name: "execution_record".into(),
            data: "private".into(),
        };
        assert_eq!(decode_sse_event(&event).unwrap(), None);
    }

    #[test]
    fn station_messages_carry_reply_quotes_and_rows_carry_comment_pairs() {
        let event = SseEvent {
            name: "message".into(),
            data:
                r#"{"type":"message","role":"assistant","content":"已改","id":"m2","reply_to":"m1",
                "quote":"对比度只有 3.9:1","quote_kind":"summary","author_agent_id":"builder"}"#
                    .into(),
        };
        let Some(DecodedSseEvent::Transcript(TranscriptMessage::Message { metadata, .. })) =
            decode_sse_event(&event).unwrap()
        else {
            panic!("message event")
        };
        let quote = metadata.reply_quote().unwrap();
        assert_eq!(quote.text, "对比度只有 3.9:1");
        assert_eq!(quote.kind, zork_client_types::chat::QuoteKind::Summary);
        // The row JSON the Android observation sends keeps the fields.
        let row = serde_json::to_value(&metadata).unwrap();
        assert_eq!(row["quote_kind"], "summary");
        // Older Stations send no quote.
        let legacy = SseEvent {
            name: "message".into(),
            data:
                r#"{"type":"message","role":"assistant","content":"x","id":"m3","reply_to":"m1"}"#
                    .into(),
        };
        let Some(DecodedSseEvent::Transcript(TranscriptMessage::Message { metadata, .. })) =
            decode_sse_event(&legacy).unwrap()
        else {
            panic!("message event")
        };
        assert_eq!(metadata.reply_quote(), None);
        assert!(serde_json::to_value(&metadata)
            .unwrap()
            .get("quote")
            .is_none());

        let payload = crate::comments::compose(
            "补充",
            &[crate::comments::DraftComment {
                id: "d".into(),
                source: crate::comments::CommentSource {
                    session_id: "chat".into(),
                    message_id: Some("m1".into()),
                    quote: "原句".into(),
                    ..Default::default()
                },
                comment: "回复".into(),
            }],
        );
        let mut value = serde_json::json!({"content": payload});
        project_payload(&mut value);
        assert_eq!(value["comment_batch"]["pairs"][0]["message_id"], "m1");
        assert_eq!(value["comment_batch"]["pairs"][0]["reply"], "回复");
        assert_eq!(value["comment_batch"]["extra_text"], "补充");
        assert!(value["display_content"]
            .as_str()
            .unwrap()
            .contains("> 原句"));
        let mut plain = serde_json::json!({"content": "hello"});
        project_payload(&mut plain);
        assert!(plain.get("comment_batch").is_none());
    }
}
