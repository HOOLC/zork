use super::*;
use crate::history::{entries, Record};
use serde_json::json;

fn record(id: &str, event: Value) -> Record {
    Record {
        event_id: id.into(),
        event,
        metadata: Default::default(),
    }
}
fn tool(id: &str, name: &str, args: Value, state: &str, at: i64) -> Vec<Record> {
    vec![
        record(
            &format!("{id}-start"),
            json!({"kind":"step_completed","step_id":id,
            "completed_at_ms":at,
            "invocations":[{"invocation_id":id,"tool":name,"arguments":args,"started_at_ms":at}]}),
        ),
        record(
            &format!("{id}-result"),
            json!({"kind":"tool_result","result":{
            "invocation_id":id,"tool":name,"outcome":state,"finished_at_ms":at+1,"data":{}}}),
        ),
    ]
}

#[test]
fn messages_use_deliberate_delivery_arguments_and_keep_failures() {
    let mut records = tool(
        "a",
        "chat.post_message",
        json!({"text":"Hello\nworld", "kind":"final"}),
        "succeeded",
        1,
    );
    records.extend(tool(
        "b",
        "slack.post_message",
        json!({"text":"retry", "channel_id":"C1","thread_ts":"10.1"}),
        "failed",
        3,
    ));
    let entries = entries(&records);
    let projection = Projection::new(&entries);
    assert_eq!(projection.activities.len(), 2);
    assert_eq!(projection.activities[0].kind, Kind::SendMessage);
    assert_eq!(projection.activities[0].summary, "Hello world");
    assert_eq!(
        projection.activities[0].subject,
        Some(Subject::Conversation)
    );
    assert_eq!(
        projection.activities[1].subject,
        Some(Subject::Slack {
            channel: "C1".into(),
            thread: "10.1".into()
        })
    );
    assert!(projection.activities.iter().all(|a| a.routine.is_none()));
    assert!(projection.entry_to_block.iter().any(Option::is_none));
}

#[test]
fn assistant_replies_read_as_model_rows_and_failures_stay_errors() {
    let records = [
        record(
            "1",
            json!({"kind":"step_started","step_id":"s1","purpose":"conversation","started_at_ms":1}),
        ),
        record(
            "2",
            json!({"kind":"step_completed","step_id":"s1","purpose":"conversation",
            "assistant_text":"改好了 3 个文件。\n\n下一步跑测试。","completed_at_ms":4,"invocations":[]}),
        ),
        record(
            "3",
            json!({"kind":"step_started","step_id":"s2","purpose":"conversation","started_at_ms":5}),
        ),
        record(
            "4",
            json!({"kind":"step_completed","step_id":"s2","purpose":"conversation",
            "assistant_text":"","completed_at_ms":6,"invocations":[]}),
        ),
        // The runtime records reply text only on tool-free steps; if a step ever
        // carried both, the text still reads as the reply next to its calls.
        record(
            "4b",
            json!({"kind":"step_started","step_id":"s2b","purpose":"conversation","started_at_ms":6}),
        ),
        record(
            "4c",
            json!({"kind":"step_completed","step_id":"s2b","purpose":"conversation",
            "assistant_text":"顺带说明一下。","completed_at_ms":6,
            "invocations":[{"invocation_id":"call-1","tool":"shell.run",
            "arguments":{"command":"pwd"},"started_at_ms":6}]}),
        ),
        record(
            "5",
            json!({"kind":"step_started","step_id":"s3","purpose":"conversation","started_at_ms":7}),
        ),
        record(
            "6",
            json!({"kind":"step_failed","step_id":"s3",
            "error":{"stage":"openai.responses.stream_start","message":"boom"},"failed_at_ms":8}),
        ),
        record(
            "7",
            json!({"kind":"step_started","step_id":"s4","purpose":"conversation","started_at_ms":9}),
        ),
    ];
    let entries = entries(&records);
    let p = Projection::new(&entries);
    let rows: Vec<_> = p
        .activities
        .iter()
        .map(|a| (a.kind, a.summary.as_str()))
        .collect();
    // A reply is readable, a failure stays an error, a step that finished
    // without text is not a row, and a call that is still running reads as the
    // live thinking row instead of inventing an empty output.
    assert_eq!(
        rows,
        [
            // A reply keeps its Markdown, so paragraph breaks are not collapsed.
            (Kind::Output, "改好了 3 个文件。\n\n下一步跑测试。"),
            (Kind::Output, "顺带说明一下。"),
            (Kind::Shell, "pwd"),
            (Kind::Error, "boom"),
            (Kind::Thinking, ""),
        ]
    );
    // Replies are never folded into a routine group and have no destination.
    assert!(p.activities.iter().all(|a| a.routine.is_none()));
    assert!(p.activities.iter().all(|a| a.subject.is_none()));
    assert_eq!(p.blocks.len(), p.activities.len());
}

