//! A bounded execution-history reader over the shared session controller.
//! Entries, event matching, cursors and detail lookup remain in core RAM.
use crate::state::{self, ConversationTopics, Device, Domains, HistoryData};
use anyhow::Result;
use serde_json::{json, Value};
use std::sync::Arc;
use zork_client_types::history::{activity, Entry};
use zork_observe::{BatchId, List, Readiness};

const PAGE: usize = 100;

struct Pending {
    device: Option<BatchId>,
    conversation: Option<BatchId>,
    history: Option<BatchId>,
    data: Arc<HistoryData>,
    window: List<Entry>,
    start: usize,
    anchor: Option<(String, isize)>,
}

pub(super) struct HistoryWire {
    peer: String,
    session: String,
    device: Arc<Device>,
    // Retain the shared producer, including its overview stream.
    _conversation: Arc<state::Conversation>,
    history: Arc<state::History>,
    device_updates: state::DeviceSubscription,
    conversation_updates: state::ConversationSubscription,
    history_updates: state::HistorySubscription,
    data: Arc<HistoryData>,
    window: List<Entry>,
    start: usize,
    anchor: Option<(String, isize)>,
    detail: Option<String>,
    pending: Option<Pending>,
}

impl HistoryWire {
    pub(super) fn new(peer: String, session: String, device: Arc<Device>) -> Result<Self> {
        crate::valid_session(&session)?;
        let conversation = device.conversation(&session);
        let history = conversation.history();
        Ok(Self {
            peer,
            session,
            device_updates: device.subscribe_domains(
                Domains::CONNECTION | Domains::AGENTS | Domains::SESSIONS | Domains::TASKS,
            ),
            conversation_updates: conversation.subscribe_topics(ConversationTopics::OVERVIEW),
            history_updates: history.subscribe(),
            device,
            _conversation: conversation,
            history,
            data: Arc::new(HistoryData::default()),
            window: List::new(),
            start: 0,
            anchor: None,
            detail: None,
            pending: None,
        })
    }

    pub(super) fn signals(&self) -> Vec<Readiness> {
        vec![
            self.device_updates.readiness(),
            self.conversation_updates.readiness(),
            self.history_updates.readiness(),
        ]
    }

    pub(super) fn prepare(&mut self) -> Result<Option<(Value, bool, bool)>> {
        let device = self.device_updates.prepare();
        let conversation = self.conversation_updates.prepare();
        let history = self.history_updates.prepare();
        if device.is_none() && conversation.is_none() && history.is_none() {
            return Ok(None);
        }
        let data = history
            .as_ref()
            .map(|h| h.state.clone())
            .unwrap_or_else(|| self.data.clone());
        let revoked = self.device.snapshot().revoked
            || data.revoked
            || conversation.as_ref().is_some_and(|c| c.state.revoked);
        let reset = revoked || history.as_ref().is_some_and(|h| h.reset);
        let metadata = device
            .as_ref()
            .map(|d| d.state.clone())
            .unwrap_or_else(|| self.device.snapshot());
        let metadata_changed = device.as_ref().is_some_and(|d| {
            d.domains
                .contains(Domains::AGENTS | Domains::SESSIONS | Domains::TASKS)
        });
        let mut value = json!({"peer":self.peer,"session":self.session});
        let mut window = self.window.clone();
        let mut start = self.start;
        let mut anchor = self.anchor.clone();
        if let Some(update) = &history {
            let range = self.range(&data);
            start = range.start;
            if update.reset {
                window = data.entries.slice(range.clone());
                value["entries"] = json!(window
                    .iter()
                    .map(|entry| row(entry, &metadata, &self.session))
                    .collect::<Vec<_>>());
            } else if let Some(changes) = &update.entries {
                let (next, edits) = super::conversation::project(
                    &window,
                    self.start,
                    &changes.edits,
                    &data.entries,
                    range.clone(),
                );
                window = next;
                value["entry_edits"] = json!(edits
                    .iter()
                    .map(|edit| json!({
                        "start":edit.remove.start,"end":edit.remove.end,
                        "insert":edit.insert.iter().map(|entry| row(entry, &metadata, &self.session)).collect::<Vec<_>>()
                    }))
                    .collect::<Vec<_>>());
            }
            if anchor.is_some() && !data.loading_older {
                anchor = window.first().map(|entry| (entry.id.clone(), 0));
            }
            value["loading"] = json!(data.loading);
            value["loaded"] = json!(data.loaded);
            value["error"] = json!(data.error);
            value["older"] = json!(start > 0 || data.older.is_some());
            value["newer"] = json!(range.end < data.entries.len());
            value["total"] = json!(data.entries.len());
            value["window_start"] = json!(start);
            value["clock_offset_ms"] = json!(data.clock_offset_ms);
            // Full outputs/JSON cross the wire only for the explicitly opened record.
            if update.reset
                || self.detail.as_ref().is_some_and(|id| {
                    update
                        .entries
                        .as_ref()
                        .is_some_and(|changes| changes.changed_ids.contains(id))
                })
            {
                value["detail"] = self
                    .detail
                    .as_deref()
                    .and_then(|id| data.lookup.index_of(id))
                    .and_then(|at| data.entries.get(at))
                    .map(detail)
                    .unwrap_or(Value::Null);
            }
        }
        if metadata_changed && value.get("entries").is_none() {
            // Names and navigable identities change independently of the ledger.
            // This remains a bounded window, never an archive-wide projection.
            value["entries"] = json!(window
                .iter()
                .map(|entry| row(entry, &metadata, &self.session))
                .collect::<Vec<_>>());
            value.as_object_mut().unwrap().remove("entry_edits");
        }
        if reset || !window.ptr_eq(&self.window) {
            value["blocks"] = blocks(&window);
        }
        if let Some(update) = &conversation {
            let overview = &update.state.overview;
            let usage = overview.usage();
            let known = overview.aggregates.complete && usage.reported_steps > 0;
            value["overview"] = json!({
                "model":overview.runtime.as_ref().and_then(|r| r.model.as_deref()),
                "thinking":overview.runtime.as_ref().and_then(|r| r.thinking.as_deref()),
                "context_tokens":overview.runtime.as_ref().and_then(|r| r.context_tokens),
                "context_limit":overview.runtime.as_ref().and_then(|r| r.context_limit),
                "input":known.then_some(usage.input),"output":known.then_some(usage.output),
                "total":known.then_some(usage.input.saturating_add(usage.output)),
                "cached":(known && usage.cache_reported_steps > 0).then_some(usage.cached),
                "cache_hit_rate":known.then(|| usage.cache_hit_rate()).flatten(),
                "partial_cache":usage.cache_reported_steps < usage.reported_steps,
                "profile":overview.runtime.as_ref().and_then(|r| r.profile.as_ref()).map(|profile| json!({
                    "name":profile.display_name(),"provider":profile.provider,
                    "quota":profile.quota(),"checked_at":profile.checked_at,
                })),
            });
        }
        if revoked {
            window.clear();
            start = 0;
            anchor = None;
            self.detail = None;
            value = json!({"peer":self.peer,"session":self.session,"revoked":true,
                "entries":[],"blocks":[],"detail":null,"overview":null,"loading":false,"loaded":true,
                "older":false,"newer":false,"total":0,"window_start":0,
                "error":"设备访问权限已撤销"});
        }
        self.pending = Some(Pending {
            device: device.and_then(|d| d.batch),
            conversation: conversation.and_then(|c| c.batch),
            history: history.and_then(|h| h.batch),
            data,
            window,
            start,
            anchor,
        });
        Ok(Some((json!({"history":value}), reset, revoked)))
    }

