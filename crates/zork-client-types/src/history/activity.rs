//! Reading-oriented history, separate from the complete execution timeline.
//!
//! Project once when durable entries change. Rendering and expanding groups never
//! parse tool arguments or scan the full event stream.
use super::Entry;
use serde_json::Value;
use std::collections::{BTreeSet, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// The user's own message, read as the session input.
    Input,
    /// The assistant's own reply text, from the model lane.
    Output,
    /// A live model call that has not produced text yet. The history page shows the same
    /// live row before a reply exists, and only the runtime owns the reasoning
    /// summary that would name it.
    Thinking,
    SendMessage,
    SendFile,
    Notify,
    Assign,
    Rework,
    Workers,
    Tasks,
    Read,
    Write,
    Edit,
    Shell,
    Browser,
    Wait,
    End,
    Cancel,
    Help,
    History,
    ChatHistory,
    Job,
    Error,
    Notice,
    UnknownTool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Subject {
    Conversation,
    User,
    Agent(String),
    Task(String),
    Slack {
        channel: String,
        thread: String,
    },
    /// An observed source label without an address we can safely navigate to.
    Source(String),
    File(String),
    Tool(String),
    Invocation(String),
    BrowserTab(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Routine {
    Read(String),
    Write(String),
    Edit(String),
    Shell,
    Query,
    Thinking,
    Other,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Details {
    pub text: String,
    pub command: String,
    pub output: String,
    pub files: Vec<File>,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    pub path: String,
    pub content: String,
    pub previous: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Activity {
    pub entry: usize,
    pub kind: Kind,
    pub subject: Option<Subject>,
    /// Bounded, plain-text preview; a model reply keeps its Markdown intact.
    pub summary: String,
    pub routine: Option<Routine>,
    pub requested_wait_ms: Option<i64>,
    pub details: Details,
    /// A received message read as a message: who sent it and what came with
    /// it. The text itself is `details.text` (Markdown) and `summary`.
    pub message: Option<Box<Message>>,
}

/// The facts a Station delivery envelope carries about a received message.
/// Identifiers stay here for resolution; clients show names, never ids.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Message {
    /// The display name the envelope carried for its author, if any.
    pub name: Option<String>,
    /// Station origin of the device the message came from; `None` is this device.
    pub origin: Option<String>,
    /// Attached file names, in the order they were sent.
    pub files: Vec<String>,
    /// The message this one replies to, when it is a reply.
    pub reply_to: Option<String>,
    /// A payload whose shape is not known: readable text is extracted, and the
    /// pretty-printed source is kept only for the expandable raw details.
    pub raw: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub read: usize,
    pub written: usize,
    pub edited: usize,
    pub shell: usize,
    pub queries: usize,
    pub thinking: usize,
    pub other: usize,
    pub failed: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub start: usize,
    pub end: usize,
    pub counts: Counts,
    pub summary: String,
    pub start_at: Option<i64>,
    pub end_at: Option<i64>,
    pub running: bool,
    /// Keep the latest active operation outside the collapsed group.
    pub active: Option<usize>,
}
impl Block {
    pub fn is_group(&self) -> bool {
        self.end - self.start > 1
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub block: usize,
    /// None is the group summary; Some is a single record (or an expanded child).
    pub activity: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct Projection {
    pub activities: Vec<Activity>,
    pub blocks: Vec<Block>,
    pub entry_to_block: Vec<Option<usize>>,
}
impl Projection {
    pub fn new<'a>(entries: impl IntoIterator<Item = &'a Entry>) -> Self {
        let entries = entries.into_iter().collect::<Vec<_>>();
        let activities: Vec<_> = entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| project(index, entry))
            .collect();
        let mut blocks = Vec::new();
        let mut entry_to_block = vec![None; entries.len()];
        let mut start = 0;
        while start < activities.len() {
            if activities[start].kind == Kind::End
                && entries[activities[start].entry].state == "succeeded"
            {
                start += 1;
                continue;
            }
            let mut end = start + 1;
            if activities[start].routine.is_some() {
                while end < activities.len() && activities[end].routine.is_some() {
                    end += 1;
                }
            }
            let mut reads = BTreeSet::new();
            let mut writes = BTreeSet::new();
            let mut edits = BTreeSet::new();
            let mut counts = Counts::default();
            let mut labels = Vec::new();
            let mut start_at = None;
            let mut end_at = None;
            let mut running = false;
            let active = (start..end).rev().find(|&i| {
                activities[i].routine.is_some() && entries[activities[i].entry].state == "running"
            });
            for (i, activity) in activities.iter().enumerate().take(end).skip(start) {
                entry_to_block[activity.entry] = Some(blocks.len());
                let entry = &entries[activity.entry];
                if let Some(time) = entry.start.or(entry.end) {
                    start_at = Some(start_at.map_or(time, |previous: i64| previous.min(time)));
                }
                if let Some(time) = entry.end.or(entry.start) {
                    end_at = Some(end_at.map_or(time, |previous: i64| previous.max(time)));
                }
                running |= entry.state == "running";
                if active == Some(i) {
                    continue;
                }
                counts.failed +=
                    usize::from(matches!(entry.state.as_str(), "failed" | "timed_out"));
                match &activity.routine {
                    Some(Routine::Read(path)) => {
                        reads.insert(path);
                    }
                    Some(Routine::Write(path)) => {
                        writes.insert(path);
                    }
                    Some(Routine::Edit(path)) => {
                        edits.insert(path);
                    }
                    Some(Routine::Shell) => counts.shell += 1,
                    Some(Routine::Query) => counts.queries += 1,
                    Some(Routine::Thinking) => counts.thinking += 1,
                    Some(Routine::Other) => counts.other += 1,
                    None => {}
                }
                if labels.len() < 3 && !activity.summary.is_empty() {
                    labels.push(activity.summary.as_str());
                }
            }
            counts.read = reads.len();
            counts.written = writes.len();
            counts.edited = edits.len();
            blocks.push(Block {
                start,
                end,
                counts,
                summary: preview(&labels.join(" · ")),
                start_at,
                end_at,
                running,
                active,
            });
            start = end;
        }
        Self {
            activities,
            blocks,
            entry_to_block,
        }
    }

    pub fn rows<'a>(
        &self,
        entries: impl IntoIterator<Item = &'a Entry>,
        expanded: &HashSet<String>,
    ) -> Vec<Row> {
        let entries = entries.into_iter().collect::<Vec<_>>();
        let mut rows = Vec::with_capacity(self.blocks.len());
        for (index, block) in self.blocks.iter().enumerate() {
            let grouped = block.end - block.start - usize::from(block.active.is_some());
            if grouped > 1 {
                rows.push(Row {
                    block: index,
                    activity: None,
                });
                let first = &entries[self.activities[block.start].entry];
                if expanded.contains(&first.id) {
                    rows.extend(
                        (block.start..block.end)
                            .filter(|i| Some(*i) != block.active)
                            .map(|activity| Row {
                                block: index,
                                activity: Some(activity),
                            }),
                    );
                }
            } else if grouped == 1 {
                rows.push(Row {
                    block: index,
                    activity: (block.start..block.end).find(|i| Some(*i) != block.active),
                });
            }
            if let Some(active) = block.active {
                rows.push(Row {
                    block: index,
                    activity: Some(active),
                });
            }
        }
        rows
    }

    /// A prepend can extend a group or turn a standalone reading anchor into a
    /// group member. Preserve expansion and keep that exact record visible.
    pub fn restore_expansion<'a>(
        &self,
        entries: impl IntoIterator<Item = &'a Entry>,
        previous: &HashSet<String>,
        reading_entry: Option<&str>,
    ) -> HashSet<String> {
        let entries = entries.into_iter().collect::<Vec<_>>();
        let mut expanded = HashSet::new();
        for block in self.blocks.iter().filter(|block| block.is_group()) {
            if self.activities[block.start..block.end].iter().any(|a| {
                let id = &entries[a.entry].id;
                previous.contains(id) || reading_entry == Some(id.as_str())
            }) {
                expanded.insert(entries[self.activities[block.start].entry].id.clone());
            }
        }
        expanded
    }
}

pub fn arguments(entry: &Entry) -> Option<&Value> {
    entry.raw.iter().find_map(|r| r.get("arguments"))
}

pub fn input(entry: &Entry) -> Option<&Value> {
    entry.raw.iter().find_map(|r| r.get("event")?.get("input"))
}

fn field(value: &Value, name: &str) -> Option<String> {
    value
        .get(name)?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// A model reply is rendered as Markdown, so it keeps its text intact up to the
/// output disclosure limit instead of the one-line preview.
pub const MODEL_TEXT_LIMIT: usize = 65_536;

pub fn model_text(text: &str) -> String {
    text.chars().take(MODEL_TEXT_LIMIT).collect()
}

/// Collapse whitespace without allocating or shaping an entire tool response.
pub fn preview(text: &str) -> String {
    let mut output = String::new();
    let mut space = false;
    for (count, ch) in text.chars().take(4096).enumerate() {
        if count >= 512 {
            output.push('…');
            break;
        }
        if ch.is_whitespace() || ch.is_control() {
            space = !output.is_empty();
        } else {
            if space {
                output.push(' ');
                space = false;
            }
            output.push(ch);
        }
    }
    output
}

fn slack_subject(args: &Value) -> Option<Subject> {
    Some(Subject::Slack {
        channel: field(args, "channel_id")?,
        thread: field(args, "thread_ts")?,
    })
}

fn project(index: usize, entry: &Entry) -> Option<Activity> {
    let mut activity = project_summary(index, entry)?;
    activity.details = details(entry, &activity);
    Some(activity)
}

fn project_summary(index: usize, entry: &Entry) -> Option<Activity> {
    let args = arguments(entry).unwrap_or(&Value::Null);
    let mut result = Activity {
        entry: index,
        kind: Kind::UnknownTool,
        subject: None,
        summary: preview(&entry.summary),
        routine: None,
        requested_wait_ms: None,
        details: Details::default(),
        message: None,
    };
    if entry.lane == 1 {
        // The model lane is the assistant's own reply. A failed step is an
        // error; a completed step with text is the session output; a call that
        // is still running without text reads as the live thinking row.
        if matches!(entry.state.as_str(), "failed" | "timed_out") {
            result.kind = Kind::Error;
        } else if entry.state == "succeeded" && !result.summary.is_empty() {
            result.kind = Kind::Output;
            result.summary = model_text(&entry.summary);
        } else if entry.state == "running" && result.summary.is_empty() {
            result.kind = Kind::Thinking;
            result.routine = Some(Routine::Thinking);
        } else {
            return None;
        }
        return Some(result);
    }
    if entry.action == "input" {
        result.kind = Kind::Input;
        project_input(&mut result, entry);
        return Some(result);
    }
    if entry.lane != 2 {
        if !(entry.action.ends_with("failed") || entry.action == "runtime_fault") {
            return None;
        }
        result.kind = Kind::Error;
        return Some(result);
    }
    let conversation = Some(Subject::Conversation);
    let file = || field(args, "path").map(Subject::File);
    let (kind, subject, body) = match entry.action.as_str() {
        "chat.post_message" | "slack.post_message" => (
            Kind::SendMessage,
            if entry.action.starts_with("slack.") {
                slack_subject(args)
            } else {
                conversation
            },
            field(args, "text"),
        ),
        "chat.post_file" | "slack.post_file" => (
            Kind::SendFile,
            if entry.action.starts_with("slack.") {
                slack_subject(args)
            } else {
                conversation
            },
            field(args, "file_path")
                .or_else(|| field(&args["attachments"][0], "file_path"))
                .map(
                    |path| match field(args, "initial_comment").or_else(|| field(args, "text")) {
                        Some(comment) => format!("{path} · {comment}"),
                        None => path,
                    },
                ),
        ),
        "chat.notify" => (Kind::Notify, conversation, field(args, "text")),
        "agent.assign" => (
            Kind::Assign,
            field(args, "worker_id").map(Subject::Agent),
            field(args, "goal"),
        ),
        "agent.rework" => (
            Kind::Rework,
            field(args, "task_id").map(Subject::Task),
            field(args, "goal"),
        ),
        "agent.workers" => (Kind::Workers, None, None),
        "agent.tasks" => (Kind::Tasks, None, None),
        "file.read" => (Kind::Read, file(), field(args, "path")),
        "file.write" => (Kind::Write, file(), field(args, "path")),
        "file.edit" => (Kind::Edit, file(), field(args, "path")),
        "shell.run" => (Kind::Shell, None, field(args, "command")),
        "browser" => (
            Kind::Browser,
            field(&args["action"], "tab_id").map(Subject::BrowserTab),
            field(&args["action"], "op").map(|op| {
                let detail = ["url", "selector", "text", "key"]
                    .iter()
                    .find_map(|key| field(&args["action"], key));
                detail.map_or_else(|| op.clone(), |value| format!("{op} · {value}"))
            }),
        ),
        "wait" => {
            result.requested_wait_ms = args["seconds"]
                .as_f64()
                .filter(|s| s.is_finite() && *s > 0.)
                .map(|s| (s * 1000.).ceil().min(i64::MAX as f64) as i64);
            (Kind::Wait, None, field(args, "reason"))
        }
        "end" => (Kind::End, None, None),
        "tool.cancel" => (
            Kind::Cancel,
            field(args, "invocation_id").map(Subject::Invocation),
            None,
        ),
        "tool.help" => (
            Kind::Help,
            field(args, "tool").map(Subject::Tool),
            field(args, "tool"),
        ),
        "history.list" => (Kind::History, None, None),
        "chat.history" | "slack.history" => (
            Kind::ChatHistory,
            if entry.action.starts_with("slack.") {
                slack_subject(args)
            } else {
                conversation
            },
            None,
        ),
        "job.register" => (
            Kind::Job,
            None,
            field(args, "kind").map(|kind| {
                field(args, "script")
                    .map_or_else(|| kind.clone(), |script| format!("{kind} · {script}"))
            }),
        ),
        _ => (Kind::UnknownTool, None, None),
    };
    result.kind = kind;
    result.subject = subject;
    if let Some(body) = body {
        result.summary = preview(&body);
    } else {
        // Tools without a user-facing body should not show their argument JSON
        // as prose. Their real result/status remains available in the detail.
        result.summary = String::new();
    }
    // Group ordinary tools regardless of their outcome. Failure counts remain
    // visible on the heading and the latest active member stays outside it.
    result.routine = match kind {
        Kind::Read => field(args, "path").map(|p| {
            if p.starts_with("zork://history/") {
                Routine::Query
            } else {
                Routine::Read(p)
            }
        }),
        Kind::Write => field(args, "path").map(Routine::Write),
        Kind::Edit => field(args, "path").map(Routine::Edit),
        Kind::Help | Kind::History | Kind::ChatHistory | Kind::Workers | Kind::Tasks => {
            Some(Routine::Query)
        }
        Kind::Shell => Some(Routine::Shell),
        Kind::UnknownTool | Kind::Cancel | Kind::Job => Some(Routine::Other),
        _ => None,
    };
    Some(result)
}

/// Only named public fields become prose. Arbitrary JSON stays out of the UI.
fn readable(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        if text.trim_start().starts_with(['{', '[']) {
            if text.len() > 128_000 {
                return String::new();
            }
            return serde_json::from_str::<Value>(text)
                .ok()
                .map(|v| readable(&v))
                .unwrap_or_default();
        }
        return model_text(text);
    }
    ["text", "message", "summary", "content", "error"]
        .into_iter()
        .find_map(|key| value[key].as_str())
        .map(model_text)
        .unwrap_or_default()
}

fn details(entry: &Entry, activity: &Activity) -> Details {
    let args = arguments(entry).unwrap_or(&Value::Null);
    let output = entry
        .raw
        .iter()
        .rev()
        .find_map(|r| (r["event"]["kind"] == "tool_result").then(|| &r["event"]["result"]["data"]))
        .unwrap_or(&Value::Null);
    let mut detail = Details::default();
    match activity.kind {
        // Parsed once with the summary; the complete Markdown body.
        Kind::Input => detail.text = activity.details.text.clone(),
        Kind::Output | Kind::Error | Kind::Notice => detail.text = entry.summary.clone(),
        Kind::Shell => {
            detail.command = field(args, "command").unwrap_or_default();
            detail.output = [output["stdout"].as_str(), output["stderr"].as_str()]
                .into_iter()
                .flatten()
                .map(model_text)
                .collect::<Vec<_>>()
                .join("\n");
            if detail.output.is_empty() {
                detail.output = readable(output);
            }
        }
        Kind::Read | Kind::Write | Kind::Edit => {
            let content = if activity.kind == Kind::Read {
                output
                    .as_str()
                    .or_else(|| output["content"].as_str())
                    .map(model_text)
                    .unwrap_or_default()
            } else {
                field(args, "content")
                    .or_else(|| field(args, "new"))
                    .unwrap_or_default()
            };
            if let Some(path) = field(args, "path") {
                if !content.is_empty() {
                    detail.files.push(File {
                        path,
                        content: model_text(&content),
                        previous: field(args, "old").map(|s| model_text(&s)),
                    });
                }
            }
            if detail.files.is_empty() {
                detail.output = readable(output);
            }
        }
        Kind::SendMessage | Kind::Notify => detail.text = field(args, "text").unwrap_or_default(),
        Kind::SendFile => detail.text = field(args, "initial_comment").unwrap_or_default(),
        Kind::Assign | Kind::Rework => detail.text = field(args, "goal").unwrap_or_default(),
        Kind::Wait => detail.text = field(args, "reason").unwrap_or_default(),
        _ => detail.text = readable(output),
    }
    if matches!(entry.state.as_str(), "failed" | "timed_out") && detail.output.is_empty() {
        detail.output = readable(output);
    }
    detail.truncated = [&detail.text, &detail.command, &detail.output]
        .into_iter()
        .any(|s| s.chars().count() > MODEL_TEXT_LIMIT);
    detail.text = model_text(&detail.text);
    detail.command = model_text(&detail.command);
    detail.output = model_text(&detail.output);
    detail
}

/// Resolve a session input into a message: its sender, Markdown text and
/// files. Station envelopes are trusted only on their own delivery path; a
/// direct user receipt keeps its authored text verbatim. A JSON payload of
/// unknown shape never becomes the message body: readable fields are shown and
/// the source stays in `Message::raw`.
fn project_input(result: &mut Activity, entry: &Entry) {
    let content = entry.summary.as_str();
    let input = input(entry);
    let receipt = input.and_then(|input| input["request_id"].as_str());
    let position = input.and_then(|input| input["position"]["source"].as_str());
    let mut set = |subject: Option<Subject>, text: &str, message: Option<Message>| {
        result.subject = subject;
        result.summary = preview(text);
        result.details.text = text.to_owned();
        result.message = message.map(Box::new);
    };
    let payload = content
        .trim_start()
        .starts_with('{')
        .then(|| serde_json::from_str::<Value>(content).ok())
        .flatten();
    if let Some(payload) = &payload {
        if payload["event"] == "worker_result"
            && payload["event_id"]
                .as_str()
                .is_some_and(|id| receipt == Some(format!("worker-result-{id}").as_str()))
        {
            let text = payload["result"].as_str().unwrap_or_default();
            set(field(payload, "worker_id").map(Subject::Agent), text, None);
            return;
        }
        // Chat deliveries and Agent-to-Agent messages arrive through the
        // ordered mailbox; assignments through their own request receipts.
        let station = match payload["source"].as_str() {
            Some("chat") => position.is_some_and(|s| s.starts_with("chat-")) || receipt.is_none(),
            Some("agent") => {
                position.is_some_and(|s| s.starts_with("agent-direct-")) || receipt.is_none()
            }
            Some("assignment") => receipt.is_none_or(|id| {
                ["assignment-", "mesh-", "rework-"]
                    .iter()
                    .any(|prefix| id.starts_with(prefix))
            }),
            _ => false,
        };
        if station {
            if let Some((subject, text, message)) = station_message(payload) {
                set(subject, &text, Some(message));
                return;
            }
        }
    }
    if receipt.is_some_and(|id| id.starts_with("worker-result-comment-")) {
        if let Some(body) = content.strip_prefix("Human comment on Task ") {
            if let Some((_, text)) = body.split_once("):\n") {
                let text = text
                    .split_once("\n\nReview this comment in the context of the Task.")
                    .map_or(text, |(content, _)| content);
                set(Some(Subject::User), text, None);
                return;
            }
        }
    }
    if receipt.is_none() {
        if let Some(body) = content.strip_prefix(
            "A broker-managed background job reported a new asynchronous event for this session.\n",
        ) {
            let job_id = body.lines().find_map(|line| line.strip_prefix("job_id: "));
            if let (Some(job), Some((_, summary))) = (job_id, body.split_once("\nsummary: ")) {
                set(Some(Subject::Source(job.to_owned())), summary, None);
                return;
            }
        }
        if let Some(envelope) = incoming_envelope(content) {
            // An IM user id is not a name; without a display name the source
            // stays unknown.
            let subject = field(&envelope["sender"], "display_name").map(Subject::Source);
            let message = Message {
                name: field(&envelope["sender"], "display_name"),
                files: file_names(&envelope["attachments"]),
                ..Default::default()
            };
            set(subject, &message_text(&envelope["text"]), Some(message));
            return;
        }
    }
    // Direct user messages have a Station request receipt. Do not interpret
    // user-authored JSON or prose as authoritative sender metadata, but do not
    // read a structured payload out as a JSON blob either.
    if let Some(payload) = payload.filter(|p| p.is_object() && content.len() <= 1024 * 1024) {
        let raw = serde_json::to_string_pretty(&payload).ok();
        let text = readable_payload(&payload);
        set(
            None,
            &text,
            Some(Message {
                files: file_names(&payload["attachments"]),
                raw,
                ..Default::default()
            }),
        );
        return;
    }
    let (text, files) = attachment_suffix(content);
    let files = files.unwrap_or_default();
    set(
        None,
        text,
        (!files.is_empty()).then(|| Message {
            files,
            ..Default::default()
        }),
    );
}

/// Station's delivery envelopes: `chat` (a Chat message delivered to this
/// member), `agent` (a direct message from another Agent) and `assignment`
/// (work handed over by a leader).
fn station_message(payload: &Value) -> Option<(Option<Subject>, String, Message)> {
    let origin = |value: &Value| {
        value
            .as_str()
            .filter(|origin| !origin.is_empty() && *origin != "local")
            .map(str::to_owned)
    };
    match payload["source"].as_str()? {
        "chat" => {
            let message = payload.get("message").filter(|m| m.is_object())?;
            let author = &message["author"];
            let id = author["id"].as_str().unwrap_or_default();
            let subject = match author["kind"].as_str() {
                Some("user") => Some(Subject::User),
                // A Session member has no Agent record to resolve.
                Some("agent") if !id.is_empty() && !id.starts_with("session:") => {
                    Some(Subject::Agent(id.to_owned()))
                }
                _ => None,
            };
            let mut text = message_text(&message["text"]);
            if text.trim().is_empty() {
                text = message
                    .get("interaction")
                    .map(readable_payload)
                    .unwrap_or_default();
            }
            Some((
                subject,
                text,
                Message {
                    name: field(author, "name"),
                    origin: origin(&payload["target"]),
                    files: file_names(&message["attachments"]),
                    reply_to: field(message, "reply_to"),
                    raw: None,
                },
            ))
        }
        "agent" => {
            let author = &payload["author"];
            let (text, files) = attachment_suffix(payload["text"].as_str()?);
            Some((
                field(author, "agent").map(Subject::Agent),
                text.to_owned(),
                Message {
                    origin: origin(&author["origin"]),
                    files: files.unwrap_or_default(),
                    ..Default::default()
                },
            ))
        }
        "assignment" => {
            let (text, files) = attachment_suffix(payload["text"].as_str()?);
            Some((
                None,
                text.to_owned(),
                Message {
                    origin: origin(&payload["target"]),
                    files: files.unwrap_or_default(),
                    ..Default::default()
                },
            ))
        }
        _ => None,
    }
}

/// Text that may itself be a Zork file envelope.
fn message_text(value: &Value) -> String {
    let text = value.as_str().unwrap_or_default();
    crate::files::decode(text).map_or_else(|| text.to_owned(), |(text, _)| text)
}

fn file_names(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|file| {
            ["name", "title", "filename", "file_name"]
                .into_iter()
                .find_map(|key| field(file, key))
                .or_else(|| {
                    field(file, "path").map(|p| p.rsplit(['/', '\\']).next().unwrap_or(&p).into())
                })
        })
        .take(crate::files::MAX_FILES)
        .collect()
}

/// Station appends local attachment snapshots to text handed to an Agent.
const ATTACHMENTS: &str =
    "\n\nConversation attachments (local snapshots, available to file tools):\n";
fn attachment_suffix(text: &str) -> (&str, Option<Vec<String>>) {
    match text.rsplit_once(ATTACHMENTS) {
        Some((body, list)) if list.len() <= 256 * 1024 => {
            match serde_json::from_str::<Value>(list.trim()) {
                Ok(list @ Value::Array(_)) => (body, Some(file_names(&list))),
                _ => (text, None),
            }
        }
        _ => (text, None),
    }
}

/// The readable prose of a payload whose shape is not known.
fn readable_payload(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_owned();
    }
    for key in [
        "text", "message", "body", "content", "summary", "result", "goal", "error",
    ] {
        match &value[key] {
            Value::String(text) if !text.trim().is_empty() => return message_text(&value[key]),
            nested @ Value::Object(_) => {
                let text = readable_payload(nested);
                if !text.is_empty() {
                    return text;
                }
            }
            _ => {}
        }
    }
    String::new()
}

/// Station's current IM envelope. Do not guess a sender from arbitrary prose.
fn incoming_envelope(content: &str) -> Option<Value> {
    for marker in [
        "structured_message_json:\n```json\n",
        "observed_message_json:\n```json\n",
    ] {
        if let Some((_, rest)) = content.split_once(marker) {
            let json = rest.split_once("\n```")?.0;
            if json.len() > 1024 * 1024 {
                return None;
            }
            return serde_json::from_str(json).ok();
        }
    }
    None
}

#[cfg(test)]
mod tests;