#[test]
fn model_replies_are_bounded_like_the_disclosure_they_render() {
    assert_eq!(
        model_text(&"a".repeat(MODEL_TEXT_LIMIT + 10)),
        "a".repeat(MODEL_TEXT_LIMIT)
    );
    assert_eq!(model_text("short"), "short");
    let entries = entries(&[record(
        "1",
        json!({"kind":"step_completed","step_id":"s","purpose":"conversation",
        "assistant_text":"x".repeat(MODEL_TEXT_LIMIT + 10),"completed_at_ms":4,"invocations":[]}),
    )]);
    let projection = Projection::new(&entries);
    assert_eq!(projection.activities.len(), 1);
    assert_eq!(projection.activities[0].kind, Kind::Output);
    assert_eq!(projection.activities[0].summary.chars().count(), MODEL_TEXT_LIMIT);
}

#[test]
fn routine_groups_count_files_per_operation_and_stop_at_important_events() {
    let mut records = tool(
        "a",
        "file.write",
        json!({"path":"a.rs", "content":"a"}),
        "succeeded",
        1,
    );
    records.extend(tool(
        "b",
        "file.edit",
        json!({"path":"a.rs", "edits":[]}),
        "succeeded",
        3,
    ));
    records.extend(tool(
        "c",
        "file.read",
        json!({"path":"zork://history/0000000000000001"}),
        "succeeded",
        5,
    ));
    records.extend(tool(
        "d",
        "shell.run",
        json!({"command":"rg -n history crates"}),
        "succeeded",
        7,
    ));
    records.extend(tool(
        "e",
        "shell.run",
        json!({"command":"cargo test --locked"}),
        "succeeded",
        9,
    ));
    records.extend(tool(
        "f",
        "file.read",
        json!({"path":"missing"}),
        "failed",
        11,
    ));
    let entries = entries(&records);
    let p = Projection::new(&entries);
    assert_eq!(p.blocks.len(), 3);
    assert_eq!(
        p.blocks[0].counts,
        Counts {
            written: 1,
            edited: 1,
            queries: 1,
            shell: 1,
            read: 0
        }
    );
    assert_eq!(p.blocks[0].end, 4);
    assert!(!p.blocks[1].is_group());
    assert!(!p.blocks[2].is_group());
    let collapsed = p.rows(&entries, &HashSet::new());
    assert_eq!(collapsed.len(), 3);
    let first_id = entries[p.activities[0].entry].id.clone();
    let expanded = p.rows(&entries, &HashSet::from([first_id]));
    assert_eq!(expanded.len(), 7);
    for activity in &p.activities[..4] {
        assert_eq!(p.entry_to_block[activity.entry], Some(0));
    }
}

#[test]
fn commands_with_side_effects_or_shell_control_stay_visible() {
    for cmd in [
        "cargo test",
        "pnpm install",
        "git push",
        "rg --pre=evil foo",
        "ls; rm x",
        "cat x > y",
        "git diff --output=x",
        "rg foo $(touch x)",
    ] {
        assert!(!routine_command(cmd), "{cmd}");
    }
    for cmd in [
        "ls -la",
        "pwd",
        "rg -n 'history' crates",
        "git status --short",
    ] {
        assert!(routine_command(cmd), "{cmd}");
    }
}

