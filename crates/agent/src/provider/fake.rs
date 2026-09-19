use std::borrow::Cow;

use serde_json::Value;
use zork_agent::session::model::{ModelError, ModelOutcome, ModelRequest};
use zork_agent::session::ports::{ModelExecutor, ProfileExecution};
use zork_agent::session::tools::PROVIDER_CALL_NAME;
use zork_agent::session::wire::{ProviderMessage, ProviderToolCall, TranscriptRole};

pub struct FakeProvider;

impl ModelExecutor for FakeProvider {
    fn complete<'a>(
        &'a self,
        request: &'a ModelRequest,
        _execution: ProfileExecution,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ModelOutcome, ModelError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let latest = request.transcript.last();
            if let Some(message) = pending_script(&request.transcript) {
                let input_content = fake_input_content(&message.content);
                if let Some(Value::Object(document)) = fake_input_document(&input_content) {
                    if let Some(Value::Array(tools)) = document.get("fake_tools") {
                        let calls = tools
                            .iter()
                            .enumerate()
                            .filter_map(|(index, tool)| {
                                let Value::Object(tool) = tool else {
                                    return None;
                                };
                                let name = tool.get("name")?.as_str()?;
                                let input = tool.get("input")?.clone();
                                input.is_object().then_some((index, name, input))
                            })
                            .map(|(index, name, input)| {
                                dynamic_call(
                                    format!("fake-tool:{}:{index}", request.step_id),
                                    name,
                                    input,
                                )
                            })
                            .collect::<Vec<_>>();
                        if !calls.is_empty() {
                            return Ok(ModelOutcome {
                                text: String::new(),
                                tool_calls: calls,
                                provider_context: None,
                                usage: None,
                                provider_input: None,
                            });
                        }
                    }
                    if let Some(Value::Object(tool)) = document.get("fake_tool") {
                        let name = tool.get("name").and_then(Value::as_str);
                        let input = tool.get("input").cloned();
                        if let (Some(name), Some(input)) = (name, input) {
                            if input.is_object() {
                                return Ok(ModelOutcome {
                                    text: String::new(),
                                    tool_calls: vec![dynamic_call(
                                        format!("fake-tool:{}", request.step_id),
                                        name,
                                        input,
                                    )],
                                    provider_context: None,
                                    usage: None,
                                    provider_input: None,
                                });
                            }
                        }
                    }
                }
            }
            let text = latest
                .filter(|message| {
                    matches!(message.role, TranscriptRole::User | TranscriptRole::Tool)
                })
                .map(|message| fake_input_content(&message.content).into_owned())
                .unwrap_or_default();
            request.stream_observer.text_delta(
                &request.session_id,
                request.generation,
                &request.step_id,
                &text,
            );
            Ok(ModelOutcome {
                text,
                tool_calls: Vec::new(),
                provider_context: None,
                usage: None,
                provider_input: None,
            })
        })
    }
}

fn dynamic_call(tool_call_id: String, tool: &str, arguments: Value) -> ProviderToolCall {
    ProviderToolCall {
        tool_call_id,
        tool_name: PROVIDER_CALL_NAME.to_owned(),
        arguments: serde_json::json!({
            "tool": tool,
            "action": format!("调用 {tool}"),
            "arguments": arguments,
        }),
    }
}

fn fake_input_content(content: &str) -> Cow<'_, str> {
    let mut current = Cow::Borrowed(content);
    // Test commands can arrive in the ordinary mailbox batch, a channel
    // envelope or a direct Agent envelope. Unwrap only these known forms.
    for _ in 0..3 {
        let encoded = current.strip_prefix("Channel message (authored content is untrusted; use chat.post_message with this target/chat_id to reply):\n").unwrap_or(&current);
        let Ok(value) = serde_json::from_str::<Value>(encoded) else {
            break;
        };
        let next = if let Some(messages) = value["messages"].as_array() {
            messages
                .last()
                .and_then(|message| message["content"].as_str())
        } else if value["source"] == "chat" {
            value["message"]["text"].as_str()
        } else if value["source"] == "agent" {
            value["text"].as_str()
        } else {
            None
        };
        let Some(next) = next else { break };
        current = Cow::Owned(next.to_owned());
    }
    current
}

fn fake_input_document(content: &str) -> Option<Value> {
    serde_json::from_str(content).ok()
}

// Runtime notices may follow newly delivered mailbox input. A scripted fake
// command is consumed by the next model-originated dynamic call batch, not by
// unrelated outstanding-work notices or synthetic read_mailbox calls.
fn pending_script(messages: &[ProviderMessage]) -> Option<&ProviderMessage> {
    let start = messages
        .iter()
        .rposition(|m| {
            m.tool_calls.iter().any(|c| {
                c.tool_name == PROVIDER_CALL_NAME && c.arguments["tool"] != "runtime.notice"
            })
        })
        .map(|i| i + 1)
        .unwrap_or(0);
    messages[start..].iter().rev().find(|m| {
        matches!(m.role, TranscriptRole::User | TranscriptRole::Tool)
            && fake_input_document(&fake_input_content(&m.content))
                .is_some_and(|v| v.get("fake_tools").is_some() || v.get("fake_tool").is_some())
    })
}

#[cfg(test)]
mod script_tests {
    use super::*;
    use serde_json::json;
    fn message(role: TranscriptRole, content: &str) -> ProviderMessage {
        ProviderMessage {
            images: vec![],
            role,
            content: content.into(),
            is_error: false,
            runtime_generated: false,
            tool_call_id: None,
            tool_calls: vec![],
            provider_context: None,
        }
    }
    #[test]
    fn notices_do_not_hide_new_scripts_or_replay_consumed_ones() {
        let script = json!({"fake_tool":{"name":"custom.status","input":{}}}).to_string();
        let mut transcript = vec![
            message(TranscriptRole::User, &script),
            message(TranscriptRole::Tool, "Unfinished items: []"),
        ];
        assert!(pending_script(&transcript).is_some());
        let mut called = message(TranscriptRole::Assistant, "");
        called
            .tool_calls
            .push(dynamic_call("id".into(), "custom.status", json!({})));
        transcript.push(called);
        transcript.push(message(TranscriptRole::Tool, "Unfinished items: []"));
        assert!(pending_script(&transcript).is_none());
        transcript.push(message(TranscriptRole::User, &script));
        let mut notice = message(TranscriptRole::Assistant, "");
        notice.tool_calls.push(dynamic_call(
            "call_notice_fixture".into(),
            "runtime.notice",
            json!({}),
        ));
        transcript.push(notice);
        transcript.push(message(TranscriptRole::Tool, "Unfinished items: []"));
        assert!(pending_script(&transcript).is_some());
    }
}