    pub(super) fn valid(&self) -> bool {
        self.pending.as_ref().is_some_and(|p| {
            p.device.is_none_or(|id| self.device_updates.valid(id))
                && p.conversation
                    .is_none_or(|id| self.conversation_updates.valid(id))
                && p.history.is_none_or(|id| self.history_updates.valid(id))
        })
    }

    pub(super) fn finish(&mut self, applied: bool) -> bool {
        let Some(pending) = self.pending.take() else {
            return false;
        };
        let mut accepted = true;
        if let Some(id) = pending.device {
            accepted &= if applied {
                self.device_updates.acknowledge(id)
            } else {
                self.device_updates.discard(id)
            };
        }
        if let Some(id) = pending.conversation {
            accepted &= if applied {
                self.conversation_updates.acknowledge(id)
            } else {
                self.conversation_updates.discard(id)
            };
        }
        if let Some(id) = pending.history {
            accepted &= if applied {
                self.history_updates.acknowledge(id)
            } else {
                self.history_updates.discard(id)
            };
        }
        if applied && accepted {
            self.data = pending.data;
            self.window = pending.window;
            self.start = pending.start;
            self.anchor = pending.anchor;
        }
        if !accepted {
            self.device_updates.reset();
            self.conversation_updates.reset();
            self.history_updates.reset();
        }
        accepted
    }

    pub(super) fn older(&mut self) {
        self.anchor = self
            .window
            .first()
            .map(|e| (e.id.clone(), -(PAGE as isize)));
        if self.start == 0 {
            self.history.load(true);
        }
        self.history_updates.reset();
    }
    pub(super) fn newer(&mut self) {
        if self.start + self.window.len() >= self.data.entries.len() {
            return;
        }
        self.anchor = self.window.first().map(|e| (e.id.clone(), PAGE as isize));
        self.history_updates.reset();
    }
    pub(super) fn window_anchor(&mut self, id: Option<String>) -> Result<()> {
        if let Some(id) = &id {
            anyhow::ensure!(
                self.data.lookup.index_of(id).is_some(),
                "执行记录已移出当前范围，请重试"
            );
        }
        if self
            .anchor
            .as_ref()
            .map(|(id, offset)| (id.as_str(), *offset))
            == id.as_deref().map(|id| (id, 0))
        {
            return Ok(());
        }
        self.anchor = id.map(|id| (id, 0));
        self.history_updates.reset();
        Ok(())
    }
    pub(super) fn detail(&mut self, id: Option<String>) -> Result<()> {
        if let Some(id) = &id {
            anyhow::ensure!(
                self.data.lookup.index_of(id).is_some(),
                "执行记录已移出当前范围，请重试"
            );
        }
        self.detail = id;
        self.history_updates.reset();
        Ok(())
    }
    pub(super) fn refresh(&self) {
        self.history.load(false);
    }

