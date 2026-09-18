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
    Received,
    /// The assistant's own reply text, from the model lane.
    Model,
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
    Shell,
    Query,
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
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub read: usize,
    pub written: usize,
    pub shell: usize,
    pub queries: usize,
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
            let mut end = start + 1;
            if activities[start].routine.is_some() {
                while end < activities.len() && activities[end].routine.is_some() {
                    end += 1;
                }
            }
            let mut reads = BTreeSet::new();
            let mut writes = BTreeSet::new();
            let mut counts = Counts::default();
            let mut labels = Vec::new();
            let mut start_at = None;
            let mut end_at = None;
            let mut running = false;
            for activity in &activities[start..end] {
                entry_to_block[activity.entry] = Some(blocks.len());
                let entry = &entries[activity.entry];
                if let Some(time) = entry.start.or(entry.end) {
                    start_at = Some(start_at.map_or(time, |previous: i64| previous.min(time)));
                }
                if let Some(time) = entry.end.or(entry.start) {
                    end_at = Some(end_at.map_or(time, |previous: i64| previous.max(time)));
                }
                running |= entry.state == "running";
                match &activity.routine {
                    Some(Routine::Read(path)) => {
                        reads.insert(path);
                    }
                    Some(Routine::Write(path)) => {
                        writes.insert(path);
                    }
                    Some(Routine::Shell) => counts.shell += 1,
                    Some(Routine::Query) => counts.queries += 1,
                    None => {}
                }
                if labels.len() < 3 && !activity.summary.is_empty() {
                    labels.push(activity.summary.as_str());
                }
            }
            counts.read = reads.len();
            counts.written = writes.len();
            blocks.push(Block {
                start,
                end,
                counts,
                summary: preview(&labels.join(" · ")),
                start_at,
                end_at,
                running,
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
            if block.is_group() {
                rows.push(Row {
                    block: index,
                    activity: None,
                });
                let first = &entries[self.activities[block.start].entry];
                if expanded.contains(&first.id) {
                    rows.extend((block.start..block.end).map(|activity| Row {
                        block: index,
                        activity: Some(activity),
                    }));
                }
            } else {
                rows.push(Row {
                    block: index,
                    activity: Some(block.start),
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
/// same bound Cue's output disclosure uses, instead of the one-line preview.
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
    let args = arguments(entry).unwrap_or(&Value::Null);
    let mut result = Activity {
        entry: index,
        kind: Kind::UnknownTool,
        subject: None,
        summary: preview(&entry.summary),
        routine: None,
        requested_wait_ms: None,
    };
    if entry.lane == 1 {
        // The model lane is the assistant's own reply. A failed step is an
        // error; a step still running has text that can still change, and a
        // step without text already lists its calls on their own rows.
        if matches!(entry.state.as_str(), "failed" | "timed_out") {
        result.kind = Kind::Error;
        } else if entry.state == "succeeded" && !result.summary.is_empty() {
            result.kind = Kind::Model;
            result.summary = model_text(&entry.summary);
        } else {
            return None;
        }
        return Some(result);
    }
    if entry.action == "input" {
        result.kind = Kind::Received;
        let receipt = input(entry).and_then(|input| input["request_id"].as_str());
        if let Some(payload) = entry
            .summary
            .starts_with('{')
            .then(|| serde_json::from_str::<Value>(&entry.summary).ok())
            .flatten()
        {
            if payload["event"] == "worker_result"
                && payload["event_id"]
                    .as_str()
                    .is_some_and(|id| receipt == Some(format!("worker-result-{id}").as_str()))
            {
                result.summary = preview(payload["result"].as_str().unwrap_or(&entry.summary));
                result.subject = field(&payload, "worker_id").map(Subject::Agent);
                return Some(result);
            }
        }
        // Direct user messages have a Station request receipt. Do not interpret
        // user-authored JSON or prose as authoritative sender metadata.
        if receipt.is_some_and(|id| id.starts_with("worker-result-comment-")) {
            if let Some(body) = entry.summary.strip_prefix("Human comment on Task ") {
                if let Some((_, text)) = body.split_once("):\n") {
                    result.subject = Some(Subject::User);
                    result.summary = preview(
                        text.split_once("\n\nReview this comment in the context of the Task.")
                            .map_or(text, |(content, _)| content),
                    );
                    return Some(result);
                }
            }
        }
        if receipt.is_some() {
            return Some(result);
        }
        if let Some(body) = entry.summary.strip_prefix(
            "A broker-managed background job reported a new asynchronous event for this session.\n",
        ) {
            let job_id = body.lines().find_map(|line| line.strip_prefix("job_id: "));
            if let (Some(job), Some((_, summary))) = (job_id, body.split_once("\nsummary: ")) {
                result.subject = Some(Subject::Source(job.to_owned()));
                result.summary = preview(summary);
                return Some(result);
            }
        }
        if let Some(envelope) = incoming_envelope(&entry.summary) {
            result.summary = preview(envelope["text"].as_str().unwrap_or(&entry.summary));
            result.subject = envelope["sender"]["display_name"]
                .as_str()
                .or_else(|| envelope["sender"]["user_id"].as_str())
                .map(|name| Subject::Source(name.to_owned()));
        }
        return Some(result);
    }
    if entry.lane != 2 {
        result.kind = if entry.action.ends_with("failed") || entry.action == "runtime_fault" {
            Kind::Error
        } else {
            Kind::Notice
        };
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
    } else if arguments(entry).is_some() {
        // Tools without a user-facing body should not show their argument JSON
        // as prose. Their real result/status remains available in the detail.
        result.summary = String::new();
    }
    if matches!(entry.state.as_str(), "failed" | "timed_out" | "cancelled") {
        if let Some(outcome) = entry.outcome_summary.as_ref().filter(|s| !s.is_empty()) {
            result.summary = preview(outcome);
        }
    }
    // Unknown, unfinished, failed, externally visible, and waiting operations
    // are never swallowed by a routine group.
    if entry.state == "succeeded" {
        result.routine = match kind {
            Kind::Read => field(args, "path").map(|p| {
                if p.starts_with("zork://history/") {
                    Routine::Query
                } else {
                    Routine::Read(p)
                }
            }),
            Kind::Write | Kind::Edit => field(args, "path").map(Routine::Write),
            Kind::Help | Kind::History | Kind::ChatHistory | Kind::Workers | Kind::Tasks => {
                Some(Routine::Query)
            }
            Kind::Shell if args["command"].as_str().is_some_and(routine_command) => {
                Some(Routine::Shell)
            }
            _ => None,
        };
    }
    Some(result)
}

/// Only known simple inspection commands are eligible. A successful shell tool
/// is not in itself low importance (tests, installs and deployments stay visible).
pub fn routine_command(command: &str) -> bool {
    if command.chars().any(|c| {
        matches!(
            c,
            '\n' | '\r' | ';' | '&' | '|' | '>' | '<' | '`' | '$' | '(' | ')' | '\\'
        )
    }) {
        return false;
    }
    let words: Vec<_> = command.split_whitespace().collect();
    if words.iter().any(|w| {
        w.contains("--pre")
            || w.starts_with("--exec")
            || w.starts_with("--output")
            || w.starts_with("--ext-diff")
            || w.starts_with("--textconv")
    }) {
        return false;
    }
    match words.first().copied() {
        Some("pwd" | "ls" | "rg" | "grep" | "cat" | "head" | "tail" | "wc") => true,
        Some("git") => matches!(
            words.get(1).copied(),
            Some("status" | "diff" | "log" | "show" | "ls-files")
        ),
        _ => false,
    }
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