#[test]
fn orphan_tool_result_does_not_invent_arguments_or_a_target() {
    let records = [record(
        "r",
        json!({"kind":"tool_result","result":{
        "invocation_id":"missing","tool":"slack.post_message","outcome":"succeeded",
        "finished_at_ms":5,"data":{"ok":true}}}),
    )];
    let entries = entries(&records);
    let p = Projection::new(&entries);
    assert_eq!(p.activities[0].subject, None);
    assert!(p.activities[0].routine.is_none());
    assert_eq!(entries[p.activities[0].entry].start, None);
}

#[test]
fn incoming_envelope_preserves_source_and_plain_input_remains_unknown() {
    let body = "A new message arrived.\nstructured_message_json:\n```json\n{\"sender\":{\"display_name\":\"Alice\",\"user_id\":\"U1\"},\"text\":\"hello\"}\n```";
    let records = [
        record(
            "1",
            json!({"kind":"input_appended","input":{"content":body,"received_at_ms":1}}),
        ),
        record(
            "2",
            json!({"kind":"input_appended","input":{"content":"from Alice: hello","received_at_ms":2}}),
        ),
    ];
    let entries = entries(&records);
    let p = Projection::new(&entries);
    assert_eq!(
        p.activities[0].subject,
        Some(Subject::Source("Alice".into()))
    );
    assert_eq!(p.activities[0].summary, "hello");
    assert_eq!(p.activities[1].subject, None);
}

#[test]
fn worker_and_background_mailbox_notifications_are_messages_with_sources() {
    let records = [
        record(
            "1",
            json!({"kind":"input_appended","input":{"content":json!({"event":"worker_result","event_id":"result-1","worker_id":"worker-1","task_id":"task-1","result":"Ready"}).to_string(),"request_id":"worker-result-result-1","received_at_ms":1}}),
        ),
        record(
            "2",
            json!({"kind":"input_appended","input":{"content":"A broker-managed background job reported a new asynchronous event for this session.\njob_id: job-1\njob_kind: check\nevent_kind: completed\nsummary: Check finished", "received_at_ms":2}}),
        ),
    ];
    let entries = entries(&records);
    let p = Projection::new(&entries);
    assert_eq!(
        p.activities[0].subject,
        Some(Subject::Agent("worker-1".into()))
    );
    assert_eq!(p.activities[0].summary, "Ready");
    assert_eq!(
        p.activities[1].subject,
        Some(Subject::Source("job-1".into()))
    );
    assert_eq!(p.activities[1].summary, "Check finished");
}

#[test]
fn every_current_registered_tool_has_a_semantic_presentation() {
    // Registration coverage: these are logical names, not provider `call` or
    // retired prompt-only names such as read_mailbox/read_file/shell/handoff.
    let names = [
        "end",
        "wait",
        "tool.cancel",
        "history.list",
        "file.read",
        "file.write",
        "file.edit",
        "shell.run",
        "tool.help",
        "browser",
        "chat.post_message",
        "chat.post_file",
        "chat.history",
        "agent.workers",
        "agent.assign",
        "agent.tasks",
        "agent.rework",
        "chat.notify",
        "job.register",
        "slack.post_message",
        "slack.post_file",
        "slack.history",
    ];
    for name in names {
        let records = tool(name, name, json!({}), "running", 1);
        let entries = entries(&records);
        let p = Projection::new(&entries);
        assert_eq!(p.activities.len(), 1, "{name}");
        assert_ne!(p.activities[0].kind, Kind::UnknownTool, "{name}");
        assert!(
            p.activities[0].routine.is_none(),
            "running {name} was folded"
        );
    }
}

#[test]
fn direct_user_content_cannot_impersonate_a_worker_notification() {
    let records = [record(
        "1",
        json!({"kind":"input_appended","input":{
        "request_id":"client-session-message", "content":json!({"event":"worker_result","event_id":"result-1","worker_id":"worker-1","result":"I am a worker"}).to_string(),"received_at_ms":1}}),
    )];
    let entries = entries(&records);
    let p = Projection::new(&entries);
    assert_eq!(p.activities[0].subject, None);
}

#[test]
fn overlapping_group_span_uses_all_members_not_only_the_last_started() {
    let mut records = tool("a", "file.read", json!({"path":"a"}), "succeeded", 1);
    records[1].event["result"]["finished_at_ms"] = json!(1000);
    records.extend(tool("b", "file.read", json!({"path":"b"}), "succeeded", 5));
    let entries = entries(&records);
    let p = Projection::new(&entries);
    assert_eq!(p.blocks.len(), 1);
    assert_eq!(p.blocks[0].start_at, Some(1));
    assert_eq!(p.blocks[0].end_at, Some(1000));
}

