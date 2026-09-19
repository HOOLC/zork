//! The single durable writer and execution loop for one active session.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use super::compression::SegmentCompressor;
use super::context::summary_transcript;
use super::deadline::DeadlineScheduler;
use super::decision::{decide, handoff_document, Decision, DecisionWorld};
use super::events::{
    DeadlineKind, Input, ProviderErrorRecord, Purpose, RuntimeFailure, Selection, SessionEvent,
    ToolInvocation, ToolOutcome, ToolResultData, Usage,
};
use super::executor::{rejected_execution, CompletedTool, ToolExecutor};
use super::model::{
    ModelError, ModelGateway, ModelOutcome, ModelReleaseSuggestion, ModelRequest,
    ModelStreamObserver, TOOL_INTERRUPTED_MESSAGE,
};
use super::ports::{CancelToolRequest, Clock, IdGenerator, ToolCancellation, ToolControl};
use super::projection::provider_transcript;
use super::state::{snapshot_value, GenerationEntry, SessionState, STATE_SCHEMA_VERSION};
use super::store::{EventEnvelope, SessionStore, StoreError};
use super::tools::{provider_call_definition, DynamicCall, ToolExecution, ToolRegistry};

pub type InputBudget = Arc<dyn Fn(&SessionState) -> Option<u64> + Send + Sync>;
pub type MaxOutputTokens = Arc<dyn Fn(&SessionState) -> Option<u32> + Send + Sync>;
/// The host owns Agent definitions; None preserves standalone session selection.
pub type SelectionSource = Arc<dyn Fn(&str) -> anyhow::Result<Option<Selection>> + Send + Sync>;
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Configuration {
    pub revision: String,
    pub selection: Selection,
    pub system_prompt: Option<String>,
    /// Host-owned instructions for confirming a text-only turn ending. None
    /// preserves standalone natural completion; confirmation uses `end`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_turn_confirmation: Option<String>,
}
pub type ConfigurationSource =
    Arc<dyn Fn(&str) -> anyhow::Result<Option<Configuration>> + Send + Sync>;

pub trait RunnerObserver: Send + Sync {
    fn output_delta(&self, _session_id: &str, _generation: u64, _step_id: &str, _bytes: u64) {}
    fn persisted(&self, _session_id: &str, _events: &[EventEnvelope]) {}
    fn state_changed(&self, _state: &SessionState) {}
    fn text_delta(&self, _session_id: &str, _generation: u64, _step_id: &str, _text: &str) {}
}

pub struct NoopRunnerObserver;

impl RunnerObserver for NoopRunnerObserver {}

#[derive(Clone)]
pub struct RunnerOptions {
    pub configuration_source: Option<ConfigurationSource>,
    pub selection_source: Option<SelectionSource>,
    pub skill_sources: Option<crate::skills::SkillSources>,
    pub skill_catalog: Option<crate::skills::SkillCatalogSource>,
    pub auto_wait: Duration,
    pub provider_retry_limit: u32,
    pub context_attempt_limit: u32,
    pub context: zork_config::ContextConfig,
    pub provider_retry_base: Duration,
    pub provider_retry_max: Duration,
    pub tool_result_capacity: usize,
    pub max_tool_result_json_bytes: usize,
    pub input_budget: InputBudget,
    pub max_output_tokens: MaxOutputTokens,
}

impl Default for RunnerOptions {
    fn default() -> Self {
        Self {
            configuration_source: None,
            selection_source: None,
            skill_sources: None,
            skill_catalog: None,
            auto_wait: Duration::from_secs(60),
            provider_retry_limit: 10,
            context_attempt_limit: 10,
            context: zork_config::ContextConfig::default(),
            provider_retry_base: Duration::from_millis(500),
            provider_retry_max: Duration::from_secs(5),
            tool_result_capacity: 64,
            max_tool_result_json_bytes: 1024 * 1024,
            input_budget: Arc::new(|_| None),
            max_output_tokens: Arc::new(|_| None),
        }
    }
}

#[derive(Clone)]
pub struct RunnerDependencies {
    pub store: Arc<dyn SessionStore>,
    pub model: Arc<dyn ModelGateway>,
    pub tools: Arc<ToolRegistry>,
    pub executor: Arc<ToolExecutor>,
    pub deadlines: DeadlineScheduler,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdGenerator>,
    pub observer: Arc<dyn RunnerObserver>,
    pub compressor: SegmentCompressor,
    pub options: RunnerOptions,
}

pub enum RunnerCommand {
    Create {
        selection: Selection,
        system_prompt: Option<String>,
        workspace: String,
        context: Option<zork_config::ContextConfig>,
        response: Response,
        capacity: Option<tokio::sync::OwnedSemaphorePermit>,
    },
    Input {
        request_id: Option<String>,
        position: Option<super::events::InputPosition>,
        wake: bool,
        content: String,
        response: Response,
        capacity: Option<tokio::sync::OwnedSemaphorePermit>,
    },
    SetSelection {
        selection: Selection,
        response: Response,
        capacity: Option<tokio::sync::OwnedSemaphorePermit>,
    },
    SetContext {
        config: zork_config::ContextConfig,
        response: Response,
        capacity: Option<tokio::sync::OwnedSemaphorePermit>,
    },
    CancelTurn {
        expected_turn: Option<String>,
        response: Response,
        capacity: Option<tokio::sync::OwnedSemaphorePermit>,
    },
    RecordFault {
        failure: RuntimeFailure,
        consecutive_count: u32,
        circuit_open: bool,
    },
    Deadline(DeadlineKind),
    Inspect(tokio::sync::oneshot::Sender<SessionState>),
    InspectOverview(tokio::sync::oneshot::Sender<super::state::OverviewProjection>),
    Stop(Response),
}