    fn range(&self, data: &HistoryData) -> std::ops::Range<usize> {
        let start = self
            .anchor
            .as_ref()
            .and_then(|(id, offset)| {
                data.lookup
                    .index_of(id)
                    .map(|at| at.saturating_add_signed(*offset))
            })
            .unwrap_or_else(|| data.entries.len().saturating_sub(PAGE));
        let start = start.min(data.entries.len());
        start..start.saturating_add(PAGE).min(data.entries.len())
    }
}

fn row(entry: &Entry, metadata: &state::DeviceData, session: &str) -> Value {
    let projection = activity::Projection::new(std::iter::once(entry));
    let activity = projection.activities.first();
    json!({"id":entry.id,"action":entry.action,"state":entry.state,"lane":entry.lane,
        "visible":activity.is_some(),
        "kind":activity.map(|a| a.kind),
        "subject":activity.and_then(|a| a.subject.as_ref()).map(|subject| subject_value(subject, metadata, session)),
        "requested_wait_ms":activity.and_then(|a| a.requested_wait_ms),
        "preview":activity.map(|a| a.summary.clone()).unwrap_or_else(|| activity::preview(&entry.summary)),
        "start":entry.start,"end":entry.end,"model":entry.model})
}

fn blocks(entries: &List<Entry>) -> Value {
    let projection = activity::Projection::new(entries.iter());
    json!(projection
        .blocks
        .iter()
        .map(|block| {
            let members = projection.activities[block.start..block.end]
                .iter()
                .map(|a| entries[a.entry].id.as_str())
                .collect::<Vec<_>>();
            json!({"id":members[0],"members":members,"grouped":block.is_group(),
            "summary":block.summary,"read":block.counts.read,"written":block.counts.written,
            "shell":block.counts.shell,"queries":block.counts.queries,
            "start":block.start_at,"end":block.end_at})
        })
        .collect::<Vec<_>>())
}

fn subject_value(subject: &activity::Subject, data: &state::DeviceData, session: &str) -> Value {
    use activity::Subject;
    let conversation = |id: &str| {
        data.sessions.iter().find(|s| s.session_id == id).map(|s| {
            let name = s
                .title
                .as_deref()
                .or_else(|| s.task.as_ref().map(|t| t.title.as_str()))
                .unwrap_or(id);
            json!({"label":name,"conversation":{"id":s.session_id,"title":name,
            "can_send":crate::conversation::can_send(s),"can_stop":crate::composer::can_stop(s)}})
        })
    };
    match subject {
        Subject::Conversation => {
            conversation(session).unwrap_or_else(|| json!({"label":"当前对话"}))
        }
        Subject::User => json!({"label":"用户"}),
        Subject::Agent(id) => data
            .agents
            .iter()
            .find(|a| a["id"] == *id)
            .map(|agent| {
                json!({"label":agent["name"].as_str().unwrap_or(id),"agent":{
                "id":id,"name":agent["name"],"avatar":agent["avatar"],"role":agent["role"],
                "model":agent["model"],"profile":agent["profile_id"],"thinking":agent["thinking"]}})
            })
            .unwrap_or_else(|| json!({"label":id})),
        Subject::Task(id) => data
            .tasks
            .values()
            .flatten()
            .find(|task| task.task_id == *id)
            .map(|task| {
                conversation(&task.conversation_id).unwrap_or_else(|| json!({"label":task.title}))
            })
            .unwrap_or_else(|| json!({"label":id})),
        Subject::Slack { channel, thread } => json!({"label":format!("{channel} · {thread}")}),
        Subject::Source(label)
        | Subject::File(label)
        | Subject::Tool(label)
        | Subject::Invocation(label)
        | Subject::BrowserTab(label) => json!({"label":label}),
    }
}

