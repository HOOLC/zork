use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::im_entry::ImEntryGateway;
use anyhow::Result;
use futures_util::StreamExt;
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tracing::warn;
use zork_agent::{session::service::LiveSessionEvent, Agent, AgentError};
use zork_agent_api::{ApiErrorCode, EventQuery};

const WAIT_REFRESH_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct AgentStatusProjector {
    agent: Agent,
    stopping: Arc<std::sync::atomic::AtomicBool>,
    entries: ImEntryGateway,
    subscriptions: Arc<Mutex<HashMap<String, Subscription>>>,
}

struct Subscription {
    agent_session_id: String,
    connection_id: String,
    channel_id: String,
    root_message_id: String,
    task: JoinHandle<()>,
    ready: tokio::sync::watch::Receiver<bool>,
}

#[derive(Clone)]
struct ProjectionTarget {
    connection_id: String,
    channel_id: String,
    root_message_id: String,
    session_key: String,
}

impl AgentStatusProjector {
    pub fn new(agent: Agent, entries: ImEntryGateway) -> Self {
        Self {
            agent,
            entries,
            subscriptions: Arc::new(Mutex::new(HashMap::new())),
            stopping: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub async fn shutdown(&self) {
        self.stopping
            .store(true, std::sync::atomic::Ordering::Release);
        let tasks: Vec<_> = self
            .subscriptions
            .lock()
            .await
            .drain()
            .map(|(_, s)| s.task)
            .collect();
        for task in &tasks {
            task.abort();
        }
        for task in tasks {
            let _ = task.await;
        }
    }

    pub async fn ensure(
        &self,
        session_key: &str,
        agent_session_id: &str,
        connection_id: &str,
        channel_id: &str,
        root_message_id: &str,
    ) {
        let mut subscriptions = self.subscriptions.lock().await;
        if self.stopping.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        if subscriptions.get(session_key).is_some_and(|subscription| {
            subscription.agent_session_id == agent_session_id
                && subscription.connection_id == connection_id
                && subscription.channel_id == channel_id
                && subscription.root_message_id == root_message_id
                && !subscription.task.is_finished()
        }) {
            let mut ready = subscriptions[session_key].ready.clone();
            drop(subscriptions);
            let waiting = !*ready.borrow_and_update();
            if waiting {
                let _ = ready.changed().await;
            }
            return;
        }
        if let Some(previous) = subscriptions.remove(session_key) {
            previous.task.abort();
        }
        let agent = self.agent.clone();
        let entries = self.entries.clone();
        let session_key_owned = session_key.to_owned();
        let agent_session_id_owned = agent_session_id.to_owned();
        let connection_id_owned = connection_id.to_owned();
        let channel_id_owned = channel_id.to_owned();
        let root_message_id_owned = root_message_id.to_owned();
        let task_session_key = session_key_owned.clone();
        let task_agent_session_id = agent_session_id_owned.clone();
        let task_connection_id = connection_id_owned.clone();
        let task_channel_id = channel_id_owned.clone();
        let task_root_message_id = root_message_id_owned.clone();
        let (ready_tx, mut ready_rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(async move {
            run_subscription(
                agent,
                entries,
                ProjectionTarget {
                    connection_id: task_connection_id,
                    channel_id: task_channel_id,
                    root_message_id: task_root_message_id,
                    session_key: task_session_key,
                },
                task_agent_session_id,
                ready_tx,
            )
            .await;
        });
        subscriptions.insert(
            session_key_owned,
            Subscription {
                agent_session_id: agent_session_id_owned,
                connection_id: connection_id_owned,
                channel_id: channel_id_owned,
                root_message_id: root_message_id_owned,
                task,
                ready: ready_rx.clone(),
            },
        );
        drop(subscriptions);
        let waiting = !*ready_rx.borrow_and_update();
        if waiting {
            let _ = ready_rx.changed().await;
        }
    }

    pub async fn remove(&self, session_key: &str) {
        let removed = self.subscriptions.lock().await.remove(session_key);
        if let Some(subscription) = removed {
            subscription.task.abort();
            self.entries
                .clear_status(
                    &subscription.connection_id,
                    session_key,
                    &subscription.channel_id,
                    &subscription.root_message_id,
                )
                .await;
        }
    }
}

async fn run_subscription(
    agent: Agent,
    entries: ImEntryGateway,
    target: ProjectionTarget,
    agent_session_id: String,
    ready: tokio::sync::watch::Sender<bool>,
) {
    let mut cursor = None;
    let mut projection = ProjectionState::default();
    let mut retry = zork_notify::retry::Retry::default();
    loop {
        let previous = cursor.clone();
        match agent
            .events(
                agent_session_id.clone(),
                cursor.clone(),
                EventQuery { transient: true },
            )
            .await
        {
            Ok(events) => {
                if let Err(error) = consume_stream(
                    events,
                    &entries,
                    &target,
                    &agent_session_id,
                    &mut cursor,
                    &mut projection,
                    &agent,
                    &ready,
                )
                .await
                {
                    ready.send_replace(true);
                    warn!(session = %target.session_key, %agent_session_id, %error, "Agent status observation stopped");
                }
            }
            Err(error) if error.code == ApiErrorCode::SessionNotFound => {
                ready.send_replace(true);
                return;
            }
            Err(error) => {
                ready.send_replace(true);
                warn!(session = %target.session_key, %agent_session_id, %error, "Agent status observation failed")
            }
        }
        if cursor != previous {
            retry.reset();
        }
        retry.wait().await;
    }
}

async fn consume_stream(
    events: impl futures_util::Stream<Item = std::result::Result<LiveSessionEvent, AgentError>>,
    entries: &ImEntryGateway,
    target: &ProjectionTarget,
    agent_session_id: &str,
    cursor: &mut Option<String>,
    projection: &mut ProjectionState,
    agent: &Agent,
    ready: &tokio::sync::watch::Sender<bool>,
) -> Result<()> {
    let ProjectionTarget {
        connection_id,
        channel_id,
        root_message_id,
        session_key,
    } = target;
    tokio::pin!(events);
    let mut wait_refresh = tokio::time::interval(WAIT_REFRESH_INTERVAL);
    wait_refresh.set_missed_tick_behavior(MissedTickBehavior::Delay);
    wait_refresh.tick().await;
    loop {
        let now = now_ms();
        let activity_wake = projection.activity.next_wake(now);
        // A previous publish or stream setup may have crossed the final
        // presentation deadline. Reconcile it before parking without a timer.
        if activity_wake.is_none() && projection.activity_status.is_some() {
            let status_event = projection.activity_event(now)?;
            if projection.last_activity_event.as_ref() != Some(&status_event) {
                projection.last_activity_event = Some(status_event.clone());
                entries
                    .set_status(
                        connection_id,
                        session_key,
                        channel_id,
                        root_message_id,
                        status_event,
                        &projection.current_tool_status(),
                    )
                    .await;
                continue;
            }
        }
        let activity_delay =
            Duration::from_millis(activity_wake.unwrap_or(now).saturating_sub(now).max(0) as u64);
        tokio::select! {
            event = events.next() => {
                let Some(event) = event else { anyhow::bail!("Agent status event stream ended"); };
                let event = event?;
                match &event {
                    LiveSessionEvent::Snapshot(snapshot) => {
                        *cursor = snapshot.cursor.clone();
                        projection.seed(snapshot, entries, now_ms())?;
                        let status_event = projection.activity_event(now_ms())?;
                        entries.set_status(connection_id, session_key, channel_id, root_message_id, status_event, &projection.current_tool_status()).await;
                        entries.publish_execution_snapshot(session_key, snapshot)?;
                        ready.send_replace(true);
                        continue;
                    }
                    LiveSessionEvent::Overview(overview) => {
                        let snapshot = agent.snapshot_from((**overview).clone()).await?;
                        entries.publish_execution_snapshot(session_key, &snapshot)?;
                        continue;
                    }
                    _ => {}
                }
                if let LiveSessionEvent::OutputDelta { step_id, bytes, .. } = &event {
                    if projection.activity.output(step_id, *bytes, now_ms()) {
                        let status_event = projection.activity_event(now_ms())?;
                        if projection.last_activity_event.as_ref() != Some(&status_event) {
                            projection.last_activity_event = Some(status_event.clone());
                            entries.set_status(connection_id, session_key, channel_id, root_message_id, status_event, &projection.current_tool_status()).await;
                        }
                    }
                }
                if let LiveSessionEvent::Durable(envelope) = event {
                    let value = serde_json::to_value(&envelope.event)?;
                    entries.project_task_run(session_key, agent_session_id, &envelope.event_id, &value)?;
                    *cursor = Some(envelope.event_id.clone());
                    let at = now_ms();
                    if value["kind"] == "step_started" {
                        if let Some(id) = value["step_id"].as_str() {
                            projection.activity.begin_request(id.to_owned(), value["started_at_ms"].as_i64().unwrap_or(at));
                        }
                    } else if matches!(value["kind"].as_str(), Some("step_completed" | "step_failed" | "step_interrupted")) {
                        if let Some(id) = value["step_id"].as_str() { projection.activity.end_request(id); }
                    }
                    let mut event = serde_json::from_value::<AgentEvent>(value)?;
                    event.resolve_activity_targets(entries);
                    let Some(event) = event.status_event() else { continue; };
                    let starts_wait = matches!(
                        &event,
                        AgentStatusEvent::Waiting { .. }
                            | AgentStatusEvent::ToolsWaiting { .. }
                    );
                    let raw: zork_client_core::api::AgentStatus = serde_json::from_value(serde_json::to_value(&event)?)?;
                    projection.activity.observe(&raw);
                    let status = projection.apply(event.clone(), at);
                    let current = serde_json::to_value(projection.current_event(event))?;
                    projection.activity_status = Some(serde_json::from_value(current)?);
                    let status_event = projection.activity_event(at)?;
                    projection.last_activity_event = Some(status_event.clone());
                    if starts_wait {
                        wait_refresh.reset_after(WAIT_REFRESH_INTERVAL);
                    }
                    entries
                        .set_status(
                            connection_id,
                            session_key,
                            channel_id,
                            root_message_id,
                            status_event,
                            &status,
                        )
                        .await;
                }
            }
            _ = tokio::time::sleep(activity_delay), if activity_wake.is_some() => {
                let status_event = projection.activity_event(now_ms())?;
                if projection.last_activity_event.as_ref() != Some(&status_event) {
                    projection.last_activity_event = Some(status_event.clone());
                    entries.set_status(connection_id, session_key, channel_id, root_message_id, status_event, &projection.current_tool_status()).await;
                }
            }
            _ = wait_refresh.tick(), if projection.waiting.is_some() => {
                let status = projection.refresh_wait(now_ms());
                entries
                    .refresh_status(
                        connection_id,
                        session_key,
                        channel_id,
                        root_message_id,
                        &status,
                    )
                    .await;
            }
        }
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum AgentEvent {
    SessionCreated,
    InputAppended,
    SelectionChanged,
    ContextConfigured,
    TurnStarted,
    TurnCancelRequested,
    TurnFinished {
        outcome: AgentTurnOutcome,
    },
    StepStarted,
    StepCompleted {
        purpose: Option<String>,
        invocations: Vec<AgentInvocation>,
        auto_wait_deadline_ms: Option<i64>,
    },
    StepFailed {
        error: AgentFailure,
    },
    StepInterrupted,
    AutoWaitEnded,
    ToolCancelRequested,
    ToolResult {
        result: AgentToolResult,
    },
    #[serde(alias = "handoff_failed")]
    ContextFailed,
    #[serde(alias = "handoff_applied")]
    ContextApplied,
    DeadlineReached,
    RuntimeFault {
        failure: AgentFailure,
    },
    Snapshot,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AgentTurnOutcome {
    Finished,
    Failed,
    Cancelled,
}

#[derive(Debug, Deserialize)]
struct AgentInvocation {
    invocation_id: String,
    tool: String,
    #[serde(default)]
    activity: Option<zork_agent::session::tools::ToolActivity>,
    #[serde(default)]
    rejection: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AgentToolResult {
    invocation_id: String,
    tool: String,
    outcome: String,
    data: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct AgentFailure {
    message: String,
}

impl AgentEvent {
    fn resolve_activity_targets(&mut self, entries: &ImEntryGateway) {
        if let Self::StepCompleted { invocations, .. } = self {
            for invocation in invocations {
                if let Some(activity) = &mut invocation.activity {
                    if let Some(target) = activity.target.take() {
                        activity.detail = entries.activity_target(&target).unwrap_or_default();
                    }
                }
            }
        }
    }

    fn status_event(self) -> Option<AgentStatusEvent> {
        match self {
            Self::SessionCreated => Some(AgentStatusEvent::Clear),
            Self::TurnStarted | Self::StepStarted | Self::ContextApplied => {
                Some(AgentStatusEvent::Thinking)
            }
            Self::StepCompleted {
                purpose: Some(purpose),
                ..
            } if purpose != "conversation" => Some(AgentStatusEvent::Thinking),
            Self::StepCompleted {
                invocations,
                auto_wait_deadline_ms,
                ..
            } if !invocations.is_empty() => {
                let calls: Vec<_> = invocations
                    .into_iter()
                    .filter(|invocation| invocation.rejection.is_none())
                    .map(|invocation| {
                        let activity = invocation.activity.unwrap_or_default();
                        AgentToolCall {
                            detail: activity.detail,
                            labels: activity.labels,
                            action: activity.action,
                            tool_call_id: invocation.invocation_id,
                            tool_name: invocation.tool,
                        }
                    })
                    .collect();
                if calls.is_empty() {
                    return Some(AgentStatusEvent::Thinking);
                }
                match auto_wait_deadline_ms {
                    Some(deadline_ms) => {
                        Some(AgentStatusEvent::ToolsWaiting { calls, deadline_ms })
                    }
                    None => Some(AgentStatusEvent::ToolsStarted {
                        calls,
                        thinking: false,
                    }),
                }
            }
            Self::StepCompleted { .. } => Some(AgentStatusEvent::Thinking),
            Self::StepFailed { error } | Self::RuntimeFault { failure: error } => {
                Some(AgentStatusEvent::Failed {
                    reason: error.message,
                })
            }
            Self::StepInterrupted | Self::TurnCancelRequested => {
                Some(AgentStatusEvent::Interrupted)
            }
            Self::ToolResult { result }
                if result.tool == "wait" && result.outcome == "succeeded" =>
            {
                result
                    .data
                    .get("until_ms")
                    .and_then(serde_json::Value::as_i64)
                    .map(|deadline_ms| AgentStatusEvent::Waiting {
                        completed_tool_call_id: Some(result.invocation_id),
                        reason: result
                            .data
                            .get("reason")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("the requested time")
                            .to_owned(),
                        deadline_ms,
                    })
            }
            Self::ToolResult { result } => Some(AgentStatusEvent::ToolFinished {
                tool_call_id: result.invocation_id,
            }),
            Self::TurnFinished { outcome } => Some(match outcome {
                AgentTurnOutcome::Finished => AgentStatusEvent::Finished,
                AgentTurnOutcome::Failed => AgentStatusEvent::Failed {
                    reason: "turn failed".to_owned(),
                },
                AgentTurnOutcome::Cancelled => AgentStatusEvent::Interrupted,
            }),
            Self::DeadlineReached => Some(AgentStatusEvent::Thinking),
            Self::ContextFailed
            | Self::AutoWaitEnded
            | Self::InputAppended
            | Self::SelectionChanged
            | Self::ContextConfigured
            | Self::ToolCancelRequested
            | Self::Snapshot => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, serde::Serialize, Eq, PartialEq)]
struct AgentToolCall {
    tool_call_id: String,
    tool_name: String,
    #[serde(default)]
    detail: String,
    #[serde(default)]
    labels: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    action: String,
}

fn tool_summary(call: &AgentToolCall) -> String {
    if !call.action.is_empty() {
        return call.action.clone();
    }
    let action = call
        .labels
        .get("en")
        .map(String::as_str)
        .filter(|label| !label.is_empty())
        .unwrap_or("Working");
    if call.detail.is_empty() {
        action.to_owned()
    } else {
        format!("{action} {}", call.detail)
    }
}

#[derive(Clone, Debug, Deserialize, serde::Serialize, Eq, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
enum AgentStatusEvent {
    Clear,
    Thinking,
    ToolsStarted {
        calls: Vec<AgentToolCall>,
        #[serde(default)]
        thinking: bool,
    },
    ToolFinished {
        tool_call_id: String,
    },
    ToolsWaiting {
        calls: Vec<AgentToolCall>,
        deadline_ms: i64,
    },
    Waiting {
        reason: String,
        deadline_ms: i64,
        // A successful wait result completes this call while other tools remain live.
        #[serde(skip)]
        completed_tool_call_id: Option<String>,
    },
    Failed {
        reason: String,
    },
    Finished,
    Interrupted,
}

#[derive(Clone, Debug)]
struct WaitingStatus {
    tools: bool,
    reason: String,
    deadline_ms: i64,
}

#[derive(Default)]
struct ProjectionState {
    activity: zork_client_core::activity::Activity,
    activity_status: Option<zork_client_core::api::AgentStatus>,
    last_activity_event: Option<serde_json::Value>,
    thinking: bool,
    active_tools: Vec<AgentToolCall>,
    waiting: Option<WaitingStatus>,
    failure: Option<String>,
}

impl ProjectionState {
    fn seed(
        &mut self,
        snapshot: &zork_agent_api::SessionSnapshot,
        entries: &ImEntryGateway,
        now: i64,
    ) -> Result<()> {
        *self = Self::default();
        let execution = &snapshot.execution;
        self.thinking = execution.active_step.is_some();
        self.active_tools = execution
            .tools
            .iter()
            .map(|tool| {
                let detail = tool
                    .target
                    .as_ref()
                    .and_then(|target| {
                        entries.activity_target(&match target {
                            zork_agent_api::ExecutionTarget::Agent(id) => {
                                zork_agent::session::tools::ActivityTarget::Agent(id.clone())
                            }
                            zork_agent_api::ExecutionTarget::Task(id) => {
                                zork_agent::session::tools::ActivityTarget::Task(id.clone())
                            }
                        })
                    })
                    .unwrap_or_else(|| tool.detail.clone());
                AgentToolCall {
                    tool_call_id: tool.invocation_id.clone(),
                    tool_name: tool.tool.clone(),
                    detail,
                    labels: tool.labels.clone(),
                    action: tool.action.clone(),
                }
            })
            .collect();
        self.waiting = execution.waiting.as_ref().map(|wait| WaitingStatus {
            tools: wait.tools,
            reason: wait.reason.clone(),
            deadline_ms: wait.deadline_ms,
        });
        self.failure = execution.failure.clone();
        let event = match execution.status {
            zork_agent_api::SessionStatus::Wait => AgentStatusEvent::Clear,
            zork_agent_api::SessionStatus::Finished => AgentStatusEvent::Finished,
            zork_agent_api::SessionStatus::Cancelled => AgentStatusEvent::Interrupted,
            zork_agent_api::SessionStatus::Failed => AgentStatusEvent::Failed {
                reason: self.failure.clone().unwrap_or_else(|| "turn failed".into()),
            },
            _ => self.current_event(AgentStatusEvent::Thinking),
        };
        if let Some(step) = &execution.active_step {
            self.activity
                .begin_request(step.step_id.clone(), step.started_at_ms);
        }
        let raw: zork_client_core::api::AgentStatus =
            serde_json::from_value(serde_json::to_value(event)?)?;
        self.activity.observe(&raw);
        self.activity_status = Some(raw);
        self.last_activity_event = Some(self.activity_event(now)?);
        Ok(())
    }
    fn activity_event(&self, now: i64) -> Result<serde_json::Value> {
        let status = self.activity.present(
            self.activity_status
                .clone()
                .unwrap_or(zork_client_core::api::AgentStatus::Clear),
            now,
        );
        Ok(serde_json::to_value(&status)?)
    }

    fn apply(&mut self, event: AgentStatusEvent, now_ms: i64) -> String {
        match event {
            AgentStatusEvent::Clear
            | AgentStatusEvent::Finished
            | AgentStatusEvent::Interrupted => {
                self.reset();
                String::new()
            }
            AgentStatusEvent::Thinking => {
                self.waiting = None;
                self.failure = None;
                self.thinking = true;
                self.current_tool_status()
            }
            AgentStatusEvent::ToolsStarted { calls, thinking } => {
                self.waiting = None;
                self.thinking = thinking;
                self.merge_tools(calls);
                self.current_tool_status()
            }
            AgentStatusEvent::ToolFinished { tool_call_id } => {
                self.active_tools
                    .retain(|call| call.tool_call_id != tool_call_id);
                if self.waiting.as_ref().is_some_and(|wait| wait.tools) {
                    if self.active_tools.is_empty() {
                        self.waiting = None;
                    } else {
                        let reason = self.tool_wait_reason();
                        self.waiting.as_mut().unwrap().reason = reason;
                    }
                }
                if self.waiting.is_some() {
                    return self.refresh_wait(now_ms);
                }
                if self.active_tools.is_empty() {
                    "Thinking...".into()
                } else {
                    self.current_tool_status()
                }
            }
            AgentStatusEvent::ToolsWaiting { calls, deadline_ms } => {
                self.thinking = false;
                self.merge_tools(calls);
                let reason = self.tool_wait_reason();
                self.waiting = Some(WaitingStatus {
                    tools: true,
                    reason,
                    deadline_ms,
                });
                self.refresh_wait(now_ms)
            }
            AgentStatusEvent::Waiting {
                reason,
                deadline_ms,
                completed_tool_call_id,
            } => {
                let (deadline_ms, reason) = match self.waiting.as_ref() {
                    Some(wait)
                        if completed_tool_call_id.is_some() && wait.deadline_ms < deadline_ms =>
                    {
                        (wait.deadline_ms, wait.reason.clone())
                    }
                    _ => (deadline_ms, reason),
                };
                if let Some(id) = completed_tool_call_id {
                    self.active_tools.retain(|call| call.tool_call_id != id);
                }
                self.thinking = false;
                self.waiting = Some(WaitingStatus {
                    tools: false,
                    reason,
                    deadline_ms,
                });
                self.refresh_wait(now_ms)
            }
            AgentStatusEvent::Failed { reason } => {
                let reason = if reason == "turn failed" {
                    self.failure.clone().unwrap_or(reason)
                } else {
                    reason
                };
                self.reset();
                self.failure = Some(reason.clone());
                format!("Failed: {reason}")
            }
        }
    }

    fn refresh_wait(&self, now_ms: i64) -> String {
        let wait = self.waiting.as_ref().expect("wait refresh without wait");
        let remaining_ms = wait.deadline_ms.saturating_sub(now_ms).max(0);
        let remaining_seconds = remaining_ms.saturating_add(999) / 1_000;
        format!(
            "Waiting: {} · {:02}:{:02}",
            wait.reason,
            remaining_seconds / 60,
            remaining_seconds % 60
        )
    }

    fn current_tool_status(&self) -> String {
        if self.thinking {
            let label = "Thinking...";
            return if self.active_tools.is_empty() {
                label.into()
            } else {
                format!("{label} · {} operations running", self.active_tools.len())
            };
        }
        self.active_tools
            .last()
            .map(tool_summary)
            .unwrap_or_else(|| "Working...".to_owned())
    }

    fn tool_wait_reason(&self) -> String {
        match self.active_tools.as_slice() {
            [call] => format!("{} to finish", tool_summary(call)),
            [] => "background tools to finish".into(),
            calls => format!("{} background tools to finish", calls.len()),
        }
    }

    // Desktop consumers receive complete current state, including remaining
    // concurrent calls. A tool-result event alone is not a presentable status.
    fn current_event(&self, event: AgentStatusEvent) -> AgentStatusEvent {
        if matches!(
            event,
            AgentStatusEvent::ToolFinished { .. }
                | AgentStatusEvent::Thinking
                | AgentStatusEvent::ToolsStarted { .. }
                | AgentStatusEvent::ToolsWaiting { .. }
                | AgentStatusEvent::Waiting { .. }
        ) {
            if let Some(wait) = &self.waiting {
                if wait.tools {
                    return AgentStatusEvent::ToolsWaiting {
                        calls: self.active_tools.clone(),
                        deadline_ms: wait.deadline_ms,
                    };
                }
                return AgentStatusEvent::Waiting {
                    reason: wait.reason.clone(),
                    deadline_ms: wait.deadline_ms,
                    completed_tool_call_id: None,
                };
            }
            if self.active_tools.is_empty() {
                AgentStatusEvent::Thinking
            } else {
                AgentStatusEvent::ToolsStarted {
                    calls: self.active_tools.clone(),
                    thinking: self.thinking,
                }
            }
        } else if let AgentStatusEvent::Failed { reason } = event {
            AgentStatusEvent::Failed {
                reason: self.failure.clone().unwrap_or(reason),
            }
        } else {
            event
        }
    }

    fn reset(&mut self) {
        self.thinking = false;
        self.failure = None;
        self.active_tools.clear();
        self.waiting = None;
    }

    fn merge_tools(&mut self, calls: Vec<AgentToolCall>) {
        for call in calls {
            if let Some(existing) = self
                .active_tools
                .iter_mut()
                .find(|old| old.tool_call_id == call.tool_call_id)
            {
                *existing = call;
            } else {
                self.active_tools.push(call);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn concurrent_subscribers_wait_for_the_shared_initial_snapshot() {
        use crate::{
            connections::ConnectionManager,
            db::{EnsureSession, GatewayDb},
            im_entry::{LOCAL_GUI_ENTRY_ID, LOCAL_GUI_PLATFORM},
        };
        let dir = tempfile::tempdir().unwrap();
        zork_config::ensure_layout(dir.path()).unwrap();
        let db = Arc::new(
            GatewayDb::open(&dir.path().join("state"), &dir.path().join("workspaces")).unwrap(),
        );
        let connections = Arc::new(
            ConnectionManager::load(dir.path().to_owned(), reqwest::Client::new())
                .await
                .unwrap(),
        );
        let entries = ImEntryGateway::new(db.clone(), connections);
        let session = db
            .create_session_at_workspace(
                EnsureSession {
                    connection_id: LOCAL_GUI_ENTRY_ID,
                    platform: LOCAL_GUI_PLATFORM,
                    channel_id: "snapshot",
                    root_thread_ts: "snapshot",
                    channel_type: Some("desktop"),
                    initiator_user_id: None,
                    initiator_message_ts: None,
                },
                &dir.path().join("project"),
            )
            .unwrap();
        let mut runtime = zork_agent::AgentRuntime::start(zork_agent::AgentOptions {
            data_root: dir.path().to_owned(),
            fake_agent: true,
            ..Default::default()
        })
        .unwrap();
        let id = runtime
            .agent()
            .service
            .create_session(
                zork_agent::session::events::Selection {
                    profile_id: "fixture".into(),
                    model: "model".into(),
                    thinking: "off".into(),
                },
                None,
                dir.path().join("project").to_string_lossy().into_owned(),
                None,
            )
            .await
            .unwrap();
        db.set_agent_session(
            &session.key,
            &id,
            &session.workspace_path,
            "fixture",
            "model",
            "off",
        )
        .unwrap();
        let projector = AgentStatusProjector::new(runtime.agent().clone(), entries.clone());
        tokio::time::timeout(Duration::from_secs(3), async {
            tokio::join!(
                projector.ensure(
                    &session.key,
                    &id,
                    LOCAL_GUI_ENTRY_ID,
                    "snapshot",
                    "snapshot"
                ),
                projector.ensure(
                    &session.key,
                    &id,
                    LOCAL_GUI_ENTRY_ID,
                    "snapshot",
                    "snapshot"
                )
            );
        })
        .await
        .unwrap();
        let bound = db.get_session(&session.key).unwrap().unwrap();
        let (_events, snapshot) = entries.subscribe_local_with_snapshot(&bound);
        assert_eq!(snapshot["session_id"], id);
        assert_eq!(snapshot["execution"]["session_id"], id);
        assert_eq!(snapshot["execution"]["aggregates"]["complete"], true);
        assert_eq!(snapshot["execution"]["aggregates"]["run_count"], 0);
        assert_eq!(snapshot["execution"]["runtime"]["model"], "model");
        assert_eq!(projector.subscriptions.lock().await.len(), 1);
        projector.shutdown().await;
        runtime.shutdown().await;
    }

    #[tokio::test]
    async fn action_deadline_publishes_core_requesting_without_another_agent_event() {
        assert_requesting_after_action_grace(2_900).await;
    }

    #[tokio::test]
    async fn expired_action_deadline_is_published_before_the_stream_parks() {
        assert_requesting_after_action_grace(3_100).await;
    }

    async fn assert_requesting_after_action_grace(request_age_ms: i64) {
        use crate::{
            connections::ConnectionManager,
            db::{EnsureSession, GatewayDb},
            im_entry::{LOCAL_GUI_ENTRY_ID, LOCAL_GUI_PLATFORM},
        };
        let dir = tempfile::tempdir().unwrap();
        zork_config::ensure_layout(dir.path()).unwrap();
        let db = Arc::new(
            GatewayDb::open(&dir.path().join("state"), &dir.path().join("workspaces")).unwrap(),
        );
        let connections = Arc::new(
            ConnectionManager::load(dir.path().to_owned(), reqwest::Client::new())
                .await
                .unwrap(),
        );
        let entries = ImEntryGateway::new(db.clone(), connections);
        let session = db
            .create_session_at_workspace(
                EnsureSession {
                    connection_id: LOCAL_GUI_ENTRY_ID,
                    platform: LOCAL_GUI_PLATFORM,
                    channel_id: "activity",
                    root_thread_ts: "activity",
                    channel_type: Some("desktop"),
                    initiator_user_id: None,
                    initiator_message_ts: None,
                },
                &dir.path().join("project"),
            )
            .unwrap();
        let target = ProjectionTarget {
            connection_id: LOCAL_GUI_ENTRY_ID.into(),
            channel_id: "activity".into(),
            root_message_id: "activity".into(),
            session_key: session.key.clone(),
        };
        let mut receiver = entries.subscribe_local(&session.key);
        let mut projection = ProjectionState::default();
        projection.activity.observe(&serde_json::from_value(serde_json::json!({
            "state":"tools_started", "calls":[{"tool_call_id":"a", "tool_name":"file.read", "goal":"检查图表", "action":"读取图例"}]
        })).unwrap());
        projection
            .activity
            .begin_request("s".into(), now_ms() - request_age_ms);
        projection.activity_status = Some(zork_client_core::api::AgentStatus::Thinking);
        let mut cursor = None;
        let mut runtime = zork_agent::AgentRuntime::start(zork_agent::AgentOptions {
            data_root: dir.path().to_owned(),
            fake_agent: true,
            ..Default::default()
        })
        .unwrap();
        let (ready, _) = tokio::sync::watch::channel(false);
        tokio::select! {
            result = consume_stream(futures_util::stream::pending(), &entries, &target, "agent", &mut cursor, &mut projection, runtime.agent(), &ready) => panic!("unexpected end: {result:?}"),
            result = tokio::time::timeout(Duration::from_secs(2), receiver.recv()) => {
                let event = result.unwrap().unwrap();
                assert_eq!(event.data["state"], "live");
                assert_eq!(event.data["presentation"]["label_zh"], "请求中");
            }
        }
        runtime.shutdown().await;
    }

    fn call(id: &str) -> AgentToolCall {
        AgentToolCall {
            tool_call_id: id.into(),
            tool_name: "custom.operation".into(),
            detail: id.into(),
            labels: [("en".into(), "Inspecting".into())].into(),
            action: String::new(),
        }
    }

    #[test]
    fn legacy_goal_is_ignored_and_tool_completion_restores_thinking() {
        let mut projection = ProjectionState::default();
        let mut operation = call("fast");
        operation.action = "读取图表局部截图".into();
        projection.apply(
            AgentStatusEvent::ToolsStarted {
                calls: vec![operation],
                thinking: false,
            },
            0,
        );
        let done = AgentStatusEvent::ToolFinished {
            tool_call_id: "fast".into(),
        };
        assert_eq!(projection.apply(done.clone(), 8), "Thinking...");
        let thinking = projection.current_event(done);
        assert_eq!(thinking, AgentStatusEvent::Thinking);
        projection.apply(AgentStatusEvent::Thinking, 10);
        assert_eq!(
            projection.current_event(AgentStatusEvent::Thinking),
            thinking
        );
        let wire = serde_json::to_value(thinking).unwrap();
        assert_eq!(wire["state"], "thinking");
        assert!(wire.get("goal").is_none());
        let client: zork_client_core::api::AgentStatus = serde_json::from_value(wire).unwrap();
        assert_eq!(client, zork_client_core::api::AgentStatus::Thinking);
        projection.apply(AgentStatusEvent::Finished, 20);
        assert!(projection.active_tools.is_empty());
    }

    #[test]
    fn model_action_is_forwarded_verbatim_without_appending_command_detail() {
        let event: AgentEvent = serde_json::from_value(serde_json::json!({
            "kind":"step_completed", "invocations":[{"invocation_id":"a","tool":"shell.run",
                "activity":{"goal":"检查仓库状态","action":"查看提交和未提交改动","labels":{"zh-CN":"执行","en":"Running"},"detail":"ssh -o BatchMode=yes host git status"}}]
        })).unwrap();
        let state = event.status_event().unwrap();
        let AgentStatusEvent::ToolsStarted { calls, .. } = &state else {
            panic!("expected tools");
        };
        assert_eq!(tool_summary(&calls[0]), "查看提交和未提交改动");
        let client: zork_client_core::api::AgentStatus =
            serde_json::from_value(serde_json::to_value(state).unwrap()).unwrap();
        let zork_client_core::api::AgentStatus::ToolsStarted { calls, .. } = client else {
            panic!("expected tools");
        };
        assert_eq!(calls[0].activity_label("zh-CN"), "查看提交和未提交改动");
        assert_eq!(calls[0].activity_label("en"), "查看提交和未提交改动");
    }

    #[test]
    fn successful_wait_is_not_retained_as_an_unfinished_tool() {
        let mut state = ProjectionState::default();
        state.apply(
            AgentStatusEvent::ToolsStarted {
                calls: vec![call("background"), call("wait")],
                thinking: false,
            },
            0,
        );
        let result: AgentEvent = serde_json::from_value(serde_json::json!({
            "kind":"tool_result", "result":{"invocation_id":"wait","tool":"wait","outcome":"succeeded","data":{"until_ms":100,"reason":"a reply"}}
        })).unwrap();
        state.apply(result.status_event().unwrap(), 0);
        state.apply(AgentStatusEvent::Thinking, 1);
        assert_eq!(
            state.current_event(AgentStatusEvent::Thinking),
            AgentStatusEvent::ToolsStarted {
                calls: vec![call("background")],
                thinking: true
            }
        );
    }

    #[test]
    fn background_calls_survive_thinking_and_new_batches() {
        let mut state = ProjectionState::default();
        state.apply(
            AgentStatusEvent::ToolsStarted {
                calls: vec![call("a")],
                thinking: false,
            },
            0,
        );
        state.apply(AgentStatusEvent::Thinking, 0);
        assert_eq!(
            state.current_event(AgentStatusEvent::Thinking),
            AgentStatusEvent::ToolsStarted {
                calls: vec![call("a")],
                thinking: true
            }
        );
        let next = AgentStatusEvent::ToolsWaiting {
            calls: vec![call("b")],
            deadline_ms: 100,
        };
        state.apply(next.clone(), 0);
        assert_eq!(
            state.current_event(next),
            AgentStatusEvent::ToolsWaiting {
                calls: vec![call("a"), call("b")],
                deadline_ms: 100
            }
        );
        let done = AgentStatusEvent::ToolFinished {
            tool_call_id: "a".into(),
        };
        state.apply(done.clone(), 0);
        assert_eq!(
            state.current_event(done),
            AgentStatusEvent::ToolsWaiting {
                calls: vec![call("b")],
                deadline_ms: 100
            }
        );
        state.apply(
            AgentStatusEvent::Waiting {
                completed_tool_call_id: None,
                reason: "a reply".into(),
                deadline_ms: 200,
            },
            0,
        );
        let done = AgentStatusEvent::ToolFinished {
            tool_call_id: "b".into(),
        };
        state.apply(done.clone(), 0);
        assert_eq!(
            state.current_event(done),
            AgentStatusEvent::Waiting {
                completed_tool_call_id: None,
                reason: "a reply".into(),
                deadline_ms: 200
            }
        );
        state.apply(AgentStatusEvent::Thinking, 0);
        assert_eq!(
            state.current_event(AgentStatusEvent::Thinking),
            AgentStatusEvent::Thinking
        );
    }

    #[test]
    fn replay_uses_captured_mapping_and_old_events_get_a_neutral_fallback() {
        let event: AgentEvent = serde_json::from_value(serde_json::json!({
            "kind":"step_completed", "invocations":[
                {"invocation_id":"custom", "tool":"custom.tool", "activity":{"labels":{"en":"Inspecting","zh-CN":"检查"},"detail":"report"}},
                {"invocation_id":"old", "tool":"file.write", "arguments":{"path":"private path","content":"private body"}}
            ]
        })).unwrap();
        let AgentStatusEvent::ToolsStarted { calls, .. } = event.status_event().unwrap() else {
            panic!("expected calls");
        };
        assert_eq!(tool_summary(&calls[0]), "Inspecting report");
        assert_eq!(tool_summary(&calls[1]), "Working");
        assert!(!serde_json::to_string(&calls).unwrap().contains("private"));
        let client: zork_client_core::api::AgentStatus = serde_json::from_value(
            serde_json::to_value(AgentStatusEvent::ToolsStarted {
                calls,
                thinking: true,
            })
            .unwrap(),
        )
        .unwrap();
        let zork_client_core::api::AgentStatus::ToolsStarted { calls, thinking } = client else {
            panic!("expected client calls");
        };
        assert!(thinking);
        assert_eq!(calls[0].activity_label("zh-CN"), "检查 report");
        assert_eq!(calls[1].activity_label("zh-CN"), "执行操作");
    }

    #[test]
    fn current_tools_keep_targets_and_complete_by_invocation_id() {
        let event: AgentEvent = serde_json::from_value(serde_json::json!({
            "kind":"step_completed", "invocations":[
                {"invocation_id":"a", "tool":"file.read", "activity":{"labels":{"en":"Reading","zh-CN":"读取"},"detail":"src/甲.rs"}},
                {"invocation_id":"b", "tool":"file.read", "activity":{"labels":{"en":"Reading","zh-CN":"读取"},"detail":"src/乙.rs"}},
                {"invocation_id":"c", "tool":"shell.run", "activity":{"labels":{"en":"Running","zh-CN":"执行"},"detail":"cargo test --locked"}}
            ]
        })).unwrap();
        let mut p = ProjectionState::default();
        assert_eq!(
            p.apply(event.status_event().unwrap(), 0),
            "Running cargo test --locked"
        );
        let done = AgentStatusEvent::ToolFinished {
            tool_call_id: "b".into(),
        };
        p.apply(done.clone(), 0);
        let state = serde_json::to_value(p.current_event(done)).unwrap();
        assert_eq!(state["state"], "tools_started");
        assert_eq!(state["calls"].as_array().unwrap().len(), 2);
        assert_eq!(state["calls"][0]["detail"], "src/甲.rs");
        assert_eq!(state["calls"][1]["detail"], "cargo test --locked");
        for id in ["a", "c"] {
            p.apply(
                AgentStatusEvent::ToolFinished {
                    tool_call_id: id.into(),
                },
                0,
            );
        }
        assert_eq!(
            p.current_event(AgentStatusEvent::ToolFinished {
                tool_call_id: "c".into()
            }),
            AgentStatusEvent::Thinking
        );
    }

    #[test]
    fn wait_uses_reason_and_refreshes_remaining_time() {
        let mut projection = ProjectionState::default();
        assert_eq!(
            projection.apply(
                AgentStatusEvent::Waiting {
                    completed_tool_call_id: None,
                    reason: "the build".to_owned(),
                    deadline_ms: 65_000,
                },
                0,
            ),
            "Waiting: the build · 01:05"
        );
        assert_eq!(projection.refresh_wait(5_000), "Waiting: the build · 01:00");
        let later = AgentStatusEvent::Waiting {
            completed_tool_call_id: Some("wait-longer".into()),
            reason: "longer check".into(),
            deadline_ms: 90_000,
        };
        projection.apply(later.clone(), 5_000);
        assert_eq!(projection.refresh_wait(5_000), "Waiting: the build · 01:00");
        assert!(matches!(
            projection.current_event(later),
            AgentStatusEvent::Waiting {
                deadline_ms: 65_000,
                ..
            }
        ));
    }

    #[test]
    fn explicit_wait_remains_projected_after_the_tool_batch_finishes() {
        let mut projection = ProjectionState::default();
        projection.apply(
            AgentStatusEvent::Waiting {
                completed_tool_call_id: None,
                reason: "the build".to_owned(),
                deadline_ms: 65_000,
            },
            0,
        );
        if let Some(event) = AgentEvent::AutoWaitEnded.status_event() {
            projection.apply(event, 0);
        }
        assert_eq!(projection.refresh_wait(5_000), "Waiting: the build · 01:00");
    }

    #[test]
    fn automatic_tool_wait_uses_the_same_countdown_projection() {
        let mut projection = ProjectionState::default();
        assert_eq!(
            projection.apply(
                AgentStatusEvent::ToolsWaiting {
                    calls: vec![AgentToolCall {
                        tool_call_id: "bash-1".to_owned(),
                        tool_name: "shell.run".to_owned(),
                        detail: String::new(),
                        labels: Default::default(),
                        action: String::new(),
                    }],
                    deadline_ms: 60_000,
                },
                0,
            ),
            "Waiting: Working to finish · 01:00"
        );
        assert_eq!(
            projection.refresh_wait(5_000),
            "Waiting: Working to finish · 00:55"
        );
    }

    #[test]
    fn failure_is_visible_and_only_real_clear_events_remove_it() {
        let mut projection = ProjectionState::default();
        assert_eq!(
            projection.apply(
                AgentStatusEvent::Failed {
                    reason: "provider rejected the request".to_owned(),
                },
                0,
            ),
            "Failed: provider rejected the request"
        );
        let end = AgentStatusEvent::Failed {
            reason: "turn failed".into(),
        };
        assert_eq!(
            projection.apply(end.clone(), 0),
            "Failed: provider rejected the request"
        );
        assert_eq!(
            projection.current_event(end),
            AgentStatusEvent::Failed {
                reason: "provider rejected the request".into()
            }
        );
        assert_eq!(projection.apply(AgentStatusEvent::Interrupted, 0), "");
    }

    #[test]
    fn tool_results_reveal_the_next_active_tool_then_thinking() {
        let mut projection = ProjectionState::default();
        assert_eq!(
            projection.apply(
                AgentStatusEvent::ToolsStarted {
                    thinking: false,
                    calls: vec![
                        AgentToolCall {
                            tool_call_id: "read-1".to_owned(),
                            tool_name: "file.read".to_owned(),
                            detail: String::new(),
                            labels: Default::default(),
                            action: String::new(),
                        },
                        AgentToolCall {
                            tool_call_id: "bash-1".to_owned(),
                            tool_name: "shell.run".to_owned(),
                            detail: String::new(),
                            labels: Default::default(),
                            action: String::new(),
                        },
                    ],
                },
                0,
            ),
            "Working"
        );
        assert_eq!(
            projection.apply(
                AgentStatusEvent::ToolFinished {
                    tool_call_id: "bash-1".to_owned(),
                },
                0,
            ),
            "Working"
        );
        assert_eq!(
            projection.apply(
                AgentStatusEvent::ToolFinished {
                    tool_call_id: "read-1".to_owned(),
                },
                0,
            ),
            "Thinking..."
        );
    }
}