pub type Response = tokio::sync::oneshot::Sender<Result<(), RunnerRequestError>>;

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct RunnerRequestError {
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunnerFailure {
    pub failure_id: String,
    pub stage: String,
    pub message: String,
}

impl RunnerFailure {
    pub fn fingerprint(&self) -> String {
        RuntimeFailure {
            failure_id: self.failure_id.clone(),
            stage: self.stage.clone(),
            message: self.message.clone(),
        }
        .fingerprint()
    }
}

pub struct IdleRunner {
    pub state: SessionState,
    pub commands: tokio::sync::mpsc::Receiver<RunnerCommand>,
}

pub enum RunnerExit {
    Idle(Box<IdleRunner>),
    CircuitOpen,
    Stopped,
    Failed(RunnerFailure),
}

// A dropped model JoinHandle detaches its request. Keep cancellation tied
// to the runner's lifetime, including panic paths, and join on normal errors.
struct ModelTask(Option<tokio::task::JoinHandle<Result<ModelOutcome, ModelError>>>);

impl ModelTask {
    async fn join(&mut self) -> Result<Result<ModelOutcome, ModelError>, tokio::task::JoinError> {
        let result = self.0.as_mut().expect("active model task").await;
        self.0.take();
        result
    }

    async fn cancel_and_wait(&mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for ModelTask {
    fn drop(&mut self) {
        if let Some(task) = &self.0 {
            task.abort();
        }
    }
}

pub struct SessionRunner {
    dependencies: RunnerDependencies,
    state: SessionState,
    commands: tokio::sync::mpsc::Receiver<RunnerCommand>,
    tool_results: tokio::sync::mpsc::Receiver<CompletedTool>,
    tool_result_sender: tokio::sync::mpsc::Sender<CompletedTool>,
    tool_control: ToolControl,
    control_requests: tokio::sync::mpsc::Receiver<CancelToolRequest>,
    cancel_waiters: HashMap<String, Vec<(bool, CancelToolRequest)>>,
}

impl SessionRunner {
    pub fn new(
        dependencies: RunnerDependencies,
        state: SessionState,
        commands: tokio::sync::mpsc::Receiver<RunnerCommand>,
    ) -> Self {
        let (tool_result_sender, tool_results) =
            tokio::sync::mpsc::channel(dependencies.options.tool_result_capacity.max(1));
        let (tool_control, control_requests) =
            ToolControl::channel(dependencies.options.tool_result_capacity);
        Self {
            dependencies,
            state,
            commands,
            tool_results,
            tool_result_sender,
            tool_control,
            control_requests,
            cancel_waiters: HashMap::new(),
        }
    }

    pub async fn run(mut self) -> RunnerExit {
        loop {
            while let Ok(command) = self.commands.try_recv() {
                match self.handle_command(command).await {
                    Ok(CommandEffect::Continue) | Ok(CommandEffect::CancelTurn) => {}
                    Ok(CommandEffect::CircuitOpen) => return RunnerExit::CircuitOpen,
                    Ok(CommandEffect::Stop) => return RunnerExit::Stopped,
                    Err(error) => return RunnerExit::Failed(error),
                }
            }
            // Completed tools leave the executor only after enqueueing. Snapshot
            // before draining so completion cannot look like an interrupted tool.
            let live_tools = self.dependencies.executor.live(&self.state.session_id);
            while let Ok(completed) = self.tool_results.try_recv() {
                if let Err(error) = self.persist_tool_result(completed).await {
                    return RunnerExit::Failed(error);
                }
            }

            while let Ok(request) = self.control_requests.try_recv() {
                if let Err(error) = self.handle_tool_control(request).await {
                    return RunnerExit::Failed(error);
                }
            }

            // Refresh before deciding compaction and output budgets, not just
            // inside the provider adapter. A request already in flight keeps
            // its immutable selection snapshot.
            if self.state.is_created() && self.state.active_step.is_none() {
                if let Err(error) = self.refresh_selection().await {
                    return RunnerExit::Failed(error);
                }
            }
            let world = self.world(live_tools);
            match decide(&self.state, &world) {
                Decision::InterruptStep { step_id, reason } => {
                    if let Err(error) = self
                        .append(vec![SessionEvent::StepInterrupted {
                            step_id,
                            reason,
                            interrupted_at_ms: self.dependencies.clock.now_ms(),
                        }])
                        .await
                    {
                        return RunnerExit::Failed(error);
                    }
                }
                Decision::InterruptTools { invocation_ids } => {
                    let events = invocation_ids
                        .into_iter()
                        .filter_map(|invocation_id| {
                            self.state.pending(&invocation_id).map(|pending| {
                                let execution =
                                    pending.invocation.rejection.as_deref().map_or_else(
                                        || ToolExecution {
                                            images: Vec::new(),
                                            outcome: ToolOutcome::Interrupted,
                                            data: serde_json::json!({
                                                "error": TOOL_INTERRUPTED_MESSAGE,
                                                "reason": "runtime_interrupted",
                                                "result_unknown": true,
                                            }),
                                            result_schema_version: 1,
                                            knowledge: None,
                                        },
                                        rejected_execution,
                                    );
                                SessionEvent::ToolResult {
                                    result: ToolResultData {
                                        images: Vec::new(),
                                        invocation_id,
                                        tool: pending.invocation.tool.clone(),
                                        outcome: execution.outcome,
                                        data: execution.data,
                                        result_schema_version: execution.result_schema_version,
                                        knowledge: execution.knowledge,
                                        finished_at_ms: self.dependencies.clock.now_ms(),
                                    },
                                }
                            })
                        })
                        .collect();
                    if let Err(error) = self.append(events).await {
                        return RunnerExit::Failed(error);
                    }
                }
                Decision::StartTurn => {
                    if let Err(error) = self
                        .append(vec![SessionEvent::TurnStarted {
                            turn_id: self.dependencies.ids.next(),
                            started_at_ms: self.dependencies.clock.now_ms(),
                        }])
                        .await
                    {
                        return RunnerExit::Failed(error);
                    }
                }
                Decision::EndAutoWait { step_id, reason } => {
                    if let Err(error) = self
                        .append(vec![SessionEvent::AutoWaitEnded {
                            step_id,
                            reason,
                            ended_at_ms: self.dependencies.clock.now_ms(),
                        }])
                        .await
                    {
                        return RunnerExit::Failed(error);
                    }
                }
                Decision::StartStep {
                    purpose,
                    include_pending_tools,
                    outstanding,
                } => {
                    match self
                        .start_step(purpose, include_pending_tools, outstanding)
                        .await
                    {
                        Ok(StepEffect::Continue) => {}
                        Ok(StepEffect::CircuitOpen) => return RunnerExit::CircuitOpen,
                        Ok(StepEffect::Stop) => return RunnerExit::Stopped,
                        Err(error) => return RunnerExit::Failed(error),
                    }
                }
                Decision::ApplyContext {
                    purpose,
                    document,
                    failure,
                } => {
                    if let Err(error) = self
                        .apply_context(Vec::new(), purpose, document, failure)
                        .await
                    {
                        return RunnerExit::Failed(error);
                    }
                }
                Decision::FinishTurn {
                    outcome,
                    outstanding,
                } => {
                    let Some(turn) = self.state.active_turn.as_ref() else {
                        return RunnerExit::Failed(invariant_failure(
                            "decision.finish_turn",
                            "FinishTurn selected without an active turn",
                        ));
                    };
                    let turn_id = turn.turn_id.clone();
                    if let Err(error) = self
                        .append(vec![SessionEvent::TurnFinished {
                            turn_id,
                            reason: (outcome == super::events::TurnOutcome::Failed)
                                .then(|| self.state.end_confirmation_failure())
                                .flatten()
                                .map(str::to_owned),
                            outcome,
                            outstanding,
                            finished_at_ms: self.dependencies.clock.now_ms(),
                        }])
                        .await
                    {
                        return RunnerExit::Failed(error);
                    }
                }
                Decision::ReachDeadline { deadline } => {
                    if let Err(error) = self
                        .append(vec![SessionEvent::DeadlineReached {
                            deadline,
                            reached_at_ms: self.dependencies.clock.now_ms(),
                        }])
                        .await
                    {
                        return RunnerExit::Failed(error);
                    }
                }
                Decision::Park { .. } => {
                    if self.is_fully_idle() {
                        return RunnerExit::Idle(Box::new(IdleRunner {
                            state: self.state,
                            commands: self.commands,
                        }));
                    }
                    if let Err(error) = self.arm_current_deadline().await {
                        return RunnerExit::Failed(error);
                    }
                    tokio::select! {
                        command = self.commands.recv() => match command {
                            Some(command) => match self.handle_command(command).await {
                                Ok(CommandEffect::Continue) | Ok(CommandEffect::CancelTurn) => {}
                                Ok(CommandEffect::CircuitOpen) => return RunnerExit::CircuitOpen,
                                Ok(CommandEffect::Stop) => return RunnerExit::Stopped,
                                Err(error) => return RunnerExit::Failed(error),
                            },
                            None => return RunnerExit::Stopped,
                        },
                        request = self.control_requests.recv() => {
                            if let Some(request) = request {
                                if let Err(error) = self.handle_tool_control(request).await {
                                    return RunnerExit::Failed(error);
                                }
                            }
                        }
                        completed = self.tool_results.recv() => {
                            if let Some(completed) = completed {
                                if let Err(error) = self.persist_tool_result(completed).await {
                                    return RunnerExit::Failed(error);
                                }
                            }
                        }
                    }
                }
            }
            tokio::task::yield_now().await;
        }
    }

    async fn refresh_selection(&mut self) -> Result<(), RunnerFailure> {
        if let Some(source) = &self.dependencies.options.configuration_source {
            if let Some(configuration) = source(&self.state.session_id)
                .map_err(|error| infrastructure_failure("configuration.resolve", error))?
            {
                if self.state.configuration_revision.as_deref() != Some(&configuration.revision)
                    || self.state.system_prompt != configuration.system_prompt
                    || self.state.end_turn_confirmation != configuration.end_turn_confirmation
                    || self.state.selection.as_ref() != Some(&configuration.selection)
                {
                    self.append(vec![SessionEvent::ConfigurationChanged { configuration }])
                        .await?;
                }
                return Ok(());
            }
        }
        let Some(source) = &self.dependencies.options.selection_source else {
            return Ok(());
        };
        let selection = source(&self.state.session_id)
            .map_err(|error| infrastructure_failure("selection.resolve", error))?;
        if let Some(selection) = selection {
            if self.state.selection.as_ref() != Some(&selection) {
                self.append(vec![SessionEvent::SelectionChanged { selection }])
                    .await?;
            }
        }
        Ok(())
    }

    fn world(&self, live_tools: std::collections::BTreeSet<String>) -> DecisionWorld {
        DecisionWorld {
            now_ms: self.dependencies.clock.now_ms(),
            live_tools,
            tool_changes: self.dependencies.tools.changes(&self.state.known_tools),
            outstanding: self.state.outstanding(&self.dependencies.tools),
            estimated_input_tokens: estimate_input_tokens(&self.state),
            input_budget: (self.dependencies.options.input_budget)(&self.state),
            provider_retry_limit: self.dependencies.options.provider_retry_limit,
            context_attempt_limit: self.dependencies.options.context_attempt_limit,
        }
    }

    fn is_fully_idle(&self) -> bool {
        self.state.is_created()
            && self.state.active_turn.is_none()
            && self.state.active_step.is_none()
            && self
                .state
                .pending_tools
                .values()
                .all(|pending| pending.result.is_some())
            && !self.state.has_waking_inputs()
            && self.state.auto_wait.is_none()
            && self.state.wait_deadline.is_none()
    }

    async fn arm_current_deadline(&self) -> Result<(), RunnerFailure> {
        if let Some(wait) = &self.state.auto_wait {
            self.dependencies
                .deadlines
                .arm(
                    self.state.session_id.clone(),
                    DeadlineKind::AutoWait {
                        step_id: wait.step_id.clone(),
                    },
                    self.state.batch_wait_deadline().expect("active tool batch"),
                )
                .await
                .map_err(|error| infrastructure_failure("deadline.arm", error))?;
        }
        if let Some(wait) = &self.state.wait_deadline {
            self.dependencies
                .deadlines
                .arm(
                    self.state.session_id.clone(),
                    DeadlineKind::Wait {
                        invocation_id: wait.invocation_id.clone(),
                    },
                    wait.deadline_ms,
                )
                .await
                .map_err(|error| infrastructure_failure("deadline.arm", error))?;
        }
        Ok(())
    }

    async fn start_step(
        &mut self,
        purpose: Purpose,
        include_pending_tools: bool,
        mut outstanding: Vec<super::events::OutstandingItem>,
    ) -> Result<StepEffect, RunnerFailure> {
        if let Some(turn) = &self.state.active_turn {
            if turn.consecutive_provider_failures > 0
                && !self
                    .state
                    .last_step_failure
                    .as_ref()
                    .is_some_and(|failure| failure.error.is_context_overflow())
            {
                match self
                    .wait_retry_backoff(turn.consecutive_provider_failures)
                    .await?
                {
                    CommandEffect::Continue => {}
                    CommandEffect::CancelTurn => return Ok(StepEffect::Continue),
                    CommandEffect::CircuitOpen => return Ok(StepEffect::CircuitOpen),
                    CommandEffect::Stop => return Ok(StepEffect::Stop),
                }
            }
        }

        let turn_id = self
            .state
            .active_turn
            .as_ref()
            .map(|turn| turn.turn_id.clone())
            .ok_or_else(|| {
                invariant_failure(
                    "decision.start_step",
                    "StartStep selected without an active turn",
                )
            })?;
        let step_id = self.dependencies.ids.next();
        let consumed_inputs = if purpose == Purpose::Conversation {
            self.state
                .unconsumed_inputs
                .iter()
                .map(|input| input.input_id.clone())
                .collect()
        } else {
            Vec::new()
        };
        outstanding
            .retain(|item| item.kind != "mailbox_input" || !consumed_inputs.contains(&item.id));
        let progress = self.state.context_progress();
        let mut deliveries = self.state.planned_deliveries(include_pending_tools);
        if let Some(progress) = progress {
            // Only close calls from a failed handoff response. Real late results
            // and notices wait for the successor, including across retries.
            let control_calls = self.state.generation.entries[progress.source_entries..]
                .iter()
                .rev()
                .find_map(|entry| match entry {
                    GenerationEntry::Assistant { invocations, .. } => Some(invocations),
                    _ => None,
                });
            deliveries.retain(|delivery| {
                let invocation = match delivery {
                    super::events::ToolDelivery::Result { invocation, .. }
                    | super::events::ToolDelivery::Pending { invocation } => invocation,
                };
                purpose == Purpose::Handoff
                    && control_calls.is_some_and(|calls| {
                        calls
                            .iter()
                            .any(|call| call.invocation_id == invocation.invocation_id)
                    })
            });
        }
        let tool_changes = if progress.is_none() {
            self.dependencies
                .tools
                .changes(&self.state.known_tools_after_deliveries(&deliveries))
        } else {
            Vec::new()
        };
        let mut notices = if purpose == Purpose::Compaction {
            Vec::new()
        } else {
            self.state
                .pending_notices
                .iter()
                .filter(|notice| {
                    !purpose.is_context() || !self.state.generation.entries.iter().any(|entry|
                    matches!(entry, GenerationEntry::Notice { message } if message == *notice))
                })
                .cloned()
                .collect()
        };
        if purpose == Purpose::Conversation {
            if self
                .state
                .active_turn
                .as_ref()
                .is_some_and(|turn| turn.unconfirmed_end_attempts > 0)
            {
                if let Some(confirmation) = &self.state.end_turn_confirmation {
                    notices.push(format!("[runtime.end_confirmation] {confirmation}"));
                }
            }
            if let Some(source) = self.dependencies.options.skill_catalog.clone().or_else(|| {
                self.dependencies
                    .options
                    .skill_sources
                    .clone()
                    .map(crate::skills::local_catalog)
            }) {
                let session = self.state.session_id.clone();
                let catalog = tokio::task::spawn_blocking(move || {
                    match source(&session) {
                        Ok(catalog) => catalog,
                        Err(error) => crate::skills::SkillCatalog {
                            skills: Vec::new(),
                            diagnostics: vec![error.to_string()],
                            continuations: vec![],
                        },
                    }
                    .notice()
                })
                .await
                .map_err(|error| infrastructure_failure("skills.discover", error))?;
                let previous = self
                    .state
                    .generation
                    .entries
                    .iter()
                    .rev()
                    .find_map(|entry| match entry {
                        GenerationEntry::Notice { message }
                            if message.starts_with(crate::skills::CATALOG_NOTICE) =>
                        {
                            Some(message)
                        }
                        _ => None,
                    });
                if previous != Some(&catalog) {
                    notices.push(catalog);
                }
            }
        }
        if purpose == Purpose::Handoff {
            notices.push(
                "A context handoff is required. Do not call tool.help or any other logical tool, and do not return explanatory text. Make exactly one provider `call` with this shape: {\"tool\":\"handoff\",\"action\":\"<brief description of the context handoff>\",\"arguments\":{\"document\":\"<complete successor-facing context>\"}}. The non-empty successor-facing context handoff document must give the successor the task, completed work, current state, relevant files and commands, remaining work, and blockers."
                    .into(),
            );
            if self
                .state
                .context_progress()
                .is_some_and(|progress| progress.attempts > 0)
            {
                if let Some((_, invocations)) = self.state.latest_assistant() {
                    if let Err(reason) = handoff_document(invocations) {
                        notices.push(format!(
                            "The previous handoff response was invalid: {reason}."
                        ));
                    }
                }
            }
        }
        let max_output_tokens = (self.dependencies.options.max_output_tokens)(&self.state);
        self.append(vec![SessionEvent::StepStarted {
            step_id: step_id.clone(),
            turn_id,
            purpose,
            consumed_inputs,
            deliveries,
            tool_changes,
            notices,
            outstanding,
            max_output_tokens,
            input_budget: (self.dependencies.options.input_budget)(&self.state),
            started_at_ms: self.dependencies.clock.now_ms(),
        }])
        .await?;

        let selection = self.state.selection.clone().ok_or_else(|| {
            invariant_failure("model.request", "session has no provider selection")
        })?;
        let request = ModelRequest {
            session_id: self.state.session_id.clone(),
            generation: self.state.generation.number,
            step_id: step_id.clone(),
            selection,
            transcript: if purpose == Purpose::Compaction {
                summary_transcript(&self.state)
            } else {
                provider_transcript(&self.state)
            },
            tools: Arc::new(if purpose == Purpose::Compaction {
                Vec::new()
            } else {
                vec![provider_call_definition()]
            }),
            max_output_tokens,
            independent: purpose == Purpose::Compaction,
            stream_observer: Arc::new(StepStreamObserver {
                observer: self.dependencies.observer.clone(),
                observe_text: purpose != Purpose::Compaction,
            }),
        };
        let model = self.dependencies.model.clone();
        let mut task = ModelTask(Some(tokio::spawn(
            async move { model.complete(&request).await },
        )));
        let result = async {
            loop {
                tokio::select! {
                    joined = task.join() => {
                        let result = joined.map_err(|error| RunnerFailure {
                            failure_id: "model_task".into(),
                            stage: "model.complete".into(),
                            message: error.to_string(),
                        })?;
                        return match result {
                            Ok(outcome) => {
                                self.complete_step(step_id, outcome).await?;
                                Ok(StepEffect::Continue)
                            }
                            Err(error) => {
                                self.fail_step(step_id, error).await?;
                                Ok(StepEffect::Continue)
                            }
                        };
                    }
                    command = self.commands.recv() => match command {
                        Some(command) => match self.handle_command(command).await? {
                            CommandEffect::Continue => {}
                            CommandEffect::CancelTurn => {
                                task.cancel_and_wait().await;
                                self.append(vec![SessionEvent::StepInterrupted {
                                    step_id,
                                    reason: super::events::StepInterruptionReason::TurnCancelled,
                                    interrupted_at_ms: self.dependencies.clock.now_ms(),
                                }]).await?;
                                return Ok(StepEffect::Continue);
                            }
                            CommandEffect::CircuitOpen => {
                                task.cancel_and_wait().await;
                                return Ok(StepEffect::CircuitOpen);
                            }
                            CommandEffect::Stop => {
                                task.cancel_and_wait().await;
                                return Ok(StepEffect::Stop);
                            }
                        },
                        None => {
                            task.cancel_and_wait().await;
                            return Ok(StepEffect::Stop);
                        }
                    },
                    request = self.control_requests.recv() => {
                        if let Some(request) = request { self.handle_tool_control(request).await?; }
                    }
                    completed = self.tool_results.recv() => {
                        if let Some(completed) = completed {
                            self.persist_tool_result(completed).await?;
                        }
                    }
                }
            }
        }
        .await;
        task.cancel_and_wait().await;
        result
    }

    async fn wait_retry_backoff(&mut self, failures: u32) -> Result<CommandEffect, RunnerFailure> {
        let shift = failures.saturating_sub(1).min(31);
        let factor = 1_u32 << shift;
        let duration = self
            .dependencies
            .options
            .provider_retry_base
            .saturating_mul(factor)
            .min(self.dependencies.options.provider_retry_max);
        let sleep = self.dependencies.clock.sleep(duration);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                _ = &mut sleep => return Ok(CommandEffect::Continue),
                command = self.commands.recv() => match command {
                    Some(command) => match self.handle_command(command).await? {
                        CommandEffect::Continue => {}
                        effect => return Ok(effect),
                    },
                    None => return Ok(CommandEffect::Stop),
                },
                request = self.control_requests.recv() => {
                    if let Some(request) = request { self.handle_tool_control(request).await?; }
                }
                completed = self.tool_results.recv() => {
                    if let Some(completed) = completed {
                        self.persist_tool_result(completed).await?;
                    }
                }
            }
        }
    }

    async fn complete_step(
        &mut self,
        step_id: String,
        outcome: ModelOutcome,
    ) -> Result<(), RunnerFailure> {
        let purpose = self
            .state
            .active_step
            .as_ref()
            .filter(|step| step.step_id == step_id)
            .map(|step| step.purpose)
            .ok_or_else(|| {
                invariant_failure(
                    "model.complete",
                    "Model outcome arrived without a matching active step",
                )
            })?;
        let completed_at_ms = self.dependencies.clock.now_ms();
        let mut invocations = if purpose == Purpose::Compaction {
            Vec::new()
        } else {
            materialize_invocations(
                &self.state,
                &outcome,
                completed_at_ms,
                self.dependencies.ids.as_ref(),
            )
        };
        for invocation in &mut invocations {
            let mut activity = self
                .dependencies
                .tools
                .activity(&invocation.tool, &invocation.arguments);
            if let Some(description) = invocation.activity.take() {
                activity.action = description.action;
            }
            invocation.activity = Some(activity);
        }
        let context_document = purpose.is_context().then(|| {
            let result = if purpose == Purpose::Compaction {
                if !outcome.tool_calls.is_empty() {
                    Err("compaction must return summary text without tool calls".into())
                } else if outcome.text.trim().is_empty() {
                    Err("compaction returned an empty summary".into())
                } else {
                    Ok(outcome.text.trim().to_owned())
                }
            } else {
                handoff_document(&invocations)
            };
            if let Err(reason) = &result {
                for invocation in &mut invocations {
                    invocation.rejection.get_or_insert_with(|| {
                        format!("not executable because the handoff response was invalid: {reason}")
                    });
                }
            }
            result
        });
        let batch_wait = outcome
            .tool_calls
            .iter()
            .zip(&invocations)
            .filter(|(_, invocation)| invocation.rejection.is_none())
            .filter_map(|(call, _)| {
                DynamicCall::from_value(call.arguments.clone())
                    .ok()?
                    .batch_wait()
            })
            .min()
            .unwrap_or(self.dependencies.options.auto_wait);
        let auto_wait_deadline_ms = (purpose == Purpose::Conversation && !invocations.is_empty())
            .then(|| {
                completed_at_ms
                    .saturating_add(i64::try_from(batch_wait.as_millis()).unwrap_or(i64::MAX))
            });
        let usage = outcome.usage.map(|usage| Usage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cached_input_tokens: usage.cached_input_tokens,
            output_reasoning_tokens: usage.output_reasoning_tokens,
            output_text_tokens: usage.output_text_tokens,
        });
        let completed = SessionEvent::StepCompleted {
            step_id,
            purpose,
            assistant_text: outcome.text,
            provider_calls: outcome.tool_calls,
            invocations: invocations.clone(),
            auto_wait_deadline_ms,
            usage,
            provider_context: outcome.provider_context,
            provider_input: outcome.provider_input,
            completed_at_ms,
        };

        if let Some(context_document) = context_document {
            return match context_document {
                Ok(document) => {
                    self.apply_context(vec![completed], purpose, Some(document), None)
                        .await
                }
                Err(reason) => {
                    let attempts = self
                        .state
                        .context_progress()
                        .map_or(1, |progress| progress.attempts.saturating_add(1));
                    if attempts >= self.dependencies.options.context_attempt_limit.max(1) {
                        self.apply_context(
                            vec![completed],
                            purpose,
                            None,
                            Some(format!(
                                "the model did not produce a valid context document in {attempts} attempts: {reason}"
                            )),
                        )
                        .await
                    } else {
                        let mut events = Vec::with_capacity(invocations.len() + 1);
                        events.push(completed);
                        events.extend(invocations.iter().map(|invocation| {
                            SessionEvent::ToolResult {
                                result: ToolResultData {
                                    images: Vec::new(),
                                    invocation_id: invocation.invocation_id.clone(),
                                    tool: invocation.tool.clone(),
                                    outcome: ToolOutcome::Failed,
                                    data: serde_json::json!({
                                        "error": format!("Invalid handoff response: {reason}"),
                                        "reason": "invalid_handoff_response",
                                    }),
                                    result_schema_version: 1,
                                    knowledge: None,
                                    finished_at_ms: completed_at_ms,
                                },
                            }
                        }));
                        self.append(events).await.map(|_| ())
                    }
                }
            };
        }

        // This response closes the active provider step in the same append.
        let outstanding = self.state.outstanding(&self.dependencies.tools);
        let has_outstanding = outstanding.iter().any(|item| item.kind != "provider_step");
        if invocations.is_empty() && !has_outstanding && self.state.end_turn_confirmation.is_none()
        {
            let turn = self.state.active_turn.as_ref().ok_or_else(|| {
                invariant_failure("model.complete", "Completed step has no active turn")
            })?;
            // Persist the response and natural completion together, so recovery
            // cannot issue another model request for an already finished response.
            let finished = SessionEvent::TurnFinished {
                turn_id: turn.turn_id.clone(),
                outcome: super::events::TurnOutcome::Finished,
                reason: None,
                outstanding: Vec::new(),
                finished_at_ms: completed_at_ms,
            };
            return self.append(vec![completed, finished]).await.map(|_| ());
        }

        self.append(vec![completed]).await?;

        self.dependencies.executor.dispatch_batch(
            &self.state.session_id,
            &self.state.workspace,
            invocations,
            self.tool_result_sender.clone(),
            Some(self.tool_control.clone()),
        );
        Ok(())
    }

    async fn fail_step(&mut self, step_id: String, error: ModelError) -> Result<(), RunnerFailure> {
        self.fail_step_with_record(step_id, provider_error_record(error))
            .await
    }

    async fn fail_step_with_record(
        &mut self,
        step_id: String,
        error: ProviderErrorRecord,
    ) -> Result<(), RunnerFailure> {
        self.append(vec![SessionEvent::StepFailed {
            step_id,
            error,
            failed_at_ms: self.dependencies.clock.now_ms(),
        }])
        .await
        .map(|_| ())
    }

    async fn handle_tool_control(
        &mut self,
        request: CancelToolRequest,
    ) -> Result<(), RunnerFailure> {
        if request.response.is_closed() {
            return Ok(());
        }
        self.cancel_waiters.retain(|_, waiters| {
            waiters.retain(|(_, request)| !request.response.is_closed());
            !waiters.is_empty()
        });
        let Some(pending) = self.state.pending(&request.target) else {
            let _ = request.response.send(Ok(ToolCancellation {
                signalled: false,
                result: None,
            }));
            return Ok(());
        };
        if let Some(result) = &pending.result {
            let _ = request.response.send(Ok(ToolCancellation {
                signalled: false,
                result: Some(result.clone()),
            }));
            return Ok(());
        }
        let persisted = self
            .append(vec![SessionEvent::ToolCancelRequested {
                invocation_id: request.target.clone(),
                requested_at_ms: self.dependencies.clock.now_ms(),
            }])
            .await;
        match persisted {
            Ok(_) => {
                let signalled = self
                    .dependencies
                    .executor
                    .cancel(&self.state.session_id, &request.target);
                // Keep consuming tool results. Blocking this handler would
                // prevent the target's terminal result from being persisted.
                self.cancel_waiters
                    .entry(request.target.clone())
                    .or_default()
                    .push((signalled, request));
                Ok(())
            }
            Err(error) => {
                let _ = request.response.send(Err(error.message.clone()));
                Err(error)
            }
        }
    }

    async fn apply_context(
        &mut self,
        mut events: Vec<SessionEvent>,
        purpose: Purpose,
        document: Option<String>,
        failure: Option<String>,
    ) -> Result<(), RunnerFailure> {
        let previous_generation = self.state.generation.number;
        let generation = previous_generation.saturating_add(1);
        let retained = if document.is_some() && purpose == Purpose::Compaction {
            self.state.context_progress().map_or(0..0, |progress| {
                progress.retain_from..progress.source_entries
            })
        } else {
            0..0
        };
        let carried_tools = self
            .state
            .pending_tools
            .values()
            .filter(|pending| pending.result.is_none())
            .map(|pending| pending.invocation.clone())
            .collect::<Vec<_>>();
        events.reserve(usize::from(failure.is_some()) + 1);
        if let Some(message) = failure {
            events.push(SessionEvent::ContextFailed {
                generation,
                purpose,
                message,
                failed_at_ms: self.dependencies.clock.now_ms(),
            });
        }
        events.push(SessionEvent::ContextApplied {
            generation,
            purpose,
            document,
            retained,
            tools: self.dependencies.tools.initial_catalog(),
            carried_tools,
            applied_at_ms: self.dependencies.clock.now_ms(),
        });
        self.append(events).await?;

        let store = self.dependencies.store.clone();
        let session_id = self.state.session_id.clone();
        let state = snapshot_value(&self.state)
            .map_err(|error| invariant_failure("state.snapshot.serialize", error))?;
        let snapshot = tokio::task::spawn_blocking(move || {
            store.append_snapshot(&session_id, STATE_SCHEMA_VERSION, state)
        })
        .await
        .map_err(|error| infrastructure_failure("store.snapshot.task", error))?
        .map_err(|error| store_failure("store.snapshot", error))?;
        self.state.overview.cursor = Some(snapshot.envelope.event_id.clone());
        self.dependencies.observer.state_changed(&self.state);
        self.dependencies.observer.persisted(
            &self.state.session_id,
            std::slice::from_ref(&snapshot.envelope),
        );
        if snapshot.sealed_segment.is_some() {
            self.dependencies.compressor.wake();
        }
        self.dependencies
            .model
            .release(ModelReleaseSuggestion::Generation {
                session_id: &self.state.session_id,
                generation: previous_generation,
            });
        Ok(())
    }

    async fn persist_tool_result(&mut self, completed: CompletedTool) -> Result<(), RunnerFailure> {
        if completed.session_id != self.state.session_id {
            return Err(invariant_failure(
                "tool.result.route",
                format!(
                    "tool result for session {} reached runner {}",
                    completed.session_id, self.state.session_id
                ),
            ));
        }
        let execution = bound_tool_execution(completed.execution, &self.dependencies.options);
        let result = ToolResultData {
            images: execution.images,
            invocation_id: completed.invocation.invocation_id,
            tool: completed.invocation.tool,
            outcome: execution.outcome,
            data: execution.data,
            result_schema_version: execution.result_schema_version,
            knowledge: execution.knowledge,
            finished_at_ms: self.dependencies.clock.now_ms(),
        };
        self.append(vec![SessionEvent::ToolResult { result }])
            .await
            .map(|_| ())
    }

    async fn handle_command(
        &mut self,
        command: RunnerCommand,
    ) -> Result<CommandEffect, RunnerFailure> {
        match command {
            RunnerCommand::Create {
                selection,
                system_prompt,
                workspace,
                context,
                response,
                capacity,
            } => {
                let _capacity = capacity;
                let result = if self.state.is_created() {
                    Err(RunnerRequestError {
                        message: "session already exists".into(),
                    })
                } else {
                    self.append(vec![
                        SessionEvent::SessionCreated {
                            session_id: self.state.session_id.clone(),
                            created_at_ms: self.dependencies.clock.now_ms(),
                            selection,
                            system_prompt,
                            workspace,
                            tools: self.dependencies.tools.initial_catalog(),
                        },
                        SessionEvent::ContextConfigured {
                            config: context
                                .unwrap_or_else(|| self.dependencies.options.context.clone()),
                        },
                    ])
                    .await
                    .map(|_| ())
                    .map_err(request_error)
                };
                let failed = result.is_err();
                let message = result.as_ref().err().map(|error| error.message.clone());
                let _ = response.send(result);
                if failed {
                    return Err(invariant_failure(
                        "session.create",
                        message.unwrap_or_else(|| "session creation failed".into()),
                    ));
                }
                Ok(CommandEffect::Continue)
            }
            RunnerCommand::Input {
                request_id,
                position,
                wake,
                content,
                response,
                capacity,
            } => {
                let _capacity = capacity;
                if let Some(position) = &position {
                    use sha2::Digest;
                    if let Some(previous) = self.state.input_streams.get(&position.source) {
                        if position.sequence <= previous.sequence {
                            let same = position.sequence < previous.sequence
                                || previous.content_hash
                                    == format!("{:x}", sha2::Sha256::digest(content.as_bytes()));
                            let _ = response.send(if same {
                                Ok(())
                            } else {
                                Err(RunnerRequestError {
                                    message:
                                        "input stream position already contains different input"
                                            .into(),
                                })
                            });
                            return Ok(CommandEffect::Continue);
                        }
                    } else if self.state.input_streams.len() >= 1024 {
                        let _ = response.send(Err(RunnerRequestError {
                            message: "input stream limit reached".into(),
                        }));
                        return Ok(CommandEffect::Continue);
                    }
                }
                if let Some(previous) =
                    self.state.last_input_receipt.as_ref().filter(|receipt| {
                        request_id.as_deref() == Some(receipt.request_id.as_str())
                    })
                {
                    use sha2::Digest;
                    let digest = format!("{:x}", sha2::Sha256::digest(content.as_bytes()));
                    if previous.content_hash == digest {
                        let _ = response.send(Ok(()));
                    } else {
                        let _ = response.send(Err(RunnerRequestError {
                            message: "request_id already contains different input".into(),
                        }));
                    }
                    return Ok(CommandEffect::Continue);
                }
                let result = self
                    .append(vec![SessionEvent::InputAppended {
                        input: Input {
                            input_id: self.dependencies.ids.next(),
                            request_id,
                            position,
                            wake,
                            content,
                            received_at_ms: self.dependencies.clock.now_ms(),
                        },
                    }])
                    .await;
                respond(response, &result);
                result?;
                Ok(CommandEffect::Continue)
            }
            RunnerCommand::SetSelection {
                selection,
                response,
                capacity,
            } => {
                let _capacity = capacity;
                let result = self
                    .append(vec![SessionEvent::SelectionChanged { selection }])
                    .await;
                respond(response, &result);
                result?;
                Ok(CommandEffect::Continue)
            }
            RunnerCommand::SetContext {
                config,
                response,
                capacity,
            } => {
                let _capacity = capacity;
                let result = self
                    .append(vec![SessionEvent::ContextConfigured { config }])
                    .await;
                respond(response, &result);
                result?;
                Ok(CommandEffect::Continue)
            }
            RunnerCommand::CancelTurn {
                expected_turn,
                response,
                capacity,
            } => {
                let _capacity = capacity;
                if expected_turn.as_deref().is_some_and(|expected| {
                    self.state
                        .active_turn
                        .as_ref()
                        .is_none_or(|turn| turn.turn_id != expected)
                }) {
                    let _ = response.send(Err(RunnerRequestError {
                        message: "agent_run_changed".into(),
                    }));
                    return Ok(CommandEffect::Continue);
                }
                let invocation_ids = self
                    .state
                    .active_turn
                    .as_ref()
                    .map(|turn| {
                        self.state
                            .pending_tools
                            .values()
                            .filter(|pending| {
                                pending.result.is_none()
                                    && pending.invocation.turn_id == turn.turn_id
                            })
                            .map(|pending| pending.invocation.invocation_id.clone())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let result = if let Some(turn) = &self.state.active_turn {
                    self.append(vec![SessionEvent::TurnCancelRequested {
                        turn_id: turn.turn_id.clone(),
                        requested_at_ms: self.dependencies.clock.now_ms(),
                    }])
                    .await
                } else {
                    Ok(Vec::new())
                };
                respond(response, &result);
                result?;
                for invocation_id in invocation_ids {
                    self.dependencies
                        .executor
                        .cancel(&self.state.session_id, &invocation_id);
                }
                Ok(CommandEffect::CancelTurn)
            }
            RunnerCommand::RecordFault {
                failure,
                consecutive_count,
                circuit_open,
            } => {
                self.append(vec![SessionEvent::RuntimeFault {
                    failure,
                    consecutive_count,
                    occurred_at_ms: self.dependencies.clock.now_ms(),
                }])
                .await?;
                Ok(if circuit_open {
                    CommandEffect::CircuitOpen
                } else {
                    CommandEffect::Continue
                })
            }
            RunnerCommand::Deadline(_deadline) => Ok(CommandEffect::Continue),
            RunnerCommand::Inspect(response) => {
                let _ = response.send(self.state.clone());
                Ok(CommandEffect::Continue)
            }
            RunnerCommand::InspectOverview(response) => {
                let _ = response.send(self.state.overview());
                Ok(CommandEffect::Continue)
            }
            RunnerCommand::Stop(response) => {
                // Unblock completed tools waiting to enqueue into a full result queue.
                self.tool_results.close();
                self.control_requests.close();
                self.dependencies
                    .executor
                    .cancel_session_and_wait(&self.state.session_id)
                    .await;
                self.dependencies
                    .deadlines
                    .cancel_session(self.state.session_id.clone())
                    .await;
                self.dependencies
                    .model
                    .release(ModelReleaseSuggestion::Session(&self.state.session_id));
                let _ = response.send(Ok(()));
                Ok(CommandEffect::Stop)
            }
        }
    }

    async fn append(
        &mut self,
        events: Vec<SessionEvent>,
    ) -> Result<Vec<EventEnvelope>, RunnerFailure> {
        if events.is_empty() {
            return Ok(Vec::new());
        }
        let store = self.dependencies.store.clone();
        let session_id = self.state.session_id.clone();
        let persisted =
            tokio::task::spawn_blocking(move || store.append_batch(&session_id, &events))
                .await
                .map_err(|error| infrastructure_failure("store.append.task", error))?
                .map_err(|error| store_failure("store.append", error))?;
        self.dependencies
            .observer
            .persisted(&self.state.session_id, &persisted);
        for envelope in &persisted {
            self.state
                .apply(&envelope.event, &self.dependencies.tools)
                .map_err(|error| invariant_failure("state.fold", error))?;
            self.state.overview.cursor = Some(envelope.event_id.clone());
            if let SessionEvent::ToolResult { result } = &envelope.event {
                if let Some(waiters) = self.cancel_waiters.remove(&result.invocation_id) {
                    for (signalled, request) in waiters {
                        let _ = request.response.send(Ok(ToolCancellation {
                            signalled,
                            result: Some(result.clone()),
                        }));
                    }
                }
            }
        }
        self.dependencies.observer.state_changed(&self.state);
        Ok(persisted)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommandEffect {
    Continue,
    CancelTurn,
    CircuitOpen,
    Stop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StepEffect {
    Continue,
    CircuitOpen,
    Stop,
}

struct StepStreamObserver {
    observe_text: bool,
    observer: Arc<dyn RunnerObserver>,
}

impl std::fmt::Debug for StepStreamObserver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("StepStreamObserver")
    }
}

impl ModelStreamObserver for StepStreamObserver {
    fn output_delta(&self, session_id: &str, generation: u64, step_id: &str, bytes: u64) {
        self.observer
            .output_delta(session_id, generation, step_id, bytes);
    }
    fn text_delta(&self, session_id: &str, generation: u64, step_id: &str, text: &str) {
        if self.observe_text {
            self.observer
                .text_delta(session_id, generation, step_id, text);
        }
    }
}

fn materialize_invocations(
    state: &SessionState,
    outcome: &ModelOutcome,
    started_at_ms: i64,
    ids: &dyn IdGenerator,
) -> Vec<ToolInvocation> {
    let turn_id = state
        .active_turn
        .as_ref()
        .map(|turn| turn.turn_id.clone())
        .unwrap_or_default();
    let mut provider_id_counts = std::collections::BTreeMap::new();
    for call in &outcome.tool_calls {
        *provider_id_counts
            .entry(call.tool_call_id.as_str())
            .or_insert(0_usize) += 1;
    }
    let mut invocations = Vec::with_capacity(outcome.tool_calls.len());
    for call in &outcome.tool_calls {
        let mut rejection = Vec::new();
        if turn_id.is_empty() {
            rejection.push("provider returned tool calls without an active turn".to_owned());
        }
        if call.tool_call_id.is_empty() {
            rejection.push("provider returned an empty tool call ID".to_owned());
        } else if provider_id_counts[call.tool_call_id.as_str()] > 1 {
            rejection.push("provider returned a duplicate tool call ID".to_owned());
        }
        if call.tool_name != super::tools::PROVIDER_CALL_NAME {
            rejection.push(format!(
                "provider called {:?}; the only registered provider tool is {:?}",
                call.tool_name,
                super::tools::PROVIDER_CALL_NAME
            ));
        }
        let fallback_tool = call
            .arguments
            .get("tool")
            .and_then(serde_json::Value::as_str)
            .filter(|tool| !tool.trim().is_empty())
            .unwrap_or(&call.tool_name)
            .to_owned();
        let (tool, arguments, activity) = match DynamicCall::from_value(call.arguments.clone()) {
            Ok(dynamic) => (
                dynamic.tool,
                dynamic.arguments,
                Some(super::tools::ToolActivity {
                    action: dynamic.action,
                    ..Default::default()
                }),
            ),
            Err(error) => {
                rejection.push(error.to_string());
                (fallback_tool, call.arguments.clone(), None)
            }
        };
        let rejection = (!rejection.is_empty()).then(|| rejection.join("; "));
        let tool_version = rejection
            .is_none()
            .then(|| state.known_tools.get(&tool).cloned())
            .flatten();
        invocations.push(ToolInvocation {
            invocation_id: ids.next(),
            provider_call_id: call.tool_call_id.clone(),
            turn_id: turn_id.clone(),
            started_at_ms,
            tool,
            arguments,
            tool_version,
            activity,
            rejection,
        });
    }
    invocations
}

fn provider_error_record(error: ModelError) -> ProviderErrorRecord {
    match error {
        ModelError::Unavailable => ProviderErrorRecord {
            stage: "model_gateway".into(),
            retryable: true,
            status_code: None,
            provider_code: Some("unavailable".into()),
            request_id: None,
            provider_input: None,
            usage: None,
            message: "Model gateway is temporarily unavailable.".into(),
        },
        ModelError::InvalidSelection => ProviderErrorRecord {
            stage: "selection".into(),
            retryable: false,
            status_code: None,
            provider_code: Some("invalid_selection".into()),
            request_id: None,
            provider_input: None,
            usage: None,
            message: "The provider selection is invalid.".into(),
        },
        ModelError::ProfileUnavailable => ProviderErrorRecord {
            stage: "profile".into(),
            retryable: false,
            status_code: None,
            provider_code: Some("profile_unavailable".into()),
            request_id: None,
            provider_input: None,
            usage: None,
            message: "The selected provider profile is unavailable or not authenticated.".into(),
        },
        ModelError::ProviderFailed(failure) => ProviderErrorRecord {
            stage: failure.stage.into(),
            retryable: failure.retryable && failure.status_code != Some(400),
            status_code: failure.status_code,
            provider_code: failure.provider_code,
            request_id: failure.request_id,
            provider_input: failure.provider_input,
            usage: failure.usage.map(|usage| Usage {
                input_tokens: usage.input_tokens,
                cached_input_tokens: usage.cached_input_tokens,
                output_tokens: usage.output_tokens,
                output_reasoning_tokens: usage.output_reasoning_tokens,
                output_text_tokens: usage.output_text_tokens,
            }),
            message: failure.message,
        },
    }
}

fn bound_tool_execution(mut execution: ToolExecution, options: &RunnerOptions) -> ToolExecution {
    let data_size = serde_json::to_vec(&execution.data)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX);
    let image_bytes = execution.images.iter().fold(0usize, |total, image| {
        total.saturating_add(image.base64.len())
    });
    if image_bytes > super::tools::MAX_IMAGE_BYTES.div_ceil(3) * 4 {
        execution.images.clear();
        execution.outcome = ToolOutcome::Failed;
        execution.data = serde_json::json!({"error": "Tool image attachments exceed the 20 MiB result limit.", "result_too_large": true});
        execution.result_schema_version = 1;
        return execution;
    }
    if data_size > options.max_tool_result_json_bytes {
        execution.images.clear();
        execution.outcome = ToolOutcome::Failed;
        execution.data = serde_json::json!({
            "error": format!(
                "Tool returned {data_size} bytes of JSON, exceeding the {} byte durable result limit. Side effects may already have occurred; the tool must place large display content in a file and return its path.",
                options.max_tool_result_json_bytes
            ),
            "result_too_large": true,
            "json_bytes": data_size,
            "limit_bytes": options.max_tool_result_json_bytes,
        });
        execution.result_schema_version = 1;
    }
    execution
}

fn estimate_input_tokens(state: &SessionState) -> Option<u64> {
    let anchor = state.anchor()?;
    let estimated_addition = state
        .generation
        .entries
        .get(anchor.entries..)?
        .iter()
        .fold(0u64, |total, entry| {
            total.saturating_add(super::context::entry_bytes(entry).div_ceil(4))
        });
    Some(anchor.input_tokens.saturating_add(estimated_addition))
}

fn respond(response: Response, result: &Result<Vec<EventEnvelope>, RunnerFailure>) {
    let _ = response.send(
        result
            .as_ref()
            .map(|_| ())
            .map_err(|error| RunnerRequestError {
                message: error.message.clone(),
            }),
    );
}

fn request_error(error: RunnerFailure) -> RunnerRequestError {
    RunnerRequestError {
        message: error.message,
    }
}

fn store_failure(stage: &str, error: StoreError) -> RunnerFailure {
    RunnerFailure {
        failure_id: "store".into(),
        stage: stage.into(),
        message: error.to_string(),
    }
}

fn infrastructure_failure(stage: &str, error: impl std::fmt::Display) -> RunnerFailure {
    RunnerFailure {
        failure_id: "infrastructure".into(),
        stage: stage.into(),
        message: error.to_string(),
    }
}

fn invariant_failure(stage: &str, error: impl std::fmt::Display) -> RunnerFailure {
    RunnerFailure {
        failure_id: "runtime_invariant".into(),
        stage: stage.into(),
        message: error.to_string(),
    }
}