fn detail(entry: &Entry) -> Value {
    json!({"id":entry.id,"action":entry.action,"state":entry.state,"lane":entry.lane,
        "summary":entry.summary,"outcome":entry.outcome_summary,"model":entry.model,
        "start":entry.start,"end":entry.end,"usage":entry.usage,
        "stages":entry.raw.iter().map(|raw| json!({
            "kind":if raw.get("arguments").is_some() {"request"} else if raw.get("outcome").is_some() {"result"}
                else {match raw["event"]["kind"].as_str() {Some("step_started") => "start", Some("step_completed" | "step_failed") => "end", _ => "event"}},
            "json":serde_json::to_string_pretty(raw).unwrap()
        })).collect::<Vec<_>>()})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        api::StationClient,
        store::ClientStore,
        subscriptions::{Key, WireSubscription},
    };
    use zork_client_types::history::Record;

    fn record(i: usize) -> Record {
        Record {
            event_id: format!("e{i}"),
            metadata: Default::default(),
            event: json!({"kind":"input_appended","input":{"content":format!("input {i}"),"received_at_ms":i}}),
        }
    }

    #[test]
    fn reading_groups_share_the_native_rules_and_leave_waits_errors_and_unknown_tools_visible() {
        let entry = |id: &str, tool: &str, arguments: Value, state: &str| Entry {
            id: id.into(),
            lane: 2,
            action: tool.into(),
            summary: String::new(),
            start: Some(10),
            end: Some(20),
            state: state.into(),
            raw: vec![json!({"arguments":arguments})],
            usage: None,
            model: None,
            outcome_summary: None,
        };
        let entries: List<_> = vec![
            entry("a", "file.read", json!({"path":"/a"}), "succeeded"),
            entry("b", "file.read", json!({"path":"/a"}), "succeeded"),
            entry("c", "file.write", json!({"path":"/b"}), "succeeded"),
            entry("d", "file.edit", json!({"path":"/b"}), "succeeded"),
            entry("e", "shell.run", json!({"command":"pwd"}), "succeeded"),
            entry("f", "history.list", json!({}), "succeeded"),
            entry("wait", "wait", json!({"seconds":60}), "succeeded"),
            entry(
                "test",
                "shell.run",
                json!({"command":"cargo test"}),
                "succeeded",
            ),
            entry("error", "file.read", json!({"path":"/bad"}), "failed"),
            entry("custom", "dynamic.tool", json!({}), "succeeded"),
            Entry {
                lane: 1,
                ..entry("model", "model", json!({}), "succeeded")
            },
        ]
        .into();
        let groups = blocks(&entries);
        let groups = groups.as_array().unwrap();
        assert_eq!(groups.len(), 5);
        assert_eq!(groups[0]["members"].as_array().unwrap().len(), 6);
        assert_eq!(groups[0]["read"], 1);
        assert_eq!(groups[0]["written"], 1);
        assert_eq!(groups[0]["shell"], 1);
        assert_eq!(groups[0]["queries"], 1);
        assert!(groups[1..].iter().all(|g| g["grouped"] == false));
        let model = row(&entries[10], &state::DeviceData::default(), "chat");
        assert_eq!(model["visible"], false);
        assert_eq!(model["lane"], 1, "model calls remain on the timeline");
        assert!(subject_value(
            &activity::Subject::Agent("missing".into()),
            &state::DeviceData::default(),
            "chat"
        )
        .get("agent")
        .is_none());
    }

    #[test]
    fn complete_mobile_fixture_uses_the_real_ledger_and_wire_projection() {
        let now = 1_789_200_060_000_i64;
        let start = now - 60_000;
        let tools = vec![
            ("file.read", json!({"path":"src/main.rs"}), "succeeded"),
            ("file.read", json!({"path":"src/main.rs"}), "succeeded"),
            (
                "file.write",
                json!({"path":"notes.md","content":"# 检查结果\n保留源内容 👋"}),
                "succeeded",
            ),
            ("file.edit", json!({"path":"notes.md"}), "succeeded"),
            ("shell.run", json!({"command":"pwd"}), "succeeded"),
            ("history.list", json!({}), "succeeded"),
            (
                "wait",
                json!({"seconds":5,"reason":"等待检查完成"}),
                "succeeded",
            ),
            (
                "shell.run",
                json!({"command":"cargo test --locked"}),
                "succeeded",
            ),
            ("file.read", json!({"path":"missing.txt"}), "failed"),
            (
                "chat.post_message",
                json!({"text":"检查完成，结果可供审阅。"}),
                "succeeded",
            ),
            (
                "chat.post_file",
                json!({"file_path":"notes.md","initial_comment":"执行报告"}),
                "succeeded",
            ),
            (
                "chat.notify",
                json!({"text":"需要确认一项输入"}),
                "succeeded",
            ),
            (
                "agent.assign",
                json!({"worker_id":"worker-1","goal":"复查边界条件"}),
                "succeeded",
            ),
            (
                "agent.rework",
                json!({"task_id":"unknown-task","goal":"补充窄屏截图"}),
                "succeeded",
            ),
            ("agent.workers", json!({}), "succeeded"),
            ("agent.tasks", json!({}), "succeeded"),
            ("tool.help", json!({"tool":"file.read"}), "succeeded"),
            ("chat.history", json!({}), "succeeded"),
            (
                "browser",
                json!({"action":{"op":"navigate","url":"https://example.test"}}),
                "succeeded",
            ),
            (
                "job.register",
                json!({"kind":"script","script":"prepare_report"}),
                "succeeded",
            ),
            (
                "tool.cancel",
                json!({"invocation_id":"background-1"}),
                "succeeded",
            ),
            ("end", json!({}), "succeeded"),
            (
                "custom.inspection",
                json!({"value":"unknown tools stay visible"}),
                "succeeded",
            ),
            (
                "wait",
                json!({"seconds":60,"reason":"等待外部反馈"}),
                "running",
            ),
        ];
        let mut records = vec![Record {
            event_id: "input".into(),
            metadata: Default::default(),
            event: json!({"kind":"input_appended","input":{"content":"检查项目，整理报告并等待反馈。","received_at_ms":start,"request_id":"user-input"}}),
        }];
        for (i, (tool, arguments, outcome)) in tools.iter().enumerate() {
            let time = start + i as i64 * 2000 + 500;
            if i == 7 {
                records.push(Record {event_id:"wake-input".into(),metadata:Default::default(),
                    event:json!({"kind":"input_appended","input":{"input_id":"wake-input","content":"检查完成，继续整理报告。","received_at_ms":time-100}})});
            }
            for (suffix, event) in [
                (
                    "model-start",
                    json!({"kind":"step_started","step_id":format!("step-{i}"),"started_at_ms":time,
                        "consumed_inputs":if i == 7 {vec!["wake-input"]} else {vec![]}}),
                ),
                (
                    "model-end",
                    json!({"kind":"step_completed","step_id":format!("step-{i}"),"completed_at_ms":time+400,
                    "assistant_text":"模型内部推理仅在执行详情中查看。", "usage":{"input_tokens":1200,"output_tokens":100,"cached_input_tokens":600},
                    "invocations":[{"invocation_id":format!("call-{i}"),"tool":tool,"started_at_ms":time+450,"arguments":arguments}]}),
                ),
            ] {
                records.push(Record {
                    event_id: format!("{i}-{suffix}"),
                    event,
                    metadata: Default::default(),
                });
            }
            if *outcome != "running" {
                records.push(Record { event_id:format!("{i}-result"), metadata:Default::default(),
                    event:json!({"kind":"tool_result","result":{"invocation_id":format!("call-{i}"),"tool":tool,"outcome":outcome,"finished_at_ms":time+1100,
                        "data":if *outcome == "failed" {json!({"error":"文件不存在：missing.txt"})} else if *tool == "wait" {json!({"until_ms":time+5450})}
                            else {json!({"stdout":if i == 7 {"通过：中英文、长文本与 emoji 👋\n".repeat(260)} else {format!("完成操作 {i}")}})}}}) });
            }
        }
        records.push(Record {event_id:"provider-error".into(),metadata:Default::default(),event:json!({"kind":"step_failed","step_id":"failed-model","failed_at_ms":now-5000,"error":{"message":"测试提供商暂时不可用"}})});
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(ClientStore::open(root.path()).unwrap());
        let device = Device::open(
            Arc::new(StationClient::new("http://127.0.0.1:9", None)),
            None,
            true,
        );
        let conversation = device.conversation("session-a");
        let source = conversation.history();
        source.seed(HistoryData {
            records: records.into(),
            loaded: true,
            clock_offset_ms: now - crate::store::delivery_now_ms() as i64,
            ..Default::default()
        });
        conversation.seed(state::ConversationData {overview:Arc::new(state::SessionOverview {
            loaded:true, runtime:Some(state::HistoryRuntime {model:Some("fixture-model".into()),thinking:Some("high".into()),context_tokens:Some(2048),context_limit:Some(32768),
                profile:Some(serde_json::from_value(json!({"profile_id":"fixture","name":"测试连接","provider":"openai","checkedAt":"2026-09-12T08:00:00Z",
                    "rateLimits":{"rateLimits":{"primary":{"usedPercent":25,"windowDurationMins":300}}}})).unwrap())}),
            aggregates:zork_agent_api::SessionAggregates {complete:true,usage:zork_agent_api::SessionUsage {input:28800,output:2400,cached:14400,reported_steps:24,cache_reported_steps:24,cache_input:28800},..Default::default()},
            ..Default::default()
        }),..Default::default()});
        let mut reader = open(&device, &store, "session-a");
        let mut mirror = vec![];
        let frame = consume(&mut reader, &mut mirror);
        let snapshot = source.subscribe().snapshot().state;
        let mut metadata = state::DeviceData::default();
        metadata.agents = Arc::new(vec![
            json!({"id":"worker-1","name":"小熊","avatar":"bear","role":"worker","model":"fixture-model","profile_id":"fixture","thinking":"high"}),
        ]);
        metadata.sessions = Arc::new(vec![serde_json::from_value(json!({"session_id":"session-a","title":"工作对话","profile_id":"fixture","model":"fixture-model","thinking":"high","workspace":"fixture","status":"wait"})).unwrap()]);
        let mut history = frame["history"].clone();
        history["entries"] = json!(snapshot
            .entries
            .iter()
            .map(|entry| row(entry, &metadata, "session-a"))
            .collect::<Vec<_>>());
        let finished_wait = snapshot
            .entries
            .iter()
            .find(|entry| entry.id == "tool:call-6")
            .unwrap();
        assert_eq!(finished_wait.start, Some(start + 13_600));
        assert_eq!(finished_wait.end, Some(start + 14_500));
        assert_eq!(finished_wait.state, "succeeded");
        assert_eq!(
            snapshot
                .entries
                .iter()
                .filter(|entry| entry.action == "wait" && entry.state == "running")
                .count(),
            1
        );
        let details = snapshot
            .entries
            .iter()
            .map(|entry| (entry.id.clone(), detail(entry)))
            .collect::<serde_json::Map<_, _>>();
        assert!(history["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["grouped"] == true && b["read"] == 1 && b["written"] == 1));
        assert!(history["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["lane"] == 1 && e["visible"] == false));
        source.seed_records(vec![
            Record {event_id:"earlier-read".into(),metadata:Default::default(),event:json!({"kind":"step_completed","step_id":"earlier-step","completed_at_ms":start+650,
                "invocations":[{"invocation_id":"earlier-read","tool":"file.read","started_at_ms":start+700,"arguments":{"path":"src/extra.rs"}}]})},
            Record {event_id:"earlier-read-result".into(),metadata:Default::default(),event:json!({"kind":"tool_result","result":{"invocation_id":"earlier-read","tool":"file.read","outcome":"succeeded","finished_at_ms":start+900,"data":{"stdout":"earlier page"}}})},
        ], true);
        let prepended = consume(&mut reader, &mut mirror);
        if let Some(path) = std::env::var_os("ZORK_HISTORY_FIXTURE_OUTPUT") {
            std::fs::write(
                path,
                serde_json::to_vec_pretty(
                    &json!({"now_ms":now,"history":history,"details":details,"prepended":prepended["history"]}),
                )
                .unwrap(),
            )
            .unwrap();
        }
    }

    #[test]
    fn desktop_timeline_fixture_uses_the_same_records() {
        // Same event fixture as zork-gui/tests/headless_history_statistics.rs.
        // Shared ledger/wire projection supplies the Android comparison data.
        const NOW: i64 = 1_800_000_600_000;
        fn record(id: usize, event: Value) -> Record {
            Record {
                event_id: format!("{id:016x}"),
                event,
                metadata: Default::default(),
            }
        }
        fn fixture() -> Vec<Record> {
            let mut rows = vec![record(
                0,
                json!({"kind":"input_appended", "input":{
                "input_id":"input-0", "request_id":"fixture-user-message", "content":"保留时间线，连续常规操作合并；等待独立显示。", "received_at_ms":NOW-90000}}),
            )];
            for (i, name, args, time, state, data) in [
                (
                    1,
                    "file.read",
                    json!({"path":"crates/zork-client-types/src/history.rs"}),
                    NOW - 85000,
                    "succeeded",
                    json!({"content":"source"}),
                ),
                (
                    2,
                    "shell.run",
                    json!({"command":"rg -n history crates/zork-ui"}),
                    NOW - 83000,
                    "succeeded",
                    json!({"stdout":"matches"}),
                ),
                (
                    3,
                    "chat.post_message",
                    json!({"text":"我会调整条目层级，保留明确的消息来源和目标。","kind":"progress"}),
                    NOW - 75000,
                    "succeeded",
                    json!({"ok":true}),
                ),
                (
                    4,
                    "wait",
                    json!({"seconds":20,"reason":"等待后台任务的新消息"}),
                    NOW - 60000,
                    "succeeded",
                    json!({"until_ms":NOW-39990}),
                ),
                (
                    5,
                    "shell.run",
                    json!({"command":"cargo test --locked -p zork-client-types"}),
                    NOW - 25000,
                    "failed",
                    json!({"error":"fixture failure: history_empty_state"}),
                ),
            ] {
                rows.push(record(i*3, json!({"kind":"step_started","step_id":format!("s{i}"),"purpose":"conversation","started_at_ms":time-1000})));
                rows.push(record(i*3+1, json!({"kind":"step_completed","step_id":format!("s{i}"),"completed_at_ms":time,
                    "assistant_text":"INTERNAL-MODEL-TEXT-MUST-NOT-BE-A-MESSAGE", "invocations":[{
                        "invocation_id":format!("tool-{i}"),"tool":name,"arguments":args,"started_at_ms":time}]})));
                rows.push(record(i*3+2, json!({"kind":"tool_result","result":{
                    "invocation_id":format!("tool-{i}"),"tool":name,"outcome":state,"data":data,"finished_at_ms":time+10}})));
                if name == "wait" {
                    rows.push(record(100, json!({"kind":"input_appended","input":{"input_id":"wake", "content":"后台任务已完成。","received_at_ms":NOW-45000}})));
                    rows.push(record(101, json!({"kind":"step_started","step_id":"wake","consumed_inputs":["wake"],"started_at_ms":NOW-44990})));
                }
            }
            rows
        }

        let mut records = fixture();
        records.push(record(
            200,
            json!({"kind":"step_started","step_id":"usage","started_at_ms":NOW-1000}),
        ));
        records.push(record(
            201,
            json!({"kind":"step_completed","step_id":"usage","completed_at_ms":NOW,
            "usage":{"input_tokens":128432,"output_tokens":2400,"cached_input_tokens":96000}}),
        ));
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(ClientStore::open(root.path()).unwrap());
        let device = Device::open(
            Arc::new(StationClient::new("http://127.0.0.1:9", None)),
            None,
            true,
        );
        let conversation = device.conversation("pc-reference");
        let source = conversation.history();
        source.seed(HistoryData {
            records: records.into(),
            loaded: true,
            clock_offset_ms: NOW - crate::store::delivery_now_ms() as i64,
            ..Default::default()
        });
        conversation.seed(state::ConversationData {
            overview: Arc::new(state::SessionOverview {
                loaded: true,
                runtime: Some(state::HistoryRuntime {
                    model: Some("gpt-5.4".into()),
                    thinking: Some("high".into()),
                    context_tokens: Some(128432),
                    context_limit: Some(256000),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        });
        let mut reader = open(&device, &store, "pc-reference");
        let frame = consume(&mut reader, &mut vec![]);
        let snapshot = source.subscribe().snapshot().state;
        assert_eq!(snapshot.entries.len(), 14);
        assert!(snapshot
            .entries
            .iter()
            .any(|e| e.lane == 1 && e.start == Some(NOW - 44990) && e.end.is_none()));
        let details = snapshot
            .entries
            .iter()
            .map(|entry| (entry.id.clone(), detail(entry)))
            .collect::<serde_json::Map<_, _>>();
        if let Some(path) = std::env::var_os("ZORK_HISTORY_PC_FIXTURE_OUTPUT") {
            std::fs::write(
                path,
                serde_json::to_vec_pretty(
                    &json!({"now_ms":NOW,"history":frame["history"],"details":details}),
                )
                .unwrap(),
            )
            .unwrap();
        }
    }
    fn open(device: &Arc<Device>, store: &Arc<ClientStore>, session: &str) -> WireSubscription {
        WireSubscription::from_device(
            Key::History {
                peer: "peer".into(),
                session: session.into(),
            },
            device.clone(),
            store.clone(),
        )
        .unwrap()
    }
    fn consume(wire: &mut WireSubscription, rows: &mut Vec<Value>) -> Arc<Value> {
        let frame = wire.prepare().unwrap().unwrap();
        let state = &frame["history"];
        if let Some(entries) = state["entries"].as_array() {
            *rows = entries.clone();
        }
        if let Some(edits) = state["entry_edits"].as_array() {
            for edit in edits {
                rows.splice(
                    edit["start"].as_u64().unwrap() as usize
                        ..edit["end"].as_u64().unwrap() as usize,
                    edit["insert"].as_array().unwrap().clone(),
                );
            }
        }
        assert!(wire.finish(frame["batch"].as_u64().unwrap(), true));
        frame
    }

    #[test]
    fn bounded_history_recovers_slow_readers_and_keeps_paging_and_detail_per_observer() {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(ClientStore::open(root.path()).unwrap());
        let device = Device::open(
            Arc::new(StationClient::new("http://127.0.0.1:9", None)),
            None,
            true,
        );
        let chat = device.conversation("chat");
        let history = chat.history();
        let mut fast = open(&device, &store, "chat");
        let mut rows = vec![];
        assert_eq!(
            consume(&mut fast, &mut rows)["history"]["entries"],
            json!([])
        );
        history.seed(HistoryData {
            records: (0..100_000).map(record).collect(),
            loaded: true,
            ..Default::default()
        });
        let first = consume(&mut fast, &mut rows);
        assert_eq!(rows.len(), PAGE);
        assert_eq!(first["history"]["window_start"], 99_900);
        assert!(!first.to_string().contains("stages"));
        let mut slow = open(&device, &store, "chat");
        let mut slow_rows = vec![];
        consume(&mut slow, &mut slow_rows);
        history.seed_records(vec![record(100_000)], false);
        let cancelled = slow.prepare().unwrap().unwrap();
        assert!(Arc::ptr_eq(&cancelled, &slow.prepare().unwrap().unwrap()));
        assert!(
            slow.older().is_err(),
            "paging must not replace an unapplied batch"
        );
        let append = consume(&mut fast, &mut rows);
        assert!(append["history"].get("entries").is_none());
        assert_eq!(
            append["history"]["entry_edits"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| e["insert"].as_array().unwrap().len())
                .sum::<usize>(),
            1
        );
        for i in 100_001..100_601 {
            history.seed_records(vec![record(i)], false);
        }
        assert!(slow.finish(cancelled["batch"].as_u64().unwrap(), false));
        let reset = consume(&mut slow, &mut slow_rows);
        consume(&mut fast, &mut rows);
        assert_eq!(rows, slow_rows);
        assert_eq!(rows.len(), PAGE);
        assert_eq!(reset["reset"], true);
        fast.older().unwrap();
        let earlier = consume(&mut fast, &mut rows);
        assert_eq!(earlier["history"]["window_start"], 100_401);
        assert!(slow.prepare().unwrap().is_none());
        let selected = rows[4]["id"].as_str().unwrap().to_owned();
        fast.detail(Some(selected.clone())).unwrap();
        let detail = consume(&mut fast, &mut rows);
        assert_eq!(detail["history"]["detail"]["id"], selected);
        assert!(detail["history"]["detail"]["stages"]
            .to_string()
            .contains("input 100405"));
        history.seed_records(vec![record(100_602)], false);
        let unrelated = consume(&mut fast, &mut rows);
        assert!(
            unrelated["history"].get("detail").is_none(),
            "an unrelated append must not re-encode an open record's full output"
        );
        fast.newer().unwrap();
        consume(&mut fast, &mut rows);
        assert_eq!(rows, slow_rows);
        // A different session never inherits this reader's selected detail or records.
        let mut other = open(&device, &store, "other");
        let empty = consume(&mut other, &mut vec![]);
        assert_eq!(empty["history"]["session"], "other");
        assert_eq!(empty["history"]["detail"], Value::Null);
        assert_eq!(empty["history"]["entries"], json!([]));
    }

    #[test]
    fn history_anchors_survive_prepend_and_orphan_result_moves_and_revocation_clears_details() {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(ClientStore::open(root.path()).unwrap());
        let device = Device::open(
            Arc::new(StationClient::new("http://127.0.0.1:9", None)),
            None,
            true,
        );
        let chat = device.conversation("chat");
        let history = chat.history();
        history.seed(HistoryData {
            records: (100..400).map(record).collect(),
            loaded: true,
            ..Default::default()
        });
        let mut wire = open(&device, &store, "chat");
        let mut rows = vec![];
        consume(&mut wire, &mut rows);
        wire.older().unwrap();
        consume(&mut wire, &mut rows);
        let first_id = rows[0]["id"].as_str().unwrap().to_owned();
        history.seed_records((0..100).map(record).collect(), true);
        consume(&mut wire, &mut rows);
        assert_eq!(rows[0]["id"], first_id);
        history.seed_records(vec![Record {event_id:"result".into(),metadata:Default::default(),
            event:json!({"kind":"tool_result","result":{"invocation_id":"tool-1","tool":"shell.run","outcome":"succeeded","finished_at_ms":410,"data":{"stdout":"private output"}}})}], false);
        wire.window_anchor(None).unwrap();
        consume(&mut wire, &mut rows);
        let tool_id = rows.last().unwrap()["id"].as_str().unwrap().to_owned();
        wire.detail(Some(tool_id.clone())).unwrap();
        consume(&mut wire, &mut rows);
        history.seed_records(vec![Record {event_id:"start".into(),metadata:Default::default(),
            event:json!({"kind":"step_completed","step_id":"s","completed_at_ms":212,"invocations":[{"invocation_id":"tool-1","tool":"shell.run","started_at_ms":210,"arguments":{"command":"pwd"}}]})}], true);
        let joined = consume(&mut wire, &mut rows);
        assert_eq!(joined["history"]["detail"]["id"], tool_id);
        let raw = joined["history"]["detail"]["stages"].to_string();
        assert!(raw.contains("private output") && raw.contains("pwd"));
        history.seed_records(vec![record(500)], false);
        let in_flight = wire.prepare().unwrap().unwrap();
        history.seed(HistoryData {
            revoked: true,
            error: Some("设备访问权限已撤销".into()),
            ..Default::default()
        });
        assert!(!wire.valid(in_flight["batch"].as_u64().unwrap()));
        let clear = consume(&mut wire, &mut rows);
        assert_eq!(clear["history"]["revoked"], true);
        assert_eq!(clear["history"]["detail"], Value::Null);
        assert!(rows.is_empty());
        assert!(!clear.to_string().contains("private output"));
    }

    #[test]
    fn history_requests_retry_and_page_with_core_cursors_without_recomputing_usage() {
        use axum::{
            extract::Query, http::StatusCode, response::IntoResponse, routing::get, Json, Router,
        };
        use std::{collections::HashMap, sync::Mutex, time::Duration};
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let listener = runtime
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        let address = listener.local_addr().unwrap();
        let calls = Arc::new(Mutex::new(Vec::<HashMap<String, String>>::new()));
        let requests = calls.clone();
        let app = Router::new().route(
            "/v1/im/sessions/member-session/history",
            get(move |Query(query): Query<HashMap<String, String>>| {
                let calls = requests.clone();
                async move {
                    let first = {
                        let mut calls = calls.lock().unwrap();
                        calls.push(query.clone());
                        calls.len() == 1
                    };
                    if first {
                        return (
                            StatusCode::SERVICE_UNAVAILABLE,
                            Json(json!({"error":"temporarily unavailable"})),
                        )
                            .into_response();
                    }
                    let before = query.contains_key("before");
                    let start = if before { 100 } else { 200 };
                    Json(
                        json!({"items":(start..start+100).map(record).collect::<Vec<_>>(),
                    "server_time_ms":crate::store::delivery_now_ms(),
                    "older_cursor":if before { None } else { Some("e200") },
                    "latest_cursor":"e299","has_more":false}),
                    )
                    .into_response()
                }
            }),
        );
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(ClientStore::open(root.path()).unwrap());
        let device = Device::open(
            Arc::new(StationClient::new(format!("http://{address}"), None)),
            None,
            true,
        );
        let chat = device.conversation("member-session");
        let mut wire = open(&device, &store, "member-session");
        let mut rows = vec![];
        consume(&mut wire, &mut rows);
        assert!(
            calls.lock().unwrap().is_empty(),
            "constructing a read adapter must not start IO"
        );
        runtime.block_on(async {
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            async fn loaded(wire: &mut WireSubscription, rows: &mut Vec<Value>) -> Arc<Value> {
                let mut signals = wire.signals();
                tokio::time::timeout(Duration::from_secs(5), async {
                    loop {
                        signals.changed().await.unwrap();
                        let frame = consume(wire, rows);
                        if frame["history"]["loading"] == false {
                            return frame;
                        }
                    }
                })
                .await
                .unwrap()
            }
            wire.refresh().unwrap();
            let failed = loaded(&mut wire, &mut rows).await;
            assert!(!failed["history"]["error"].is_null());
            assert_eq!(failed["history"]["loaded"], false);
            wire.refresh().unwrap();
            let ready = loaded(&mut wire, &mut rows).await;
            assert_eq!(ready["history"]["loaded"], true);
            assert_eq!(ready["history"]["error"], Value::Null);
            assert_eq!(rows.len(), 100);
            assert_eq!(rows[0]["preview"], "input 200");
            wire.older().unwrap();
            loaded(&mut wire, &mut rows).await;
            assert_eq!(rows[0]["preview"], "input 100");
            assert_eq!(
                calls.lock().unwrap()[2].get("before").map(String::as_str),
                Some("e200")
            );
            chat.seed(state::ConversationData {
                overview: Arc::new(state::SessionOverview {
                    loaded: true,
                    aggregates: zork_agent_api::SessionAggregates {
                        complete: true,
                        usage: zork_agent_api::SessionUsage {
                            input: 9000,
                            output: 1000,
                            reported_steps: 20,
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                ..Default::default()
            });
            let overview = consume(&mut wire, &mut rows);
            assert_eq!(overview["history"]["overview"]["total"], 10_000);
            assert!(overview["history"].get("entries").is_none());
            assert_eq!(
                calls.lock().unwrap().len(),
                3,
                "overview must not read more execution pages"
            );
            server.abort();
        });
    }
}