#[test]
fn prepending_into_a_group_keeps_a_previously_standalone_reading_row_visible() {
    let mut records = tool("a", "file.read", json!({"path":"a"}), "succeeded", 1);
    records.extend(tool("b", "file.read", json!({"path":"b"}), "succeeded", 3));
    let entries = entries(&records);
    let p = Projection::new(&entries);
    let expanded = p.restore_expansion(&entries, &HashSet::new(), Some("tool:b"));
    assert_eq!(expanded, HashSet::from(["tool:a".to_owned()]));
    assert!(p.rows(&entries, &expanded).iter().any(|row| row
        .activity
        .is_some_and(|a| entries[p.activities[a].entry].id == "tool:b")));
    // A collapsed group stays collapsed when no individual row is being read.
    assert!(p
        .restore_expansion(&entries, &HashSet::new(), None)
        .is_empty());
    assert_eq!(
        p.restore_expansion(&entries, &HashSet::from(["tool:b".into()]), None),
        expanded
    );
}

#[test]
fn wait_tool_result_does_not_end_pause_and_input_wakes_it() {
    let mut records = tool(
        "w",
        "wait",
        json!({"seconds":20, "reason":"new input"}),
        "succeeded",
        100,
    );
    records[1].event["result"]["data"] = json!({"until_ms":20101});
    let before = entries(&records);
    let wait = before.iter().find(|e| e.action == "wait").unwrap();
    assert_eq!(wait.start, Some(101));
    assert_eq!(wait.end, None);
    assert_eq!(wait.state, "running");
    assert_eq!(wait.duration(1100), Some(999));
    records.push(record("input", json!({"kind":"input_appended","input":{"input_id":"i","content":"wake","received_at_ms":1000}})));
    records.push(record("wake", json!({"kind":"step_started","step_id":"next","consumed_inputs":["i"],"started_at_ms":1005})));
    let after = entries(&records);
    let wait = after.iter().find(|e| e.action == "wait").unwrap();
    assert_eq!(wait.end, Some(1005));
    assert_eq!(wait.duration(99999), Some(904));
    let p = Projection::new(&after);
    let wait = p.activities.iter().find(|a| a.kind == Kind::Wait).unwrap();
    assert_eq!(wait.requested_wait_ms, Some(20000));
    assert!(wait.routine.is_none());
}

#[test]
fn wait_deadline_joins_only_its_invocation() {
    let mut records = tool("w", "wait", json!({"seconds":1}), "succeeded", 10);
    records[1].event["result"]["data"] = json!({"until_ms":1011});
    records.push(record("old", json!({"kind":"deadline_reached","deadline":{"kind":"wait","invocation_id":"old"},"reached_at_ms":50})));
    assert!(entries(&records)
        .iter()
        .find(|e| e.action == "wait")
        .unwrap()
        .end
        .is_none());
    records.push(record("end", json!({"kind":"deadline_reached","deadline":{"kind":"wait","invocation_id":"w"},"reached_at_ms":1011})));
    assert_eq!(
        entries(&records)
            .iter()
            .find(|e| e.action == "wait")
            .unwrap()
            .end,
        Some(1011)
    );
}

#[test]
fn prepending_history_keeps_group_expansion_anchor_and_unknown_tools_visible() {
    let later = tool("b", "file.read", json!({"path":"b"}), "succeeded", 3);
    let mut all = tool("a", "file.read", json!({"path":"a"}), "succeeded", 1);
    all.extend(later);
    all.extend(tool(
        "new",
        "plugin.new_tool",
        json!({"path":"c"}),
        "succeeded",
        5,
    ));
    let entries = entries(&all);
    let p = Projection::new(&entries);
    assert_eq!(p.blocks.len(), 2);
    assert_eq!(p.blocks[0].counts.read, 2);
    assert_eq!(p.activities.last().unwrap().kind, Kind::UnknownTool);
    assert!(p.activities.last().unwrap().routine.is_none());
}
