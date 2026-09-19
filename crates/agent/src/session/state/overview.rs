//! Aggregate state folded alongside the authoritative session and included in
//! its existing snapshots. No event archive, per-history index or history query.
use super::super::events::{ToolOutcome, WAIT_TOOL_NAME};
use super::*;
use zork_agent_api::{
    ExecutionActivity, ExecutionRun, ExecutionStep, ExecutionTarget, ExecutionTool, ExecutionWait,
    SessionAggregates, SessionExecution, SessionStatus,
};

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct OverviewState {
    pub cursor: Option<String>,
    pub aggregates: SessionAggregates,
    pub last_provider: Option<ResolvedProvider>,
    waiting: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ResolvedProvider {
    pub profile_id: String,
    pub model: String,
}

/// Copied directly in the runner's command turn, without cloning its generation
/// transcript. The application enriches only profile metadata afterwards.
#[derive(Clone, Debug)]
pub struct OverviewProjection {
    pub session_id: String,
    pub cursor: Option<String>,
    pub selection: Option<Selection>,
    pub resolved_profile: Option<String>,
    pub context_tokens: Option<u64>,
    pub aggregates: SessionAggregates,
    pub execution: SessionExecution,
}

pub(super) enum OverviewUpdate {
    Created,
    TurnStarted,
    TurnFinished {
        run: Option<ExecutionRun>,
        at: i64,
    },
    Completed {
        usage: Option<Usage>,
        provider: Option<ResolvedProvider>,
        matched: bool,
    },
    ContextApplied {
        provider: Option<ResolvedProvider>,
    },
    Failed {
        usage: Option<Usage>,
        activity: ExecutionActivity,
        matched: bool,
    },
    Tool {
        activity: ExecutionActivity,
        wait: bool,
        matched: bool,
    },
    Wake {
        invocation: Option<String>,
        at: i64,
        state: &'static str,
    },
    None,
}

fn bounded(text: &str) -> String {
    text.chars().take(256).collect()
}

impl OverviewUpdate {
    pub(super) fn from_event(event: &SessionEvent, state: &SessionState) -> Self {
        match event {
            SessionEvent::SessionCreated { .. } => Self::Created,
            SessionEvent::TurnStarted { .. } => Self::TurnStarted,
            SessionEvent::TurnFinished {
                turn_id,
                outcome,
                finished_at_ms,
                ..
            } => Self::TurnFinished {
                run: state
                    .active_turn
                    .as_ref()
                    .filter(|turn| turn.turn_id == *turn_id)
                    .map(|turn| ExecutionRun {
                        turn_id: turn_id.clone(),
                        started_at_ms: turn.started_at_ms,
                        finished_at_ms: Some(*finished_at_ms),
                        outcome: Some(
                            match outcome {
                                TurnOutcome::Finished => "finished",
                                TurnOutcome::Failed => "failed",
                                TurnOutcome::Cancelled => "cancelled",
                            }
                            .into(),
                        ),
                    }),
                at: *finished_at_ms,
            },
            SessionEvent::StepCompleted {
                step_id,
                usage,
                provider_context,
                ..
            } => Self::Completed {
                usage: usage.clone(),
                provider: provider_context.as_ref().map(|p| ResolvedProvider {
                    profile_id: p.profile_id.clone(),
                    model: p.model.clone(),
                }),
                matched: state
                    .active_step
                    .as_ref()
                    .is_some_and(|step| step.step_id == *step_id),
            },
            SessionEvent::ContextApplied { retained, .. } => Self::ContextApplied {
                // This is part of the context transition's existing retained
                // range work. Overview reads never scan the generation text.
                provider: state
                    .generation
                    .entries
                    .iter()
                    .take(retained.end)
                    .skip(retained.start)
                    .rev()
                    .find_map(|entry| match entry {
                        GenerationEntry::Assistant {
                            provider_context: Some(provider),
                            ..
                        } => Some(ResolvedProvider {
                            profile_id: provider.profile_id.clone(),
                            model: provider.model.clone(),
                        }),
                        _ => None,
                    }),
            },
            SessionEvent::StepFailed {
                step_id,
                error,
                failed_at_ms,
            } => Self::Failed {
                usage: error.usage.clone(),
                activity: ExecutionActivity {
                    id: format!("model:{step_id}"),
                    lane: 1,
                    tool: String::new(),
                    action: "conversation".into(),
                    state: "failed".into(),
                    finished_at_ms: *failed_at_ms,
                    error: Some(bounded(&error.message)),
                },
                matched: state
                    .active_step
                    .as_ref()
                    .is_some_and(|step| step.step_id == *step_id),
            },
            SessionEvent::ToolResult { result } => {
                let pending = state.pending_tools.get(&result.invocation_id);
                let error = (result.outcome != ToolOutcome::Succeeded)
                    .then(|| {
                        ["error", "stderr", "message"]
                            .iter()
                            .find_map(|key| result.data[*key].as_str())
                            .or_else(|| result.data["error"]["message"].as_str())
                            .map(bounded)
                    })
                    .flatten();
                Self::Tool {
                    activity: ExecutionActivity {
                        id: format!("tool:{}", result.invocation_id),
                        lane: 2,
                        tool: result.tool.clone(),
                        action: pending
                            .and_then(|p| p.invocation.activity.as_ref())
                            .map(|a| bounded(&a.action))
                            .unwrap_or_else(|| result.tool.clone()),
                        state: match result.outcome {
                            ToolOutcome::Succeeded => "succeeded",
                            ToolOutcome::Failed => "failed",
                            ToolOutcome::TimedOut => "timed_out",
                            ToolOutcome::Cancelled => "cancelled",
                            ToolOutcome::Interrupted => "interrupted",
                        }
                        .into(),
                        finished_at_ms: result.finished_at_ms,
                        error,
                    },
                    wait: result.tool == WAIT_TOOL_NAME
                        && result.outcome == ToolOutcome::Succeeded
                        && result.data["until_ms"].as_i64().is_some(),
                    matched: pending.is_some_and(|p| p.result.is_none()),
                }
            }
            SessionEvent::StepStarted {
                consumed_inputs,
                started_at_ms,
                ..
            } if !consumed_inputs.is_empty() => Self::Wake {
                invocation: None,
                at: *started_at_ms,
                state: "succeeded",
            },
            SessionEvent::DeadlineReached {
                deadline: DeadlineKind::Wait { invocation_id },
                reached_at_ms,
            } => Self::Wake {
                invocation: Some(format!("tool:{invocation_id}")),
                at: *reached_at_ms,
                state: "succeeded",
            },
            _ => Self::None,
        }
    }
}

impl OverviewState {
    pub fn fresh() -> Self {
        Self {
            aggregates: SessionAggregates {
                complete: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }
    pub(super) fn apply(&mut self, update: OverviewUpdate) {
        match update {
            OverviewUpdate::Created => *self = Self::fresh(),
            OverviewUpdate::TurnStarted => {
                self.aggregates.run_count = self.aggregates.run_count.saturating_add(1)
            }
            OverviewUpdate::TurnFinished { run, at } => {
                if run.is_some() {
                    self.aggregates.last_run = run;
                }
                self.end_wait(None, at, "interrupted");
            }
            OverviewUpdate::Completed {
                usage,
                provider,
                matched,
            } => {
                self.add_usage(usage.as_ref(), matched);
                if matched && provider.is_some() {
                    self.last_provider = provider;
                }
            }
            OverviewUpdate::ContextApplied { provider } => self.last_provider = provider,
            OverviewUpdate::Failed {
                usage,
                activity,
                matched,
            } => {
                self.add_usage(usage.as_ref(), matched);
                if matched {
                    self.recent(activity);
                }
            }
            OverviewUpdate::Tool {
                activity,
                wait,
                matched,
            } => {
                if !matched {
                    return;
                }
                if wait {
                    self.end_wait(None, activity.finished_at_ms, "interrupted");
                    self.waiting = Some(activity.id);
                } else {
                    self.recent(activity);
                }
            }
            OverviewUpdate::Wake {
                invocation,
                at,
                state,
            } => self.end_wait(invocation.as_deref(), at, state),
            OverviewUpdate::None => {}
        }
    }
    fn add_usage(&mut self, usage: Option<&Usage>, matched: bool) {
        if !matched {
            self.aggregates.complete = false;
            return;
        }
        let Some(usage) = usage else {
            return;
        };
        let total = &mut self.aggregates.usage;
        total.input = total.input.saturating_add(usage.input_tokens);
        total.output = total.output.saturating_add(usage.output_tokens);
        total.reported_steps = total.reported_steps.saturating_add(1);
        if let Some(cached) = usage.cached_input_tokens {
            total.cached = total.cached.saturating_add(cached.min(usage.input_tokens));
            total.cache_input = total.cache_input.saturating_add(usage.input_tokens);
            total.cache_reported_steps = total.cache_reported_steps.saturating_add(1);
        }
    }
    fn recent(&mut self, activity: ExecutionActivity) {
        self.aggregates.recent.retain(|old| old.id != activity.id);
        self.aggregates.recent.insert(0, activity);
        self.aggregates.recent.truncate(2);
    }
    fn end_wait(&mut self, id: Option<&str>, at: i64, state: &str) {
        if self
            .waiting
            .as_deref()
            .is_none_or(|waiting| id.is_some_and(|id| id != waiting))
        {
            return;
        }
        let id = self.waiting.take().unwrap();
        self.recent(ExecutionActivity {
            id,
            lane: 2,
            tool: WAIT_TOOL_NAME.into(),
            action: WAIT_TOOL_NAME.into(),
            state: state.into(),
            finished_at_ms: at,
            error: None,
        });
    }
}

impl SessionState {
    pub fn session_status(&self) -> SessionStatus {
        if self.active_step.is_some() {
            SessionStatus::Thinking
        } else if self.auto_wait.is_some() || self.wait_deadline.is_some() {
            SessionStatus::Waiting
        } else if self.active_turn.is_some()
            || (!self.pending_tools.is_empty()
                && self.last_turn_outcome != Some(TurnOutcome::Cancelled))
        {
            SessionStatus::Working
        } else {
            match self.last_turn_outcome {
                Some(TurnOutcome::Finished) => SessionStatus::Finished,
                Some(TurnOutcome::Failed) => SessionStatus::Failed,
                Some(TurnOutcome::Cancelled) => SessionStatus::Cancelled,
                None => SessionStatus::Wait,
            }
        }
    }
    pub fn overview(&self) -> OverviewProjection {
        let resolved_profile = self.selection.as_ref().and_then(|selection| {
            if !selection.is_auto() {
                return Some(selection.profile_id.clone());
            }
            self.overview
                .last_provider
                .as_ref()
                .filter(|provider| provider.model == selection.model)
                .map(|provider| provider.profile_id.clone())
        });
        let mut tools = self
            .pending_tools
            .values()
            .filter(|pending| pending.result.is_none() && pending.invocation.rejection.is_none())
            .map(|pending| {
                let invocation = &pending.invocation;
                let activity = invocation.activity.clone().unwrap_or_default();
                ExecutionTool {
                    invocation_id: invocation.invocation_id.clone(),
                    tool: invocation.tool.clone(),
                    started_at_ms: invocation.started_at_ms,
                    action: activity.action,
                    labels: activity.labels,
                    detail: activity.detail,
                    target: activity.target.map(|target| match target {
                        super::super::tools::ActivityTarget::Agent(id) => {
                            ExecutionTarget::Agent(id)
                        }
                        super::super::tools::ActivityTarget::Task(id) => ExecutionTarget::Task(id),
                    }),
                }
            })
            .collect::<Vec<_>>();
        tools.sort_by(|a, b| {
            (a.started_at_ms, &a.invocation_id).cmp(&(b.started_at_ms, &b.invocation_id))
        });
        let waiting = self
            .wait_deadline
            .as_ref()
            .map(|wait| ExecutionWait {
                deadline_ms: wait.deadline_ms,
                tools: false,
                reason: self
                    .pending_tools
                    .get(&wait.invocation_id)
                    .and_then(|pending| pending.result.as_ref())
                    .and_then(|result| result.data["reason"].as_str())
                    .map(bounded)
                    .unwrap_or_else(|| "the requested time".into()),
            })
            .or_else(|| {
                self.auto_wait.as_ref().map(|wait| ExecutionWait {
                    deadline_ms: wait.deadline_ms,
                    tools: true,
                    reason: "background tools to finish".into(),
                })
            });
        let execution = SessionExecution {
            generation: self.generation.number,
            status: self.session_status(),
            active_step: self.active_step.as_ref().map(|step| ExecutionStep {
                step_id: step.step_id.clone(),
                started_at_ms: step.started_at_ms,
            }),
            active_turn: self.active_turn.as_ref().map(|turn| ExecutionRun {
                turn_id: turn.turn_id.clone(),
                started_at_ms: turn.started_at_ms,
                finished_at_ms: None,
                outcome: None,
            }),
            tools,
            waiting,
            failure: self
                .last_turn_failure
                .as_ref()
                .map(|reason| bounded(reason))
                .or_else(|| {
                    self.last_step_failure
                        .as_ref()
                        .map(|failure| bounded(&failure.error.message))
                }),
        };
        OverviewProjection {
            session_id: self.session_id.clone(),
            cursor: self.overview.cursor.clone(),
            selection: self.selection.clone(),
            resolved_profile,
            context_tokens: self.anchor().map(|anchor| anchor.input_tokens),
            aggregates: self.overview.aggregates.clone(),
            execution,
        }
    }
}
